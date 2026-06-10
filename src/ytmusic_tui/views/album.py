"""Album detail view with track listing."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any, ClassVar

from textual.containers import Vertical
from textual.widgets import DataTable, Label

from ytmusic_tui.formatting import format_duration as _format_duration
from ytmusic_tui.views.base import FetchView
from ytmusic_tui.views.filter_bar import FilterBar
from ytmusic_tui.views.guards import teardown_safe

if TYPE_CHECKING:
    from textual.app import ComposeResult

    from ytmusic_tui.api import AlbumInfo
    from ytmusic_tui.queue import Track


class AlbumView(FetchView):
    """Album detail view showing tracks in a DataTable.

    Displays album title, artist, and year at the top.
    Enter on a track queues all album tracks starting from
    the selected position (spotify_player style).
    Escape goes back to the previous view.
    """

    STATUS_LABEL_ID: ClassVar[str] = "#album-status"

    DEFAULT_CSS = """
    AlbumView {
        width: 1fr;
        height: 1fr;
    }
    AlbumView #album-header {
        height: auto;
        padding: 1;
    }
    AlbumView #album-title {
        text-style: bold;
        color: $accent;
    }
    AlbumView #album-meta {
        color: $text-muted;
        text-style: italic;
    }
    AlbumView #album-status {
        height: 1;
        padding: 0 1;
        text-style: italic;
        color: $text-muted;
    }
    AlbumView #album-table-container {
        height: 1fr;
        padding: 0 1;
    }
    """

    def __init__(self, **kwargs: Any) -> None:
        super().__init__(**kwargs)
        self._album: AlbumInfo | None = None
        self._tracks: list[Track] = []

    def compose(self) -> ComposeResult:
        """Build the album layout: header, status, track table, and filter bar."""
        with Vertical(id="album-header"):
            yield Label("", id="album-title")
            yield Label("", id="album-meta")
        yield Label("", id="album-status")
        with Vertical(id="album-table-container"):
            table: DataTable[Any] = DataTable(id="album-table")
            table.cursor_type = "row"
            table.add_columns("#", "Title", "Artist", "Duration")
            yield table
        yield FilterBar("album-table", id="album-filter")

    def load_album(self, browse_id: str) -> None:
        """Kick off a background fetch for the given album."""
        self._clear()
        self._run_fetch(
            lambda: self.music_app.music_api.get_album(browse_id),
            self._populate,
            loading="Loading album...",
        )

    def on_show(self) -> None:
        """Auto-focus the table when the view becomes visible."""
        self.query_one("#album-table", DataTable).focus()

    def show_album(self, album: AlbumInfo) -> None:
        """Display an already-fetched AlbumInfo (no API call needed)."""
        self._clear()
        self._populate(album)

    def _clear(self) -> None:
        """Reset view state."""
        self._album = None
        self._tracks = []
        table = self.query_one("#album-table", DataTable)
        table.clear()
        self.query_one("#album-title", Label).update("")
        self.query_one("#album-meta", Label).update("")
        self._set_status("")

    @teardown_safe
    def _populate(self, album: AlbumInfo) -> None:
        """Fill the view with album data."""
        self._album = album
        self._tracks = list(album.tracks)

        # Update header
        self.query_one("#album-title", Label).update(album.title)
        meta_parts: list[str] = []
        if album.artist:
            meta_parts.append(album.artist)
        if album.year:
            meta_parts.append(album.year)
        meta_text = " - ".join(meta_parts) if meta_parts else ""
        self.query_one("#album-meta", Label).update(meta_text)

        # Fill track table
        table = self.query_one("#album-table", DataTable)
        table.clear()

        if not self._tracks:
            self._set_status("No tracks")
            return

        self._set_status(f"{len(self._tracks)} track(s) [Esc to go back]")

        for idx, track in enumerate(self._tracks, start=1):
            table.add_row(
                str(idx),
                track.title,
                track.artist,
                _format_duration(track.duration_seconds),
            )

    def on_data_table_row_selected(self, event: DataTable.RowSelected) -> None:
        """Handle Enter on a track: queue album from selected position."""
        row_index = event.cursor_row
        if row_index < 0 or row_index >= len(self._tracks):
            return

        track = self._tracks[row_index]
        self.music_app.queue_manager.set_playlist(self._tracks, start_index=row_index)
        self.music_app.player.play(track.video_id)

    def get_focused_item(self) -> Track | None:
        """Return the track at the cursor row, or ``None``."""
        row_index = self._cursor_row("#album-table")
        if row_index is None:
            return None
        if 0 <= row_index < len(self._tracks):
            return self._tracks[row_index]
        return None

    def toggle_filter(self) -> None:
        """Toggle the filter bar for the album track table."""
        filter_bar = self.query_one("#album-filter", FilterBar)
        if filter_bar.is_visible:
            filter_bar.hide()
        else:
            filter_bar.show()
