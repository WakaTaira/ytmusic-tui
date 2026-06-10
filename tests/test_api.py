"""Tests for the YouTube Music API wrapper."""

from __future__ import annotations

from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest
from helpers import load_fixture

from ytmusic_tui.api import (
    AlbumInfo,
    ArtistInfo,
    HomeSection,
    MusicAPI,
    PlaylistInfo,
    RelatedArtist,
    SearchResults,
    parse_duration,
)
from ytmusic_tui.queue import Track

# ---------------------------------------------------------------------------
# Duration parsing
# ---------------------------------------------------------------------------


class TestParseDuration:
    """Test the duration string parser (M:SS, H:MM:SS, edge cases)."""

    def test_minutes_seconds(self) -> None:
        assert parse_duration("3:45") == 225.0

    def test_hour_minutes_seconds(self) -> None:
        assert parse_duration("1:02:30") == 3750.0

    def test_zero_duration(self) -> None:
        assert parse_duration("0:00") == 0.0

    def test_none_returns_zero(self) -> None:
        assert parse_duration(None) == 0.0

    def test_empty_string_returns_zero(self) -> None:
        assert parse_duration("") == 0.0

    def test_single_digit_seconds(self) -> None:
        assert parse_duration("4:05") == 245.0

    def test_long_song(self) -> None:
        assert parse_duration("12:34") == 754.0

    def test_seconds_only(self) -> None:
        # Some API responses may return just seconds
        assert parse_duration("45") == 45.0


# ---------------------------------------------------------------------------
# Mock response fixtures
# ---------------------------------------------------------------------------


_UNSET: object = object()


def _make_search_song_result(
    *,
    video_id: str = "dQw4w9WgXcQ",
    title: str = "Never Gonna Give You Up",
    artists: list[dict[str, str]] | None | object = _UNSET,
    album: dict[str, str] | None | object = _UNSET,
    duration: str | None = "3:33",
    thumbnails: list[dict[str, int | str]] | None | object = _UNSET,
) -> dict:
    """Build a realistic ytmusicapi search result dict for a song.

    Loads the canonical payload from ``search_song.json`` and applies
    the given overrides.  Pass None explicitly to simulate a missing
    field.  Omit the kwarg (or pass _UNSET) to keep the fixture default.
    """
    result: dict = load_fixture("search_song.json")
    result["videoId"] = video_id
    result["title"] = title
    result["duration"] = duration
    if artists is not _UNSET:
        result["artists"] = artists
    if album is not _UNSET:
        result["album"] = album
    if thumbnails is not _UNSET:
        result["thumbnails"] = thumbnails
    return result


def _make_playlist_item(
    *,
    playlist_id: str = "VLPL_abc123",
    title: str = "My Favourites",
    description: str = "Best songs ever",
    count: int | str = 42,
    thumbnails: list[dict[str, int | str]] | None = None,
) -> dict:
    """Build a realistic ytmusicapi playlist item.

    Loads the canonical payload from ``playlist_item.json`` and applies
    the given overrides.
    """
    result: dict = load_fixture("playlist_item.json")
    result["playlistId"] = playlist_id
    result["title"] = title
    result["description"] = description
    result["count"] = count
    if thumbnails is not None:
        result["thumbnails"] = thumbnails
    return result


def _make_playlist_track(
    *,
    video_id: str = "xYZtrack1",
    title: str = "Playlist Song",
    artists: list[dict[str, str]] | None = None,
    album: dict[str, str] | None = None,
    duration: str | None = "4:12",
    thumbnails: list[dict[str, int | str]] | None = None,
) -> dict:
    """Build a ytmusicapi track dict as returned inside get_playlist.

    Loads the canonical payload from ``playlist_track.json`` and applies
    the given overrides.
    """
    result: dict = load_fixture("playlist_track.json")
    result["videoId"] = video_id
    result["title"] = title
    result["duration"] = duration
    if artists is not None:
        result["artists"] = artists
    if album is not None:
        result["album"] = album
    if thumbnails is not None:
        result["thumbnails"] = thumbnails
    return result


def _make_home_section(
    *,
    title: str = "Quick picks",
    contents: list[dict] | None = None,
) -> dict:
    """Build a ytmusicapi home section.

    Loads the canonical payload for the first section from
    ``home_sections.json`` and applies the given overrides.
    """
    result: dict = load_fixture("home_sections.json")[0]
    result["title"] = title
    if contents is not None:
        result["contents"] = contents
    return result


def _make_library_album_item(
    browse_id: str = "MPREb_lib1",
    title: str = "Lib Album",
    year: str = "2024",
) -> dict:
    """Build a minimal library album dict as returned by ytmusicapi.

    Loads the canonical payload from ``library_album_item.json`` and
    applies the given overrides.
    """
    result: dict = load_fixture("library_album_item.json")
    result["browseId"] = browse_id
    result["title"] = title
    result["year"] = year
    return result


def _make_library_artist_item(
    browse_id: str = "UClib1",
    artist: str = "Lib Artist",
) -> dict:
    """Build a minimal library artist dict as returned by ytmusicapi.

    ytmusicapi's get_library_artists() returns dicts with 'browseId'
    and 'artist' keys (not 'name').

    Loads the canonical payload from ``library_artist_item.json`` and
    applies the given overrides.
    """
    result: dict = load_fixture("library_artist_item.json")
    result["browseId"] = browse_id
    result["artist"] = artist
    return result


# ---------------------------------------------------------------------------
# MusicAPI.__init__
# ---------------------------------------------------------------------------


class TestSessionValidity:
    """is_session_valid: stale cookies are served as logged-out pages."""

    @patch("ytmusic_tui.api.YTMusic")
    def test_valid_when_account_info_parses(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        mock_client.get_account_info.return_value = {"accountName": "taira"}
        mock_ytmusic_cls.return_value = mock_client

        assert MusicAPI("/fake/path").is_session_valid() is True

    @patch("ytmusic_tui.api.YTMusic")
    def test_signed_out_response_is_invalid(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        # ytmusicapi raises KeyError when the signed-out page lacks the
        # account header structure.
        mock_client.get_account_info.side_effect = KeyError("header")
        mock_ytmusic_cls.return_value = mock_client

        assert MusicAPI("/fake/path").is_session_valid() is False

    @patch("ytmusic_tui.api.YTMusic")
    def test_network_error_assumed_valid(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        mock_client.get_account_info.side_effect = OSError("connection refused")
        mock_ytmusic_cls.return_value = mock_client

        assert MusicAPI("/fake/path").is_session_valid() is True

    @patch("ytmusic_tui.api.YTMusic")
    def test_unusable_auth_file_is_invalid(self, mock_ytmusic_cls: MagicMock) -> None:
        """YTMusic() itself raising (e.g. OAuth misdetection) means the
        session cannot possibly be valid."""
        mock_ytmusic_cls.side_effect = Exception("oauth JSON provided via auth argument")

        assert MusicAPI("/fake/path").is_session_valid() is False

    @patch("ytmusic_tui.api.YTMusic")
    def test_construction_never_raises(self, mock_ytmusic_cls: MagicMock) -> None:
        """MusicAPI() must not raise even for unusable auth files; the
        error surfaces on first use inside classified worker paths."""
        mock_ytmusic_cls.side_effect = Exception("oauth JSON provided via auth argument")

        api = MusicAPI("/fake/path")  # must not raise

        import pytest as _pytest

        with _pytest.raises(Exception, match="oauth JSON"):
            api.search_all("query")


class TestMusicAPIInit:
    """Test client construction."""

    @patch("ytmusic_tui.api.YTMusic")
    def test_client_created_lazily_with_auth_path(self, mock_ytmusic_cls: MagicMock) -> None:
        api = MusicAPI("/fake/browser.json")
        # Construction must not touch YTMusic: a malformed auth file
        # would otherwise crash the app before it can show an error.
        mock_ytmusic_cls.assert_not_called()
        _ = api._client
        mock_ytmusic_cls.assert_called_once_with("/fake/browser.json")

    @patch("ytmusic_tui.api.YTMusic")
    def test_accepts_path_object(self, mock_ytmusic_cls: MagicMock) -> None:
        api = MusicAPI(Path("/fake/browser.json"))
        _ = api._client
        mock_ytmusic_cls.assert_called_once_with("/fake/browser.json")


# ---------------------------------------------------------------------------
# Track conversion
# ---------------------------------------------------------------------------


class TestTrackConversion:
    """Test converting raw ytmusicapi song dicts into Track dataclasses.

    Exercised through search_all (the public song-search entry point),
    which routes "song"/"video" results through the same _dict_to_track
    helper. Covers duration parsing, thumbnail picking, and artist
    joining — the parsing behavior the now-removed search() carried.
    """

    @patch("ytmusic_tui.api.YTMusic")
    def test_basic_song_conversion(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        mock_client.search.return_value = [
            _make_search_song_result(),
        ]
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        results = api.search_all("rick astley", filter="songs").tracks

        assert len(results) == 1
        track = results[0]
        assert isinstance(track, Track)
        assert track.video_id == "dQw4w9WgXcQ"
        assert track.title == "Never Gonna Give You Up"
        assert track.artist == "Rick Astley"
        assert track.album == "Whenever You Need Somebody"
        assert track.duration_seconds == 213.0
        assert track.thumbnail_url == "https://lh3.google.com/large.jpg"

    @patch("ytmusic_tui.api.YTMusic")
    def test_multiple_artists_joined(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        mock_client.search.return_value = [
            _make_search_song_result(
                artists=[
                    {"name": "Artist A", "id": "UC1"},
                    {"name": "Artist B", "id": "UC2"},
                    {"name": "Artist C", "id": "UC3"},
                ]
            ),
        ]
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        results = api.search_all("collab", filter="songs").tracks
        assert results[0].artist == "Artist A, Artist B, Artist C"

    @patch("ytmusic_tui.api.YTMusic")
    def test_missing_album(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        mock_client.search.return_value = [
            _make_search_song_result(album=None),
        ]
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        results = api.search_all("single", filter="songs").tracks
        assert results[0].album == ""

    @patch("ytmusic_tui.api.YTMusic")
    def test_missing_artists(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        mock_client.search.return_value = [
            _make_search_song_result(artists=None),
        ]
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        results = api.search_all("unknown", filter="songs").tracks
        assert results[0].artist == ""

    @patch("ytmusic_tui.api.YTMusic")
    def test_missing_duration(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        result = _make_search_song_result(duration=None)
        result.pop("duration_seconds", None)
        mock_client.search.return_value = [result]
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        results = api.search_all("no duration", filter="songs").tracks
        assert results[0].duration_seconds == 0.0

    @patch("ytmusic_tui.api.YTMusic")
    def test_missing_thumbnails(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        mock_client.search.return_value = [
            _make_search_song_result(thumbnails=None),
        ]
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        results = api.search_all("no thumb", filter="songs").tracks
        assert results[0].thumbnail_url == ""

    @patch("ytmusic_tui.api.YTMusic")
    def test_empty_thumbnails_list(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        mock_client.search.return_value = [
            _make_search_song_result(thumbnails=[]),
        ]
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        results = api.search_all("empty thumbs", filter="songs").tracks
        assert results[0].thumbnail_url == ""

    @patch("ytmusic_tui.api.YTMusic")
    def test_skips_items_without_video_id(self, mock_ytmusic_cls: MagicMock) -> None:
        """Song results without videoId should be skipped, not crash."""
        mock_client = MagicMock()
        mock_client.search.return_value = [
            {"resultType": "song", "title": "No ID"},
            _make_search_song_result(),
        ]
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        results = api.search_all("mixed results").tracks
        assert len(results) == 1
        assert results[0].video_id == "dQw4w9WgXcQ"


# ---------------------------------------------------------------------------
# get_library_playlists()
# ---------------------------------------------------------------------------


class TestGetLibraryPlaylists:
    """Test the library playlists method."""

    @patch("ytmusic_tui.api.YTMusic")
    def test_returns_playlist_info_list(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        mock_client.get_library_playlists.return_value = load_fixture("library_playlists.json")
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        playlists = api.get_library_playlists(limit=50)

        assert len(playlists) == 2
        assert all(isinstance(p, PlaylistInfo) for p in playlists)

        assert playlists[0].playlist_id == "PL_1"
        assert playlists[0].title == "Chill"
        assert playlists[0].track_count == 10
        assert playlists[0].thumbnail_url == "https://lh3.google.com/pl_thumb.jpg"

        # count can come as a string from the API
        assert playlists[1].track_count == 25

    @patch("ytmusic_tui.api.YTMusic")
    def test_passes_limit(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        mock_client.get_library_playlists.return_value = []
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        api.get_library_playlists(limit=10)

        mock_client.get_library_playlists.assert_called_once_with(limit=10)

    @patch("ytmusic_tui.api.YTMusic")
    def test_handles_missing_description(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        item = _make_playlist_item()
        item.pop("description", None)
        mock_client.get_library_playlists.return_value = [item]
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        playlists = api.get_library_playlists()
        assert playlists[0].description == ""

    @patch("ytmusic_tui.api.YTMusic")
    def test_handles_missing_count(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        item = _make_playlist_item()
        item.pop("count", None)
        mock_client.get_library_playlists.return_value = [item]
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        playlists = api.get_library_playlists()
        assert playlists[0].track_count == 0


# ---------------------------------------------------------------------------
# get_library_albums()
# ---------------------------------------------------------------------------


class TestGetLibraryAlbums:
    """Test the library albums method."""

    @patch("ytmusic_tui.api.YTMusic")
    def test_returns_album_info_list(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        mock_client.get_library_albums.return_value = load_fixture("library_albums.json")
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        albums = api.get_library_albums(limit=50)

        assert len(albums) == 2
        assert all(isinstance(a, AlbumInfo) for a in albums)
        assert albums[0].browse_id == "MPREb_1"
        assert albums[0].title == "Album A"
        assert albums[0].artist == "Lib Artist"
        assert albums[1].browse_id == "MPREb_2"

    @patch("ytmusic_tui.api.YTMusic")
    def test_passes_limit(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        mock_client.get_library_albums.return_value = []
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        api.get_library_albums(limit=10)

        mock_client.get_library_albums.assert_called_once_with(limit=10)

    @patch("ytmusic_tui.api.YTMusic")
    def test_skips_items_without_browse_id(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        item = _make_library_album_item()
        item.pop("browseId")
        mock_client.get_library_albums.return_value = [item]
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        albums = api.get_library_albums()
        assert albums == []

    @patch("ytmusic_tui.api.YTMusic")
    def test_handles_empty_response(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        mock_client.get_library_albums.return_value = []
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        albums = api.get_library_albums()
        assert albums == []


# ---------------------------------------------------------------------------
# get_library_artists()
# ---------------------------------------------------------------------------


class TestGetLibraryArtists:
    """Test the library artists method."""

    @patch("ytmusic_tui.api.YTMusic")
    def test_returns_artist_info_list(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        mock_client.get_library_artists.return_value = load_fixture("library_artists.json")
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        artists = api.get_library_artists(limit=50)

        assert len(artists) == 2
        assert all(isinstance(a, ArtistInfo) for a in artists)
        assert artists[0].channel_id == "UC_1"
        assert artists[0].name == "Artist A"
        assert artists[1].channel_id == "UC_2"
        assert artists[1].name == "Artist B"

    @patch("ytmusic_tui.api.YTMusic")
    def test_passes_limit(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        mock_client.get_library_artists.return_value = []
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        api.get_library_artists(limit=15)

        mock_client.get_library_artists.assert_called_once_with(limit=15)

    @patch("ytmusic_tui.api.YTMusic")
    def test_skips_items_without_browse_id(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        item = _make_library_artist_item()
        item.pop("browseId")
        mock_client.get_library_artists.return_value = [item]
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        artists = api.get_library_artists()
        assert artists == []

    @patch("ytmusic_tui.api.YTMusic")
    def test_handles_empty_response(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        mock_client.get_library_artists.return_value = []
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        artists = api.get_library_artists()
        assert artists == []

    @patch("ytmusic_tui.api.YTMusic")
    def test_falls_back_to_name_key(self, mock_ytmusic_cls: MagicMock) -> None:
        """Some responses may use 'name' instead of 'artist'."""
        mock_client = MagicMock()
        item = {"browseId": "UC_fb", "name": "Fallback Name", "thumbnails": []}
        mock_client.get_library_artists.return_value = [item]
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        artists = api.get_library_artists()
        assert len(artists) == 1
        assert artists[0].name == "Fallback Name"

    @patch("ytmusic_tui.api.YTMusic")
    def test_returns_simplified_artist_info(self, mock_ytmusic_cls: MagicMock) -> None:
        """Library artists should have empty top_songs, albums, etc."""
        mock_client = MagicMock()
        item = _make_library_artist_item(browse_id="UC_simp", artist="Simple")
        mock_client.get_library_artists.return_value = [item]
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        artists = api.get_library_artists()
        assert len(artists) == 1
        assert artists[0].top_songs == []
        assert artists[0].albums == []
        assert artists[0].related_artists == []


# ---------------------------------------------------------------------------
# get_playlist_tracks()
# ---------------------------------------------------------------------------


class TestGetPlaylistTracks:
    """Test getting tracks from a playlist."""

    @patch("ytmusic_tui.api.YTMusic")
    def test_returns_track_list(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        mock_client.get_playlist.return_value = load_fixture("playlist_with_tracks.json")
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        tracks = api.get_playlist_tracks("PL_test")

        assert len(tracks) == 2
        assert all(isinstance(t, Track) for t in tracks)
        assert tracks[0].video_id == "t1"
        assert tracks[1].video_id == "t2"

    @patch("ytmusic_tui.api.YTMusic")
    def test_skips_unavailable_tracks(self, mock_ytmusic_cls: MagicMock) -> None:
        """Tracks with videoId=None (deleted/unavailable) should be skipped."""
        mock_client = MagicMock()
        unavailable = _make_playlist_track(video_id="ok1")
        deleted = _make_playlist_track()
        deleted["videoId"] = None
        mock_client.get_playlist.return_value = {
            "id": "PL_test",
            "title": "Test",
            "tracks": [unavailable, deleted],
        }
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        tracks = api.get_playlist_tracks("PL_test")
        assert len(tracks) == 1
        assert tracks[0].video_id == "ok1"

    @patch("ytmusic_tui.api.YTMusic")
    def test_handles_empty_playlist(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        mock_client.get_playlist.return_value = {
            "id": "PL_empty",
            "title": "Empty",
            "tracks": [],
        }
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        tracks = api.get_playlist_tracks("PL_empty")
        assert tracks == []

    @patch("ytmusic_tui.api.YTMusic")
    def test_handles_none_tracks_key(self, mock_ytmusic_cls: MagicMock) -> None:
        """Some API responses may have tracks=None."""
        mock_client = MagicMock()
        mock_client.get_playlist.return_value = {
            "id": "PL_none",
            "title": "Broken",
            "tracks": None,
        }
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        tracks = api.get_playlist_tracks("PL_none")
        assert tracks == []


# ---------------------------------------------------------------------------
# get_home()
# ---------------------------------------------------------------------------


class TestGetHome:
    """Test the home page recommendations."""

    @patch("ytmusic_tui.api.YTMusic")
    def test_returns_home_sections(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        mock_client.get_home.return_value = load_fixture("home_sections.json")
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        sections = api.get_home()

        assert len(sections) == 2
        assert all(isinstance(s, HomeSection) for s in sections)
        assert sections[0].title == "Quick picks"
        assert sections[1].title == "Forgotten favourites"

    @patch("ytmusic_tui.api.YTMusic")
    def test_home_section_contains_tracks_and_playlists(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        mock_client.get_home.return_value = [_make_home_section()]
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        sections = api.get_home()

        items = sections[0].items
        assert len(items) == 2
        assert isinstance(items[0], Track)
        assert isinstance(items[1], PlaylistInfo)

    @patch("ytmusic_tui.api.YTMusic")
    def test_handles_empty_home(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        mock_client.get_home.return_value = []
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        sections = api.get_home()
        assert sections == []

    @patch("ytmusic_tui.api.YTMusic")
    def test_skips_sections_without_contents(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        mock_client.get_home.return_value = [
            {"title": "Broken Section"},  # no contents key
            _make_home_section(title="Good Section"),
        ]
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        sections = api.get_home()
        assert len(sections) == 1
        assert sections[0].title == "Good Section"


# ---------------------------------------------------------------------------
# get_liked_songs()
# ---------------------------------------------------------------------------


class TestGetLikedSongs:
    """Test getting liked/thumbs-up songs."""

    @patch("ytmusic_tui.api.YTMusic")
    def test_returns_liked_tracks(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        mock_client.get_liked_songs.return_value = load_fixture("liked_songs.json")
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        tracks = api.get_liked_songs(limit=50)

        assert len(tracks) == 2
        assert tracks[0].video_id == "like1"
        assert tracks[1].video_id == "like2"

    @patch("ytmusic_tui.api.YTMusic")
    def test_passes_limit(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        mock_client.get_liked_songs.return_value = {"tracks": []}
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        api.get_liked_songs(limit=200)

        mock_client.get_liked_songs.assert_called_once_with(limit=200)

    @patch("ytmusic_tui.api.YTMusic")
    def test_handles_empty_likes(self, mock_ytmusic_cls: MagicMock) -> None:
        mock_client = MagicMock()
        mock_client.get_liked_songs.return_value = {"tracks": []}
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        tracks = api.get_liked_songs()
        assert tracks == []


# ---------------------------------------------------------------------------
# PlaylistInfo dataclass
# ---------------------------------------------------------------------------


class TestPlaylistInfo:
    """Test PlaylistInfo is frozen and has correct defaults."""

    def test_frozen(self) -> None:
        info = PlaylistInfo(playlist_id="PL1", title="Test")
        with pytest.raises(AttributeError):
            info.title = "Mutated"  # type: ignore[misc]

    def test_defaults(self) -> None:
        info = PlaylistInfo(playlist_id="PL1", title="Test")
        assert info.description == ""
        assert info.track_count == 0
        assert info.thumbnail_url == ""


# ---------------------------------------------------------------------------
# HomeSection dataclass
# ---------------------------------------------------------------------------


class TestHomeSection:
    """Test HomeSection is frozen."""

    def test_frozen(self) -> None:
        section = HomeSection(title="Test", items=[])
        with pytest.raises(AttributeError):
            section.title = "Mutated"  # type: ignore[misc]


# ---------------------------------------------------------------------------
# SearchResults dataclass
# ---------------------------------------------------------------------------


class TestSearchResults:
    """Test SearchResults dataclass."""

    def test_frozen(self) -> None:
        results = SearchResults()
        with pytest.raises(AttributeError):
            results.tracks = []  # type: ignore[misc]

    def test_defaults_empty(self) -> None:
        results = SearchResults()
        assert results.tracks == []
        assert results.albums == []
        assert results.artists == []
        assert results.playlists == []

    def test_with_data(self) -> None:
        track = Track(video_id="v1", title="Song", artist="Art")
        album = AlbumInfo(browse_id="b1", title="Alb", artist="Art")
        artist = RelatedArtist(channel_id="c1", name="Art")
        playlist = PlaylistInfo(playlist_id="p1", title="PL")

        results = SearchResults(
            tracks=[track],
            albums=[album],
            artists=[artist],
            playlists=[playlist],
        )
        assert len(results.tracks) == 1
        assert len(results.albums) == 1
        assert len(results.artists) == 1
        assert len(results.playlists) == 1


# ---------------------------------------------------------------------------
# search_all()
# ---------------------------------------------------------------------------


class TestSearchAll:
    """Test the search_all method for multi-category search."""

    @patch("ytmusic_tui.api.YTMusic")
    def test_categorizes_mixed_results(self, mock_ytmusic_cls: MagicMock) -> None:
        """Mixed search results should be categorized by resultType."""
        mock_client = MagicMock()
        mock_client.search.return_value = load_fixture("search_results_mixed.json")
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        results = api.search_all("test query", limit=20)

        assert isinstance(results, SearchResults)
        # 1 song + 1 video = 2 tracks
        assert len(results.tracks) == 2
        assert results.tracks[0].video_id == "song1"
        assert results.tracks[1].video_id == "vid1"

        assert len(results.albums) == 1
        assert results.albums[0].browse_id == "MPREb_alb1"
        assert results.albums[0].title == "Great Album"

        assert len(results.artists) == 1
        assert results.artists[0].channel_id == "UCartist1"
        assert results.artists[0].name == "Famous Artist"

        assert len(results.playlists) == 1
        assert results.playlists[0].playlist_id == "VLPL_search1"

    @patch("ytmusic_tui.api.YTMusic")
    def test_empty_search(self, mock_ytmusic_cls: MagicMock) -> None:
        """Empty search results should return empty SearchResults."""
        mock_client = MagicMock()
        mock_client.search.return_value = []
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        results = api.search_all("nonexistent", limit=10)

        assert results.tracks == []
        assert results.albums == []
        assert results.artists == []
        assert results.playlists == []

    @patch("ytmusic_tui.api.YTMusic")
    def test_passes_limit(self, mock_ytmusic_cls: MagicMock) -> None:
        """search_all should pass limit (and a None filter) to the client."""
        mock_client = MagicMock()
        mock_client.search.return_value = []
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        api.search_all("test", limit=5)

        mock_client.search.assert_called_once_with("test", filter=None, limit=5)

    @patch("ytmusic_tui.api.YTMusic")
    def test_passes_explicit_filter(self, mock_ytmusic_cls: MagicMock) -> None:
        """An explicit category filter should reach the underlying client."""
        mock_client = MagicMock()
        mock_client.search.return_value = []
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        api.search_all("test query", limit=10, filter="albums")

        mock_client.search.assert_called_once_with("test query", filter="albums", limit=10)

    @patch("ytmusic_tui.api.YTMusic")
    def test_skips_invalid_items(self, mock_ytmusic_cls: MagicMock) -> None:
        """Items without required IDs should be skipped."""
        mock_client = MagicMock()
        mock_client.search.return_value = [
            # Song without videoId
            {"resultType": "song", "title": "No ID"},
            # Album without browseId
            {"resultType": "album", "title": "No Browse ID"},
            # Artist without browseId/channelId
            {"resultType": "artist", "title": "No Channel"},
            # Playlist without playlistId
            {"resultType": "playlist", "title": "No Playlist ID"},
            # Valid song
            {
                "resultType": "song",
                "videoId": "valid1",
                "title": "Valid Song",
                "artists": [],
                "duration": "2:00",
                "thumbnails": [],
            },
        ]
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        results = api.search_all("test")

        assert len(results.tracks) == 1
        assert results.tracks[0].video_id == "valid1"
        assert len(results.albums) == 0
        assert len(results.artists) == 0
        assert len(results.playlists) == 0

    @patch("ytmusic_tui.api.YTMusic")
    def test_songs_only_result(self, mock_ytmusic_cls: MagicMock) -> None:
        """When only songs are returned, other categories should be empty."""
        mock_client = MagicMock()
        mock_client.search.return_value = [
            _make_search_song_result(video_id="s1"),
            _make_search_song_result(video_id="s2"),
        ]
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        results = api.search_all("songs only")

        assert len(results.tracks) == 2
        assert results.albums == []
        assert results.artists == []
        assert results.playlists == []
