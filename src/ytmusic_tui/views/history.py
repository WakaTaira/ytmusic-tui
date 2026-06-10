"""Recently played view (YouTube Music listening history)."""

from __future__ import annotations

import contextlib
from typing import TYPE_CHECKING, Any, ClassVar

from textual.containers import Vertical
from textual.css.query import NoMatches
from textual.widgets import DataTable, Label

from ytmusic_tui.formatting import format_duration as _format_duration
from ytmusic_tui.views.base import FetchView
from ytmusic_tui.views.filter_bar import FilterBar
from ytmusic_tui.views.guards import teardown_safe
from ytmusic_tui.views.widgets import NavDataTable

if TYPE_CHECKING:
    from textual.app import ComposeResult

    from ytmusic_tui.queue import Track


class HistoryView(FetchView):
    """Recently played tracks, newest first.

    The list is refetched every time the view is opened (history moves
    fast). Enter on a row queues the visible history from the selected
    position, album-style.
    """

    STATUS_LABEL_ID: ClassVar[str] = "#history-status"

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
            table: DataTable[Any] = NavDataTable(id="history-table")
            table.cursor_type = "row"
            table.add_columns("Title", "Artist", "Album", "Duration")
            yield table
        yield FilterBar("history-table", id="history-filter")

    def on_show(self) -> None:
        with contextlib.suppress(NoMatches):
            self.query_one("#history-table", DataTable).focus()

    def refresh_history(self) -> None:
        """Refetch the listening history."""
        self._run_fetch(
            self.music_app.music_api.get_history,
            self._populate,
            loading="Loading history...",
        )

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
        self.music_app.queue_manager.set_playlist(self._tracks, start_index=row_index)
        self.music_app.player.play(track.video_id)

    def get_focused_item(self) -> Track | None:
        """Return the track at the cursor row, or ``None``."""
        row_index = self._cursor_row("#history-table")
        if row_index is None:
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
