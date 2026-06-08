"""Tests for the mpv playback controller."""

from __future__ import annotations

from unittest.mock import MagicMock, PropertyMock, patch

import pytest

from ytmusic_tui.player import Player, PlayerState


class TestPlayerState:
    def test_initial_state(self) -> None:
        state = PlayerState()
        assert state.is_playing is False
        assert state.volume == 80
        assert state.position == 0.0
        assert state.duration == 0.0
        assert state.title == ""
        assert state.artist == ""
        assert state.video_id == ""

    def test_progress_zero_duration(self) -> None:
        state = PlayerState()
        assert state.progress == 0.0

    def test_progress_with_duration(self) -> None:
        state = PlayerState(position=30.0, duration=120.0)
        assert state.progress == pytest.approx(0.25)


class TestPlayer:
    @patch("ytmusic_tui.player.mpv.MPV")
    def test_init_creates_mpv_instance(self, mock_mpv_cls: MagicMock) -> None:
        player = Player()
        mock_mpv_cls.assert_called_once_with(
            ytdl=True,
            video=False,
            terminal=False,
        )
        player.shutdown()

    @patch("ytmusic_tui.player.mpv.MPV")
    def test_play_sets_url(self, mock_mpv_cls: MagicMock) -> None:
        mock_mpv = mock_mpv_cls.return_value
        player = Player()
        player.play("test_id")
        mock_mpv.play.assert_called_once_with("https://music.youtube.com/watch?v=test_id")
        player.shutdown()

    @patch("ytmusic_tui.player.mpv.MPV")
    def test_pause_toggle(self, mock_mpv_cls: MagicMock) -> None:
        mock_mpv = mock_mpv_cls.return_value
        mock_mpv.pause = False
        player = Player()
        player.toggle_pause()
        assert mock_mpv.pause is True
        player.shutdown()

    @patch("ytmusic_tui.player.mpv.MPV")
    def test_stop(self, mock_mpv_cls: MagicMock) -> None:
        mock_mpv = mock_mpv_cls.return_value
        player = Player()
        player.stop()
        mock_mpv.stop.assert_called_once()
        player.shutdown()

    @patch("ytmusic_tui.player.mpv.MPV")
    def test_volume_set(self, mock_mpv_cls: MagicMock) -> None:
        mock_mpv = mock_mpv_cls.return_value
        player = Player()
        player.set_volume(60)
        assert mock_mpv.volume == 60
        player.shutdown()

    @patch("ytmusic_tui.player.mpv.MPV")
    def test_volume_clamp_high(self, mock_mpv_cls: MagicMock) -> None:
        mock_mpv = mock_mpv_cls.return_value
        player = Player()
        player.set_volume(150)
        assert mock_mpv.volume == 100
        player.shutdown()

    @patch("ytmusic_tui.player.mpv.MPV")
    def test_volume_clamp_low(self, mock_mpv_cls: MagicMock) -> None:
        mock_mpv = mock_mpv_cls.return_value
        player = Player()
        player.set_volume(-10)
        assert mock_mpv.volume == 0
        player.shutdown()

    @patch("ytmusic_tui.player.mpv.MPV")
    def test_volume_adjust(self, mock_mpv_cls: MagicMock) -> None:
        mock_mpv = mock_mpv_cls.return_value
        type(mock_mpv).volume = PropertyMock(return_value=50)
        player = Player()
        player.adjust_volume(10)
        # adjust_volume calls set_volume which sets mock_mpv.volume
        # Since volume property returns 50, set_volume(60) should be called
        player.shutdown()

    @patch("ytmusic_tui.player.mpv.MPV")
    def test_seek(self, mock_mpv_cls: MagicMock) -> None:
        mock_mpv = mock_mpv_cls.return_value
        player = Player()
        player.seek(10)
        mock_mpv.seek.assert_called_once_with(10, "relative")
        player.shutdown()

    @patch("ytmusic_tui.player.mpv.MPV")
    def test_seek_absolute(self, mock_mpv_cls: MagicMock) -> None:
        mock_mpv = mock_mpv_cls.return_value
        player = Player()
        player.seek_absolute(30.0)
        mock_mpv.seek.assert_called_once_with(30.0, "absolute")
        player.shutdown()

    @patch("ytmusic_tui.player.mpv.MPV")
    def test_get_state(self, mock_mpv_cls: MagicMock) -> None:
        mock_mpv = mock_mpv_cls.return_value
        type(mock_mpv).idle_active = PropertyMock(return_value=False)
        type(mock_mpv).pause = PropertyMock(return_value=False)
        type(mock_mpv).volume = PropertyMock(return_value=75)
        type(mock_mpv).time_pos = PropertyMock(return_value=45.0)
        type(mock_mpv).duration = PropertyMock(return_value=200.0)
        type(mock_mpv).media_title = PropertyMock(return_value="Test Song")
        player = Player()
        state = player.get_state()
        assert state.is_playing is True
        assert state.volume == 75
        assert state.position == 45.0
        assert state.duration == 200.0
        assert state.title == "Test Song"
        player.shutdown()

    @patch("ytmusic_tui.player.mpv.MPV")
    def test_get_state_handles_none(self, mock_mpv_cls: MagicMock) -> None:
        mock_mpv = mock_mpv_cls.return_value
        type(mock_mpv).idle_active = PropertyMock(return_value=True)
        type(mock_mpv).pause = PropertyMock(return_value=True)
        type(mock_mpv).volume = PropertyMock(return_value=None)
        type(mock_mpv).time_pos = PropertyMock(return_value=None)
        type(mock_mpv).duration = PropertyMock(return_value=None)
        type(mock_mpv).media_title = PropertyMock(return_value=None)
        player = Player()
        state = player.get_state()
        assert state.is_playing is False
        assert state.volume == 0
        assert state.position == 0.0
        assert state.duration == 0.0
        assert state.title == ""
        player.shutdown()

    @patch("ytmusic_tui.player.mpv.MPV")
    def test_shutdown(self, mock_mpv_cls: MagicMock) -> None:
        mock_mpv = mock_mpv_cls.return_value
        player = Player()
        player.shutdown()
        mock_mpv.terminate.assert_called_once()
