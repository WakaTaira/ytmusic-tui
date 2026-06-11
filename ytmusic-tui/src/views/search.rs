//! Search view with a 2x2 grid of category panes (spotify_player style).
//!
//! Port of `src/ytmusic_tui/views/search.py`. The layout is a text input row on
//! top, then a 2x2 grid of result panes — Tracks, Albums, Artists, Playlists.
//! Enter in the input runs the search (Enter-confirm, no live suggestions); a
//! `#songs:` / `#albums:` / `#artists:` / `#playlists:` prefix restricts the
//! search to one result type. Tab / Shift-Tab cycle pane focus; Enter on a
//! result row dispatches a pane-specific [`SearchAction`].
//!
//! # State source vs Python
//!
//! Textual's `SearchView` fetched inside the view via `_run_fetch` and pushed
//! rows into four `DataTable`s; here the view is a pure value and the *runtime*
//! owns the API client (see [`crate::app`]). So the view holds a
//! [`PageState`]-backed [`SearchResults`] that the main loop fills from
//! [`crate::app::AppEvent::SearchLoaded`], and Enter resolves to a
//! [`SearchAction`] the main loop turns into a command. The input text buffer
//! and the focused-pane / per-pane cursor state machine live here.
//!
//! # Input mode (the M5b-2a minimal port)
//!
//! Textual used its `Input` widget for the query box. The Rust port keeps a
//! minimal input buffer on the view ([`SearchView::input`]) plus an
//! input-focused flag owned by the main loop's `AppModel`: while the input is
//! focused, printable keys append, Backspace deletes, and Enter submits; the
//! grid keys (Tab / arrows / Enter-on-row) only act once focus has left the
//! input. This mirrors Textual's behavior (the `Input` swallowed keys while
//! focused) without retaining a widget tree.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, TableState};
use ytmusic_api::{AlbumInfo, PlaylistInfo, RelatedArtist, SearchResults, Track};

use super::{PageState, Theme, borderless_table, table_header, table_row};
use crate::formatting::format_duration;
use crate::layout::{Orientation, detect_orientation};

// ---------------------------------------------------------------------------
// Prefix parsing (ported 1:1 from search.py::_parse_search_prefix)
// ---------------------------------------------------------------------------

/// The recognized `#category:` prefixes and the filter name each maps to.
///
/// The filter name (`"songs"` etc.) is exactly what
/// [`crate::app::AppCommand::Search`] forwards to `search_all` as its `filter`.
/// Python derived the category from `prefix[1:-1]` (stripping the leading `#`
/// and trailing `:`); the pairs are spelled out here for clarity.
const CATEGORY_PREFIXES: &[(&str, &str)] = &[
    ("#songs:", "songs"),
    ("#albums:", "albums"),
    ("#artists:", "artists"),
    ("#playlists:", "playlists"),
];

/// Parse an optional `#category:query` prefix.
///
/// Returns `(Some(category), query)` when `raw` starts (case-insensitively)
/// with a known prefix *and* has a non-empty query after trimming; otherwise
/// `(None, raw)` with the original string preserved verbatim (a bare `#songs:`
/// or a mid-string `#songs:` falls through to an all-category search of the
/// literal text). Faithful port of Python's `_parse_search_prefix`.
#[must_use]
pub fn parse_search_prefix(raw: &str) -> (Option<String>, String) {
    let lower = raw.to_lowercase();
    for (prefix, category) in CATEGORY_PREFIXES {
        if lower.starts_with(prefix) {
            // Slice the original (not the lowercased) text after the prefix so
            // the query keeps its original case, then trim surrounding space.
            let query = raw[prefix.len()..].trim();
            if !query.is_empty() {
                return (Some((*category).to_owned()), query.to_owned());
            }
            // Prefix present but empty query: search the raw text as-is.
            return (None, raw.to_owned());
        }
    }
    (None, raw.to_owned())
}

// ---------------------------------------------------------------------------
// Pane index
// ---------------------------------------------------------------------------

/// Identifies each of the four search result panes.
///
/// The discriminant order matches Python's `Pane` IntEnum
/// (TRACKS=0, ALBUMS=1, ARTISTS=2, PLAYLISTS=3) so the Tab-cycling modular
/// arithmetic is identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Tracks,
    Albums,
    Artists,
    Playlists,
}

/// The four panes in Tab-cycle order (the grid is laid out row-major:
/// Tracks, Albums on the top row; Artists, Playlists on the bottom).
const PANE_ORDER: [Pane; 4] = [Pane::Tracks, Pane::Albums, Pane::Artists, Pane::Playlists];

impl Pane {
    /// This pane's index in [`PANE_ORDER`] (its IntEnum value in Python).
    fn index(self) -> usize {
        match self {
            Pane::Tracks => 0,
            Pane::Albums => 1,
            Pane::Artists => 2,
            Pane::Playlists => 3,
        }
    }

    /// The next pane, wrapping after Playlists (Python `(value + 1) % 4`).
    fn next(self) -> Self {
        PANE_ORDER[(self.index() + 1) % PANE_ORDER.len()]
    }

    /// The previous pane, wrapping before Tracks (Python `(value - 1) % 4`).
    fn previous(self) -> Self {
        let len = PANE_ORDER.len();
        PANE_ORDER[(self.index() + len - 1) % len]
    }

    /// The pane's title label.
    fn title(self) -> &'static str {
        match self {
            Pane::Tracks => "Tracks",
            Pane::Albums => "Albums",
            Pane::Artists => "Artists",
            Pane::Playlists => "Playlists",
        }
    }
}

// ---------------------------------------------------------------------------
// SearchAction — what Enter on a result row resolves to
// ---------------------------------------------------------------------------

/// What an Enter keypress on a result row resolves to.
///
/// Returned by [`SearchView::activate_selected`] so the main loop can translate
/// it into an [`crate::app::AppCommand`]. Mirrors Python's
/// `on_data_table_row_selected` dispatch: a track plays; a playlist opens its
/// tracks (reusing the playlist view); an album/artist defers to the M5b-2b
/// album/artist views.
#[derive(Debug, Clone, PartialEq)]
pub enum SearchAction {
    /// Play the selected track (Python `_on_track_selected`).
    PlayTrack(Track),
    /// Open the selected playlist's tracks, reusing the playlist view (Python
    /// `_on_playlist_selected` → `show_track_list`).
    OpenPlaylist(PlaylistInfo),
    /// Open the selected album (Python `_on_album_selected` →
    /// `action_open_album`). The album view is M5b-2b; for now the main loop
    /// shows a deferral status.
    OpenAlbum(AlbumInfo),
    /// Open the selected artist (Python `_on_artist_selected` →
    /// `action_open_artist`). The artist view is M5b-2b; deferred like albums.
    OpenArtist(RelatedArtist),
}

// ---------------------------------------------------------------------------
// SearchView
// ---------------------------------------------------------------------------

/// The search view: a query input buffer, a fetch-state-backed result set, and
/// a `(focused_pane, per-pane cursor)` selection over the four result lists.
///
/// The cursor is kept per pane so switching panes restores each pane's own
/// position (the four Textual `DataTable`s each kept their own cursor).
#[derive(Debug, Clone)]
pub struct SearchView {
    /// The current query input text (the top input row's buffer).
    input: String,
    /// The fetch state of the last search. `Loading` is shown only after a
    /// search is submitted; a fresh view is `Loaded(empty)` so the panes render
    /// empty rather than a spurious "Loading..." before the first search.
    state: PageState<SearchResults>,
    /// The pane that currently has the grid cursor.
    focused_pane: Pane,
    /// Per-pane row cursors, indexed by [`Pane::index`].
    cursors: [usize; 4],
}

impl Default for SearchView {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchView {
    /// A fresh search view: empty input, empty (loaded) results, Tracks focused.
    #[must_use]
    pub fn new() -> Self {
        Self {
            input: String::new(),
            state: PageState::Loaded(SearchResults::default()),
            focused_pane: Pane::Tracks,
            cursors: [0; 4],
        }
    }

    // -- Input buffer (driven by the main loop's input mode) ---------------

    /// The current query input text (for rendering and submission).
    #[must_use]
    pub fn input(&self) -> &str {
        &self.input
    }

    /// Append a printable character to the input buffer (a keypress in input
    /// mode).
    pub fn push_input_char(&mut self, ch: char) {
        self.input.push(ch);
    }

    /// Delete the last character of the input buffer (Backspace in input mode).
    pub fn backspace_input(&mut self) {
        self.input.pop();
    }

    /// Take the trimmed query and its parsed `#category:` filter for submission,
    /// or `None` when the input is blank (Python returned early on an empty
    /// query). The input buffer is left intact so the box still shows the text;
    /// the main loop clears or keeps it as it sees fit.
    #[must_use]
    pub fn submit_query(&self) -> Option<(String, Option<String>)> {
        let query = self.input.trim();
        if query.is_empty() {
            return None;
        }
        let (filter, parsed) = parse_search_prefix(query);
        Some((parsed, filter))
    }

    /// Put the view into the loading state for an in-flight search.
    pub fn set_loading(&mut self) {
        self.state = PageState::Loading;
    }

    /// The currently focused pane (for the main loop and tests).
    #[must_use]
    pub fn focused_pane(&self) -> Pane {
        self.focused_pane
    }

    // -- Results loading (driven by the main loop from AppEvents) ----------

    /// Load a finished search's results and focus the first non-empty pane
    /// (Python `_populate_all_results`). All per-pane cursors reset to the top.
    pub fn set_results(&mut self, results: SearchResults) {
        // Focus the first pane that actually has results, matching Python's
        // `if results.tracks: ... elif results.albums: ...` cascade. When every
        // pane is empty the focus stays on Tracks (Python left it unchanged).
        self.focused_pane = if !results.tracks.is_empty() {
            Pane::Tracks
        } else if !results.albums.is_empty() {
            Pane::Albums
        } else if !results.artists.is_empty() {
            Pane::Artists
        } else if !results.playlists.is_empty() {
            Pane::Playlists
        } else {
            Pane::Tracks
        };
        self.state = PageState::Loaded(results);
        self.cursors = [0; 4];
    }

    /// Put the view into the error state with a classified message.
    pub fn set_error(&mut self, message: impl Into<String>) {
        self.state = PageState::Error(message.into());
    }

    // -- Pane focus + navigation -------------------------------------------

    /// Cycle focus to the next pane (Tab). Python `focus_next_pane`.
    pub fn focus_next_pane(&mut self) {
        self.focused_pane = self.focused_pane.next();
    }

    /// Cycle focus to the previous pane (Shift-Tab). Python `focus_previous_pane`.
    pub fn focus_previous_pane(&mut self) {
        self.focused_pane = self.focused_pane.previous();
    }

    /// The results, or `None` while loading / on error.
    fn results(&self) -> Option<&SearchResults> {
        self.state.loaded()
    }

    /// The number of rows in a given pane (0 when not loaded).
    fn pane_len(&self, pane: Pane) -> usize {
        match self.results() {
            None => 0,
            Some(r) => match pane {
                Pane::Tracks => r.tracks.len(),
                Pane::Albums => r.albums.len(),
                Pane::Artists => r.artists.len(),
                Pane::Playlists => r.playlists.len(),
            },
        }
    }

    /// Move the cursor down one row in the focused pane, clamping at the end.
    pub fn select_next(&mut self) {
        let last = self.pane_len(self.focused_pane).saturating_sub(1);
        let cursor = &mut self.cursors[self.focused_pane.index()];
        if *cursor < last {
            *cursor += 1;
        }
    }

    /// Move the cursor up one row in the focused pane, clamping at the top.
    pub fn select_previous(&mut self) {
        let cursor = &mut self.cursors[self.focused_pane.index()];
        *cursor = cursor.saturating_sub(1);
    }

    /// Resolve an Enter keypress on the focused pane's selected row into a
    /// [`SearchAction`], or `None` when nothing is selected.
    ///
    /// Mirrors Python's `on_data_table_row_selected` dispatch by pane type.
    #[must_use]
    pub fn activate_selected(&self) -> Option<SearchAction> {
        let results = self.results()?;
        let cursor = self.cursors[self.focused_pane.index()];
        match self.focused_pane {
            Pane::Tracks => results
                .tracks
                .get(cursor)
                .map(|t| SearchAction::PlayTrack(t.clone())),
            Pane::Albums => results
                .albums
                .get(cursor)
                .map(|a| SearchAction::OpenAlbum(a.clone())),
            Pane::Artists => results
                .artists
                .get(cursor)
                .map(|a| SearchAction::OpenArtist(a.clone())),
            Pane::Playlists => results
                .playlists
                .get(cursor)
                .map(|p| SearchAction::OpenPlaylist(p.clone())),
        }
    }

    /// The item under the cursor in the focused pane as a [`PopupItem`] for the
    /// action popup. The Artists pane has no [`PopupItem`] variant (artists are
    /// not actionable here), so it yields `None`.
    #[must_use]
    pub fn selected_popup_item(&self) -> Option<super::popup::PopupItem> {
        let results = self.results()?;
        let cursor = self.cursors[self.focused_pane.index()];
        match self.focused_pane {
            Pane::Tracks => results
                .tracks
                .get(cursor)
                .map(|t| super::popup::PopupItem::Track(t.clone())),
            Pane::Albums => results
                .albums
                .get(cursor)
                .map(|a| super::popup::PopupItem::Album(a.clone())),
            Pane::Playlists => results
                .playlists
                .get(cursor)
                .map(|p| super::popup::PopupItem::Playlist(p.clone())),
            Pane::Artists => None,
        }
    }

    // -- Rendering ---------------------------------------------------------

    /// Render the search view into `area`: input row, status line, 2x2 grid.
    ///
    /// `input_focused` styles the input box to show it has keyboard focus (the
    /// main loop owns the flag).
    pub fn render(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme, input_focused: bool) {
        let chunks = Layout::vertical([
            Constraint::Length(3), // bordered input box
            Constraint::Length(1), // status line
            Constraint::Min(2),    // 2x2 grid
        ])
        .split(area);

        self.render_input(frame, chunks[0], theme, input_focused);
        self.render_status(frame, chunks[1], theme);
        self.render_grid(frame, chunks[2], theme);
    }

    /// Draw the bordered input box with the current query (and a cursor caret
    /// when focused).
    fn render_input(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme, focused: bool) {
        let border = if focused {
            Style::default().fg(theme.accent)
        } else {
            Style::default().fg(theme.primary_background)
        };
        let text = if self.input.is_empty() && !focused {
            Span::styled(
                "Search YouTube Music...",
                Style::default().fg(theme.text_muted),
            )
        } else {
            let shown = if focused {
                format!("{}\u{2588}", self.input) // trailing block cursor
            } else {
                self.input.clone()
            };
            Span::styled(shown, Style::default().fg(theme.text))
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border)
            .style(Style::default().bg(theme.surface))
            .title(Span::styled("Search", Style::default().fg(theme.accent)));
        frame.render_widget(Paragraph::new(Line::from(text)).block(block), area);
    }

    /// Draw the result-count / status line (Loading / Error / "N result(s)").
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

    /// Compute the status text and whether it is an error (for styling).
    ///
    /// Mirrors Python's totals: "Searching..." while loading, "N result(s)" or
    /// "No results found" once loaded, and the classified message on error.
    fn status_text(&self) -> (String, bool) {
        match &self.state {
            PageState::Loading => ("Searching...".to_owned(), false),
            PageState::Error(msg) => (msg.clone(), true),
            PageState::Loaded(results) => {
                let total = results.tracks.len()
                    + results.albums.len()
                    + results.artists.len()
                    + results.playlists.len();
                if total == 0 {
                    // A blank input (never searched) shows the same idle hint as
                    // an empty result set; Python only set this after a search,
                    // but an empty grid with this line reads the same.
                    ("No results found".to_owned(), false)
                } else {
                    (format!("{total} result(s)"), false)
                }
            }
        }
    }

    /// Draw the four panes responsively (port of spotify_player's layout switch).
    ///
    /// * **Horizontal** (wide terminal, aspect > 2.3): a 2x2 grid — Tracks,
    ///   Albums on top; Artists, Playlists below.
    /// * **Vertical** (portrait-ish, aspect ≤ 2.3): the four panes stacked in
    ///   one column, so each keeps a usable width on a narrow terminal.
    ///
    /// The orientation comes from [`crate::layout::detect_orientation`] over the
    /// available area, matching Python's `layout.py` consumed by the view.
    fn render_grid(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        match detect_orientation(area.width, area.height) {
            Orientation::Horizontal => {
                let rows =
                    Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)])
                        .split(area);
                let top =
                    Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                        .split(rows[0]);
                let bottom =
                    Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                        .split(rows[1]);
                self.render_pane(frame, top[0], theme, Pane::Tracks);
                self.render_pane(frame, top[1], theme, Pane::Albums);
                self.render_pane(frame, bottom[0], theme, Pane::Artists);
                self.render_pane(frame, bottom[1], theme, Pane::Playlists);
            }
            Orientation::Vertical => {
                let cells = Layout::vertical([
                    Constraint::Percentage(25),
                    Constraint::Percentage(25),
                    Constraint::Percentage(25),
                    Constraint::Percentage(25),
                ])
                .split(area);
                self.render_pane(frame, cells[0], theme, Pane::Tracks);
                self.render_pane(frame, cells[1], theme, Pane::Albums);
                self.render_pane(frame, cells[2], theme, Pane::Artists);
                self.render_pane(frame, cells[3], theme, Pane::Playlists);
            }
        }
    }

    /// Draw one pane: a bordered box (Python `_SearchPane { border: solid }`,
    /// focused `$accent` / un-focused `$primary-background`) with the pane title
    /// on the border, containing a borderless DataTable with that pane's columns
    /// (Tracks: Title/Artist/Album/Duration; Albums: Title/Artist/Year;
    /// Artists: Name; Playlists: Title/Tracks).
    fn render_pane(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme, pane: Pane) {
        let is_active = pane == self.focused_pane;

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
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let (labels, widths) = pane_columns(pane);
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let header = table_header(theme, &label_refs, is_active);
        let rows = self
            .pane_rows(pane)
            .into_iter()
            .map(|cols| table_row(theme, &cols, is_active))
            .collect();
        let table = borderless_table(theme, header, rows, widths, is_active);

        let mut state = TableState::default();
        if is_active && self.pane_len(pane) > 0 {
            let cursor = self.cursors[pane.index()].min(self.pane_len(pane) - 1);
            state.select(Some(cursor));
        }
        frame.render_stateful_widget(table, inner, &mut state);
    }

    /// Build the column rows for a pane from the loaded results (empty otherwise).
    fn pane_rows(&self, pane: Pane) -> Vec<Vec<String>> {
        let Some(results) = self.results() else {
            return Vec::new();
        };
        match pane {
            Pane::Tracks => results.tracks.iter().map(track_columns).collect(),
            Pane::Albums => results.albums.iter().map(album_columns).collect(),
            Pane::Artists => results.artists.iter().map(artist_columns).collect(),
            Pane::Playlists => results.playlists.iter().map(playlist_columns).collect(),
        }
    }
}

// ---------------------------------------------------------------------------
// Column labels / widths / formatters (per Python `_PANE_COLUMNS`)
// ---------------------------------------------------------------------------

/// The column labels and widths for a search pane's DataTable.
///
/// Search panes live in a 2x2 grid, so each is narrow; the columns use
/// proportional ([`Constraint::Percentage`]/[`Constraint::Ratio`]) widths so the
/// Title column always keeps a usable share rather than being starved by
/// fixed-width secondary columns (which `Table` would satisfy first).
fn pane_columns(pane: Pane) -> (Vec<String>, Vec<Constraint>) {
    let owned =
        |labels: &[&str]| -> Vec<String> { labels.iter().map(|s| (*s).to_owned()).collect() };
    match pane {
        Pane::Tracks => (
            owned(&["Title", "Artist", "Album", "Duration"]),
            vec![
                Constraint::Ratio(2, 5),
                Constraint::Ratio(1, 5),
                Constraint::Ratio(1, 5),
                Constraint::Ratio(1, 5),
            ],
        ),
        Pane::Albums => (
            owned(&["Title", "Artist", "Year"]),
            vec![
                Constraint::Ratio(1, 2),
                Constraint::Ratio(1, 3),
                Constraint::Ratio(1, 6),
            ],
        ),
        Pane::Artists => (owned(&["Name"]), vec![Constraint::Percentage(100)]),
        Pane::Playlists => (
            owned(&["Title", "Tracks"]),
            vec![Constraint::Ratio(3, 4), Constraint::Ratio(1, 4)],
        ),
    }
}

/// Track columns: `Title`/`Artist`/`Album`/`Duration`.
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

/// Album columns: `Title`/`Artist`/`Year`.
fn album_columns(album: &AlbumInfo) -> Vec<String> {
    vec![
        album.title.clone(),
        album.artist.clone(),
        album.year.clone(),
    ]
}

/// Artist columns: `Name`.
fn artist_columns(artist: &RelatedArtist) -> Vec<String> {
    vec![artist.name.clone()]
}

/// Playlist columns: `Title`/`Tracks`.
fn playlist_columns(playlist: &PlaylistInfo) -> Vec<String> {
    let count = if playlist.track_count > 0 {
        playlist.track_count.to_string()
    } else {
        String::new()
    };
    vec![playlist.title.clone(), count]
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    // -- prefix parsing (port of test_search_filter.py::TestParseSearchPrefix)

    #[test]
    fn no_prefix() {
        let (cat, query) = parse_search_prefix("hello world");
        assert_eq!(cat, None);
        assert_eq!(query, "hello world");
    }

    #[test]
    fn songs_prefix() {
        let (cat, query) = parse_search_prefix("#songs:lofi beats");
        assert_eq!(cat.as_deref(), Some("songs"));
        assert_eq!(query, "lofi beats");
    }

    #[test]
    fn albums_prefix() {
        let (cat, query) = parse_search_prefix("#albums:dark side of the moon");
        assert_eq!(cat.as_deref(), Some("albums"));
        assert_eq!(query, "dark side of the moon");
    }

    #[test]
    fn artists_prefix() {
        let (cat, query) = parse_search_prefix("#artists:radiohead");
        assert_eq!(cat.as_deref(), Some("artists"));
        assert_eq!(query, "radiohead");
    }

    #[test]
    fn playlists_prefix() {
        let (cat, query) = parse_search_prefix("#playlists:chill vibes");
        assert_eq!(cat.as_deref(), Some("playlists"));
        assert_eq!(query, "chill vibes");
    }

    #[test]
    fn case_insensitive_prefix() {
        let (cat, query) = parse_search_prefix("#SONGS:test");
        assert_eq!(cat.as_deref(), Some("songs"));
        assert_eq!(query, "test");
    }

    #[test]
    fn mixed_case_prefix() {
        let (cat, query) = parse_search_prefix("#Albums:discovery");
        assert_eq!(cat.as_deref(), Some("albums"));
        assert_eq!(query, "discovery");
    }

    #[test]
    fn prefix_without_query_returns_none() {
        let (cat, query) = parse_search_prefix("#songs:");
        assert_eq!(cat, None);
        assert_eq!(query, "#songs:");
    }

    #[test]
    fn prefix_with_whitespace_only_returns_none() {
        let (cat, query) = parse_search_prefix("#songs:   ");
        assert_eq!(cat, None);
        assert_eq!(query, "#songs:   ");
    }

    #[test]
    fn unknown_prefix_ignored() {
        let (cat, query) = parse_search_prefix("#videos:music video");
        assert_eq!(cat, None);
        assert_eq!(query, "#videos:music video");
    }

    #[test]
    fn hash_in_middle_not_treated_as_prefix() {
        let (cat, query) = parse_search_prefix("my #songs:query");
        assert_eq!(cat, None);
        assert_eq!(query, "my #songs:query");
    }

    #[test]
    fn strips_whitespace_after_prefix() {
        let (cat, query) = parse_search_prefix("#artists:   frank ocean  ");
        assert_eq!(cat.as_deref(), Some("artists"));
        assert_eq!(query, "frank ocean");
    }

    #[test]
    fn preserves_original_case_in_query() {
        let (cat, query) = parse_search_prefix("#Songs:The Beatles");
        assert_eq!(cat.as_deref(), Some("songs"));
        assert_eq!(query, "The Beatles");
    }

    #[test]
    fn empty_string_prefix() {
        let (cat, query) = parse_search_prefix("");
        assert_eq!(cat, None);
        assert_eq!(query, "");
    }

    // -- submit_query (the input-handler wiring, port of TestSearchDispatch)

    #[test]
    fn submit_plain_query_searches_all_categories() {
        let mut view = SearchView::new();
        "lofi beats".chars().for_each(|c| view.push_input_char(c));
        assert_eq!(view.submit_query(), Some(("lofi beats".to_owned(), None)));
    }

    #[test]
    fn submit_prefixed_query_restricts_category() {
        let mut view = SearchView::new();
        "#albums:ok computer"
            .chars()
            .for_each(|c| view.push_input_char(c));
        assert_eq!(
            view.submit_query(),
            Some(("ok computer".to_owned(), Some("albums".to_owned())))
        );
    }

    #[test]
    fn submit_songs_prefix_dispatches_songs() {
        let mut view = SearchView::new();
        "#songs:rick astley"
            .chars()
            .for_each(|c| view.push_input_char(c));
        assert_eq!(
            view.submit_query(),
            Some(("rick astley".to_owned(), Some("songs".to_owned())))
        );
    }

    #[test]
    fn submit_empty_input_does_not_search() {
        let mut view = SearchView::new();
        "   ".chars().for_each(|c| view.push_input_char(c));
        assert_eq!(view.submit_query(), None);
    }

    #[test]
    fn submit_prefix_without_query_falls_back_to_full_search() {
        let mut view = SearchView::new();
        "#songs:".chars().for_each(|c| view.push_input_char(c));
        assert_eq!(view.submit_query(), Some(("#songs:".to_owned(), None)));
    }

    // -- input buffer editing ----------------------------------------------

    #[test]
    fn backspace_removes_last_char() {
        let mut view = SearchView::new();
        "abc".chars().for_each(|c| view.push_input_char(c));
        view.backspace_input();
        assert_eq!(view.input(), "ab");
        view.backspace_input();
        view.backspace_input();
        view.backspace_input(); // extra backspace on empty is a no-op
        assert_eq!(view.input(), "");
    }

    // -- pane focus cycling (port of test_views.py focus_next/previous_pane)

    #[test]
    fn focus_next_pane_cycles_and_wraps() {
        let mut view = SearchView::new();
        assert_eq!(view.focused_pane(), Pane::Tracks);
        view.focus_next_pane();
        assert_eq!(view.focused_pane(), Pane::Albums);
        view.focus_next_pane();
        assert_eq!(view.focused_pane(), Pane::Artists);
        view.focus_next_pane();
        assert_eq!(view.focused_pane(), Pane::Playlists);
        view.focus_next_pane();
        assert_eq!(view.focused_pane(), Pane::Tracks); // wraps
    }

    #[test]
    fn focus_previous_pane_cycles_in_reverse() {
        let mut view = SearchView::new();
        assert_eq!(view.focused_pane(), Pane::Tracks);
        view.focus_previous_pane();
        assert_eq!(view.focused_pane(), Pane::Playlists); // wraps to last
        view.focus_previous_pane();
        assert_eq!(view.focused_pane(), Pane::Artists);
    }

    // -- result loading + focus-first-non-empty pane -----------------------

    fn track(id: &str, title: &str, artist: &str) -> Track {
        Track::new(id, title, artist, "Album", 100.0, "")
    }

    fn results_with(
        tracks: Vec<Track>,
        albums: Vec<AlbumInfo>,
        artists: Vec<RelatedArtist>,
        playlists: Vec<PlaylistInfo>,
    ) -> SearchResults {
        SearchResults {
            tracks,
            albums,
            artists,
            playlists,
        }
    }

    #[test]
    fn set_results_focuses_first_non_empty_pane() {
        // Only albums present -> Albums pane focused.
        let mut view = SearchView::new();
        view.focus_next_pane(); // move off Tracks first
        view.set_results(results_with(
            vec![],
            vec![AlbumInfo::new_without_tracks(
                "b1",
                "OK Computer",
                "Radiohead",
                "1997",
                "",
            )],
            vec![],
            vec![],
        ));
        assert_eq!(view.focused_pane(), Pane::Albums);
    }

    #[test]
    fn set_results_prefers_tracks_when_present() {
        let mut view = SearchView::new();
        view.set_results(results_with(
            vec![track("v1", "Song", "Band")],
            vec![AlbumInfo::new_without_tracks("b1", "Al", "Ar", "2020", "")],
            vec![],
            vec![],
        ));
        assert_eq!(view.focused_pane(), Pane::Tracks);
    }

    #[test]
    fn set_results_resets_cursors() {
        let mut view = SearchView::new();
        view.set_results(results_with(
            vec![track("v1", "A", "X"), track("v2", "B", "Y")],
            vec![],
            vec![],
            vec![],
        ));
        view.select_next();
        // Re-loading resets the cursor back to the top.
        view.set_results(results_with(
            vec![track("v3", "C", "Z")],
            vec![],
            vec![],
            vec![],
        ));
        // Activating selects the first (and only) track.
        match view.activate_selected() {
            Some(SearchAction::PlayTrack(t)) => assert_eq!(t.video_id, "v3"),
            other => panic!("expected PlayTrack(v3), got {other:?}"),
        }
    }

    // -- navigation + activation routing -----------------------------------

    #[test]
    fn select_next_clamps_at_end_of_pane() {
        let mut view = SearchView::new();
        view.set_results(results_with(
            vec![track("v1", "A", "X"), track("v2", "B", "Y")],
            vec![],
            vec![],
            vec![],
        ));
        view.select_next();
        view.select_next(); // would be index 2 -> clamps at 1
        match view.activate_selected() {
            Some(SearchAction::PlayTrack(t)) => assert_eq!(t.video_id, "v2"),
            other => panic!("expected PlayTrack(v2), got {other:?}"),
        }
    }

    #[test]
    fn enter_on_track_yields_play() {
        let mut view = SearchView::new();
        view.set_results(results_with(
            vec![track("v1", "Song", "Band")],
            vec![],
            vec![],
            vec![],
        ));
        match view.activate_selected() {
            Some(SearchAction::PlayTrack(t)) => assert_eq!(t.video_id, "v1"),
            other => panic!("expected PlayTrack, got {other:?}"),
        }
    }

    #[test]
    fn enter_on_album_yields_open_album() {
        let mut view = SearchView::new();
        view.set_results(results_with(
            vec![],
            vec![AlbumInfo::new_without_tracks(
                "b1",
                "OK Computer",
                "Radiohead",
                "1997",
                "",
            )],
            vec![],
            vec![],
        ));
        match view.activate_selected() {
            Some(SearchAction::OpenAlbum(a)) => assert_eq!(a.browse_id, "b1"),
            other => panic!("expected OpenAlbum(b1), got {other:?}"),
        }
    }

    #[test]
    fn enter_on_artist_yields_open_artist() {
        let mut view = SearchView::new();
        view.set_results(results_with(
            vec![],
            vec![],
            vec![RelatedArtist::new("c1", "Radiohead", "")],
            vec![],
        ));
        match view.activate_selected() {
            Some(SearchAction::OpenArtist(a)) => assert_eq!(a.channel_id, "c1"),
            other => panic!("expected OpenArtist(c1), got {other:?}"),
        }
    }

    #[test]
    fn enter_on_playlist_yields_open_playlist() {
        let mut view = SearchView::new();
        view.set_results(results_with(
            vec![],
            vec![],
            vec![],
            vec![PlaylistInfo::new("PL1", "Chill", "", 12, "")],
        ));
        match view.activate_selected() {
            Some(SearchAction::OpenPlaylist(p)) => assert_eq!(p.playlist_id, "PL1"),
            other => panic!("expected OpenPlaylist(PL1), got {other:?}"),
        }
    }

    #[test]
    fn activate_empty_pane_is_none() {
        let view = SearchView::new(); // empty loaded results
        assert!(view.activate_selected().is_none());
    }

    #[test]
    fn out_of_range_cursor_does_not_panic() {
        // Loaded with one track, but the focused pane (Albums) is empty: a stale
        // cursor must not index out of bounds.
        let mut view = SearchView::new();
        view.set_results(results_with(
            vec![track("v1", "A", "X")],
            vec![],
            vec![],
            vec![],
        ));
        view.focus_next_pane(); // Albums (empty)
        assert!(view.activate_selected().is_none());
    }

    // -- rendering (TestBackend) -------------------------------------------

    fn render_to_string(view: &SearchView, w: u16, h: u16, input_focused: bool) -> String {
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = Theme::default();
        terminal
            .draw(|frame| view.render(frame, frame.area(), &theme, input_focused))
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
    fn render_shows_four_pane_titles() {
        let view = SearchView::new();
        let text = render_to_string(&view, 70, 20, false);
        assert!(text.contains("Tracks"), "missing Tracks pane:\n{text}");
        assert!(text.contains("Albums"), "missing Albums pane:\n{text}");
        assert!(text.contains("Artists"), "missing Artists pane:\n{text}");
        assert!(
            text.contains("Playlists"),
            "missing Playlists pane:\n{text}"
        );
    }

    #[test]
    fn portrait_layout_stacks_all_four_panes() {
        // A narrow/tall terminal (aspect < 2.3) stacks the four panes; all four
        // titles must still render.
        let view = SearchView::new();
        let text = render_to_string(&view, 40, 40, false); // 40/40 = 1.0 → vertical
        assert!(text.contains("Tracks"), "missing Tracks pane:\n{text}");
        assert!(text.contains("Albums"), "missing Albums pane:\n{text}");
        assert!(text.contains("Artists"), "missing Artists pane:\n{text}");
        assert!(
            text.contains("Playlists"),
            "missing Playlists pane:\n{text}"
        );
    }

    #[test]
    fn render_shows_input_placeholder_when_unfocused_and_empty() {
        let view = SearchView::new();
        let text = render_to_string(&view, 70, 20, false);
        assert!(
            text.contains("Search YouTube Music"),
            "missing placeholder:\n{text}"
        );
    }

    #[test]
    fn render_shows_typed_input() {
        let mut view = SearchView::new();
        "daft punk".chars().for_each(|c| view.push_input_char(c));
        let text = render_to_string(&view, 70, 20, true);
        assert!(text.contains("daft punk"), "missing typed query:\n{text}");
    }

    #[test]
    fn render_shows_results_and_count() {
        let mut view = SearchView::new();
        view.set_results(results_with(
            vec![track("v1", "Get Lucky", "Daft Punk")],
            vec![AlbumInfo::new_without_tracks(
                "b1",
                "Discovery",
                "Daft Punk",
                "2001",
                "",
            )],
            vec![],
            vec![],
        ));
        let text = render_to_string(&view, 70, 20, false);
        assert!(text.contains("Get Lucky"), "missing track:\n{text}");
        assert!(text.contains("Discovery"), "missing album:\n{text}");
        assert!(text.contains("2 result(s)"), "missing count:\n{text}");
    }

    #[test]
    fn render_loading_shows_searching() {
        let mut view = SearchView::new();
        view.set_loading();
        let text = render_to_string(&view, 60, 20, false);
        assert!(text.contains("Searching..."), "missing loading:\n{text}");
    }

    #[test]
    fn render_error_shows_message() {
        let mut view = SearchView::new();
        view.set_error("Network down");
        let text = render_to_string(&view, 60, 20, false);
        assert!(text.contains("Network down"), "missing error:\n{text}");
    }
}
