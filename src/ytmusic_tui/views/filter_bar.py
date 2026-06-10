"""Inline search filter bar for DataTable filtering.

Provides a reusable FilterBar widget that attaches to any DataTable and
filters its rows incrementally via case-insensitive substring matching.
Inspired by spotify_player's popup filter bar.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from textual.containers import Horizontal
from textual.reactive import reactive
from textual.widgets import Input, Label, Static

if TYPE_CHECKING:
    from textual.app import ComposeResult
    from textual.widgets import DataTable


# Row data stored as a tuple of string cell values
type RowData = tuple[str, ...]


class FilterBar(Static):
    """Inline search filter bar that attaches to a DataTable.

    When shown, captures keystrokes in an Input widget and filters
    the target DataTable's rows by case-insensitive substring match
    across all columns.

    The filter is non-destructive: original rows are stored internally
    and restored when the filter bar is hidden.
    """

    DEFAULT_CSS = """
    FilterBar {
        dock: bottom;
        height: 3;
        background: $surface;
        border-top: solid $primary-background;
        display: none;
    }
    FilterBar.visible {
        display: block;
    }
    FilterBar #filter-container {
        height: 3;
        padding: 0 1;
    }
    FilterBar #filter-label {
        width: auto;
        height: 1;
        margin: 1 1 0 0;
        text-style: bold;
        color: $accent;
    }
    FilterBar #filter-input {
        width: 1fr;
        margin: 0;
    }
    FilterBar #filter-count {
        width: auto;
        height: 1;
        margin: 1 0 0 1;
        color: $text-muted;
    }
    """

    filter_active: reactive[bool] = reactive(False)

    def __init__(
        self,
        target_table_id: str,
        *,
        name: str | None = None,
        id: str | None = None,
        classes: str | None = None,
    ) -> None:
        """Initialize the filter bar.

        Args:
            target_table_id: The DOM id of the DataTable to filter.
            name: Widget name.
            id: Widget id.
            classes: CSS classes.
        """
        super().__init__(name=name, id=id, classes=classes)
        self._target_table_id = target_table_id
        self._original_rows: list[RowData] = []
        self._has_stored_rows = False

    @property
    def target_table_id(self) -> str:
        """The DOM id of the currently targeted DataTable."""
        return self._target_table_id

    def retarget(self, table_id: str) -> None:
        """Change the target table.

        If the filter is currently active, hides it first to restore
        the old table's rows, then sets the new target.
        """
        if self.filter_active:
            self.hide()
        self._target_table_id = table_id

    def compose(self) -> ComposeResult:
        """Build the filter bar layout: label, input, count."""
        with Horizontal(id="filter-container"):
            yield Label("/", id="filter-label")
            yield Input(
                placeholder="Type to filter...",
                id="filter-input",
            )
            yield Label("", id="filter-count")

    @property
    def target_table(self) -> DataTable[Any] | None:
        """Resolve the target DataTable from the DOM."""
        from textual.widgets import DataTable

        try:
            return self.screen.query_one(f"#{self._target_table_id}", DataTable)
        except Exception:
            return None

    # -----------------------------------------------------------------
    # Show / hide
    # -----------------------------------------------------------------

    def show(self) -> None:
        """Show the filter bar, store original rows, and focus input."""
        table = self.target_table
        if table is None:
            self.app.notify("Nothing to filter here", severity="warning", timeout=4)
            return

        self._store_original_rows(table)
        self.add_class("visible")
        self.filter_active = True

        input_widget = self.query_one("#filter-input", Input)
        input_widget.value = ""
        input_widget.focus()
        self._update_count(len(self._original_rows), len(self._original_rows))

    def hide(self) -> None:
        """Hide the filter bar and restore all original rows."""
        self.remove_class("visible")
        self.filter_active = False

        table = self.target_table
        if table is not None and self._has_stored_rows:
            self._restore_rows(table)

        self._has_stored_rows = False
        self._original_rows = []

    @property
    def is_visible(self) -> bool:
        """Whether the filter bar is currently displayed."""
        return self.filter_active

    # -----------------------------------------------------------------
    # Row storage
    # -----------------------------------------------------------------

    def _store_original_rows(self, table: DataTable[Any]) -> None:
        """Snapshot all current rows from the table."""
        rows: list[RowData] = []
        for row_key in table.rows:
            row_obj = table.get_row(row_key)
            rows.append(tuple(str(cell) for cell in row_obj))
        self._original_rows = rows
        self._has_stored_rows = True

    def _restore_rows(self, table: DataTable[Any]) -> None:
        """Replace table content with the stored original rows."""
        table.clear()
        for row in self._original_rows:
            table.add_row(*row)

    # -----------------------------------------------------------------
    # Filtering
    # -----------------------------------------------------------------

    def _apply_filter(self, query: str) -> None:
        """Filter the target table rows by the given query string."""
        table = self.target_table
        if table is None or not self._has_stored_rows:
            return

        if not query.strip():
            # Empty query: show all rows
            self._restore_rows(table)
            self._update_count(len(self._original_rows), len(self._original_rows))
            return

        query_lower = query.lower()
        matched: list[RowData] = []
        for row in self._original_rows:
            # Check if query appears in any column
            if any(query_lower in cell.lower() for cell in row):
                matched.append(row)

        table.clear()
        for row in matched:
            table.add_row(*row)

        self._update_count(len(matched), len(self._original_rows))

    def _update_count(self, visible: int, total: int) -> None:
        """Update the match count label."""
        label = self.query_one("#filter-count", Label)
        label.update(f"{visible}/{total}")

    # -----------------------------------------------------------------
    # Event handlers
    # -----------------------------------------------------------------

    def on_input_changed(self, event: Input.Changed) -> None:
        """Handle incremental input changes to update the filter."""
        if self.filter_active:
            self._apply_filter(event.value)

    def on_key(self, event: object) -> None:
        """Handle Escape to close the filter bar."""
        key = getattr(event, "key", "")
        if key == "escape" and self.filter_active:
            self.hide()
            # Prevent the Escape from bubbling up
            stop = getattr(event, "stop", None)
            if stop is not None:
                stop()
            prevent_default = getattr(event, "prevent_default", None)
            if prevent_default is not None:
                prevent_default()
