"""YouTube Music API wrapper using ytmusicapi.

Converts raw ytmusicapi response dicts into typed dataclasses
(Track, PlaylistInfo, SearchResults, HomeSection) for consumption
by the TUI layer.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import TYPE_CHECKING, Any, cast

from ytmusicapi import YTMusic

from ytmusic_tui.queue import Track

if TYPE_CHECKING:
    from pathlib import Path


# ---------------------------------------------------------------------------
# Supporting dataclasses
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class RelatedArtist:
    """Lightweight artist reference (e.g. from "related artists" section)."""

    channel_id: str
    name: str
    thumbnail_url: str = ""


@dataclass(frozen=True)
class AlbumInfo:
    """Album metadata with track listing."""

    browse_id: str
    title: str
    artist: str
    year: str = ""
    tracks: list[Track] = field(default_factory=list)
    thumbnail_url: str = ""


@dataclass(frozen=True)
class ArtistInfo:
    """Artist page data: top songs, albums, and related artists."""

    channel_id: str
    name: str
    description: str = ""
    top_songs: list[Track] = field(default_factory=list)
    albums: list[AlbumInfo] = field(default_factory=list)
    related_artists: list[RelatedArtist] = field(default_factory=list)
    thumbnail_url: str = ""


@dataclass(frozen=True)
class PlaylistInfo:
    """Metadata for a playlist (no track contents)."""

    playlist_id: str
    title: str
    description: str = ""
    track_count: int = 0
    thumbnail_url: str = ""


@dataclass(frozen=True)
class SearchResults:
    """Categorized search results across all content types."""

    tracks: list[Track] = field(default_factory=list)
    albums: list[AlbumInfo] = field(default_factory=list)
    artists: list[RelatedArtist] = field(default_factory=list)
    playlists: list[PlaylistInfo] = field(default_factory=list)


@dataclass(frozen=True)
class HomeSection:
    """A section on the home page (e.g. "Quick picks")."""

    title: str
    items: list[Track | PlaylistInfo] = field(default_factory=list)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def parse_duration(raw: str | None) -> float:
    """Parse a duration string into seconds.

    Accepts formats like "3:45" (M:SS), "1:02:30" (H:MM:SS),
    or just "45" (seconds only). Returns 0.0 for None or empty string.
    """
    if not raw:
        return 0.0

    parts = raw.split(":")
    try:
        int_parts = [int(p) for p in parts]
    except ValueError:
        return 0.0

    if len(int_parts) == 1:
        return float(int_parts[0])
    if len(int_parts) == 2:
        return float(int_parts[0] * 60 + int_parts[1])
    if len(int_parts) == 3:
        return float(int_parts[0] * 3600 + int_parts[1] * 60 + int_parts[2])

    return 0.0


def _pick_largest_thumbnail(thumbnails: list[dict[str, Any]] | None) -> str:
    """Return the URL of the largest thumbnail, or empty string."""
    if not thumbnails:
        return ""

    best = max(thumbnails, key=lambda t: t.get("width", 0))
    return str(best.get("url", ""))


def _join_artists(artists: list[dict[str, Any]] | None) -> str:
    """Join artist names with ", ". Returns empty string if missing."""
    if not artists:
        return ""
    return ", ".join(a.get("name", "") for a in artists)


def _extract_duration(item: dict[str, Any]) -> float:
    """Extract duration in seconds from an API response item.

    Prefers the numeric ``duration_seconds`` field when available,
    falling back to parsing the ``duration`` string.
    """
    duration_sec = item.get("duration_seconds")
    if duration_sec is not None:
        try:
            return float(duration_sec)
        except (ValueError, TypeError):
            pass

    return parse_duration(item.get("duration"))


def _dict_to_track(item: dict[str, Any]) -> Track | None:
    """Convert a raw ytmusicapi dict into a Track, or None if invalid."""
    video_id = item.get("videoId")
    if not video_id:
        return None

    album_data = item.get("album")
    album_name = ""
    if isinstance(album_data, dict):
        album_name = album_data.get("name", "")

    return Track(
        video_id=video_id,
        title=item.get("title", ""),
        artist=_join_artists(item.get("artists")),
        album=album_name,
        duration_seconds=_extract_duration(item),
        thumbnail_url=_pick_largest_thumbnail(item.get("thumbnails")),
    )


def _dict_to_album_track(item: dict[str, Any], album_artist: str = "") -> Track | None:
    """Convert a raw album track dict into a Track.

    Album tracks from get_album() use a slightly different schema:
    the artist info may live under "artists" or be inherited from
    the album-level artist.
    """
    video_id = item.get("videoId")
    if not video_id:
        return None

    album_data = item.get("album")
    album_name = ""
    if isinstance(album_data, dict):
        album_name = album_data.get("name", "")

    artist = _join_artists(item.get("artists"))
    if not artist:
        artist = album_artist

    return Track(
        video_id=video_id,
        title=item.get("title", ""),
        artist=artist,
        album=album_name,
        duration_seconds=_extract_duration(item),
        thumbnail_url=_pick_largest_thumbnail(item.get("thumbnails")),
    )


def _dict_to_album_info(item: dict[str, Any]) -> AlbumInfo | None:
    """Convert a raw ytmusicapi album dict (from artist page) into AlbumInfo."""
    browse_id = item.get("browseId")
    if not browse_id:
        return None

    return AlbumInfo(
        browse_id=browse_id,
        title=item.get("title", ""),
        artist=_join_artists(item.get("artists")),
        year=str(item.get("year", "")),
        thumbnail_url=_pick_largest_thumbnail(item.get("thumbnails")),
    )


def _dict_to_related_artist(item: dict[str, Any]) -> RelatedArtist | None:
    """Convert a raw artist dict from the 'related' section."""
    channel_id = item.get("browseId") or item.get("channelId")
    if not channel_id:
        return None

    name = item.get("title", "") or item.get("name", "")
    return RelatedArtist(
        channel_id=channel_id,
        name=name,
        thumbnail_url=_pick_largest_thumbnail(item.get("thumbnails")),
    )


def _dict_to_playlist_info(item: dict[str, Any]) -> PlaylistInfo | None:
    """Convert a raw ytmusicapi dict into a PlaylistInfo, or None if invalid."""
    playlist_id = item.get("playlistId")
    if not playlist_id:
        return None

    count_raw = item.get("count", 0)
    try:
        track_count = int(count_raw) if count_raw is not None else 0
    except (ValueError, TypeError):
        track_count = 0

    return PlaylistInfo(
        playlist_id=playlist_id,
        title=item.get("title", ""),
        description=item.get("description", "") or "",
        track_count=track_count,
        thumbnail_url=_pick_largest_thumbnail(item.get("thumbnails")),
    )


# ---------------------------------------------------------------------------
# Main API class
# ---------------------------------------------------------------------------


class MusicAPI:
    """Authenticated wrapper around ytmusicapi.YTMusic.

    Converts raw API responses into typed dataclasses for the TUI layer.
    """

    def __init__(self, auth_path: str | Path) -> None:
        """Create an authenticated YTMusic client.

        Args:
            auth_path: Path to the browser authentication JSON file.
        """
        self._client = YTMusic(str(auth_path))

    # ------------------------------------------------------------------
    # Session
    # ------------------------------------------------------------------

    def is_session_valid(self) -> bool:
        """Best-effort check that the cookies still carry a signed-in session.

        When browser cookies go stale, YouTube serves logged-out pages
        with HTTP 200 instead of raising auth errors: library endpoints
        silently come back empty. Requesting the account info is a cheap
        canary because it only parses for a signed-in session.
        """
        try:
            self._client.get_account_info()
        except (KeyError, TypeError):
            # Signed-out responses lack the account header structure.
            return False
        except Exception:
            # Network or transient errors: cannot verify, assume valid.
            return True
        return True

    # ------------------------------------------------------------------
    # Search
    # ------------------------------------------------------------------

    def search(self, query: str, filter: str | None = None, limit: int = 20) -> list[Track]:
        """Search YouTube Music.

        Args:
            query: Search string.
            filter: One of 'songs', 'albums', 'playlists', 'artists',
                    or None for all result types.
            limit: Maximum number of results.

        Returns:
            List of Track objects from song results. Non-song results
            (artists, albums without videoId) are filtered out.
        """
        # ytmusicapi types `filter` as a Literal; this wrapper deliberately
        # accepts plain str | None, so cast at the client boundary.
        raw_results: list[dict[str, Any]] = self._client.search(
            query, filter=cast("Any", filter), limit=limit
        )

        tracks: list[Track] = []
        for item in raw_results:
            track = _dict_to_track(item)
            if track is not None:
                tracks.append(track)

        return tracks

    def search_all(self, query: str, limit: int = 10, filter: str | None = None) -> SearchResults:
        """Search across all categories and return categorized results.

        Calls the ytmusicapi search, then categorizes each result by its
        ``resultType`` field.

        Args:
            query: Search string.
            limit: Maximum number of results to request.
            filter: Optional category restriction passed straight to
                ytmusicapi ('songs', 'albums', 'artists', 'playlists').
                When given, only the matching SearchResults field is
                populated.

        Returns:
            SearchResults with tracks, albums, artists, and playlists.
        """
        # ytmusicapi types `filter` as a Literal; this wrapper deliberately
        # accepts plain str | None, so cast at the client boundary.
        raw_results: list[dict[str, Any]] = self._client.search(
            query, filter=cast("Any", filter), limit=limit
        )

        tracks: list[Track] = []
        albums: list[AlbumInfo] = []
        artists: list[RelatedArtist] = []
        playlists: list[PlaylistInfo] = []

        for item in raw_results:
            result_type = item.get("resultType", "")

            if result_type == "song" or result_type == "video":
                track = _dict_to_track(item)
                if track is not None:
                    tracks.append(track)

            elif result_type == "album":
                album = _dict_to_album_info(item)
                if album is not None:
                    albums.append(album)

            elif result_type == "artist":
                artist = _dict_to_related_artist(item)
                if artist is not None:
                    artists.append(artist)

            elif result_type == "playlist":
                playlist = _dict_to_playlist_info(item)
                if playlist is not None:
                    playlists.append(playlist)

        return SearchResults(
            tracks=tracks,
            albums=albums,
            artists=artists,
            playlists=playlists,
        )

    # ------------------------------------------------------------------
    # Library
    # ------------------------------------------------------------------

    def get_library_playlists(self, limit: int = 25) -> list[PlaylistInfo]:
        """Get the user's library playlists.

        Args:
            limit: Maximum number of playlists to return.

        Returns:
            List of PlaylistInfo objects.
        """
        raw_playlists: list[dict[str, Any]] = self._client.get_library_playlists(limit=limit)

        playlists: list[PlaylistInfo] = []
        for item in raw_playlists:
            info = _dict_to_playlist_info(item)
            if info is not None:
                playlists.append(info)

        return playlists

    # ------------------------------------------------------------------
    # Playlist tracks
    # ------------------------------------------------------------------

    def get_playlist_tracks(self, playlist_id: str) -> list[Track]:
        """Get all tracks in a playlist.

        Args:
            playlist_id: The playlist ID (e.g. "VLPL_abc123").

        Returns:
            List of Track objects. Unavailable tracks are skipped.
        """
        raw_playlist: dict[str, Any] = self._client.get_playlist(playlist_id)
        raw_tracks: list[dict[str, Any]] = raw_playlist.get("tracks") or []

        tracks: list[Track] = []
        for item in raw_tracks:
            track = _dict_to_track(item)
            if track is not None:
                tracks.append(track)

        return tracks

    # ------------------------------------------------------------------
    # Home
    # ------------------------------------------------------------------

    def get_home(self) -> list[HomeSection]:
        """Get home page recommendations.

        Returns:
            List of HomeSection objects, each containing a mix of
            Track and PlaylistInfo items.
        """
        raw_sections: list[dict[str, Any]] = self._client.get_home()

        sections: list[HomeSection] = []
        for section in raw_sections:
            contents = section.get("contents")
            if contents is None:
                continue

            items: list[Track | PlaylistInfo] = []
            for item in contents:
                result_type = item.get("resultType", "")

                if result_type == "playlist" or ("playlistId" in item and "videoId" not in item):
                    playlist_info = _dict_to_playlist_info(item)
                    if playlist_info is not None:
                        items.append(playlist_info)
                elif "browseId" in item and "videoId" not in item:
                    # Album-like items (browseId + audioPlaylistId, no videoId)
                    album = _dict_to_album_info(item)
                    if album is not None:
                        items.append(
                            PlaylistInfo(
                                playlist_id=item.get("audioPlaylistId", album.browse_id),
                                title=album.title,
                                description=album.artist,
                                thumbnail_url=album.thumbnail_url,
                            )
                        )
                else:
                    track = _dict_to_track(item)
                    if track is not None:
                        items.append(track)

            sections.append(
                HomeSection(
                    title=section.get("title", ""),
                    items=items,
                )
            )

        return sections

    # ------------------------------------------------------------------
    # Library albums
    # ------------------------------------------------------------------

    def get_library_albums(self, limit: int = 25) -> list[AlbumInfo]:
        """Get the user's saved/liked albums.

        Args:
            limit: Maximum number of albums to return.

        Returns:
            List of AlbumInfo objects (without track listings).
        """
        raw_albums: list[dict[str, Any]] = self._client.get_library_albums(limit=limit)

        albums: list[AlbumInfo] = []
        for item in raw_albums:
            album = _dict_to_album_info(item)
            if album is not None:
                albums.append(album)

        return albums

    # ------------------------------------------------------------------
    # Library artists
    # ------------------------------------------------------------------

    def get_library_artists(self, limit: int = 25) -> list[ArtistInfo]:
        """Get the user's followed/subscribed artists.

        Args:
            limit: Maximum number of artists to return.

        Returns:
            List of ArtistInfo objects (simplified, without top songs
            or albums; only channel_id and name are populated).
        """
        raw_artists: list[dict[str, Any]] = self._client.get_library_artists(limit=limit)

        artists: list[ArtistInfo] = []
        for item in raw_artists:
            channel_id = item.get("browseId") or ""
            name = item.get("artist") or item.get("name") or ""
            if not channel_id:
                continue

            artists.append(
                ArtistInfo(
                    channel_id=channel_id,
                    name=name,
                    thumbnail_url=_pick_largest_thumbnail(item.get("thumbnails")),
                )
            )

        return artists

    # ------------------------------------------------------------------
    # Liked songs
    # ------------------------------------------------------------------

    def get_liked_songs(self, limit: int = 100) -> list[Track]:
        """Get the user's liked/thumbs-up songs.

        Args:
            limit: Maximum number of liked songs to return.

        Returns:
            List of Track objects.
        """
        raw_response: dict[str, Any] = self._client.get_liked_songs(limit=limit)
        raw_tracks: list[dict[str, Any]] = raw_response.get("tracks") or []

        tracks: list[Track] = []
        for item in raw_tracks:
            track = _dict_to_track(item)
            if track is not None:
                tracks.append(track)

        return tracks

    # ------------------------------------------------------------------
    # Lyrics
    # ------------------------------------------------------------------

    def get_lyrics(self, video_id: str) -> str | None:
        """Fetch lyrics for a track."""
        try:
            watch = self._client.get_watch_playlist(video_id)
            lyrics_id = watch.get("lyrics")
            if not lyrics_id or not isinstance(lyrics_id, str):
                return None
            lyrics_data = self._client.get_lyrics(lyrics_id)
            if isinstance(lyrics_data, dict):
                return lyrics_data.get("lyrics") or None
        except Exception:
            pass
        return None

    # ------------------------------------------------------------------
    # Playlist mutation
    # ------------------------------------------------------------------

    def create_playlist(
        self, title: str, description: str = "", privacy: str = "PRIVATE"
    ) -> str | None:
        """Create a new playlist."""
        try:
            result = self._client.create_playlist(title, description, privacy_status=privacy)
            if isinstance(result, str):
                return result
        except Exception:
            pass
        return None

    def add_playlist_items(self, playlist_id: str, video_ids: list[str]) -> bool:
        """Add tracks to an existing playlist."""
        try:
            result = self._client.add_playlist_items(playlist_id, video_ids)
            if isinstance(result, dict) and result.get("status") == "STATUS_SUCCEEDED":
                return True
            if isinstance(result, str) and result == "STATUS_SUCCEEDED":
                return True
        except Exception:
            pass
        return False

    def remove_playlist_items(self, playlist_id: str, video_ids: list[str]) -> bool:
        """Remove tracks from a playlist."""
        try:
            playlist_data = self._client.get_playlist(playlist_id)
            raw_tracks = playlist_data.get("tracks") or []
            to_remove = []
            target_set = set(video_ids)
            for t in raw_tracks:
                vid = t.get("videoId")
                if vid in target_set:
                    set_vid = t.get("setVideoId")
                    if set_vid:
                        to_remove.append({"videoId": vid, "setVideoId": set_vid})
                        target_set.discard(vid)
                if not target_set:
                    break
            if not to_remove:
                return False
            result = self._client.remove_playlist_items(playlist_id, to_remove)
            if isinstance(result, str) and result == "STATUS_SUCCEEDED":
                return True
            if isinstance(result, dict) and result.get("status") == "STATUS_SUCCEEDED":
                return True
        except Exception:
            pass
        return False

    # ------------------------------------------------------------------
    # Album
    # ------------------------------------------------------------------

    def get_album(self, browse_id: str) -> AlbumInfo:
        """Get album details and tracks.

        Args:
            browse_id: The album browse ID (e.g. "MPREb_abc123").

        Returns:
            AlbumInfo with parsed tracks.
        """
        raw: dict[str, Any] = self._client.get_album(browse_id)

        artist = _join_artists(raw.get("artists"))
        title = raw.get("title", "")
        year = str(raw.get("year", ""))
        thumbnail_url = _pick_largest_thumbnail(raw.get("thumbnails"))

        raw_tracks: list[dict[str, Any]] = raw.get("tracks") or []
        tracks: list[Track] = []
        for item in raw_tracks:
            track = _dict_to_album_track(item, album_artist=artist)
            if track is not None:
                tracks.append(track)

        return AlbumInfo(
            browse_id=browse_id,
            title=title,
            artist=artist,
            year=year,
            tracks=tracks,
            thumbnail_url=thumbnail_url,
        )

    # ------------------------------------------------------------------
    # Artist
    # ------------------------------------------------------------------

    def get_artist(self, channel_id: str) -> ArtistInfo:
        """Get artist page: top songs, albums, related artists.

        Args:
            channel_id: The artist channel ID.

        Returns:
            ArtistInfo with parsed sections.
        """
        raw: dict[str, Any] = self._client.get_artist(channel_id)

        name = raw.get("name", "")
        description = raw.get("description", "") or ""
        thumbnail_url = _pick_largest_thumbnail(raw.get("thumbnails"))

        # Top songs: raw["songs"]["results"]
        top_songs: list[Track] = []
        songs_section = raw.get("songs")
        if isinstance(songs_section, dict):
            for item in songs_section.get("results") or []:
                track = _dict_to_track(item)
                if track is not None:
                    top_songs.append(track)

        # Albums: raw["albums"]["results"]
        albums: list[AlbumInfo] = []
        albums_section = raw.get("albums")
        if isinstance(albums_section, dict):
            for item in albums_section.get("results") or []:
                album = _dict_to_album_info(item)
                if album is not None:
                    albums.append(album)

        # Related artists: raw["related"]["results"]
        related: list[RelatedArtist] = []
        related_section = raw.get("related")
        if isinstance(related_section, dict):
            for item in related_section.get("results") or []:
                artist_ref = _dict_to_related_artist(item)
                if artist_ref is not None:
                    related.append(artist_ref)

        return ArtistInfo(
            channel_id=channel_id,
            name=name,
            description=description,
            top_songs=top_songs,
            albums=albums,
            related_artists=related,
            thumbnail_url=thumbnail_url,
        )
