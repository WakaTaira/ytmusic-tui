"""Home screen view (recommendations).

Displays recommendation sections from YouTube Music home.
Each section shows items in a DataTable for keyboard navigation.
Enter on a track queues and plays it; Enter on a playlist
navigates to the playlist view.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any, ClassVar

from textual.containers import VerticalScroll
from textual.css.query import NoMatches
from textual.widgets import DataTable, Label, Static

from ytmusic_tui.api import PlaylistInfo
from ytmusic_tui.formatting import format_duration as _format_duration
from ytmusic_tui.queue import Track
from ytmusic_tui.views.base import FetchView
from ytmusic_tui.views.filter_bar import FilterBar
from ytmusic_tui.views.guards import teardown_safe
from ytmusic_tui.views.playlist import PlaylistView

if TYPE_CHECKING:
    from textual.app import ComposeResult

    from ytmusic_tui.api import HomeSection


class _SectionTable(Static):
    """A single recommendation section with a title and DataTable.

    Stores references to its items so row selection events can
    look up the underlying Track or PlaylistInfo.
    """

    DEFAULT_CSS = """
    _SectionTable {
        width: 1fr;
        height: auto;
        margin: 0 0 1 0;
    }
    _SectionTable .section-title {
        text-style: bold;
        color: $accent;
        padding: 1 0 0 1;
    }
    _SectionTable DataTable {
        height: auto;
        max-height: 12;
        margin: 0 1;
    }
    """

    def __init__(
        self,
        section_title: str,
        items: list[Track | PlaylistInfo],
        section_index: int,
        **kwargs: Any,
    ) -> None:
        super().__init__(**kwargs)
        self._section_title = section_title
        self._items: list[Track | PlaylistInfo] = items
        self._section_index = section_index

    def compose(self) -> ComposeResult:
        """Render a section title and its items table."""
        yield Label(self._section_title, classes="section-title")
        table: DataTable[Any] = DataTable(id=f"home-section-{self._section_index}")
        table.cursor_type = "row"
        table.add_columns("Title", "Artist / Info", "Duration")
        yield table

    def on_mount(self) -> None:
        """Populate the table once mounted."""
        table = self.query_one(DataTable)
        for item in self._items:
            row = _format_row(item)
            table.add_row(*row)
        if not self._items:
            table.can_focus = False

    @property
    def items(self) -> list[Track | PlaylistInfo]:
        """Access the underlying items list."""
        return self._items


class HomeView(FetchView):
    """Home screen displaying recommendation sections.

    On mount, launches a background worker to fetch home data
    from the YouTube Music API, then renders section titles
    and interactive item tables.
    """

    STATUS_LABEL_ID: ClassVar[str] = "#home-status"

    DEFAULT_CSS = """
    HomeView {
        width: 1fr;
        height: 1fr;
    }
    HomeView #home-status {
        padding: 1;
        text-style: italic;
        color: $text-muted;
    }
    HomeView #home-content {
        width: 1fr;
        height: 1fr;
    }
    """

    def compose(self) -> ComposeResult:
        """Render initial loading state."""
        yield Label("Loading...", id="home-status")
        yield VerticalScroll(id="home-content")
        yield FilterBar("home-section-0", id="home-filter")

    def on_mount(self) -> None:
        """Kick off the background data fetch."""
        self._run_fetch(self.music_app.music_api.get_home, self._render_sections)

    def on_show(self) -> None:
        """Auto-focus the first table when the view becomes visible."""
        tables = self.query("DataTable")
        focusable = [t for t in tables if t.can_focus]
        if focusable:
            focusable[0].focus()

    def on_key(self, event: object) -> None:
        """Handle Tab/Shift-Tab to cycle between section tables."""
        key = getattr(event, "key", "")
        if key == "tab":
            self._focus_adjacent_section(forward=True)
        elif key == "shift+tab":
            self._focus_adjacent_section(forward=False)

    def _focus_adjacent_section(self, *, forward: bool) -> None:
        """Move focus to the next/previous section's DataTable."""
        tables = self.query("DataTable")
        focusable = [t for t in tables if t.can_focus]
        if not focusable:
            return

        focused = self.app.focused
        current_idx = -1
        for i, table in enumerate(focusable):
            if table is focused:
                current_idx = i
                break

        if forward:
            next_idx = (current_idx + 1) % len(focusable)
        else:
            next_idx = (current_idx - 1) % len(focusable)

        target = focusable[next_idx]
        target.focus()
        target.scroll_visible(animate=False)

    @teardown_safe
    def _render_sections(self, sections: list[HomeSection]) -> None:
        """Populate the scroll container with fetched sections."""
        status = self.query_one("#home-status", Label)
        content = self.query_one("#home-content", VerticalScroll)

        if not sections:
            status.update("No recommendations available")
            return

        status.update("")

        for idx, section in enumerate(sections):
            if not section.items:
                continue
            widget = _SectionTable(
                section_title=section.title,
                items=section.items,
                section_index=idx,
            )
            content.mount(widget)

        # Focus the first table after rendering
        tables = self.query("DataTable")
        focusable = [t for t in tables if t.can_focus]
        if focusable:
            focusable[0].focus()

    def on_data_table_row_selected(self, event: DataTable.RowSelected) -> None:
        """Handle Enter on a row in any section table.

        Tracks: queue the single track and start playing.
        Playlists: switch to playlist view and load the playlist.
        """
        # Walk up from the DataTable to find the _SectionTable parent
        section_widget = event.data_table.parent
        if not isinstance(section_widget, _SectionTable):
            return

        row_index = event.cursor_row
        items = section_widget.items
        if row_index < 0 or row_index >= len(items):
            return

        item = items[row_index]
        self._handle_item_selection(item)

    def _handle_item_selection(self, item: Track | PlaylistInfo) -> None:
        """Dispatch selection based on item type."""
        if isinstance(item, Track):
            self._play_track(item)
        elif isinstance(item, PlaylistInfo):
            self._open_playlist(item)

    def _play_track(self, track: Track) -> None:
        """Queue a single track and start playing."""
        self.music_app.queue_manager.set_playlist([track], start_index=0)
        self.music_app.player.play(track.video_id)

    def _open_playlist(self, playlist: PlaylistInfo) -> None:
        """Switch to playlist view and load the selected playlist."""
        app = self.music_app
        app.action_switch_view("playlist")
        app.query_one(PlaylistView).show_track_list(playlist)

    def toggle_filter(self) -> None:
        """Toggle the filter bar for the focused section table."""
        filter_bar = self.query_one("#home-filter", FilterBar)
        if filter_bar.is_visible:
            filter_bar.hide()
        else:
            # Find the focused DataTable
            focused = self.app.focused
            target_id = "home-section-0"
            if focused is not None and isinstance(focused, DataTable):
                fid = getattr(focused, "id", "")
                if fid and fid.startswith("home-section-"):
                    target_id = fid
            filter_bar.retarget(target_id)
            filter_bar.show()

    def get_focused_item(self) -> Track | PlaylistInfo | None:
        """Return the item under the cursor in the focused DataTable.

        Walks the focused widget up to find its _SectionTable parent,
        then looks up the item by cursor row.
        """
        focused = self.app.focused
        if focused is None:
            return None

        # The focused widget is typically a DataTable inside a _SectionTable
        section_widget = focused.parent
        if not isinstance(section_widget, _SectionTable):
            # Maybe the DataTable itself is focused directly
            section_widget = focused
            if not isinstance(section_widget, _SectionTable):
                return None

        try:
            table = section_widget.query_one(DataTable)
        except NoMatches:
            return None
        row_index = table.cursor_row

        items = section_widget.items
        if 0 <= row_index < len(items):
            return items[row_index]
        return None


def _format_row(item: Track | PlaylistInfo) -> tuple[str, str, str]:
    """Format a home section item as a table row (title, info, duration)."""
    if isinstance(item, Track):
        return (
            item.title,
            item.artist or "",
            _format_duration(item.duration_seconds),
        )
    if isinstance(item, PlaylistInfo):
        count_str = f"{item.track_count} tracks" if item.track_count else "Playlist"
        return (item.title, count_str, "")
    return (str(item), "", "")
