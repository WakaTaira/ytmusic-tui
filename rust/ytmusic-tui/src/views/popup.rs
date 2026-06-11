//! Popup overlay widgets: context actions, theme picker, and playlist picker.
//!
//! Port of `src/ytmusic_tui/views/popup.py`. Each popup is a centered floating
//! box drawn over the current view (ratatui [`Clear`] + a bordered list). Only
//! one is shown at a time; the active one is tracked by [`PopupState`] on the
//! main loop's model. While a popup is open, keys route to it: `j`/`k` (and
//! arrows) navigate, Enter selects, Esc dismisses.
//!
//! # Architecture vs Python
//!
//! Textual's popups were `Static` widgets posting `Message`s. Here they are
//! plain values: the main loop owns the [`PopupState`], routes keys via
//! [`PopupState::on_key`], and reads the selection out via the `*Selected`
//! return types. The action lists per item type are spelled out in
//! [`build_actions`], 1:1 with the Python builders.

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState};
use ytmusic_api::{AlbumInfo, PlaylistInfo, Track};

use super::Theme;

// ---------------------------------------------------------------------------
// Action definitions
// ---------------------------------------------------------------------------

/// The kind of a context action (port of Python's `ActionKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    /// Play this single item now.
    Play,
    /// Append this item to the queue.
    AddToQueue,
    /// Start a radio seeded by this item.
    StartRadio,
    /// Toggle the like/unlike state of this track.
    ToggleLike,
    /// Navigate to the item's artist.
    GoToArtist,
    /// Navigate to the item's album.
    GoToAlbum,
    /// Add this track to a playlist (opens the playlist picker).
    AddToPlaylist,
    /// Play all of a playlist/album.
    PlayAll,
    /// Open a playlist/album's detail view.
    Open,
}

/// A single selectable action within the action popup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PopupAction {
    /// The action's kind (what it does when selected).
    pub kind: ActionKind,
    /// The human-readable label shown in the list.
    pub label: String,
}

impl PopupAction {
    fn new(kind: ActionKind, label: &str) -> Self {
        Self {
            kind,
            label: label.to_owned(),
        }
    }
}

/// The item an action popup was opened for, plus the context it was opened in.
///
/// The context discriminates the track action list (a plain track vs a track in
/// the queue vs a track inside a playlist), mirroring Python's `context`
/// parameter to `build_actions`.
#[derive(Debug, Clone, PartialEq)]
pub enum PopupItem {
    /// A track (the most common case).
    Track(Track),
    /// A playlist.
    Playlist(PlaylistInfo),
    /// An album.
    Album(AlbumInfo),
}

impl PopupItem {
    /// A human-readable title for the popup header (Python's `_item_title`).
    fn title(&self) -> String {
        match self {
            PopupItem::Track(t) => {
                if t.artist.is_empty() {
                    t.title.clone()
                } else {
                    format!("{} - {}", t.title, t.artist)
                }
            }
            PopupItem::Playlist(p) => p.title.clone(),
            PopupItem::Album(a) => {
                if a.artist.is_empty() {
                    a.title.clone()
                } else {
                    format!("{} - {}", a.title, a.artist)
                }
            }
        }
    }
}

/// Build the action list for `item` (port of Python's `build_actions`).
///
/// Track action lists are spelled out for the plain-track case (the only one
/// the Rust UI currently reaches — the queue/playlist-track context popups are
/// a later refinement). Playlists and albums match the Python builders exactly.
#[must_use]
pub fn build_actions(item: &PopupItem) -> Vec<PopupAction> {
    match item {
        PopupItem::Track(_) => vec![
            PopupAction::new(ActionKind::Play, "Play"),
            PopupAction::new(ActionKind::AddToQueue, "Add to queue"),
            PopupAction::new(ActionKind::StartRadio, "Start radio"),
            PopupAction::new(ActionKind::GoToArtist, "Go to artist"),
            PopupAction::new(ActionKind::GoToAlbum, "Go to album"),
            PopupAction::new(ActionKind::AddToPlaylist, "Add to playlist"),
            PopupAction::new(ActionKind::ToggleLike, "Like / Unlike"),
        ],
        PopupItem::Playlist(_) => vec![
            PopupAction::new(ActionKind::PlayAll, "Play all"),
            PopupAction::new(ActionKind::Open, "Open"),
        ],
        PopupItem::Album(_) => vec![
            PopupAction::new(ActionKind::PlayAll, "Play all"),
            PopupAction::new(ActionKind::Open, "Open"),
            PopupAction::new(ActionKind::GoToArtist, "Go to artist"),
        ],
    }
}

// ---------------------------------------------------------------------------
// ActionPopup
// ---------------------------------------------------------------------------

/// The context-action popup: a list of [`PopupAction`]s for one item.
#[derive(Debug, Clone)]
pub struct ActionPopup {
    item: PopupItem,
    actions: Vec<PopupAction>,
    cursor: usize,
}

impl ActionPopup {
    /// Build the popup for `item`, populating its action list.
    #[must_use]
    pub fn new(item: PopupItem) -> Self {
        let actions = build_actions(&item);
        // Every PopupItem variant must yield at least one action: an empty
        // popup would open, render nothing, and silently dismiss on Enter.
        debug_assert!(
            !actions.is_empty(),
            "build_actions returned no actions for {item:?}"
        );
        Self {
            item,
            actions,
            cursor: 0,
        }
    }

    /// The item this popup was opened for.
    #[must_use]
    pub fn item(&self) -> &PopupItem {
        &self.item
    }

    /// The currently selected action, if any.
    #[must_use]
    pub fn selected(&self) -> Option<&PopupAction> {
        self.actions.get(self.cursor)
    }

    fn select_next(&mut self) {
        let last = self.actions.len().saturating_sub(1);
        if self.cursor < last {
            self.cursor += 1;
        }
    }

    fn select_previous(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }
}

// ---------------------------------------------------------------------------
// ThemePopup
// ---------------------------------------------------------------------------

/// The theme-picker popup: a list of theme names with the current one marked.
#[derive(Debug, Clone)]
pub struct ThemePopup {
    names: Vec<String>,
    current: String,
    cursor: usize,
}

impl ThemePopup {
    /// Build the popup from the available theme names and the current theme.
    /// The cursor starts on the current theme so Enter-without-moving is a
    /// no-op re-apply.
    #[must_use]
    pub fn new(names: Vec<String>, current: &str) -> Self {
        let cursor = names.iter().position(|n| n == current).unwrap_or(0);
        Self {
            names,
            current: current.to_owned(),
            cursor,
        }
    }

    /// The currently highlighted theme name, if any.
    #[must_use]
    pub fn selected(&self) -> Option<&str> {
        self.names.get(self.cursor).map(String::as_str)
    }

    fn select_next(&mut self) {
        let last = self.names.len().saturating_sub(1);
        if self.cursor < last {
            self.cursor += 1;
        }
    }

    fn select_previous(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }
}

// ---------------------------------------------------------------------------
// PlaylistPickerPopup
// ---------------------------------------------------------------------------

/// The sentinel index for the "New playlist…" entry (always row 0).
const NEW_PLAYLIST_ROW: usize = 0;

/// What a playlist-picker selection resolves to.
#[derive(Debug, Clone, PartialEq)]
pub enum PickerChoice {
    /// The user chose "New playlist…" (create then add).
    NewPlaylist,
    /// The user chose an existing playlist (by id) to add the track to.
    Existing(String),
}

/// The playlist-picker popup: pick a target playlist for "Add to playlist".
///
/// Row 0 is always "New playlist…"; the rest are the user's playlists. Holds the
/// `track` the add applies to so the caller can issue the add command on select.
#[derive(Debug, Clone)]
pub struct PlaylistPickerPopup {
    /// `(playlist_id, title)` pairs for the existing playlists.
    playlists: Vec<(String, String)>,
    /// The track being added (carried through to the selection result).
    track: Track,
    cursor: usize,
}

impl PlaylistPickerPopup {
    /// Build the picker for `track` from the user's `playlists`.
    #[must_use]
    pub fn new(playlists: Vec<(String, String)>, track: Track) -> Self {
        Self {
            playlists,
            track,
            cursor: 0,
        }
    }

    /// The track this picker is adding.
    #[must_use]
    pub fn track(&self) -> &Track {
        &self.track
    }

    /// Resolve the current selection to a [`PickerChoice`], or `None` if the
    /// cursor is somehow out of range.
    #[must_use]
    pub fn selected(&self) -> Option<PickerChoice> {
        if self.cursor == NEW_PLAYLIST_ROW {
            return Some(PickerChoice::NewPlaylist);
        }
        let adj = self.cursor - 1;
        self.playlists
            .get(adj)
            .map(|(id, _)| PickerChoice::Existing(id.clone()))
    }

    /// The total number of rows (the "New playlist…" row plus the playlists).
    fn row_count(&self) -> usize {
        self.playlists.len() + 1
    }

    fn select_next(&mut self) {
        let last = self.row_count().saturating_sub(1);
        if self.cursor < last {
            self.cursor += 1;
        }
    }

    fn select_previous(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }
}

// ---------------------------------------------------------------------------
// PopupState — the one-popup-at-a-time state machine
// ---------------------------------------------------------------------------

/// Which popup (if any) is currently shown. Owned by the main loop's model.
#[derive(Debug, Clone, Default)]
pub enum PopupState {
    /// No popup open (the normal case).
    #[default]
    None,
    /// The context-action popup.
    Action(ActionPopup),
    /// The theme picker.
    Theme(ThemePopup),
    /// The playlist picker.
    PlaylistPicker(PlaylistPickerPopup),
}

/// The outcome of routing a key into the active popup.
#[derive(Debug, Clone, PartialEq)]
pub enum PopupOutcome {
    /// The key was consumed but no terminal selection was made (navigation, or
    /// a no-op). The popup stays open.
    Consumed,
    /// The popup was dismissed (Esc) without a selection.
    Dismissed,
    /// An action was selected in the action popup. Carries the chosen action and
    /// the item it applies to.
    ActionSelected {
        /// The selected action.
        action: PopupAction,
        /// The item the action applies to.
        item: PopupItem,
    },
    /// A theme was selected in the theme popup.
    ThemeSelected(String),
    /// A playlist target was selected in the playlist picker. Carries the choice
    /// and the track being added.
    PlaylistChosen {
        /// The chosen target (new playlist or an existing id).
        choice: PickerChoice,
        /// The track to add.
        track: Track,
    },
}

impl PopupState {
    /// Whether any popup is currently open.
    #[must_use]
    pub fn is_open(&self) -> bool {
        !matches!(self, PopupState::None)
    }

    /// Close the popup (return to [`PopupState::None`]).
    pub fn close(&mut self) {
        *self = PopupState::None;
    }

    /// Route a key into the active popup.
    ///
    /// `j`/Down and `k`/Up navigate; Enter selects (returning the terminal
    /// outcome and closing the popup); Esc dismisses. Any other key is consumed
    /// as a no-op so it does not leak to the underlying view.
    pub fn on_key(&mut self, key: crossterm::event::KeyCode) -> PopupOutcome {
        use crossterm::event::KeyCode;
        match self {
            PopupState::None => PopupOutcome::Consumed,
            PopupState::Action(popup) => match key {
                KeyCode::Char('j') | KeyCode::Down => {
                    popup.select_next();
                    PopupOutcome::Consumed
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    popup.select_previous();
                    PopupOutcome::Consumed
                }
                KeyCode::Enter => {
                    let outcome = popup.selected().map(|action| PopupOutcome::ActionSelected {
                        action: action.clone(),
                        item: popup.item.clone(),
                    });
                    self.close();
                    outcome.unwrap_or(PopupOutcome::Dismissed)
                }
                KeyCode::Esc => {
                    self.close();
                    PopupOutcome::Dismissed
                }
                _ => PopupOutcome::Consumed,
            },
            PopupState::Theme(popup) => match key {
                KeyCode::Char('j') | KeyCode::Down => {
                    popup.select_next();
                    PopupOutcome::Consumed
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    popup.select_previous();
                    PopupOutcome::Consumed
                }
                KeyCode::Enter => {
                    let outcome = popup
                        .selected()
                        .map(|name| PopupOutcome::ThemeSelected(name.to_owned()));
                    self.close();
                    outcome.unwrap_or(PopupOutcome::Dismissed)
                }
                KeyCode::Esc => {
                    self.close();
                    PopupOutcome::Dismissed
                }
                _ => PopupOutcome::Consumed,
            },
            PopupState::PlaylistPicker(popup) => match key {
                KeyCode::Char('j') | KeyCode::Down => {
                    popup.select_next();
                    PopupOutcome::Consumed
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    popup.select_previous();
                    PopupOutcome::Consumed
                }
                KeyCode::Enter => {
                    let outcome = popup.selected().map(|choice| PopupOutcome::PlaylistChosen {
                        choice,
                        track: popup.track.clone(),
                    });
                    self.close();
                    outcome.unwrap_or(PopupOutcome::Dismissed)
                }
                KeyCode::Esc => {
                    self.close();
                    PopupOutcome::Dismissed
                }
                _ => PopupOutcome::Consumed,
            },
        }
    }

    /// Render the active popup as a centered floating box over `area`.
    pub fn render(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        match self {
            PopupState::None => {}
            PopupState::Action(popup) => {
                let title = popup.item.title();
                let rows: Vec<ListItem> = popup
                    .actions
                    .iter()
                    .map(|a| ListItem::new(Line::from(Span::raw(a.label.clone()))))
                    .collect();
                render_list_popup(frame, area, theme, &title, rows, popup.cursor);
            }
            PopupState::Theme(popup) => {
                let rows: Vec<ListItem> = popup
                    .names
                    .iter()
                    .map(|n| {
                        let marker = if *n == popup.current { " *" } else { "" };
                        ListItem::new(Line::from(Span::raw(format!("{n}{marker}"))))
                    })
                    .collect();
                render_list_popup(frame, area, theme, "Select Theme", rows, popup.cursor);
            }
            PopupState::PlaylistPicker(popup) => {
                let mut rows: Vec<ListItem> =
                    vec![ListItem::new(Line::from(Span::raw("+ New playlist...")))];
                rows.extend(
                    popup
                        .playlists
                        .iter()
                        .map(|(_, title)| ListItem::new(Line::from(Span::raw(title.clone())))),
                );
                render_list_popup(frame, area, theme, "Add to playlist", rows, popup.cursor);
            }
        }
    }
}

/// Draw a centered, bordered list popup with `title` and a highlighted cursor.
///
/// The box is sized to the content (clamped to the available area) and centered
/// both axes. A [`Clear`] is drawn first so the popup fully occludes the view
/// underneath.
fn render_list_popup(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &Theme,
    title: &str,
    rows: Vec<ListItem<'static>>,
    cursor: usize,
) {
    let row_count = rows.len();
    // Width: the widest of the title / rows, plus borders + a little padding,
    // clamped to the area. Height: rows + the two border lines.
    let content_width = rows
        .iter()
        .map(ListItem::width)
        .chain(std::iter::once(title.len()))
        .max()
        .unwrap_or(0) as u16;
    let width = (content_width + 4)
        .clamp(10, area.width.max(10))
        .min(area.width);
    let height = ((row_count as u16) + 2)
        .clamp(3, area.height.max(3))
        .min(area.height);

    let popup_area = center_rect(area, width, height);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent))
        .title(Span::styled(
            title.to_owned(),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ));

    let list = List::new(rows)
        .block(block)
        .style(Style::default().fg(theme.text).bg(theme.surface))
        .highlight_style(
            Style::default()
                .fg(theme.background)
                .bg(theme.primary)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    let mut list_state = ListState::default();
    if row_count > 0 {
        list_state.select(Some(cursor.min(row_count - 1)));
    }

    frame.render_widget(Clear, popup_area);
    frame.render_stateful_widget(list, popup_area, &mut list_state);
}

/// Compute a centered `width`×`height` rect inside `area` (both axes centered
/// via ratatui's [`Flex::Center`]).
fn center_rect(area: Rect, width: u16, height: u16) -> Rect {
    let [row] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    let [cell] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(row);
    cell
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyCode;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn track(id: &str, title: &str, artist: &str) -> Track {
        Track::new(id, title, artist, "Album", 100.0, "")
    }

    // -- action lists ------------------------------------------------------

    #[test]
    fn track_action_list_matches_python() {
        let actions = build_actions(&PopupItem::Track(track("v1", "Song", "Band")));
        let kinds: Vec<ActionKind> = actions.iter().map(|a| a.kind).collect();
        assert_eq!(
            kinds,
            vec![
                ActionKind::Play,
                ActionKind::AddToQueue,
                ActionKind::StartRadio,
                ActionKind::GoToArtist,
                ActionKind::GoToAlbum,
                ActionKind::AddToPlaylist,
                ActionKind::ToggleLike,
            ]
        );
    }

    #[test]
    fn playlist_action_list_matches_python() {
        let pl = PlaylistInfo::new("PL1", "Mix", "", 10, "");
        let actions = build_actions(&PopupItem::Playlist(pl));
        let kinds: Vec<ActionKind> = actions.iter().map(|a| a.kind).collect();
        assert_eq!(kinds, vec![ActionKind::PlayAll, ActionKind::Open]);
    }

    #[test]
    fn album_action_list_matches_python() {
        let al = AlbumInfo::new_without_tracks("b1", "LP", "Band", "2020", "");
        let actions = build_actions(&PopupItem::Album(al));
        let kinds: Vec<ActionKind> = actions.iter().map(|a| a.kind).collect();
        assert_eq!(
            kinds,
            vec![
                ActionKind::PlayAll,
                ActionKind::Open,
                ActionKind::GoToArtist
            ]
        );
    }

    // -- action popup navigation + selection -------------------------------

    #[test]
    fn action_popup_enter_returns_selected_action() {
        let mut state = PopupState::Action(ActionPopup::new(PopupItem::Track(track(
            "v1", "Song", "Band",
        ))));
        // Move to "Add to queue" (index 1) and select.
        assert_eq!(state.on_key(KeyCode::Char('j')), PopupOutcome::Consumed);
        match state.on_key(KeyCode::Enter) {
            PopupOutcome::ActionSelected { action, item } => {
                assert_eq!(action.kind, ActionKind::AddToQueue);
                assert!(matches!(item, PopupItem::Track(t) if t.video_id == "v1"));
            }
            other => panic!("expected ActionSelected, got {other:?}"),
        }
        assert!(!state.is_open(), "popup closes after selection");
    }

    #[test]
    fn action_popup_esc_dismisses() {
        let mut state = PopupState::Action(ActionPopup::new(PopupItem::Track(track(
            "v1", "Song", "Band",
        ))));
        assert_eq!(state.on_key(KeyCode::Esc), PopupOutcome::Dismissed);
        assert!(!state.is_open());
    }

    #[test]
    fn action_popup_navigation_clamps() {
        let mut state = PopupState::Action(ActionPopup::new(PopupItem::Track(track(
            "v1", "Song", "Band",
        ))));
        // k at the top stays at the top.
        state.on_key(KeyCode::Char('k'));
        if let PopupState::Action(p) = &state {
            assert_eq!(p.cursor, 0);
        }
    }

    // -- theme popup -------------------------------------------------------

    #[test]
    fn theme_popup_starts_on_current() {
        let state = PopupState::Theme(ThemePopup::new(
            vec!["synthwave".into(), "nord".into(), "gruvbox".into()],
            "nord",
        ));
        if let PopupState::Theme(p) = &state {
            assert_eq!(p.selected(), Some("nord"));
        }
    }

    #[test]
    fn theme_popup_enter_returns_selection() {
        let mut state = PopupState::Theme(ThemePopup::new(
            vec!["synthwave".into(), "nord".into()],
            "synthwave",
        ));
        state.on_key(KeyCode::Char('j')); // → nord
        match state.on_key(KeyCode::Enter) {
            PopupOutcome::ThemeSelected(name) => assert_eq!(name, "nord"),
            other => panic!("expected ThemeSelected, got {other:?}"),
        }
    }

    // -- playlist picker ---------------------------------------------------

    #[test]
    fn picker_row_zero_is_new_playlist() {
        let mut state = PopupState::PlaylistPicker(PlaylistPickerPopup::new(
            vec![("PL1".into(), "Mix".into())],
            track("v1", "Song", "Band"),
        ));
        match state.on_key(KeyCode::Enter) {
            PopupOutcome::PlaylistChosen { choice, track } => {
                assert_eq!(choice, PickerChoice::NewPlaylist);
                assert_eq!(track.video_id, "v1");
            }
            other => panic!("expected PlaylistChosen, got {other:?}"),
        }
    }

    #[test]
    fn picker_existing_playlist_returns_id() {
        let mut state = PopupState::PlaylistPicker(PlaylistPickerPopup::new(
            vec![("PL1".into(), "Mix".into()), ("PL2".into(), "Chill".into())],
            track("v1", "Song", "Band"),
        ));
        state.on_key(KeyCode::Char('j')); // → PL1
        state.on_key(KeyCode::Char('j')); // → PL2
        match state.on_key(KeyCode::Enter) {
            PopupOutcome::PlaylistChosen { choice, .. } => {
                assert_eq!(choice, PickerChoice::Existing("PL2".into()));
            }
            other => panic!("expected PlaylistChosen, got {other:?}"),
        }
    }

    // -- rendering ---------------------------------------------------------

    fn render_state(state: &PopupState, w: u16, h: u16) -> String {
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = Theme::default();
        terminal
            .draw(|frame| state.render(frame, frame.area(), &theme))
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
    fn action_popup_renders_title_and_actions() {
        let state = PopupState::Action(ActionPopup::new(PopupItem::Track(track(
            "v1",
            "Get Lucky",
            "Daft Punk",
        ))));
        let text = render_state(&state, 50, 16);
        assert!(text.contains("Get Lucky"), "missing title:\n{text}");
        assert!(text.contains("Play"), "missing Play action:\n{text}");
        assert!(
            text.contains("Add to queue"),
            "missing queue action:\n{text}"
        );
        assert!(
            text.contains("Start radio"),
            "missing radio action:\n{text}"
        );
    }

    #[test]
    fn theme_popup_marks_current() {
        let state = PopupState::Theme(ThemePopup::new(
            vec!["synthwave".into(), "nord".into()],
            "nord",
        ));
        let text = render_state(&state, 40, 12);
        assert!(text.contains("Select Theme"), "missing title:\n{text}");
        assert!(text.contains("nord *"), "missing current marker:\n{text}");
    }

    #[test]
    fn picker_renders_new_playlist_row() {
        let state = PopupState::PlaylistPicker(PlaylistPickerPopup::new(
            vec![("PL1".into(), "My Mix".into())],
            track("v1", "Song", "Band"),
        ));
        let text = render_state(&state, 40, 12);
        assert!(text.contains("Add to playlist"), "missing title:\n{text}");
        assert!(text.contains("New playlist"), "missing new row:\n{text}");
        assert!(text.contains("My Mix"), "missing playlist:\n{text}");
    }
}
