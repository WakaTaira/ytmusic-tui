"""Tests for Sprint 4: Album view, Artist page, and browse navigation."""

from __future__ import annotations

from unittest.mock import MagicMock, patch

import pytest

from ytmusic_tui.api import (
    AlbumInfo,
    ArtistInfo,
    MusicAPI,
    RelatedArtist,
)
from ytmusic_tui.player import PlayerState
from ytmusic_tui.queue import Track

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _make_track(n: int) -> Track:
    """Create a dummy track with a numeric suffix."""
    return Track(
        video_id=f"vid_{n}",
        title=f"Song {n}",
        artist=f"Artist {n}",
        album=f"Album {n}",
        duration_seconds=float(180 + n),
    )


def _make_tracks(count: int) -> list[Track]:
    """Create *count* dummy tracks numbered 1..count."""
    return [_make_track(i) for i in range(1, count + 1)]


def _make_album_info(browse_id: str = "MPREb_test1") -> AlbumInfo:
    """Create a dummy AlbumInfo."""
    return AlbumInfo(
        browse_id=browse_id,
        title="Test Album",
        artist="Test Artist",
        year="2024",
        tracks=_make_tracks(5),
        thumbnail_url="https://example.com/album.jpg",
    )


def _make_artist_info(channel_id: str = "UCtest123") -> ArtistInfo:
    """Create a dummy ArtistInfo."""
    return ArtistInfo(
        channel_id=channel_id,
        name="Test Artist",
        description="A test artist",
        top_songs=_make_tracks(3),
        albums=[
            AlbumInfo(
                browse_id="MPREb_a1",
                title="Album One",
                artist="Test Artist",
                year="2023",
            ),
            AlbumInfo(
                browse_id="MPREb_a2",
                title="Album Two",
                artist="Test Artist",
                year="2024",
            ),
        ],
        related_artists=[
            RelatedArtist(channel_id="UCrel1", name="Similar Artist 1"),
            RelatedArtist(channel_id="UCrel2", name="Similar Artist 2"),
        ],
        thumbnail_url="https://example.com/artist.jpg",
    )


def _make_app():
    """Create a YtMusicTui app with mocked dependencies."""
    with (
        patch("ytmusic_tui.app.MusicAPI") as mock_api_cls,
        patch("ytmusic_tui.app.Player") as mock_player_cls,
    ):
        mock_api = mock_api_cls.return_value
        mock_api.get_home.return_value = []
        mock_api.search.return_value = []
        mock_api.get_library_playlists.return_value = []
        mock_api.get_library_albums.return_value = []
        mock_api.get_library_artists.return_value = []
        mock_api.get_playlist_tracks.return_value = []
        mock_api.get_liked_songs.return_value = []
        mock_api.get_album.return_value = _make_album_info()
        mock_api.get_artist.return_value = _make_artist_info()

        mock_player = mock_player_cls.return_value
        mock_player.get_state.return_value = PlayerState()

        from ytmusic_tui.app import YtMusicTui

        app = YtMusicTui(auth_path="/fake/auth.json")
        return app


# ---------------------------------------------------------------------------
# Mock ytmusicapi response builders
# ---------------------------------------------------------------------------


def _make_raw_album_response(
    *,
    title: str = "Test Album",
    artists: list[dict[str, str]] | None = None,
    year: int | str = 2024,
    tracks: list[dict] | None = None,
    thumbnails: list[dict] | None = None,
) -> dict:
    """Build a realistic ytmusicapi get_album() response."""
    if artists is None:
        artists = [{"name": "Test Artist", "id": "UCtest"}]
    if thumbnails is None:
        thumbnails = [{"url": "https://lh3.google.com/album_lg.jpg", "width": 544, "height": 544}]
    if tracks is None:
        tracks = [
            {
                "videoId": f"alb_t{i}",
                "title": f"Album Track {i}",
                "artists": [{"name": "Test Artist", "id": "UCtest"}],
                "album": {"name": title, "id": "MPREb_test"},
                "duration": f"{3 + i}:00",
                "duration_seconds": (3 + i) * 60,
                "thumbnails": thumbnails,
            }
            for i in range(1, 4)
        ]
    return {
        "title": title,
        "artists": artists,
        "year": year,
        "tracks": tracks,
        "thumbnails": thumbnails,
    }


def _make_raw_artist_response(
    *,
    name: str = "Test Artist",
    description: str = "A great artist",
    songs: dict | None = None,
    albums: dict | None = None,
    related: dict | None = None,
    thumbnails: list[dict] | None = None,
) -> dict:
    """Build a realistic ytmusicapi get_artist() response."""
    if thumbnails is None:
        thumbnails = [{"url": "https://lh3.google.com/artist.jpg", "width": 1440, "height": 1440}]
    if songs is None:
        songs = {
            "results": [
                {
                    "videoId": f"top_{i}",
                    "title": f"Hit Song {i}",
                    "artists": [{"name": name, "id": "UCtest"}],
                    "album": {"name": "Greatest Hits", "id": "MPREhits"},
                    "duration": "3:30",
                    "duration_seconds": 210,
                    "thumbnails": thumbnails,
                }
                for i in range(1, 4)
            ]
        }
    if albums is None:
        albums = {
            "results": [
                {
                    "browseId": f"MPREb_alb{i}",
                    "title": f"Album {i}",
                    "artists": [{"name": name, "id": "UCtest"}],
                    "year": str(2020 + i),
                    "thumbnails": thumbnails,
                }
                for i in range(1, 3)
            ]
        }
    if related is None:
        related = {
            "results": [
                {
                    "browseId": f"UCrel_{i}",
                    "title": f"Related Artist {i}",
                    "thumbnails": thumbnails,
                }
                for i in range(1, 3)
            ]
        }
    return {
        "name": name,
        "description": description,
        "songs": songs,
        "albums": albums,
        "related": related,
        "thumbnails": thumbnails,
    }


# ===================================================================
# Dataclass creation tests
# ===================================================================


class TestAlbumInfoDataclass:
    """Test AlbumInfo dataclass creation and immutability."""

    def test_creation_with_defaults(self) -> None:
        album = AlbumInfo(browse_id="MPREb_1", title="Test", artist="Artist")
        assert album.browse_id == "MPREb_1"
        assert album.title == "Test"
        assert album.artist == "Artist"
        assert album.year == ""
        assert album.tracks == []
        assert album.thumbnail_url == ""

    def test_creation_with_all_fields(self) -> None:
        tracks = _make_tracks(3)
        album = AlbumInfo(
            browse_id="MPREb_2",
            title="Full Album",
            artist="Full Artist",
            year="2024",
            tracks=tracks,
            thumbnail_url="https://example.com/thumb.jpg",
        )
        assert album.year == "2024"
        assert len(album.tracks) == 3
        assert album.thumbnail_url == "https://example.com/thumb.jpg"

    def test_frozen(self) -> None:
        album = AlbumInfo(browse_id="MPREb_1", title="Test", artist="A")
        with pytest.raises(AttributeError):
            album.title = "Mutated"  # type: ignore[misc]


class TestArtistInfoDataclass:
    """Test ArtistInfo dataclass creation and immutability."""

    def test_creation_with_defaults(self) -> None:
        artist = ArtistInfo(channel_id="UC1", name="Test")
        assert artist.channel_id == "UC1"
        assert artist.name == "Test"
        assert artist.description == ""
        assert artist.top_songs == []
        assert artist.albums == []
        assert artist.related_artists == []
        assert artist.thumbnail_url == ""

    def test_creation_with_all_fields(self) -> None:
        artist = _make_artist_info()
        assert artist.name == "Test Artist"
        assert len(artist.top_songs) == 3
        assert len(artist.albums) == 2
        assert len(artist.related_artists) == 2
        assert artist.thumbnail_url == "https://example.com/artist.jpg"

    def test_frozen(self) -> None:
        artist = ArtistInfo(channel_id="UC1", name="Test")
        with pytest.raises(AttributeError):
            artist.name = "Mutated"  # type: ignore[misc]


class TestRelatedArtistDataclass:
    """Test RelatedArtist dataclass creation and immutability."""

    def test_creation_with_defaults(self) -> None:
        rel = RelatedArtist(channel_id="UC1", name="Related")
        assert rel.channel_id == "UC1"
        assert rel.name == "Related"
        assert rel.thumbnail_url == ""

    def test_frozen(self) -> None:
        rel = RelatedArtist(channel_id="UC1", name="Related")
        with pytest.raises(AttributeError):
            rel.name = "Mutated"  # type: ignore[misc]


# ===================================================================
# API conversion tests
# ===================================================================


class TestGetAlbumConversion:
    """Test MusicAPI.get_album() response parsing."""

    @patch("ytmusic_tui.api.YTMusic")
    def test_basic_album_conversion(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        mock_client.get_album.return_value = _make_raw_album_response()
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        album = api.get_album("MPREb_test")

        assert isinstance(album, AlbumInfo)
        assert album.browse_id == "MPREb_test"
        assert album.title == "Test Album"
        assert album.artist == "Test Artist"
        assert album.year == "2024"
        assert len(album.tracks) == 3
        assert album.tracks[0].video_id == "alb_t1"
        assert album.tracks[0].title == "Album Track 1"
        assert album.thumbnail_url == "https://lh3.google.com/album_lg.jpg"

    @patch("ytmusic_tui.api.YTMusic")
    def test_album_with_no_tracks(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        mock_client.get_album.return_value = _make_raw_album_response(tracks=[])
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        album = api.get_album("MPREb_empty")

        assert album.tracks == []

    @patch("ytmusic_tui.api.YTMusic")
    def test_album_with_none_tracks(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        raw = _make_raw_album_response()
        raw["tracks"] = None
        mock_client.get_album.return_value = raw
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        album = api.get_album("MPREb_none")

        assert album.tracks == []

    @patch("ytmusic_tui.api.YTMusic")
    def test_album_track_inherits_album_artist(self, mock_ytmusic_cls: MagicMock) -> None:
        """Track without artists should fall back to album artist."""
        mock_client = MagicMock()
        raw = _make_raw_album_response()
        # Remove artists from first track
        raw["tracks"][0]["artists"] = None
        mock_client.get_album.return_value = raw
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        album = api.get_album("MPREb_test")

        # Should inherit the album-level artist
        assert album.tracks[0].artist == "Test Artist"

    @patch("ytmusic_tui.api.YTMusic")
    def test_album_missing_year(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        raw = _make_raw_album_response()
        raw.pop("year", None)
        mock_client.get_album.return_value = raw
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        album = api.get_album("MPREb_test")

        assert album.year == ""

    @patch("ytmusic_tui.api.YTMusic")
    def test_album_missing_thumbnails(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        raw = _make_raw_album_response(thumbnails=None)
        raw["thumbnails"] = None
        mock_client.get_album.return_value = raw
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        album = api.get_album("MPREb_test")

        assert album.thumbnail_url == ""


class TestGetArtistConversion:
    """Test MusicAPI.get_artist() response parsing."""

    @patch("ytmusic_tui.api.YTMusic")
    def test_basic_artist_conversion(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        mock_client.get_artist.return_value = _make_raw_artist_response()
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        artist = api.get_artist("UCtest")

        assert isinstance(artist, ArtistInfo)
        assert artist.channel_id == "UCtest"
        assert artist.name == "Test Artist"
        assert artist.description == "A great artist"
        assert len(artist.top_songs) == 3
        assert artist.top_songs[0].video_id == "top_1"
        assert len(artist.albums) == 2
        assert artist.albums[0].browse_id == "MPREb_alb1"
        assert len(artist.related_artists) == 2
        assert artist.related_artists[0].channel_id == "UCrel_1"

    @patch("ytmusic_tui.api.YTMusic")
    def test_artist_with_missing_songs_section(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        raw = _make_raw_artist_response()
        raw.pop("songs", None)
        mock_client.get_artist.return_value = raw
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        artist = api.get_artist("UCtest")

        assert artist.top_songs == []

    @patch("ytmusic_tui.api.YTMusic")
    def test_artist_with_missing_albums_section(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        raw = _make_raw_artist_response()
        raw.pop("albums", None)
        mock_client.get_artist.return_value = raw
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        artist = api.get_artist("UCtest")

        assert artist.albums == []

    @patch("ytmusic_tui.api.YTMusic")
    def test_artist_with_missing_related_section(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        raw = _make_raw_artist_response()
        raw.pop("related", None)
        mock_client.get_artist.return_value = raw
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        artist = api.get_artist("UCtest")

        assert artist.related_artists == []

    @patch("ytmusic_tui.api.YTMusic")
    def test_artist_with_empty_results(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        mock_client.get_artist.return_value = _make_raw_artist_response(
            songs={"results": []},
            albums={"results": []},
            related={"results": []},
        )
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        artist = api.get_artist("UCtest")

        assert artist.top_songs == []
        assert artist.albums == []
        assert artist.related_artists == []

    @patch("ytmusic_tui.api.YTMusic")
    def test_artist_with_none_description(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        raw = _make_raw_artist_response(description=None)  # type: ignore[arg-type]
        raw["description"] = None
        mock_client.get_artist.return_value = raw
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        artist = api.get_artist("UCtest")

        assert artist.description == ""

    @patch("ytmusic_tui.api.YTMusic")
    def test_artist_songs_section_as_non_dict(self, mock_ytmusic_cls: MagicMock) -> None:
        """If songs is not a dict (e.g. a string), handle gracefully."""
        mock_client = MagicMock()
        raw = _make_raw_artist_response()
        raw["songs"] = "not a dict"
        mock_client.get_artist.return_value = raw
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        artist = api.get_artist("UCtest")

        assert artist.top_songs == []

    @patch("ytmusic_tui.api.YTMusic")
    def test_related_artist_uses_browseid(self, mock_ytmusic_cls: MagicMock) -> None:
        """Related artists should use browseId as channel_id."""
        mock_client = MagicMock()
        mock_client.get_artist.return_value = _make_raw_artist_response(
            related={
                "results": [
                    {
                        "browseId": "UCfromBrowse",
                        "title": "From BrowseId",
                        "thumbnails": [],
                    }
                ]
            }
        )
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        artist = api.get_artist("UCtest")

        assert artist.related_artists[0].channel_id == "UCfromBrowse"
        assert artist.related_artists[0].name == "From BrowseId"

    @patch("ytmusic_tui.api.YTMusic")
    def test_album_info_from_artist_page(self, mock_ytmusic_cls: MagicMock) -> None:
        """Albums on artist page should parse into AlbumInfo."""
        mock_client = MagicMock()
        mock_client.get_artist.return_value = _make_raw_artist_response()
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        artist = api.get_artist("UCtest")

        album = artist.albums[0]
        assert isinstance(album, AlbumInfo)
        assert album.browse_id == "MPREb_alb1"
        assert album.title == "Album 1"
        assert album.year == "2021"

    @patch("ytmusic_tui.api.YTMusic")
    def test_skips_albums_without_browse_id(self, mock_ytmusic_cls: MagicMock) -> None:
        """Albums without browseId should be skipped."""
        mock_client = MagicMock()
        mock_client.get_artist.return_value = _make_raw_artist_response(
            albums={
                "results": [
                    {"title": "No ID Album", "year": "2024", "thumbnails": []},
                    {
                        "browseId": "MPREb_valid",
                        "title": "Valid Album",
                        "year": "2024",
                        "thumbnails": [],
                    },
                ]
            }
        )
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        artist = api.get_artist("UCtest")

        assert len(artist.albums) == 1
        assert artist.albums[0].browse_id == "MPREb_valid"


# ===================================================================
# AlbumView tests
# ===================================================================


class TestAlbumView:
    @pytest.mark.asyncio
    async def test_album_view_has_table(self) -> None:
        """AlbumView should contain a DataTable for tracks."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import DataTable

            from ytmusic_tui.views.album import AlbumView

            view = app.query_one(AlbumView)
            table = view.query_one("#album-table", DataTable)
            assert table is not None

    @pytest.mark.asyncio
    async def test_album_view_has_header(self) -> None:
        """AlbumView should have title and meta labels."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import Label

            from ytmusic_tui.views.album import AlbumView

            view = app.query_one(AlbumView)
            title_label = view.query_one("#album-title", Label)
            meta_label = view.query_one("#album-meta", Label)
            assert title_label is not None
            assert meta_label is not None

    @pytest.mark.asyncio
    async def test_album_view_populate(self) -> None:
        """show_album should populate the view with album data."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import DataTable, Label

            from ytmusic_tui.views.album import AlbumView

            view = app.query_one(AlbumView)
            album = _make_album_info()
            view.show_album(album)
            await _pilot.pause()

            title_label = view.query_one("#album-title", Label)
            assert title_label.content == "Test Album"

            meta_label = view.query_one("#album-meta", Label)
            assert "Test Artist" in meta_label.content
            assert "2024" in meta_label.content

            table = view.query_one("#album-table", DataTable)
            assert table.row_count == 5

    @pytest.mark.asyncio
    async def test_album_view_empty_album(self) -> None:
        """Empty album should show 'No tracks' status."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import Label

            from ytmusic_tui.views.album import AlbumView

            view = app.query_one(AlbumView)
            empty = AlbumInfo(
                browse_id="MPREb_empty",
                title="Empty Album",
                artist="Nobody",
            )
            view.show_album(empty)
            await _pilot.pause()

            status = view.query_one("#album-status", Label)
            assert "No tracks" in status.content

    @pytest.mark.asyncio
    async def test_album_view_track_selection_queues_from_position(self) -> None:
        """Selecting a track should queue all album tracks from that position."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from ytmusic_tui.views.album import AlbumView

            view = app.query_one(AlbumView)
            album = _make_album_info()
            view.show_album(album)
            await _pilot.pause()

            # Simulate selecting track at row index 2
            mock_event = MagicMock()
            mock_event.cursor_row = 2
            view.on_data_table_row_selected(mock_event)

            # Queue should have all 5 tracks with current at index 2
            assert len(app.queue_manager.tracks) == 5
            assert app.queue_manager.current_track == album.tracks[2]
            app.player.play.assert_called_once_with("vid_3")

    @pytest.mark.asyncio
    async def test_album_view_clear_resets_state(self) -> None:
        """Loading a new album should clear the previous state."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import DataTable

            from ytmusic_tui.views.album import AlbumView

            view = app.query_one(AlbumView)

            # First album
            view.show_album(_make_album_info())
            await _pilot.pause()

            table = view.query_one("#album-table", DataTable)
            assert table.row_count == 5

            # Second album (empty)
            view.show_album(AlbumInfo(browse_id="MPREb_2", title="Empty", artist="X"))
            await _pilot.pause()

            assert table.row_count == 0

    @pytest.mark.asyncio
    async def test_album_view_out_of_range_selection_ignored(self) -> None:
        """Out-of-range row selection should be silently ignored."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from ytmusic_tui.views.album import AlbumView

            view = app.query_one(AlbumView)
            album = _make_album_info()
            view.show_album(album)
            await _pilot.pause()

            mock_event = MagicMock()
            mock_event.cursor_row = 99
            view.on_data_table_row_selected(mock_event)

            # No crash, no play call
            app.player.play.assert_not_called()


# ===================================================================
# ArtistView tests
# ===================================================================


class TestArtistView:
    @pytest.mark.asyncio
    async def test_artist_view_has_sections(self) -> None:
        """ArtistView should have three DataTables."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import DataTable

            from ytmusic_tui.views.artist import ArtistView

            view = app.query_one(ArtistView)
            songs_table = view.query_one("#artist-top-songs", DataTable)
            albums_table = view.query_one("#artist-albums", DataTable)
            related_table = view.query_one("#artist-related", DataTable)
            assert songs_table is not None
            assert albums_table is not None
            assert related_table is not None

    @pytest.mark.asyncio
    async def test_artist_view_populate(self) -> None:
        """show_artist should populate all three sections."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import DataTable, Label

            from ytmusic_tui.views.artist import ArtistView

            view = app.query_one(ArtistView)
            artist = _make_artist_info()
            view.show_artist(artist)
            await _pilot.pause()

            name_label = view.query_one("#artist-name", Label)
            assert name_label.content == "Test Artist"

            songs_table = view.query_one("#artist-top-songs", DataTable)
            assert songs_table.row_count == 3

            albums_table = view.query_one("#artist-albums", DataTable)
            assert albums_table.row_count == 2

            related_table = view.query_one("#artist-related", DataTable)
            assert related_table.row_count == 2

    @pytest.mark.asyncio
    async def test_artist_view_song_selection_plays(self) -> None:
        """Selecting a top song should queue and play it."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import DataTable

            from ytmusic_tui.views.artist import ArtistView

            view = app.query_one(ArtistView)
            artist = _make_artist_info()
            view.show_artist(artist)
            await _pilot.pause()

            # Simulate selecting a row in the top songs table
            songs_table = view.query_one("#artist-top-songs", DataTable)
            mock_event = MagicMock()
            mock_event.data_table = songs_table
            mock_event.cursor_row = 1

            view.on_data_table_row_selected(mock_event)

            assert app.queue_manager.current_track == artist.top_songs[1]
            app.player.play.assert_called_once_with("vid_2")

    @pytest.mark.asyncio
    async def test_artist_view_album_selection_opens_album(self) -> None:
        """Selecting an album should call action_open_album."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import DataTable

            from ytmusic_tui.views.artist import ArtistView

            view = app.query_one(ArtistView)
            artist = _make_artist_info()
            view.show_artist(artist)
            await _pilot.pause()

            albums_table = view.query_one("#artist-albums", DataTable)
            mock_event = MagicMock()
            mock_event.data_table = albums_table
            mock_event.cursor_row = 0

            # Mock the app's action_open_album
            app.action_open_album = MagicMock()
            view.on_data_table_row_selected(mock_event)

            app.action_open_album.assert_called_once_with("MPREb_a1")

    @pytest.mark.asyncio
    async def test_artist_view_related_selection_opens_artist(self) -> None:
        """Selecting a related artist should call action_open_artist."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import DataTable

            from ytmusic_tui.views.artist import ArtistView

            view = app.query_one(ArtistView)
            artist = _make_artist_info()
            view.show_artist(artist)
            await _pilot.pause()

            related_table = view.query_one("#artist-related", DataTable)
            mock_event = MagicMock()
            mock_event.data_table = related_table
            mock_event.cursor_row = 0

            # Mock the app's action_open_artist
            app.action_open_artist = MagicMock()
            view.on_data_table_row_selected(mock_event)

            app.action_open_artist.assert_called_once_with("UCrel1")

    @pytest.mark.asyncio
    async def test_artist_view_empty_artist(self) -> None:
        """ArtistView should handle artist with no songs/albums/related."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import DataTable

            from ytmusic_tui.views.artist import ArtistView

            view = app.query_one(ArtistView)
            empty_artist = ArtistInfo(channel_id="UCempty", name="Empty Artist")
            view.show_artist(empty_artist)
            await _pilot.pause()

            songs_table = view.query_one("#artist-top-songs", DataTable)
            albums_table = view.query_one("#artist-albums", DataTable)
            related_table = view.query_one("#artist-related", DataTable)
            assert songs_table.row_count == 0
            assert albums_table.row_count == 0
            assert related_table.row_count == 0

    @pytest.mark.asyncio
    async def test_artist_view_clear_resets(self) -> None:
        """Loading a new artist should clear previous data."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import DataTable

            from ytmusic_tui.views.artist import ArtistView

            view = app.query_one(ArtistView)

            # Load one artist
            view.show_artist(_make_artist_info())
            await _pilot.pause()

            songs_table = view.query_one("#artist-top-songs", DataTable)
            assert songs_table.row_count == 3

            # Load empty artist
            view.show_artist(ArtistInfo(channel_id="UC2", name="Empty"))
            await _pilot.pause()

            assert songs_table.row_count == 0

    @pytest.mark.asyncio
    async def test_artist_view_out_of_range_ignored(self) -> None:
        """Out-of-range row selection in any table should be silent."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import DataTable

            from ytmusic_tui.views.artist import ArtistView

            view = app.query_one(ArtistView)
            view.show_artist(_make_artist_info())
            await _pilot.pause()

            songs_table = view.query_one("#artist-top-songs", DataTable)
            mock_event = MagicMock()
            mock_event.data_table = songs_table
            mock_event.cursor_row = 99

            # Should not crash
            view.on_data_table_row_selected(mock_event)
            app.player.play.assert_not_called()


# ===================================================================
# Navigation / keybinding tests
# ===================================================================


class TestNavigationActions:
    @pytest.mark.asyncio
    async def test_album_view_exists_in_app(self) -> None:
        """AlbumView should be mounted in the ContentSwitcher."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from ytmusic_tui.views.album import AlbumView

            album_view = app.query_one(AlbumView)
            assert album_view is not None
            assert album_view.id == "album"

    @pytest.mark.asyncio
    async def test_artist_view_exists_in_app(self) -> None:
        """ArtistView should be mounted in the ContentSwitcher."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from ytmusic_tui.views.artist import ArtistView

            artist_view = app.query_one(ArtistView)
            assert artist_view is not None
            assert artist_view.id == "artist"

    @pytest.mark.asyncio
    async def test_action_open_album_switches_view(self) -> None:
        """action_open_album should switch to album view."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import ContentSwitcher

            app.action_open_album("MPREb_test")
            await _pilot.pause()

            switcher = app.query_one(ContentSwitcher)
            assert switcher.current == "album"

    @pytest.mark.asyncio
    async def test_action_open_artist_switches_view(self) -> None:
        """action_open_artist should switch to artist view."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import ContentSwitcher

            app.action_open_artist("UCtest")
            await _pilot.pause()

            switcher = app.query_one(ContentSwitcher)
            assert switcher.current == "artist"

    @pytest.mark.asyncio
    async def test_keybinding_a_exists(self) -> None:
        """'a' keybinding for artist page should exist."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            binding_keys = [b.key for b in app.BINDINGS]
            assert "a" in binding_keys

    @pytest.mark.asyncio
    async def test_keybinding_shift_a_exists(self) -> None:
        """'A' keybinding for album page should exist."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            binding_keys = [b.key for b in app.BINDINGS]
            assert "A" in binding_keys

    @pytest.mark.asyncio
    async def test_open_current_artist_no_track_does_nothing(self) -> None:
        """action_open_current_artist should do nothing when no track is playing."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import ContentSwitcher

            # No track in queue
            app.action_open_current_artist()
            await _pilot.pause()

            switcher = app.query_one(ContentSwitcher)
            # Should still be on home
            assert switcher.current == "home"

    @pytest.mark.asyncio
    async def test_open_current_album_no_track_does_nothing(self) -> None:
        """action_open_current_album should do nothing when no track is playing."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import ContentSwitcher

            app.action_open_current_album()
            await _pilot.pause()

            switcher = app.query_one(ContentSwitcher)
            assert switcher.current == "home"

    @pytest.mark.asyncio
    async def test_open_current_album_no_album_name_does_nothing(self) -> None:
        """action_open_current_album should do nothing when track has no album."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import ContentSwitcher

            # Add track without album
            track = Track(
                video_id="vid_noalbum",
                title="No Album Song",
                artist="Artist",
                album="",
            )
            app.queue_manager.set_playlist([track])

            app.action_open_current_album()
            await _pilot.pause()

            switcher = app.query_one(ContentSwitcher)
            assert switcher.current == "home"

    @pytest.mark.asyncio
    async def test_switch_view_to_album(self) -> None:
        """action_switch_view('album') should work."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import ContentSwitcher

            app.action_switch_view("album")
            await _pilot.pause()

            switcher = app.query_one(ContentSwitcher)
            assert switcher.current == "album"

    @pytest.mark.asyncio
    async def test_switch_view_to_artist(self) -> None:
        """action_switch_view('artist') should work."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from textual.widgets import ContentSwitcher

            app.action_switch_view("artist")
            await _pilot.pause()

            switcher = app.query_one(ContentSwitcher)
            assert switcher.current == "artist"
