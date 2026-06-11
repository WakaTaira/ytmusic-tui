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
const TOAST_WIDTH: u16 = 40;

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

/// Box height for `message` at `width`: the wrapped line count plus the two
/// border rows, capped so a very long message cannot dominate the screen.
fn toast_height(message: &str, width: u16) -> u16 {
    const MAX_LINES: u16 = 4;
    // Inner text width excludes the two vertical border columns and one padding
    // column each side.
    let inner = width.saturating_sub(4).max(1) as usize;
    let chars = message.chars().count().max(1);
    // Ceil-div the character count by the inner width for a wrap estimate.
    let lines = chars.div_ceil(inner) as u16;
    lines.clamp(1, MAX_LINES) + 2
}

/// Draw one toast box: a bordered, surface-filled paragraph in the severity
/// color.
fn render_one(frame: &mut Frame<'_>, area: Rect, theme: &Theme, toast: &Toast) {
    let color = toast.severity.color(theme);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(color))
        .style(Style::default().bg(theme.surface));

    let paragraph = Paragraph::new(Line::from(Span::styled(
        toast.message.as_str(),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )))
    .block(block)
    .wrap(Wrap { trim: true });

    frame.render_widget(Clear, area);
    frame.render_widget(paragraph, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn t0() -> Instant {
        Instant::now()
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
        let terminal = render(&mgr, 50, 12);
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
        let terminal = render(&mgr, 50, 12);
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
        let terminal = render(&mgr, 50, 40);
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
        let terminal = render(&mgr, 50, 12);
        let buffer = terminal.backend().buffer();
        // The bottom-right corner cell should be a box border (the toast hugs
        // the corner), not blank.
        let corner = &buffer[(49, 11)];
        assert_ne!(corner.symbol(), " ", "toast did not reach the bottom-right");
    }

    #[test]
    fn error_toast_border_uses_primary_color() {
        let mut mgr = ToastManager::new();
        mgr.push("boom", Severity::Error, t0());
        let terminal = render(&mgr, 50, 12);
        let theme = Theme::default();
        let buffer = terminal.backend().buffer();
        // Find a border cell and confirm it carries the error (primary) fg.
        let mut found = false;
        for cell in buffer.content() {
            if cell.symbol() == "─" || cell.symbol() == "│" {
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
        let terminal = render(&mgr, 50, 12);
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
}
