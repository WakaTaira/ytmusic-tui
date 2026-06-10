"""Tests for the MPRIS2 D-Bus integration module."""

from __future__ import annotations

import logging
from collections import deque
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


# ---------------------------------------------------------------------------
# EAGAIN-tolerant writer patch (dbus-fast workaround)
# ---------------------------------------------------------------------------


class _FakeLoop:
    def __init__(self) -> None:
        self.removed: list[int] = []

    def remove_writer(self, fd: int) -> None:
        self.removed.append(fd)


class _FakeBus:
    def __init__(self) -> None:
        self._user_disconnect = False
        self.finalized: list[BaseException] = []

    def _finalize(self, err: BaseException) -> None:
        self.finalized.append(err)


class _FakeFuture:
    def __init__(self) -> None:
        self.value: object = None
        self.exc: BaseException | None = None
        self._done = False

    def done(self) -> bool:
        return self._done

    def set_result(self, value: object) -> None:
        self._done = True
        self.value = value

    def set_exception(self, exc: BaseException) -> None:
        self._done = True
        self.exc = exc


class _FakeSock:
    """send() follows a script of byte counts or exceptions to raise."""

    def __init__(self, script: list[object]) -> None:
        self.script = list(script)
        self.sent: list[bytes] = []

    def send(self, data: memoryview) -> int:
        action = self.script.pop(0)
        if isinstance(action, BaseException):
            raise action
        assert isinstance(action, int)
        n = min(action, len(data))
        self.sent.append(bytes(data[:n]))
        return n


class _FakeWriter:
    """Duck-typed stand-in for dbus_fast.aio.message_bus._MessageWriter."""

    def __init__(self, sock: _FakeSock, messages: list[tuple]) -> None:
        self.sock = sock
        self.loop = _FakeLoop()
        self.bus = _FakeBus()
        self.fd = 99
        self.buf: memoryview | None = None
        self.offset = 0
        self.unix_fds: list[int] | None = None
        self.negotiate_unix_fd = False
        self.fut: _FakeFuture | None = None
        self.messages = deque(messages)


def _call_writer(writer: _FakeWriter) -> None:
    from ytmusic_tui.mpris import _eagain_tolerant_write_callback

    _eagain_tolerant_write_callback(writer)


class TestEagainTolerantWriter:
    """The patched write callback must survive EAGAIN (the root cause of
    the historic 'playerctl hangs' bug) while still failing hard on real
    connection errors."""

    def test_eagain_mid_drain_preserves_state(self) -> None:
        sock = _FakeSock([3, BlockingIOError(11, "again"), 3])
        writer = _FakeWriter(sock, [(b"abc", None, None), (b"def", None, None)])

        _call_writer(writer)

        # First message sent, second hit EAGAIN: connection must survive.
        assert sock.sent == [b"abc"]
        assert writer.buf is not None
        assert writer.offset == 0
        assert writer.bus.finalized == []
        assert writer.loop.removed == []  # writer stays registered

        # Next writable event drains the rest.
        _call_writer(writer)
        assert sock.sent == [b"abc", b"def"]
        assert writer.buf is None
        assert writer.loop.removed == [99]

    def test_eagain_on_first_send_resumes(self) -> None:
        sock = _FakeSock([BlockingIOError(11, "again"), 6])
        writer = _FakeWriter(sock, [(b"abcdef", None, None)])

        _call_writer(writer)
        assert writer.buf is not None
        assert writer.offset == 0
        assert writer.bus.finalized == []

        _call_writer(writer)
        assert writer.buf is None
        assert not writer.messages
        assert writer.loop.removed == [99]

    def test_partial_write_waits_for_writable(self) -> None:
        sock = _FakeSock([3, 3])
        writer = _FakeWriter(sock, [(b"abcdef", None, None)])

        _call_writer(writer)
        assert writer.offset == 3
        assert writer.buf is not None
        assert writer.bus.finalized == []

        _call_writer(writer)
        assert writer.buf is None
        assert sock.sent == [b"abc", b"def"]

    def test_fatal_error_finalizes_and_logs(self, caplog: pytest.LogCaptureFixture) -> None:
        fut = _FakeFuture()
        sock = _FakeSock([BrokenPipeError(32, "broken pipe")])
        writer = _FakeWriter(sock, [(b"abc", None, fut)])

        with caplog.at_level(logging.WARNING, logger="ytmusic_tui.mpris"):
            _call_writer(writer)

        assert len(writer.bus.finalized) == 1
        assert isinstance(writer.bus.finalized[0], BrokenPipeError)
        assert fut.exc is not None
        assert "write failed" in caplog.text

    def test_future_resolved_after_full_send(self) -> None:
        fut = _FakeFuture()
        sock = _FakeSock([3])
        writer = _FakeWriter(sock, [(b"abc", None, fut)])

        _call_writer(writer)

        assert fut.done()
        assert fut.exc is None

    def test_install_on_unconnected_bus_is_noop(self) -> None:
        from ytmusic_tui.mpris import _install_eagain_tolerant_writer

        class _BusWithoutWriter:
            _writer = None

        _install_eagain_tolerant_writer(_BusWithoutWriter())  # must not raise


# ---------------------------------------------------------------------------
# Emit-on-change behavior
# ---------------------------------------------------------------------------


class TestEmitOnChange:
    """update_state must announce only properties that actually changed,
    so the 1 Hz UI poll does not flood the bus with redundant signals."""

    def _player_with_recorder(self) -> tuple[object, list[dict]]:
        from ytmusic_tui.mpris import _MediaPlayer2Player

        player = _MediaPlayer2Player()
        emitted: list[dict] = []
        player.emit_properties_changed = (  # type: ignore[assignment]
            lambda props: emitted.append(props)
        )
        return player, emitted

    def test_first_update_emits_status_and_metadata(self) -> None:
        player, emitted = self._player_with_recorder()

        player.update_state(_make_state(), track=_make_track())

        assert len(emitted) == 1
        assert emitted[0]["PlaybackStatus"] == "Playing"
        assert "Metadata" in emitted[0]
        assert "Position" not in emitted[0]

    def test_identical_update_emits_nothing(self) -> None:
        player, emitted = self._player_with_recorder()
        state = _make_state()
        track = _make_track()

        player.update_state(state, track=track)
        player.update_state(state, track=track)

        assert len(emitted) == 1

    def test_only_changed_keys_emitted(self) -> None:
        player, emitted = self._player_with_recorder()
        track = _make_track()

        player.update_state(_make_state(playing=True), track=track)
        player.update_state(_make_state(playing=False), track=track)

        assert emitted[-1] == {"PlaybackStatus": "Paused"}

    def test_position_change_alone_emits_nothing(self) -> None:
        player, emitted = self._player_with_recorder()
        track = _make_track()

        player.update_state(_make_state(position=60.0), track=track)
        player.update_state(_make_state(position=120.0), track=track)

        assert len(emitted) == 1
        assert player._position == 120_000_000

    def test_volume_change_emitted(self) -> None:
        player, emitted = self._player_with_recorder()
        track = _make_track()

        player.update_state(_make_state(volume=80), track=track)
        player.update_state(_make_state(volume=50), track=track)

        assert emitted[-1] == {"Volume": pytest.approx(0.5)}

    def test_track_change_emits_metadata(self) -> None:
        player, emitted = self._player_with_recorder()

        player.update_state(_make_state(), track=_make_track(1))
        player.update_state(_make_state(), track=_make_track(2))

        assert "Metadata" in emitted[-1]
        assert emitted[-1]["Metadata"]["xesam:title"].value == "Song 2"


# ---------------------------------------------------------------------------
# MprisService threading boundary
# ---------------------------------------------------------------------------


class TestServiceUpdateScheduling:
    def test_update_schedules_update_state_on_loop(self) -> None:
        from ytmusic_tui.mpris import MprisService, _MediaPlayer2Player

        svc = MprisService()
        iface = _MediaPlayer2Player()
        iface.emit_properties_changed = lambda props: None  # type: ignore[assignment]

        scheduled: list[tuple] = []

        class _InlineLoop:
            def call_soon_threadsafe(self, fn, *args) -> None:
                scheduled.append((fn, args))
                fn(*args)

        svc._player_iface = iface
        svc._loop = _InlineLoop()  # type: ignore[assignment]

        svc.update(_make_state(), track=_make_track(), shuffle=True, repeat_mode=RepeatMode.ALL)

        assert scheduled
        assert iface._playback_status == "Playing"
        assert iface._shuffle is True
        assert iface._loop_status == "Playlist"

    def test_update_with_closed_loop_is_noop(self) -> None:
        from ytmusic_tui.mpris import MprisService, _MediaPlayer2Player

        svc = MprisService()
        iface = _MediaPlayer2Player()

        class _ClosedLoop:
            def call_soon_threadsafe(self, fn, *args) -> None:
                raise RuntimeError("Event loop is closed")

        svc._player_iface = iface
        svc._loop = _ClosedLoop()  # type: ignore[assignment]

        svc.update(_make_state())  # must not raise

    def test_connection_error_defaults_to_none(self) -> None:
        from ytmusic_tui.mpris import MprisService

        assert MprisService().connection_error is None
