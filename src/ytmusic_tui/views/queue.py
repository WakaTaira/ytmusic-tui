"""Queue display view with track listing and current-track highlight."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from textual.widgets import DataTable, Label, Static

from ytmusic_tui.formatting import format_duration as _format_duration
from ytmusic_tui.views.filter_bar import FilterBar

if TYPE_CHECKING:
    from textual.app import ComposeResult

    from ytmusic_tui.queue import QueueManager, Track


class QueueView(Static):
    """Displays the current playback queue.

    Shows all tracks with the currently playing track highlighted.
    Press 'd' on a row to remove it from the queue.
    """

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

    def on_focus(self) -> None:
        """Refresh queue when the view gains focus."""
        self.refresh_queue()

    def refresh_queue(self) -> None:
        """Rebuild the table from the current queue state."""
        queue: QueueManager | None = getattr(self.app, "queue_manager", None)
        table = self.query_one("#queue-table", DataTable)
        table.clear()

        if queue is None:
            self._set_status("Queue not available")
            return

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
        queue: QueueManager | None = getattr(self.app, "queue_manager", None)

        if queue is None:
            return

        tracks = queue.tracks
        if row_index < 0 or row_index >= len(tracks):
            return

        queue.remove(row_index)
        self.refresh_queue()

    def get_focused_item(self) -> Track | None:
        """Return the track at the cursor row, or ``None``."""
        queue: QueueManager | None = getattr(self.app, "queue_manager", None)
        if queue is None:
            return None

        try:
            table = self.query_one("#queue-table", DataTable)
            row_index = table.cursor_row
        except Exception:
            return None

        tracks = queue.tracks
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

    def _set_status(self, text: str) -> None:
        """Update the status label."""
        self.query_one("#queue-status", Label).update(text)
