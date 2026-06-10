"""Main Textual application."""

from __future__ import annotations

import contextlib
from pathlib import Path
from typing import TYPE_CHECKING, ClassVar

from textual import work
from textual.app import App, ComposeResult
from textual.binding import Binding
from textual.widgets import ContentSwitcher, Header

from ytmusic_tui.api import MusicAPI
from ytmusic_tui.auth import classify_api_error, is_auth_error, validate_auth_file
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
from ytmusic_tui.views.lyrics import LyricsView
from ytmusic_tui.views.player import PlayerBar
from ytmusic_tui.views.playlist import PlaylistView
from ytmusic_tui.views.popup import ActionKind, ActionPopup, PlaylistPickerPopup, ThemePopup
from ytmusic_tui.views.queue import QueueView
from ytmusic_tui.views.search import SearchView

if TYPE_CHECKING:
    from ytmusic_tui.mpris import MprisService

# Default browser auth JSON path (used when no config loaded)
_DEFAULT_AUTH_PATH = Path.home() / ".config" / "ytmusic-tui" / "browser.json"

# Volume adjustment step size
_VOLUME_STEP = 5


class YtMusicTui(App):
    """YouTube Music TUI client."""

    TITLE = "ytmusic-tui"
    CSS_PATH = "app.tcss"

    BINDINGS: ClassVar[list[Binding]] = [
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
        with contextlib.suppress(Exception):
            self.query_one(LibraryView).update_orientation(orientation)
        with contextlib.suppress(Exception):
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
        yield ActionPopup(id="action-popup")
        yield ThemePopup(id="theme-popup")
        yield PlaylistPickerPopup(id="playlist-picker")
        yield PlayerBar()

    def on_mount(self) -> None:
        self.player.on_track_end = self._on_track_end

        textual_theme = build_textual_theme(self.config.ui.theme)
        self.register_theme(textual_theme)
        self.theme = textual_theme.name

        auth_warning = validate_auth_file(self._auth_path)
        if auth_warning:
            self.notify(auth_warning, severity="warning", timeout=8)

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

    def _on_track_end(self) -> None:
        next_track = self.queue_manager.next_track()
        if next_track is not None:
            self.player.play(next_track.video_id)

    # -----------------------------------------------------------------
    # Actions
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
        if page.page_type == "library":
            self.query_one(LibraryView).on_mount()
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

    def action_toggle_pause(self) -> None:
        state = self.player.get_state()
        if not state.video_id and self.queue_manager.current_track is not None:
            self.player.play(self.queue_manager.current_track.video_id)
            return
        self.player.toggle_pause()

    def action_next_track(self) -> None:
        next_track = self.queue_manager.next_track()
        if next_track is not None:
            self.player.play(next_track.video_id)
        self._refresh_queue_view_if_active()

    def action_previous_track(self) -> None:
        prev_track = self.queue_manager.previous_track()
        if prev_track is not None:
            self.player.play(prev_track.video_id)
        self._refresh_queue_view_if_active()

    def _refresh_queue_view_if_active(self) -> None:
        switcher = self.query_one(ContentSwitcher)
        if switcher.current == "queue":
            with contextlib.suppress(Exception):
                self.query_one(QueueView).refresh_queue()

    def action_toggle_shuffle(self) -> None:
        self.queue_manager.toggle_shuffle()

    def action_cycle_repeat(self) -> None:
        self.queue_manager.cycle_repeat()

    def action_volume_up(self) -> None:
        self.player.adjust_volume(_VOLUME_STEP)

    def action_volume_down(self) -> None:
        self.player.adjust_volume(-_VOLUME_STEP)

    def action_focus_search(self) -> None:
        self.action_switch_view("search")
        self.query_one(SearchView).focus_input()

    def action_toggle_filter(self) -> None:
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
            "lyrics": LyricsView,
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

    def action_open_lyrics(self) -> None:
        current = self.queue_manager.current_track
        if current is None:
            return
        self._navigate_to(
            PageState(
                page_type="lyrics",
                context={"video_id": current.video_id},
            )
        )

    def action_open_album(self, browse_id: str) -> None:
        self._navigate_to(PageState(page_type="album", context={"browse_id": browse_id}))

    def action_open_artist(self, channel_id: str) -> None:
        self._navigate_to(PageState(page_type="artist", context={"channel_id": channel_id}))

    def action_open_current_artist(self) -> None:
        current = self.queue_manager.current_track
        if current is None or not current.artist:
            return
        self._lookup_and_open_artist(current.artist)

    @work(thread=True)
    def _lookup_and_open_artist(self, artist_name: str) -> None:
        try:
            raw = self.music_api._client.search(artist_name, filter="artists", limit=5)
            for item in raw:
                cid = item.get("browseId")
                if cid:
                    self.call_from_thread(self.action_open_artist, cid)
                    return
        except Exception:
            pass

    def action_open_current_album(self) -> None:
        current = self.queue_manager.current_track
        if current is None or not current.album:
            return
        self._lookup_and_open_album(current.album, current.artist)

    @work(thread=True)
    def _lookup_and_open_album(self, album_name: str, artist_name: str = "") -> None:
        try:
            q = f"{album_name} {artist_name}".strip()
            raw = self.music_api._client.search(q, filter="albums", limit=5)
            for item in raw:
                bid = item.get("browseId")
                if bid:
                    self.call_from_thread(self.action_open_album, bid)
                    return
        except Exception:
            pass

    # -----------------------------------------------------------------
    # Popup actions
    # -----------------------------------------------------------------

    def action_open_action_popup(self) -> None:
        item = self._get_focused_item()
        if item is None:
            return
        theme_popup = self.query_one(ThemePopup)
        if theme_popup.is_visible:
            theme_popup.dismiss()
        switcher = self.query_one(ContentSwitcher)
        context = ""
        if switcher.current == "queue":
            context = "queue"
        elif switcher.current == "playlist":
            pv = self.query_one(PlaylistView)
            if getattr(pv, "_viewing_tracks", False):
                context = "playlist_tracks"
        self.query_one(ActionPopup).show(item, context=context)

    def action_open_theme_popup(self) -> None:
        action_popup = self.query_one(ActionPopup)
        if action_popup.is_visible:
            action_popup.dismiss()
        self.query_one(ThemePopup).show(
            theme_names=list(THEMES.keys()),
            current_theme=self.config.ui.theme,
        )

    def _get_focused_item(self) -> Track | object | None:
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
            "lyrics": LyricsView,
        }
        view_cls = view_map.get(current_id or "")
        if view_cls is None:
            return None
        try:
            view = self.query_one(view_cls)
        except Exception:
            return None
        getter = getattr(view, "get_focused_item", None)
        return getter() if getter is not None else None

    def on_action_popup_action_selected(self, event: ActionPopup.ActionSelected) -> None:
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
        elif action.kind is ActionKind.ADD_TO_PLAYLIST:
            if isinstance(item, Track):
                self._show_playlist_picker(item)
        elif action.kind is ActionKind.REMOVE_FROM_QUEUE:
            if isinstance(item, Track):
                self._remove_from_queue(item)
        elif action.kind is ActionKind.REMOVE_FROM_PLAYLIST:
            if isinstance(item, Track):
                self._remove_from_playlist(item)
        elif action.kind is ActionKind.PLAY_ALL:
            self._handle_play_all(item)
        elif action.kind is ActionKind.OPEN:
            self._handle_open(item)

    def _handle_play_all(self, item: object) -> None:
        from ytmusic_tui.api import AlbumInfo, PlaylistInfo

        if isinstance(item, PlaylistInfo):
            self._play_all_playlist(item.playlist_id)
        elif isinstance(item, AlbumInfo):
            self._play_all_album(item.browse_id)

    @work(thread=True)
    def _play_all_playlist(self, playlist_id: str) -> None:
        try:
            tracks = self.music_api.get_playlist_tracks(playlist_id)
            if tracks:
                self.call_from_thread(self._queue_and_play, tracks)
        except Exception:
            pass

    @work(thread=True)
    def _play_all_album(self, browse_id: str) -> None:
        try:
            album = self.music_api.get_album(browse_id)
            if album.tracks:
                self.call_from_thread(self._queue_and_play, album.tracks)
        except Exception:
            pass

    def _queue_and_play(self, tracks: list[Track]) -> None:
        self.queue_manager.set_playlist(tracks, start_index=0)
        self.player.play(tracks[0].video_id)

    def _handle_open(self, item: object) -> None:
        from ytmusic_tui.api import AlbumInfo, PlaylistInfo

        if isinstance(item, PlaylistInfo):
            self.action_switch_view("playlist")
            self.query_one(PlaylistView)._show_track_list(item)
        elif isinstance(item, AlbumInfo):
            self.action_open_album(item.browse_id)

    def on_theme_popup_theme_selected(self, event: ThemePopup.ThemeSelected) -> None:
        textual_theme = build_textual_theme(event.theme_name)
        self.register_theme(textual_theme)
        self.theme = textual_theme.name

    # -----------------------------------------------------------------
    # Playlist picker
    # -----------------------------------------------------------------

    @work(thread=True)
    def _show_playlist_picker(self, track: Track) -> None:
        try:
            playlists = self.music_api.get_library_playlists(limit=50)
            items = [(p.playlist_id, p.title) for p in playlists]
            self.call_from_thread(self._open_playlist_picker, items, track)
        except Exception:
            pass

    def _open_playlist_picker(self, playlists: list[tuple[str, str]], track: Track) -> None:
        self.query_one(PlaylistPickerPopup).show(playlists, track)

    def on_playlist_picker_popup_playlist_chosen(
        self,
        event: PlaylistPickerPopup.PlaylistChosen,
    ) -> None:
        track = event.track
        if not isinstance(track, Track) or not track.video_id:
            return
        playlist_id = event.playlist_id
        if playlist_id == PlaylistPickerPopup._NEW_PLAYLIST_SENTINEL:
            self._create_and_add(track)
        elif playlist_id:
            self._add_to_existing_playlist(playlist_id, track)

    @work(thread=True)
    def _create_and_add(self, track: Track) -> None:
        try:
            new_id = self.music_api.create_playlist("New Playlist")
            if new_id:
                self.music_api.add_playlist_items(new_id, [track.video_id])
        except Exception:
            pass

    @work(thread=True)
    def _add_to_existing_playlist(self, playlist_id: str, track: Track) -> None:
        try:
            self.music_api.add_playlist_items(playlist_id, [track.video_id])
        except Exception:
            pass

    # -----------------------------------------------------------------
    # Remove actions
    # -----------------------------------------------------------------

    def _remove_from_queue(self, track: Track) -> None:
        tracks = self.queue_manager.tracks
        for i, t in enumerate(tracks):
            if t.video_id == track.video_id:
                self.queue_manager.remove(i)
                break
        self._refresh_queue_view_if_active()

    @work(thread=True)
    def _remove_from_playlist(self, track: Track) -> None:
        try:
            pv = self.query_one(PlaylistView)
            playlist_id = getattr(pv, "_current_playlist_id", "")
            if not playlist_id:
                return
            self.music_api.remove_playlist_items(playlist_id, [track.video_id])
            self.call_from_thread(self._reload_playlist_view, playlist_id)
        except Exception:
            pass

    def _reload_playlist_view(self, playlist_id: str) -> None:
        from ytmusic_tui.api import PlaylistInfo

        pv = self.query_one(PlaylistView)
        title = getattr(pv, "_current_playlist_title", "")
        pv._show_track_list(PlaylistInfo(playlist_id=playlist_id, title=title))

    def on_unmount(self) -> None:
        with contextlib.suppress(Exception):
            if self._mpris is not None:
                self._mpris.shutdown()
        with contextlib.suppress(Exception):
            self.player.shutdown()


def main() -> None:
    """Entry point for ytmusic-tui."""
    app = YtMusicTui()
    app.run()
