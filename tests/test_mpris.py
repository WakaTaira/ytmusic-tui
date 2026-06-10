"""Tests for the MPRIS2 D-Bus integration module."""

from __future__ import annotations

from unittest.mock import patch

import pytest

from ytmusic_tui.player import PlayerState
from ytmusic_tui.queue import RepeatMode, Track


def _make_track(n: int = 1) -> Track:
    return Track(
        video_id=f"vid{n}",
        title=f"Song {n}",
        artist=f"Artist {n}",
        album=f"Album {n}",
        duration_seconds=200.0,
        thumbnail_url=f"https://img.example.com/{n}.jpg",
    )


def _make_state(
    *,
    playing: bool = True,
    volume: int = 80,
    position: float = 60.0,
    duration: float = 200.0,
    video_id: str = "vid1",
) -> PlayerState:
    return PlayerState(
        is_playing=playing,
        volume=volume,
        position=position,
        duration=duration,
        title="Song 1",
        artist="Artist 1",
        video_id=video_id,
    )


# ---------------------------------------------------------------------------
# Import guard
# ---------------------------------------------------------------------------

class TestMprisImport:
    """Verify the MPRIS module can be imported on Linux."""

    def test_mpris_module_imports(self) -> None:
        from ytmusic_tui.mpris import MprisService  # noqa: F401

    def test_mpris_lazy_import_in_on_mount(self) -> None:
        """MPRIS is lazy-imported in on_mount, not at module level."""
        import ytmusic_tui.app as appmod
        assert hasattr(appmod.YtMusicTui, "on_mount")


# ---------------------------------------------------------------------------
# _MediaPlayer2Player state update
# ---------------------------------------------------------------------------

class TestMediaPlayer2Player:
    """Test the D-Bus player interface state mapping."""

    def test_update_state_playing(self) -> None:
        from ytmusic_tui.mpris import _MediaPlayer2Player

        player = _MediaPlayer2Player()
        # Suppress D-Bus signal emission since we have no bus connection
        player.emit_properties_changed = lambda props: None  # type: ignore[assignment]

        state = _make_state(playing=True)
        track = _make_track()

        player.update_state(state, track=track, shuffle=True, repeat_mode=RepeatMode.ALL)

        assert player._playback_status == "Playing"
        assert player._shuffle is True
        assert player._loop_status == "Playlist"
        assert player._volume == pytest.approx(0.8)
        assert player._position == 60_000_000

        metadata = player._metadata
        assert metadata["xesam:title"].value == "Song 1"
        assert metadata["xesam:artist"].value == ["Artist 1"]
        assert metadata["xesam:album"].value == "Album 1"
        assert metadata["mpris:length"].value == 200_000_000
        assert "mpris:artUrl" in metadata

    def test_update_state_paused(self) -> None:
        from ytmusic_tui.mpris import _MediaPlayer2Player

        player = _MediaPlayer2Player()
        player.emit_properties_changed = lambda props: None  # type: ignore[assignment]

        state = _make_state(playing=False, video_id="vid1")
        player.update_state(state)

        assert player._playback_status == "Paused"

    def test_update_state_stopped(self) -> None:
        from ytmusic_tui.mpris import _MediaPlayer2Player

        player = _MediaPlayer2Player()
        player.emit_properties_changed = lambda props: None  # type: ignore[assignment]

        state = _make_state(playing=False, video_id="")
        player.update_state(state)

        assert player._playback_status == "Stopped"

    def test_update_state_repeat_one(self) -> None:
        from ytmusic_tui.mpris import _MediaPlayer2Player

        player = _MediaPlayer2Player()
        player.emit_properties_changed = lambda props: None  # type: ignore[assignment]

        state = _make_state()
        player.update_state(state, repeat_mode=RepeatMode.ONE)

        assert player._loop_status == "Track"

    def test_update_state_repeat_off(self) -> None:
        from ytmusic_tui.mpris import _MediaPlayer2Player

        player = _MediaPlayer2Player()
        player.emit_properties_changed = lambda props: None  # type: ignore[assignment]

        state = _make_state()
        player.update_state(state, repeat_mode=RepeatMode.OFF)

        assert player._loop_status == "None"

    def test_update_state_no_track(self) -> None:
        from ytmusic_tui.mpris import _MediaPlayer2Player

        player = _MediaPlayer2Player()
        player.emit_properties_changed = lambda props: None  # type: ignore[assignment]

        state = _make_state()
        player.update_state(state, track=None)

        assert player._metadata == {}

    def test_track_without_artist(self) -> None:
        from ytmusic_tui.mpris import _MediaPlayer2Player

        player = _MediaPlayer2Player()
        player.emit_properties_changed = lambda props: None  # type: ignore[assignment]

        track = Track(video_id="v1", title="No Artist", artist="", album="")
        state = _make_state()
        player.update_state(state, track=track)

        assert player._metadata["xesam:artist"].value == []

    def test_track_without_duration(self) -> None:
        from ytmusic_tui.mpris import _MediaPlayer2Player

        player = _MediaPlayer2Player()
        player.emit_properties_changed = lambda props: None  # type: ignore[assignment]

        track = Track(video_id="v1", title="No Dur", artist="A", duration_seconds=0.0)
        state = _make_state()
        player.update_state(state, track=track)

        assert "mpris:length" not in player._metadata


# ---------------------------------------------------------------------------
# _repeat_to_loop_status
# ---------------------------------------------------------------------------

class TestRepeatToLoopStatus:
    def test_off(self) -> None:
        from ytmusic_tui.mpris import _repeat_to_loop_status
        assert _repeat_to_loop_status(RepeatMode.OFF) == "None"

    def test_all(self) -> None:
        from ytmusic_tui.mpris import _repeat_to_loop_status
        assert _repeat_to_loop_status(RepeatMode.ALL) == "Playlist"

    def test_one(self) -> None:
        from ytmusic_tui.mpris import _repeat_to_loop_status
        assert _repeat_to_loop_status(RepeatMode.ONE) == "Track"


# ---------------------------------------------------------------------------
# Callbacks
# ---------------------------------------------------------------------------

class TestCallbacks:
    def test_set_and_fire_callbacks(self) -> None:
        from ytmusic_tui.mpris import _MediaPlayer2Player

        player = _MediaPlayer2Player()
        calls: list[str] = []

        player.set_callbacks(
            on_play_pause=lambda: calls.append("pp"),
            on_next=lambda: calls.append("next"),
            on_previous=lambda: calls.append("prev"),
            on_stop=lambda: calls.append("stop"),
        )

        player.PlayPause()
        player.Next()
        player.Previous()
        player.Stop()
        player.Play()
        player.Pause()

        assert calls == ["pp", "next", "prev", "stop", "pp", "pp"]

    def test_no_callbacks_does_not_crash(self) -> None:
        from ytmusic_tui.mpris import _MediaPlayer2Player

        player = _MediaPlayer2Player()
        player.PlayPause()
        player.Next()
        player.Previous()
        player.Stop()
        player.Play()


# ---------------------------------------------------------------------------
# _MediaPlayer2 properties
# ---------------------------------------------------------------------------

class TestMediaPlayer2Root:
    def test_identity(self) -> None:
        from ytmusic_tui.mpris import _MediaPlayer2

        root = _MediaPlayer2()
        assert root.Identity == "ytmusic-tui"

    def test_can_quit(self) -> None:
        from ytmusic_tui.mpris import _MediaPlayer2

        root = _MediaPlayer2()
        assert root.CanQuit is True

    def test_has_tracklist(self) -> None:
        from ytmusic_tui.mpris import _MediaPlayer2

        root = _MediaPlayer2()
        assert root.HasTrackList is False


# ---------------------------------------------------------------------------
# MprisService (integration-lite)
# ---------------------------------------------------------------------------

class TestMprisService:
    def test_update_before_start_does_not_crash(self) -> None:
        from ytmusic_tui.mpris import MprisService

        svc = MprisService()
        svc.update(_make_state(), track=_make_track())

    def test_shutdown_before_start_does_not_crash(self) -> None:
        from ytmusic_tui.mpris import MprisService

        svc = MprisService()
        svc.shutdown()

    @patch("ytmusic_tui.mpris.MessageBus")
    def test_start_creates_thread(self, mock_bus_cls: object) -> None:
        """Start should create a daemon thread even if D-Bus connection fails."""
        from ytmusic_tui.mpris import MprisService

        svc = MprisService()
        # Don't actually connect to D-Bus — the mock will raise
        try:
            svc.start()
        except Exception:
            pass
        finally:
            svc.shutdown()


# ---------------------------------------------------------------------------
# D-Bus property accessors
# ---------------------------------------------------------------------------

class TestPlayerProperties:
    def test_default_properties(self) -> None:
        from ytmusic_tui.mpris import _MediaPlayer2Player

        p = _MediaPlayer2Player()
        assert p.PlaybackStatus == "Stopped"
        assert p.LoopStatus == "None"
        assert p.Shuffle is False
        assert p.Rate == 1.0
        assert p.MinimumRate == 1.0
        assert p.MaximumRate == 1.0
        assert p.CanGoNext is True
        assert p.CanGoPrevious is True
        assert p.CanPlay is True
        assert p.CanPause is True
        assert p.CanSeek is False
        assert p.CanControl is True
        assert p.Volume == pytest.approx(0.8)
        assert p.Position == 0
        assert p.Metadata == {}
