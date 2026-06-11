//! Album detail view with track listing.
//!
//! Port of `src/ytmusic_tui/views/album.py`. Displays the album header
//! (title, artist, year) and a scrollable track list. Enter on a track plays
//! from the selected index, queueing the rest of the album (spotify_player
//! style — the whole `AlbumInfo.tracks` slice, starting at the cursor).
//!
//! # Fetch flow vs Python
//!
//! Python's `AlbumView.load_album(browse_id)` ran a Textual worker that called
//! `get_album(browse_id)`. Here the view is a pure value; the runtime owns the
//! API client. The main loop issues [`crate::app::AppCommand::FetchAlbum`] and
//! folds the reply as [`crate::app::AppEvent::AlbumLoaded`] into this view.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, TableState};
use ytmusic_api::{AlbumInfo, Track};

use super::{PageState, Theme, borderless_table, table_header, table_row};
use crate::formatting::format_duration;

/// What an Enter keypress on the album view resolves to.
///
/// Returned by [`AlbumView::activate_selected`] so the main loop can issue an
/// [`crate::app::AppCommand`]. Mirrors Python's `on_data_table_row_selected`:
/// queue the whole album starting at `start_index`.
#[derive(Debug, Clone, PartialEq)]
pub enum AlbumAction {
    /// Play the album from `start_index`, queueing the rest (spotify_player).
    /// `tracks` is the complete album track list; `start_index` is the cursor.
    PlayTracks {
        tracks: Vec<Track>,
        start_index: usize,
    },
}

/// The album detail view: a fetch state plus a single cursor over the track
/// list.
///
/// The cursor is kept on the struct (not inside the state enum) so it survives
/// a re-render and resets deliberately when new data arrives.
#[derive(Debug, Clone)]
pub struct AlbumView {
    /// Fetch state; the payload is the loaded album.
    state: PageState<AlbumInfo>,
    /// Cursor into the track list (meaningful only in the Loaded state).
    cursor: usize,
}

impl Default for AlbumView {
    fn default() -> Self {
        Self::new()
    }
}

impl AlbumView {
    /// A fresh album view in the loading state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: PageState::Loading,
            cursor: 0,
        }
    }

    /// The current fetch state (for tests and the main loop).
    #[must_use]
    pub fn state(&self) -> &PageState<AlbumInfo> {
        &self.state
    }

    // -- Data loading (driven by the main loop from AppEvents) ---------------

    /// Load the album data and reset the cursor.
    ///
    /// Called when [`crate::app::AppEvent::AlbumLoaded`] arrives.
    pub fn set_album(&mut self, album: AlbumInfo) {
        self.state = PageState::Loaded(album);
        self.cursor = 0;
    }

    /// Transition into the error state with a classified message.
    pub fn set_error(&mut self, message: impl Into<String>) {
        self.state = PageState::Error(message.into());
    }

    /// Reset to loading (e.g. when navigating to a new album before the reply
    /// arrives).
    pub fn set_loading(&mut self) {
        self.state = PageState::Loading;
        self.cursor = 0;
    }

    // -- Navigation ----------------------------------------------------------

    /// The number of tracks (0 unless loaded).
    fn track_count(&self) -> usize {
        self.state.loaded().map_or(0, |a| a.tracks.len())
    }

    /// Move the cursor down one row, clamping at the last track.
    pub fn select_next(&mut self) {
        let last = self.track_count().saturating_sub(1);
        if self.cursor < last {
            self.cursor += 1;
        }
    }

    /// Move the cursor up one row, clamping at the first track.
    pub fn select_previous(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    /// Resolve an Enter keypress into an [`AlbumAction`], or `None` when
    /// nothing is selected. Mirrors Python's `on_data_table_row_selected`:
    /// queue the full album starting at the cursor.
    #[must_use]
    pub fn activate_selected(&self) -> Option<AlbumAction> {
        let album = self.state.loaded()?;
        if album.tracks.is_empty() || self.cursor >= album.tracks.len() {
            return None;
        }
        Some(AlbumAction::PlayTracks {
            tracks: album.tracks.clone(),
            start_index: self.cursor,
        })
    }

    /// The album track under the cursor as a [`PopupItem`] for the action popup.
    /// `None` when the album is empty / not loaded.
    #[must_use]
    pub fn selected_popup_item(&self) -> Option<super::popup::PopupItem> {
        let album = self.state.loaded()?;
        album
            .tracks
            .get(self.cursor)
            .map(|t| super::popup::PopupItem::Track(t.clone()))
    }

    // -- Rendering -----------------------------------------------------------

    /// Render the album view into `area`.
    ///
    /// An accent album-title header + muted `Artist - Year` meta line + a muted
    /// status line, over a borderless DataTable with `#`/`Title`/`Artist`/
    /// `Duration` columns (no "Album" panel border — flat `surface`).
    pub fn render(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        // Header (2 rows: title + meta) + status (1 row) + track table.
        let chunks = Layout::vertical([
            Constraint::Length(2), // title + meta
            Constraint::Length(1), // status / track count
            Constraint::Min(1),    // track table
        ])
        .split(area);

        self.render_header(frame, chunks[0], theme);
        self.render_status(frame, chunks[1], theme);
        self.render_tracks(frame, chunks[2], theme);
    }

    /// Draw the album title and `Artist - Year` meta line.
    fn render_header(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        let rows = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(area);

        let (title, meta) = match self.state.loaded() {
            Some(album) => {
                let title = album.title.clone();
                let meta = {
                    let mut p: Vec<String> = Vec::new();
                    if !album.artist.is_empty() {
                        p.push(album.artist.clone());
                    }
                    if !album.year.is_empty() {
                        p.push(album.year.clone());
                    }
                    p.join(" - ")
                };
                (title, meta)
            }
            None => (String::new(), String::new()),
        };

        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                title,
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            )))
            .style(Style::default().bg(theme.surface)),
            rows[0],
        );
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                meta,
                Style::default()
                    .fg(theme.text_muted)
                    .add_modifier(Modifier::ITALIC),
            )))
            .style(Style::default().bg(theme.surface)),
            rows[1],
        );
    }

    /// Draw the status / track-count line.
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

    /// Compute the status line text and error flag.
    fn status_text(&self) -> (String, bool) {
        match &self.state {
            PageState::Loading => ("Loading album...".to_owned(), false),
            PageState::Error(msg) => (msg.clone(), true),
            PageState::Loaded(album) => {
                if album.tracks.is_empty() {
                    ("No tracks".to_owned(), false)
                } else {
                    (
                        format!("{} track(s) [Esc to go back]", album.tracks.len()),
                        false,
                    )
                }
            }
        }
    }

    /// Draw the track table.
    fn render_tracks(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        let PageState::Loaded(album) = &self.state else {
            return; // loading / error: status line carries the message
        };
        if album.tracks.is_empty() {
            return;
        }

        let header = table_header(theme, &ALBUM_COLUMNS, true);
        let rows = album
            .tracks
            .iter()
            .enumerate()
            .map(|(i, t)| table_row(theme, &track_columns(i + 1, t), true))
            .collect();
        let table = borderless_table(theme, header, rows, album_widths(), true);

        let mut state = TableState::default();
        // saturating_sub keeps this safe even if the early empty-return above
        // is ever bypassed (the guard-consistent form used by all other views).
        state.select(Some(self.cursor.min(album.tracks.len().saturating_sub(1))));
        frame.render_stateful_widget(table, area, &mut state);
    }
}

/// Column labels for the album track table (Python `add_columns`).
const ALBUM_COLUMNS: [&str; 4] = ["#", "Title", "Artist", "Duration"];

/// Column widths (including the one-space cell padding on each side).
fn album_widths() -> Vec<Constraint> {
    vec![
        Constraint::Length(5),  // "#"
        Constraint::Min(10),    // Title (flex)
        Constraint::Length(24), // Artist
        Constraint::Length(10), // Duration
    ]
}

/// Format a track into its `#`/`Title`/`Artist`/`Duration` column strings.
fn track_columns(num: usize, track: &Track) -> Vec<String> {
    let duration = format_duration(track.duration_seconds);
    let duration = if duration == "—" {
        String::new()
    } else {
        duration
    };
    vec![
        num.to_string(),
        track.title.clone(),
        track.artist.clone(),
        duration,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ytmusic_api::AlbumInfo;

    // -- fixtures ------------------------------------------------------------

    fn make_track(id: &str, title: &str, artist: &str) -> Track {
        Track::new(id, title, artist, "Album", 180.0, "")
    }

    fn make_album(tracks: Vec<Track>) -> AlbumInfo {
        AlbumInfo::new("b1", "OK Computer", "Radiohead", "1997", tracks, "")
    }

    fn loaded_view() -> AlbumView {
        let mut view = AlbumView::new();
        view.set_album(make_album(vec![
            make_track("v1", "Airbag", "Radiohead"),
            make_track("v2", "Paranoid Android", "Radiohead"),
            make_track("v3", "Subterranean Homesick Alien", "Radiohead"),
        ]));
        view
    }

    fn render_to_string(view: &AlbumView, w: u16, h: u16) -> String {
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
        let view = AlbumView::new();
        assert!(matches!(view.state(), PageState::Loading));
        let text = render_to_string(&view, 60, 8);
        assert!(text.contains("Loading album..."), "text:\n{text}");
    }

    // -- set_album -----------------------------------------------------------

    #[test]
    fn set_album_loads_data_and_resets_cursor() {
        let view = loaded_view();
        assert!(matches!(view.state(), PageState::Loaded(_)));
        assert_eq!(view.cursor, 0);
        assert_eq!(view.track_count(), 3);
    }

    #[test]
    fn set_album_resets_cursor_even_after_navigation() {
        let mut view = loaded_view();
        view.select_next();
        view.select_next();
        assert_eq!(view.cursor, 2);
        // Re-loading resets.
        view.set_album(make_album(vec![make_track("v4", "New", "Artist")]));
        assert_eq!(view.cursor, 0);
    }

    // -- navigation: clamp at ends -------------------------------------------

    #[test]
    fn select_next_clamps_at_last_track() {
        let mut view = loaded_view(); // 3 tracks
        view.select_next();
        view.select_next();
        view.select_next(); // would be index 3 -> clamps at 2
        assert_eq!(view.cursor, 2);
    }

    #[test]
    fn select_previous_clamps_at_first_track() {
        let mut view = loaded_view();
        view.select_previous();
        assert_eq!(view.cursor, 0);
    }

    #[test]
    fn navigation_is_noop_when_not_loaded() {
        let mut view = AlbumView::new();
        view.select_next();
        assert_eq!(view.cursor, 0);
        assert!(view.activate_selected().is_none());
    }

    // -- activation (Enter) --------------------------------------------------

    #[test]
    fn enter_on_first_track_plays_from_index_zero() {
        let view = loaded_view();
        match view.activate_selected() {
            Some(AlbumAction::PlayTracks {
                tracks,
                start_index,
            }) => {
                assert_eq!(start_index, 0);
                assert_eq!(tracks.len(), 3);
                assert_eq!(tracks[0].video_id, "v1");
            }
            other => panic!("expected PlayTracks, got {other:?}"),
        }
    }

    #[test]
    fn enter_after_moving_plays_from_correct_index() {
        let mut view = loaded_view();
        view.select_next(); // cursor 1
        match view.activate_selected() {
            Some(AlbumAction::PlayTracks {
                tracks,
                start_index,
            }) => {
                // Whole album is queued; start is 1.
                assert_eq!(start_index, 1);
                assert_eq!(tracks.len(), 3);
                assert_eq!(tracks[start_index].video_id, "v2");
            }
            other => panic!("expected PlayTracks, got {other:?}"),
        }
    }

    #[test]
    fn enter_on_empty_album_is_none() {
        let mut view = AlbumView::new();
        view.set_album(make_album(vec![]));
        assert!(view.activate_selected().is_none());
    }

    // -- error state ---------------------------------------------------------

    #[test]
    fn set_error_renders_message() {
        let mut view = AlbumView::new();
        view.set_error("Session expired — run: ytmusic-tui auth");
        let text = render_to_string(&view, 60, 8);
        assert!(text.contains("Session expired"), "text:\n{text}");
    }

    // -- rendering (TestBackend) ---------------------------------------------

    #[test]
    fn loaded_render_shows_title_and_tracks() {
        let view = loaded_view();
        let text = render_to_string(&view, 70, 12);
        assert!(text.contains("OK Computer"), "missing album title:\n{text}");
        assert!(text.contains("Radiohead"), "missing artist:\n{text}");
        assert!(text.contains("1997"), "missing year:\n{text}");
        assert!(text.contains("Airbag"), "missing first track:\n{text}");
        assert!(
            text.contains("Paranoid Android"),
            "missing second track:\n{text}"
        );
        assert!(text.contains("3 track(s)"), "missing count:\n{text}");
        assert!(text.contains("Esc to go back"), "missing hint:\n{text}");
    }

    #[test]
    fn loaded_render_highlights_selection_with_primary_and_shows_columns() {
        // Borderless DataTable: primary-bg cursor (no `▶`), with the album's
        // `#`/`Title`/`Artist`/`Duration` column headers.
        let view = loaded_view();
        let text = render_to_string(&view, 70, 12);
        assert!(!text.contains('▶'), "stray cursor glyph:\n{text}");
        for col in ["Title", "Artist", "Duration"] {
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
    fn empty_album_render_shows_no_tracks() {
        let mut view = AlbumView::new();
        view.set_album(make_album(vec![]));
        let text = render_to_string(&view, 60, 8);
        assert!(
            text.contains("No tracks"),
            "missing no-tracks message:\n{text}"
        );
    }
}
