"""Main Textual application."""

from __future__ import annotations

import contextlib
from pathlib import Path
from typing import ClassVar

from textual import work
from textual.app import App, ComposeResult
from textual.binding import Binding
from textual.widgets import ContentSwitcher, Header

from ytmusic_tui.api import MusicAPI
from ytmusic_tui.config import (
    THEMES,
    AppConfig,
    build_textual_theme,
    load_config,
    load_keymap,
)
from ytmusic_tui.layout import Orientation, detect_orientation
from ytmusic_tui.navigation import NavigationManager, PageState
from ytmusic_tui.player import Player
from ytmusic_tui.queue import QueueManager, Track
from ytmusic_tui.views.album import AlbumView
from ytmusic_tui.views.artist import ArtistView
from ytmusic_tui.views.home import HomeView
from ytmusic_tui.views.library import LibraryView
from ytmusic_tui.views.player import PlayerBar
from ytmusic_tui.views.playlist import PlaylistView
from ytmusic_tui.views.popup import ActionKind, ActionPopup, ThemePopup
from ytmusic_tui.views.queue import QueueView
from ytmusic_tui.views.search import SearchView

# Default browser auth JSON path (used when no config loaded)
_DEFAULT_AUTH_PATH = Path.home() / ".config" / "ytmusic-tui" / "browser.json"

# Volume adjustment step size
_VOLUME_STEP = 5


class YtMusicTui(App):
    """YouTube Music TUI client."""

    TITLE = "ytmusic-tui"
    CSS_PATH = "app.tcss"

    BINDINGS: ClassVar[list[Binding]] = [
        # spotify_player-compatible keybindings
        Binding("space", "toggle_pause", "Play/Pause", show=True),
        Binding("n", "next_track", "Next", show=True),
        Binding("p", "previous_track", "Prev", show=True),
        Binding("s", "toggle_shuffle", "Shuffle", show=True),
        Binding("r", "cycle_repeat", "Repeat", show=True),
        Binding("plus,equal", "volume_up", "Vol+", show=False),
        Binding("minus", "volume_down", "Vol-", show=False),
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
        # Numeric shortcuts for direct view switching
        Binding("1", "switch_view('home')", "Home", show=False),
        Binding("2", "switch_view('search')", "Search", show=False),
        Binding("3", "switch_view('library')", "Library", show=False),
        Binding("4", "switch_view('playlist')", "Playlist", show=False),
        Binding("5", "switch_view('queue')", "Queue", show=False),
        Binding("6", "switch_view('album')", "Album", show=False),
        Binding("7", "switch_view('artist')", "Artist", show=False),
    ]

    # Maps keymap action names to the Textual action strings used in
    # BINDINGS.  Actions that take parameters (like switch_view) are
    # handled via switch_home / switch_library / switch_queue names.
    _ACTION_TO_TEXTUAL: ClassVar[dict[str, str]] = {
        "toggle_pause": "toggle_pause",
        "next_track": "next_track",
        "previous_track": "previous_track",
        "toggle_shuffle": "toggle_shuffle",
        "cycle_repeat": "cycle_repeat",
        "volume_up": "volume_up",
        "volume_down": "volume_down",
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
    }

    def __init__(
        self,
        auth_path: str | Path | None = None,
        config: AppConfig | None = None,
        keymap_path: Path | None = None,
    ) -> None:
        """Initialize the application.

        Args:
            auth_path: Path to the browser authentication JSON.
                       Defaults to the value from config or
                       ~/.config/ytmusic-tui/browser.json.
            config: Pre-loaded config (mainly for testing).
                    When ``None``, :func:`load_config` is called.
            keymap_path: Override path for keymap.toml (testing).
        """
        super().__init__()
        self.config = config if config is not None else load_config()

        # Load keymap and apply overrides to bindings
        self._keymap = load_keymap(keymap_path=keymap_path)
        self._apply_keymap(self._keymap)

        # Track current orientation for responsive layouts
        self._orientation: Orientation = Orientation.HORIZONTAL

        # Resolve auth path: explicit arg > config > default
        if auth_path is not None:
            resolved_path = str(auth_path)
        else:
            cfg_path = self.config.auth.browser_auth_path
            resolved_path = str(Path(cfg_path).expanduser())

        self.music_api = MusicAPI(resolved_path)
        self.player = Player()
        self.queue_manager = QueueManager()
        self.nav = NavigationManager(PageState(page_type="home"))

        # Apply initial volume from config
        self.player.set_volume(self.config.player.volume)

    # -----------------------------------------------------------------
    # Keymap support
    # -----------------------------------------------------------------

    def _apply_keymap(self, keymap: dict[str, str]) -> None:
        """Rebuild instance bindings from the class BINDINGS with keymap overrides.

        For each action in *keymap* that differs from the compiled-in
        default key, the matching Binding is replaced with the new key.
        Numeric shortcuts (1-7) are never remapped.
        """
        # Build a reverse map: textual_action -> index in BINDINGS
        action_index: dict[str, int] = {}
        for idx, binding in enumerate(self.BINDINGS):
            action_index.setdefault(binding.action, idx)

        # Create a mutable copy
        new_bindings: list[Binding] = list(self.BINDINGS)

        for action_name, key in keymap.items():
            textual_action = self._ACTION_TO_TEXTUAL.get(action_name)
            if textual_action is None:
                continue

            idx = action_index.get(textual_action)
            if idx is None:
                continue

            old = new_bindings[idx]
            if old.key != key:
                new_bindings[idx] = Binding(
                    key,
                    old.action,
                    old.description,
                    show=old.show,
                    key_display=old.key_display if old.key_display else None,
                )

        # Replace the instance-level bindings
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
        """Re-evaluate orientation when the terminal is resized."""
        size = getattr(event, "size", None)
        if size is None:
            return
        new_orientation = detect_orientation(size.width, size.height)
        if new_orientation != self._orientation:
            self._orientation = new_orientation
            self._notify_views_orientation(new_orientation)

    def _notify_views_orientation(self, orientation: Orientation) -> None:
        """Tell responsive views about the new orientation."""
        with contextlib.suppress(Exception):
            library_view = self.query_one(LibraryView)
            library_view.update_orientation(orientation)
        with contextlib.suppress(Exception):
            search_view = self.query_one(SearchView)
            search_view.update_orientation(orientation)

    def compose(self) -> ComposeResult:
        """Build the application layout."""
        yield Header()
        with ContentSwitcher(initial="home"):
            yield HomeView(id="home")
            yield SearchView(id="search")
            yield LibraryView(id="library")
            yield PlaylistView(id="playlist")
            yield QueueView(id="queue")
            yield AlbumView(id="album")
            yield ArtistView(id="artist")
        yield ActionPopup(id="action-popup")
        yield ThemePopup(id="theme-popup")
        yield PlayerBar()

    def on_mount(self) -> None:
        """Wire up player callbacks and apply the configured theme."""
        self.player.on_track_end = self._on_track_end

        # Register and apply the configured theme
        textual_theme = build_textual_theme(self.config.ui.theme)
        self.register_theme(textual_theme)
        self.theme = textual_theme.name

    def _on_track_end(self) -> None:
        """Auto-advance to the next track when the current one finishes."""
        next_track = self.queue_manager.next_track()
        if next_track is not None:
            self.player.play(next_track.video_id)

    # -----------------------------------------------------------------
    # Actions
    # -----------------------------------------------------------------

    def action_switch_view(self, view_id: str) -> None:
        """Switch the active content view, pushing current page to history."""
        self._navigate_to(PageState(page_type=view_id))

    def _navigate_to(self, page: PageState) -> None:
        """Navigate to *page*, pushing the current page onto the history stack.

        This is the single entry point for all navigation.  Views should
        call ``action_switch_view``, ``action_open_album``, or
        ``action_open_artist`` which delegate here.
        """
        self.nav.push(page)
        self._apply_page(page)

    def _apply_page(self, page: PageState) -> None:
        """Apply *page* to the UI without touching the history stack.

        Switches the ContentSwitcher, refreshes view-specific state,
        and loads contextual data (album / artist) when present.
        """
        switcher = self.query_one(ContentSwitcher)
        switcher.current = page.page_type

        # Refresh queue view when switching to it
        if page.page_type == "queue":
            queue_view = self.query_one(QueueView)
            queue_view.refresh_queue()

        # Refresh library view when switching to it
        if page.page_type == "library":
            library_view = self.query_one(LibraryView)
            library_view.on_mount()

        # Load contextual data for album / artist pages
        if page.page_type == "album" and "browse_id" in page.context:
            album_view = self.query_one(AlbumView)
            album_view.load_album(page.context["browse_id"])

        if page.page_type == "artist" and "channel_id" in page.context:
            artist_view = self.query_one(ArtistView)
            artist_view.load_artist(page.context["channel_id"])

    def action_toggle_pause(self) -> None:
        """Toggle play/pause on the player.

        If nothing is currently loaded but the queue has a track, start
        playing it instead of sending a no-op pause toggle to mpv.
        """
        state = self.player.get_state()
        if not state.video_id and self.queue_manager.current_track is not None:
            self.player.play(self.queue_manager.current_track.video_id)
            return
        self.player.toggle_pause()

    def action_next_track(self) -> None:
        """Advance to the next track in the queue."""
        next_track = self.queue_manager.next_track()
        if next_track is not None:
            self.player.play(next_track.video_id)
        self._refresh_queue_view_if_active()

    def action_previous_track(self) -> None:
        """Go back to the previous track in the queue."""
        prev_track = self.queue_manager.previous_track()
        if prev_track is not None:
            self.player.play(prev_track.video_id)
        self._refresh_queue_view_if_active()

    def _refresh_queue_view_if_active(self) -> None:
        """Refresh the queue view display if it is the currently visible view."""
        switcher = self.query_one(ContentSwitcher)
        if switcher.current == "queue":
            with contextlib.suppress(Exception):
                queue_view = self.query_one(QueueView)
                queue_view.refresh_queue()

    def action_toggle_shuffle(self) -> None:
        """Toggle shuffle mode on the queue."""
        self.queue_manager.toggle_shuffle()

    def action_cycle_repeat(self) -> None:
        """Cycle through repeat modes (OFF -> ALL -> ONE -> OFF)."""
        self.queue_manager.cycle_repeat()

    def action_volume_up(self) -> None:
        """Increase volume by one step."""
        self.player.adjust_volume(_VOLUME_STEP)

    def action_volume_down(self) -> None:
        """Decrease volume by one step."""
        self.player.adjust_volume(-_VOLUME_STEP)

    def action_focus_search(self) -> None:
        """Switch to search view and focus the input."""
        self.action_switch_view("search")
        search_view = self.query_one(SearchView)
        search_view.focus_input()

    def action_toggle_filter(self) -> None:
        """Toggle the in-page filter bar on the active view.

        Delegates to the active view's ``toggle_filter()`` method.
        Views without filterable DataTables are silently ignored.
        """
        switcher = self.query_one(ContentSwitcher)
        current_id = switcher.current

        view_map: dict[str, type] = {
            "home": HomeView,
            "search": SearchView,
            "library": LibraryView,
            "playlist": PlaylistView,
            "queue": QueueView,
            "album": AlbumView,
            "artist": ArtistView,
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
        """Pop the history stack to return to the previous page.

        If the history is empty, fall back to the home view.
        """
        # Dismiss any open popup first
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
            # Fallback: navigate to home (without pushing history)
            home = PageState(page_type="home")
            self.nav.replace(home)
            self._apply_page(home)

    def action_open_album(self, browse_id: str) -> None:
        """Switch to album view and load the given album."""
        self._navigate_to(PageState(page_type="album", context={"browse_id": browse_id}))

    def action_open_artist(self, channel_id: str) -> None:
        """Switch to artist view and load the given artist."""
        self._navigate_to(PageState(page_type="artist", context={"channel_id": channel_id}))

    def action_open_current_artist(self) -> None:
        """Open the artist page for the currently playing track.

        Looks up the artist name from the queue, searches for the
        artist's channel ID, and navigates to ArtistView.
        """
        current = self.queue_manager.current_track
        if current is None or not current.artist:
            return

        self._lookup_and_open_artist(current.artist)

    @work(thread=True)
    def _lookup_and_open_artist(self, artist_name: str) -> None:
        """Search for an artist by name and open their page."""
        try:
            # search() only returns Track objects; use the raw client
            # to get artist results with browseId.
            raw_results = self.music_api._client.search(artist_name, filter="artists", limit=5)
            for item in raw_results:
                channel_id = item.get("browseId")
                if channel_id:
                    self.call_from_thread(self.action_open_artist, channel_id)
                    return
        except Exception:
            pass

    def action_open_current_album(self) -> None:
        """Open the album page for the currently playing track.

        Searches for the album's browse ID using the track's album
        name and navigates to AlbumView.
        """
        current = self.queue_manager.current_track
        if current is None or not current.album:
            return

        self._lookup_and_open_album(current.album, current.artist)

    @work(thread=True)
    def _lookup_and_open_album(self, album_name: str, artist_name: str = "") -> None:
        """Search for an album by name and open it."""
        try:
            query = f"{album_name} {artist_name}".strip()
            raw_results = self.music_api._client.search(query, filter="albums", limit=5)
            for item in raw_results:
                browse_id = item.get("browseId")
                if browse_id:
                    self.call_from_thread(self.action_open_album, browse_id)
                    return
        except Exception:
            pass

    # -----------------------------------------------------------------
    # Popup actions
    # -----------------------------------------------------------------

    def action_open_action_popup(self) -> None:
        """Open the action popup for the focused item in the current view."""
        item = self._get_focused_item()
        if item is None:
            return

        # Dismiss theme popup if open
        theme_popup = self.query_one(ThemePopup)
        if theme_popup.is_visible:
            theme_popup.dismiss()

        action_popup = self.query_one(ActionPopup)
        action_popup.show(item)

    def action_open_theme_popup(self) -> None:
        """Open the theme selection popup."""
        # Dismiss action popup if open
        action_popup = self.query_one(ActionPopup)
        if action_popup.is_visible:
            action_popup.dismiss()

        theme_popup = self.query_one(ThemePopup)
        theme_popup.show(
            theme_names=list(THEMES.keys()),
            current_theme=self.config.ui.theme,
        )

    def _get_focused_item(self) -> Track | object | None:
        """Ask the active view for its focused item.

        Returns the item (Track, PlaylistInfo, AlbumInfo) or None.
        """
        switcher = self.query_one(ContentSwitcher)
        current_id = switcher.current

        view_map: dict[str, type] = {
            "home": HomeView,
            "search": SearchView,
            "library": LibraryView,
            "playlist": PlaylistView,
            "queue": QueueView,
            "album": AlbumView,
            "artist": ArtistView,
        }

        view_cls = view_map.get(current_id or "")
        if view_cls is None:
            return None

        try:
            view = self.query_one(view_cls)
        except Exception:
            return None

        getter = getattr(view, "get_focused_item", None)
        if getter is None:
            return None

        return getter()

    def on_action_popup_action_selected(
        self,
        event: ActionPopup.ActionSelected,
    ) -> None:
        """Execute the action chosen from the action popup."""
        action = event.action
        item = event.item

        if action.kind is ActionKind.PLAY:
            if isinstance(item, Track):
                self.queue_manager.set_playlist([item], start_index=0)
                self.player.play(item.video_id)

        elif action.kind is ActionKind.ADD_TO_QUEUE:
            if isinstance(item, Track):
                self.queue_manager.add(item)

        elif action.kind is ActionKind.GO_TO_ARTIST:
            artist_name = ""
            if isinstance(item, Track) or hasattr(item, "artist"):
                artist_name = item.artist
            if artist_name:
                self._lookup_and_open_artist(artist_name)

        elif action.kind is ActionKind.GO_TO_ALBUM:
            if isinstance(item, Track) and item.album:
                self._lookup_and_open_album(item.album, item.artist)

        elif action.kind is ActionKind.PLAY_ALL:
            self._handle_play_all(item)

        elif action.kind is ActionKind.OPEN:
            self._handle_open(item)

    def _handle_play_all(self, item: object) -> None:
        """Handle 'Play all' action for playlists and albums."""
        from ytmusic_tui.api import AlbumInfo, PlaylistInfo

        if isinstance(item, PlaylistInfo):
            self._play_all_playlist(item.playlist_id)
        elif isinstance(item, AlbumInfo):
            self._play_all_album(item.browse_id)

    @work(thread=True)
    def _play_all_playlist(self, playlist_id: str) -> None:
        """Fetch playlist tracks and queue them all."""
        try:
            tracks = self.music_api.get_playlist_tracks(playlist_id)
            if tracks:
                self.call_from_thread(self._queue_and_play, tracks)
        except Exception:
            pass

    @work(thread=True)
    def _play_all_album(self, browse_id: str) -> None:
        """Fetch album tracks and queue them all."""
        try:
            album = self.music_api.get_album(browse_id)
            if album.tracks:
                self.call_from_thread(self._queue_and_play, album.tracks)
        except Exception:
            pass

    def _queue_and_play(self, tracks: list[Track]) -> None:
        """Queue a list of tracks and start playing the first one."""
        self.queue_manager.set_playlist(tracks, start_index=0)
        self.player.play(tracks[0].video_id)

    def _handle_open(self, item: object) -> None:
        """Handle 'Open' action for playlists and albums."""
        from ytmusic_tui.api import AlbumInfo, PlaylistInfo

        if isinstance(item, PlaylistInfo):
            self.action_switch_view("playlist")
            playlist_view = self.query_one(PlaylistView)
            playlist_view._show_track_list(item)
        elif isinstance(item, AlbumInfo):
            self.action_open_album(item.browse_id)

    def on_theme_popup_theme_selected(
        self,
        event: ThemePopup.ThemeSelected,
    ) -> None:
        """Apply the theme chosen from the theme popup."""
        theme_name = event.theme_name
        textual_theme = build_textual_theme(theme_name)
        self.register_theme(textual_theme)
        self.theme = textual_theme.name

    def on_unmount(self) -> None:
        """Clean up resources on exit."""
        with contextlib.suppress(Exception):
            self.player.shutdown()


def main() -> None:
    """Entry point for ytmusic-tui."""
    app = YtMusicTui()
    app.run()
