"""Artist page view with top songs, albums, and related artists."""

from __future__ import annotations

import contextlib
from typing import TYPE_CHECKING, Any

from textual import work
from textual.containers import Vertical, VerticalScroll
from textual.widgets import DataTable, Label, Static

from ytmusic_tui.auth import classify_api_error
from ytmusic_tui.formatting import format_duration as _format_duration
from ytmusic_tui.views.filter_bar import FilterBar
from ytmusic_tui.views.guards import teardown_safe

if TYPE_CHECKING:
    from textual.app import ComposeResult

    from ytmusic_tui.api import AlbumInfo, ArtistInfo, RelatedArtist
    from ytmusic_tui.queue import Track


class ArtistView(Static):
    """Artist page showing top songs, albums, and related artists.

    Three DataTable sections inside a VerticalScroll container.
    Enter on a top song queues and plays it.
    Enter on an album opens the AlbumView.
    Enter on a related artist opens that artist's page.
    Escape goes back to the previous view.
    """

    DEFAULT_CSS = """
    ArtistView {
        width: 1fr;
        height: 1fr;
    }
    ArtistView #artist-name {
        text-style: bold;
        color: $accent;
        padding: 1;
    }
    ArtistView #artist-status {
        height: 1;
        padding: 0 1;
        text-style: italic;
        color: $text-muted;
    }
    ArtistView #artist-content {
        width: 1fr;
        height: 1fr;
    }
    ArtistView .section-label {
        text-style: bold;
        padding: 1 0 0 1;
    }
    ArtistView DataTable {
        height: auto;
        max-height: 15;
        margin: 0 1 1 1;
    }
    """

    def __init__(self, **kwargs: Any) -> None:
        super().__init__(**kwargs)
        self._artist: ArtistInfo | None = None
        self._top_songs: list[Track] = []
        self._albums: list[AlbumInfo] = []
        self._related_artists: list[RelatedArtist] = []

    def compose(self) -> ComposeResult:
        """Build the artist layout."""
        yield Label("", id="artist-name")
        yield Label("", id="artist-status")
        with VerticalScroll(id="artist-content"):
            # Top Songs section
            yield Label("Top Songs", classes="section-label", id="label-top-songs")
            with Vertical(id="top-songs-container"):
                table: DataTable[Any] = DataTable(id="artist-top-songs")
                table.cursor_type = "row"
                table.add_columns("Title", "Album", "Duration")
                yield table
            # Albums section
            yield Label("Albums", classes="section-label", id="label-albums")
            with Vertical(id="albums-container"):
                table = DataTable(id="artist-albums")
                table.cursor_type = "row"
                table.add_columns("Title", "Year")
                yield table
            # Related Artists section
            yield Label("Related Artists", classes="section-label", id="label-related")
            with Vertical(id="related-container"):
                table = DataTable(id="artist-related")
                table.cursor_type = "row"
                table.add_columns(
                    "Name",
                )
                yield table
        yield FilterBar("artist-top-songs", id="artist-filter")

    def on_show(self) -> None:
        """Auto-focus the top songs table when the view becomes visible."""
        with contextlib.suppress(Exception):
            self.query_one("#artist-top-songs", DataTable).focus()

    def load_artist(self, channel_id: str) -> None:
        """Kick off a background fetch for the given artist."""
        self._clear()
        self._set_status("Loading artist...")
        self._fetch_artist(channel_id)

    def show_artist(self, artist: ArtistInfo) -> None:
        """Display an already-fetched ArtistInfo (no API call needed)."""
        self._clear()
        self._populate(artist)

    def _clear(self) -> None:
        """Reset all view state."""
        self._artist = None
        self._top_songs = []
        self._albums = []
        self._related_artists = []
        self.query_one("#artist-name", Label).update("")
        self._set_status("")
        for table_id in ("artist-top-songs", "artist-albums", "artist-related"):
            self.query_one(f"#{table_id}", DataTable).clear()

    @work(thread=True)
    def _fetch_artist(self, channel_id: str) -> None:
        """Fetch artist data in a background thread."""
        api = getattr(self.app, "music_api", None)
        if api is None:
            self.app.call_from_thread(self._set_status, "Error: API not initialized")
            return

        try:
            artist: ArtistInfo = api.get_artist(channel_id)
            self.app.call_from_thread(self._populate, artist)
        except Exception as exc:
            self.app.call_from_thread(self._set_status, classify_api_error(exc))

    @teardown_safe
    def _populate(self, artist: ArtistInfo) -> None:
        """Fill all sections with artist data."""
        self._artist = artist
        self._top_songs = list(artist.top_songs)
        self._albums = list(artist.albums)
        self._related_artists = list(artist.related_artists)

        self.query_one("#artist-name", Label).update(artist.name)
        self._set_status("[Esc to go back]")

        # Top Songs
        songs_table = self.query_one("#artist-top-songs", DataTable)
        songs_table.clear()
        for track in self._top_songs:
            songs_table.add_row(
                track.title,
                track.album,
                _format_duration(track.duration_seconds),
            )

        # Albums
        albums_table = self.query_one("#artist-albums", DataTable)
        albums_table.clear()
        for album in self._albums:
            albums_table.add_row(album.title, album.year)

        # Related Artists
        related_table = self.query_one("#artist-related", DataTable)
        related_table.clear()
        for rel in self._related_artists:
            related_table.add_row(rel.name)

    def on_data_table_row_selected(self, event: DataTable.RowSelected) -> None:
        """Handle Enter on a row in any of the three sections."""
        table_id = event.data_table.id
        row_index = event.cursor_row

        if table_id == "artist-top-songs":
            self._handle_song_selection(row_index)
        elif table_id == "artist-albums":
            self._handle_album_selection(row_index)
        elif table_id == "artist-related":
            self._handle_related_selection(row_index)

    def _handle_song_selection(self, row_index: int) -> None:
        """Queue the selected top song and play."""
        if row_index < 0 or row_index >= len(self._top_songs):
            return

        track = self._top_songs[row_index]
        queue = getattr(self.app, "queue_manager", None)
        player = getattr(self.app, "player", None)

        if queue is not None:
            queue.set_playlist([track], start_index=0)
        if player is not None:
            player.play(track.video_id)

    def _handle_album_selection(self, row_index: int) -> None:
        """Open the selected album in AlbumView."""
        if row_index < 0 or row_index >= len(self._albums):
            return

        album = self._albums[row_index]
        app = self.app
        open_album = getattr(app, "action_open_album", None)
        if open_album is not None:
            open_album(album.browse_id)

    def _handle_related_selection(self, row_index: int) -> None:
        """Open the selected related artist's page."""
        if row_index < 0 or row_index >= len(self._related_artists):
            return

        related = self._related_artists[row_index]
        app = self.app
        open_artist = getattr(app, "action_open_artist", None)
        if open_artist is not None:
            open_artist(related.channel_id)

    def get_focused_item(self) -> Track | AlbumInfo | None:
        """Return the item at the cursor in the focused section.

        Top Songs section returns a Track. Albums section returns
        an AlbumInfo. Related Artists section returns None (no popup
        actions defined for RelatedArtist).
        """
        focused = self.app.focused
        if not isinstance(focused, DataTable):
            return None

        table_id = focused.id or ""
        row_index = focused.cursor_row

        if table_id == "artist-top-songs" and 0 <= row_index < len(self._top_songs):
            return self._top_songs[row_index]
        if table_id == "artist-albums" and 0 <= row_index < len(self._albums):
            return self._albums[row_index]

        return None

    def toggle_filter(self) -> None:
        """Toggle the filter bar for the focused section's table."""
        filter_bar = self.query_one("#artist-filter", FilterBar)
        if filter_bar.is_visible:
            filter_bar.hide()
        else:
            # Determine which table is focused, default to top-songs
            focused = self.app.focused
            target_id = "artist-top-songs"
            if focused is not None:
                fid = getattr(focused, "id", "")
                if fid in ("artist-top-songs", "artist-albums", "artist-related"):
                    target_id = fid
            filter_bar.retarget(target_id)
            filter_bar.show()

    @teardown_safe
    def _set_status(self, text: str) -> None:
        """Update the status label."""
        self.query_one("#artist-status", Label).update(text)
