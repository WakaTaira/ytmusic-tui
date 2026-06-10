"""Recently played view (YouTube Music listening history)."""

from __future__ import annotations

import contextlib
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

    from ytmusic_tui.queue import Track


class HistoryView(Static):
    """Recently played tracks, newest first.

    The list is refetched every time the view is opened (history moves
    fast). Enter on a row queues the visible history from the selected
    position, album-style.
    """

    DEFAULT_CSS = """
    HistoryView {
        width: 1fr;
        height: 1fr;
    }
    HistoryView #history-title {
        text-style: bold;
        color: $accent;
        padding: 1 1 0 1;
    }
    HistoryView #history-status {
        height: 1;
        padding: 0 1;
        text-style: italic;
        color: $text-muted;
    }
    HistoryView #history-table-container {
        height: 1fr;
        padding: 0 1;
    }
    """

    def __init__(self, **kwargs: Any) -> None:
        super().__init__(**kwargs)
        self._tracks: list[Track] = []

    def compose(self) -> ComposeResult:
        yield Label("Recently played", id="history-title")
        yield Label("", id="history-status")
        with Vertical(id="history-table-container"):
            table: DataTable[Any] = DataTable(id="history-table")
            table.cursor_type = "row"
            table.add_columns("Title", "Artist", "Album", "Duration")
            yield table
        yield FilterBar("history-table", id="history-filter")

    def on_show(self) -> None:
        with contextlib.suppress(Exception):
            self.query_one("#history-table", DataTable).focus()

    def refresh_history(self) -> None:
        """Refetch the listening history."""
        self._set_status("Loading history...")
        self._fetch_history()

    @work(thread=True)
    def _fetch_history(self) -> None:
        api = getattr(self.app, "music_api", None)
        if api is None:
            self.app.call_from_thread(self._set_status, "Error: API not initialized")
            return

        try:
            tracks = api.get_history()
            self.app.call_from_thread(self._populate, tracks)
        except Exception as exc:
            self.app.call_from_thread(self._set_status, classify_api_error(exc))

    @teardown_safe
    def _populate(self, tracks: list[Track]) -> None:
        self._tracks = list(tracks)
        table = self.query_one("#history-table", DataTable)
        table.clear()

        if not self._tracks:
            self._set_status("No history")
            return

        self._set_status(f"{len(self._tracks)} track(s) [Esc to go back]")
        for track in self._tracks:
            table.add_row(
                track.title,
                track.artist,
                track.album,
                _format_duration(track.duration_seconds),
            )

    def on_data_table_row_selected(self, event: DataTable.RowSelected) -> None:
        """Queue the history from the selected position and play."""
        row_index = event.cursor_row
        if row_index < 0 or row_index >= len(self._tracks):
            return

        track = self._tracks[row_index]
        queue = getattr(self.app, "queue_manager", None)
        player = getattr(self.app, "player", None)

        if queue is not None:
            queue.set_playlist(self._tracks, start_index=row_index)
        if player is not None:
            player.play(track.video_id)

    def get_focused_item(self) -> Track | None:
        """Return the track at the cursor row, or ``None``."""
        try:
            table = self.query_one("#history-table", DataTable)
            row_index = table.cursor_row
        except Exception:
            return None

        if 0 <= row_index < len(self._tracks):
            return self._tracks[row_index]
        return None

    def toggle_filter(self) -> None:
        """Toggle the filter bar for the history table."""
        filter_bar = self.query_one("#history-filter", FilterBar)
        if filter_bar.is_visible:
            filter_bar.hide()
        else:
            filter_bar.show()

    @teardown_safe
    def _set_status(self, text: str) -> None:
        """Update the status label."""
        self.query_one("#history-status", Label).update(text)
