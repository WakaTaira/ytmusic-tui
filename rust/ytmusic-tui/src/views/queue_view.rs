//! Queue view: displays the current playback queue with position highlight.
//!
//! Port of `src/ytmusic_tui/views/queue.py`. Shows all tracks with the
//! currently playing track marked `▶`. Enter jumps to that queue position
//! (Python used `set_playlist` with the selected index as `start_index`, which
//! effectively resumes the queue from that track).
//!
//! # Design choice: QueueSnapshot command
//!
//! The `QueueManager` lives in the runtime thread. Rather than share it across
//! threads, the view is populated via a dedicated
//! [`crate::app::AppCommand::FetchQueue`] /
//! [`crate::app::AppEvent::QueueSnapshot`] round-trip that sends a snapshot of
//! the queue's current state to the UI. The UI folds the snapshot into
//! [`QueueView`] via [`QueueView::set_snapshot`].
//!
//! This keeps the queue single-owned in the runtime; the snapshot hop is
//! invisible to the user (the view is read-only — the runtime re-emits
//! `QueueSnapshot` after any queue-mutating command).
//!
//! # NowPlaying → queue refresh
//!
//! When [`crate::app::AppEvent::NowPlaying`] arrives while the queue view is
//! visible, the UI fold returns [`crate::app::AppCommand::FetchQueue`] to
//! re-snapshot the queue. This keeps the `▶` marker accurate as tracks
//! advance (the same FetchQueue path used when the view is first opened).

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ytmusic_api::Track;

use super::{PageState, Theme};
use crate::formatting::format_duration;

/// A point-in-time snapshot of the queue (sent from the runtime thread).
///
/// The runtime emits this in response to
/// [`crate::app::AppCommand::FetchQueue`] and after any queue-mutating command.
/// Keeping it `Clone + PartialEq` makes it cheap to cache and compare.
#[derive(Debug, Clone, PartialEq)]
pub struct QueueSnapshot {
    /// All tracks currently in the queue.
    pub tracks: Vec<Track>,
    /// Index of the currently playing track (`None` when the queue is idle).
    pub current_index: Option<usize>,
}

/// What an Enter keypress on the queue view resolves to.
///
/// Mirrors Python's `on_data_table_row_selected`: play from the selected
/// index, queueing the rest — same as the playlist/album/history views.
#[derive(Debug, Clone, PartialEq)]
pub enum QueueAction {
    /// Jump to the selected queue position and resume playing. The runtime
    /// handles this by calling `play_playlist(tracks, start_index)`.
    JumpTo {
        tracks: Vec<Track>,
        start_index: usize,
    },
}

/// The queue view.
///
/// Holds a [`PageState`]-wrapped [`QueueSnapshot`] so the UI can display
/// "Loading…" before the snapshot arrives, and a row cursor for the user's
/// selection (distinct from the queue's own current-track marker).
#[derive(Debug, Clone)]
pub struct QueueView {
    state: PageState<QueueSnapshot>,
    cursor: usize,
}

impl Default for QueueView {
    fn default() -> Self {
        Self::new()
    }
}

impl QueueView {
    /// A fresh queue view in the loading state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: PageState::Loading,
            cursor: 0,
        }
    }

    /// The current fetch state.
    #[must_use]
    pub fn state(&self) -> &PageState<QueueSnapshot> {
        &self.state
    }

    // -- Data loading --------------------------------------------------------

    /// Apply a fresh queue snapshot (from [`crate::app::AppEvent::QueueSnapshot`]).
    pub fn set_snapshot(&mut self, snapshot: QueueSnapshot) {
        // Clamp the cursor so it stays valid if the queue shrank.
        if !snapshot.tracks.is_empty() {
            self.cursor = self.cursor.min(snapshot.tracks.len() - 1);
        } else {
            self.cursor = 0;
        }
        self.state = PageState::Loaded(snapshot);
    }

    /// Reset to loading (issued when the queue view is first opened).
    pub fn set_loading(&mut self) {
        self.state = PageState::Loading;
        self.cursor = 0;
    }

    /// Transition into the error state.
    pub fn set_error(&mut self, message: impl Into<String>) {
        self.state = PageState::Error(message.into());
    }

    // -- Navigation ----------------------------------------------------------

    fn track_count(&self) -> usize {
        self.state.loaded().map_or(0, |s| s.tracks.len())
    }

    /// Move the cursor down one row, clamping at the end.
    pub fn select_next(&mut self) {
        let last = self.track_count().saturating_sub(1);
        if self.cursor < last {
            self.cursor += 1;
        }
    }

    /// Move the cursor up one row, clamping at the top.
    pub fn select_previous(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    /// Resolve an Enter keypress into a [`QueueAction`], or `None`.
    ///
    /// Mirrors Python: `set_playlist(tracks, start_index=row_index)` — the
    /// whole queue is kept but playback resumes from the selected row.
    #[must_use]
    pub fn activate_selected(&self) -> Option<QueueAction> {
        let snap = self.state.loaded()?;
        if snap.tracks.is_empty() || self.cursor >= snap.tracks.len() {
            return None;
        }
        Some(QueueAction::JumpTo {
            tracks: snap.tracks.clone(),
            start_index: self.cursor,
        })
    }

    // -- Rendering -----------------------------------------------------------

    /// Render the queue view into `area`.
    pub fn render(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        // Status (1 row) + track list.
        let chunks = Layout::vertical([
            Constraint::Length(1), // status
            Constraint::Min(1),    // list
        ])
        .split(area);

        self.render_status(frame, chunks[0], theme);
        self.render_tracks(frame, chunks[1], theme);
    }

    fn render_status(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        let (text, is_error) = self.status_text();
        let style = if is_error {
            Style::default().fg(theme.primary)
        } else {
            Style::default()
                .fg(theme.text_muted)
                .add_modifier(Modifier::ITALIC)
        };
        frame.render_widget(Paragraph::new(Line::from(Span::styled(text, style))), area);
    }

    fn status_text(&self) -> (String, bool) {
        match &self.state {
            PageState::Loading => ("Loading queue...".to_owned(), false),
            PageState::Error(msg) => (msg.clone(), true),
            PageState::Loaded(snap) => {
                if snap.tracks.is_empty() {
                    ("Queue is empty".to_owned(), false)
                } else {
                    (format!("{} track(s) in queue", snap.tracks.len()), false)
                }
            }
        }
    }

    fn render_tracks(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        let PageState::Loaded(snap) = &self.state else {
            return;
        };
        if snap.tracks.is_empty() {
            return;
        }

        let items: Vec<ListItem> = snap
            .tracks
            .iter()
            .enumerate()
            .map(|(i, t)| track_row(i + 1, t, snap.current_index == Some(i)))
            .collect();

        let list = List::new(items)
            .block(Block::default().borders(Borders::NONE))
            .style(Style::default().fg(theme.text))
            .highlight_style(
                Style::default()
                    .fg(theme.background)
                    .bg(theme.primary)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▶ ");

        let mut list_state = ListState::default();
        if !snap.tracks.is_empty() {
            list_state.select(Some(self.cursor.min(snap.tracks.len() - 1)));
        }
        frame.render_stateful_widget(list, area, &mut list_state);
    }
}

/// Format a queue row: `[>] N. Title — Artist  Duration`.
///
/// The `is_current` flag renders a `>` marker in the index column for the
/// currently playing track (mirrors Python's `">" if track == current else " "`).
fn track_row(num: usize, track: &Track, is_current: bool) -> ListItem<'static> {
    let marker = if is_current { ">" } else { " " };
    let mut spans = vec![
        Span::styled(
            format!("{marker}{num:2}. "),
            if is_current {
                Style::default()
                    .fg(ratatui::style::Color::Reset) // accented by the highlight when selected
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().add_modifier(Modifier::DIM)
            },
        ),
        Span::raw(track.title.clone()),
    ];
    if !track.artist.is_empty() {
        spans.push(Span::raw(" — "));
        spans.push(Span::raw(track.artist.clone()));
    }
    let duration = format_duration(track.duration_seconds);
    if duration != "—" {
        spans.push(Span::raw("  "));
        spans.push(Span::raw(duration));
    }
    ListItem::new(Line::from(spans))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    // -- fixtures ------------------------------------------------------------

    fn make_track(id: &str, title: &str, artist: &str) -> Track {
        Track::new(id, title, artist, "Album", 180.0, "")
    }

    fn snapshot_with_current(current: Option<usize>) -> QueueSnapshot {
        QueueSnapshot {
            tracks: vec![
                make_track("v1", "Airbag", "Radiohead"),
                make_track("v2", "Paranoid Android", "Radiohead"),
                make_track("v3", "Subterranean", "Radiohead"),
            ],
            current_index: current,
        }
    }

    fn loaded_view() -> QueueView {
        let mut view = QueueView::new();
        view.set_snapshot(snapshot_with_current(Some(1)));
        view
    }

    fn render_to_string(view: &QueueView, w: u16, h: u16) -> String {
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = Theme::default();
        terminal
            .draw(|frame| view.render(frame, frame.area(), &theme))
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

    // -- initial state -------------------------------------------------------

    #[test]
    fn new_view_is_loading() {
        let view = QueueView::new();
        assert!(matches!(view.state(), PageState::Loading));
        let text = render_to_string(&view, 60, 8);
        assert!(text.contains("Loading queue..."), "text:\n{text}");
    }

    // -- set_snapshot --------------------------------------------------------

    #[test]
    fn set_snapshot_loads_data() {
        let view = loaded_view();
        assert!(matches!(view.state(), PageState::Loaded(_)));
        assert_eq!(view.track_count(), 3);
    }

    #[test]
    fn set_snapshot_clamps_cursor_if_queue_shrank() {
        let mut view = loaded_view();
        view.cursor = 2;
        // Shorter snapshot.
        view.set_snapshot(QueueSnapshot {
            tracks: vec![make_track("v1", "Song", "Artist")],
            current_index: Some(0),
        });
        assert_eq!(view.cursor, 0);
    }

    // -- navigation ----------------------------------------------------------

    #[test]
    fn select_next_clamps_at_end() {
        let mut view = loaded_view(); // 3 tracks
        view.select_next();
        view.select_next();
        view.select_next(); // would be 3 -> clamps at 2
        assert_eq!(view.cursor, 2);
    }

    #[test]
    fn select_previous_clamps_at_top() {
        let mut view = loaded_view();
        view.select_previous();
        assert_eq!(view.cursor, 0);
    }

    // -- activation (Enter) --------------------------------------------------

    #[test]
    fn enter_on_row_produces_jump_to_with_correct_index() {
        let mut view = loaded_view();
        view.select_next(); // cursor 1
        match view.activate_selected() {
            Some(QueueAction::JumpTo {
                tracks,
                start_index,
            }) => {
                assert_eq!(start_index, 1);
                assert_eq!(tracks.len(), 3);
                assert_eq!(tracks[start_index].video_id, "v2");
            }
            other => panic!("expected JumpTo, got {other:?}"),
        }
    }

    #[test]
    fn enter_on_empty_queue_is_none() {
        let mut view = QueueView::new();
        view.set_snapshot(QueueSnapshot {
            tracks: vec![],
            current_index: None,
        });
        assert!(view.activate_selected().is_none());
    }

    // -- error state ---------------------------------------------------------

    #[test]
    fn set_error_renders_message() {
        let mut view = QueueView::new();
        view.set_error("Runtime error");
        let text = render_to_string(&view, 60, 8);
        assert!(text.contains("Runtime error"), "text:\n{text}");
    }

    // -- rendering -----------------------------------------------------------

    #[test]
    fn loaded_render_shows_tracks_and_count() {
        let view = loaded_view();
        let text = render_to_string(&view, 70, 10);
        assert!(text.contains("Airbag"), "missing first track:\n{text}");
        assert!(
            text.contains("Paranoid Android"),
            "missing second track:\n{text}"
        );
        assert!(
            text.contains("3 track(s) in queue"),
            "missing count:\n{text}"
        );
    }

    #[test]
    fn loaded_render_shows_current_track_marker() {
        let view = loaded_view(); // current_index = 1
        let text = render_to_string(&view, 70, 10);
        // The ">" marker should appear in the rendered output for the current track.
        assert!(text.contains('>'), "missing current marker:\n{text}");
    }

    #[test]
    fn empty_queue_render_shows_empty_message() {
        let mut view = QueueView::new();
        view.set_snapshot(QueueSnapshot {
            tracks: vec![],
            current_index: None,
        });
        let text = render_to_string(&view, 60, 8);
        assert!(text.contains("Queue is empty"), "text:\n{text}");
    }
}
