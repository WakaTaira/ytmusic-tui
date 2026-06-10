"""Search view with multi-category grid layout (spotify_player style).

Input at top, then a 2x2 grid of panes: Tracks, Albums, Artists, Playlists.
Tab/Shift-Tab cycles focus between panes. Enter triggers category-specific
actions.
"""

from __future__ import annotations

from enum import IntEnum
from typing import TYPE_CHECKING, Any, ClassVar

from textual.containers import Horizontal, Vertical
from textual.css.query import NoMatches
from textual.widgets import DataTable, Input, Label

from ytmusic_tui.formatting import format_duration as _format_duration
from ytmusic_tui.layout import Orientation
from ytmusic_tui.views.base import FetchView
from ytmusic_tui.views.filter_bar import FilterBar
from ytmusic_tui.views.guards import teardown_safe
from ytmusic_tui.views.playlist import PlaylistView

if TYPE_CHECKING:
    from textual.app import ComposeResult

    from ytmusic_tui.api import AlbumInfo, PlaylistInfo, RelatedArtist, SearchResults
    from ytmusic_tui.queue import Track


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

_CATEGORY_PREFIXES = {"#songs:", "#albums:", "#artists:", "#playlists:"}


def _parse_search_prefix(raw: str) -> tuple[str | None, str]:
    """Parse an optional ``#category:query`` prefix."""
    lower = raw.lower()
    for prefix in _CATEGORY_PREFIXES:
        if lower.startswith(prefix):
            category = prefix[1:-1]
            query = raw[len(prefix) :].strip()
            if query:
                return category, query
            return None, raw
    return None, raw


# ---------------------------------------------------------------------------
# Pane index
# ---------------------------------------------------------------------------


class Pane(IntEnum):
    """Identifies each search result pane."""

    TRACKS = 0
    ALBUMS = 1
    ARTISTS = 2
    PLAYLISTS = 3


_PANE_COUNT = len(Pane)

# Widget IDs for each pane's DataTable
_TABLE_IDS: dict[Pane, str] = {
    Pane.TRACKS: "search-tracks",
    Pane.ALBUMS: "search-albums",
    Pane.ARTISTS: "search-artists",
    Pane.PLAYLISTS: "search-playlists",
}

# Container IDs for each pane wrapper
_PANE_IDS: dict[Pane, str] = {
    Pane.TRACKS: "pane-tracks",
    Pane.ALBUMS: "pane-albums",
    Pane.ARTISTS: "pane-artists",
    Pane.PLAYLISTS: "pane-playlists",
}

# Pane titles
_PANE_TITLES: dict[Pane, str] = {
    Pane.TRACKS: "Tracks",
    Pane.ALBUMS: "Albums",
    Pane.ARTISTS: "Artists",
    Pane.PLAYLISTS: "Playlists",
}

# Column headers for each pane's DataTable
_PANE_COLUMNS: dict[Pane, tuple[str, ...]] = {
    Pane.TRACKS: ("Title", "Artist", "Album", "Duration"),
    Pane.ALBUMS: ("Title", "Artist", "Year"),
    Pane.ARTISTS: ("Name",),
    Pane.PLAYLISTS: ("Title", "Tracks"),
}


# ---------------------------------------------------------------------------
# Search result pane widget
# ---------------------------------------------------------------------------


class _SearchPane(Vertical):
    """A titled pane containing a DataTable for one search category."""

    DEFAULT_CSS = """
    _SearchPane {
        height: 1fr;
        border: solid $primary-background;
        padding: 0;
    }
    _SearchPane.focused-pane {
        border: solid $accent;
    }
    _SearchPane .pane-title {
        height: 1;
        padding: 0 1;
        text-style: bold;
        color: $text-muted;
    }
    _SearchPane.focused-pane .pane-title {
        color: $accent;
    }
    """

    def __init__(
        self,
        pane: Pane,
        **kwargs: Any,
    ) -> None:
        super().__init__(id=_PANE_IDS[pane], **kwargs)
        self.pane = pane


# ---------------------------------------------------------------------------
# SearchView
# ---------------------------------------------------------------------------


class SearchView(FetchView):
    """Search YouTube Music with a multi-category grid layout.

    Enter on the input triggers a search. Tab/Shift-Tab cycles focus
    between the four result panes. Enter on a row dispatches an action
    based on the pane type.
    """

    STATUS_LABEL_ID: ClassVar[str] = "#search-status"

    DEFAULT_CSS = """
    SearchView {
        width: 1fr;
        height: 1fr;
    }
    SearchView #search-input {
        dock: top;
        margin: 1 1 0 1;
    }
    SearchView #search-status {
        height: 1;
        padding: 0 1;
        text-style: italic;
        color: $text-muted;
    }
    SearchView #search-grid {
        height: 1fr;
        padding: 0 1;
    }
    SearchView .search-row {
        height: 1fr;
    }
    SearchView #search-grid.vertical-layout .search-row {
        layout: horizontal;
        height: auto;
    }
    """

    def __init__(self, **kwargs: Any) -> None:
        super().__init__(**kwargs)
        self._results: SearchResults | None = None
        self._track_list: list[Track] = []
        self._album_list: list[AlbumInfo] = []
        self._artist_list: list[RelatedArtist] = []
        self._playlist_list: list[PlaylistInfo] = []
        self._focused_pane: Pane = Pane.TRACKS
        self._orientation: Orientation = Orientation.HORIZONTAL

    def compose(self) -> ComposeResult:
        """Build the search layout: input, status, 2x2 grid of panes."""
        yield Input(placeholder="Search YouTube Music...", id="search-input")
        yield Label("", id="search-status")
        with Vertical(id="search-grid"):
            for row_panes in (
                (Pane.TRACKS, Pane.ALBUMS),
                (Pane.ARTISTS, Pane.PLAYLISTS),
            ):
                with Horizontal(classes="search-row"):
                    for pane in row_panes:
                        with _SearchPane(pane):
                            yield Label(_PANE_TITLES[pane], classes="pane-title")
                            table: DataTable[Any] = DataTable(id=_TABLE_IDS[pane])
                            table.cursor_type = "row"
                            table.add_columns(*_PANE_COLUMNS[pane])
                            yield table
        yield FilterBar(_TABLE_IDS[Pane.TRACKS], id="search-filter")

    # -----------------------------------------------------------------
    # Input handling
    # -----------------------------------------------------------------

    def on_show(self) -> None:
        """Auto-focus the active pane when the view becomes visible."""
        self._switch_focus(self._focused_pane)

    def on_input_submitted(self, event: Input.Submitted) -> None:
        """Handle Enter in the search input.

        A ``#category:query`` prefix restricts the search to one result
        type; anything else searches across all categories.
        """
        query = event.value.strip()
        if not query:
            return
        category, parsed_query = _parse_search_prefix(query)
        self._run_search(parsed_query, category)

    def _run_search(self, query: str, category: str | None = None) -> None:
        """Search in a background thread and populate every pane.

        When *category* is given, the API call is restricted to that
        result type and only the matching pane is filled. A "Searching..."
        status is shown until results arrive.
        """
        self._run_fetch(
            lambda: self.music_app.music_api.search_all(query, limit=20, filter=category),
            self._populate_all_results,
            loading="Searching...",
        )

    # -----------------------------------------------------------------
    # Status / populate
    # -----------------------------------------------------------------

    @teardown_safe
    def _populate_all_results(self, results: SearchResults) -> None:
        """Fill all four panes with categorized search results."""
        self._results = results
        self._track_list = list(results.tracks)
        self._album_list = list(results.albums)
        self._artist_list = list(results.artists)
        self._playlist_list = list(results.playlists)

        total = (
            len(results.tracks)
            + len(results.albums)
            + len(results.artists)
            + len(results.playlists)
        )

        if total == 0:
            self._set_status("No results found")
        else:
            self._set_status(f"{total} result(s)")

        # Populate tracks pane
        tracks_table = self.query_one(f"#{_TABLE_IDS[Pane.TRACKS]}", DataTable)
        tracks_table.clear()
        for track in results.tracks:
            tracks_table.add_row(
                track.title,
                track.artist,
                track.album,
                _format_duration(track.duration_seconds),
            )

        # Populate albums pane
        albums_table = self.query_one(f"#{_TABLE_IDS[Pane.ALBUMS]}", DataTable)
        albums_table.clear()
        for album in results.albums:
            albums_table.add_row(album.title, album.artist, album.year)

        # Populate artists pane
        artists_table = self.query_one(f"#{_TABLE_IDS[Pane.ARTISTS]}", DataTable)
        artists_table.clear()
        for artist in results.artists:
            artists_table.add_row(artist.name)

        # Populate playlists pane
        playlists_table = self.query_one(f"#{_TABLE_IDS[Pane.PLAYLISTS]}", DataTable)
        playlists_table.clear()
        for playlist in results.playlists:
            playlists_table.add_row(playlist.title, str(playlist.track_count))

        # Focus the first non-empty pane
        if results.tracks:
            self._switch_focus(Pane.TRACKS)
        elif results.albums:
            self._switch_focus(Pane.ALBUMS)
        elif results.artists:
            self._switch_focus(Pane.ARTISTS)
        elif results.playlists:
            self._switch_focus(Pane.PLAYLISTS)

    # -----------------------------------------------------------------
    # Focus management
    # -----------------------------------------------------------------

    def _switch_focus(self, pane: Pane) -> None:
        """Switch visual focus to the given pane."""
        # Remove focused class from old pane
        for p in Pane:
            pane_widget = self.query_one(f"#{_PANE_IDS[p]}", _SearchPane)
            pane_widget.remove_class("focused-pane")

        # Add focused class to new pane
        self._focused_pane = pane
        pane_widget = self.query_one(f"#{_PANE_IDS[pane]}", _SearchPane)
        pane_widget.add_class("focused-pane")

        # Focus the DataTable inside the pane
        table = self.query_one(f"#{_TABLE_IDS[pane]}", DataTable)
        table.focus()

    def focus_next_pane(self) -> None:
        """Cycle focus to the next pane (Tab)."""
        next_pane = Pane((self._focused_pane + 1) % _PANE_COUNT)
        self._switch_focus(next_pane)

    def focus_previous_pane(self) -> None:
        """Cycle focus to the previous pane (Shift-Tab)."""
        prev_pane = Pane((self._focused_pane - 1) % _PANE_COUNT)
        self._switch_focus(prev_pane)

    @property
    def focused_pane(self) -> Pane:
        """The currently focused pane."""
        return self._focused_pane

    def on_key(self, event: object) -> None:
        """Handle Tab/Shift-Tab to cycle pane focus."""
        key = getattr(event, "key", "")
        if key == "tab":
            self.focus_next_pane()
        elif key == "shift+tab":
            self.focus_previous_pane()

    # -----------------------------------------------------------------
    # Row selection
    # -----------------------------------------------------------------

    def on_data_table_row_selected(self, event: DataTable.RowSelected) -> None:
        """Handle Enter on a result row: dispatch based on the pane."""
        row_index = event.cursor_row

        # Determine which pane the event came from
        table_id = event.data_table.id
        if table_id == _TABLE_IDS[Pane.TRACKS]:
            self._on_track_selected(row_index)
        elif table_id == _TABLE_IDS[Pane.ALBUMS]:
            self._on_album_selected(row_index)
        elif table_id == _TABLE_IDS[Pane.ARTISTS]:
            self._on_artist_selected(row_index)
        elif table_id == _TABLE_IDS[Pane.PLAYLISTS]:
            self._on_playlist_selected(row_index)

    def _on_track_selected(self, row_index: int) -> None:
        """Queue and play the selected track."""
        if row_index < 0 or row_index >= len(self._track_list):
            return

        track = self._track_list[row_index]
        self.music_app.queue_manager.set_playlist([track], start_index=0)
        self.music_app.player.play(track.video_id)

    def _on_album_selected(self, row_index: int) -> None:
        """Open the album detail view."""
        if row_index < 0 or row_index >= len(self._album_list):
            return

        album = self._album_list[row_index]
        self.music_app.action_open_album(album.browse_id)

    def _on_artist_selected(self, row_index: int) -> None:
        """Open the artist detail view."""
        if row_index < 0 or row_index >= len(self._artist_list):
            return

        artist = self._artist_list[row_index]
        self.music_app.action_open_artist(artist.channel_id)

    def _on_playlist_selected(self, row_index: int) -> None:
        """Open the playlist view with the selected playlist's tracks."""
        if row_index < 0 or row_index >= len(self._playlist_list):
            return

        playlist = self._playlist_list[row_index]
        self.music_app.action_switch_view("playlist")
        self.music_app.query_one(PlaylistView).show_track_list(playlist)

    # -----------------------------------------------------------------
    # Public API
    # -----------------------------------------------------------------

    def get_focused_item(self) -> Track | PlaylistInfo | AlbumInfo | None:
        """Return the item under the cursor in the active pane.

        Returns:
            The Track, AlbumInfo, PlaylistInfo, or RelatedArtist at the
            cursor row, or ``None`` if the pane is empty.
        """

        row_index = self._cursor_row(f"#{_TABLE_IDS[self._focused_pane]}")
        if row_index is None:
            return None

        if self._focused_pane == Pane.TRACKS:
            if 0 <= row_index < len(self._track_list):
                return self._track_list[row_index]
        elif self._focused_pane == Pane.ALBUMS:
            if 0 <= row_index < len(self._album_list):
                return self._album_list[row_index]
        elif self._focused_pane == Pane.PLAYLISTS and 0 <= row_index < len(self._playlist_list):
            return self._playlist_list[row_index]
        return None

    def toggle_filter(self) -> None:
        """Toggle the filter bar for the currently focused pane's table."""
        filter_bar = self.query_one("#search-filter", FilterBar)
        if filter_bar.is_visible:
            filter_bar.hide()
        else:
            # Retarget to the currently focused pane
            target_id = _TABLE_IDS[self._focused_pane]
            filter_bar.retarget(target_id)
            filter_bar.show()

    def update_orientation(self, orientation: Orientation) -> None:
        """Switch between 2x2 grid (horizontal) and 4x1 stack (vertical).

        In horizontal mode the search grid uses two rows of two panes.
        In vertical mode all four panes stack into a single column.

        Args:
            orientation: The new layout orientation.
        """
        if orientation == self._orientation:
            return

        self._orientation = orientation
        try:
            grid = self.query_one("#search-grid")
        except NoMatches:
            return

        if orientation is Orientation.VERTICAL:
            grid.add_class("vertical-layout")
        else:
            grid.remove_class("vertical-layout")

    def focus_input(self) -> None:
        """Focus the search input widget."""
        self.query_one("#search-input", Input).focus()
