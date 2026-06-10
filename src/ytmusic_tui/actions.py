"""Action handler mixins for the main application.

app.py keeps the application skeleton (bindings, keymap, compose,
mount); the user-facing action handlers live here, grouped by concern:

* PlaybackActions -- transport control (play/pause/next/volume/...)
* BrowseActions   -- jumping to artist/album/lyrics pages
* PopupActions    -- action/theme/playlist-picker popups and their
  follow-up operations

The mixins subclass textual's App only for the type checker; at runtime
they are plain classes mixed into YtMusicTui ahead of App.
"""

from __future__ import annotations

import contextlib
from typing import TYPE_CHECKING, Any, cast

from textual import work
from textual.widgets import ContentSwitcher, Static

from ytmusic_tui.auth import classify_api_error
from ytmusic_tui.config import THEMES, build_textual_theme
from ytmusic_tui.navigation import PageState
from ytmusic_tui.queue import Track
from ytmusic_tui.views.album import AlbumView
from ytmusic_tui.views.artist import ArtistView
from ytmusic_tui.views.home import HomeView
from ytmusic_tui.views.library import LibraryView
from ytmusic_tui.views.lyrics import LyricsView
from ytmusic_tui.views.playlist import PlaylistView
from ytmusic_tui.views.popup import ActionKind, ActionPopup, PlaylistPickerPopup, ThemePopup
from ytmusic_tui.views.queue import QueueView
from ytmusic_tui.views.search import SearchView

if TYPE_CHECKING:
    from textual.app import App

    from ytmusic_tui.api import AlbumInfo, MusicAPI, PlaylistInfo
    from ytmusic_tui.config import AppConfig
    from ytmusic_tui.navigation import NavigationManager
    from ytmusic_tui.player import Player
    from ytmusic_tui.queue import QueueManager

    _Base = App[None]
else:
    _Base = object

# Volume adjustment step size
_VOLUME_STEP = 5

# Seek step in seconds (spotify_player compatible)
_SEEK_STEP = 5.0


class PlaybackActions(_Base):
    """Transport control: play/pause, track skip, seek, shuffle, volume."""

    if TYPE_CHECKING:
        player: Player
        queue_manager: QueueManager

    def _on_track_end(self) -> None:
        next_track = self.queue_manager.next_track()
        if next_track is not None:
            self.player.play(next_track.video_id)

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

    def action_seek_forward(self) -> None:
        self._seek_relative(_SEEK_STEP)

    def action_seek_backward(self) -> None:
        self._seek_relative(-_SEEK_STEP)

    def _seek_relative(self, seconds: float) -> None:
        if not self.player.get_state().video_id:
            return
        # The stream may not be seekable yet while the ytdl-hook is still
        # resolving the URL; a failed seek is a harmless no-op.
        with contextlib.suppress(Exception):
            self.player.seek(seconds)

    def _queue_and_play(self, tracks: list[Track]) -> None:
        self.queue_manager.set_playlist(tracks, start_index=0)
        self.player.play(tracks[0].video_id)


class BrowseActions(_Base):
    """Jump to artist, album, and lyrics pages."""

    if TYPE_CHECKING:
        music_api: MusicAPI
        player: Player
        queue_manager: QueueManager

        def _navigate_to(self, page: PageState) -> None: ...

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
            self.call_from_thread(
                self.notify, f"Artist not found: {artist_name}", severity="warning"
            )
        except Exception as exc:
            self.call_from_thread(self.notify, classify_api_error(exc), severity="error")

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
            self.call_from_thread(
                self.notify, f"Album not found: {album_name}", severity="warning"
            )
        except Exception as exc:
            self.call_from_thread(self.notify, classify_api_error(exc), severity="error")


class PopupActions(_Base):
    """Action/theme/playlist-picker popups and their follow-ups."""

    if TYPE_CHECKING:
        config: AppConfig
        music_api: MusicAPI
        nav: NavigationManager
        player: Player
        queue_manager: QueueManager

        # Provided by sibling mixins / the app at runtime. The lookup
        # stubs return Any because @work wraps them to return a Worker.
        def _lookup_and_open_artist(self, artist_name: str) -> Any: ...
        def _lookup_and_open_album(self, album_name: str, artist_name: str = "") -> Any: ...
        def _queue_and_play(self, tracks: list[Track]) -> None: ...
        def _refresh_queue_view_if_active(self) -> None: ...
        def action_switch_view(self, view_id: str) -> None: ...
        def action_open_album(self, browse_id: str) -> None: ...

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

    def _get_focused_item(self) -> Track | PlaylistInfo | AlbumInfo | None:
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
        return cast("Track | PlaylistInfo | AlbumInfo | None", getter())

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
        except Exception as exc:
            self.call_from_thread(self.notify, classify_api_error(exc), severity="error")

    @work(thread=True)
    def _play_all_album(self, browse_id: str) -> None:
        try:
            album = self.music_api.get_album(browse_id)
            if album.tracks:
                self.call_from_thread(self._queue_and_play, album.tracks)
        except Exception as exc:
            self.call_from_thread(self.notify, classify_api_error(exc), severity="error")

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

    # -- Playlist picker ------------------------------------------------

    @work(thread=True)
    def _show_playlist_picker(self, track: Track) -> None:
        try:
            playlists = self.music_api.get_library_playlists(limit=50)
            items = [(p.playlist_id, p.title) for p in playlists]
            self.call_from_thread(self._open_playlist_picker, items, track)
        except Exception as exc:
            self.call_from_thread(self.notify, classify_api_error(exc), severity="error")

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
            if not new_id:
                self.call_from_thread(self.notify, "Could not create playlist", severity="error")
                return
            if self.music_api.add_playlist_items(new_id, [track.video_id]):
                self.call_from_thread(self.notify, "Created playlist and added track")
            else:
                self.call_from_thread(
                    self.notify,
                    "Playlist created, but adding the track failed",
                    severity="error",
                )
        except Exception as exc:
            self.call_from_thread(self.notify, classify_api_error(exc), severity="error")

    @work(thread=True)
    def _add_to_existing_playlist(self, playlist_id: str, track: Track) -> None:
        try:
            if self.music_api.add_playlist_items(playlist_id, [track.video_id]):
                self.call_from_thread(self.notify, "Added to playlist")
            else:
                self.call_from_thread(self.notify, "Could not add to playlist", severity="error")
        except Exception as exc:
            self.call_from_thread(self.notify, classify_api_error(exc), severity="error")

    # -- Remove actions --------------------------------------------------

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
            if not self.music_api.remove_playlist_items(playlist_id, [track.video_id]):
                self.call_from_thread(
                    self.notify, "Could not remove from playlist", severity="error"
                )
                return
            self.call_from_thread(self._reload_playlist_view, playlist_id)
        except Exception as exc:
            self.call_from_thread(self.notify, classify_api_error(exc), severity="error")

    def _reload_playlist_view(self, playlist_id: str) -> None:
        from ytmusic_tui.api import PlaylistInfo

        pv = self.query_one(PlaylistView)
        title = getattr(pv, "_current_playlist_title", "")
        pv._show_track_list(PlaylistInfo(playlist_id=playlist_id, title=title))
