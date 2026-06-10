"""Queue display view with track listing and current-track highlight."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any, ClassVar

from textual.widgets import DataTable, Label

from ytmusic_tui.formatting import format_duration as _format_duration
from ytmusic_tui.views.base import FetchView
from ytmusic_tui.views.filter_bar import FilterBar

if TYPE_CHECKING:
    from textual.app import ComposeResult

    from ytmusic_tui.queue import Track


class QueueView(FetchView):
    """Displays the current playback queue.

    Shows all tracks with the currently playing track highlighted.
    Press 'd' on a row to remove it from the queue.

    Reads directly from ``queue_manager`` rather than the API, so it has
    no fetch worker. It still subclasses :class:`FetchView` for typed app
    access (``music_app.queue_manager``), the shared teardown-safe
    ``_set_status``, and the ``_cursor_row`` helper.
    """

    STATUS_LABEL_ID: ClassVar[str] = "#queue-status"

    DEFAULT_CSS = """
    QueueView {
        width: 1fr;
        height: 1fr;
    }
    QueueView #queue-status {
        height: 1;
        padding: 0 1;
        text-style: italic;
        color: $text-muted;
    }
    QueueView #queue-table {
        height: 1fr;
        margin: 0 1;
    }
    """

    def compose(self) -> ComposeResult:
        """Build the queue layout: status label, data table, and filter bar."""
        yield Label("", id="queue-status")
        table: DataTable[Any] = DataTable(id="queue-table")
        table.cursor_type = "row"
        table.add_columns("#", "Title", "Artist", "Album", "Duration")
        yield table
        yield FilterBar("queue-table", id="queue-filter")

    def on_mount(self) -> None:
        """Refresh queue display on mount."""
        self.refresh_queue()

    def on_show(self) -> None:
        """Auto-focus the table and refresh when the view becomes visible."""
        self.query_one("#queue-table", DataTable).focus()
        self.refresh_queue()

    def refresh_queue(self) -> None:
        """Rebuild the table from the current queue state."""
        queue = self.music_app.queue_manager
        table = self.query_one("#queue-table", DataTable)
        table.clear()

        tracks: list[Track] = queue.tracks
        current: Track | None = queue.current_track

        if not tracks:
            self._set_status("Queue is empty")
            return

        self._set_status(f"{len(tracks)} track(s) in queue")

        for i, track in enumerate(tracks):
            marker = ">" if track == current else " "
            table.add_row(
                f"{marker}{i + 1}",
                track.title,
                track.artist,
                track.album,
                _format_duration(track.duration_seconds),
            )

    def on_key(self, event: object) -> None:
        """Handle 'd' key to remove the selected track from the queue."""
        key_event = event
        if getattr(key_event, "key", None) != "d":
            return

        table = self.query_one("#queue-table", DataTable)
        row_index = table.cursor_row
        queue = self.music_app.queue_manager

        tracks = queue.tracks
        if row_index < 0 or row_index >= len(tracks):
            return

        queue.remove(row_index)
        self.refresh_queue()

    def get_focused_item(self) -> Track | None:
        """Return the track at the cursor row, or ``None``."""
        row_index = self._cursor_row("#queue-table")
        if row_index is None:
            return None

        tracks = self.music_app.queue_manager.tracks
        if 0 <= row_index < len(tracks):
            return tracks[row_index]
        return None

    def toggle_filter(self) -> None:
        """Toggle the filter bar for the queue table."""
        filter_bar = self.query_one("#queue-filter", FilterBar)
        if filter_bar.is_visible:
            filter_bar.hide()
        else:
            filter_bar.show()
