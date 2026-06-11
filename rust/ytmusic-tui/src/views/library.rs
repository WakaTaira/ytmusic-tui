//! Library view with a 3-pane layout: Playlists, Albums, Artists.
//!
//! Port of `src/ytmusic_tui/views/library.py`. Three side-by-side panes show the
//! user's playlists, saved albums, and followed artists. Tab / Shift-Tab cycle
//! pane focus; Enter drills into the selected item. Escape returns from the
//! playlist track-list drill-down to the playlist list.
//!
//! # State source vs Python
//!
//! Textual's `LibraryView` fetched all three sources inside the view and pushed
//! rows into three `DataTable`s; here the view is a pure value and the *runtime*
//! owns the API client (see [`crate::app`]). The main loop fires
//! [`crate::app::AppCommand::FetchLibraryPlaylists`] /
//! `FetchLibraryAlbums` / `FetchLibraryArtists` / `FetchLikedSongs` and folds
//! the replies into this view via [`LibraryView::set_playlists`] etc.; Enter
//! resolves to a [`LibraryAction`] the main loop turns into a command. The
//! three [`PageState`]-backed panes, the focused-pane state machine, and the
//! playlists-pane two-level drill-down live here.
//!
//! # Liked songs (a documented enhancement over Python)
//!
//! The Python `LibraryView` showed only Playlists / Albums / Artists — its
//! `get_liked_songs` API method was never wired into a view. This port surfaces
//! liked songs as a synthetic **"★ Liked Songs"** pseudo-playlist row at the top
//! of the Playlists pane (the spotify_player / YouTube Music convention).
//! Selecting it drills into the liked-songs track list using the same track-list
//! mode as a real playlist, reusing the tracks already in hand (no extra fetch).
//! See [`LibraryView::set_liked_songs`].

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ytmusic_api::{AlbumInfo, ArtistInfo, PlaylistInfo, Track};

use super::{PageState, Theme};
use crate::formatting::format_duration;
use crate::layout::{Orientation, detect_orientation};

// ---------------------------------------------------------------------------
// Pane index
// ---------------------------------------------------------------------------

/// Identifies each of the three library panes.
///
/// Order matches Python's `_PANE_ORDER` (Playlists, Albums, Artists) so the
/// Tab-cycling is identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryPane {
    Playlists,
    Albums,
    Artists,
}

/// The three panes in Tab-cycle order.
const PANE_ORDER: [LibraryPane; 3] = [
    LibraryPane::Playlists,
    LibraryPane::Albums,
    LibraryPane::Artists,
];

impl LibraryPane {
    /// This pane's index in [`PANE_ORDER`].
    fn index(self) -> usize {
        match self {
            LibraryPane::Playlists => 0,
            LibraryPane::Albums => 1,
            LibraryPane::Artists => 2,
        }
    }

    /// The next pane, wrapping (Python `(idx + 1) % len`).
    fn next(self) -> Self {
        PANE_ORDER[(self.index() + 1) % PANE_ORDER.len()]
    }

    /// The previous pane, wrapping (Python `(idx - 1) % len`).
    fn previous(self) -> Self {
        let len = PANE_ORDER.len();
        PANE_ORDER[(self.index() + len - 1) % len]
    }

    /// The pane's title label.
    fn title(self) -> &'static str {
        match self {
            LibraryPane::Playlists => "Playlists",
            LibraryPane::Albums => "Albums",
            LibraryPane::Artists => "Artists",
        }
    }
}

// ---------------------------------------------------------------------------
// LibraryAction — what Enter on a row resolves to
// ---------------------------------------------------------------------------

/// What an Enter keypress on a library row resolves to.
///
/// Returned by [`LibraryView::activate_selected`] so the main loop can translate
/// it into an [`crate::app::AppCommand`]. Mirrors Python's
/// `on_data_table_row_selected` dispatch.
#[derive(Debug, Clone, PartialEq)]
pub enum LibraryAction {
    /// Open a playlist's tracks, reusing the playlist view (Python
    /// `_show_track_list` on the playlists pane). The main loop drives the
    /// existing playlist drill-in flow.
    OpenPlaylist(PlaylistInfo),
    /// Play the playlists-pane track list from `start_index`, queueing the rest
    /// (spotify_player). Returned when Enter lands on a track row in the
    /// drill-down or on the liked-songs list. Python `_handle_playlist_selection`
    /// track branch.
    PlayTracks {
        tracks: Vec<Track>,
        start_index: usize,
    },
    /// Open the selected album (Python `_handle_album_selection` →
    /// `action_open_album`). Deferred to the M5b-2b album view.
    OpenAlbum(AlbumInfo),
    /// Open the selected artist (Python `_handle_artist_selection` →
    /// `action_open_artist`). Deferred to the M5b-2b artist view.
    OpenArtist(ArtistInfo),
}

// ---------------------------------------------------------------------------
// Playlists pane two-level state
// ---------------------------------------------------------------------------

/// The playlists pane is two-level, like the standalone playlist view: the
/// playlist *list* (with the synthetic liked-songs row) or one playlist's
/// *tracks* after a drill-in.
#[derive(Debug, Clone)]
enum PlaylistsLevel {
    /// Level 1: the playlist list (loaded playlists, plus the liked-songs row).
    List(PageState<Vec<PlaylistInfo>>),
    /// Level 2: the tracks of one drilled-in playlist (or liked songs).
    /// `title` labels the list; `state` is `Loading` while a real playlist's
    /// tracks are being fetched, then `Loaded`. Liked songs (already in hand)
    /// go straight to `Loaded`.
    Tracks {
        title: String,
        state: PageState<Vec<Track>>,
    },
}

// ---------------------------------------------------------------------------
// LibraryView
// ---------------------------------------------------------------------------

/// The 3-pane library browser: per-pane fetch state, the active pane, per-pane
/// cursors, and the playlists-pane drill-down level.
#[derive(Debug, Clone)]
pub struct LibraryView {
    /// Level state of the Playlists pane (list ↔ tracks).
    playlists_level: PlaylistsLevel,
    /// The Albums pane data.
    albums: PageState<Vec<AlbumInfo>>,
    /// The Artists pane data.
    artists: PageState<Vec<ArtistInfo>>,
    /// The user's liked songs, surfaced as the synthetic top row of the
    /// Playlists pane (empty until [`set_liked_songs`](Self::set_liked_songs)).
    liked_songs: Vec<Track>,
    /// The pane that currently has the cursor.
    active_pane: LibraryPane,
    /// Per-pane row cursors, indexed by [`LibraryPane::index`].
    cursors: [usize; 3],
}

/// The label of the synthetic liked-songs row at the top of the Playlists pane.
const LIKED_SONGS_LABEL: &str = "★ Liked Songs";

impl Default for LibraryView {
    fn default() -> Self {
        Self::new()
    }
}

impl LibraryView {
    /// A fresh library view: all three panes loading, Playlists focused.
    #[must_use]
    pub fn new() -> Self {
        Self {
            playlists_level: PlaylistsLevel::List(PageState::Loading),
            albums: PageState::Loading,
            artists: PageState::Loading,
            liked_songs: Vec::new(),
            active_pane: LibraryPane::Playlists,
            cursors: [0; 3],
        }
    }

    /// Reset all three panes to the loading state (the moment the user switches
    /// to the library and the four fetches are fired). Mirrors Python's
    /// `_fetch_all_data` showing "Loading library...".
    pub fn set_loading(&mut self) {
        self.playlists_level = PlaylistsLevel::List(PageState::Loading);
        self.albums = PageState::Loading;
        self.artists = PageState::Loading;
        self.liked_songs.clear();
        self.cursors = [0; 3];
    }

    /// The active pane (for the main loop and tests).
    #[must_use]
    pub fn active_pane(&self) -> LibraryPane {
        self.active_pane
    }

    /// Whether the Playlists pane is showing a drilled-in track list (level 2).
    #[must_use]
    pub fn is_viewing_tracks(&self) -> bool {
        matches!(self.playlists_level, PlaylistsLevel::Tracks { .. })
    }

    // -- Data loading (driven by the main loop from AppEvents) -------------

    /// Load the Playlists pane list (`LibraryPlaylistsLoaded`). Returns to the
    /// list level and resets the playlists cursor.
    pub fn set_playlists(&mut self, playlists: Vec<PlaylistInfo>) {
        self.playlists_level = PlaylistsLevel::List(PageState::Loaded(playlists));
        self.cursors[LibraryPane::Playlists.index()] = 0;
    }

    /// Load the Albums pane (`LibraryAlbumsLoaded`).
    pub fn set_albums(&mut self, albums: Vec<AlbumInfo>) {
        self.albums = PageState::Loaded(albums);
        self.cursors[LibraryPane::Albums.index()] = 0;
    }

    /// Load the Artists pane (`LibraryArtistsLoaded`).
    pub fn set_artists(&mut self, artists: Vec<ArtistInfo>) {
        self.artists = PageState::Loaded(artists);
        self.cursors[LibraryPane::Artists.index()] = 0;
    }

    /// Store the liked songs (`LikedSongsLoaded`), surfaced as the synthetic
    /// "★ Liked Songs" row at the top of the Playlists pane.
    pub fn set_liked_songs(&mut self, tracks: Vec<Track>) {
        self.liked_songs = tracks;
    }

    /// Put every pane into the error state with a classified message.
    ///
    /// `AppEvent::ApiError` is flat (no source tag), so a library fetch failure
    /// is shown across the panes that are still loading; the status line carries
    /// the message regardless.
    pub fn set_error(&mut self, message: impl Into<String>) {
        let message = message.into();
        match &mut self.playlists_level {
            PlaylistsLevel::List(state) => *state = PageState::Error(message.clone()),
            PlaylistsLevel::Tracks { state, .. } => *state = PageState::Error(message.clone()),
        }
        self.albums = PageState::Error(message.clone());
        self.artists = PageState::Error(message);
    }

    // -- Pane focus + navigation -------------------------------------------

    /// Cycle focus to the next pane (Tab). Inert while drilled into a track list
    /// (Python `on_key` returned early when `_viewing_tracks`).
    pub fn focus_next_pane(&mut self) {
        if self.is_viewing_tracks() {
            return;
        }
        self.active_pane = self.active_pane.next();
    }

    /// Cycle focus to the previous pane (Shift-Tab). Inert while viewing tracks.
    pub fn focus_previous_pane(&mut self) {
        if self.is_viewing_tracks() {
            return;
        }
        self.active_pane = self.active_pane.previous();
    }

    /// The number of rows in the Playlists pane at its current level (including
    /// the synthetic liked-songs row at the list level when present).
    fn playlists_len(&self) -> usize {
        match &self.playlists_level {
            PlaylistsLevel::List(state) => {
                let real = state.loaded().map_or(0, Vec::len);
                real + usize::from(self.has_liked_row())
            }
            PlaylistsLevel::Tracks { state, .. } => state.loaded().map_or(0, Vec::len),
        }
    }

    /// Whether the synthetic liked-songs row is shown (only at the list level,
    /// and only when there are liked songs to show).
    fn has_liked_row(&self) -> bool {
        matches!(self.playlists_level, PlaylistsLevel::List(_)) && !self.liked_songs.is_empty()
    }

    /// The number of selectable rows in a pane (0 when not loaded).
    fn pane_len(&self, pane: LibraryPane) -> usize {
        match pane {
            LibraryPane::Playlists => self.playlists_len(),
            LibraryPane::Albums => self.albums.loaded().map_or(0, Vec::len),
            LibraryPane::Artists => self.artists.loaded().map_or(0, Vec::len),
        }
    }

    /// Move the cursor down one row in the active pane, clamping at the end.
    pub fn select_next(&mut self) {
        let last = self.pane_len(self.active_pane).saturating_sub(1);
        let cursor = &mut self.cursors[self.active_pane.index()];
        if *cursor < last {
            *cursor += 1;
        }
    }

    /// Move the cursor up one row in the active pane, clamping at the top.
    pub fn select_previous(&mut self) {
        let cursor = &mut self.cursors[self.active_pane.index()];
        *cursor = cursor.saturating_sub(1);
    }

    /// Resolve an Enter keypress on the active pane's selected row into a
    /// [`LibraryAction`], or `None` when nothing is selected.
    ///
    /// Mirrors Python's `on_data_table_row_selected` dispatch. In the Playlists
    /// pane: at the list level a real playlist opens and the liked-songs row
    /// drills into the liked tracks; at the track level Enter plays from the
    /// cursor, queueing the rest.
    #[must_use]
    pub fn activate_selected(&self) -> Option<LibraryAction> {
        match self.active_pane {
            LibraryPane::Playlists => self.activate_playlists(),
            LibraryPane::Albums => {
                let cursor = self.cursors[LibraryPane::Albums.index()];
                self.albums
                    .loaded()?
                    .get(cursor)
                    .map(|a| LibraryAction::OpenAlbum(a.clone()))
            }
            LibraryPane::Artists => {
                let cursor = self.cursors[LibraryPane::Artists.index()];
                self.artists
                    .loaded()?
                    .get(cursor)
                    .map(|a| LibraryAction::OpenArtist(a.clone()))
            }
        }
    }

    /// The item under the cursor in the focused pane as a [`PopupItem`] for the
    /// action popup. The Artists pane and the synthetic "Liked songs" row have
    /// no [`PopupItem`] variant, so they yield `None`.
    #[must_use]
    pub fn selected_popup_item(&self) -> Option<super::popup::PopupItem> {
        match self.active_pane {
            LibraryPane::Playlists => {
                let cursor = self.cursors[LibraryPane::Playlists.index()];
                match &self.playlists_level {
                    PlaylistsLevel::List(state) => {
                        if self.has_liked_row() {
                            if cursor == 0 {
                                return None; // the synthetic liked-songs row
                            }
                            state
                                .loaded()?
                                .get(cursor - 1)
                                .map(|p| super::popup::PopupItem::Playlist(p.clone()))
                        } else {
                            state
                                .loaded()?
                                .get(cursor)
                                .map(|p| super::popup::PopupItem::Playlist(p.clone()))
                        }
                    }
                    PlaylistsLevel::Tracks { state, .. } => state
                        .loaded()?
                        .get(cursor)
                        .map(|t| super::popup::PopupItem::Track(t.clone())),
                }
            }
            LibraryPane::Albums => {
                let cursor = self.cursors[LibraryPane::Albums.index()];
                self.albums
                    .loaded()?
                    .get(cursor)
                    .map(|a| super::popup::PopupItem::Album(a.clone()))
            }
            LibraryPane::Artists => None,
        }
    }

    /// Resolve Enter on the Playlists pane, handling the liked-songs row, real
    /// playlists, and the track drill-down.
    fn activate_playlists(&self) -> Option<LibraryAction> {
        let cursor = self.cursors[LibraryPane::Playlists.index()];
        match &self.playlists_level {
            PlaylistsLevel::List(state) => {
                if self.has_liked_row() {
                    if cursor == 0 {
                        // The synthetic liked-songs row: play the whole liked
                        // list from the top (the main loop opens the track list).
                        return Some(LibraryAction::PlayTracks {
                            tracks: self.liked_songs.clone(),
                            start_index: 0,
                        });
                    }
                    // Real playlists are offset by one for the liked row.
                    let playlist = state.loaded()?.get(cursor - 1)?;
                    Some(LibraryAction::OpenPlaylist(playlist.clone()))
                } else {
                    let playlist = state.loaded()?.get(cursor)?;
                    Some(LibraryAction::OpenPlaylist(playlist.clone()))
                }
            }
            PlaylistsLevel::Tracks { state, .. } => {
                let tracks = state.loaded()?;
                if cursor >= tracks.len() {
                    return None;
                }
                Some(LibraryAction::PlayTracks {
                    tracks: tracks.clone(),
                    start_index: cursor,
                })
            }
        }
    }

    /// Switch the Playlists pane to the track level in the loading state for
    /// `title` (the moment the user drills into a real playlist, before its
    /// tracks arrive). Python `_show_track_list` set the loading status before
    /// the fetch returned. Resets the playlists cursor.
    pub fn show_track_list_loading(&mut self, title: impl Into<String>) {
        self.playlists_level = PlaylistsLevel::Tracks {
            title: title.into(),
            state: PageState::Loading,
        };
        self.cursors[LibraryPane::Playlists.index()] = 0;
    }

    /// Fill the drilled-in track list (`PlaylistTracksLoaded`). Keeps the
    /// current `title` if already at the track level; otherwise adopts `title`.
    pub fn set_tracks(&mut self, title: impl Into<String>, tracks: Vec<Track>) {
        let title = match &self.playlists_level {
            PlaylistsLevel::Tracks { title, .. } => title.clone(),
            PlaylistsLevel::List(_) => title.into(),
        };
        self.playlists_level = PlaylistsLevel::Tracks {
            title,
            state: PageState::Loaded(tracks),
        };
        self.cursors[LibraryPane::Playlists.index()] = 0;
    }

    /// Drill the Playlists pane directly into an already-loaded track list (the
    /// liked-songs row, whose tracks are in hand — no fetch). Resets the cursor.
    pub fn show_tracks(&mut self, title: impl Into<String>, tracks: Vec<Track>) {
        self.playlists_level = PlaylistsLevel::Tracks {
            title: title.into(),
            state: PageState::Loaded(tracks),
        };
        self.cursors[LibraryPane::Playlists.index()] = 0;
    }

    /// Restore the Playlists pane from the track list to the playlist list
    /// (Escape / Python `_restore_playlists_pane`). Returns `true` when it was
    /// viewing tracks and handled the Escape; `false` otherwise (Escape is the
    /// app's go-back at the list level).
    ///
    /// The list is set back to [`PageState::Loading`] so the caller re-fetches
    /// the playlists (matching the standalone playlist view's `go_back`).
    pub fn go_back(&mut self) -> bool {
        if self.is_viewing_tracks() {
            self.playlists_level = PlaylistsLevel::List(PageState::Loading);
            self.cursors[LibraryPane::Playlists.index()] = 0;
            true
        } else {
            false
        }
    }

    // -- Rendering ---------------------------------------------------------

    /// Render the library view into `area`: a status line plus three side-by-side
    /// panes.
    pub fn render(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        let chunks = Layout::vertical([Constraint::Length(1), Constraint::Min(2)]).split(area);
        self.render_status(frame, chunks[0], theme);
        self.render_panes(frame, chunks[1], theme);
    }

    /// Draw the combined-count / track-header status line.
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
    ///
    /// While drilled into tracks it shows the playlist header with the Esc hint
    /// (Python `_populate_tracks`); otherwise the combined counts (Python
    /// `_update_combined_status`), "Loading library..." while any pane loads, or
    /// the classified error message.
    fn status_text(&self) -> (String, bool) {
        if let PlaylistsLevel::Tracks { title, state } = &self.playlists_level {
            return match state {
                PageState::Loading => (format!("Loading tracks for {title}..."), false),
                PageState::Error(msg) => (msg.clone(), true),
                PageState::Loaded(tracks) if tracks.is_empty() => {
                    (format!("{title} - empty playlist [Esc to go back]"), false)
                }
                PageState::Loaded(tracks) => (
                    format!("{title} - {} track(s) [Esc to go back]", tracks.len()),
                    false,
                ),
            };
        }

        // Surface the first errored pane's message, if any. The three panes
        // hold different payload types, so each is checked for its error string
        // separately rather than via one heterogeneous collection.
        let playlists_err = self.playlists_list_state().and_then(|s| error_message(s));
        if let Some(msg) = playlists_err
            .or_else(|| error_message(&self.albums))
            .or_else(|| error_message(&self.artists))
        {
            return (msg.to_owned(), true);
        }

        // Still loading while any pane has not resolved.
        if self.any_pane_loading() {
            return ("Loading library...".to_owned(), false);
        }

        // Combined counts (Python `_update_combined_status`).
        let mut parts: Vec<String> = Vec::new();
        let playlist_count = self
            .playlists_list_state()
            .and_then(PageState::loaded)
            .map_or(0, Vec::len)
            + usize::from(self.has_liked_row());
        if playlist_count > 0 {
            parts.push(format!("{playlist_count} playlist(s)"));
        }
        if let Some(albums) = self.albums.loaded()
            && !albums.is_empty()
        {
            parts.push(format!("{} album(s)", albums.len()));
        }
        if let Some(artists) = self.artists.loaded()
            && !artists.is_empty()
        {
            parts.push(format!("{} artist(s)", artists.len()));
        }
        if parts.is_empty() {
            ("Library empty".to_owned(), false)
        } else {
            (parts.join(" | "), false)
        }
    }

    /// The Playlists pane's list-level state, or `None` while drilled in.
    fn playlists_list_state(&self) -> Option<&PageState<Vec<PlaylistInfo>>> {
        match &self.playlists_level {
            PlaylistsLevel::List(state) => Some(state),
            PlaylistsLevel::Tracks { .. } => None,
        }
    }

    /// Whether any pane is still in the loading state.
    fn any_pane_loading(&self) -> bool {
        let playlists_loading = matches!(self.playlists_list_state(), Some(PageState::Loading));
        playlists_loading
            || matches!(self.albums, PageState::Loading)
            || matches!(self.artists, PageState::Loading)
    }

    /// Draw the three panes responsively (port of spotify_player's layout
    /// switch, consuming [`crate::layout::detect_orientation`]).
    ///
    /// * **Horizontal** (wide terminal): three columns side by side — Playlists
    ///   2fr, Albums 2fr, Artists 1fr (Python's CSS widths).
    /// * **Vertical** (portrait-ish): the three panes stacked in one column so
    ///   each keeps usable width on a narrow terminal.
    fn render_panes(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        let cells = match detect_orientation(area.width, area.height) {
            Orientation::Horizontal => Layout::horizontal([
                Constraint::Ratio(2, 5),
                Constraint::Ratio(2, 5),
                Constraint::Ratio(1, 5),
            ])
            .split(area),
            Orientation::Vertical => Layout::vertical([
                Constraint::Ratio(1, 3),
                Constraint::Ratio(1, 3),
                Constraint::Ratio(1, 3),
            ])
            .split(area),
        };
        self.render_pane(frame, cells[0], theme, LibraryPane::Playlists);
        self.render_pane(frame, cells[1], theme, LibraryPane::Albums);
        self.render_pane(frame, cells[2], theme, LibraryPane::Artists);
    }

    /// Draw one pane as a titled, bordered list with the active pane highlighted.
    fn render_pane(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme, pane: LibraryPane) {
        let is_active = pane == self.active_pane;
        let items = self.pane_items(pane);

        let title_style = if is_active {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text_muted)
        };
        let border_style = if is_active {
            Style::default().fg(theme.accent)
        } else {
            Style::default().fg(theme.primary_background)
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .style(Style::default().bg(theme.surface))
            .title(Span::styled(pane.title(), title_style));

        let list = List::new(items)
            .block(block)
            .style(Style::default().fg(theme.text).bg(theme.surface))
            .highlight_style(super::selected_row_style(theme))
            .highlight_symbol("▶ ");

        let mut list_state = ListState::default();
        if is_active && self.pane_len(pane) > 0 {
            let cursor = self.cursors[pane.index()].min(self.pane_len(pane) - 1);
            list_state.select(Some(cursor));
        }
        frame.render_stateful_widget(list, area, &mut list_state);
    }

    /// Build the list rows for a pane.
    fn pane_items(&self, pane: LibraryPane) -> Vec<ListItem<'static>> {
        match pane {
            LibraryPane::Playlists => self.playlists_items(),
            LibraryPane::Albums => self
                .albums
                .loaded()
                .map(|albums| albums.iter().map(album_row).collect())
                .unwrap_or_default(),
            LibraryPane::Artists => self
                .artists
                .loaded()
                .map(|artists| artists.iter().map(artist_row).collect())
                .unwrap_or_default(),
        }
    }

    /// Build the Playlists pane rows (the liked-songs row + playlists, or the
    /// drilled-in track rows).
    fn playlists_items(&self) -> Vec<ListItem<'static>> {
        match &self.playlists_level {
            PlaylistsLevel::List(state) => {
                let mut items: Vec<ListItem<'static>> = Vec::new();
                if self.has_liked_row() {
                    items.push(liked_songs_row(self.liked_songs.len()));
                }
                if let Some(playlists) = state.loaded() {
                    items.extend(playlists.iter().map(playlist_row));
                }
                items
            }
            PlaylistsLevel::Tracks { state, .. } => state
                .loaded()
                .map(|tracks| tracks.iter().map(track_row).collect())
                .unwrap_or_default(),
        }
    }
}

/// The error message of a pane state, or `None` when it is not errored.
///
/// Generic over the payload `T` so it works uniformly across the three panes'
/// differently-typed [`PageState`]s.
fn error_message<T>(state: &PageState<Vec<T>>) -> Option<&str> {
    match state {
        PageState::Error(msg) => Some(msg),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Row formatters
// ---------------------------------------------------------------------------

/// The synthetic "★ Liked Songs (N)" pseudo-playlist row.
fn liked_songs_row(count: usize) -> ListItem<'static> {
    ListItem::new(Line::from(vec![
        Span::styled(
            LIKED_SONGS_LABEL,
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            format!("{count} tracks"),
            Style::default().add_modifier(Modifier::DIM),
        ),
    ]))
}

/// Format a playlist row: `Title  (N tracks)`.
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

/// Format an album row: `Title — Artist  Year`.
fn album_row(album: &AlbumInfo) -> ListItem<'static> {
    let mut spans = vec![Span::raw(album.title.clone())];
    if !album.artist.is_empty() {
        spans.push(Span::raw(" — "));
        spans.push(Span::raw(album.artist.clone()));
    }
    if !album.year.is_empty() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            album.year.clone(),
            Style::default().add_modifier(Modifier::DIM),
        ));
    }
    ListItem::new(Line::from(spans))
}

/// Format an artist row: just the name.
fn artist_row(artist: &ArtistInfo) -> ListItem<'static> {
    ListItem::new(Line::from(Span::raw(artist.name.clone())))
}

/// Format a track row (the drill-down): `Title — Artist  Duration`.
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

    fn album(id: &str, title: &str, artist: &str, year: &str) -> AlbumInfo {
        AlbumInfo::new_without_tracks(id, title, artist, year, "")
    }

    fn artist(id: &str, name: &str) -> ArtistInfo {
        ArtistInfo::new_minimal(id, name, "")
    }

    fn track(id: &str, title: &str, artist: &str) -> Track {
        Track::new(id, title, artist, "Album", 100.0, "")
    }

    /// A library with all three panes loaded (no liked songs).
    fn loaded_view() -> LibraryView {
        let mut view = LibraryView::new();
        view.set_playlists(vec![
            playlist("PL1", "My Mix", 25),
            playlist("PL2", "Chill", 10),
        ]);
        view.set_albums(vec![album("AL1", "Discovery", "Daft Punk", "2001")]);
        view.set_artists(vec![artist("AR1", "Radiohead")]);
        view
    }

    fn render_to_string(view: &LibraryView, w: u16, h: u16) -> String {
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

    // -- pane focus cycling (port of test_views.py library pane cycling) ----

    #[test]
    fn focus_next_pane_cycles_and_wraps() {
        let mut view = LibraryView::new();
        assert_eq!(view.active_pane(), LibraryPane::Playlists);
        view.focus_next_pane();
        assert_eq!(view.active_pane(), LibraryPane::Albums);
        view.focus_next_pane();
        assert_eq!(view.active_pane(), LibraryPane::Artists);
        view.focus_next_pane();
        assert_eq!(view.active_pane(), LibraryPane::Playlists); // wraps
    }

    #[test]
    fn focus_previous_pane_cycles_in_reverse() {
        let mut view = LibraryView::new();
        assert_eq!(view.active_pane(), LibraryPane::Playlists);
        view.focus_previous_pane();
        assert_eq!(view.active_pane(), LibraryPane::Artists); // wraps to last
        view.focus_previous_pane();
        assert_eq!(view.active_pane(), LibraryPane::Albums);
    }

    #[test]
    fn pane_focus_inert_while_viewing_tracks() {
        // Python returned early from on_key Tab while _viewing_tracks.
        let mut view = loaded_view();
        view.show_tracks("My Mix", vec![track("v1", "A", "X")]);
        assert!(view.is_viewing_tracks());
        view.focus_next_pane();
        assert_eq!(view.active_pane(), LibraryPane::Playlists); // unchanged
    }

    // -- routing: playlists / albums / artists -----------------------------

    #[test]
    fn enter_on_playlist_opens_it() {
        let view = loaded_view();
        match view.activate_selected() {
            Some(LibraryAction::OpenPlaylist(info)) => assert_eq!(info.playlist_id, "PL1"),
            other => panic!("expected OpenPlaylist(PL1), got {other:?}"),
        }
    }

    #[test]
    fn enter_after_moving_opens_right_playlist() {
        let mut view = loaded_view();
        view.select_next();
        match view.activate_selected() {
            Some(LibraryAction::OpenPlaylist(info)) => assert_eq!(info.playlist_id, "PL2"),
            other => panic!("expected OpenPlaylist(PL2), got {other:?}"),
        }
    }

    #[test]
    fn enter_on_album_yields_open_album() {
        let mut view = loaded_view();
        view.focus_next_pane(); // Albums
        match view.activate_selected() {
            Some(LibraryAction::OpenAlbum(a)) => assert_eq!(a.browse_id, "AL1"),
            other => panic!("expected OpenAlbum(AL1), got {other:?}"),
        }
    }

    #[test]
    fn enter_on_artist_yields_open_artist() {
        let mut view = loaded_view();
        view.focus_next_pane(); // Albums
        view.focus_next_pane(); // Artists
        match view.activate_selected() {
            Some(LibraryAction::OpenArtist(a)) => assert_eq!(a.channel_id, "AR1"),
            other => panic!("expected OpenArtist(AR1), got {other:?}"),
        }
    }

    // -- track drill-down --------------------------------------------------

    #[test]
    fn show_tracks_enters_track_level() {
        let mut view = loaded_view();
        view.show_tracks(
            "My Mix",
            vec![track("v1", "First", "A"), track("v2", "Second", "B")],
        );
        assert!(view.is_viewing_tracks());
        assert_eq!(view.pane_len(LibraryPane::Playlists), 2);
    }

    #[test]
    fn enter_on_track_plays_from_index_queueing_rest() {
        let mut view = loaded_view();
        view.show_tracks(
            "My Mix",
            vec![track("v1", "First", "A"), track("v2", "Second", "B")],
        );
        view.select_next(); // cursor on the second track
        match view.activate_selected() {
            Some(LibraryAction::PlayTracks {
                tracks,
                start_index,
            }) => {
                assert_eq!(start_index, 1);
                assert_eq!(tracks.len(), 2);
                assert_eq!(tracks[start_index].video_id, "v2");
            }
            other => panic!("expected PlayTracks, got {other:?}"),
        }
    }

    #[test]
    fn go_back_from_tracks_returns_to_list() {
        let mut view = loaded_view();
        view.show_tracks("My Mix", vec![track("v1", "A", "X")]);
        assert!(view.is_viewing_tracks());
        let handled = view.go_back();
        assert!(handled);
        assert!(!view.is_viewing_tracks());
        assert_eq!(view.cursors[LibraryPane::Playlists.index()], 0);
    }

    #[test]
    fn go_back_from_list_is_not_handled() {
        let mut view = loaded_view();
        assert!(!view.go_back());
        assert!(!view.is_viewing_tracks());
    }

    // -- liked songs synthetic row -----------------------------------------

    #[test]
    fn liked_songs_row_prepends_playlists() {
        let mut view = loaded_view();
        view.set_liked_songs(vec![
            track("l1", "Liked One", "A"),
            track("l2", "Liked Two", "B"),
        ]);
        // Liked row + 2 playlists = 3 rows.
        assert_eq!(view.pane_len(LibraryPane::Playlists), 3);
    }

    #[test]
    fn enter_on_liked_row_plays_liked_songs() {
        let mut view = loaded_view();
        view.set_liked_songs(vec![
            track("l1", "Liked One", "A"),
            track("l2", "Liked Two", "B"),
        ]);
        // Cursor at 0 is the liked-songs row.
        match view.activate_selected() {
            Some(LibraryAction::PlayTracks {
                tracks,
                start_index,
            }) => {
                assert_eq!(start_index, 0);
                assert_eq!(tracks.len(), 2);
                assert_eq!(tracks[0].video_id, "l1");
            }
            other => panic!("expected PlayTracks(liked), got {other:?}"),
        }
    }

    #[test]
    fn real_playlist_offset_by_liked_row() {
        let mut view = loaded_view();
        view.set_liked_songs(vec![track("l1", "Liked", "A")]);
        view.select_next(); // cursor 1 -> first real playlist (PL1)
        match view.activate_selected() {
            Some(LibraryAction::OpenPlaylist(info)) => assert_eq!(info.playlist_id, "PL1"),
            other => panic!("expected OpenPlaylist(PL1), got {other:?}"),
        }
    }

    #[test]
    fn no_liked_row_when_liked_empty() {
        let view = loaded_view(); // no liked songs set
        assert_eq!(view.pane_len(LibraryPane::Playlists), 2);
        match view.activate_selected() {
            Some(LibraryAction::OpenPlaylist(info)) => assert_eq!(info.playlist_id, "PL1"),
            other => panic!("expected OpenPlaylist(PL1), got {other:?}"),
        }
    }

    // -- navigation clamps --------------------------------------------------

    #[test]
    fn select_next_clamps_at_end() {
        let mut view = loaded_view();
        view.select_next();
        view.select_next(); // 2 playlists -> clamps at 1
        match view.activate_selected() {
            Some(LibraryAction::OpenPlaylist(info)) => assert_eq!(info.playlist_id, "PL2"),
            other => panic!("expected OpenPlaylist(PL2), got {other:?}"),
        }
    }

    #[test]
    fn select_previous_clamps_at_top() {
        let mut view = loaded_view();
        view.select_previous();
        assert_eq!(view.cursors[LibraryPane::Playlists.index()], 0);
    }

    #[test]
    fn activate_empty_pane_is_none() {
        let mut view = LibraryView::new();
        view.set_playlists(vec![]);
        view.set_albums(vec![]);
        view.set_artists(vec![]);
        assert!(view.activate_selected().is_none());
    }

    // -- error / loading state ---------------------------------------------

    #[test]
    fn set_error_sets_panes_and_status() {
        let mut view = LibraryView::new();
        view.set_error("Session expired — run: ytmusic-tui auth");
        let (text, is_error) = view.status_text();
        assert!(is_error);
        assert!(text.contains("Session expired"));
    }

    #[test]
    fn loading_status_while_panes_unresolved() {
        let view = LibraryView::new(); // all loading
        let (text, _) = view.status_text();
        assert_eq!(text, "Loading library...");
    }

    #[test]
    fn combined_status_counts_all_panes() {
        let mut view = loaded_view();
        view.set_liked_songs(vec![track("l1", "L", "A")]);
        let (text, is_error) = view.status_text();
        assert!(!is_error);
        // 2 real playlists + 1 liked row = 3.
        assert!(text.contains("3 playlist(s)"), "status: {text}");
        assert!(text.contains("1 album(s)"), "status: {text}");
        assert!(text.contains("1 artist(s)"), "status: {text}");
    }

    // -- rendering (TestBackend) -------------------------------------------

    #[test]
    fn render_shows_three_pane_titles() {
        let view = loaded_view();
        let text = render_to_string(&view, 90, 16);
        assert!(text.contains("Playlists"), "missing Playlists:\n{text}");
        assert!(text.contains("Albums"), "missing Albums:\n{text}");
        assert!(text.contains("Artists"), "missing Artists:\n{text}");
    }

    #[test]
    fn render_shows_pane_content() {
        let view = loaded_view();
        let text = render_to_string(&view, 90, 16);
        assert!(text.contains("My Mix"), "missing playlist:\n{text}");
        assert!(text.contains("Discovery"), "missing album:\n{text}");
        assert!(text.contains("Radiohead"), "missing artist:\n{text}");
    }

    #[test]
    fn render_shows_liked_songs_row() {
        let mut view = loaded_view();
        view.set_liked_songs(vec![track("l1", "L", "A")]);
        let text = render_to_string(&view, 90, 16);
        assert!(text.contains("Liked Songs"), "missing liked row:\n{text}");
    }

    #[test]
    fn render_track_drilldown_shows_back_hint() {
        let mut view = loaded_view();
        view.show_tracks("My Mix", vec![track("v1", "First", "A")]);
        let text = render_to_string(&view, 90, 16);
        assert!(text.contains("First"), "missing track:\n{text}");
        assert!(text.contains("Esc to go back"), "missing hint:\n{text}");
    }

    #[test]
    fn portrait_layout_stacks_three_panes() {
        // A narrow/tall terminal (aspect < 2.3) stacks the three panes; all
        // three titles must still render.
        let view = loaded_view();
        let stacked = render_to_string(&view, 40, 40); // 40/40 = 1.0 → vertical
        assert!(
            stacked.contains("Playlists"),
            "missing Playlists:\n{stacked}"
        );
        assert!(stacked.contains("Albums"), "missing Albums:\n{stacked}");
        assert!(stacked.contains("Artists"), "missing Artists:\n{stacked}");
    }
}
