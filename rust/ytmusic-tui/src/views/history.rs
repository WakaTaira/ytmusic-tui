//! Recently-played (history) view.
//!
//! Port of `src/ytmusic_tui/views/history.py`. Flat track list from
//! `get_history()`, newest first. Enter plays from the selected position,
//! queueing the rest (same "album-style" queue semantics as the album and
//! playlist views — Python's `set_playlist(self._tracks, start_index=row_index)`).
//!
//! # Fetch flow vs Python
//!
//! Python's `HistoryView.refresh_history()` ran a Textual worker. Here the
//! view is a pure value; the main loop issues
//! [`crate::app::AppCommand::FetchHistory`] when switching to this view and
//! folds [`crate::app::AppEvent::HistoryLoaded`] back. The history is
//! re-fetched on every view-switch (Python: re-fetched on `on_show`).

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ytmusic_api::Track;

use super::{PageState, Theme};
use crate::formatting::format_duration;

/// What an Enter keypress on the history view resolves to.
///
/// Mirrors Python's `on_data_table_row_selected`: queue the history slice from
/// the cursor onward and play (same as album / playlist track-list semantics).
#[derive(Debug, Clone, PartialEq)]
pub enum HistoryAction {
    /// Play the history from `start_index`, queueing the rest.
    PlayTracks {
        tracks: Vec<Track>,
        start_index: usize,
    },
}

/// The history view: a fetch state plus a single cursor over the track list.
#[derive(Debug, Clone)]
pub struct HistoryView {
    state: PageState<Vec<Track>>,
    cursor: usize,
}

impl Default for HistoryView {
    fn default() -> Self {
        Self::new()
    }
}

impl HistoryView {
    /// A fresh history view in the loading state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: PageState::Loading,
            cursor: 0,
        }
    }

    /// The current fetch state.
    #[must_use]
    pub fn state(&self) -> &PageState<Vec<Track>> {
        &self.state
    }

    // -- Data loading --------------------------------------------------------

    /// Load the history and reset the cursor.
    pub fn set_tracks(&mut self, tracks: Vec<Track>) {
        self.state = PageState::Loaded(tracks);
        self.cursor = 0;
    }

    /// Transition into the error state.
    pub fn set_error(&mut self, message: impl Into<String>) {
        self.state = PageState::Error(message.into());
    }

    /// Reset to loading (re-fetch triggered when switching to this view).
    pub fn set_loading(&mut self) {
        self.state = PageState::Loading;
        self.cursor = 0;
    }

    // -- Navigation ----------------------------------------------------------

    fn track_count(&self) -> usize {
        self.state.loaded().map_or(0, Vec::len)
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

    /// Resolve an Enter keypress into a [`HistoryAction`], or `None`.
    ///
    /// Mirrors Python: `set_playlist(self._tracks, start_index=row_index)`.
    #[must_use]
    pub fn activate_selected(&self) -> Option<HistoryAction> {
        let tracks = self.state.loaded()?;
        if tracks.is_empty() || self.cursor >= tracks.len() {
            return None;
        }
        Some(HistoryAction::PlayTracks {
            tracks: tracks.clone(),
            start_index: self.cursor,
        })
    }

    // -- Rendering -----------------------------------------------------------

    /// Render the history view into `area`.
    pub fn render(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        // Title (1 row) + status (1 row) + track list.
        let chunks = Layout::vertical([
            Constraint::Length(1), // "Recently played" title
            Constraint::Length(1), // status
            Constraint::Min(1),    // list
        ])
        .split(area);

        // Static title label (mirrors Python's Label("Recently played")).
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "Recently played",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ))),
            chunks[0],
        );

        self.render_status(frame, chunks[1], theme);
        self.render_tracks(frame, chunks[2], theme);
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
            PageState::Loading => ("Loading history...".to_owned(), false),
            PageState::Error(msg) => (msg.clone(), true),
            PageState::Loaded(tracks) => {
                if tracks.is_empty() {
                    ("No history".to_owned(), false)
                } else {
                    (format!("{} track(s) [Esc to go back]", tracks.len()), false)
                }
            }
        }
    }

    fn render_tracks(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        let PageState::Loaded(tracks) = &self.state else {
            return;
        };
        if tracks.is_empty() {
            return;
        }

        let items: Vec<ListItem> = tracks.iter().map(track_row).collect();

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
        if !tracks.is_empty() {
            list_state.select(Some(self.cursor.min(tracks.len() - 1)));
        }
        frame.render_stateful_widget(list, area, &mut list_state);
    }
}

/// Format a track row: `Title — Artist  Album  Duration`.
fn track_row(track: &Track) -> ListItem<'static> {
    let mut spans = vec![Span::raw(track.title.clone())];
    if !track.artist.is_empty() {
        spans.push(Span::raw(" — "));
        spans.push(Span::raw(track.artist.clone()));
    }
    if !track.album.is_empty() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            track.album.clone(),
            Style::default().add_modifier(Modifier::DIM),
        ));
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
        Track::new(id, title, artist, "Album", 200.0, "")
    }

    fn loaded_view() -> HistoryView {
        let mut view = HistoryView::new();
        view.set_tracks(vec![
            make_track("v1", "Pyramid Song", "Radiohead"),
            make_track("v2", "Idioteque", "Radiohead"),
            make_track("v3", "How to Disappear", "Radiohead"),
        ]);
        view
    }

    fn render_to_string(view: &HistoryView, w: u16, h: u16) -> String {
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
        let view = HistoryView::new();
        let text = render_to_string(&view, 60, 8);
        assert!(text.contains("Loading history..."), "text:\n{text}");
    }

    // -- data loading --------------------------------------------------------

    #[test]
    fn set_tracks_loads_data_and_resets_cursor() {
        let view = loaded_view();
        assert_eq!(view.track_count(), 3);
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

    #[test]
    fn navigation_is_noop_when_not_loaded() {
        let mut view = HistoryView::new();
        view.select_next();
        assert_eq!(view.cursor, 0);
        assert!(view.activate_selected().is_none());
    }

    // -- activation (Enter) --------------------------------------------------

    #[test]
    fn enter_plays_from_index_queueing_rest() {
        let mut view = loaded_view();
        view.select_next(); // cursor 1
        match view.activate_selected() {
            Some(HistoryAction::PlayTracks {
                tracks,
                start_index,
            }) => {
                assert_eq!(start_index, 1);
                assert_eq!(tracks.len(), 3); // whole history list
                assert_eq!(tracks[start_index].video_id, "v2");
            }
            other => panic!("expected PlayTracks, got {other:?}"),
        }
    }

    #[test]
    fn enter_on_empty_history_is_none() {
        let mut view = HistoryView::new();
        view.set_tracks(vec![]);
        assert!(view.activate_selected().is_none());
    }

    // -- error state ---------------------------------------------------------

    #[test]
    fn set_error_renders_message() {
        let mut view = HistoryView::new();
        view.set_error("Not signed in");
        let text = render_to_string(&view, 60, 8);
        assert!(text.contains("Not signed in"), "text:\n{text}");
    }

    // -- rendering -----------------------------------------------------------

    #[test]
    fn loaded_render_shows_title_and_tracks() {
        let view = loaded_view();
        let text = render_to_string(&view, 70, 12);
        assert!(text.contains("Recently played"), "missing title:\n{text}");
        assert!(
            text.contains("Pyramid Song"),
            "missing first track:\n{text}"
        );
        assert!(text.contains("Radiohead"), "missing artist:\n{text}");
        assert!(text.contains("3 track(s)"), "missing count:\n{text}");
        assert!(text.contains("Esc to go back"), "missing hint:\n{text}");
    }

    #[test]
    fn loaded_render_shows_selection_marker() {
        let view = loaded_view();
        let text = render_to_string(&view, 70, 12);
        assert!(text.contains("▶"), "missing selection marker:\n{text}");
    }

    #[test]
    fn empty_history_render_shows_no_history() {
        let mut view = HistoryView::new();
        view.set_tracks(vec![]);
        let text = render_to_string(&view, 60, 8);
        assert!(
            text.contains("No history"),
            "missing no-history message:\n{text}"
        );
    }
}
