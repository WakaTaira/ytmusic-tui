"""Tests for the FilterBar widget and its integration with views."""

from __future__ import annotations

from typing import TYPE_CHECKING
from unittest.mock import MagicMock

import pytest
from helpers import make_app as _make_app
from helpers import make_tracks as _make_tracks

from ytmusic_tui.api import (
    AlbumInfo,
    ArtistInfo,
    PlaylistInfo,
    RelatedArtist,
    SearchResults,
)
from ytmusic_tui.views.filter_bar import FilterBar

if TYPE_CHECKING:
    from ytmusic_tui.queue import Track

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _make_playlist_info(n: int) -> PlaylistInfo:
    """Create a dummy PlaylistInfo."""
    return PlaylistInfo(
        playlist_id=f"PL_{n}",
        title=f"Playlist {n}",
        description=f"Description {n}",
        track_count=n * 5,
    )


def _make_album_info(n: int) -> AlbumInfo:
    """Create a dummy AlbumInfo."""
    return AlbumInfo(
        browse_id=f"MPREb_{n}",
        title=f"Album {n}",
        artist=f"Artist {n}",
        year=str(2020 + n),
    )


def _make_artist_info(n: int) -> ArtistInfo:
    """Create a dummy ArtistInfo (library-style, minimal data)."""
    return ArtistInfo(
        channel_id=f"UC_{n}",
        name=f"Artist {n}",
    )


def _make_related_artist(n: int) -> RelatedArtist:
    """Create a dummy RelatedArtist for search results."""
    return RelatedArtist(
        channel_id=f"UCsearch_{n}",
        name=f"Search Artist {n}",
    )


def _make_search_results(
    *,
    tracks: list[Track] | None = None,
    albums: list[AlbumInfo] | None = None,
    artists: list[RelatedArtist] | None = None,
    playlists: list[PlaylistInfo] | None = None,
) -> SearchResults:
    """Create a SearchResults with the given content."""
    return SearchResults(
        tracks=tracks or [],
        albums=albums or [],
        artists=artists or [],
        playlists=playlists or [],
    )


# ===================================================================
# FilterBar widget tests
# ===================================================================


class TestFilterBarWidget:
    """Unit tests for FilterBar show/hide and filtering logic."""

    @pytest.mark.asyncio
    async def test_filter_bar_initially_hidden(self) -> None:
        """FilterBar should be hidden (display: none) by default."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from ytmusic_tui.views.queue import QueueView

            view = app.query_one(QueueView)
            filter_bar = view.query_one("#queue-filter", FilterBar)
            assert not filter_bar.is_visible

    @pytest.mark.asyncio
    async def test_filter_bar_show_makes_visible(self) -> None:
        """Calling show() should make the FilterBar visible."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from ytmusic_tui.views.queue import QueueView

            # Populate some data so the filter has rows
            tracks = _make_tracks(3)
            app.queue_manager.set_playlist(tracks)
            app.action_switch_view("queue")
            await _pilot.pause()

            view = app.query_one(QueueView)
            view.refresh_queue()
            await _pilot.pause()

            filter_bar = view.query_one("#queue-filter", FilterBar)
            filter_bar.show()
            await _pilot.pause()

            assert filter_bar.is_visible

    @pytest.mark.asyncio
    async def test_filter_bar_hide_restores_rows(self) -> None:
        """Hiding the filter bar should restore all original rows."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import DataTable

            from ytmusic_tui.views.queue import QueueView

            tracks = _make_tracks(5)
            app.queue_manager.set_playlist(tracks)
            app.action_switch_view("queue")
            await _pilot.pause()

            view = app.query_one(QueueView)
            view.refresh_queue()
            await _pilot.pause()

            table = view.query_one("#queue-table", DataTable)
            assert table.row_count == 5

            filter_bar = view.query_one("#queue-filter", FilterBar)
            filter_bar.show()
            await _pilot.pause()

            # Type a filter that matches nothing
            filter_bar._apply_filter("zzzzz_no_match")
            await _pilot.pause()
            assert table.row_count == 0

            # Hide should restore
            filter_bar.hide()
            await _pilot.pause()
            assert table.row_count == 5
            assert not filter_bar.is_visible

    @pytest.mark.asyncio
    async def test_filter_reduces_visible_rows(self) -> None:
        """Filtering should reduce the number of visible rows."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import DataTable

            from ytmusic_tui.views.queue import QueueView

            tracks = _make_tracks(5)
            app.queue_manager.set_playlist(tracks)
            app.action_switch_view("queue")
            await _pilot.pause()

            view = app.query_one(QueueView)
            view.refresh_queue()
            await _pilot.pause()

            filter_bar = view.query_one("#queue-filter", FilterBar)
            filter_bar.show()
            await _pilot.pause()

            table = view.query_one("#queue-table", DataTable)
            assert table.row_count == 5

            # Filter for "Song 3" -- should match one row
            filter_bar._apply_filter("Song 3")
            await _pilot.pause()
            assert table.row_count == 1

    @pytest.mark.asyncio
    async def test_filter_is_case_insensitive(self) -> None:
        """Filtering should be case-insensitive."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import DataTable

            from ytmusic_tui.views.queue import QueueView

            tracks = _make_tracks(3)
            app.queue_manager.set_playlist(tracks)
            app.action_switch_view("queue")
            await _pilot.pause()

            view = app.query_one(QueueView)
            view.refresh_queue()
            await _pilot.pause()

            filter_bar = view.query_one("#queue-filter", FilterBar)
            filter_bar.show()
            await _pilot.pause()

            table = view.query_one("#queue-table", DataTable)

            # Use uppercase
            filter_bar._apply_filter("SONG 2")
            await _pilot.pause()
            assert table.row_count == 1

            # Use lowercase
            filter_bar._apply_filter("song 2")
            await _pilot.pause()
            assert table.row_count == 1

    @pytest.mark.asyncio
    async def test_empty_filter_shows_all_rows(self) -> None:
        """An empty filter query should show all rows."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import DataTable

            from ytmusic_tui.views.queue import QueueView

            tracks = _make_tracks(3)
            app.queue_manager.set_playlist(tracks)
            app.action_switch_view("queue")
            await _pilot.pause()

            view = app.query_one(QueueView)
            view.refresh_queue()
            await _pilot.pause()

            filter_bar = view.query_one("#queue-filter", FilterBar)
            filter_bar.show()
            await _pilot.pause()

            table = view.query_one("#queue-table", DataTable)

            # Filter to reduce rows
            filter_bar._apply_filter("Song 1")
            await _pilot.pause()
            assert table.row_count == 1

            # Clear filter
            filter_bar._apply_filter("")
            await _pilot.pause()
            assert table.row_count == 3

    @pytest.mark.asyncio
    async def test_filter_matches_any_column(self) -> None:
        """Filter should match text in any column, not just the first."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import DataTable

            from ytmusic_tui.views.queue import QueueView

            tracks = _make_tracks(3)
            app.queue_manager.set_playlist(tracks)
            app.action_switch_view("queue")
            await _pilot.pause()

            view = app.query_one(QueueView)
            view.refresh_queue()
            await _pilot.pause()

            filter_bar = view.query_one("#queue-filter", FilterBar)
            filter_bar.show()
            await _pilot.pause()

            table = view.query_one("#queue-table", DataTable)

            # Filter by artist name (second column for queue)
            filter_bar._apply_filter("Artist 2")
            await _pilot.pause()
            assert table.row_count == 1

            # Filter by album name
            filter_bar._apply_filter("Album 3")
            await _pilot.pause()
            assert table.row_count == 1

    @pytest.mark.asyncio
    async def test_filter_count_label_updates(self) -> None:
        """The count label should show visible/total."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import Label

            from ytmusic_tui.views.queue import QueueView

            tracks = _make_tracks(5)
            app.queue_manager.set_playlist(tracks)
            app.action_switch_view("queue")
            await _pilot.pause()

            view = app.query_one(QueueView)
            view.refresh_queue()
            await _pilot.pause()

            filter_bar = view.query_one("#queue-filter", FilterBar)
            filter_bar.show()
            await _pilot.pause()

            count_label = filter_bar.query_one("#filter-count", Label)
            assert "5/5" in count_label.content

            filter_bar._apply_filter("Song 1")
            await _pilot.pause()
            assert "1/5" in count_label.content

    @pytest.mark.asyncio
    async def test_filter_bar_retarget(self) -> None:
        """retarget() should change the targeted table."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from ytmusic_tui.views.search import SearchView

            app.action_switch_view("search")
            await _pilot.pause()

            view = app.query_one(SearchView)
            filter_bar = view.query_one("#search-filter", FilterBar)

            assert filter_bar.target_table_id == "search-tracks"

            filter_bar.retarget("search-albums")
            assert filter_bar.target_table_id == "search-albums"


# ===================================================================
# View integration tests
# ===================================================================


class TestQueueViewFilter:
    """FilterBar integration with QueueView."""

    @pytest.mark.asyncio
    async def test_queue_toggle_filter(self) -> None:
        """toggle_filter() should show and hide the filter bar."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from ytmusic_tui.views.queue import QueueView

            tracks = _make_tracks(3)
            app.queue_manager.set_playlist(tracks)
            app.action_switch_view("queue")
            await _pilot.pause()

            view = app.query_one(QueueView)
            view.refresh_queue()
            await _pilot.pause()

            filter_bar = view.query_one("#queue-filter", FilterBar)
            assert not filter_bar.is_visible

            view.toggle_filter()
            await _pilot.pause()
            assert filter_bar.is_visible

            view.toggle_filter()
            await _pilot.pause()
            assert not filter_bar.is_visible


class TestPlaylistViewFilter:
    """FilterBar integration with PlaylistView."""

    @pytest.mark.asyncio
    async def test_playlist_toggle_filter(self) -> None:
        """toggle_filter() should show and hide the filter bar."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from ytmusic_tui.views.playlist import PlaylistView

            app.action_switch_view("playlist")
            await _pilot.pause()

            view = app.query_one(PlaylistView)
            playlists = [_make_playlist_info(1), _make_playlist_info(2)]
            view._populate_playlists(playlists)
            await _pilot.pause()

            filter_bar = view.query_one("#playlist-filter", FilterBar)
            assert not filter_bar.is_visible

            view.toggle_filter()
            await _pilot.pause()
            assert filter_bar.is_visible

            view.toggle_filter()
            await _pilot.pause()
            assert not filter_bar.is_visible

    @pytest.mark.asyncio
    async def test_playlist_filter_reduces_rows(self) -> None:
        """Filtering should reduce visible playlist rows."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import DataTable

            from ytmusic_tui.views.playlist import PlaylistView

            app.action_switch_view("playlist")
            await _pilot.pause()

            view = app.query_one(PlaylistView)
            playlists = [_make_playlist_info(1), _make_playlist_info(2), _make_playlist_info(3)]
            view._populate_playlists(playlists)
            await _pilot.pause()

            table = view.query_one("#playlist-table", DataTable)
            assert table.row_count == 3

            filter_bar = view.query_one("#playlist-filter", FilterBar)
            filter_bar.show()
            await _pilot.pause()

            filter_bar._apply_filter("Playlist 2")
            await _pilot.pause()
            assert table.row_count == 1


class TestAlbumViewFilter:
    """FilterBar integration with AlbumView."""

    @pytest.mark.asyncio
    async def test_album_toggle_filter(self) -> None:
        """toggle_filter() should show and hide the filter bar."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from ytmusic_tui.views.album import AlbumView

            app.action_switch_view("album")
            await _pilot.pause()

            view = app.query_one(AlbumView)
            album = AlbumInfo(
                browse_id="MPREb_1",
                title="Test Album",
                artist="Test Artist",
                year="2024",
                tracks=_make_tracks(5),
            )
            view._populate(album)
            await _pilot.pause()

            filter_bar = view.query_one("#album-filter", FilterBar)
            assert not filter_bar.is_visible

            view.toggle_filter()
            await _pilot.pause()
            assert filter_bar.is_visible

            view.toggle_filter()
            await _pilot.pause()
            assert not filter_bar.is_visible


class TestSearchViewFilter:
    """FilterBar integration with SearchView."""

    @pytest.mark.asyncio
    async def test_search_toggle_filter(self) -> None:
        """toggle_filter() should show the filter bar."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from ytmusic_tui.views.search import SearchView

            app.action_switch_view("search")
            await _pilot.pause()

            view = app.query_one(SearchView)

            # Populate with some results
            results = _make_search_results(tracks=_make_tracks(3))
            view._populate_all_results(results)
            await _pilot.pause()

            filter_bar = view.query_one("#search-filter", FilterBar)
            assert not filter_bar.is_visible

            view.toggle_filter()
            await _pilot.pause()
            assert filter_bar.is_visible

            view.toggle_filter()
            await _pilot.pause()
            assert not filter_bar.is_visible

    @pytest.mark.asyncio
    async def test_search_filter_targets_focused_pane(self) -> None:
        """Filter bar should target the currently focused pane's table."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from ytmusic_tui.views.search import Pane, SearchView

            app.action_switch_view("search")
            await _pilot.pause()

            view = app.query_one(SearchView)
            filter_bar = view.query_one("#search-filter", FilterBar)

            # Default focus is TRACKS
            view.toggle_filter()
            await _pilot.pause()
            assert filter_bar.target_table_id == "search-tracks"
            filter_bar.hide()
            await _pilot.pause()

            # Switch focus to ALBUMS
            view._switch_focus(Pane.ALBUMS)
            view.toggle_filter()
            await _pilot.pause()
            assert filter_bar.target_table_id == "search-albums"


class TestLibraryViewFilter:
    """FilterBar integration with LibraryView."""

    @pytest.mark.asyncio
    async def test_library_toggle_filter(self) -> None:
        """toggle_filter() should show the filter bar."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from ytmusic_tui.views.library import LibraryView

            app.action_switch_view("library")
            await _pilot.pause()

            view = app.query_one(LibraryView)
            playlists = [_make_playlist_info(1)]
            view._populate_playlists(playlists)
            await _pilot.pause()

            filter_bar = view.query_one("#library-filter", FilterBar)
            assert not filter_bar.is_visible

            view.toggle_filter()
            await _pilot.pause()
            assert filter_bar.is_visible

            view.toggle_filter()
            await _pilot.pause()
            assert not filter_bar.is_visible

    @pytest.mark.asyncio
    async def test_library_filter_targets_active_pane(self) -> None:
        """Filter bar should target the active pane's table."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from ytmusic_tui.views.library import LibraryPane, LibraryView

            app.action_switch_view("library")
            await _pilot.pause()

            view = app.query_one(LibraryView)
            filter_bar = view.query_one("#library-filter", FilterBar)

            # Default active pane is playlists
            view.toggle_filter()
            await _pilot.pause()
            assert filter_bar.target_table_id == "library-playlists"
            filter_bar.hide()
            await _pilot.pause()

            # Switch to albums pane
            view._active_pane = LibraryPane.ALBUMS
            view.toggle_filter()
            await _pilot.pause()
            assert filter_bar.target_table_id == "library-albums"


class TestArtistViewFilter:
    """FilterBar integration with ArtistView."""

    @pytest.mark.asyncio
    async def test_artist_toggle_filter(self) -> None:
        """toggle_filter() should show the filter bar."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from ytmusic_tui.views.artist import ArtistView

            app.action_switch_view("artist")
            await _pilot.pause()

            view = app.query_one(ArtistView)
            filter_bar = view.query_one("#artist-filter", FilterBar)
            assert not filter_bar.is_visible

            view.toggle_filter()
            await _pilot.pause()
            assert filter_bar.is_visible

            view.toggle_filter()
            await _pilot.pause()
            assert not filter_bar.is_visible


# ===================================================================
# App-level integration (action_toggle_filter)
# ===================================================================


class TestAppToggleFilter:
    """App-level /  keybinding integration."""

    @pytest.mark.asyncio
    async def test_action_toggle_filter_on_queue(self) -> None:
        """action_toggle_filter should toggle the queue filter bar."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from ytmusic_tui.views.queue import QueueView

            tracks = _make_tracks(3)
            app.queue_manager.set_playlist(tracks)
            app.action_switch_view("queue")
            await _pilot.pause()

            view = app.query_one(QueueView)
            view.refresh_queue()
            await _pilot.pause()

            filter_bar = view.query_one("#queue-filter", FilterBar)
            assert not filter_bar.is_visible

            app.action_toggle_filter()
            await _pilot.pause()
            assert filter_bar.is_visible

            app.action_toggle_filter()
            await _pilot.pause()
            assert not filter_bar.is_visible

    @pytest.mark.asyncio
    async def test_action_toggle_filter_on_search(self) -> None:
        """action_toggle_filter should work on search view."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from ytmusic_tui.views.search import SearchView

            app.action_switch_view("search")
            await _pilot.pause()

            view = app.query_one(SearchView)
            results = _make_search_results(tracks=_make_tracks(2))
            view._populate_all_results(results)
            await _pilot.pause()

            filter_bar = view.query_one("#search-filter", FilterBar)
            assert not filter_bar.is_visible

            app.action_toggle_filter()
            await _pilot.pause()
            assert filter_bar.is_visible

    @pytest.mark.asyncio
    async def test_slash_binding_mapped_to_toggle_filter(self) -> None:
        """The slash key binding should map to action_toggle_filter."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            bindings = {b.key: b.action for b in app.BINDINGS}
            assert bindings.get("slash") == "toggle_filter"

    @pytest.mark.asyncio
    async def test_filter_bar_escape_closes(self) -> None:
        """Pressing Escape on an open filter bar should close it."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from ytmusic_tui.views.queue import QueueView

            tracks = _make_tracks(3)
            app.queue_manager.set_playlist(tracks)
            app.action_switch_view("queue")
            await _pilot.pause()

            view = app.query_one(QueueView)
            view.refresh_queue()
            await _pilot.pause()

            filter_bar = view.query_one("#queue-filter", FilterBar)
            filter_bar.show()
            await _pilot.pause()
            assert filter_bar.is_visible

            # Simulate Escape key event on the filter bar
            mock_event = MagicMock()
            mock_event.key = "escape"
            filter_bar.on_key(mock_event)
            await _pilot.pause()
            assert not filter_bar.is_visible


# ===================================================================
# FilterBar: missing target notifies the user (Task 3)
# ===================================================================


class TestFilterBarMissingTarget:
    """show() with a target table id that resolves to None must notify the user."""

    @pytest.mark.asyncio
    async def test_show_with_bad_target_notifies_and_stays_hidden(self) -> None:
        """When target_table is None (bad id), show() notifies and remains hidden."""
        from helpers import capture_notifications, make_app

        from ytmusic_tui.views.filter_bar import FilterBar
        from ytmusic_tui.views.queue import QueueView

        app = make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            captured = capture_notifications(app)
            app.action_switch_view("queue")
            await _pilot.pause()

            view = app.query_one(QueueView)
            filter_bar = view.query_one("#queue-filter", FilterBar)

            # Retarget to a non-existent table id
            filter_bar._target_table_id = "this-table-does-not-exist"
            assert filter_bar.target_table is None

            filter_bar.show()
            await _pilot.pause()

            # Filter bar must remain hidden
            assert not filter_bar.is_visible

            # A warning notification must have been posted
            assert any("Nothing to filter" in msg and sev == "warning" for msg, sev in captured), (
                f"Expected 'Nothing to filter' warning, got: {captured}"
            )

    @pytest.mark.asyncio
    async def test_show_with_valid_target_does_not_notify(self) -> None:
        """show() with a valid target must not emit the 'Nothing to filter' warning."""
        from helpers import capture_notifications, make_app, make_tracks

        from ytmusic_tui.views.filter_bar import FilterBar
        from ytmusic_tui.views.queue import QueueView

        app = make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            captured = capture_notifications(app)

            tracks = make_tracks(3)
            app.queue_manager.set_playlist(tracks)
            app.action_switch_view("queue")
            await _pilot.pause()

            view = app.query_one(QueueView)
            view.refresh_queue()
            await _pilot.pause()

            filter_bar = view.query_one("#queue-filter", FilterBar)
            filter_bar.show()
            await _pilot.pause()

            # The filter bar must be visible and no "Nothing to filter" warning
            assert filter_bar.is_visible
            assert not any("Nothing to filter" in msg for msg, _sev in captured)
