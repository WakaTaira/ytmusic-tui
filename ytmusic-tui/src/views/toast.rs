//! Floating toast notifications (bottom-right overlay).
//!
//! Port of Textual's `App.notify()` toast system. The Python app surfaced every
//! transient message — playback errors, API errors, like/add confirmations, the
//! audio-quality change, MPRIS warnings — as a floating
//! [`Toast`](https://textual.textualize.io/widgets/toast/): a bordered box in
//! the bottom-right corner that stacks (newest at the bottom) and auto-dismisses
//! after a per-severity timeout. The M5a Rust port collapsed all of these onto a
//! single persistent header line with no timeout, so messages never disappeared
//! (the `b` audio-quality note in particular stuck forever). This module restores
//! the floating, self-expiring behavior.
//!
//! # Timeouts (mirroring Textual / the Python call sites)
//!
//! Textual's default notification timeout is 5 s
//! ([`textual.app.App.NOTIFICATION_TIMEOUT`]). The Python code overrode it at a
//! few call sites; we mirror those exactly:
//!
//! | Severity / source        | Timeout | Python call site                         |
//! |--------------------------|---------|------------------------------------------|
//! | Info (default)           | 5 s     | Textual `NOTIFICATION_TIMEOUT`           |
//! | Warning (default)        | 8 s     | auth / MPRIS warnings (`timeout=8`)      |
//! | Warning (filter)         | 4 s     | `filter_bar.py` "Nothing to filter"      |
//! | Error                    | 8 s     | `_PLAYBACK_ERROR_TIMEOUT = 8.0`          |
//! | Session warning          | 10 s    | `_probe_session` (`timeout=10`)          |
//!
//! Severity colors mirror Textual's toast CSS (`_toast.py`): the left border is
//! `$success` (info), `$warning` (warning), or `$error` (error). The Rust theme
//! has no semantic success/warning/error keys, so we map them onto the closest
//! palette colors: info → `secondary`, warning → `accent`, error → `primary`
//! (the same color the player bar already uses for the active/attention state).
//! TODO(post-parity): a semantic `error` palette key would let error toasts be
//! red on every theme instead of riding `primary`.
//!
//! # Determinism
//!
//! Time is injected. [`ToastManager::push`] takes the current [`Instant`] and
//! [`ToastManager::prune_at`] takes the "now" used to expire toasts, so unit
//! tests advance time explicitly rather than sleeping. The main loop calls
//! [`ToastManager::prune`] (which reads the real clock) once per ~60 ms tick.

use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use super::Theme;

/// A notification's severity, driving its color and default timeout.
///
/// Mirrors Textual's three `SeverityLevel`s (`information` / `warning` /
/// `error`). The variant order matches increasing urgency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// An informational confirmation (a like, an add-to-queue, a theme change).
    Info,
    /// A non-fatal warning (auth/MPRIS/session/filter).
    Warning,
    /// A failure the user should notice (playback / API error).
    Error,
}

/// Textual's default notification timeout (`App.NOTIFICATION_TIMEOUT = 5`).
const DEFAULT_INFO_TIMEOUT: Duration = Duration::from_secs(5);
/// The warning timeout the Python auth / MPRIS notifications used (`timeout=8`).
const DEFAULT_WARNING_TIMEOUT: Duration = Duration::from_secs(8);
/// The error timeout (`actions._PLAYBACK_ERROR_TIMEOUT = 8.0`).
const DEFAULT_ERROR_TIMEOUT: Duration = Duration::from_secs(8);

impl Severity {
    /// The default on-screen lifetime for this severity, matching the Python
    /// call sites (see the module-level table). Call sites that overrode the
    /// timeout (the 4 s filter warning, the 10 s session warning) push with an
    /// explicit duration via [`ToastManager::push_with_timeout`].
    #[must_use]
    pub fn default_timeout(self) -> Duration {
        match self {
            Severity::Info => DEFAULT_INFO_TIMEOUT,
            Severity::Warning => DEFAULT_WARNING_TIMEOUT,
            Severity::Error => DEFAULT_ERROR_TIMEOUT,
        }
    }

    /// The bold title line shown at the top of the toast (textual's `.toast--title`).
    fn title(self) -> &'static str {
        match self {
            Severity::Info => "Information",
            Severity::Warning => "Warning",
            Severity::Error => "Error",
        }
    }

    /// The theme color for this severity's border and title.
    ///
    /// The Rust palette has no semantic success/warning/error keys, so the
    /// Textual `$success`/`$warning`/`$error` border colors map onto the
    /// closest palette entries: info → `secondary` (the cyan-ish accent),
    /// warning → `accent`, error → `primary`.
    fn color(self, theme: &Theme) -> ratatui::style::Color {
        match self {
            Severity::Info => theme.secondary,
            Severity::Warning => theme.accent,
            Severity::Error => theme.primary,
        }
    }
}

/// A single live toast: its text, severity, and the instant it expires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    /// The message body.
    pub message: String,
    /// The severity (color + default timeout).
    pub severity: Severity,
    /// The instant at or after which [`ToastManager::prune_at`] removes it.
    pub expires_at: Instant,
}

/// The maximum number of toasts rendered at once. Older toasts beyond this count
/// are still tracked (and still expire) but only the newest [`MAX_VISIBLE`] draw,
/// so a burst of messages does not fill the screen. Textual capped its toast
/// rack similarly via the screen height; four is a comfortable visible stack.
const MAX_VISIBLE: usize = 4;

/// Outer width of a toast box in columns, clamped to the available area.
const TOAST_WIDTH: u16 = 60;

/// Holds the live toasts and expires them over time.
///
/// The newest toast is last in the vector (append order), so rendering draws the
/// stack with the newest at the bottom — matching Textual, where new toasts push
/// up from the bottom-right corner.
#[derive(Debug, Clone, Default)]
pub struct ToastManager {
    toasts: Vec<Toast>,
}

impl ToastManager {
    /// An empty manager.
    #[must_use]
    pub fn new() -> Self {
        Self { toasts: Vec::new() }
    }

    /// Push a toast with its severity's default timeout, computed from `now`.
    pub fn push(&mut self, message: impl Into<String>, severity: Severity, now: Instant) {
        self.push_with_timeout(message, severity, severity.default_timeout(), now);
    }

    /// Push a toast with an explicit `timeout` (for the call sites that overrode
    /// the default — the 4 s filter warning and the 10 s session warning).
    pub fn push_with_timeout(
        &mut self,
        message: impl Into<String>,
        severity: Severity,
        timeout: Duration,
        now: Instant,
    ) {
        // Hard cap on the backing store: MAX_VISIBLE only limits rendering.
        // An error storm (e.g. one ApiError per failed request during an
        // outage) must not grow the Vec unboundedly — drop the oldest.
        const MAX_STORED: usize = 32;
        if self.toasts.len() >= MAX_STORED {
            self.toasts.remove(0);
        }
        self.toasts.push(Toast {
            message: message.into(),
            severity,
            expires_at: now + timeout,
        });
    }

    /// Remove every toast whose `expires_at` is at or before `now`.
    ///
    /// Split from [`ToastManager::prune`] so tests drive expiry with an injected
    /// clock instead of sleeping.
    pub fn prune_at(&mut self, now: Instant) {
        self.toasts.retain(|t| t.expires_at > now);
    }

    /// Remove expired toasts using the real clock. Called once per UI tick.
    pub fn prune(&mut self) {
        self.prune_at(Instant::now());
    }

    /// The live toasts (oldest first), for tests.
    #[must_use]
    pub fn toasts(&self) -> &[Toast] {
        &self.toasts
    }

    /// Whether any toast is currently live (skips the render pass when empty).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.toasts.is_empty()
    }

    /// Render the toast stack in the bottom-right of `area`.
    ///
    /// The newest [`MAX_VISIBLE`] toasts are drawn as bordered boxes anchored to
    /// the bottom-right corner, newest at the bottom (so a new toast appears
    /// closest to the player bar and older ones rise above it). Each box is
    /// [`Clear`]ed first so it fully occludes the view underneath.
    pub fn render(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        if self.toasts.is_empty() || area.width < 6 || area.height < 3 {
            return;
        }

        let width = TOAST_WIDTH.min(area.width);
        // Draw only the newest MAX_VISIBLE toasts; the newest is the last
        // element, so take the tail and render bottom-up.
        let visible: Vec<&Toast> = self.toasts.iter().rev().take(MAX_VISIBLE).collect();

        // The newest (index 0 in `visible`) sits at the bottom; each older toast
        // stacks one box-height above it.
        let mut bottom = area.y + area.height;
        for toast in &visible {
            let height = toast_height(&toast.message, width);
            if height > bottom.saturating_sub(area.y) {
                // No vertical room left for another box.
                break;
            }
            let top = bottom - height;
            let toast_area = Rect {
                x: area.x + area.width - width,
                y: top,
                width,
                height,
            };
            render_one(frame, toast_area, theme, toast);
            bottom = top;
        }
    }
}

/// Box height for `message` at `width`.
///
/// Layout: 1 top-pad + 1 title + 1 blank + msg_lines + 1 bottom-pad.
/// The left-border-only style (textual `border-left: outer`) adds no extra
/// rows. Message lines are capped so a burst of text cannot dominate the screen.
fn toast_height(message: &str, width: u16) -> u16 {
    const MAX_MSG_LINES: u16 = 4;
    // Inner text width: subtract 1 (left border) + 1 (left pad) + 1 (right pad).
    let inner = width.saturating_sub(3).max(1) as usize;
    // Measure DISPLAY width, not chars: ratatui wraps by display width, and a
    // CJK/emoji glyph occupies two columns — counting chars underestimates the
    // height and silently clips the bottom of e.g. Japanese error messages.
    let display_width = UnicodeWidthStr::width(message).max(1);
    let msg_lines = (display_width.div_ceil(inner) as u16).clamp(1, MAX_MSG_LINES);
    // 1 top-pad + 1 title + 1 blank + msg_lines + 1 bottom-pad
    msg_lines + 4
}

/// Draw one toast box: left-border-only with padding, title + message body.
///
/// Mirrors Textual's Toast CSS: `border-left: outer $severity`, `padding: 1 1`,
/// title in bold above the message body.
fn render_one(frame: &mut Frame<'_>, area: Rect, theme: &Theme, toast: &Toast) {
    let color = toast.severity.color(theme);
    let title_text = toast.severity.title();

    // Left border only (textual `border-left: outer`) + surface background.
    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(color))
        .style(Style::default().bg(theme.surface));

    // Inner area after the left border.
    let inner = block.inner(area);

    frame.render_widget(Clear, area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    // Apply 1-cell padding on all sides within the inner area.
    let padded = Rect {
        x: inner.x + 1,
        y: inner.y + 1,
        width: inner.width.saturating_sub(2),
        height: inner.height.saturating_sub(2),
    };

    if padded.height == 0 || padded.width == 0 {
        return;
    }

    // Title line (bold, severity color).
    let title_area = Rect {
        height: 1,
        ..padded
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            title_text,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ))),
        title_area,
    );

    // Message body below the title (if room).
    if padded.height > 2 {
        let msg_area = Rect {
            y: padded.y + 2,
            height: padded.height.saturating_sub(2),
            ..padded
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                toast.message.as_str(),
                Style::default().fg(theme.text),
            )))
            .wrap(Wrap { trim: true }),
            msg_area,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn t0() -> Instant {
        Instant::now()
    }

    // -- height math --------------------------------------------------------

    #[test]
    fn toast_height_counts_display_width_for_cjk() {
        // 28 CJK chars render as 56 columns. At box width 30 (inner 27), a
        // char-count estimate gives ceil(28/27) = 2 lines, but the real wrap
        // needs ceil(56/27) = 3. Regression for the clipped-Japanese-toast bug.
        let msg = "あ".repeat(28);
        assert_eq!(toast_height(&msg, 30), 3 + 4);
    }

    // -- timeouts ----------------------------------------------------------

    #[test]
    fn default_timeouts_match_python_call_sites() {
        assert_eq!(Severity::Info.default_timeout(), Duration::from_secs(5));
        assert_eq!(Severity::Warning.default_timeout(), Duration::from_secs(8));
        assert_eq!(Severity::Error.default_timeout(), Duration::from_secs(8));
    }

    // -- push / prune (injected clock) -------------------------------------

    #[test]
    fn push_adds_a_live_toast() {
        let mut mgr = ToastManager::new();
        mgr.push("hello", Severity::Info, t0());
        assert_eq!(mgr.toasts().len(), 1);
        assert_eq!(mgr.toasts()[0].message, "hello");
        assert_eq!(mgr.toasts()[0].severity, Severity::Info);
    }

    #[test]
    fn prune_removes_expired_keeps_live() {
        let now = t0();
        let mut mgr = ToastManager::new();
        // Info expires after 5 s; warning after 8 s.
        mgr.push("info", Severity::Info, now);
        mgr.push("warn", Severity::Warning, now);

        // At +6 s the info toast is gone, the warning remains.
        mgr.prune_at(now + Duration::from_secs(6));
        assert_eq!(mgr.toasts().len(), 1);
        assert_eq!(mgr.toasts()[0].message, "warn");

        // At +9 s both are gone.
        mgr.prune_at(now + Duration::from_secs(9));
        assert!(mgr.is_empty());
    }

    #[test]
    fn prune_exactly_at_expiry_removes_the_toast() {
        let now = t0();
        let mut mgr = ToastManager::new();
        mgr.push("info", Severity::Info, now);
        // Exactly at expiry (expires_at == now) the toast is removed (retain
        // keeps strictly-greater timestamps).
        mgr.prune_at(now + Duration::from_secs(5));
        assert!(mgr.is_empty());
    }

    #[test]
    fn push_with_timeout_overrides_default() {
        let now = t0();
        let mut mgr = ToastManager::new();
        // The filter warning uses a 4 s override (shorter than the 8 s default).
        mgr.push_with_timeout("filter", Severity::Warning, Duration::from_secs(4), now);
        mgr.prune_at(now + Duration::from_secs(5));
        assert!(mgr.is_empty());
    }

    #[test]
    fn newest_toast_is_last() {
        let now = t0();
        let mut mgr = ToastManager::new();
        mgr.push("first", Severity::Info, now);
        mgr.push("second", Severity::Info, now);
        assert_eq!(mgr.toasts().first().unwrap().message, "first");
        assert_eq!(mgr.toasts().last().unwrap().message, "second");
    }

    // -- rendering (TestBackend) -------------------------------------------

    fn render(mgr: &ToastManager, w: u16, h: u16) -> Terminal<TestBackend> {
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = Theme::default();
        terminal
            .draw(|frame| mgr.render(frame, frame.area(), &theme))
            .unwrap();
        terminal
    }

    fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
        let buffer = terminal.backend().buffer();
        let width = buffer.area().width as usize;
        let mut out = String::new();
        for (i, cell) in buffer.content().iter().enumerate() {
            out.push_str(cell.symbol());
            if (i + 1) % width == 0 {
                out.push('\n');
            }
        }
        out
    }

    #[test]
    fn render_draws_message_text() {
        let mut mgr = ToastManager::new();
        mgr.push("Liked song", Severity::Info, t0());
        // Terminal must be tall enough: toast_height for a short message = 5 rows.
        let terminal = render(&mgr, 70, 20);
        let text = buffer_text(&terminal);
        assert!(text.contains("Liked song"), "missing toast text:\n{text}");
    }

    #[test]
    fn render_empty_draws_nothing() {
        let mgr = ToastManager::new();
        let terminal = render(&mgr, 50, 12);
        let text = buffer_text(&terminal);
        // No border glyphs when there is nothing to show.
        assert!(!text.contains('│'), "empty manager drew a box:\n{text}");
    }

    #[test]
    fn render_stacks_multiple_toasts() {
        let mut mgr = ToastManager::new();
        mgr.push("Alpha message", Severity::Info, t0());
        mgr.push("Bravo message", Severity::Error, t0());
        // Each toast now needs 5+ rows (padding + title + gap + message + padding).
        let terminal = render(&mgr, 70, 30);
        let text = buffer_text(&terminal);
        assert!(text.contains("Alpha message"), "missing first:\n{text}");
        assert!(text.contains("Bravo message"), "missing second:\n{text}");
    }

    #[test]
    fn render_caps_visible_stack_at_max() {
        let mut mgr = ToastManager::new();
        for i in 0..8 {
            mgr.push(format!("Toast number {i}"), Severity::Info, t0());
        }
        // A tall enough terminal would fit all 8, but only MAX_VISIBLE draw.
        // Each toast is now ~5 rows tall; 4 × 5 = 20 rows minimum.
        let terminal = render(&mgr, 70, 80);
        let text = buffer_text(&terminal);
        // The newest four (4..=7) are visible; the oldest (0) is not.
        assert!(text.contains("Toast number 7"), "newest missing:\n{text}");
        assert!(
            !text.contains("Toast number 0"),
            "oldest should be hidden:\n{text}"
        );
    }

    #[test]
    fn render_anchors_to_bottom_right() {
        let mut mgr = ToastManager::new();
        mgr.push("Edge", Severity::Info, t0());
        // With left-border-only, the right edge has no border char, but the
        // left edge of the toast (width 60, terminal 70 wide → left at col 10)
        // carries the │ border character.
        let terminal = render(&mgr, 70, 20);
        let buffer = terminal.backend().buffer();
        let mut found_border = false;
        for row in 0..buffer.area().height {
            let cell = &buffer[(10, row)]; // TOAST_WIDTH=60 in a 70-col terminal → left at col 10
            if cell.symbol() == "│" {
                found_border = true;
                break;
            }
        }
        assert!(
            found_border,
            "toast left border not rendered at expected column"
        );
    }

    #[test]
    fn error_toast_border_uses_primary_color() {
        let mut mgr = ToastManager::new();
        mgr.push("boom", Severity::Error, t0());
        // Use a terminal tall enough for the new layout (5 rows per toast).
        let terminal = render(&mgr, 70, 20);
        let theme = Theme::default();
        let buffer = terminal.backend().buffer();
        // Find the left-border cell (│) and confirm it carries the error (primary) fg.
        let mut found = false;
        for cell in buffer.content() {
            if cell.symbol() == "│" {
                assert_eq!(cell.style().fg, Some(theme.primary));
                found = true;
                break;
            }
        }
        assert!(found, "no border cell rendered for the error toast");
    }

    #[test]
    fn toast_carries_surface_background() {
        let mut mgr = ToastManager::new();
        mgr.push("bg check", Severity::Info, t0());
        let terminal = render(&mgr, 70, 20);
        let theme = Theme::default();
        let buffer = terminal.backend().buffer();
        // The interior of the toast (where the text sits) carries the surface bg.
        let mut found_surface = false;
        for cell in buffer.content() {
            if cell.style().bg == Some(theme.surface) {
                found_surface = true;
                break;
            }
        }
        assert!(found_surface, "toast interior missing surface background");
    }

    #[test]
    fn render_shows_severity_title() {
        let mut mgr = ToastManager::new();
        mgr.push("some message", Severity::Info, t0());
        let terminal = render(&mgr, 70, 20);
        let text = buffer_text(&terminal);
        assert!(
            text.contains("Information"),
            "severity title missing:\n{text}"
        );
        assert!(
            text.contains("some message"),
            "message body missing:\n{text}"
        );
    }
}
