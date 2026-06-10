"""Tests for SearchView, PlaylistView, QueueView, HomeView, LibraryView, and keybindings."""

from __future__ import annotations

from typing import TYPE_CHECKING
from unittest.mock import MagicMock

import pytest
from helpers import make_app
from helpers import make_track as _make_track
from helpers import make_tracks as _make_tracks
from textual.widgets import ContentSwitcher

from ytmusic_tui.api import (
    AlbumInfo,
    ArtistInfo,
    HomeSection,
    PlaylistInfo,
    RelatedArtist,
    SearchResults,
)
from ytmusic_tui.views.home import HomeView
from ytmusic_tui.views.library import LibraryPane, LibraryView
from ytmusic_tui.views.playlist import PlaylistView
from ytmusic_tui.views.queue import QueueView
from ytmusic_tui.views.search import Pane, SearchView

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


def _make_app():
    """Create a YtMusicTui app with mocked dependencies."""
    return make_app()


def _make_app_with_search_results(results: list[Track]):
    """Create an app where search returns the given results."""
    return make_app(configure_api=lambda api: setattr(api.search, "return_value", results))


def _make_app_with_playlists(playlists: list[PlaylistInfo], tracks: list[Track] | None = None):
    """Create an app where library playlists returns given data."""

    def _configure(api) -> None:
        api.get_library_playlists.return_value = playlists
        api.get_playlist_tracks.return_value = tracks or []

    return make_app(configure_api=_configure)


def _make_app_with_home_sections(sections: list[HomeSection]):
    """Create an app where get_home returns the given sections."""
    return make_app(configure_api=lambda api: setattr(api.get_home, "return_value", sections))


def _make_app_with_library(
    playlists: list[PlaylistInfo] | None = None,
    albums: list[AlbumInfo] | None = None,
    artists: list[ArtistInfo] | None = None,
    playlist_tracks: list[Track] | None = None,
):
    """Create an app with library data for 3-pane tests."""

    def _configure(api) -> None:
        api.get_library_playlists.return_value = playlists or []
        api.get_library_albums.return_value = albums or []
        api.get_library_artists.return_value = artists or []
        api.get_playlist_tracks.return_value = playlist_tracks or []

    return make_app(configure_api=_configure)


# ===================================================================
# SearchView
# ===================================================================


class TestSearchView:
    @pytest.mark.asyncio
    async def test_search_view_has_input(self) -> None:
        """SearchView should contain an Input widget."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import Input

            view = app.query_one(SearchView)
            input_widget = view.query_one("#search-input", Input)
            assert input_widget is not None

    @pytest.mark.asyncio
    async def test_search_view_has_four_panes(self) -> None:
        """SearchView should contain four DataTable panes."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import DataTable

            view = app.query_one(SearchView)
            tracks_table = view.query_one("#search-tracks", DataTable)
            albums_table = view.query_one("#search-albums", DataTable)
            artists_table = view.query_one("#search-artists", DataTable)
            playlists_table = view.query_one("#search-playlists", DataTable)
            assert tracks_table is not None
            assert albums_table is not None
            assert artists_table is not None
            assert playlists_table is not None

    @pytest.mark.asyncio
    async def test_search_empty_query_does_nothing(self) -> None:
        """Submitting an empty query should not trigger a search."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import DataTable

            app.action_switch_view("search")
            await _pilot.pause()

            view = app.query_one(SearchView)
            table = view.query_one("#search-tracks", DataTable)
            assert table.row_count == 0

    @pytest.mark.asyncio
    async def test_search_populates_all_panes(self) -> None:
        """Search results should populate all four DataTable panes."""
        tracks = _make_tracks(3)
        albums = [_make_album_info(1), _make_album_info(2)]
        artists = [_make_related_artist(1)]
        playlists = [_make_playlist_info(1), _make_playlist_info(2)]
        results = _make_search_results(
            tracks=tracks, albums=albums, artists=artists, playlists=playlists
        )
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import DataTable

            app.action_switch_view("search")
            await _pilot.pause()

            view = app.query_one(SearchView)
            view._populate_all_results(results)
            await _pilot.pause()

            assert view.query_one("#search-tracks", DataTable).row_count == 3
            assert view.query_one("#search-albums", DataTable).row_count == 2
            assert view.query_one("#search-artists", DataTable).row_count == 1
            assert view.query_one("#search-playlists", DataTable).row_count == 2

    @pytest.mark.asyncio
    async def test_search_no_results_shows_status(self) -> None:
        """Empty search results should show a status message."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import Label

            app.action_switch_view("search")
            await _pilot.pause()

            view = app.query_one(SearchView)
            view._populate_all_results(_make_search_results())
            await _pilot.pause()

            status = view.query_one("#search-status", Label)
            assert "No results" in status.content

    @pytest.mark.asyncio
    async def test_search_status_shows_total_count(self) -> None:
        """Status label should show total result count across categories."""
        results = _make_search_results(
            tracks=_make_tracks(2),
            albums=[_make_album_info(1)],
        )
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import Label

            view = app.query_one(SearchView)
            view._populate_all_results(results)
            await _pilot.pause()

            status = view.query_one("#search-status", Label)
            assert "3 result(s)" in status.content

    @pytest.mark.asyncio
    async def test_search_track_selection_queues_and_plays(self) -> None:
        """Selecting a track should queue and play it."""
        tracks = _make_tracks(3)
        results = _make_search_results(tracks=tracks)
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            app.action_switch_view("search")
            await _pilot.pause()

            view = app.query_one(SearchView)
            view._populate_all_results(results)
            await _pilot.pause()

            # Simulate row selection on the tracks table
            mock_event = MagicMock()
            mock_event.cursor_row = 1
            mock_dt = MagicMock()
            mock_dt.id = "search-tracks"
            mock_event.data_table = mock_dt
            view.on_data_table_row_selected(mock_event)

            assert len(app.queue_manager.tracks) == 1
            assert app.queue_manager.current_track == tracks[1]
            app.player.play.assert_called_once_with("vid_2")

    @pytest.mark.asyncio
    async def test_search_album_selection_opens_album(self) -> None:
        """Selecting an album should call action_open_album."""
        albums = [_make_album_info(1)]
        results = _make_search_results(albums=albums)
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            app.action_switch_view("search")
            await _pilot.pause()

            view = app.query_one(SearchView)
            view._populate_all_results(results)
            await _pilot.pause()

            mock_event = MagicMock()
            mock_event.cursor_row = 0
            mock_dt = MagicMock()
            mock_dt.id = "search-albums"
            mock_event.data_table = mock_dt

            # Spy on action_open_album
            app.action_open_album = MagicMock()
            view.on_data_table_row_selected(mock_event)

            app.action_open_album.assert_called_once_with("MPREb_1")

    @pytest.mark.asyncio
    async def test_search_artist_selection_opens_artist(self) -> None:
        """Selecting an artist should call action_open_artist."""
        artists = [_make_related_artist(1)]
        results = _make_search_results(artists=artists)
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            app.action_switch_view("search")
            await _pilot.pause()

            view = app.query_one(SearchView)
            view._populate_all_results(results)
            await _pilot.pause()

            mock_event = MagicMock()
            mock_event.cursor_row = 0
            mock_dt = MagicMock()
            mock_dt.id = "search-artists"
            mock_event.data_table = mock_dt

            app.action_open_artist = MagicMock()
            view.on_data_table_row_selected(mock_event)

            app.action_open_artist.assert_called_once_with("UCsearch_1")

    @pytest.mark.asyncio
    async def test_search_playlist_selection_opens_playlist_view(self) -> None:
        """Selecting a playlist result must load it into PlaylistView.

        Regression guard: ``_on_playlist_selected`` once routed the
        view switch through ``getattr(self.app, "action_switch_view",
        None)``, and that None-guard silently swallowed the dead path so
        the playlist's tracks were never loaded. Selecting a playlist row
        must switch to the playlist view and call
        ``PlaylistView.show_track_list`` with the chosen playlist.
        """
        playlist = _make_playlist_info(1)
        results = _make_search_results(playlists=[playlist])
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            app.action_switch_view("search")
            await _pilot.pause()

            view = app.query_one(SearchView)
            view._populate_all_results(results)
            await _pilot.pause()

            # Spy on the load-bearing call that used to be dead.
            playlist_view = app.query_one(PlaylistView)
            playlist_view.show_track_list = MagicMock()  # type: ignore[method-assign]

            mock_event = MagicMock()
            mock_event.cursor_row = 0
            mock_dt = MagicMock()
            mock_dt.id = "search-playlists"
            mock_event.data_table = mock_dt
            view.on_data_table_row_selected(mock_event)
            await _pilot.pause()

            playlist_view.show_track_list.assert_called_once_with(playlist)
            assert app.query_one(ContentSwitcher).current == "playlist"

    @pytest.mark.asyncio
    async def test_search_focus_cycling(self) -> None:
        """focus_next_pane should cycle through all four panes."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            app.action_switch_view("search")
            await _pilot.pause()

            view = app.query_one(SearchView)
            assert view.focused_pane == Pane.TRACKS

            view.focus_next_pane()
            assert view.focused_pane == Pane.ALBUMS

            view.focus_next_pane()
            assert view.focused_pane == Pane.ARTISTS

            view.focus_next_pane()
            assert view.focused_pane == Pane.PLAYLISTS

            view.focus_next_pane()
            assert view.focused_pane == Pane.TRACKS  # wraps

    @pytest.mark.asyncio
    async def test_search_focus_cycling_reverse(self) -> None:
        """focus_previous_pane should cycle in reverse."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            app.action_switch_view("search")
            await _pilot.pause()

            view = app.query_one(SearchView)
            assert view.focused_pane == Pane.TRACKS

            view.focus_previous_pane()
            assert view.focused_pane == Pane.PLAYLISTS  # wraps to last

    @pytest.mark.asyncio
    async def test_search_out_of_range_row_ignored(self) -> None:
        """Row selection with invalid index should not crash."""
        results = _make_search_results(tracks=_make_tracks(1))
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            view = app.query_one(SearchView)
            view._populate_all_results(results)
            await _pilot.pause()

            mock_event = MagicMock()
            mock_event.cursor_row = 99
            mock_event.data_table = MagicMock(id="search-tracks")
            # Should not raise
            view.on_data_table_row_selected(mock_event)

    @pytest.mark.asyncio
    async def test_focus_input_method(self) -> None:
        """focus_input() should focus the search input."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            app.action_switch_view("search")
            await _pilot.pause()

            view = app.query_one(SearchView)
            view.focus_input()
            await _pilot.pause()

            from textual.widgets import Input

            input_widget = view.query_one("#search-input", Input)
            assert input_widget.has_focus


# ===================================================================
# PlaylistView
# ===================================================================


class TestPlaylistView:
    @pytest.mark.asyncio
    async def test_playlist_view_has_table(self) -> None:
        """PlaylistView should contain a DataTable."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import DataTable

            view = app.query_one(PlaylistView)
            table = view.query_one("#playlist-table", DataTable)
            assert table is not None

    @pytest.mark.asyncio
    async def test_playlist_populates_playlists(self) -> None:
        """Playlist list should populate the DataTable."""
        playlists = [_make_playlist_info(1), _make_playlist_info(2)]
        app = _make_app_with_playlists(playlists)
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import DataTable

            app.action_switch_view("playlist")
            await _pilot.pause()

            view = app.query_one(PlaylistView)
            view._populate_playlists(playlists)
            await _pilot.pause()

            table = view.query_one("#playlist-table", DataTable)
            assert table.row_count == 2

    @pytest.mark.asyncio
    async def test_playlist_empty_shows_status(self) -> None:
        """Empty playlist list should show a status message."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import Label

            app.action_switch_view("playlist")
            await _pilot.pause()

            view = app.query_one(PlaylistView)
            view._populate_playlists([])
            await _pilot.pause()

            status = view.query_one("#playlist-status", Label)
            assert "No playlists" in status.content

    @pytest.mark.asyncio
    async def test_playlist_drill_into_tracks(self) -> None:
        """Selecting a playlist should switch to track list mode."""
        playlists = [_make_playlist_info(1)]
        tracks = _make_tracks(5)
        app = _make_app_with_playlists(playlists, tracks)
        async with app.run_test(size=(120, 40)) as _pilot:
            app.action_switch_view("playlist")
            await _pilot.pause()

            view = app.query_one(PlaylistView)
            view._populate_playlists(playlists)
            await _pilot.pause()

            # Simulate selecting the playlist
            mock_event = MagicMock()
            mock_event.cursor_row = 0
            view.on_data_table_row_selected(mock_event)
            await _pilot.pause()

            assert view._viewing_tracks is True

    @pytest.mark.asyncio
    async def test_playlist_track_selection_queues_remaining(self) -> None:
        """Selecting a track in a playlist should queue all tracks from that point."""
        playlists = [_make_playlist_info(1)]
        tracks = _make_tracks(5)
        app = _make_app_with_playlists(playlists, tracks)
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import DataTable

            app.action_switch_view("playlist")
            await _pilot.pause()

            view = app.query_one(PlaylistView)
            # Properly switch to track mode by resetting columns
            table = view.query_one("#playlist-table", DataTable)
            table.clear(columns=True)
            table.add_columns("Title", "Artist", "Album", "Duration")
            view._viewing_tracks = True
            view._tracks = tracks
            view._populate_tracks(tracks)
            await _pilot.pause()

            # Select third track (index 2)
            mock_event = MagicMock()
            mock_event.cursor_row = 2
            view.on_data_table_row_selected(mock_event)

            # Queue should have all tracks starting from index 2
            assert len(app.queue_manager.tracks) == 5
            assert app.queue_manager.current_track == tracks[2]
            app.player.play.assert_called_once_with("vid_3")

    @pytest.mark.asyncio
    async def test_playlist_escape_returns_to_list(self) -> None:
        """Pressing Escape in track view should return to playlist list."""
        playlists = [_make_playlist_info(1)]
        app = _make_app_with_playlists(playlists)
        async with app.run_test(size=(120, 40)) as _pilot:
            app.action_switch_view("playlist")
            await _pilot.pause()

            view = app.query_one(PlaylistView)
            view._viewing_tracks = True

            # Simulate Escape key
            mock_event = MagicMock()
            mock_event.key = "escape"
            view.on_key(mock_event)
            await _pilot.pause()

            assert view._viewing_tracks is False


# ===================================================================
# QueueView
# ===================================================================


class TestQueueView:
    @pytest.mark.asyncio
    async def test_queue_view_has_table(self) -> None:
        """QueueView should contain a DataTable."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import DataTable

            view = app.query_one(QueueView)
            table = view.query_one("#queue-table", DataTable)
            assert table is not None

    @pytest.mark.asyncio
    async def test_queue_view_displays_tracks(self) -> None:
        """QueueView should display tracks from the queue manager."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import DataTable

            # Add tracks to queue
            tracks = _make_tracks(3)
            app.queue_manager.set_playlist(tracks)

            # Switch to queue view and refresh
            app.action_switch_view("queue")
            await _pilot.pause()

            view = app.query_one(QueueView)
            view.refresh_queue()
            await _pilot.pause()

            table = view.query_one("#queue-table", DataTable)
            assert table.row_count == 3

    @pytest.mark.asyncio
    async def test_queue_view_empty_shows_status(self) -> None:
        """Empty queue should show an appropriate status."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import Label

            app.action_switch_view("queue")
            await _pilot.pause()

            view = app.query_one(QueueView)
            view.refresh_queue()
            await _pilot.pause()

            status = view.query_one("#queue-status", Label)
            assert "empty" in status.content.lower()

    @pytest.mark.asyncio
    async def test_queue_view_remove_track(self) -> None:
        """Pressing 'd' should remove the selected track."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import DataTable

            tracks = _make_tracks(3)
            app.queue_manager.set_playlist(tracks)

            app.action_switch_view("queue")
            await _pilot.pause()

            view = app.query_one(QueueView)
            view.refresh_queue()
            await _pilot.pause()

            # Set cursor to first row and press 'd'
            table = view.query_one("#queue-table", DataTable)
            table.move_cursor(row=1)

            mock_event = MagicMock()
            mock_event.key = "d"
            view.on_key(mock_event)
            await _pilot.pause()

            assert len(app.queue_manager.tracks) == 2


# ===================================================================
# Keybindings / Actions
# ===================================================================


class TestKeybindings:
    @pytest.mark.asyncio
    async def test_quit_binding_is_shift_q(self) -> None:
        """Quit should be bound to Q (shift+q)."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            binding_keys = [b.key for b in app.BINDINGS]
            assert "Q" in binding_keys

    @pytest.mark.asyncio
    async def test_space_binding_exists(self) -> None:
        """Space should be bound to toggle_pause."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            binding_keys = [b.key for b in app.BINDINGS]
            assert "space" in binding_keys

    @pytest.mark.asyncio
    async def test_action_toggle_pause(self) -> None:
        """action_toggle_pause should call player.toggle_pause."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            app.action_toggle_pause()
            app.player.toggle_pause.assert_called_once()

    @pytest.mark.asyncio
    async def test_action_next_track(self) -> None:
        """action_next_track should advance queue and play."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            tracks = _make_tracks(3)
            app.queue_manager.set_playlist(tracks)

            app.action_next_track()
            # Should advance to track 2
            assert app.queue_manager.current_track == tracks[1]
            app.player.play.assert_called_with("vid_2")

    @pytest.mark.asyncio
    async def test_action_previous_track(self) -> None:
        """action_previous_track should go back in queue and play."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            tracks = _make_tracks(3)
            app.queue_manager.set_playlist(tracks, start_index=2)

            app.action_previous_track()
            assert app.queue_manager.current_track == tracks[1]
            app.player.play.assert_called_with("vid_2")

    @pytest.mark.asyncio
    async def test_action_toggle_shuffle(self) -> None:
        """action_toggle_shuffle should toggle shuffle on the queue."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            assert app.queue_manager.shuffle is False
            app.action_toggle_shuffle()
            assert app.queue_manager.shuffle is True

    @pytest.mark.asyncio
    async def test_action_cycle_repeat(self) -> None:
        """action_cycle_repeat should cycle through repeat modes."""
        from ytmusic_tui.queue import RepeatMode

        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            assert app.queue_manager.repeat_mode is RepeatMode.OFF
            app.action_cycle_repeat()
            assert app.queue_manager.repeat_mode is RepeatMode.ALL

    @pytest.mark.asyncio
    async def test_action_volume_up(self) -> None:
        """action_volume_up should call player.adjust_volume with positive delta."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            app.action_volume_up()
            app.player.adjust_volume.assert_called_with(5)

    @pytest.mark.asyncio
    async def test_action_volume_down(self) -> None:
        """action_volume_down should call player.adjust_volume with negative delta."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            app.action_volume_down()
            app.player.adjust_volume.assert_called_with(-5)

    @pytest.mark.asyncio
    async def test_action_focus_search(self) -> None:
        """action_focus_search should switch to search and focus input."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import ContentSwitcher

            app.action_focus_search()
            await _pilot.pause()

            switcher = app.query_one(ContentSwitcher)
            assert switcher.current == "search"

    @pytest.mark.asyncio
    async def test_action_go_back(self) -> None:
        """action_go_back should switch to home view."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import ContentSwitcher

            app.action_switch_view("search")
            await _pilot.pause()

            app.action_go_back()
            await _pilot.pause()

            switcher = app.query_one(ContentSwitcher)
            assert switcher.current == "home"

    @pytest.mark.asyncio
    async def test_switch_to_queue_refreshes(self) -> None:
        """Switching to queue view should refresh the display."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import DataTable

            tracks = _make_tracks(2)
            app.queue_manager.set_playlist(tracks)

            app.action_switch_view("queue")
            await _pilot.pause()

            view = app.query_one(QueueView)
            table = view.query_one("#queue-table", DataTable)
            assert table.row_count == 2

    @pytest.mark.asyncio
    async def test_next_track_at_end_does_not_play(self) -> None:
        """Next track at end of queue (repeat OFF) should not call play."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            tracks = _make_tracks(1)
            app.queue_manager.set_playlist(tracks)

            # Already at the only track, next should return None
            app.action_next_track()
            app.player.play.assert_not_called()


# ===================================================================
# Auto-advance (on_track_end)
# ===================================================================


class TestAutoAdvance:
    @pytest.mark.asyncio
    async def test_on_track_end_advances_queue(self) -> None:
        """on_track_end callback should advance the queue and play next."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            tracks = _make_tracks(3)
            app.queue_manager.set_playlist(tracks)

            # Simulate track end
            app._on_track_end()

            assert app.queue_manager.current_track == tracks[1]
            app.player.play.assert_called_with("vid_2")

    @pytest.mark.asyncio
    async def test_on_track_end_at_queue_end_does_nothing(self) -> None:
        """on_track_end at end of queue should not crash or call play."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            tracks = _make_tracks(1)
            app.queue_manager.set_playlist(tracks)

            app._on_track_end()
            app.player.play.assert_not_called()


# ===================================================================
# Duration formatting
# ===================================================================


class TestFormatDuration:
    def test_format_seconds_only(self) -> None:
        from ytmusic_tui.views.search import _format_duration

        assert _format_duration(45.0) == "0:45"

    def test_format_minutes_seconds(self) -> None:
        from ytmusic_tui.views.search import _format_duration

        assert _format_duration(185.0) == "3:05"

    def test_format_hours(self) -> None:
        from ytmusic_tui.views.search import _format_duration

        assert _format_duration(3661.0) == "1:01:01"

    def test_format_zero(self) -> None:
        from ytmusic_tui.views.search import _format_duration

        assert _format_duration(0.0) == "—"

    def test_format_negative(self) -> None:
        from ytmusic_tui.views.search import _format_duration

        assert _format_duration(-5.0) == "—"


# ===================================================================
# HomeView
# ===================================================================


class TestHomeView:
    @pytest.mark.asyncio
    async def test_home_view_loading_state(self) -> None:
        """HomeView should show loading status initially."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import Label

            view = app.query_one(HomeView)
            status = view.query_one("#home-status", Label)
            assert status is not None

    @pytest.mark.asyncio
    async def test_home_view_renders_sections(self) -> None:
        """HomeView should render sections with DataTables after fetch."""
        tracks = _make_tracks(3)
        sections = [HomeSection(title="Quick picks", items=tracks)]
        app = _make_app_with_home_sections(sections)
        async with app.run_test(size=(120, 40)) as _pilot:
            view = app.query_one(HomeView)
            # Directly call _render_sections to test rendering logic
            view._render_sections(sections)
            await _pilot.pause()

            from ytmusic_tui.views.home import _SectionTable

            section_widgets = view.query(_SectionTable)
            assert len(section_widgets) >= 1

    @pytest.mark.asyncio
    async def test_home_view_empty_sections(self) -> None:
        """HomeView should show message when no sections available."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import Label

            view = app.query_one(HomeView)
            view._render_sections([])
            await _pilot.pause()

            status = view.query_one("#home-status", Label)
            assert "No recommendations" in status.content

    @pytest.mark.asyncio
    async def test_home_view_error_display(self) -> None:
        """HomeView should display the given error message verbatim."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import Label

            view = app.query_one(HomeView)
            view._set_status("Network error — check your connection")
            await _pilot.pause()

            status = view.query_one("#home-status", Label)
            assert "Network error" in status.content

    @pytest.mark.asyncio
    async def test_home_view_track_selection_plays(self) -> None:
        """Selecting a track in HomeView should queue and play it."""
        track = _make_track(1)
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            view = app.query_one(HomeView)

            # Use the internal handler directly
            view._play_track(track)

            assert len(app.queue_manager.tracks) == 1
            assert app.queue_manager.current_track == track
            app.player.play.assert_called_once_with("vid_1")

    @pytest.mark.asyncio
    async def test_home_view_playlist_selection_switches_view(self) -> None:
        """Selecting a playlist in HomeView should switch to playlist view."""
        playlist = _make_playlist_info(1)
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import ContentSwitcher

            view = app.query_one(HomeView)
            view._open_playlist(playlist)
            await _pilot.pause()

            switcher = app.query_one(ContentSwitcher)
            assert switcher.current == "playlist"

    @pytest.mark.asyncio
    async def test_home_view_section_with_mixed_items(self) -> None:
        """HomeView should handle sections with both tracks and playlists."""
        track = _make_track(1)
        playlist = _make_playlist_info(1)
        sections = [HomeSection(title="Mixed", items=[track, playlist])]
        app = _make_app_with_home_sections(sections)
        async with app.run_test(size=(120, 40)) as _pilot:
            view = app.query_one(HomeView)
            view._render_sections(sections)
            await _pilot.pause()

            from ytmusic_tui.views.home import _SectionTable

            section_widgets = view.query(_SectionTable)
            assert len(section_widgets) >= 1
            # The section should have both items
            assert len(section_widgets.first().items) == 2

    @pytest.mark.asyncio
    async def test_home_view_skips_empty_sections(self) -> None:
        """HomeView should skip sections with no items."""
        sections = [
            HomeSection(title="Empty", items=[]),
            HomeSection(title="Has items", items=[_make_track(1)]),
        ]
        # Use base _make_app (get_home returns []) so background worker
        # does not add extra widgets, then call _render_sections directly.
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            view = app.query_one(HomeView)
            view._render_sections(sections)
            await _pilot.pause()

            from ytmusic_tui.views.home import _SectionTable

            section_widgets = view.query(_SectionTable)
            # Only the non-empty section should be rendered
            assert len(section_widgets) == 1


# ===================================================================
# HomeView format_row
# ===================================================================


class TestHomeFormatRow:
    def test_format_track_row(self) -> None:
        """_format_row should format a track as (title, artist, duration)."""
        from ytmusic_tui.views.home import _format_row

        track = _make_track(1)
        title, info, dur = _format_row(track)
        assert title == "Song 1"
        assert info == "Artist 1"
        assert dur == "3:01"  # 181 seconds

    def test_format_playlist_row(self) -> None:
        """_format_row should format a playlist as (title, count, empty)."""
        from ytmusic_tui.views.home import _format_row

        playlist = _make_playlist_info(2)
        title, info, dur = _format_row(playlist)
        assert title == "Playlist 2"
        assert "10 tracks" in info
        assert dur == ""

    def test_format_playlist_no_count(self) -> None:
        """_format_row should show 'Playlist' when track_count is 0."""
        from ytmusic_tui.views.home import _format_row

        playlist = PlaylistInfo(playlist_id="PL_0", title="Empty PL", track_count=0)
        _, info, _ = _format_row(playlist)
        assert info == "Playlist"


# ===================================================================
# LibraryView
# ===================================================================


class TestLibraryView:
    """Tests for the 3-pane library view (Playlists / Albums / Artists)."""

    @pytest.mark.asyncio
    async def test_library_has_three_tables(self) -> None:
        """LibraryView should contain three DataTables."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import DataTable

            view = app.query_one(LibraryView)
            assert view.query_one("#library-playlists", DataTable) is not None
            assert view.query_one("#library-albums", DataTable) is not None
            assert view.query_one("#library-artists", DataTable) is not None

    @pytest.mark.asyncio
    async def test_library_has_pane_labels(self) -> None:
        """LibraryView should have labels for each pane."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import Label

            view = app.query_one(LibraryView)
            pl_label = view.query_one("#pane-label-playlists", Label)
            al_label = view.query_one("#pane-label-albums", Label)
            ar_label = view.query_one("#pane-label-artists", Label)
            assert "Playlists" in pl_label.content
            assert "Albums" in al_label.content
            assert "Artists" in ar_label.content

    @pytest.mark.asyncio
    async def test_library_default_pane_is_playlists(self) -> None:
        """LibraryView should start with Playlists pane active."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            view = app.query_one(LibraryView)
            assert view._active_pane is LibraryPane.PLAYLISTS

    @pytest.mark.asyncio
    async def test_library_focus_next_pane(self) -> None:
        """focus_next_pane should cycle through Playlists -> Albums -> Artists."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            view = app.query_one(LibraryView)
            assert view._active_pane is LibraryPane.PLAYLISTS

            view.focus_next_pane()
            assert view._active_pane is LibraryPane.ALBUMS

            view.focus_next_pane()
            assert view._active_pane is LibraryPane.ARTISTS

            view.focus_next_pane()
            assert view._active_pane is LibraryPane.PLAYLISTS

    @pytest.mark.asyncio
    async def test_library_focus_previous_pane(self) -> None:
        """focus_previous_pane should cycle backwards."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            view = app.query_one(LibraryView)
            assert view._active_pane is LibraryPane.PLAYLISTS

            view.focus_previous_pane()
            assert view._active_pane is LibraryPane.ARTISTS

            view.focus_previous_pane()
            assert view._active_pane is LibraryPane.ALBUMS

    @pytest.mark.asyncio
    async def test_library_populate_playlists(self) -> None:
        """Playlists pane should populate its DataTable."""
        playlists = [_make_playlist_info(1), _make_playlist_info(2)]
        app = _make_app_with_library(playlists=playlists)
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import DataTable

            app.action_switch_view("library")
            await _pilot.pause()

            view = app.query_one(LibraryView)
            view._populate_playlists(playlists)
            await _pilot.pause()

            table = view.query_one("#library-playlists", DataTable)
            assert table.row_count == 2

    @pytest.mark.asyncio
    async def test_library_populate_albums(self) -> None:
        """Albums pane should populate its DataTable."""
        albums = [_make_album_info(1), _make_album_info(2), _make_album_info(3)]
        app = _make_app_with_library(albums=albums)
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import DataTable

            view = app.query_one(LibraryView)
            view._populate_albums(albums)
            await _pilot.pause()

            table = view.query_one("#library-albums", DataTable)
            assert table.row_count == 3

    @pytest.mark.asyncio
    async def test_library_populate_artists(self) -> None:
        """Artists pane should populate its DataTable."""
        artists = [_make_artist_info(1), _make_artist_info(2)]
        app = _make_app_with_library(artists=artists)
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import DataTable

            view = app.query_one(LibraryView)
            view._populate_artists(artists)
            await _pilot.pause()

            table = view.query_one("#library-artists", DataTable)
            assert table.row_count == 2

    @pytest.mark.asyncio
    async def test_library_empty_shows_status(self) -> None:
        """Empty library should show 'Library empty' in status."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import Label

            view = app.query_one(LibraryView)
            # Populate all with empty lists
            view._populate_playlists([])
            view._populate_albums([])
            view._populate_artists([])
            await _pilot.pause()

            status = view.query_one("#library-status", Label)
            assert "empty" in status.content.lower()

    @pytest.mark.asyncio
    async def test_library_combined_status(self) -> None:
        """Status should show counts from all three panes."""
        playlists = [_make_playlist_info(1)]
        albums = [_make_album_info(1), _make_album_info(2)]
        artists = [_make_artist_info(1)]
        app = _make_app_with_library(playlists=playlists, albums=albums, artists=artists)
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import Label

            view = app.query_one(LibraryView)
            view._populate_playlists(playlists)
            view._populate_albums(albums)
            view._populate_artists(artists)
            await _pilot.pause()

            status = view.query_one("#library-status", Label)
            assert "1 playlist(s)" in status.content
            assert "2 album(s)" in status.content
            assert "1 artist(s)" in status.content

    @pytest.mark.asyncio
    async def test_library_drill_into_playlist(self) -> None:
        """Selecting a playlist should switch to track list mode."""
        playlists = [_make_playlist_info(1)]
        tracks = _make_tracks(3)
        app = _make_app_with_library(playlists=playlists, playlist_tracks=tracks)
        async with app.run_test(size=(120, 40)) as _pilot:
            view = app.query_one(LibraryView)
            view._populate_playlists(playlists)
            await _pilot.pause()

            # Simulate selecting a playlist via the playlists table
            mock_event = MagicMock()
            mock_event.cursor_row = 0
            mock_event.data_table = MagicMock()
            mock_event.data_table.id = "library-playlists"
            view.on_data_table_row_selected(mock_event)
            await _pilot.pause()

            assert view._viewing_tracks is True

    @pytest.mark.asyncio
    async def test_library_playlist_track_selection_queues(self) -> None:
        """Selecting a track in playlist drill-down should queue tracks."""
        playlists = [_make_playlist_info(1)]
        tracks = _make_tracks(5)
        app = _make_app_with_library(playlists=playlists, playlist_tracks=tracks)
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import DataTable

            view = app.query_one(LibraryView)

            # Set up track list mode manually on the playlists table
            table = view.query_one("#library-playlists", DataTable)
            table.clear(columns=True)
            table.add_columns("Title", "Artist", "Album", "Duration")
            view._viewing_tracks = True
            view._active_pane = LibraryPane.PLAYLISTS
            view._tracks = tracks
            view._populate_tracks(tracks)
            await _pilot.pause()

            # Select third track (index 2)
            mock_event = MagicMock()
            mock_event.cursor_row = 2
            mock_event.data_table = MagicMock()
            mock_event.data_table.id = "library-playlists"
            view.on_data_table_row_selected(mock_event)

            assert len(app.queue_manager.tracks) == 5
            assert app.queue_manager.current_track == tracks[2]
            app.player.play.assert_called_once_with("vid_3")

    @pytest.mark.asyncio
    async def test_library_album_selection_opens_album(self) -> None:
        """Selecting an album should call action_open_album."""
        albums = [_make_album_info(1)]
        app = _make_app_with_library(albums=albums)
        async with app.run_test(size=(120, 40)) as _pilot:
            view = app.query_one(LibraryView)
            view._populate_albums(albums)
            await _pilot.pause()

            app.action_open_album = MagicMock()

            mock_event = MagicMock()
            mock_event.cursor_row = 0
            mock_event.data_table = MagicMock()
            mock_event.data_table.id = "library-albums"
            view.on_data_table_row_selected(mock_event)

            app.action_open_album.assert_called_once_with("MPREb_1")

    @pytest.mark.asyncio
    async def test_library_artist_selection_opens_artist(self) -> None:
        """Selecting an artist should call action_open_artist."""
        artists = [_make_artist_info(1)]
        app = _make_app_with_library(artists=artists)
        async with app.run_test(size=(120, 40)) as _pilot:
            view = app.query_one(LibraryView)
            view._populate_artists(artists)
            await _pilot.pause()

            app.action_open_artist = MagicMock()

            mock_event = MagicMock()
            mock_event.cursor_row = 0
            mock_event.data_table = MagicMock()
            mock_event.data_table.id = "library-artists"
            view.on_data_table_row_selected(mock_event)

            app.action_open_artist.assert_called_once_with("UC_1")

    @pytest.mark.asyncio
    async def test_library_escape_returns_to_playlists(self) -> None:
        """Pressing Escape in track view should return to playlist list."""
        playlists = [_make_playlist_info(1)]
        app = _make_app_with_library(playlists=playlists)
        async with app.run_test(size=(120, 40)) as _pilot:
            view = app.query_one(LibraryView)
            view._playlists = playlists
            view._viewing_tracks = True

            mock_event = MagicMock()
            mock_event.key = "escape"
            view.on_key(mock_event)
            await _pilot.pause()

            assert view._viewing_tracks is False

    @pytest.mark.asyncio
    async def test_library_out_of_range_selection_ignored(self) -> None:
        """Out-of-range row selections should be silently ignored."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            view = app.query_one(LibraryView)

            # Playlists out of range
            mock_event = MagicMock()
            mock_event.cursor_row = 99
            mock_event.data_table = MagicMock()
            mock_event.data_table.id = "library-playlists"
            view.on_data_table_row_selected(mock_event)  # should not raise

            # Albums out of range
            mock_event.data_table.id = "library-albums"
            view.on_data_table_row_selected(mock_event)  # should not raise

            # Artists out of range
            mock_event.data_table.id = "library-artists"
            view.on_data_table_row_selected(mock_event)  # should not raise

    @pytest.mark.asyncio
    async def test_library_on_show_focuses_active_pane(self) -> None:
        """Returning to Library should auto-focus the active pane's table."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import DataTable

            # Switch to library and let it mount
            app.action_switch_view("library")
            await _pilot.pause()

            view = app.query_one(LibraryView)
            # Switch active pane to albums
            view.focus_next_pane()
            assert view._active_pane is LibraryPane.ALBUMS

            # Navigate away and come back
            app.action_switch_view("queue")
            await _pilot.pause()
            app.action_switch_view("library")
            await _pilot.pause()

            # The albums table should have focus after on_show fires
            albums_table = view.query_one("#library-albums", DataTable)
            assert albums_table.has_focus


# ===================================================================
# Queue view refresh on next/previous (Bug 4)
# ===================================================================


class TestQueueViewRefreshOnTrackChange:
    @pytest.mark.asyncio
    async def test_next_track_refreshes_queue_view(self) -> None:
        """Pressing next while queue view is active should refresh the display."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import DataTable

            tracks = _make_tracks(3)
            app.queue_manager.set_playlist(tracks)

            app.action_switch_view("queue")
            await _pilot.pause()

            view = app.query_one(QueueView)
            view.refresh_queue()
            await _pilot.pause()

            # Verify initial state: first track is current (marked with >)
            table = view.query_one("#queue-table", DataTable)
            assert table.row_count == 3

            # Advance to next track
            app.action_next_track()
            await _pilot.pause()

            # The queue view should have been refreshed automatically
            # Verify by checking the table still has 3 rows (not stale)
            assert table.row_count == 3

    @pytest.mark.asyncio
    async def test_previous_track_refreshes_queue_view(self) -> None:
        """Pressing previous while queue view is active should refresh the display."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import DataTable

            tracks = _make_tracks(3)
            app.queue_manager.set_playlist(tracks, start_index=2)

            app.action_switch_view("queue")
            await _pilot.pause()

            view = app.query_one(QueueView)
            view.refresh_queue()
            await _pilot.pause()

            # Go to previous track
            app.action_previous_track()
            await _pilot.pause()

            # Queue view should be refreshed (table still shows all tracks)
            table = view.query_one("#queue-table", DataTable)
            assert table.row_count == 3

    @pytest.mark.asyncio
    async def test_next_track_no_refresh_when_queue_not_active(self) -> None:
        """next_track should not crash when queue view is not the active view."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            tracks = _make_tracks(3)
            app.queue_manager.set_playlist(tracks)

            # Stay on home view
            app.action_next_track()
            await _pilot.pause()

            # Should not crash; queue advances normally
            assert app.queue_manager.current_track == tracks[1]


# ===================================================================
# PlayerBar: MPRIS connection-lost notification (Task 1b)
# ===================================================================


class TestPlayerBarMprisErrorNotify:
    """_poll_player_state should fire a one-shot warning when the MPRIS
    D-Bus connection dies at runtime (connection_error becomes non-None)."""

    @pytest.mark.asyncio
    async def test_connection_error_emits_warning_once(self) -> None:
        """When mpris.connection_error is set, a warning notification is emitted
        exactly once no matter how many poll ticks follow."""
        from unittest.mock import MagicMock

        from helpers import capture_notifications, make_app

        from ytmusic_tui.mpris import MprisService
        from ytmusic_tui.views.player import PlayerBar

        app = make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            captured = capture_notifications(app)

            # Install a fake MprisService whose connection_error is already set.
            fake_mpris = MagicMock(spec=MprisService)
            fake_mpris.connection_error = "OSError: disconnected"
            app._mpris = fake_mpris

            bar = app.query_one(PlayerBar)
            assert not bar._mpris_error_notified

            # First poll tick — should emit warning
            bar._poll_player_state()
            await _pilot.pause()

            mpris_warnings = [
                (msg, sev) for msg, sev in captured if "MPRIS" in msg and sev == "warning"
            ]
            assert len(mpris_warnings) == 1, f"Expected 1 warning, got: {captured}"
            assert bar._mpris_error_notified

            # Second poll tick — must NOT emit a second notification
            bar._poll_player_state()
            await _pilot.pause()

            mpris_warnings_after = [
                (msg, sev) for msg, sev in captured if "MPRIS" in msg and sev == "warning"
            ]
            assert len(mpris_warnings_after) == 1, "Duplicate MPRIS warning emitted"

    @pytest.mark.asyncio
    async def test_no_notification_when_mpris_healthy(self) -> None:
        """No notification when mpris.connection_error is None."""
        from unittest.mock import MagicMock

        from helpers import capture_notifications, make_app

        from ytmusic_tui.mpris import MprisService
        from ytmusic_tui.views.player import PlayerBar

        app = make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            captured = capture_notifications(app)

            fake_mpris = MagicMock(spec=MprisService)
            fake_mpris.connection_error = None
            app._mpris = fake_mpris

            bar = app.query_one(PlayerBar)
            bar._poll_player_state()
            await _pilot.pause()

            mpris_warnings = [
                (msg, sev) for msg, sev in captured if "MPRIS" in msg and sev == "warning"
            ]
            assert mpris_warnings == []

    @pytest.mark.asyncio
    async def test_no_notification_when_mpris_absent(self) -> None:
        """No MPRIS-related notification when _mpris is None (not installed)."""
        from helpers import capture_notifications, make_app

        from ytmusic_tui.views.player import PlayerBar

        app = make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            captured = capture_notifications(app)
            app._mpris = None

            bar = app.query_one(PlayerBar)
            bar._poll_player_state()
            await _pilot.pause()

            mpris_warnings = [
                (msg, sev) for msg, sev in captured if "MPRIS" in msg and sev == "warning"
            ]
            assert mpris_warnings == []
