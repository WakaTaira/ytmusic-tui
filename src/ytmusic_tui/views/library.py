"""Library view with 3-pane layout: Playlists, Albums, Artists.

Inspired by spotify_player's library page. Three side-by-side panes
display the user's playlists, saved albums, and followed artists.
Tab/Shift-Tab cycles focus between panes. Enter drills into the
selected item.
"""

from __future__ import annotations

from enum import Enum
from typing import TYPE_CHECKING, Any, ClassVar

from textual.containers import Horizontal, Vertical
from textual.css.query import NoMatches
from textual.widgets import DataTable, Label

from ytmusic_tui.formatting import format_duration as _format_duration
from ytmusic_tui.layout import Orientation
from ytmusic_tui.views.base import FetchView
from ytmusic_tui.views.filter_bar import FilterBar
from ytmusic_tui.views.guards import teardown_safe

if TYPE_CHECKING:
    from textual.app import ComposeResult

    from ytmusic_tui.api import AlbumInfo, ArtistInfo, PlaylistInfo
    from ytmusic_tui.queue import Track


class LibraryPane(Enum):
    """Active pane in the library view."""

    PLAYLISTS = "playlists"
    ALBUMS = "albums"
    ARTISTS = "artists"


# Ordered list for Tab cycling
_PANE_ORDER: list[LibraryPane] = [
    LibraryPane.PLAYLISTS,
    LibraryPane.ALBUMS,
    LibraryPane.ARTISTS,
]

# Table IDs for each pane
_LIBRARY_TABLE_IDS: dict[LibraryPane, str] = {
    LibraryPane.PLAYLISTS: "library-playlists",
    LibraryPane.ALBUMS: "library-albums",
    LibraryPane.ARTISTS: "library-artists",
}


class LibraryView(FetchView):
    """Library view with three panes: Playlists, Albums, Artists.

    Tab/Shift-Tab cycles focus between panes. Enter drills into
    the selected item. Escape returns from track list to playlist list.
    """

    STATUS_LABEL_ID: ClassVar[str] = "#library-status"

    DEFAULT_CSS = """
    LibraryView {
        width: 1fr;
        height: 1fr;
    }
    LibraryView #library-status {
        height: 1;
        padding: 0 1;
        text-style: italic;
        color: $text-muted;
    }
    LibraryView #library-panes {
        width: 1fr;
        height: 1fr;
    }
    LibraryView .library-pane {
        height: 1fr;
        padding: 0 1;
    }
    LibraryView .library-pane-playlists {
        width: 2fr;
    }
    LibraryView .library-pane-albums {
        width: 2fr;
    }
    LibraryView .library-pane-artists {
        width: 1fr;
    }
    LibraryView .pane-label {
        height: 1;
        text-style: bold;
        color: $text-muted;
    }
    LibraryView .pane-label-active {
        height: 1;
        text-style: bold;
        color: $accent;
    }
    LibraryView #library-panes.vertical-layout {
        layout: vertical;
    }
    LibraryView #library-panes.vertical-layout .library-pane {
        width: 1fr;
        height: 1fr;
    }
    """

    def __init__(self, **kwargs: Any) -> None:
        super().__init__(**kwargs)
        self._active_pane: LibraryPane = LibraryPane.PLAYLISTS
        self._playlists: list[PlaylistInfo] = []
        self._albums: list[AlbumInfo] = []
        self._artists: list[ArtistInfo] = []
        self._tracks: list[Track] = []
        self._viewing_tracks: bool = False
        self._current_playlist_title: str = ""
        self._orientation: Orientation = Orientation.HORIZONTAL

    def compose(self) -> ComposeResult:
        """Build the 3-pane library layout."""
        yield Label("", id="library-status")
        with Horizontal(id="library-panes"):
            # Playlists pane (left)
            with Vertical(classes="library-pane library-pane-playlists"):
                yield Label("Playlists", id="pane-label-playlists", classes="pane-label-active")
                table: DataTable[Any] = DataTable(id="library-playlists")
                table.cursor_type = "row"
                yield table
            # Albums pane (center)
            with Vertical(classes="library-pane library-pane-albums"):
                yield Label("Albums", id="pane-label-albums", classes="pane-label")
                table = DataTable(id="library-albums")
                table.cursor_type = "row"
                yield table
            # Artists pane (right)
            with Vertical(classes="library-pane library-pane-artists"):
                yield Label("Artists", id="pane-label-artists", classes="pane-label")
                table = DataTable(id="library-artists")
                table.cursor_type = "row"
                yield table
        yield FilterBar("library-playlists", id="library-filter")

    def on_mount(self) -> None:
        """Initialize all three panes and fetch data."""
        self.refresh_library()

    def refresh_library(self) -> None:
        """Set up columns, reset labels, and re-fetch all library data.

        Safe to call after mount to reload content without remounting.
        """
        self._setup_columns()
        self._update_pane_labels()
        self._fetch_all_data()

    def on_show(self) -> None:
        """Focus the active pane's DataTable when the view becomes visible.

        Without this, returning to Library from another view leaves
        focus on an ancestor widget and requires two Tab presses to
        reach a pane.
        """
        self._focus_active_table()

    # ------------------------------------------------------------------
    # Column setup
    # ------------------------------------------------------------------

    def _setup_columns(self) -> None:
        """Set up columns for all three tables."""
        playlists_table = self.query_one("#library-playlists", DataTable)
        playlists_table.clear(columns=True)
        playlists_table.add_columns("Title", "Tracks")

        albums_table = self.query_one("#library-albums", DataTable)
        albums_table.clear(columns=True)
        albums_table.add_columns("Title", "Artist", "Year")

        artists_table = self.query_one("#library-artists", DataTable)
        artists_table.clear(columns=True)
        artists_table.add_columns(
            "Name",
        )

    # ------------------------------------------------------------------
    # Pane focus management
    # ------------------------------------------------------------------

    def _update_pane_labels(self) -> None:
        """Highlight the active pane label."""
        for pane in _PANE_ORDER:
            label_id = f"pane-label-{pane.value}"
            label = self.query_one(f"#{label_id}", Label)
            if pane is self._active_pane:
                label.set_classes("pane-label-active")
            else:
                label.set_classes("pane-label")

    def _focus_active_table(self) -> None:
        """Move keyboard focus to the active pane's DataTable."""
        table_ids = {
            LibraryPane.PLAYLISTS: "#library-playlists",
            LibraryPane.ALBUMS: "#library-albums",
            LibraryPane.ARTISTS: "#library-artists",
        }
        table = self.query_one(table_ids[self._active_pane], DataTable)
        table.focus()

    def focus_next_pane(self) -> None:
        """Move focus to the next pane (Tab)."""
        idx = _PANE_ORDER.index(self._active_pane)
        self._active_pane = _PANE_ORDER[(idx + 1) % len(_PANE_ORDER)]
        self._update_pane_labels()
        self._focus_active_table()

    def focus_previous_pane(self) -> None:
        """Move focus to the previous pane (Shift-Tab)."""
        idx = _PANE_ORDER.index(self._active_pane)
        self._active_pane = _PANE_ORDER[(idx - 1) % len(_PANE_ORDER)]
        self._update_pane_labels()
        self._focus_active_table()

    def on_key(self, event: object) -> None:
        """Handle Tab/Shift-Tab for pane cycling, Escape to go back."""
        key_event = event
        key = getattr(key_event, "key", None)

        if key == "tab":
            if self._viewing_tracks:
                return
            self.focus_next_pane()
            return

        if key == "shift+tab":
            if self._viewing_tracks:
                return
            self.focus_previous_pane()
            return

        # Let the filter bar handle its own Escape
        filter_bar = self.query_one("#library-filter", FilterBar)
        if key == "escape" and filter_bar.is_visible:
            return

        if key == "escape" and self._viewing_tracks:
            self._restore_playlists_pane()

    # ------------------------------------------------------------------
    # Data fetching
    # ------------------------------------------------------------------

    def _fetch_all_data(self) -> None:
        """Kick off parallel fetches for all three data sources.

        One combined "Loading library..." status is shown up front; the
        three workers each pass ``loading=None`` so they do not stomp on
        it (any of them may set an error status on failure).
        """
        self._set_status("Loading library...")
        api = self.music_app.music_api
        self._run_fetch(api.get_library_playlists, self._populate_playlists)
        self._run_fetch(api.get_library_albums, self._populate_albums)
        self._run_fetch(api.get_library_artists, self._populate_artists)

    # ------------------------------------------------------------------
    # Population callbacks
    # ------------------------------------------------------------------

    @teardown_safe
    def _populate_playlists(self, playlists: list[PlaylistInfo]) -> None:
        """Fill the playlists pane with data."""
        self._playlists = playlists
        table = self.query_one("#library-playlists", DataTable)
        table.clear()

        if not playlists:
            self._update_combined_status()
            return

        for pl in playlists:
            table.add_row(pl.title, str(pl.track_count))

        self._update_combined_status()

    @teardown_safe
    def _populate_albums(self, albums: list[AlbumInfo]) -> None:
        """Fill the albums pane with data."""
        self._albums = albums
        table = self.query_one("#library-albums", DataTable)
        table.clear()

        if not albums:
            self._update_combined_status()
            return

        for album in albums:
            table.add_row(album.title, album.artist, album.year)

        self._update_combined_status()

    @teardown_safe
    def _populate_artists(self, artists: list[ArtistInfo]) -> None:
        """Fill the artists pane with data."""
        self._artists = artists
        table = self.query_one("#library-artists", DataTable)
        table.clear()

        if not artists:
            self._update_combined_status()
            return

        for artist in artists:
            table.add_row(artist.name)

        self._update_combined_status()

    # ------------------------------------------------------------------
    # Playlist drill-down (track list)
    # ------------------------------------------------------------------

    def _show_track_list(self, playlist: PlaylistInfo) -> None:
        """Replace playlists pane content with playlist tracks."""
        self._viewing_tracks = True
        self._current_playlist_title = playlist.title

        table = self.query_one("#library-playlists", DataTable)
        table.clear(columns=True)
        table.add_columns("Title", "Artist", "Album", "Duration")

        playlist_id = playlist.playlist_id
        self._run_fetch(
            lambda: self.music_app.music_api.get_playlist_tracks(playlist_id),
            self._populate_tracks,
            loading=f"Loading tracks for {playlist.title}...",
        )

    @teardown_safe
    def _populate_tracks(self, tracks: list[Track]) -> None:
        """Fill the playlists pane with track data."""
        self._tracks = tracks
        table = self.query_one("#library-playlists", DataTable)
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

    def _restore_playlists_pane(self) -> None:
        """Restore the playlists pane from track list view."""
        self._viewing_tracks = False
        self._tracks = []

        table = self.query_one("#library-playlists", DataTable)
        table.clear(columns=True)
        table.add_columns("Title", "Tracks")

        for pl in self._playlists:
            table.add_row(pl.title, str(pl.track_count))

        self._update_combined_status()

    # ------------------------------------------------------------------
    # Row selection handler
    # ------------------------------------------------------------------

    def on_data_table_row_selected(self, event: DataTable.RowSelected) -> None:
        """Handle Enter on a row in any pane.

        Playlists pane (list): drill into the selected playlist.
        Playlists pane (tracks): queue remaining tracks from position.
        Albums pane: open album in AlbumView.
        Artists pane: open artist in ArtistView.
        """
        table_id = event.data_table.id
        row_index = event.cursor_row

        if table_id == "library-playlists":
            self._handle_playlist_selection(row_index)
        elif table_id == "library-albums":
            self._handle_album_selection(row_index)
        elif table_id == "library-artists":
            self._handle_artist_selection(row_index)

    def _handle_playlist_selection(self, row_index: int) -> None:
        """Handle selection in the playlists pane."""
        if self._viewing_tracks:
            # Track list mode: queue from selected position
            if row_index < 0 or row_index >= len(self._tracks):
                return
            track = self._tracks[row_index]
            self.music_app.queue_manager.set_playlist(self._tracks, start_index=row_index)
            self.music_app.player.play(track.video_id)
        else:
            # Playlist list mode: drill into playlist
            if row_index < 0 or row_index >= len(self._playlists):
                return
            playlist = self._playlists[row_index]
            self._show_track_list(playlist)

    def _handle_album_selection(self, row_index: int) -> None:
        """Open the selected album in AlbumView."""
        if row_index < 0 or row_index >= len(self._albums):
            return

        album = self._albums[row_index]
        self.music_app.action_open_album(album.browse_id)

    def _handle_artist_selection(self, row_index: int) -> None:
        """Open the selected artist in ArtistView."""
        if row_index < 0 or row_index >= len(self._artists):
            return

        artist = self._artists[row_index]
        self.music_app.action_open_artist(artist.channel_id)

    # ------------------------------------------------------------------
    # Helpers
    # ------------------------------------------------------------------

    def get_focused_item(self) -> Track | PlaylistInfo | AlbumInfo | None:
        """Return the item at the cursor in the active pane.

        In playlist track-list mode: returns a Track.
        Otherwise: returns PlaylistInfo, AlbumInfo, or ArtistInfo based
        on the active pane.
        """
        if self._active_pane is LibraryPane.PLAYLISTS:
            row_index = self._cursor_row("#library-playlists")
            if row_index is None:
                return None
            if self._viewing_tracks:
                if 0 <= row_index < len(self._tracks):
                    return self._tracks[row_index]
            else:
                if 0 <= row_index < len(self._playlists):
                    return self._playlists[row_index]
        elif self._active_pane is LibraryPane.ALBUMS:
            row_index = self._cursor_row("#library-albums")
            if row_index is None:
                return None
            if 0 <= row_index < len(self._albums):
                return self._albums[row_index]
        return None

    def toggle_filter(self) -> None:
        """Toggle the filter bar for the active pane's table."""
        filter_bar = self.query_one("#library-filter", FilterBar)
        if filter_bar.is_visible:
            filter_bar.hide()
        else:
            target_id = _LIBRARY_TABLE_IDS[self._active_pane]
            filter_bar.retarget(target_id)
            filter_bar.show()

    def update_orientation(self, orientation: Orientation) -> None:
        """Switch between horizontal and vertical pane layout.

        Args:
            orientation: The new layout orientation.
        """
        if orientation == self._orientation:
            return

        self._orientation = orientation
        try:
            panes_container = self.query_one("#library-panes")
        except NoMatches:
            return

        if orientation is Orientation.VERTICAL:
            panes_container.add_class("vertical-layout")
        else:
            panes_container.remove_class("vertical-layout")

    def _update_combined_status(self) -> None:
        """Update status with counts from all panes."""
        parts: list[str] = []
        if self._playlists:
            parts.append(f"{len(self._playlists)} playlist(s)")
        if self._albums:
            parts.append(f"{len(self._albums)} album(s)")
        if self._artists:
            parts.append(f"{len(self._artists)} artist(s)")

        if parts:
            self._set_status(" | ".join(parts))
        else:
            self._set_status("Library empty")
