"""Tests for lyrics API and LyricsView."""

from __future__ import annotations

from unittest.mock import MagicMock, patch

import pytest

from ytmusic_tui.queue import Track


def _make_track(n: int = 1) -> Track:
    return Track(
        video_id=f"vid{n}",
        title=f"Song {n}",
        artist=f"Artist {n}",
        album=f"Album {n}",
        duration_seconds=200.0,
    )


# ---------------------------------------------------------------------------
# API: get_lyrics
# ---------------------------------------------------------------------------


class TestGetLyrics:
    @patch("ytmusic_tui.api.YTMusic")
    def test_get_lyrics_success(self, mock_cls: MagicMock) -> None:
        from ytmusic_tui.api import MusicAPI

        mock_client = mock_cls.return_value
        mock_client.get_watch_playlist.return_value = {"lyrics": "LYRICS_BROWSE_ID"}
        mock_client.get_lyrics.return_value = {
            "lyrics": "Never gonna give you up\nNever gonna let you down",
            "source": "LyricFind",
        }

        api = MusicAPI("/fake/path")
        result = api.get_lyrics("vid1")

        assert result == "Never gonna give you up\nNever gonna let you down"
        mock_client.get_watch_playlist.assert_called_once_with("vid1")
        mock_client.get_lyrics.assert_called_once_with("LYRICS_BROWSE_ID")

    @patch("ytmusic_tui.api.YTMusic")
    def test_get_lyrics_no_lyrics_id(self, mock_cls: MagicMock) -> None:
        from ytmusic_tui.api import MusicAPI

        mock_client = mock_cls.return_value
        mock_client.get_watch_playlist.return_value = {}

        api = MusicAPI("/fake/path")
        result = api.get_lyrics("vid1")

        assert result is None
        mock_client.get_lyrics.assert_not_called()

    @patch("ytmusic_tui.api.YTMusic")
    def test_get_lyrics_empty_lyrics(self, mock_cls: MagicMock) -> None:
        from ytmusic_tui.api import MusicAPI

        mock_client = mock_cls.return_value
        mock_client.get_watch_playlist.return_value = {"lyrics": "LID"}
        mock_client.get_lyrics.return_value = {"lyrics": ""}

        api = MusicAPI("/fake/path")
        result = api.get_lyrics("vid1")

        assert result is None

    @patch("ytmusic_tui.api.YTMusic")
    def test_get_lyrics_none_lyrics(self, mock_cls: MagicMock) -> None:
        from ytmusic_tui.api import MusicAPI

        mock_client = mock_cls.return_value
        mock_client.get_watch_playlist.return_value = {"lyrics": "LID"}
        mock_client.get_lyrics.return_value = {"lyrics": None}

        api = MusicAPI("/fake/path")
        result = api.get_lyrics("vid1")

        assert result is None

    @patch("ytmusic_tui.api.YTMusic")
    def test_get_lyrics_exception(self, mock_cls: MagicMock) -> None:
        from ytmusic_tui.api import MusicAPI

        mock_client = mock_cls.return_value
        mock_client.get_watch_playlist.side_effect = Exception("Network error")

        api = MusicAPI("/fake/path")
        result = api.get_lyrics("vid1")

        assert result is None

    @patch("ytmusic_tui.api.YTMusic")
    def test_get_lyrics_non_dict_response(self, mock_cls: MagicMock) -> None:
        from ytmusic_tui.api import MusicAPI

        mock_client = mock_cls.return_value
        mock_client.get_watch_playlist.return_value = {"lyrics": "LID"}
        mock_client.get_lyrics.return_value = "plain string"

        api = MusicAPI("/fake/path")
        result = api.get_lyrics("vid1")

        assert result is None


# ---------------------------------------------------------------------------
# LyricsView
# ---------------------------------------------------------------------------


class TestLyricsView:
    def test_lyrics_view_imports(self) -> None:
        from ytmusic_tui.views.lyrics import LyricsView  # noqa: F401

    def test_initial_video_id_empty(self) -> None:
        from ytmusic_tui.views.lyrics import LyricsView

        view = LyricsView()
        assert view._current_video_id == ""


# ---------------------------------------------------------------------------
# App integration: keybinding
# ---------------------------------------------------------------------------


class TestLyricsKeybinding:
    def test_lyrics_in_default_keymap(self) -> None:
        from ytmusic_tui.config import DEFAULT_KEYMAP

        assert "open_lyrics" in DEFAULT_KEYMAP
        assert DEFAULT_KEYMAP["open_lyrics"] == "L"

    def test_lyrics_in_action_map(self) -> None:
        from ytmusic_tui.app import YtMusicTui

        assert "open_lyrics" in YtMusicTui._ACTION_TO_TEXTUAL
