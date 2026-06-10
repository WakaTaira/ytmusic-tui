"""Main Textual application."""

from __future__ import annotations

import contextlib
from pathlib import Path
from typing import TYPE_CHECKING, ClassVar

from textual import work
from textual.app import App, ComposeResult
from textual.binding import Binding
from textual.css.query import NoMatches
from textual.widgets import ContentSwitcher, Header, Static

from ytmusic_tui.actions import BrowseActions, PlaybackActions, PopupActions
from ytmusic_tui.api import MusicAPI
from ytmusic_tui.auth import validate_auth_file
from ytmusic_tui.config import (
    AppConfig,
    build_textual_theme,
    load_config,
    load_keymap,
)
from ytmusic_tui.layout import Orientation, detect_orientation
from ytmusic_tui.navigation import NavigationManager, PageState
from ytmusic_tui.player import Player
from ytmusic_tui.queue import QueueManager
from ytmusic_tui.views.album import AlbumView
from ytmusic_tui.views.artist import ArtistView
from ytmusic_tui.views.history import HistoryView
from ytmusic_tui.views.home import HomeView
from ytmusic_tui.views.library import LibraryView
from ytmusic_tui.views.lyrics import LyricsView
from ytmusic_tui.views.player import PlayerBar
from ytmusic_tui.views.playlist import PlaylistView
from ytmusic_tui.views.popup import ActionPopup, PlaylistPickerPopup, ThemePopup
from ytmusic_tui.views.queue import QueueView
from ytmusic_tui.views.search import SearchView

if TYPE_CHECKING:
    from ytmusic_tui.mpris import MprisService

# Default browser auth JSON path (used when no config loaded)
_DEFAULT_AUTH_PATH = Path.home() / ".config" / "ytmusic-tui" / "browser.json"


class YtMusicTui(PlaybackActions, BrowseActions, PopupActions, App[None]):
    """YouTube Music TUI client."""

    TITLE = "ytmusic-tui"
    CSS_PATH = "app.tcss"

    # Narrower than App's BindingType list: _apply_keymap relies on
    # every entry being a full Binding.
    BINDINGS: ClassVar[list[Binding]] = [  # type: ignore[assignment]
        Binding("space", "toggle_pause", "Play/Pause", show=True),
        Binding("n", "next_track", "Next", show=True),
        Binding("p", "previous_track", "Prev", show=True),
        Binding("s", "toggle_shuffle", "Shuffle", show=True),
        Binding("r", "cycle_repeat", "Repeat", show=True),
        Binding("plus,equal", "volume_up", "Vol+", show=False),
        Binding("minus", "volume_down", "Vol-", show=False),
        Binding("greater_than_sign", "seek_forward", "Seek +5s", show=False),
        Binding("less_than_sign", "seek_backward", "Seek -5s", show=False),
        Binding("circumflex_accent", "seek_start", "Seek 0:00", show=False),
        Binding("underscore", "toggle_mute", "Mute", show=False),
        Binding("f", "toggle_like", "Like", show=True),
        Binding("R", "start_radio", "Radio", show=True, key_display="R"),
        Binding("H", "switch_view('history')", "History", show=False, key_display="H"),
        Binding("slash", "toggle_filter", "Filter", show=True),
        Binding("g", "switch_view('home')", "Home", show=True),
        Binding("l", "switch_view('library')", "Library", show=True),
        Binding("q", "switch_view('queue')", "Queue", show=True),
        Binding("Q", "quit", "Quit", show=True, key_display="Q"),
        Binding("a", "open_current_artist", "Artist", show=True),
        Binding("A", "open_current_album", "Album", show=True, key_display="A"),
        Binding("escape", "go_back", "Back", show=False),
        Binding("full_stop", "open_action_popup", "Actions", show=True),
        Binding("T", "open_theme_popup", "Theme", show=True, key_display="T"),
        Binding("L", "open_lyrics", "Lyrics", show=True, key_display="L"),
        Binding("1", "switch_view('home')", "Home", show=False),
        Binding("2", "switch_view('search')", "Search", show=False),
        Binding("3", "switch_view('library')", "Library", show=False),
        Binding("4", "switch_view('playlist')", "Playlist", show=False),
        Binding("5", "switch_view('queue')", "Queue", show=False),
        Binding("6", "switch_view('album')", "Album", show=False),
        Binding("7", "switch_view('artist')", "Artist", show=False),
        Binding("8", "open_lyrics", "Lyrics", show=False),
    ]

    _ACTION_TO_TEXTUAL: ClassVar[dict[str, str]] = {
        "toggle_pause": "toggle_pause",
        "next_track": "next_track",
        "previous_track": "previous_track",
        "toggle_shuffle": "toggle_shuffle",
        "cycle_repeat": "cycle_repeat",
        "volume_up": "volume_up",
        "volume_down": "volume_down",
        "seek_forward": "seek_forward",
        "seek_backward": "seek_backward",
        "seek_start": "seek_start",
        "toggle_mute": "toggle_mute",
        "toggle_like": "toggle_like",
        "start_radio": "start_radio",
        "switch_history": "switch_view('history')",
        "focus_search": "toggle_filter",
        "switch_home": "switch_view('home')",
        "switch_library": "switch_view('library')",
        "switch_queue": "switch_view('queue')",
        "quit": "quit",
        "open_current_artist": "open_current_artist",
        "open_current_album": "open_current_album",
        "go_back": "go_back",
        "open_action_popup": "open_action_popup",
        "open_theme_popup": "open_theme_popup",
        "open_lyrics": "open_lyrics",
    }

    def __init__(
        self,
        auth_path: str | Path | None = None,
        config: AppConfig | None = None,
        keymap_path: Path | None = None,
    ) -> None:
        super().__init__()
        self.config = config if config is not None else load_config()

        self._keymap = load_keymap(keymap_path=keymap_path)
        self._apply_keymap(self._keymap)

        self._orientation: Orientation = Orientation.HORIZONTAL

        if auth_path is not None:
            resolved_path = str(auth_path)
        else:
            cfg_path = self.config.auth.browser_auth_path
            resolved_path = str(Path(cfg_path).expanduser())

        self._auth_path = resolved_path
        self.music_api = MusicAPI(resolved_path)
        self.player = Player()
        self.queue_manager = QueueManager()
        self.nav = NavigationManager(PageState(page_type="home"))

        self.player.set_volume(self.config.player.volume)

        self._mpris: MprisService | None = None

    # -----------------------------------------------------------------
    # Keymap
    # -----------------------------------------------------------------

    def _apply_keymap(self, keymap: dict[str, str]) -> None:
        action_index: dict[str, int] = {}
        for idx, binding in enumerate(self.BINDINGS):
            action_index.setdefault(binding.action, idx)

        new_bindings: list[Binding] = list(self.BINDINGS)

        for action_name, key in keymap.items():
            textual_action = self._ACTION_TO_TEXTUAL.get(action_name)
            if textual_action is None:
                continue
            target_idx = action_index.get(textual_action)
            if target_idx is None:
                continue
            old = new_bindings[target_idx]
            if old.key != key:
                new_bindings[target_idx] = Binding(
                    key,
                    old.action,
                    old.description,
                    show=old.show,
                    key_display=old.key_display if old.key_display else None,
                )

        self._bindings.key_to_bindings.clear()
        for binding in new_bindings:
            self._bindings.bind(
                binding.key,
                binding.action,
                binding.description,
                show=binding.show,
                key_display=binding.key_display if binding.key_display else "",
            )

    # -----------------------------------------------------------------
    # Responsive layout
    # -----------------------------------------------------------------

    def on_resize(self, event: object) -> None:
        size = getattr(event, "size", None)
        if size is None:
            return
        new_orientation = detect_orientation(size.width, size.height)
        if new_orientation != self._orientation:
            self._orientation = new_orientation
            self._notify_views_orientation(new_orientation)

    def _notify_views_orientation(self, orientation: Orientation) -> None:
        with contextlib.suppress(NoMatches):
            self.query_one(LibraryView).update_orientation(orientation)
        with contextlib.suppress(NoMatches):
            self.query_one(SearchView).update_orientation(orientation)

    def compose(self) -> ComposeResult:
        yield Header()
        with ContentSwitcher(initial="home"):
            yield HomeView(id="home")
            yield SearchView(id="search")
            yield LibraryView(id="library")
            yield PlaylistView(id="playlist")
            yield QueueView(id="queue")
            yield AlbumView(id="album")
            yield ArtistView(id="artist")
            yield LyricsView(id="lyrics")
            yield HistoryView(id="history")
        yield ActionPopup(id="action-popup")
        yield ThemePopup(id="theme-popup")
        yield PlaylistPickerPopup(id="playlist-picker")
        yield PlayerBar()

    def on_mount(self) -> None:
        self.player.on_track_end = self._on_track_end
        # mpv fires end-file callbacks on its own event thread, so the
        # error handler must re-enter the Textual app via call_from_thread,
        # just like the MPRIS callbacks below.
        self.player.on_track_error = lambda description: self.call_from_thread(
            self._on_playback_error, description
        )

        textual_theme = build_textual_theme(self.config.ui.theme)
        self.register_theme(textual_theme)
        self.theme = textual_theme.name

        auth_warning = validate_auth_file(self._auth_path)
        if auth_warning:
            self.notify(auth_warning, severity="warning", timeout=8)
        else:
            self._probe_session()

        try:
            from ytmusic_tui.mpris import MprisService

            self._mpris = MprisService()
            # MPRIS callbacks fire on the D-Bus event loop thread, so they
            # must re-enter the Textual app via call_from_thread.
            self._mpris.start(
                on_play_pause=lambda: self.call_from_thread(self.action_toggle_pause),
                on_next=lambda: self.call_from_thread(self.action_next_track),
                on_previous=lambda: self.call_from_thread(self.action_previous_track),
                on_stop=lambda: self.call_from_thread(self.player.stop),
            )
        except Exception:
            self._mpris = None

    @work(thread=True)
    def _probe_session(self) -> None:
        """Warn when the cookies look signed out.

        Stale browser cookies do not raise auth errors: YouTube serves
        logged-out pages (HTTP 200) and library views silently come back
        empty. Probe once at startup so the user gets an actionable hint
        instead of a mysteriously empty library.
        """
        if self.music_api.is_session_valid():
            return
        self.call_from_thread(
            self.notify,
            "YouTube session looks signed out — library will be empty. Run: ytmusic-tui auth",
            severity="warning",
            timeout=10,
        )

    # -----------------------------------------------------------------
    # Navigation
    # -----------------------------------------------------------------

    def action_switch_view(self, view_id: str) -> None:
        self._navigate_to(PageState(page_type=view_id))

    def _navigate_to(self, page: PageState) -> None:
        self.nav.push(page)
        self._apply_page(page)

    def _apply_page(self, page: PageState) -> None:
        switcher = self.query_one(ContentSwitcher)
        switcher.current = page.page_type

        if page.page_type == "queue":
            self.query_one(QueueView).refresh_queue()
        if page.page_type == "history":
            self.query_one(HistoryView).refresh_history()
        if page.page_type == "library":
            self.query_one(LibraryView).refresh_library()
        if page.page_type == "album" and "browse_id" in page.context:
            self.query_one(AlbumView).load_album(page.context["browse_id"])
        if page.page_type == "artist" and "channel_id" in page.context:
            self.query_one(ArtistView).load_artist(page.context["channel_id"])
        if page.page_type == "lyrics" and "video_id" in page.context:
            track = self.queue_manager.current_track
            self.query_one(LyricsView).load_lyrics(
                page.context["video_id"],
                title=track.title if track else "",
                artist=track.artist if track else "",
            )

    def action_focus_search(self) -> None:
        self.action_switch_view("search")
        self.query_one(SearchView).focus_input()

    def action_toggle_filter(self) -> None:
        switcher = self.query_one(ContentSwitcher)
        current_id = switcher.current
        view_map: dict[str, type[Static]] = {
            "home": HomeView,
            "search": SearchView,
            "library": LibraryView,
            "playlist": PlaylistView,
            "queue": QueueView,
            "album": AlbumView,
            "artist": ArtistView,
            "lyrics": LyricsView,
            "history": HistoryView,
        }
        view_cls = view_map.get(current_id or "")
        if view_cls is None:
            return
        try:
            view = self.query_one(view_cls)
        except Exception:
            return
        toggle = getattr(view, "toggle_filter", None)
        if toggle is not None:
            toggle()

    def action_go_back(self) -> None:
        picker = self.query_one(PlaylistPickerPopup)
        if picker.is_visible:
            picker.dismiss()
            return
        action_popup = self.query_one(ActionPopup)
        theme_popup = self.query_one(ThemePopup)
        if action_popup.is_visible:
            action_popup.dismiss()
            return
        if theme_popup.is_visible:
            theme_popup.dismiss()
            return

        previous = self.nav.pop()
        if previous is not None:
            self._apply_page(previous)
        else:
            home = PageState(page_type="home")
            self.nav.replace(home)
            self._apply_page(home)

    # -----------------------------------------------------------------
    # Shutdown
    # -----------------------------------------------------------------

    def on_unmount(self) -> None:
        with contextlib.suppress(Exception):
            if self._mpris is not None:
                self._mpris.shutdown()
        with contextlib.suppress(Exception):
            self.player.shutdown()


def main() -> None:
    """Entry point for ytmusic-tui.

    ``ytmusic-tui auth`` runs the interactive browser-auth setup
    instead of starting the TUI.
    """
    import sys

    if len(sys.argv) > 1 and sys.argv[1] == "auth":
        from ytmusic_tui.auth import run_auth_setup

        raise SystemExit(run_auth_setup())

    app = YtMusicTui()
    app.run()
