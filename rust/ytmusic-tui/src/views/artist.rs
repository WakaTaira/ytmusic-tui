//! Artist page view: top songs, albums, and related artists.
//!
//! Port of `src/ytmusic_tui/views/artist.py`. Three vertically-stacked
//! sections (Top Songs / Albums / Related Artists) with Tab cycling between
//! them. Enter semantics per section:
//!
//! * **Top Songs** — play the single selected track (`Play(track)`, **not**
//!   queue-album-rest — Python `_handle_song_selection` called
//!   `set_playlist([track], start_index=0)`, i.e. a single-track queue).
//! * **Albums** — open the album view (`OpenAlbum`).
//! * **Related Artists** — open that artist's page (`OpenArtist`, recursive).
//!
//! # Fetch flow vs Python
//!
//! Python's `ArtistView.load_artist(channel_id)` ran a Textual worker. Here
//! the view is a pure value; the main loop issues
//! [`crate::app::AppCommand::FetchArtist`] and folds
//! [`crate::app::AppEvent::ArtistLoaded`] back.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Row, TableState};
use ytmusic_api::{AlbumInfo, ArtistInfo, RelatedArtist, Track};

use super::{PageState, Theme, borderless_table, section_title, table_header, table_row};
use crate::formatting::format_duration;

// ---------------------------------------------------------------------------
// Section index
// ---------------------------------------------------------------------------

/// The three artist-page sections in Tab-cycle order.
///
/// Mirrors Python's three Textual `DataTable`s (artist-top-songs,
/// artist-albums, artist-related); Tab focuses the next table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtistSection {
    TopSongs,
    Albums,
    RelatedArtists,
}

const SECTION_ORDER: [ArtistSection; 3] = [
    ArtistSection::TopSongs,
    ArtistSection::Albums,
    ArtistSection::RelatedArtists,
];

impl ArtistSection {
    fn index(self) -> usize {
        match self {
            ArtistSection::TopSongs => 0,
            ArtistSection::Albums => 1,
            ArtistSection::RelatedArtists => 2,
        }
    }

    fn next(self) -> Self {
        let len = SECTION_ORDER.len();
        SECTION_ORDER[(self.index() + 1) % len]
    }

    fn previous(self) -> Self {
        let len = SECTION_ORDER.len();
        SECTION_ORDER[(self.index() + len - 1) % len]
    }

    fn title(self) -> &'static str {
        match self {
            ArtistSection::TopSongs => "Top Songs",
            ArtistSection::Albums => "Albums",
            ArtistSection::RelatedArtists => "Related Artists",
        }
    }
}

// ---------------------------------------------------------------------------
// ArtistAction
// ---------------------------------------------------------------------------

/// What an Enter keypress on the artist view resolves to.
///
/// Mirrors Python's `on_data_table_row_selected` dispatch:
/// * Top Songs → play the single track (Python `set_playlist([track], 0)`).
/// * Albums → open the album view.
/// * Related Artists → open that artist's page (recursive navigation).
#[derive(Debug, Clone, PartialEq)]
pub enum ArtistAction {
    /// Play the selected top song (single-track queue).
    PlayTrack(Track),
    /// Open the selected album (Python `action_open_album`).
    OpenAlbum(AlbumInfo),
    /// Open the selected related artist (Python `action_open_artist`).
    OpenArtist(RelatedArtist),
}

// ---------------------------------------------------------------------------
// ArtistView
// ---------------------------------------------------------------------------

/// The artist page view: fetch state, focused section, and per-section
/// cursors.
#[derive(Debug, Clone)]
pub struct ArtistView {
    /// Fetch state; the payload is the loaded artist data.
    state: PageState<ArtistInfo>,
    /// Which section currently has focus.
    focused: ArtistSection,
    /// Per-section row cursors, indexed by [`ArtistSection::index`].
    cursors: [usize; 3],
}

impl Default for ArtistView {
    fn default() -> Self {
        Self::new()
    }
}

impl ArtistView {
    /// A fresh artist view in the loading state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: PageState::Loading,
            focused: ArtistSection::TopSongs,
            cursors: [0; 3],
        }
    }

    /// The current fetch state (for tests and the main loop).
    #[must_use]
    pub fn state(&self) -> &PageState<ArtistInfo> {
        &self.state
    }

    // -- Data loading --------------------------------------------------------

    /// Load the artist data and reset focus + cursors.
    pub fn set_artist(&mut self, artist: ArtistInfo) {
        self.state = PageState::Loaded(artist);
        self.focused = ArtistSection::TopSongs;
        self.cursors = [0; 3];
    }

    /// Transition into the error state.
    pub fn set_error(&mut self, message: impl Into<String>) {
        self.state = PageState::Error(message.into());
    }

    /// Reset to loading (new artist fetch initiated).
    pub fn set_loading(&mut self) {
        self.state = PageState::Loading;
        self.focused = ArtistSection::TopSongs;
        self.cursors = [0; 3];
    }

    // -- Section navigation (Tab / Shift-Tab) --------------------------------

    /// Move focus to the next section (Tab). Mirrors Python's `NavDataTable`
    /// j/k focus cycle on the three tables.
    pub fn focus_next_section(&mut self) {
        self.focused = self.focused.next();
    }

    /// Move focus to the previous section (Shift-Tab).
    pub fn focus_previous_section(&mut self) {
        self.focused = self.focused.previous();
    }

    /// The currently focused section (for tests and rendering).
    #[must_use]
    pub fn focused_section(&self) -> ArtistSection {
        self.focused
    }

    // -- Row navigation (Up/Down within section) -----------------------------

    fn section_len(&self, section: ArtistSection) -> usize {
        match self.state.loaded() {
            None => 0,
            Some(a) => match section {
                ArtistSection::TopSongs => a.top_songs.len(),
                ArtistSection::Albums => a.albums.len(),
                ArtistSection::RelatedArtists => a.related_artists.len(),
            },
        }
    }

    /// Move the cursor down one row in the focused section, clamping at the
    /// end.
    pub fn select_next(&mut self) {
        let last = self.section_len(self.focused).saturating_sub(1);
        let cursor = &mut self.cursors[self.focused.index()];
        if *cursor < last {
            *cursor += 1;
        }
    }

    /// Move the cursor up one row in the focused section, clamping at the
    /// top.
    pub fn select_previous(&mut self) {
        let cursor = &mut self.cursors[self.focused.index()];
        *cursor = cursor.saturating_sub(1);
    }

    // -- Activation (Enter) --------------------------------------------------

    /// Resolve an Enter keypress into an [`ArtistAction`], or `None` when
    /// nothing is selected. Dispatches by focused section.
    ///
    /// Top Songs: play the single track (Python `_handle_song_selection` uses
    /// `set_playlist([track], start_index=0)` — a single-item queue, not
    /// album-rest semantics). Albums: open the album view. Related Artists:
    /// open the artist view (recursive nav).
    #[must_use]
    pub fn activate_selected(&self) -> Option<ArtistAction> {
        let artist = self.state.loaded()?;
        let cursor = self.cursors[self.focused.index()];
        match self.focused {
            ArtistSection::TopSongs => artist
                .top_songs
                .get(cursor)
                .map(|t| ArtistAction::PlayTrack(t.clone())),
            ArtistSection::Albums => artist
                .albums
                .get(cursor)
                .map(|a| ArtistAction::OpenAlbum(a.clone())),
            ArtistSection::RelatedArtists => artist
                .related_artists
                .get(cursor)
                .map(|r| ArtistAction::OpenArtist(r.clone())),
        }
    }

    /// The item under the cursor in the focused section as a [`PopupItem`] for
    /// the action popup: a top-song track, or an album. The Related Artists
    /// section has no [`PopupItem`] variant and yields `None`.
    #[must_use]
    pub fn selected_popup_item(&self) -> Option<super::popup::PopupItem> {
        let artist = self.state.loaded()?;
        let cursor = self.cursors[self.focused.index()];
        match self.focused {
            ArtistSection::TopSongs => artist
                .top_songs
                .get(cursor)
                .map(|t| super::popup::PopupItem::Track(t.clone())),
            ArtistSection::Albums => artist
                .albums
                .get(cursor)
                .map(|a| super::popup::PopupItem::Album(a.clone())),
            ArtistSection::RelatedArtists => None,
        }
    }

    // -- Rendering -----------------------------------------------------------

    /// Render the artist view into `area`.
    ///
    /// An accent artist-name line + muted status line, over three borderless
    /// DataTable sections (Top Songs / Albums / Related Artists) stacked
    /// vertically, each with a section-label line and focused styling on the
    /// active section (no "Artist" panel border — flat `surface`).
    pub fn render(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        // Name (1 row) + status (1 row) + three sections.
        let chunks = Layout::vertical([
            Constraint::Length(1), // artist name
            Constraint::Length(1), // status
            Constraint::Min(1),    // three-section body
        ])
        .split(area);

        self.render_name(frame, chunks[0], theme);
        self.render_status(frame, chunks[1], theme);
        self.render_body(frame, chunks[2], theme);
    }

    fn render_name(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        let name = self.state.loaded().map(|a| a.name.as_str()).unwrap_or("");
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                name.to_owned(),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            )))
            .style(Style::default().bg(theme.surface)),
            area,
        );
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
            PageState::Loading => ("Loading artist...".to_owned(), false),
            PageState::Error(msg) => (msg.clone(), true),
            PageState::Loaded(_) => ("[Esc to go back]".to_owned(), false),
        }
    }

    fn render_body(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        let PageState::Loaded(artist) = &self.state else {
            return; // loading / error: status line carries the message
        };

        // Three vertical sections stacked. Each: 1 label row + 1 header row +
        // up to SECTION_CAP item rows.
        let song_h = section_height(artist.top_songs.len());
        let album_h = section_height(artist.albums.len());
        let related_h = section_height(artist.related_artists.len());

        let chunks = Layout::vertical([
            Constraint::Length(song_h),
            Constraint::Length(album_h),
            Constraint::Length(related_h),
        ])
        .split(area);

        let song_focus = self.focused == ArtistSection::TopSongs;
        self.render_section(
            frame,
            chunks[0],
            theme,
            ArtistSection::TopSongs,
            table_header(theme, &["Title", "Album", "Duration"], song_focus),
            artist
                .top_songs
                .iter()
                .map(|t| table_row(theme, &song_columns(t), song_focus))
                .collect(),
            vec![
                Constraint::Min(10),
                Constraint::Length(24),
                Constraint::Length(10),
            ],
        );

        let album_focus = self.focused == ArtistSection::Albums;
        self.render_section(
            frame,
            chunks[1],
            theme,
            ArtistSection::Albums,
            table_header(theme, &["Title", "Year"], album_focus),
            artist
                .albums
                .iter()
                .map(|a| table_row(theme, &album_columns(a), album_focus))
                .collect(),
            vec![Constraint::Min(10), Constraint::Length(6)],
        );

        let related_focus = self.focused == ArtistSection::RelatedArtists;
        self.render_section(
            frame,
            chunks[2],
            theme,
            ArtistSection::RelatedArtists,
            table_header(theme, &["Name"], related_focus),
            artist
                .related_artists
                .iter()
                .map(|r| table_row(theme, &related_columns(r), related_focus))
                .collect(),
            vec![Constraint::Min(6)],
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn render_section(
        &self,
        frame: &mut Frame<'_>,
        area: Rect,
        theme: &Theme,
        section: ArtistSection,
        header: Row<'static>,
        rows: Vec<Row<'static>>,
        widths: Vec<Constraint>,
    ) {
        if area.height == 0 {
            return;
        }
        let is_active = section == self.focused;
        let [title_area, table_area] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);

        // Section label (accent when focused, muted otherwise — the Python
        // `.section-label { color: $accent }`, dimmed for the inactive ones).
        frame.render_widget(
            Paragraph::new(Line::from(section_title(theme, section.title(), is_active)))
                .style(Style::default().bg(theme.surface)),
            title_area,
        );

        let table = borderless_table(theme, header, rows, widths, is_active);
        let mut state = TableState::default();
        if is_active && self.section_len(section) > 0 {
            let cursor = self.cursors[section.index()].min(self.section_len(section) - 1);
            state.select(Some(cursor));
        }
        frame.render_stateful_widget(table, table_area, &mut state);
    }
}

/// Maximum item rows per section before it stops growing (mirrors Python's
/// `max-height: 15` on `ArtistView DataTable`).
const SECTION_ITEM_CAP: u16 = 15;

fn section_height(item_count: usize) -> u16 {
    let items = item_count.min(SECTION_ITEM_CAP as usize) as u16;
    // +1 for the section label, +1 for the column header row.
    items.saturating_add(2)
}

/// Top-songs columns: `Title`/`Album`/`Duration`.
fn song_columns(track: &Track) -> Vec<String> {
    let duration = format_duration(track.duration_seconds);
    let duration = if duration == "—" {
        String::new()
    } else {
        duration
    };
    vec![track.title.clone(), track.album.clone(), duration]
}

/// Albums columns: `Title`/`Year`.
fn album_columns(album: &AlbumInfo) -> Vec<String> {
    vec![album.title.clone(), album.year.clone()]
}

/// Related-artists columns: `Name`.
fn related_columns(artist: &RelatedArtist) -> Vec<String> {
    vec![artist.name.clone()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    // -- fixtures ------------------------------------------------------------

    fn make_track(id: &str, title: &str) -> Track {
        Track::new(id, title, "Radiohead", "OK Computer", 180.0, "")
    }

    fn make_album(id: &str, title: &str) -> AlbumInfo {
        AlbumInfo::new_without_tracks(id, title, "Radiohead", "1997", "")
    }

    fn make_related(id: &str, name: &str) -> RelatedArtist {
        RelatedArtist::new(id, name, "")
    }

    fn loaded_view() -> ArtistView {
        let mut view = ArtistView::new();
        view.set_artist(ArtistInfo::new(
            "ch1",
            "Radiohead",
            "",
            vec![make_track("v1", "Karma Police"), make_track("v2", "Creep")],
            vec![
                make_album("b1", "OK Computer"),
                make_album("b2", "The Bends"),
            ],
            vec![make_related("c2", "Thom Yorke"), make_related("c3", "Beck")],
            "",
        ));
        view
    }

    fn render_to_string(view: &ArtistView, w: u16, h: u16) -> String {
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
        let view = ArtistView::new();
        assert!(matches!(view.state(), PageState::Loading));
        let text = render_to_string(&view, 60, 10);
        assert!(text.contains("Loading artist..."), "text:\n{text}");
    }

    // -- data loading --------------------------------------------------------

    #[test]
    fn set_artist_loads_data_and_resets_focus() {
        let view = loaded_view();
        assert!(matches!(view.state(), PageState::Loaded(_)));
        assert_eq!(view.focused_section(), ArtistSection::TopSongs);
        assert_eq!(view.cursors, [0; 3]);
    }

    // -- section navigation (Tab) --------------------------------------------

    #[test]
    fn focus_next_section_cycles_through_all_three() {
        let mut view = loaded_view();
        assert_eq!(view.focused_section(), ArtistSection::TopSongs);
        view.focus_next_section();
        assert_eq!(view.focused_section(), ArtistSection::Albums);
        view.focus_next_section();
        assert_eq!(view.focused_section(), ArtistSection::RelatedArtists);
        view.focus_next_section();
        assert_eq!(view.focused_section(), ArtistSection::TopSongs); // wraps
    }

    #[test]
    fn focus_previous_section_wraps_from_first_to_last() {
        let mut view = loaded_view();
        view.focus_previous_section();
        assert_eq!(view.focused_section(), ArtistSection::RelatedArtists);
    }

    // -- within-section navigation -------------------------------------------

    #[test]
    fn select_next_clamps_at_last_item_in_section() {
        let mut view = loaded_view(); // 2 top songs
        view.select_next();
        view.select_next(); // would be index 2 -> clamps at 1
        assert_eq!(view.cursors[ArtistSection::TopSongs.index()], 1);
    }

    #[test]
    fn select_previous_clamps_at_top() {
        let mut view = loaded_view();
        view.select_previous();
        assert_eq!(view.cursors[ArtistSection::TopSongs.index()], 0);
    }

    #[test]
    fn navigation_is_independent_per_section() {
        let mut view = loaded_view();
        view.select_next(); // songs cursor -> 1
        view.focus_next_section(); // now Albums
        view.select_next(); // albums cursor -> 1
        assert_eq!(view.cursors[ArtistSection::TopSongs.index()], 1);
        assert_eq!(view.cursors[ArtistSection::Albums.index()], 1);
    }

    // -- activation (Enter) --------------------------------------------------

    #[test]
    fn enter_on_top_song_yields_play_track() {
        let view = loaded_view();
        match view.activate_selected() {
            Some(ArtistAction::PlayTrack(t)) => assert_eq!(t.video_id, "v1"),
            other => panic!("expected PlayTrack(v1), got {other:?}"),
        }
    }

    #[test]
    fn enter_on_album_yields_open_album() {
        let mut view = loaded_view();
        view.focus_next_section(); // Albums
        match view.activate_selected() {
            Some(ArtistAction::OpenAlbum(a)) => assert_eq!(a.browse_id, "b1"),
            other => panic!("expected OpenAlbum(b1), got {other:?}"),
        }
    }

    #[test]
    fn enter_on_related_artist_yields_open_artist() {
        let mut view = loaded_view();
        view.focus_next_section(); // Albums
        view.focus_next_section(); // Related Artists
        match view.activate_selected() {
            Some(ArtistAction::OpenArtist(r)) => assert_eq!(r.channel_id, "c2"),
            other => panic!("expected OpenArtist(c2), got {other:?}"),
        }
    }

    #[test]
    fn enter_after_moving_to_second_track_selects_it() {
        let mut view = loaded_view();
        view.select_next();
        match view.activate_selected() {
            Some(ArtistAction::PlayTrack(t)) => assert_eq!(t.video_id, "v2"),
            other => panic!("expected PlayTrack(v2), got {other:?}"),
        }
    }

    #[test]
    fn activate_on_empty_section_is_none() {
        let mut view = ArtistView::new();
        // Artist with no top songs.
        view.set_artist(ArtistInfo::new(
            "ch1",
            "Someone",
            "",
            vec![],
            vec![make_album("b1", "Album")],
            vec![],
            "",
        ));
        assert!(view.activate_selected().is_none());
    }

    // -- error state ---------------------------------------------------------

    #[test]
    fn set_error_renders_message() {
        let mut view = ArtistView::new();
        view.set_error("Network error");
        let text = render_to_string(&view, 60, 10);
        assert!(text.contains("Network error"), "text:\n{text}");
    }

    // -- rendering (TestBackend) ---------------------------------------------

    #[test]
    fn loaded_render_shows_artist_name_and_all_sections() {
        let view = loaded_view();
        let text = render_to_string(&view, 70, 20);
        assert!(text.contains("Radiohead"), "missing artist name:\n{text}");
        assert!(text.contains("Top Songs"), "missing Top Songs:\n{text}");
        assert!(text.contains("Albums"), "missing Albums:\n{text}");
        assert!(text.contains("Related Artists"), "missing Related:\n{text}");
        assert!(text.contains("Karma Police"), "missing song:\n{text}");
        assert!(text.contains("OK Computer"), "missing album:\n{text}");
        assert!(text.contains("Thom Yorke"), "missing related:\n{text}");
    }

    #[test]
    fn loaded_render_shows_esc_hint() {
        let view = loaded_view();
        let text = render_to_string(&view, 70, 20);
        assert!(text.contains("Esc to go back"), "missing hint:\n{text}");
    }

    #[test]
    fn loaded_render_highlights_active_section_with_primary_no_glyph() {
        // The focused (Top Songs) section's selected row uses a primary-bg
        // cursor (no `▶` glyph), and the three section labels render.
        let view = loaded_view();
        let text = render_to_string(&view, 70, 20);
        assert!(!text.contains('▶'), "stray cursor glyph:\n{text}");
        for label in ["Top Songs", "Albums", "Related Artists"] {
            assert!(text.contains(label), "missing '{label}' label:\n{text}");
        }

        let backend = TestBackend::new(70, 20);
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
            "active section's selected row not highlighted with primary"
        );
    }
}
