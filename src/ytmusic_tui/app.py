"""Main Textual application."""

from __future__ import annotations

import contextlib
from pathlib import Path
from typing import TYPE_CHECKING, ClassVar

from textual import work
from textual.app import App, ComposeResult
from textual.binding import Binding, BindingType
from textual.css.query import NoMatches
from textual.widgets import ContentSwitcher, Header

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
    from collections.abc import Mapping

    from ytmusic_tui.mpris import MprisService
    from ytmusic_tui.views.base import FetchView

# Default browser auth JSON path (used when no config loaded)
_DEFAULT_AUTH_PATH = Path.home() / ".config" / "ytmusic-tui" / "browser.json"

# Single source of truth mapping a ContentSwitcher pane id to its view
# class. Both action_toggle_filter and PopupActions._get_focused_item
# resolve the focused view through current_view(), which consults this
# registry. The keys mirror the ``id=`` values used in compose().
VIEW_REGISTRY: dict[str, type[FetchView]] = {
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


class YtMusicTui(PlaybackActions, BrowseActions, PopupActions, App[None]):
    """YouTube Music TUI client."""

    TITLE = "ytmusic-tui"
    CSS_PATH = "app.tcss"

    # Each remappable binding carries an ``id`` matching its canonical
    # keymap action name (the keys of config.DEFAULT_KEYMAP). The id is
    # what keymap.toml overrides target, so the keymap name can differ
    # freely from Textual's action string (e.g. id="search" binds the
    # "toggle_filter" action; id="switch_home" binds
    # "switch_view('home')"). _apply_keymap feeds these ids to
    # App.set_keymap.
    #
    # The number-key bindings (1-8) and the secondary "Quit" alias are
    # deliberately id-less: they are not exposed in keymap.toml, so they
    # keep their compiled-in keys regardless of any override.
    #
    # Annotated with App's own ClassVar[list[BindingType]] (not the
    # narrower list[Binding]) so no type-ignore is needed for the
    # invariant-list override; _apply_keymap narrows to Binding at the
    # read site with isinstance.
    BINDINGS: ClassVar[list[BindingType]] = [
        Binding("space", "toggle_pause", "Play/Pause", show=True, id="toggle_pause"),
        Binding("n", "next_track", "Next", show=True, id="next_track"),
        Binding("p", "previous_track", "Prev", show=True, id="previous_track"),
        Binding("s", "toggle_shuffle", "Shuffle", show=True, id="toggle_shuffle"),
        Binding("r", "cycle_repeat", "Repeat", show=True, id="cycle_repeat"),
        Binding("plus,equal", "volume_up", "Vol+", show=False, id="volume_up"),
        Binding("minus", "volume_down", "Vol-", show=False, id="volume_down"),
        Binding("greater_than_sign", "seek_forward", "Seek +5s", show=False, id="seek_forward"),
        Binding("less_than_sign", "seek_backward", "Seek -5s", show=False, id="seek_backward"),
        Binding("circumflex_accent", "seek_start", "Seek 0:00", show=False, id="seek_start"),
        Binding("underscore", "toggle_mute", "Mute", show=False, id="toggle_mute"),
        Binding("b", "cycle_audio_quality", "Quality", show=False, id="cycle_audio_quality"),
        Binding("f", "toggle_like", "Like", show=True, id="toggle_like"),
        Binding("R", "start_radio", "Radio", show=True, key_display="R", id="start_radio"),
        Binding(
            "H",
            "switch_view('history')",
            "History",
            show=False,
            key_display="H",
            id="switch_history",
        ),
        Binding("slash", "toggle_filter", "Filter", show=True, id="search"),
        Binding("g", "switch_view('home')", "Home", show=True, id="switch_home"),
        Binding("l", "switch_view('library')", "Library", show=True, id="switch_library"),
        Binding("q", "switch_view('queue')", "Queue", show=True, id="switch_queue"),
        Binding("Q", "quit", "Quit", show=True, key_display="Q", id="quit"),
        Binding("a", "open_current_artist", "Artist", show=True, id="open_current_artist"),
        Binding(
            "A", "open_current_album", "Album", show=True, key_display="A", id="open_current_album"
        ),
        Binding("escape", "go_back", "Back", show=False, id="go_back"),
        Binding("full_stop", "open_action_popup", "Actions", show=True, id="open_action_popup"),
        Binding(
            "T", "open_theme_popup", "Theme", show=True, key_display="T", id="open_theme_popup"
        ),
        Binding("L", "open_lyrics", "Lyrics", show=True, key_display="L", id="open_lyrics"),
        Binding("1", "switch_view('home')", "Home", show=False),
        Binding("2", "switch_view('search')", "Search", show=False),
        Binding("3", "switch_view('library')", "Library", show=False),
        Binding("4", "switch_view('playlist')", "Playlist", show=False),
        Binding("5", "switch_view('queue')", "Queue", show=False),
        Binding("6", "switch_view('album')", "Album", show=False),
        Binding("7", "switch_view('artist')", "Artist", show=False),
        Binding("8", "open_lyrics", "Lyrics", show=False),
    ]

    # Actions that are user-bindable via keymap.toml but ship with NO
    # default key. spotify_player binds "SearchPage" to the two-key
    # sequence ``g s``, which Textual cannot express, so we leave it
    # unbound and let the user assign a single key if they want it.
    #
    # Maps keymap name -> Textual action string. Deliberately absent from
    # config.DEFAULT_KEYMAP (no default key). _apply_keymap binds any
    # keymap.toml entry whose name appears here.
    UNBOUND_ACTIONS: ClassVar[Mapping[str, str]] = {"search_page": "search_page"}

    def __init__(
        self,
        auth_path: str | Path | None = None,
        config: AppConfig | None = None,
        keymap_path: Path | None = None,
    ) -> None:
        super().__init__()
        self.config = config if config is not None else load_config()

        self._apply_keymap(load_keymap(keymap_path=keymap_path))

        self._orientation: Orientation = Orientation.HORIZONTAL

        if auth_path is not None:
            resolved_path = str(auth_path)
        else:
            cfg_path = self.config.auth.browser_auth_path
            resolved_path = str(Path(cfg_path).expanduser())

        self._auth_path = resolved_path
        self.music_api = MusicAPI(resolved_path)
        self.player = Player(audio_quality=self.config.player.audio_quality)
        self.queue_manager = QueueManager()
        self.nav = NavigationManager(PageState(page_type="home"))

        self.player.set_volume(self.config.player.volume)

        self._mpris: MprisService | None = None

    # -----------------------------------------------------------------
    # Keymap
    # -----------------------------------------------------------------

    def _apply_keymap(self, keymap: dict[str, str]) -> None:
        """Apply a loaded keymap via Textual's official keymap mechanism.

        ``keymap`` maps canonical action names to key strings (the shape
        returned by :func:`config.load_keymap`). Each name is matched
        against a ``Binding.id`` in :attr:`BINDINGS`; matching entries are
        handed to :meth:`App.set_keymap`, which overrides the compiled-in
        key per binding id and refreshes the Footer.

        Names that are neither a binding id nor a key of
        :attr:`UNBOUND_ACTIONS` (e.g. stale entries in a user
        keymap.toml) are dropped. Names in :attr:`UNBOUND_ACTIONS` have no
        compiled-in binding, so any key the user assigns them is added via
        :meth:`App.bind`.

        Safe to call from ``__init__``: ``set_keymap`` only stores the
        mapping and the override is applied lazily when active bindings
        are computed (after mount); ``bind`` mutates the binding table
        directly.
        """
        binding_ids = {
            binding.id
            for binding in self.BINDINGS
            if isinstance(binding, Binding) and binding.id is not None
        }
        self.set_keymap({name: key for name, key in keymap.items() if name in binding_ids})

        # Bind the unbound-but-bindable actions the user opted into.
        # App.bind is documented as public-but-warned ("may be private or
        # removed in the future"). We accept that risk because these binds
        # happen once at startup; if the method disappears we would switch
        # to a hidden Binding(..., show=False) plus set_keymap instead.
        for name, action in self.UNBOUND_ACTIONS.items():
            key = keymap.get(name)
            if key is not None:
                self.bind(key, action, description="Search page", show=False)

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
        except ImportError:
            # dbus-fast not installed (non-Linux environment) — expected,
            # silently skip MPRIS setup.
            self._mpris = None
        except Exception:
            # Unexpected startup error (D-Bus misconfigured, session bus
            # unavailable, etc.) — tell the user desktop controls are off.
            self._mpris = None
            self.notify(
                "MPRIS unavailable — desktop controls disabled",
                severity="warning",
                timeout=8,
            )

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

    def action_search_page(self) -> None:
        """Switch to the search view and focus its input.

        Reachable only when the user binds ``search_page`` in keymap.toml
        (see :attr:`UNBOUND_ACTIONS`); ships unbound by default.

        The input focus is deferred via ``call_after_refresh`` so it wins
        the race against ``SearchView.on_show``, which otherwise focuses
        the active result pane when the view becomes visible.
        """
        self.action_switch_view("search")
        self.call_after_refresh(self.query_one(SearchView).focus_input)

    def current_view(self) -> FetchView | None:
        """Return the currently displayed view, or ``None`` if unresolved.

        Resolves the ContentSwitcher's active pane id through
        :data:`VIEW_REGISTRY` and queries the live widget. Shared by
        :meth:`action_toggle_filter` and
        ``PopupActions._get_focused_item`` so the pane-id -> class ->
        widget lookup lives in exactly one place.
        """
        switcher = self.query_one(ContentSwitcher)
        view_cls = VIEW_REGISTRY.get(switcher.current or "")
        if view_cls is None:
            return None
        try:
            return self.query_one(view_cls)
        except NoMatches:
            return None

    def action_toggle_filter(self) -> None:
        view = self.current_view()
        if view is None:
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
