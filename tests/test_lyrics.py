"""Tests for lyrics API and LyricsView."""

from __future__ import annotations

from unittest.mock import MagicMock, patch

import pytest
from helpers import make_app

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
    def test_get_lyrics_exception_propagates(self, mock_cls: MagicMock) -> None:
        """A transport/auth error must propagate, not masquerade as
        "no lyrics" (None). The view's classify_api_error path then shows
        a real error instead of the misleading "No lyrics available"."""
        import pytest

        from ytmusic_tui.api import MusicAPI

        mock_client = mock_cls.return_value
        mock_client.get_watch_playlist.side_effect = Exception("Network error")

        api = MusicAPI("/fake/path")
        with pytest.raises(Exception, match="Network error"):
            api.get_lyrics("vid1")

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

    @pytest.mark.asyncio
    async def test_no_lyrics_shows_unavailable(self) -> None:
        """get_lyrics returning None (domain absence) must render the
        "No lyrics available" message, not an error."""
        from textual.widgets import Label

        from ytmusic_tui.views.lyrics import LyricsView

        app = make_app(configure_api=lambda api: setattr(api.get_lyrics, "return_value", None))
        async with app.run_test(size=(120, 40)) as pilot:
            view = app.query_one(LyricsView)
            view.load_lyrics("vid1", title="Song", artist="Artist")
            await app.workers.wait_for_complete()
            await pilot.pause()

            status = view.query_one("#lyrics-status", Label)
            assert "No lyrics available" in status.content

    @pytest.mark.asyncio
    async def test_fetch_error_shows_classified_message(self) -> None:
        """A transport error from get_lyrics must surface the classified
        error in the status label, never the misleading "No lyrics
        available" (which would be a lie on auth expiry)."""
        from textual.widgets import Label

        from ytmusic_tui.views.lyrics import LyricsView

        def _configure(api: MagicMock) -> None:
            api.get_lyrics.side_effect = Exception("401 Unauthorized")

        app = make_app(configure_api=_configure)
        async with app.run_test(size=(120, 40)) as pilot:
            view = app.query_one(LyricsView)
            view.load_lyrics("vid1", title="Song", artist="Artist")
            await app.workers.wait_for_complete()
            await pilot.pause()

            status = view.query_one("#lyrics-status", Label).content
            assert "Auth expired" in status
            assert "No lyrics available" not in status


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
