"""Popup overlay widgets for context actions and theme selection.

Implements spotify_player-style popups that overlay at the bottom of the
screen. Only one popup is visible at a time. Escape dismisses any popup.
"""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum, auto
from typing import TYPE_CHECKING, Any

from textual.message import Message
from textual.widgets import Label, ListItem, ListView, Static

if TYPE_CHECKING:
    from textual.app import ComposeResult

    from ytmusic_tui.api import AlbumInfo, PlaylistInfo
    from ytmusic_tui.queue import Track


# ---------------------------------------------------------------------------
# Action definitions
# ---------------------------------------------------------------------------


class ActionKind(Enum):
    """Identifies the type of action in an action popup."""

    PLAY = auto()
    ADD_TO_QUEUE = auto()
    START_RADIO = auto()
    TOGGLE_LIKE = auto()
    GO_TO_ARTIST = auto()
    GO_TO_ALBUM = auto()
    ADD_TO_PLAYLIST = auto()
    PLAY_ALL = auto()
    OPEN = auto()
    REMOVE_FROM_QUEUE = auto()
    REMOVE_FROM_PLAYLIST = auto()


@dataclass(frozen=True)
class PopupAction:
    """A single selectable action within the popup."""

    kind: ActionKind
    label: str
    enabled: bool = True


# ---------------------------------------------------------------------------
# Action builders per item type
# ---------------------------------------------------------------------------


def actions_for_track(track: Track) -> list[PopupAction]:
    """Return the action list for a Track item."""
    return [
        PopupAction(kind=ActionKind.PLAY, label="Play"),
        PopupAction(kind=ActionKind.ADD_TO_QUEUE, label="Add to queue"),
        PopupAction(kind=ActionKind.START_RADIO, label="Start radio"),
        PopupAction(kind=ActionKind.GO_TO_ARTIST, label="Go to artist"),
        PopupAction(kind=ActionKind.GO_TO_ALBUM, label="Go to album"),
        PopupAction(kind=ActionKind.ADD_TO_PLAYLIST, label="Add to playlist"),
        PopupAction(kind=ActionKind.TOGGLE_LIKE, label="Like / Unlike"),
    ]


def actions_for_playlist(playlist: PlaylistInfo) -> list[PopupAction]:
    """Return the action list for a PlaylistInfo item."""
    return [
        PopupAction(kind=ActionKind.PLAY_ALL, label="Play all"),
        PopupAction(kind=ActionKind.OPEN, label="Open"),
    ]


def actions_for_album(album: AlbumInfo) -> list[PopupAction]:
    """Return the action list for an AlbumInfo item."""
    return [
        PopupAction(kind=ActionKind.PLAY_ALL, label="Play all"),
        PopupAction(kind=ActionKind.OPEN, label="Open"),
        PopupAction(kind=ActionKind.GO_TO_ARTIST, label="Go to artist"),
    ]


def actions_for_queue_track(track: Track) -> list[PopupAction]:
    """Return the action list for a Track in the queue view."""
    return [
        PopupAction(kind=ActionKind.PLAY, label="Play"),
        PopupAction(kind=ActionKind.REMOVE_FROM_QUEUE, label="Remove from queue"),
        PopupAction(kind=ActionKind.GO_TO_ARTIST, label="Go to artist"),
        PopupAction(kind=ActionKind.GO_TO_ALBUM, label="Go to album"),
        PopupAction(kind=ActionKind.ADD_TO_PLAYLIST, label="Add to playlist"),
    ]


def actions_for_playlist_track(track: Track) -> list[PopupAction]:
    """Return the action list for a Track inside a playlist detail view."""
    return [
        PopupAction(kind=ActionKind.PLAY, label="Play"),
        PopupAction(kind=ActionKind.ADD_TO_QUEUE, label="Add to queue"),
        PopupAction(kind=ActionKind.REMOVE_FROM_PLAYLIST, label="Remove from playlist"),
        PopupAction(kind=ActionKind.GO_TO_ARTIST, label="Go to artist"),
        PopupAction(kind=ActionKind.GO_TO_ALBUM, label="Go to album"),
    ]


def build_actions(
    item: Track | PlaylistInfo | AlbumInfo,
    *,
    context: str = "",
) -> list[PopupAction]:
    """Build the appropriate action list based on the item type."""
    from ytmusic_tui.api import AlbumInfo as _AlbumInfo
    from ytmusic_tui.api import PlaylistInfo as _PlaylistInfo
    from ytmusic_tui.queue import Track as _Track

    if isinstance(item, _Track):
        if context == "queue":
            return actions_for_queue_track(item)
        if context == "playlist_tracks":
            return actions_for_playlist_track(item)
        return actions_for_track(item)
    if isinstance(item, _PlaylistInfo):
        return actions_for_playlist(item)
    if isinstance(item, _AlbumInfo):
        return actions_for_album(item)
    return []


# ---------------------------------------------------------------------------
# ActionPopup
# ---------------------------------------------------------------------------


class ActionPopup(Static):
    """Overlay popup showing context actions for a selected item.

    Hidden by default. Call :meth:`show` with an item to populate
    actions and make the popup visible. Call :meth:`dismiss` or
    press Escape to hide.
    """

    DEFAULT_CSS = """
    ActionPopup {
        layer: overlay;
        dock: bottom;
        offset-y: -4;
        height: auto;
        max-height: 12;
        background: $surface;
        border-top: solid $accent;
        padding: 0 1;
        display: none;
    }
    ActionPopup.visible {
        display: block;
    }
    ActionPopup #popup-title {
        height: 1;
        text-style: bold;
        color: $accent;
        padding: 0 0 0 0;
    }
    ActionPopup ListView {
        height: auto;
        max-height: 8;
        background: $surface;
    }
    ActionPopup ListItem {
        height: 1;
        padding: 0 1;
    }
    ActionPopup ListItem.disabled-action {
        color: $text-muted;
        text-style: italic;
    }
    """

    class ActionSelected(Message):
        """Emitted when the user selects an action."""

        def __init__(self, action: PopupAction, item: Any, context: str = "") -> None:
            super().__init__()
            self.action = action
            self.item = item
            self.context = context

    class Dismissed(Message):
        """Emitted when the popup is closed without an action."""

    def __init__(self, **kwargs: Any) -> None:
        super().__init__(**kwargs)
        self._actions: list[PopupAction] = []
        self._item: Any = None
        self._popup_context: str = ""

    def compose(self) -> ComposeResult:
        """Build the popup layout."""
        yield Label("", id="popup-title")
        yield ListView(id="popup-actions")

    def show(self, item: Track | PlaylistInfo | AlbumInfo, *, context: str = "") -> None:
        """Populate actions for *item* and display the popup."""
        self._item = item
        self._popup_context = context
        self._actions = build_actions(item, context=context)

        # Update title
        title = _item_title(item)
        self.query_one("#popup-title", Label).update(title)

        # Populate action list
        list_view = self.query_one("#popup-actions", ListView)
        list_view.clear()
        for action in self._actions:
            label_text = action.label if action.enabled else f"{action.label} (unavailable)"
            li = ListItem(Label(label_text))
            if not action.enabled:
                li.add_class("disabled-action")
            list_view.append(li)

        self.add_class("visible")
        list_view.focus()

    def dismiss(self) -> None:
        """Hide the popup and clear state."""
        self.remove_class("visible")
        self._actions = []
        self._item = None
        self.post_message(self.Dismissed())

    @property
    def is_visible(self) -> bool:
        """Whether the popup is currently shown."""
        return self.has_class("visible")

    @property
    def actions(self) -> list[PopupAction]:
        """Currently displayed actions (read-only copy)."""
        return list(self._actions)

    @property
    def item(self) -> Any:
        """The item the popup was opened for."""
        return self._item

    def on_list_view_selected(self, event: ListView.Selected) -> None:
        """Handle Enter on an action item."""
        index = event.list_view.index
        if index is None or index < 0 or index >= len(self._actions):
            return

        action = self._actions[index]
        if not action.enabled:
            return

        item = self._item
        context = self._popup_context
        self.remove_class("visible")
        self.post_message(self.ActionSelected(action=action, item=item, context=context))

    def on_key(self, event: object) -> None:
        """Handle Escape to dismiss the popup."""
        key = getattr(event, "key", "")
        if key == "escape":
            self.dismiss()
            # Prevent the key from bubbling further
            stop = getattr(event, "stop", None)
            if callable(stop):
                stop()


# ---------------------------------------------------------------------------
# ThemePopup
# ---------------------------------------------------------------------------


class ThemePopup(Static):
    """Overlay popup for switching the application theme.

    Hidden by default. Call :meth:`show` to display available themes.
    Enter applies the selected theme. Escape dismisses.
    """

    DEFAULT_CSS = """
    ThemePopup {
        layer: overlay;
        dock: bottom;
        offset-y: -4;
        height: auto;
        max-height: 10;
        background: $surface;
        border-top: solid $accent;
        padding: 0 1;
        display: none;
    }
    ThemePopup.visible {
        display: block;
    }
    ThemePopup #theme-title {
        height: 1;
        text-style: bold;
        color: $accent;
    }
    ThemePopup ListView {
        height: auto;
        max-height: 6;
        background: $surface;
    }
    ThemePopup ListItem {
        height: 1;
        padding: 0 1;
    }
    """

    class ThemeSelected(Message):
        """Emitted when the user selects a theme."""

        def __init__(self, theme_name: str) -> None:
            super().__init__()
            self.theme_name = theme_name

    class Dismissed(Message):
        """Emitted when the popup is closed without selecting a theme."""

    def __init__(self, **kwargs: Any) -> None:
        super().__init__(**kwargs)
        self._theme_names: list[str] = []

    def compose(self) -> ComposeResult:
        """Build the theme popup layout."""
        yield Label("Select Theme", id="theme-title")
        yield ListView(id="theme-list")

    def show(self, theme_names: list[str], current_theme: str = "") -> None:
        """Populate and display the theme popup.

        Args:
            theme_names: Available theme names.
            current_theme: The currently active theme (highlighted).
        """
        self._theme_names = list(theme_names)
        list_view = self.query_one("#theme-list", ListView)
        list_view.clear()

        for name in self._theme_names:
            marker = " *" if name == current_theme else ""
            li = ListItem(Label(f"{name}{marker}"))
            list_view.append(li)

        self.add_class("visible")
        list_view.focus()

    def dismiss(self) -> None:
        """Hide the popup and clear state."""
        self.remove_class("visible")
        self._theme_names = []
        self.post_message(self.Dismissed())

    @property
    def is_visible(self) -> bool:
        """Whether the popup is currently shown."""
        return self.has_class("visible")

    @property
    def theme_names(self) -> list[str]:
        """Currently listed theme names."""
        return list(self._theme_names)

    def on_list_view_selected(self, event: ListView.Selected) -> None:
        """Handle Enter on a theme."""
        index = event.list_view.index
        if index is None or index < 0 or index >= len(self._theme_names):
            return

        theme_name = self._theme_names[index]
        self.remove_class("visible")
        self.post_message(self.ThemeSelected(theme_name=theme_name))

    def on_key(self, event: object) -> None:
        """Handle Escape to dismiss the popup."""
        key = getattr(event, "key", "")
        if key == "escape":
            self.dismiss()
            stop = getattr(event, "stop", None)
            if callable(stop):
                stop()


# ---------------------------------------------------------------------------
# PlaylistPickerPopup
# ---------------------------------------------------------------------------


class PlaylistPickerPopup(Static):
    """Overlay popup for selecting a playlist to add a track to."""

    DEFAULT_CSS = """
    PlaylistPickerPopup {
        layer: overlay;
        dock: bottom;
        offset-y: -4;
        height: auto;
        max-height: 14;
        background: $surface;
        border-top: solid $accent;
        padding: 0 1;
        display: none;
    }
    PlaylistPickerPopup.visible {
        display: block;
    }
    PlaylistPickerPopup #picker-title {
        height: 1;
        text-style: bold;
        color: $accent;
    }
    PlaylistPickerPopup ListView {
        height: auto;
        max-height: 10;
        background: $surface;
    }
    PlaylistPickerPopup ListItem {
        height: 1;
        padding: 0 1;
    }
    """

    class PlaylistChosen(Message):
        """Emitted when a playlist is selected."""

        def __init__(self, playlist_id: str | None, track: Any) -> None:
            super().__init__()
            self.playlist_id = playlist_id
            self.track = track

    class Dismissed(Message):
        """Emitted when the popup is closed."""

    _NEW_PLAYLIST_SENTINEL = "__new__"

    def __init__(self, **kwargs: Any) -> None:
        super().__init__(**kwargs)
        self._playlists: list[tuple[str, str]] = []
        self._track: Any = None

    def compose(self) -> ComposeResult:
        yield Label("Add to playlist", id="picker-title")
        yield ListView(id="picker-list")

    def show(self, playlists: list[tuple[str, str]], track: Any) -> None:
        self._playlists = list(playlists)
        self._track = track
        list_view = self.query_one("#picker-list", ListView)
        list_view.clear()
        list_view.append(ListItem(Label("+ New playlist...")))
        for _pid, title in self._playlists:
            list_view.append(ListItem(Label(title)))
        self.add_class("visible")
        list_view.focus()

    def dismiss(self) -> None:
        self.remove_class("visible")
        self._playlists = []
        self._track = None
        self.post_message(self.Dismissed())

    @property
    def is_visible(self) -> bool:
        return self.has_class("visible")

    def on_list_view_selected(self, event: ListView.Selected) -> None:
        index = event.list_view.index
        if index is None or index < 0:
            return
        if index == 0:
            playlist_id = self._NEW_PLAYLIST_SENTINEL
        else:
            adj = index - 1
            if adj >= len(self._playlists):
                return
            playlist_id = self._playlists[adj][0]
        track = self._track
        self.remove_class("visible")
        self.post_message(self.PlaylistChosen(playlist_id=playlist_id, track=track))

    def on_key(self, event: object) -> None:
        key = getattr(event, "key", "")
        if key == "escape":
            self.dismiss()
            stop = getattr(event, "stop", None)
            if callable(stop):
                stop()


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _item_title(item: Track | PlaylistInfo | AlbumInfo) -> str:
    """Extract a human-readable title from a popup target item."""
    from ytmusic_tui.api import AlbumInfo as _AlbumInfo
    from ytmusic_tui.api import PlaylistInfo as _PlaylistInfo
    from ytmusic_tui.queue import Track as _Track

    if isinstance(item, _Track):
        return f"{item.title} - {item.artist}" if item.artist else item.title
    if isinstance(item, _PlaylistInfo):
        return item.title
    if isinstance(item, _AlbumInfo):
        return f"{item.title} - {item.artist}" if item.artist else item.title
    return str(item)
