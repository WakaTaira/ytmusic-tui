"""Shared test helpers.

Canonical factories for dummy tracks and a fully mocked application.
Import these instead of redefining per-file copies::

    from helpers import make_app, make_track, make_tracks
"""

from __future__ import annotations

from typing import TYPE_CHECKING
from unittest.mock import patch

from ytmusic_tui.player import PlayerState
from ytmusic_tui.queue import Track

if TYPE_CHECKING:
    from collections.abc import Callable
    from pathlib import Path
    from unittest.mock import MagicMock

    from ytmusic_tui.app import YtMusicTui


def make_track(n: int = 1) -> Track:
    """Create a dummy track with a numeric suffix."""
    return Track(
        video_id=f"vid_{n}",
        title=f"Song {n}",
        artist=f"Artist {n}",
        album=f"Album {n}",
        duration_seconds=float(180 + n),
        thumbnail_url=f"https://img.example.com/{n}.jpg",
    )


def make_tracks(count: int) -> list[Track]:
    """Create *count* dummy tracks numbered 1..count."""
    return [make_track(i) for i in range(1, count + 1)]


def make_app(
    keymap_path: Path | None = None,
    *,
    configure_api: Callable[[MagicMock], None] | None = None,
    configure_player: Callable[[MagicMock], None] | None = None,
) -> YtMusicTui:
    """Create a YtMusicTui app with fully mocked API and player.

    Every API list method returns an empty list by default and the
    player reports an idle PlayerState. Use *configure_api* /
    *configure_player* to override mock behavior for a specific test;
    they run before the app is constructed.
    """
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

        mock_player = mock_player_cls.return_value
        mock_player.get_state.return_value = PlayerState()

        if configure_api is not None:
            configure_api(mock_api)
        if configure_player is not None:
            configure_player(mock_player)

        from ytmusic_tui.app import YtMusicTui

        app = YtMusicTui(auth_path="/fake/auth.json", keymap_path=keymap_path)
        return app
