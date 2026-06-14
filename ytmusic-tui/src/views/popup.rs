//! Popup overlay widgets: context actions, theme picker, and playlist picker.
//!
//! Port of `src/ytmusic_tui/views/popup.py`. Each popup is a bottom-docked
//! sheet drawn over the current view (ratatui [`Clear`] + an accent top-border
//! divider over a surface-filled list), matching the Textual CSS
//! (`dock: bottom; border-top: solid $accent`). Only one is shown at a time;
//! the active one is tracked by [`PopupState`] on the main loop's model. While
//! a popup is open, keys route to it: `j`/`k` (and arrows) navigate, Enter
//! selects, Esc dismisses.
//!
//! # Architecture vs Python
//!
//! Textual's popups were `Static` widgets posting `Message`s. Here they are
//! plain values: the main loop owns the [`PopupState`], routes keys via
//! [`PopupState::on_key`], and reads the selection out via the `*Selected`
//! return types. The action lists per item type are spelled out in
//! [`build_actions`], 1:1 with the Python builders.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
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
    /// Remove this track from the queue (queue-view context only).
    RemoveFromQueue,
    /// Remove this track from the playlist it is shown in (playlist-track
    /// context only).
    RemoveFromPlaylist,
    /// Play all of a playlist/album.
    PlayAll,
    /// Open a playlist/album's detail view.
    Open,
    /// Copy a shareable YouTube Music link for the item to the system clipboard
    /// (OSC52). Available on every item type the popup surfaces.
    CopyLink,
    /// Subscribe to (follow) an artist. Offered on artist-bearing rows where
    /// the channel id can be resolved.
    FollowArtist,
    /// Unsubscribe from (unfollow) an artist. Offered alongside [`FollowArtist`]
    /// because the API does not expose the current subscription state on the
    /// artist browse response, so both actions are listed and the user picks.
    UnfollowArtist,
    /// Save an album / playlist to the user's library (issue #12). The save
    /// endpoint does not expose the current state on the browse response, so
    /// both [`SaveToLibrary`] and [`RemoveFromLibrary`] are listed alongside
    /// each other and the user picks the matching action.
    SaveToLibrary,
    /// Remove an album / playlist from the user's library (issue #12). See
    /// [`SaveToLibrary`] for why both directions are always offered.
    RemoveFromLibrary,
}

/// The context an action popup was opened in, discriminating the track action
/// list. Port of Python's `context` string argument to `build_actions`
/// (`""` / `"queue"` / `"playlist_tracks"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PopupContext {
    /// A plain track row (home / search / library / album / artist / history).
    #[default]
    Plain,
    /// A track in the queue view (offers "Remove from queue").
    Queue,
    /// A track inside a playlist's track list (offers "Remove from playlist").
    PlaylistTracks,
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

/// Build the action list for `item` in `context` (port of Python's
/// `build_actions`). Playlists and albums ignore the context (they have a single
/// action list each); only tracks vary by context.
#[must_use]
pub fn build_actions(item: &PopupItem, context: PopupContext) -> Vec<PopupAction> {
    match item {
        PopupItem::Track(_) => track_actions(context),
        PopupItem::Playlist(_) => vec![
            PopupAction::new(ActionKind::PlayAll, "Play all"),
            PopupAction::new(ActionKind::Open, "Open"),
            // Issue #12: save / remove the playlist from the user's library.
            // The API does not expose the current saved state on the browse
            // response, so both directions are listed and the user picks.
            PopupAction::new(ActionKind::SaveToLibrary, "Save to library"),
            PopupAction::new(ActionKind::RemoveFromLibrary, "Remove from library"),
            PopupAction::new(ActionKind::CopyLink, "Copy link"),
        ],
        PopupItem::Album(_) => vec![
            PopupAction::new(ActionKind::PlayAll, "Play all"),
            PopupAction::new(ActionKind::Open, "Open"),
            PopupAction::new(ActionKind::GoToArtist, "Go to artist"),
            // Issue #11: follow / unfollow the album's artist. The API does not
            // expose the current subscription state on the artist browse
            // response, so both actions are listed and the user picks the
            // appropriate one.
            PopupAction::new(ActionKind::FollowArtist, "Follow artist"),
            PopupAction::new(ActionKind::UnfollowArtist, "Unfollow artist"),
            // Issue #12: save / remove the album from the user's library.
            // Same "both directions always listed" reasoning as the artist
            // follow / unfollow pair above.
            PopupAction::new(ActionKind::SaveToLibrary, "Save to library"),
            PopupAction::new(ActionKind::RemoveFromLibrary, "Remove from library"),
            PopupAction::new(ActionKind::CopyLink, "Copy link"),
        ],
    }
}

/// The per-context track action list (port of Python's `actions_for_track` /
/// `actions_for_queue_track` / `actions_for_playlist_track`). The ordering and
/// labels are 1:1 with the Python builders — those lists are the contract.
fn track_actions(context: PopupContext) -> Vec<PopupAction> {
    match context {
        PopupContext::Plain => vec![
            PopupAction::new(ActionKind::Play, "Play"),
            PopupAction::new(ActionKind::AddToQueue, "Add to queue"),
            PopupAction::new(ActionKind::StartRadio, "Start radio"),
            PopupAction::new(ActionKind::GoToArtist, "Go to artist"),
            PopupAction::new(ActionKind::GoToAlbum, "Go to album"),
            PopupAction::new(ActionKind::AddToPlaylist, "Add to playlist"),
            PopupAction::new(ActionKind::ToggleLike, "Like / Unlike"),
            PopupAction::new(ActionKind::CopyLink, "Copy link"),
        ],
        PopupContext::Queue => vec![
            PopupAction::new(ActionKind::Play, "Play"),
            PopupAction::new(ActionKind::RemoveFromQueue, "Remove from queue"),
            PopupAction::new(ActionKind::GoToArtist, "Go to artist"),
            PopupAction::new(ActionKind::GoToAlbum, "Go to album"),
            PopupAction::new(ActionKind::AddToPlaylist, "Add to playlist"),
            PopupAction::new(ActionKind::CopyLink, "Copy link"),
        ],
        PopupContext::PlaylistTracks => vec![
            PopupAction::new(ActionKind::Play, "Play"),
            PopupAction::new(ActionKind::AddToQueue, "Add to queue"),
            PopupAction::new(ActionKind::RemoveFromPlaylist, "Remove from playlist"),
            PopupAction::new(ActionKind::GoToArtist, "Go to artist"),
            PopupAction::new(ActionKind::GoToAlbum, "Go to album"),
            PopupAction::new(ActionKind::CopyLink, "Copy link"),
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
    /// Build the popup for `item` in the plain (no-context) track context.
    #[must_use]
    pub fn new(item: PopupItem) -> Self {
        Self::with_context(item, PopupContext::Plain)
    }

    /// Build the popup for `item` in `context`, populating its action list.
    /// The context only changes the *track* action list (queue / playlist-track
    /// rows get their remove actions); playlists and albums are context-agnostic.
    #[must_use]
    pub fn with_context(item: PopupItem, context: PopupContext) -> Self {
        let actions = build_actions(&item, context);
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
    /// The user chose "New playlist…" and named it (create then add). `name` is
    /// the title typed into the name-entry prompt (Python prompted for one via
    /// `PlaylistPickerPopup`'s "New playlist" entry).
    NewPlaylist { name: String },
    /// The user chose an existing playlist (by id) to add the track to.
    Existing(String),
}

/// The playlist-picker popup: pick a target playlist for "Add to playlist".
///
/// Row 0 is always "New playlist…"; the rest are the user's playlists. Holds the
/// `track` the add applies to so the caller can issue the add command on select.
///
/// Selecting "New playlist…" does not resolve immediately: it enters a
/// **name-entry** sub-mode (`naming` becomes `Some(buffer)`), where printable
/// keys build the title, Backspace deletes, Enter confirms (resolving to
/// [`PickerChoice::NewPlaylist`] with the typed name), and Esc returns to the
/// list. This is the Rust equivalent of Python's "New playlist" prompt, kept
/// inside the popup so the app's search-specific input mode is untouched.
#[derive(Debug, Clone)]
pub struct PlaylistPickerPopup {
    /// `(playlist_id, title)` pairs for the existing playlists.
    playlists: Vec<(String, String)>,
    /// The track being added (carried through to the selection result).
    track: Track,
    cursor: usize,
    /// `Some(buffer)` while the user is typing a new playlist name; `None` in
    /// the normal list-navigation mode.
    naming: Option<String>,
}

impl PlaylistPickerPopup {
    /// Build the picker for `track` from the user's `playlists`.
    #[must_use]
    pub fn new(playlists: Vec<(String, String)>, track: Track) -> Self {
        Self {
            playlists,
            track,
            cursor: 0,
            naming: None,
        }
    }

    /// The track this picker is adding.
    #[must_use]
    pub fn track(&self) -> &Track {
        &self.track
    }

    /// Whether the popup is in the name-entry sub-mode.
    #[must_use]
    pub fn is_naming(&self) -> bool {
        self.naming.is_some()
    }

    /// Resolve the current *list* selection to a [`PickerChoice`], or `None`.
    ///
    /// Used only for existing-playlist rows: the "New playlist…" row does not
    /// resolve here (it enters the name-entry sub-mode instead — see
    /// [`PopupState::on_key`]). Kept for the existing-row terminal path.
    #[must_use]
    pub fn selected_existing(&self) -> Option<PickerChoice> {
        if self.cursor == NEW_PLAYLIST_ROW {
            return None;
        }
        let adj = self.cursor - 1;
        self.playlists
            .get(adj)
            .map(|(id, _)| PickerChoice::Existing(id.clone()))
    }

    /// Enter the name-entry sub-mode with an empty buffer (the user picked the
    /// "New playlist…" row).
    fn begin_naming(&mut self) {
        self.naming = Some(String::new());
    }

    /// The currently-typed new-playlist name, if in the name-entry sub-mode.
    #[must_use]
    pub fn naming_buffer(&self) -> Option<&str> {
        self.naming.as_deref()
    }

    /// Append a character to the name buffer (name-entry mode only).
    fn push_name_char(&mut self, ch: char) {
        if let Some(buf) = self.naming.as_mut() {
            buf.push(ch);
        }
    }

    /// Delete the last character of the name buffer (name-entry mode only).
    fn backspace_name(&mut self) {
        if let Some(buf) = self.naming.as_mut() {
            buf.pop();
        }
    }

    /// Leave the name-entry sub-mode, discarding the buffer, and return to the
    /// list (Esc inside naming).
    fn cancel_naming(&mut self) {
        self.naming = None;
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
            PopupState::PlaylistPicker(popup) if popup.is_naming() => match key {
                // Name-entry sub-mode: build the new playlist title.
                KeyCode::Enter => {
                    // An empty name is not a valid title; ignore Enter until at
                    // least one character is typed (Esc still cancels).
                    let name = popup.naming_buffer().unwrap_or("").trim().to_owned();
                    if name.is_empty() {
                        return PopupOutcome::Consumed;
                    }
                    let track = popup.track.clone();
                    self.close();
                    PopupOutcome::PlaylistChosen {
                        choice: PickerChoice::NewPlaylist { name },
                        track,
                    }
                }
                KeyCode::Backspace => {
                    popup.backspace_name();
                    PopupOutcome::Consumed
                }
                KeyCode::Esc => {
                    // Esc inside naming returns to the list, not dismissing the
                    // whole popup.
                    popup.cancel_naming();
                    PopupOutcome::Consumed
                }
                KeyCode::Char(ch) => {
                    popup.push_name_char(ch);
                    PopupOutcome::Consumed
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
                    // "New playlist…" (row 0) enters the name-entry sub-mode;
                    // an existing row resolves immediately.
                    if popup.cursor == NEW_PLAYLIST_ROW {
                        popup.begin_naming();
                        PopupOutcome::Consumed
                    } else {
                        let outcome =
                            popup
                                .selected_existing()
                                .map(|choice| PopupOutcome::PlaylistChosen {
                                    choice,
                                    track: popup.track.clone(),
                                });
                        self.close();
                        outcome.unwrap_or(PopupOutcome::Dismissed)
                    }
                }
                KeyCode::Esc => {
                    self.close();
                    PopupOutcome::Dismissed
                }
                _ => PopupOutcome::Consumed,
            },
        }
    }

    /// Render the active popup as a bottom-docked sheet over `area`.
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
                if let Some(buffer) = popup.naming_buffer() {
                    // Name-entry sub-mode: a single prompt row with the typed
                    // title and a block cursor, so the user sees what they type.
                    let prompt = format!("Name: {buffer}_");
                    let rows = vec![ListItem::new(Line::from(Span::raw(prompt)))];
                    render_list_popup(frame, area, theme, "New playlist", rows, 0);
                } else {
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
}

/// Draw a bottom-docked list popup sheet with `title` and a highlighted cursor.
///
/// Reproduces popup.py's CSS: the popup is a full-width sheet docked to the
/// bottom of `area` (`dock: bottom; height: auto`), filled with `$surface`, a
/// `border-top: solid $accent` divider, an accent-bold title row, and one-cell
/// horizontal padding (`padding: 0 1`). The selected list row uses Textual's
/// default `$primary` cursor.
///
/// ratatui cannot translucently dim the view behind the sheet (Textual's docked
/// sheets do not dim either), so only the sheet area is cleared — the view stays
/// visible above it, matching the Textual behavior.
fn render_list_popup(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &Theme,
    title: &str,
    rows: Vec<ListItem<'static>>,
    cursor: usize,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let row_count = rows.len();

    // Sheet height: top-border divider (1) + title (1) + list rows, capped at
    // Python's `max-height: 12/14` and the available area. The sheet docks to
    // the bottom of `area`. Python caps differ per popup (Action 12 / Theme 10
    // / Picker 14); one unified cap of 14 is fine because content is
    // height-driven and the deepest real list (the picker) is the 14 case.
    const MAX_SHEET_ROWS: u16 = 14;
    let body_rows = (row_count as u16).min(MAX_SHEET_ROWS);
    let height = (body_rows + 2).min(area.height); // +1 border, +1 title
    let sheet_area = Rect {
        x: area.x,
        y: area.y + area.height - height,
        width: area.width,
        height,
    };

    // Clear only the sheet so it reads as a solid surface; the view remains
    // visible above (Textual's docked sheets do not dim the backdrop).
    frame.render_widget(Clear, sheet_area);

    // Top-border divider in accent (Python `border-top: solid $accent`) over a
    // surface-filled body.
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(theme.accent))
        .style(Style::default().bg(theme.surface));
    let inner = block.inner(sheet_area);
    frame.render_widget(block, sheet_area);

    // Title row + list, with one cell of left/right padding (`padding: 0 1`).
    let [title_area, list_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
    let [title_pad] = Layout::horizontal([Constraint::Min(0)])
        .horizontal_margin(1)
        .areas(title_area);
    let [list_pad] = Layout::horizontal([Constraint::Min(0)])
        .horizontal_margin(1)
        .areas(list_area);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            title.to_owned(),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )))
        .style(Style::default().bg(theme.surface)),
        title_pad,
    );

    let list = List::new(rows)
        .style(Style::default().fg(theme.text).bg(theme.surface))
        .highlight_style(super::selected_row_style(theme));

    let mut list_state = ListState::default();
    if row_count > 0 {
        list_state.select(Some(cursor.min(row_count - 1)));
    }
    frame.render_stateful_widget(list, list_pad, &mut list_state);
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
        // Issue #14 appends `CopyLink` to every track action list (Plain/Queue/
        // PlaylistTracks) and to the playlist/album lists. The earlier kinds
        // remain unchanged so the keyboard muscle memory still hits the same
        // entries.
        let actions = build_actions(
            &PopupItem::Track(track("v1", "Song", "Band")),
            PopupContext::Plain,
        );
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
                ActionKind::CopyLink,
            ]
        );
    }

    #[test]
    fn queue_track_action_list_matches_python() {
        let actions = build_actions(
            &PopupItem::Track(track("v1", "Song", "Band")),
            PopupContext::Queue,
        );
        let kinds: Vec<ActionKind> = actions.iter().map(|a| a.kind).collect();
        assert_eq!(
            kinds,
            vec![
                ActionKind::Play,
                ActionKind::RemoveFromQueue,
                ActionKind::GoToArtist,
                ActionKind::GoToAlbum,
                ActionKind::AddToPlaylist,
                ActionKind::CopyLink,
            ]
        );
    }

    #[test]
    fn playlist_track_action_list_matches_python() {
        let actions = build_actions(
            &PopupItem::Track(track("v1", "Song", "Band")),
            PopupContext::PlaylistTracks,
        );
        let kinds: Vec<ActionKind> = actions.iter().map(|a| a.kind).collect();
        assert_eq!(
            kinds,
            vec![
                ActionKind::Play,
                ActionKind::AddToQueue,
                ActionKind::RemoveFromPlaylist,
                ActionKind::GoToArtist,
                ActionKind::GoToAlbum,
                ActionKind::CopyLink,
            ]
        );
    }

    #[test]
    fn playlist_action_list_matches_python() {
        let pl = PlaylistInfo::new("PL1", "Mix", "", 10, "");
        let actions = build_actions(&PopupItem::Playlist(pl), PopupContext::Plain);
        let kinds: Vec<ActionKind> = actions.iter().map(|a| a.kind).collect();
        // Issue #12 inserts SaveToLibrary + RemoveFromLibrary between Open and
        // CopyLink (both shown — the API does not expose the current saved
        // state on the playlist browse response).
        assert_eq!(
            kinds,
            vec![
                ActionKind::PlayAll,
                ActionKind::Open,
                ActionKind::SaveToLibrary,
                ActionKind::RemoveFromLibrary,
                ActionKind::CopyLink,
            ]
        );
    }

    #[test]
    fn album_action_list_matches_python() {
        let al = AlbumInfo::new_without_tracks("b1", "LP", "Band", "2020", "");
        let actions = build_actions(&PopupItem::Album(al), PopupContext::Plain);
        let kinds: Vec<ActionKind> = actions.iter().map(|a| a.kind).collect();
        // Issue #11 inserts FollowArtist + UnfollowArtist between GoToArtist
        // and CopyLink (both shown — the API does not expose the current
        // subscription state on the artist browse response). Issue #12 appends
        // SaveToLibrary + RemoveFromLibrary just before CopyLink for the same
        // "both directions always listed" reason.
        assert_eq!(
            kinds,
            vec![
                ActionKind::PlayAll,
                ActionKind::Open,
                ActionKind::GoToArtist,
                ActionKind::FollowArtist,
                ActionKind::UnfollowArtist,
                ActionKind::SaveToLibrary,
                ActionKind::RemoveFromLibrary,
                ActionKind::CopyLink,
            ]
        );
    }

    #[test]
    fn album_action_list_includes_save_and_remove_from_library() {
        // Issue #12 acceptance: every album action popup surfaces both Save
        // and Remove so the user can act regardless of the unknown current
        // saved state.
        let al = AlbumInfo::new_without_tracks("b1", "LP", "Band", "2020", "");
        let actions = build_actions(&PopupItem::Album(al), PopupContext::Plain);
        assert!(
            actions.iter().any(|a| a.kind == ActionKind::SaveToLibrary),
            "album popup missing SaveToLibrary"
        );
        assert!(
            actions
                .iter()
                .any(|a| a.kind == ActionKind::RemoveFromLibrary),
            "album popup missing RemoveFromLibrary"
        );
    }

    #[test]
    fn playlist_action_list_includes_save_and_remove_from_library() {
        // Issue #12 acceptance: every playlist action popup surfaces both
        // Save and Remove for the same "unknown current saved state" reason.
        let pl = PlaylistInfo::new("PL1", "Mix", "", 10, "");
        let actions = build_actions(&PopupItem::Playlist(pl), PopupContext::Plain);
        assert!(
            actions.iter().any(|a| a.kind == ActionKind::SaveToLibrary),
            "playlist popup missing SaveToLibrary"
        );
        assert!(
            actions
                .iter()
                .any(|a| a.kind == ActionKind::RemoveFromLibrary),
            "playlist popup missing RemoveFromLibrary"
        );
    }

    #[test]
    fn album_action_list_includes_follow_and_unfollow_artist() {
        // Issue #11 acceptance: every album action popup surfaces both Follow
        // and Unfollow so the user can act regardless of the unknown current
        // subscription state.
        let al = AlbumInfo::new_without_tracks("b1", "LP", "Band", "2020", "");
        let actions = build_actions(&PopupItem::Album(al), PopupContext::Plain);
        assert!(
            actions.iter().any(|a| a.kind == ActionKind::FollowArtist),
            "album popup missing FollowArtist"
        );
        assert!(
            actions.iter().any(|a| a.kind == ActionKind::UnfollowArtist),
            "album popup missing UnfollowArtist"
        );
    }

    #[test]
    fn copylink_is_present_in_every_action_list() {
        // Issue #14 acceptance: every popup that opens on a track, playlist, or
        // album surfaces "Copy link". The Plain track list has it last (after
        // ToggleLike); the playlist and album lists have it last. This locks
        // the contract so regressions in build_actions are caught in one test.
        let track_item = PopupItem::Track(track("v1", "Song", "Band"));
        for ctx in [
            PopupContext::Plain,
            PopupContext::Queue,
            PopupContext::PlaylistTracks,
        ] {
            let actions = build_actions(&track_item, ctx);
            assert!(
                actions.iter().any(|a| a.kind == ActionKind::CopyLink),
                "{ctx:?} track list missing CopyLink"
            );
        }
        let pl_actions = build_actions(
            &PopupItem::Playlist(PlaylistInfo::new("PL1", "Mix", "", 10, "")),
            PopupContext::Plain,
        );
        assert!(pl_actions.iter().any(|a| a.kind == ActionKind::CopyLink));
        let al_actions = build_actions(
            &PopupItem::Album(AlbumInfo::new_without_tracks(
                "b1", "LP", "Band", "2020", "",
            )),
            PopupContext::Plain,
        );
        assert!(al_actions.iter().any(|a| a.kind == ActionKind::CopyLink));
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
    fn picker_row_zero_enters_naming_mode_then_resolves_typed_name() {
        let mut state = PopupState::PlaylistPicker(PlaylistPickerPopup::new(
            vec![("PL1".into(), "Mix".into())],
            track("v1", "Song", "Band"),
        ));
        // Enter on "New playlist…" enters name-entry mode (does NOT resolve yet).
        assert_eq!(state.on_key(KeyCode::Enter), PopupOutcome::Consumed);
        if let PopupState::PlaylistPicker(p) = &state {
            assert!(p.is_naming(), "Enter on row 0 begins naming");
        } else {
            panic!("picker should still be open");
        }
        // Type a name, then Enter to confirm.
        for ch in "Roadtrip".chars() {
            assert_eq!(state.on_key(KeyCode::Char(ch)), PopupOutcome::Consumed);
        }
        match state.on_key(KeyCode::Enter) {
            PopupOutcome::PlaylistChosen { choice, track } => {
                assert_eq!(
                    choice,
                    PickerChoice::NewPlaylist {
                        name: "Roadtrip".into()
                    }
                );
                assert_eq!(track.video_id, "v1");
            }
            other => panic!("expected PlaylistChosen, got {other:?}"),
        }
        assert!(!state.is_open(), "popup closes after naming confirm");
    }

    #[test]
    fn picker_naming_empty_name_does_not_resolve() {
        let mut state = PopupState::PlaylistPicker(PlaylistPickerPopup::new(
            vec![],
            track("v1", "Song", "Band"),
        ));
        state.on_key(KeyCode::Enter); // enter naming
        // Enter with an empty buffer is ignored (stays open, still naming).
        assert_eq!(state.on_key(KeyCode::Enter), PopupOutcome::Consumed);
        assert!(state.is_open());
        if let PopupState::PlaylistPicker(p) = &state {
            assert!(p.is_naming());
        }
    }

    #[test]
    fn picker_naming_esc_returns_to_list() {
        let mut state = PopupState::PlaylistPicker(PlaylistPickerPopup::new(
            vec![("PL1".into(), "Mix".into())],
            track("v1", "Song", "Band"),
        ));
        state.on_key(KeyCode::Enter); // enter naming
        state.on_key(KeyCode::Char('x'));
        state.on_key(KeyCode::Backspace);
        // Esc inside naming returns to the list without dismissing the popup.
        assert_eq!(state.on_key(KeyCode::Esc), PopupOutcome::Consumed);
        assert!(state.is_open());
        if let PopupState::PlaylistPicker(p) = &state {
            assert!(!p.is_naming(), "Esc leaves naming back to the list");
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

    // -- contract: bottom-docked accent-bordered surface sheet -------------

    #[test]
    fn popup_is_bottom_docked_with_accent_top_border() {
        // The sheet docks to the bottom of the area with a `─` accent divider
        // along its top edge (Python `dock: bottom; border-top: solid $accent`),
        // and a surface-filled body with a primary-colored selected row.
        let state = PopupState::Action(ActionPopup::new(PopupItem::Track(track(
            "v1",
            "Get Lucky",
            "Daft Punk",
        ))));
        let (w, h) = (50u16, 16u16);
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = Theme::default();
        terminal
            .draw(|frame| state.render(frame, frame.area(), &theme))
            .unwrap();
        let buffer = terminal.backend().buffer();

        // The bottom row carries surface fill (the sheet is docked there).
        assert_eq!(
            buffer[(0, h - 1)].bg,
            theme.surface,
            "bottom row is not the docked sheet's surface"
        );
        // The top edge of the sheet is an accent-colored `─` divider.
        let has_accent_top_border = buffer
            .content()
            .iter()
            .any(|c| c.symbol() == "─" && c.fg == theme.accent);
        assert!(has_accent_top_border, "missing accent top-border divider");
        // The selected action row uses the primary cursor.
        assert!(
            buffer.content().iter().any(|c| c.bg == theme.primary),
            "selected popup row not highlighted with primary"
        );
        // No box-corner glyphs (it is a top-border sheet, not a full box).
        for cell in buffer.content() {
            assert!(
                !"┌┐└┘".contains(cell.symbol()),
                "popup drew a box corner: {:?}",
                cell.symbol()
            );
        }
    }
}
