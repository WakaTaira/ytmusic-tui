//! Playlist view with two-level navigation (playlist list → track list).
//!
//! Port of `src/ytmusic_tui/views/playlist.py`. The view has two levels:
//!
//! * **Level 1 — playlist list:** the user's library playlists (title, track
//!   count). Enter drills into the selected playlist's tracks.
//! * **Level 2 — track list:** the tracks of one playlist. Enter plays from the
//!   selected index, queueing the rest (spotify_player style); Escape returns to
//!   the playlist list.
//!
//! # Fetch flow vs Python
//!
//! Textual's `PlaylistView` fetched inside the view via `_run_fetch`; here the
//! view is a pure value and the *runtime* owns the API client (see
//! [`crate::app`]). So the view exposes the two levels as
//! [`PageState`]-backed lists that the main loop fills from
//! [`crate::app::AppEvent::LibraryPlaylistsLoaded`] /
//! [`crate::app::AppEvent::PlaylistTracksLoaded`], and Enter/Escape resolve to a
//! [`PlaylistAction`] the main loop turns into commands. The two-level state
//! machine (which list is showing, the per-level cursor) lives here.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ytmusic_api::{PlaylistInfo, Track};

use super::{PageState, Theme};
use crate::formatting::format_duration;

/// What an Enter / Escape keypress on the playlist view resolves to.
///
/// Returned by [`PlaylistView::activate_selected`] (Enter) and
/// [`PlaylistView::go_back`] (Escape) so the main loop can translate it into an
/// [`crate::app::AppCommand`]. Mirrors Python's `on_data_table_row_selected`
/// dispatch.
#[derive(Debug, Clone, PartialEq)]
pub enum PlaylistAction {
    /// Drill into a playlist: fetch and show its tracks. The caller issues a
    /// [`crate::app::AppCommand::FetchPlaylistTracks`]. Python:
    /// `show_track_list(playlist)`.
    OpenPlaylist(PlaylistInfo),
    /// Play the loaded tracks from `start_index`, queueing the rest
    /// (spotify_player). Python: `set_playlist(tracks, start_index); play(...)`.
    PlayTracks {
        tracks: Vec<Track>,
        start_index: usize,
    },
}

/// Which of the two levels the view is currently showing.
///
/// The fetch state of each level is held inline so the illegal "showing tracks
/// but the playlist list errored" combination cannot arise — the view is in
/// exactly one level at a time.
#[derive(Debug, Clone)]
enum Level {
    /// Level 1: the library playlist list.
    Playlists(PageState<Vec<PlaylistInfo>>),
    /// Level 2: the tracks of one playlist. `title` labels the list.
    Tracks {
        title: String,
        state: PageState<Vec<Track>>,
    },
}

/// The two-level playlist browser: the active level plus a per-level cursor.
///
/// The cursor is kept on the struct (not inside [`Level`]) so it survives a
/// re-render, and is reset deliberately when a level's data is (re)loaded or the
/// level switches — matching a freshly focused Textual table.
#[derive(Debug, Clone)]
pub struct PlaylistView {
    level: Level,
    /// Cursor into the active level's items.
    cursor: usize,
}

impl Default for PlaylistView {
    fn default() -> Self {
        Self::new()
    }
}

impl PlaylistView {
    /// A fresh playlist view at level 1 in the loading state.
    ///
    /// The main loop issues [`crate::app::AppCommand::FetchLibraryPlaylists`]
    /// when it first switches to this view; until the reply lands the level-1
    /// list shows "Loading…" (Python `on_mount` → `_show_playlist_list`).
    #[must_use]
    pub fn new() -> Self {
        Self {
            level: Level::Playlists(PageState::Loading),
            cursor: 0,
        }
    }

    /// Whether the view is showing the track list (level 2) rather than the
    /// playlist list (level 1). Python `is_viewing_tracks`.
    #[must_use]
    pub fn is_viewing_tracks(&self) -> bool {
        matches!(self.level, Level::Tracks { .. })
    }

    // -- Data loading (driven by the main loop from AppEvents) -------------

    /// Load the level-1 playlist list and reset to level 1
    /// (`LibraryPlaylistsLoaded`).
    pub fn set_playlists(&mut self, playlists: Vec<PlaylistInfo>) {
        self.level = Level::Playlists(PageState::Loaded(playlists));
        self.cursor = 0;
    }

    /// Switch to level 2 in the loading state for `title`
    /// (the moment the user drills in, before the tracks arrive).
    pub fn show_track_list_loading(&mut self, title: impl Into<String>) {
        self.level = Level::Tracks {
            title: title.into(),
            state: PageState::Loading,
        };
        self.cursor = 0;
    }

    /// Fill the level-2 track list (`PlaylistTracksLoaded`).
    ///
    /// Keeps the current `title` if already at level 2; otherwise adopts the
    /// supplied `title` (covers a tracks-arrive-before-show race, which cannot
    /// happen with the current single-threaded fold but keeps the method total).
    pub fn set_tracks(&mut self, title: impl Into<String>, tracks: Vec<Track>) {
        let title = match &self.level {
            Level::Tracks { title, .. } => title.clone(),
            Level::Playlists(_) => title.into(),
        };
        self.level = Level::Tracks {
            title,
            state: PageState::Loaded(tracks),
        };
        self.cursor = 0;
    }

    /// Put the active level into the error state with a classified message.
    pub fn set_error(&mut self, message: impl Into<String>) {
        let message = message.into();
        match &mut self.level {
            Level::Playlists(state) => *state = PageState::Error(message),
            Level::Tracks { state, .. } => *state = PageState::Error(message),
        }
        self.cursor = 0;
    }

    // -- Navigation --------------------------------------------------------

    /// The number of selectable rows in the active level (0 unless loaded).
    fn item_count(&self) -> usize {
        match &self.level {
            Level::Playlists(state) => state.loaded().map_or(0, Vec::len),
            Level::Tracks { state, .. } => state.loaded().map_or(0, Vec::len),
        }
    }

    /// Move the cursor down one row, clamping at the last item (Textual tables
    /// clamp at their ends; no wrap).
    pub fn select_next(&mut self) {
        let last = self.item_count().saturating_sub(1);
        if self.cursor < last {
            self.cursor += 1;
        }
    }

    /// Move the cursor up one row, clamping at the first item.
    pub fn select_previous(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    /// Resolve an Enter keypress into a [`PlaylistAction`].
    ///
    /// At level 1: drill into the selected playlist
    /// ([`PlaylistAction::OpenPlaylist`]). At level 2: play from the cursor,
    /// queueing the rest ([`PlaylistAction::PlayTracks`]). Returns `None` when
    /// nothing is selected (not loaded, or an empty list).
    #[must_use]
    pub fn activate_selected(&self) -> Option<PlaylistAction> {
        match &self.level {
            Level::Playlists(state) => {
                let playlist = state.loaded()?.get(self.cursor)?;
                Some(PlaylistAction::OpenPlaylist(playlist.clone()))
            }
            Level::Tracks { state, .. } => {
                let tracks = state.loaded()?;
                if self.cursor >= tracks.len() {
                    return None;
                }
                Some(PlaylistAction::PlayTracks {
                    tracks: tracks.clone(),
                    start_index: self.cursor,
                })
            }
        }
    }

    /// Resolve an Escape keypress: at level 2, return to the level-1 list.
    ///
    /// Returns `true` when the view handled the Escape (was at level 2 and
    /// popped back to level 1), so the caller knows it was consumed and need not
    /// pop the navigation stack itself. Returns `false` at level 1 (Escape there
    /// is the app's "go back" / no-op). Mirrors Python's `on_key` Escape branch
    /// which calls `_show_playlist_list()` only while viewing tracks.
    ///
    /// The level-1 list is set back to [`PageState::Loading`]: the caller
    /// re-issues [`crate::app::AppCommand::FetchLibraryPlaylists`] (Python
    /// re-fetched in `_show_playlist_list`). Until those tracks arrive the list
    /// shows "Loading…".
    pub fn go_back(&mut self) -> bool {
        if self.is_viewing_tracks() {
            self.level = Level::Playlists(PageState::Loading);
            self.cursor = 0;
            true
        } else {
            false
        }
    }

    // -- Rendering ---------------------------------------------------------

    /// Render the active level into `area`.
    pub fn render(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        // A one-row status header + the list below it.
        let chunks = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(area);
        self.render_status(frame, chunks[0], theme);
        self.render_list(frame, chunks[1], theme);
    }

    /// Draw the status line (the playlist count, the track-list header with the
    /// "[Esc to go back]" hint, or the Loading/Error line).
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

    /// Compute the status text and whether it is an error (for styling).
    fn status_text(&self) -> (String, bool) {
        match &self.level {
            Level::Playlists(PageState::Loaded(playlists)) => {
                if playlists.is_empty() {
                    ("No playlists found".to_owned(), false)
                } else {
                    (format!("{} playlist(s)", playlists.len()), false)
                }
            }
            Level::Playlists(PageState::Loading) => ("Loading playlists...".to_owned(), false),
            Level::Playlists(PageState::Error(msg)) => (msg.clone(), true),
            Level::Tracks {
                title,
                state: PageState::Loaded(tracks),
            } => {
                if tracks.is_empty() {
                    (format!("{title} - empty playlist [Esc to go back]"), false)
                } else {
                    (
                        format!("{title} - {} track(s) [Esc to go back]", tracks.len()),
                        false,
                    )
                }
            }
            Level::Tracks {
                title,
                state: PageState::Loading,
            } => (format!("Loading tracks for {title}..."), false),
            Level::Tracks {
                state: PageState::Error(msg),
                ..
            } => (msg.clone(), true),
        }
    }

    /// Draw the active level's list (or nothing while loading/errored — the
    /// status line carries that message).
    fn render_list(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        let items: Vec<ListItem> = match &self.level {
            Level::Playlists(PageState::Loaded(playlists)) => {
                playlists.iter().map(playlist_row).collect()
            }
            Level::Tracks {
                state: PageState::Loaded(tracks),
                ..
            } => tracks.iter().map(track_row).collect(),
            // Loading / Error: the status line already shows the message.
            _ => return,
        };

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
        if self.item_count() > 0 {
            list_state.select(Some(self.cursor.min(self.item_count() - 1)));
        }
        frame.render_stateful_widget(list, area, &mut list_state);
    }
}

/// Format a playlist as a level-1 row: `Title  (N tracks)`.
fn playlist_row(playlist: &PlaylistInfo) -> ListItem<'static> {
    let count = if playlist.track_count > 0 {
        format!("{} tracks", playlist.track_count)
    } else {
        "Playlist".to_owned()
    };
    ListItem::new(Line::from(vec![
        Span::raw(playlist.title.clone()),
        Span::raw("  "),
        Span::styled(count, Style::default().add_modifier(Modifier::DIM)),
    ]))
}

/// Format a track as a level-2 row: `Title — Artist  Duration`.
fn track_row(track: &Track) -> ListItem<'static> {
    let mut spans = vec![Span::raw(track.title.clone())];
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

    // -- fixtures ----------------------------------------------------------

    fn playlist(id: &str, title: &str, count: u32) -> PlaylistInfo {
        PlaylistInfo::new(id, title, "", count, "")
    }

    fn track(id: &str, title: &str, artist: &str) -> Track {
        Track::new(id, title, artist, "Album", 100.0, "")
    }

    fn loaded_list_view() -> PlaylistView {
        let mut view = PlaylistView::new();
        view.set_playlists(vec![
            playlist("PL1", "My Mix", 25),
            playlist("PL2", "Chill", 10),
        ]);
        view
    }

    fn loaded_track_view() -> PlaylistView {
        let mut view = PlaylistView::new();
        view.show_track_list_loading("My Mix");
        view.set_tracks(
            "My Mix",
            vec![
                track("v1", "First", "Artist A"),
                track("v2", "Second", "Artist B"),
            ],
        );
        view
    }

    fn render_to_string(view: &PlaylistView, w: u16, h: u16) -> String {
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

    // -- initial state -----------------------------------------------------

    #[test]
    fn new_view_is_level_one_loading() {
        let view = PlaylistView::new();
        assert!(!view.is_viewing_tracks());
        // Loading status renders before data lands.
        let text = render_to_string(&view, 40, 6);
        assert!(text.contains("Loading playlists..."), "text:\n{text}");
    }

    // -- level 1: playlist list --------------------------------------------

    #[test]
    fn set_playlists_loads_level_one() {
        let view = loaded_list_view();
        assert!(!view.is_viewing_tracks());
        assert_eq!(view.item_count(), 2);
    }

    #[test]
    fn enter_on_playlist_opens_it() {
        let view = loaded_list_view();
        match view.activate_selected() {
            Some(PlaylistAction::OpenPlaylist(info)) => assert_eq!(info.playlist_id, "PL1"),
            other => panic!("expected OpenPlaylist(PL1), got {other:?}"),
        }
    }

    #[test]
    fn enter_after_moving_opens_the_right_playlist() {
        let mut view = loaded_list_view();
        view.select_next();
        match view.activate_selected() {
            Some(PlaylistAction::OpenPlaylist(info)) => assert_eq!(info.playlist_id, "PL2"),
            other => panic!("expected OpenPlaylist(PL2), got {other:?}"),
        }
    }

    // -- level 2: track list -----------------------------------------------

    #[test]
    fn show_track_list_loading_switches_level() {
        let mut view = loaded_list_view();
        view.show_track_list_loading("My Mix");
        assert!(view.is_viewing_tracks());
        let text = render_to_string(&view, 50, 6);
        assert!(text.contains("Loading tracks for My Mix"), "text:\n{text}");
    }

    #[test]
    fn set_tracks_loads_level_two() {
        let view = loaded_track_view();
        assert!(view.is_viewing_tracks());
        assert_eq!(view.item_count(), 2);
    }

    #[test]
    fn enter_on_track_plays_from_index_queueing_rest() {
        let mut view = loaded_track_view();
        view.select_next(); // cursor on the second track
        match view.activate_selected() {
            Some(PlaylistAction::PlayTracks {
                tracks,
                start_index,
            }) => {
                // spotify_player: the WHOLE list is queued, starting at index 1.
                assert_eq!(start_index, 1);
                assert_eq!(tracks.len(), 2);
                assert_eq!(tracks[start_index].video_id, "v2");
            }
            other => panic!("expected PlayTracks, got {other:?}"),
        }
    }

    // -- navigation: cursor clamps -----------------------------------------

    #[test]
    fn select_next_clamps_at_end() {
        let mut view = loaded_list_view(); // 2 items
        view.select_next();
        view.select_next(); // would be index 2 -> clamps at 1
        assert_eq!(view.cursor, 1);
    }

    #[test]
    fn select_previous_clamps_at_top() {
        let mut view = loaded_list_view();
        view.select_previous();
        assert_eq!(view.cursor, 0);
    }

    #[test]
    fn navigation_is_noop_when_not_loaded() {
        let mut view = PlaylistView::new(); // level 1 loading
        view.select_next();
        assert_eq!(view.cursor, 0);
        assert!(view.activate_selected().is_none());
    }

    // -- Escape / go_back --------------------------------------------------

    #[test]
    fn go_back_from_tracks_returns_to_list_and_is_handled() {
        let mut view = loaded_track_view();
        assert!(view.is_viewing_tracks());
        let handled = view.go_back();
        assert!(handled, "Escape at level 2 must be consumed by the view");
        assert!(!view.is_viewing_tracks());
        assert_eq!(view.cursor, 0);
    }

    #[test]
    fn go_back_from_list_is_not_handled() {
        let mut view = loaded_list_view();
        let handled = view.go_back();
        assert!(
            !handled,
            "Escape at level 1 is the app's go-back, not the view's"
        );
        assert!(!view.is_viewing_tracks());
    }

    #[test]
    fn drilling_in_then_back_resets_cursor() {
        let mut view = loaded_list_view();
        view.select_next(); // cursor 1 on the list
        view.show_track_list_loading("Chill");
        assert_eq!(view.cursor, 0); // reset on level switch
        view.go_back();
        assert_eq!(view.cursor, 0); // reset on go-back
    }

    // -- error state -------------------------------------------------------

    #[test]
    fn set_error_renders_on_active_level() {
        let mut view = loaded_list_view();
        view.set_error("Session expired — run: ytmusic-tui auth");
        let text = render_to_string(&view, 60, 6);
        assert!(text.contains("Session expired"), "text:\n{text}");
    }

    // -- rendering (TestBackend) -------------------------------------------

    #[test]
    fn level_one_render_shows_playlist_titles_and_counts() {
        let view = loaded_list_view();
        let text = render_to_string(&view, 50, 8);
        assert!(text.contains("My Mix"), "text:\n{text}");
        assert!(text.contains("Chill"), "text:\n{text}");
        assert!(text.contains("25 tracks"), "text:\n{text}");
        assert!(text.contains("2 playlist(s)"), "text:\n{text}");
    }

    #[test]
    fn level_two_render_shows_tracks_and_back_hint() {
        let view = loaded_track_view();
        let text = render_to_string(&view, 60, 8);
        assert!(text.contains("First"), "text:\n{text}");
        assert!(text.contains("Artist A"), "text:\n{text}");
        assert!(text.contains("Esc to go back"), "text:\n{text}");
        assert!(text.contains("2 track(s)"), "text:\n{text}");
    }

    #[test]
    fn render_shows_selection_marker() {
        let view = loaded_list_view();
        let text = render_to_string(&view, 50, 8);
        assert!(text.contains("▶"), "missing selection marker:\n{text}");
    }

    #[test]
    fn empty_playlist_list_renders_message() {
        let mut view = PlaylistView::new();
        view.set_playlists(vec![]);
        let text = render_to_string(&view, 40, 6);
        assert!(text.contains("No playlists found"), "text:\n{text}");
    }
}
