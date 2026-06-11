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
use ratatui::widgets::{Paragraph, TableState};
use ytmusic_api::Track;

use super::filter_bar::matches_filter;
use super::{PageState, Theme, borderless_table, section_title, table_header, table_row};
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
///
/// When the in-page filter is active (`filter` is `Some`), the cursor and
/// activation operate over the *filtered* subset — the rows the user actually
/// sees — so navigation and "play from here" stay consistent with the display.
#[derive(Debug, Clone)]
pub struct HistoryView {
    state: PageState<Vec<Track>>,
    cursor: usize,
    /// The active in-page filter query (`None` = no filter). Set by the main
    /// loop from the filter bar.
    filter: Option<String>,
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
            filter: None,
        }
    }

    /// The current fetch state.
    #[must_use]
    pub fn state(&self) -> &PageState<Vec<Track>> {
        &self.state
    }

    // -- Data loading --------------------------------------------------------

    /// Load the history and reset the cursor (clears any active filter).
    pub fn set_tracks(&mut self, tracks: Vec<Track>) {
        self.state = PageState::Loaded(tracks);
        self.cursor = 0;
        self.filter = None;
    }

    /// Transition into the error state.
    pub fn set_error(&mut self, message: impl Into<String>) {
        self.state = PageState::Error(message.into());
    }

    /// Reset to loading (re-fetch triggered when switching to this view).
    pub fn set_loading(&mut self) {
        self.state = PageState::Loading;
        self.cursor = 0;
        self.filter = None;
    }

    // -- In-page filter ------------------------------------------------------

    /// Set (or clear) the in-page filter query. A change resets the cursor to
    /// the top of the filtered list so it never points past the visible rows.
    pub fn set_filter(&mut self, query: Option<&str>) {
        let new = query.map(str::to_owned);
        if new != self.filter {
            self.filter = new;
            self.cursor = 0;
        }
    }

    /// The indices into the loaded track list that pass the active filter, in
    /// order. With no filter, every index is returned. Empty when not loaded.
    fn visible_indices(&self) -> Vec<usize> {
        let Some(tracks) = self.state.loaded() else {
            return Vec::new();
        };
        match &self.filter {
            None => (0..tracks.len()).collect(),
            Some(q) => tracks
                .iter()
                .enumerate()
                .filter(|(_, t)| matches_filter(q, &[&t.title, &t.artist, &t.album]))
                .map(|(i, _)| i)
                .collect(),
        }
    }

    /// The `(visible, total)` row counts for the filter bar's label: the number
    /// of rows passing the filter, and the total loaded row count.
    #[must_use]
    pub fn filter_counts(&self) -> (usize, usize) {
        let total = self.state.loaded().map_or(0, Vec::len);
        (self.visible_indices().len(), total)
    }

    /// The track under the cursor, as a [`PopupItem`] for the action popup.
    /// `None` when the list is empty / not loaded. Respects the active filter.
    #[must_use]
    pub fn selected_popup_item(&self) -> Option<super::popup::PopupItem> {
        let visible = self.visible_indices();
        let original = *visible.get(self.cursor)?;
        let track = self.state.loaded()?.get(original)?;
        Some(super::popup::PopupItem::Track(track.clone()))
    }

    /// The tracks that pass the active filter (the visible rows), cloned for
    /// playback queueing.
    fn visible_tracks(&self) -> Vec<Track> {
        let Some(tracks) = self.state.loaded() else {
            return Vec::new();
        };
        self.visible_indices()
            .into_iter()
            .filter_map(|i| tracks.get(i).cloned())
            .collect()
    }

    // -- Navigation ----------------------------------------------------------

    /// The number of *visible* (post-filter) rows.
    fn track_count(&self) -> usize {
        self.visible_indices().len()
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
    /// Plays from the selected (visible) row, queueing the rest of the *visible*
    /// rows — so with a filter active, only the filtered subset is queued, which
    /// matches what the user sees. Mirrors Python's
    /// `set_playlist(self._tracks, start_index=row_index)` over the visible rows.
    #[must_use]
    pub fn activate_selected(&self) -> Option<HistoryAction> {
        let visible = self.visible_tracks();
        if visible.is_empty() || self.cursor >= visible.len() {
            return None;
        }
        Some(HistoryAction::PlayTracks {
            tracks: visible,
            start_index: self.cursor,
        })
    }

    // -- Rendering -----------------------------------------------------------

    /// Render the history view into `area`.
    ///
    /// An accent "Recently played" title line + a muted status line, over a
    /// borderless DataTable with `Title`/`Artist`/`Album`/`Duration` columns
    /// (no panel border — flat `surface`).
    pub fn render(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        let chunks = Layout::vertical([
            Constraint::Length(1), // "Recently played" title
            Constraint::Length(1), // status
            Constraint::Min(1),    // table
        ])
        .split(area);

        frame.render_widget(
            Paragraph::new(Line::from(section_title(theme, "Recently played", true)))
                .style(Style::default().bg(theme.surface)),
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
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(text, style)))
                .style(Style::default().bg(theme.surface)),
            area,
        );
    }

    fn status_text(&self) -> (String, bool) {
        match &self.state {
            PageState::Loading => ("Loading history...".to_owned(), false),
            PageState::Error(msg) => (msg.clone(), true),
            PageState::Loaded(tracks) => {
                if tracks.is_empty() {
                    ("No history".to_owned(), false)
                } else if self.filter.is_some() {
                    // With a filter active, report the visible / total counts.
                    let visible = self.track_count();
                    (
                        format!("{visible}/{} track(s) [Esc to go back]", tracks.len()),
                        false,
                    )
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
        let visible = self.visible_indices();
        if visible.is_empty() {
            return;
        }

        let header = table_header(theme, &HISTORY_COLUMNS, true);
        let rows = visible
            .iter()
            .filter_map(|&i| tracks.get(i))
            .map(|t| table_row(theme, &track_columns(t), true))
            .collect();
        let table = borderless_table(theme, header, rows, history_widths(), true);

        let mut state = TableState::default();
        state.select(Some(self.cursor.min(visible.len() - 1)));
        frame.render_stateful_widget(table, area, &mut state);
    }
}

/// Column labels for the history table (Python `add_columns`).
const HISTORY_COLUMNS: [&str; 4] = ["Title", "Artist", "Album", "Duration"];

/// Column widths (including the one-space cell padding on each side).
fn history_widths() -> Vec<Constraint> {
    vec![
        Constraint::Min(10),    // Title (flex)
        Constraint::Length(22), // Artist
        Constraint::Length(22), // Album
        Constraint::Length(10), // Duration
    ]
}

/// Format a track into its `Title`/`Artist`/`Album`/`Duration` columns.
fn track_columns(track: &Track) -> Vec<String> {
    let duration = format_duration(track.duration_seconds);
    let duration = if duration == "—" {
        String::new()
    } else {
        duration
    };
    vec![
        track.title.clone(),
        track.artist.clone(),
        track.album.clone(),
        duration,
    ]
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
    fn loaded_render_highlights_selection_with_primary_and_shows_columns() {
        // Borderless DataTable: primary-bg cursor (no `▶`), with the
        // Title/Artist/Album/Duration column headers and the "Recently played"
        // title.
        let view = loaded_view();
        let text = render_to_string(&view, 70, 12);
        assert!(!text.contains('▶'), "stray cursor glyph:\n{text}");
        assert!(text.contains("Recently played"), "missing title:\n{text}");
        for col in ["Title", "Artist", "Album", "Duration"] {
            assert!(text.contains(col), "missing '{col}' column:\n{text}");
        }

        let backend = TestBackend::new(70, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = Theme::default();
        terminal
            .draw(|frame| view.render(frame, frame.area(), &theme))
            .unwrap();
        assert!(
            terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .any(|c| c.bg == theme.primary),
            "selected row not highlighted with primary"
        );
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

    // -- in-page filter ------------------------------------------------------

    #[test]
    fn filter_narrows_visible_rows() {
        let mut view = HistoryView::new();
        view.set_tracks(vec![
            make_track("v1", "Pyramid Song", "Radiohead"),
            make_track("v2", "Get Lucky", "Daft Punk"),
            make_track("v3", "Idioteque", "Radiohead"),
        ]);
        view.set_filter(Some("daft"));
        assert_eq!(view.track_count(), 1, "only the Daft Punk track matches");
    }

    #[test]
    fn filter_render_shows_only_matches_and_count() {
        let mut view = HistoryView::new();
        view.set_tracks(vec![
            make_track("v1", "Pyramid Song", "Radiohead"),
            make_track("v2", "Get Lucky", "Daft Punk"),
        ]);
        view.set_filter(Some("lucky"));
        let text = render_to_string(&view, 70, 12);
        assert!(text.contains("Get Lucky"), "missing match:\n{text}");
        assert!(!text.contains("Pyramid Song"), "non-match shown:\n{text}");
        assert!(
            text.contains("1/2 track(s)"),
            "missing filtered count:\n{text}"
        );
    }

    #[test]
    fn filter_activate_queues_visible_subset() {
        let mut view = HistoryView::new();
        view.set_tracks(vec![
            make_track("v1", "Pyramid Song", "Radiohead"),
            make_track("v2", "Get Lucky", "Daft Punk"),
            make_track("v3", "Idioteque", "Radiohead"),
        ]);
        view.set_filter(Some("radiohead"));
        // Two Radiohead tracks visible; play from the first.
        match view.activate_selected() {
            Some(HistoryAction::PlayTracks {
                tracks,
                start_index,
            }) => {
                assert_eq!(tracks.len(), 2, "only the filtered subset is queued");
                assert_eq!(start_index, 0);
                assert_eq!(tracks[0].video_id, "v1");
                assert_eq!(tracks[1].video_id, "v3");
            }
            other => panic!("expected PlayTracks, got {other:?}"),
        }
    }

    #[test]
    fn clearing_filter_restores_full_list() {
        let mut view = loaded_view();
        view.set_filter(Some("nomatch"));
        assert_eq!(view.track_count(), 0);
        view.set_filter(None);
        assert_eq!(view.track_count(), 3, "clearing restores all rows");
    }

    #[test]
    fn changing_filter_resets_cursor() {
        let mut view = loaded_view();
        view.select_next();
        view.select_next();
        assert_eq!(view.cursor, 2);
        view.set_filter(Some("radiohead"));
        assert_eq!(view.cursor, 0, "cursor resets when the filter changes");
    }
}
