//! Player bar view (now playing, progress, volume).
//!
//! Port of `src/ytmusic_tui/views/player.py`. The bottom bar mirrors
//! spotify_player's playback window:
//!
//! ```text
//! Row 1:  ▶  Track - Artist                         S R:all   Vol: 80
//! Row 2:     Album Name
//! Row 3:  ━━━━━━╶────────────                            1:23 / 3:45
//! ```
//!
//! # State source
//!
//! Textual polled the player at 1 Hz. Here the bar is a pure value
//! ([`PlayerBarState`]) folded forward from [`crate::app::AppEvent`]s by the
//! main loop: `PlayerProgress` / `PlayerDuration` update the counters,
//! `PlayerStarted` marks a track active, `TrackEnded` clears it. [`PlayerBar`]
//! renders a borrowed `&PlayerBarState`; it holds no state of its own.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::Theme;
use crate::formatting::format_duration;

/// Repeat mode for the bar's mode indicator.
///
/// Port of `queue::RepeatMode` for display purposes (the queue module's own
/// enum is the source of truth once the queue is wired into the app in M5b;
/// this mirror keeps the bar self-contained until then). Variant order matches
/// Python's `RepeatMode` (OFF / ALL / ONE).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RepeatMode {
    /// No repeat.
    #[default]
    Off,
    /// Repeat the whole queue.
    All,
    /// Repeat the current track.
    One,
}

/// Immutable snapshot driving the player bar's render.
///
/// Equivalent to the data Python's `PlayerBar.update_state` consumed (a
/// `PlayerState` plus the queue-derived `album` / `shuffle` / `repeat_mode`).
/// The main loop owns one of these and mutates it as events arrive; the bar
/// borrows it read-only.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerBarState {
    /// Whether mpv is actively playing (drives the ▶ / ⏸ icon).
    pub is_playing: bool,
    /// Volume 0–100.
    pub volume: i64,
    /// Whether audio is muted (shows `Vol: MUTE`).
    pub is_muted: bool,
    /// Current position in seconds.
    pub position: f64,
    /// Track duration in seconds (0.0 until mpv resolves it).
    pub duration: f64,
    /// Current track title (empty when nothing is loaded).
    pub title: String,
    /// Current track artist.
    pub artist: String,
    /// Current track album (dimmed second row).
    pub album: String,
    /// Whether a track is loaded — `true` between `PlayerStarted` and
    /// `TrackEnded`. Mirrors Python keying the duration display on
    /// `state.video_id` being set: while a track is active a resolving duration
    /// shows `0:00` rather than a dash.
    pub has_track: bool,
    /// Whether shuffle is enabled.
    pub shuffle: bool,
    /// The queue's repeat mode.
    pub repeat: RepeatMode,
}

impl Default for PlayerBarState {
    /// The idle bar: nothing playing, volume 80 (the config default), no track.
    fn default() -> Self {
        Self {
            is_playing: false,
            volume: 80,
            is_muted: false,
            position: 0.0,
            duration: 0.0,
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            has_track: false,
            shuffle: false,
            repeat: RepeatMode::Off,
        }
    }
}

impl PlayerBarState {
    /// Playback progress as a 0.0–1.0 ratio (0.0 when duration is non-positive).
    ///
    /// Same math as `PlayerState::progress` in the player module.
    #[must_use]
    pub fn progress(&self) -> f64 {
        if self.duration <= 0.0 {
            0.0
        } else {
            (self.position / self.duration).clamp(0.0, 1.0)
        }
    }

    /// Fold a progress tick in (`AppEvent::PlayerProgress`).
    pub fn on_progress(&mut self, seconds: f64) {
        // f64::max returns the other operand for NaN, so this also scrubs
        // NaN reports from mpv state transitions (NaN.max(0.0) == 0.0).
        self.position = seconds.max(0.0);
    }

    /// Fold a duration observation in (`AppEvent::PlayerDuration`).
    pub fn on_duration(&mut self, seconds: f64) {
        // Same NaN/negative scrub as on_progress.
        self.duration = seconds.max(0.0);
    }

    /// Mark a track active and playing (`AppEvent::PlayerStarted`).
    ///
    /// Resets the position so the bar does not briefly show the previous
    /// track's elapsed time before the first progress tick lands.
    pub fn on_started(&mut self) {
        self.has_track = true;
        self.is_playing = true;
        self.position = 0.0;
    }

    /// Clear the now-playing state when the track ends (`AppEvent::TrackEnded`).
    ///
    /// Until the queue auto-advance is wired (M5b) this returns the bar to idle.
    pub fn on_track_ended(&mut self) {
        self.has_track = false;
        self.is_playing = false;
        self.position = 0.0;
        self.duration = 0.0;
        self.title.clear();
        self.artist.clear();
        self.album.clear();
    }
}

// ---------------------------------------------------------------------------
// Display string builders (pure — unit-tested without a terminal)
// ---------------------------------------------------------------------------

/// Play / pause icon (Python: `"⏸" if is_playing else "▶"`).
const ICON_PLAYING: &str = "⏸";
const ICON_PAUSED: &str = "▶";

/// The shuffle indicator letter; styling (bold/dim) is applied by the renderer.
const SHUFFLE_LABEL: &str = "S";

/// Placeholder text when no track is loaded.
///
/// Python uses the literal "No track" for the track-info line; kept verbatim
/// for parity (the M5a directive's "Nothing playing" wording refers to the same
/// idle state).
const NO_TRACK: &str = "No track";

/// Format the position counter (Python `_format_time`): always `0:00` for zero,
/// otherwise `format_duration`. Unlike a bare duration, the *position* starts
/// genuinely at zero, so it never shows a dash.
fn format_position(seconds: f64) -> String {
    if seconds.max(0.0) as i64 == 0 {
        "0:00".to_owned()
    } else {
        format_duration(seconds)
    }
}

/// The `title - artist` line, or [`NO_TRACK`] when there is no title.
///
/// Mirrors Python: `f"{title} - {artist}"` when an artist is present, else just
/// the title.
fn track_info_text(state: &PlayerBarState) -> String {
    if state.title.is_empty() {
        NO_TRACK.to_owned()
    } else if state.artist.is_empty() {
        state.title.clone()
    } else {
        format!("{} - {}", state.title, state.artist)
    }
}

/// The volume text: `Vol: MUTE` when muted, else `Vol: N`.
fn volume_text(state: &PlayerBarState) -> String {
    if state.is_muted {
        "Vol: MUTE".to_owned()
    } else {
        format!("Vol: {}", state.volume)
    }
}

/// The `position / duration` time display.
///
/// While a track is active (`has_track`) the duration uses [`format_position`]
/// so a still-resolving duration shows `0:00` rather than a dash (Python keyed
/// this on `state.video_id`). With no track it uses [`format_duration`], whose
/// dash signals "no known duration".
fn time_text(state: &PlayerBarState) -> String {
    let pos = format_position(state.position);
    let dur = if state.has_track {
        format_position(state.duration)
    } else {
        format_duration(state.duration)
    };
    format!("{pos} / {dur}")
}

/// Build the text progress bar glyph string for a given inner width.
///
/// Verbatim port of Python's bar math:
/// `"━" * filled + "╶" + "─" * max(0, empty - 1)` where
/// `filled = int(bar_width * progress)` and `empty = bar_width - filled`.
/// `bar_width` is floored at 10 (Python `max(10, term_width)`).
fn progress_bar_text(progress: f64, available_width: u16) -> String {
    const MIN_BAR: u16 = 10;
    let bar_width = available_width.max(MIN_BAR) as usize;
    let filled = ((bar_width as f64) * progress) as usize;
    let filled = filled.min(bar_width);
    let empty = bar_width - filled;
    let trailing = empty.saturating_sub(1);
    let mut bar = String::with_capacity(bar_width * 3);
    for _ in 0..filled {
        bar.push('━');
    }
    bar.push('╶');
    for _ in 0..trailing {
        bar.push('─');
    }
    bar
}

// ---------------------------------------------------------------------------
// PlayerBar widget
// ---------------------------------------------------------------------------

/// The bottom now-playing bar. Stateless: it renders a borrowed
/// [`PlayerBarState`].
#[derive(Debug, Clone, Copy, Default)]
pub struct PlayerBar;

/// Fixed bar height in terminal rows (three content rows; Python used
/// `height: 4` to include its border, the content is three lines).
pub const PLAYER_BAR_HEIGHT: u16 = 3;

/// Width reserved on row 3 for the ` pos / dur ` time display, subtracted from
/// the bar area to size the progress glyphs (Python subtracted 18 from the
/// terminal width).
const TIME_COLUMN_WIDTH: u16 = 18;

impl PlayerBar {
    /// Render the three-row bar into `area` using `state` and `theme`.
    pub fn render(self, frame: &mut Frame<'_>, area: Rect, state: &PlayerBarState, theme: &Theme) {
        let rows = Layout::vertical([
            Constraint::Length(1), // track info + modes + volume
            Constraint::Length(1), // album
            Constraint::Length(1), // progress + time
        ])
        .split(area);

        self.render_top_row(frame, rows[0], state, theme);
        self.render_album_row(frame, rows[1], state, theme);
        self.render_progress_row(frame, rows[2], state, theme);
    }

    /// Row 1: icon, `title - artist`, shuffle/repeat modes, volume.
    fn render_top_row(
        self,
        frame: &mut Frame<'_>,
        area: Rect,
        state: &PlayerBarState,
        theme: &Theme,
    ) {
        let icon = if state.is_playing {
            ICON_PLAYING
        } else {
            ICON_PAUSED
        };

        // Left: icon + track info. Right: modes + volume. Split so the right
        // cluster stays flush-right like the Textual fixed-width columns.
        let cols = Layout::horizontal([Constraint::Min(10), Constraint::Length(24)]).split(area);

        let left = Line::from(vec![
            Span::styled(format!("{icon} "), Style::default().fg(theme.primary)),
            Span::styled(track_info_text(state), Style::default().fg(theme.text)),
        ]);
        frame.render_widget(Paragraph::new(left), cols[0]);

        let mut right_spans = mode_spans(state, theme);
        right_spans.push(Span::raw("  "));
        right_spans.push(Span::styled(
            volume_text(state),
            Style::default().fg(theme.secondary),
        ));
        frame.render_widget(
            Paragraph::new(Line::from(right_spans)).right_aligned(),
            cols[1],
        );
    }

    /// Row 2: album name, dimmed (Python `#player-album { color: $text-muted }`).
    fn render_album_row(
        self,
        frame: &mut Frame<'_>,
        area: Rect,
        state: &PlayerBarState,
        theme: &Theme,
    ) {
        // A four-cell indent mirrors Python's `#player-album-spacer { width: 4 }`.
        let line = Line::from(vec![
            Span::raw("    "),
            Span::styled(state.album.clone(), Style::default().fg(theme.text_muted)),
        ]);
        frame.render_widget(Paragraph::new(line), area);
    }

    /// Row 3: the text progress bar and the `pos / dur` time display.
    fn render_progress_row(
        self,
        frame: &mut Frame<'_>,
        area: Rect,
        state: &PlayerBarState,
        theme: &Theme,
    ) {
        let time = time_text(state);
        let time_width = (time.chars().count() as u16).max(TIME_COLUMN_WIDTH);
        let cols =
            Layout::horizontal([Constraint::Min(10), Constraint::Length(time_width)]).split(area);

        let bar = progress_bar_text(state.progress(), cols[0].width);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                bar,
                Style::default().fg(theme.primary),
            ))),
            cols[0],
        );
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                time,
                Style::default().fg(theme.text_muted),
            )))
            .right_aligned(),
            cols[1],
        );
    }
}

/// Build the shuffle + repeat indicator spans.
///
/// Python rendered `S` (bold-green when on, dim when off) and `R` / `R:all` /
/// `R:one` likewise. Here "on" uses the theme accent + bold and "off" uses a
/// dimmed muted color, preserving the always-visible-but-dimmed semantics.
fn mode_spans(state: &PlayerBarState, theme: &Theme) -> Vec<Span<'static>> {
    let on = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let off = Style::default()
        .fg(theme.text_muted)
        .add_modifier(Modifier::DIM);

    let shuffle = Span::styled(SHUFFLE_LABEL, if state.shuffle { on } else { off });

    let (repeat_label, repeat_on): (&str, bool) = match state.repeat {
        RepeatMode::All => ("R:all", true),
        RepeatMode::One => ("R:one", true),
        RepeatMode::Off => ("R", false),
    };
    let repeat = Span::styled(repeat_label, if repeat_on { on } else { off });

    vec![shuffle, Span::raw(" "), repeat]
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    // -- pure formatting ---------------------------------------------------

    #[test]
    fn position_zero_is_double_zero() {
        assert_eq!(format_position(0.0), "0:00");
    }

    #[test]
    fn position_nonzero_delegates_to_duration_format() {
        assert_eq!(format_position(83.0), "1:23");
    }

    #[test]
    fn track_info_with_artist_uses_dash() {
        let state = PlayerBarState {
            title: "Song".to_owned(),
            artist: "Band".to_owned(),
            ..Default::default()
        };
        assert_eq!(track_info_text(&state), "Song - Band");
    }

    #[test]
    fn track_info_without_artist_is_title_only() {
        let state = PlayerBarState {
            title: "Song".to_owned(),
            ..Default::default()
        };
        assert_eq!(track_info_text(&state), "Song");
    }

    #[test]
    fn track_info_without_title_is_no_track() {
        assert_eq!(track_info_text(&PlayerBarState::default()), "No track");
    }

    #[test]
    fn volume_text_normal_and_muted() {
        let mut state = PlayerBarState {
            volume: 65,
            ..Default::default()
        };
        assert_eq!(volume_text(&state), "Vol: 65");
        state.is_muted = true;
        assert_eq!(volume_text(&state), "Vol: MUTE");
    }

    #[test]
    fn time_text_active_track_shows_zero_zero_for_resolving_duration() {
        // has_track but duration still 0 -> "0:00 / 0:00", not a dash.
        let state = PlayerBarState {
            has_track: true,
            position: 0.0,
            duration: 0.0,
            ..Default::default()
        };
        assert_eq!(time_text(&state), "0:00 / 0:00");
    }

    #[test]
    fn time_text_no_track_duration_is_dash() {
        let state = PlayerBarState::default();
        assert_eq!(time_text(&state), "0:00 / —");
    }

    #[test]
    fn time_text_playing_shows_pos_and_dur() {
        let state = PlayerBarState {
            has_track: true,
            position: 83.0,
            duration: 225.0,
            ..Default::default()
        };
        assert_eq!(time_text(&state), "1:23 / 3:45");
    }

    #[test]
    fn progress_ratio_clamps_and_handles_zero_duration() {
        let mut state = PlayerBarState::default();
        assert_eq!(state.progress(), 0.0); // zero duration
        state.duration = 100.0;
        state.position = 25.0;
        assert!((state.progress() - 0.25).abs() < f64::EPSILON);
        state.position = 999.0; // beyond end clamps to 1.0
        assert_eq!(state.progress(), 1.0);
    }

    // -- progress bar glyphs (Python's exact math) -------------------------

    #[test]
    fn progress_bar_empty_has_caret_and_dashes() {
        // 0% over width 12: no filled, a caret, then 11 dashes.
        let bar = progress_bar_text(0.0, 12);
        assert!(bar.starts_with('╶'), "bar: {bar}");
        assert_eq!(bar.chars().filter(|&c| c == '━').count(), 0);
        assert!(bar.contains('─'));
    }

    #[test]
    fn progress_bar_half_filled() {
        // 50% over width 20 -> 10 filled glyphs.
        let bar = progress_bar_text(0.5, 20);
        assert_eq!(bar.chars().filter(|&c| c == '━').count(), 10);
        assert!(bar.contains('╶'));
    }

    #[test]
    fn progress_bar_floors_width_at_ten() {
        // available width below the minimum still produces a 10-wide bar
        // (filled + caret + trailing == 10 cells for 0%).
        let bar = progress_bar_text(0.0, 3);
        let cells = bar.chars().count();
        assert_eq!(cells, 10, "bar should floor at 10 cells: {bar} ({cells})");
    }

    // -- event folding -----------------------------------------------------

    #[test]
    fn on_started_marks_active_and_resets_position() {
        let mut state = PlayerBarState {
            position: 42.0,
            ..Default::default()
        };
        state.on_started();
        assert!(state.has_track);
        assert!(state.is_playing);
        assert_eq!(state.position, 0.0);
    }

    #[test]
    fn on_progress_and_duration_update_counters() {
        let mut state = PlayerBarState::default();
        state.on_duration(200.0);
        state.on_progress(50.0);
        assert_eq!(state.duration, 200.0);
        assert_eq!(state.position, 50.0);
    }

    #[test]
    fn on_track_ended_returns_to_idle() {
        let mut state = PlayerBarState {
            has_track: true,
            is_playing: true,
            position: 100.0,
            duration: 200.0,
            title: "Song".to_owned(),
            artist: "Band".to_owned(),
            album: "Album".to_owned(),
            ..Default::default()
        };
        state.on_track_ended();
        assert!(!state.has_track);
        assert!(!state.is_playing);
        assert_eq!(state.position, 0.0);
        assert!(state.title.is_empty());
        assert!(state.album.is_empty());
    }

    // -- rendering (TestBackend) -------------------------------------------

    fn render_bar(state: &PlayerBarState, w: u16) -> String {
        let backend = TestBackend::new(w, PLAYER_BAR_HEIGHT);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = Theme::default();
        terminal
            .draw(|frame| PlayerBar.render(frame, frame.area(), state, &theme))
            .unwrap();
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
    fn render_playing_shows_title_artist_album_volume() {
        let state = PlayerBarState {
            is_playing: true,
            has_track: true,
            volume: 80,
            position: 83.0,
            duration: 225.0,
            title: "Around the World".to_owned(),
            artist: "Daft Punk".to_owned(),
            album: "Homework".to_owned(),
            ..Default::default()
        };
        let text = render_bar(&state, 70);
        assert!(text.contains("Around the World"), "missing title:\n{text}");
        assert!(text.contains("Daft Punk"), "missing artist:\n{text}");
        assert!(text.contains("Homework"), "missing album:\n{text}");
        assert!(text.contains("Vol: 80"), "missing volume:\n{text}");
        assert!(text.contains("1:23 / 3:45"), "missing time:\n{text}");
        // Pause icon shows while playing.
        assert!(
            text.contains('⏸'),
            "missing pause icon while playing:\n{text}"
        );
    }

    #[test]
    fn render_idle_shows_no_track_and_play_icon() {
        let state = PlayerBarState::default();
        let text = render_bar(&state, 60);
        assert!(text.contains("No track"), "missing idle label:\n{text}");
        assert!(text.contains('▶'), "missing play icon while idle:\n{text}");
        assert!(text.contains("Vol: 80"), "missing default volume:\n{text}");
    }

    #[test]
    fn render_shows_progress_glyphs() {
        let state = PlayerBarState {
            has_track: true,
            position: 50.0,
            duration: 100.0,
            ..Default::default()
        };
        let text = render_bar(&state, 60);
        assert!(text.contains('━'), "missing filled progress glyph:\n{text}");
        assert!(text.contains('╶'), "missing progress caret:\n{text}");
    }

    #[test]
    fn render_shows_shuffle_and_repeat_labels() {
        let state = PlayerBarState {
            shuffle: true,
            repeat: RepeatMode::All,
            ..Default::default()
        };
        let text = render_bar(&state, 70);
        assert!(text.contains('S'), "missing shuffle label:\n{text}");
        assert!(text.contains("R:all"), "missing repeat-all label:\n{text}");
    }

    #[test]
    fn render_muted_shows_mute_label() {
        let state = PlayerBarState {
            is_muted: true,
            ..Default::default()
        };
        let text = render_bar(&state, 60);
        assert!(text.contains("Vol: MUTE"), "missing mute label:\n{text}");
    }
}
