"""Playlist view with two-level navigation (playlist list / track list)."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from textual import work
from textual.containers import Vertical
from textual.widgets import DataTable, Label, Static

from ytmusic_tui.auth import classify_api_error
from ytmusic_tui.formatting import format_duration as _format_duration
from ytmusic_tui.views.filter_bar import FilterBar
from ytmusic_tui.views.guards import teardown_safe

if TYPE_CHECKING:
    from textual.app import ComposeResult

    from ytmusic_tui.api import PlaylistInfo
    from ytmusic_tui.queue import Track


class PlaylistView(Static):
    """Two-level playlist browser.

    Level 1: list of library playlists (title, track count).
    Level 2: tracks within a selected playlist.

    Enter drills into playlists / selects a track to play.
    Escape returns from track list to playlist list.
    """

    DEFAULT_CSS = """
    PlaylistView {
        width: 1fr;
        height: 1fr;
    }
    PlaylistView #playlist-status {
        height: 1;
        padding: 0 1;
        text-style: italic;
        color: $text-muted;
    }
    PlaylistView #playlist-table-container {
        height: 1fr;
        padding: 0 1;
    }
    """

    def __init__(self, **kwargs: Any) -> None:
        super().__init__(**kwargs)
        self._playlists: list[PlaylistInfo] = []
        self._tracks: list[Track] = []
        self._viewing_tracks: bool = False
        self._current_playlist_title: str = ""
        self._current_playlist_id: str = ""

    def compose(self) -> ComposeResult:
        """Build the playlist layout: status label, data table, and filter bar."""
        yield Label("", id="playlist-status")
        with Vertical(id="playlist-table-container"):
            table: DataTable[Any] = DataTable(id="playlist-table")
            table.cursor_type = "row"
            yield table
        yield FilterBar("playlist-table", id="playlist-filter")

    def on_mount(self) -> None:
        """Fetch library playlists on mount."""
        self._show_playlist_list()

    def on_show(self) -> None:
        """Auto-focus the table when the view becomes visible."""
        self.query_one("#playlist-table", DataTable).focus()

    def _show_playlist_list(self) -> None:
        """Switch to the playlist list view and fetch data."""
        self._viewing_tracks = False
        self._tracks = []
        table = self.query_one("#playlist-table", DataTable)
        table.clear(columns=True)
        table.add_columns("Title", "Tracks")
        self._fetch_playlists()

    @work(thread=True)
    def _fetch_playlists(self) -> None:
        """Fetch library playlists in a background thread."""
        self.app.call_from_thread(self._set_status, "Loading playlists...")

        api = getattr(self.app, "music_api", None)
        if api is None:
            self.app.call_from_thread(self._set_status, "Error: API not initialized")
            return

        try:
            playlists: list[PlaylistInfo] = api.get_library_playlists()
            self.app.call_from_thread(self._populate_playlists, playlists)
        except Exception as exc:
            self.app.call_from_thread(self._set_status, classify_api_error(exc))

    @teardown_safe
    def _populate_playlists(self, playlists: list[PlaylistInfo]) -> None:
        """Fill the table with playlist data."""
        self._playlists = playlists
        table = self.query_one("#playlist-table", DataTable)
        table.clear()

        if not playlists:
            self._set_status("No playlists found")
            return

        self._set_status(f"{len(playlists)} playlist(s)")

        for pl in playlists:
            table.add_row(pl.title, str(pl.track_count))

    def _show_track_list(self, playlist: PlaylistInfo) -> None:
        """Switch to track list view for the given playlist."""
        self._viewing_tracks = True
        self._current_playlist_title = playlist.title
        self._current_playlist_id = playlist.playlist_id
        table = self.query_one("#playlist-table", DataTable)
        table.clear(columns=True)
        table.add_columns("Title", "Artist", "Album", "Duration")
        self._fetch_tracks(playlist.playlist_id)

    @work(thread=True)
    def _fetch_tracks(self, playlist_id: str) -> None:
        """Fetch playlist tracks in a background thread."""
        self.app.call_from_thread(
            self._set_status, f"Loading tracks for {self._current_playlist_title}..."
        )

        api = getattr(self.app, "music_api", None)
        if api is None:
            self.app.call_from_thread(self._set_status, "Error: API not initialized")
            return

        try:
            tracks: list[Track] = api.get_playlist_tracks(playlist_id)
            self.app.call_from_thread(self._populate_tracks, tracks)
        except Exception as exc:
            self.app.call_from_thread(self._set_status, classify_api_error(exc))

    @teardown_safe
    def _populate_tracks(self, tracks: list[Track]) -> None:
        """Fill the table with track data."""
        self._tracks = tracks
        table = self.query_one("#playlist-table", DataTable)
        table.clear()

        if not tracks:
            self._set_status(f"{self._current_playlist_title} - empty playlist")
            return

        self._set_status(
            f"{self._current_playlist_title} - {len(tracks)} track(s) [Esc to go back]"
        )

        for track in tracks:
            table.add_row(
                track.title,
                track.artist,
                track.album,
                _format_duration(track.duration_seconds),
            )

    def on_data_table_row_selected(self, event: DataTable.RowSelected) -> None:
        """Handle Enter on a row.

        In playlist list mode: drill into the selected playlist.
        In track list mode: queue remaining tracks from selected position
        and start playing (spotify_player style).
        """
        row_index = event.cursor_row

        if not self._viewing_tracks:
            # Playlist list mode
            if row_index < 0 or row_index >= len(self._playlists):
                return
            playlist = self._playlists[row_index]
            self._show_track_list(playlist)
        else:
            # Track list mode
            if row_index < 0 or row_index >= len(self._tracks):
                return
            track = self._tracks[row_index]
            queue = getattr(self.app, "queue_manager", None)
            player = getattr(self.app, "player", None)

            if queue is not None:
                queue.set_playlist(self._tracks, start_index=row_index)
            if player is not None:
                player.play(track.video_id)

    def on_key(self, event: object) -> None:
        """Handle Escape to go back to playlist list from track list."""
        key_event = event
        key = getattr(key_event, "key", None)

        # Let the filter bar handle its own Escape
        filter_bar = self.query_one("#playlist-filter", FilterBar)
        if key == "escape" and filter_bar.is_visible:
            return

        if key == "escape" and self._viewing_tracks:
            self._show_playlist_list()

    def toggle_filter(self) -> None:
        """Toggle the filter bar for the playlist table."""
        filter_bar = self.query_one("#playlist-filter", FilterBar)
        if filter_bar.is_visible:
            filter_bar.hide()
        else:
            filter_bar.show()

    def get_focused_item(self) -> Track | PlaylistInfo | None:
        """Return the item at the cursor row.

        In playlist list mode: returns a PlaylistInfo.
        In track list mode: returns a Track.
        Returns ``None`` if the list is empty.
        """
        try:
            table = self.query_one("#playlist-table", DataTable)
            row_index = table.cursor_row
        except Exception:
            return None

        if self._viewing_tracks:
            if 0 <= row_index < len(self._tracks):
                return self._tracks[row_index]
        else:
            if 0 <= row_index < len(self._playlists):
                return self._playlists[row_index]
        return None

    @teardown_safe
    def _set_status(self, text: str) -> None:
        """Update the status label."""
        self.query_one("#playlist-status", Label).update(text)
