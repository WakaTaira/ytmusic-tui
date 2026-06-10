"""MPRIS2 D-Bus interface for desktop integration.

Exposes playback state over the org.mpris.MediaPlayer2 interface so
that tools like playerctl, waybar, and KDE Connect can display and
control ytmusic-tui.

Runs its own asyncio event loop in a daemon thread so it does not
block the Textual event loop.

Implementation notes
--------------------
* Built on dbus-fast, the maintained fork of dbus-next.  dbus-next has
  been unmaintained since 2021 and depends on
  ``typing.no_type_check_decorator``, which is removed in Python 3.15.
* dbus-fast's asyncio message writer drains its queue in a tight loop
  and treats ``BlockingIOError`` from the non-blocking ``socket.send()``
  as fatal: it silently deregisters the reader/writer without closing
  the socket or logging.  The bus name then stays registered while the
  service is deaf, so every client call blocks until its timeout (this
  is exactly the "playerctl hangs" failure mode).  EAGAIN is normal
  kernel backpressure, however, so :func:`_install_eagain_tolerant_writer`
  replaces the writer callback with a variant that resumes the write on
  the next writable event instead of tearing the connection down.
"""

import array
import asyncio
import contextlib
import logging
import socket
import threading
from collections.abc import Callable
from types import MethodType
from typing import Any

from dbus_fast import BusType, PropertyAccess, Variant
from dbus_fast.aio import MessageBus
from dbus_fast.service import ServiceInterface, dbus_property, method, signal

from ytmusic_tui.player import PlayerState
from ytmusic_tui.queue import RepeatMode, Track

_LOGGER = logging.getLogger(__name__)

_BUS_NAME = "org.mpris.MediaPlayer2.ytmusic_tui"
_OBJECT_PATH = "/org/mpris/MediaPlayer2"


# ---------------------------------------------------------------------------
# dbus-fast EAGAIN workaround
# ---------------------------------------------------------------------------


def _set_future_result(fut: asyncio.Future[Any] | None, value: Any) -> None:
    if fut is not None and not fut.done():
        fut.set_result(value)


def _set_future_exception(fut: asyncio.Future[Any] | None, exc: BaseException) -> None:
    if fut is not None and not fut.done():
        fut.set_exception(exc)


def _eagain_tolerant_write_callback(writer: Any, remove_writer: bool = True) -> None:
    """Drop-in replacement for dbus-fast's ``_MessageWriter.write_callback``.

    Mirrors dbus_fast.aio.message_bus._MessageWriter.write_callback
    (5.0.x) with one fix: ``BlockingIOError`` (EAGAIN) from ``send()``
    is treated as recoverable backpressure instead of a fatal error.

    On EAGAIN the partially written buffer is kept and the function
    simply returns.  Both call paths then resume the write correctly:

    * epoll path (``loop.add_writer``): the writer stays registered
      because it is only removed once the queue fully drains, so the
      loop re-invokes this callback when the socket becomes writable.
    * optimistic path (``schedule_write``): the caller re-registers the
      writer whenever ``writer.buf`` is left non-None.
    """
    sock = writer.sock
    try:
        while True:
            if writer.buf is None:
                if not writer.messages:
                    # Nothing more to write.
                    if remove_writer:
                        writer.loop.remove_writer(writer.fd)
                    return
                buf, unix_fds, fut = writer.messages.popleft()
                writer.unix_fds = unix_fds
                writer.buf = memoryview(buf)
                writer.offset = 0
                writer.fut = fut

            try:
                if writer.unix_fds and writer.negotiate_unix_fd:
                    ancdata = [
                        (
                            socket.SOL_SOCKET,
                            socket.SCM_RIGHTS,
                            array.array("i", writer.unix_fds),
                        )
                    ]
                    writer.offset += sock.sendmsg([writer.buf[writer.offset :]], ancdata)
                    writer.unix_fds = None
                else:
                    writer.offset += sock.send(writer.buf[writer.offset :])
            except BlockingIOError:
                # Kernel send buffer is momentarily full: keep the
                # partial buffer and resume on the next writable event.
                return

            if writer.offset < len(writer.buf):
                # Partial write: wait until the socket is writable again.
                return

            # Finished writing this message.
            writer.buf = None
            _set_future_result(writer.fut, None)
    except Exception as exc:
        if writer.bus._user_disconnect:
            _set_future_result(writer.fut, None)
        else:
            _set_future_exception(writer.fut, exc)
            _LOGGER.warning("MPRIS: D-Bus write failed, closing connection: %s", exc)
        writer.bus._finalize(exc)


def _install_eagain_tolerant_writer(bus: MessageBus) -> None:
    """Bind the EAGAIN-tolerant write callback onto the bus's writer.

    ``_MessageWriter`` is a plain Python class in
    ``dbus_fast.aio.message_bus`` (not cythonized, no ``__slots__``), so
    an instance-level override reliably shadows the original method for
    both the epoll path and the optimistic ``schedule_write`` path.
    The writer instance only exists after ``connect()``.
    """
    writer = getattr(bus, "_writer", None)
    if writer is None:
        return
    writer.write_callback = MethodType(_eagain_tolerant_write_callback, writer)


# ---------------------------------------------------------------------------
# org.mpris.MediaPlayer2 (root interface)
# ---------------------------------------------------------------------------


class _MediaPlayer2(ServiceInterface):
    """Implements org.mpris.MediaPlayer2 (root interface)."""

    def __init__(self) -> None:
        super().__init__("org.mpris.MediaPlayer2")

    @method()
    def Raise(self) -> None:  # noqa: N802
        pass

    @method()
    def Quit(self) -> None:  # noqa: N802
        pass

    @dbus_property(access=PropertyAccess.READ)
    def CanQuit(self) -> "b":  # noqa: N802
        return True

    @dbus_property(access=PropertyAccess.READ)
    def CanRaise(self) -> "b":  # noqa: N802
        return False

    @dbus_property(access=PropertyAccess.READ)
    def HasTrackList(self) -> "b":  # noqa: N802
        return False

    @dbus_property(access=PropertyAccess.READ)
    def Identity(self) -> "s":  # noqa: N802
        return "ytmusic-tui"

    @dbus_property(access=PropertyAccess.READ)
    def SupportedUriSchemes(self) -> "as":  # noqa: N802
        return []

    @dbus_property(access=PropertyAccess.READ)
    def SupportedMimeTypes(self) -> "as":  # noqa: N802
        return []


# ---------------------------------------------------------------------------
# org.mpris.MediaPlayer2.Player
# ---------------------------------------------------------------------------


def _repeat_to_loop_status(repeat_mode: RepeatMode) -> str:
    if repeat_mode is RepeatMode.ALL:
        return "Playlist"
    if repeat_mode is RepeatMode.ONE:
        return "Track"
    return "None"


def _build_metadata(track: Track | None) -> dict[str, Variant]:
    """Map a Track to the xesam/mpris metadata dictionary."""
    metadata: dict[str, Variant] = {}
    if track is None:
        return metadata
    track_id = f"/org/mpris/MediaPlayer2/Track/{track.video_id}"
    metadata["mpris:trackid"] = Variant("s", track_id)
    metadata["xesam:title"] = Variant("s", track.title)
    metadata["xesam:artist"] = Variant("as", [track.artist] if track.artist else [])
    metadata["xesam:album"] = Variant("s", track.album)
    if track.duration_seconds > 0:
        metadata["mpris:length"] = Variant("x", int(track.duration_seconds * 1_000_000))
    if track.thumbnail_url:
        metadata["mpris:artUrl"] = Variant("s", track.thumbnail_url)
    return metadata


def _metadata_equal(a: dict[str, Variant], b: dict[str, Variant]) -> bool:
    """Compare metadata dictionaries by their unwrapped values."""
    return a.keys() == b.keys() and all(a[key].value == b[key].value for key in a)


class _MediaPlayer2Player(ServiceInterface):
    """Implements org.mpris.MediaPlayer2.Player."""

    def __init__(self) -> None:
        super().__init__("org.mpris.MediaPlayer2.Player")
        self._metadata: dict[str, Variant] = {}
        self._playback_status: str = "Stopped"
        self._volume: float = 0.8
        self._position: int = 0
        self._loop_status: str = "None"
        self._shuffle: bool = False

        self._on_play_pause: Callable[[], None] | None = None
        self._on_next: Callable[[], None] | None = None
        self._on_previous: Callable[[], None] | None = None
        self._on_stop: Callable[[], None] | None = None

    def set_callbacks(
        self,
        *,
        on_play_pause: Callable[[], None] | None = None,
        on_next: Callable[[], None] | None = None,
        on_previous: Callable[[], None] | None = None,
        on_stop: Callable[[], None] | None = None,
    ) -> None:
        self._on_play_pause = on_play_pause
        self._on_next = on_next
        self._on_previous = on_previous
        self._on_stop = on_stop

    # -- Methods --

    @method()
    def Next(self) -> None:  # noqa: N802
        if self._on_next:
            self._on_next()

    @method()
    def Previous(self) -> None:  # noqa: N802
        if self._on_previous:
            self._on_previous()

    @method()
    def Pause(self) -> None:  # noqa: N802
        if self._on_play_pause:
            self._on_play_pause()

    @method()
    def PlayPause(self) -> None:  # noqa: N802
        if self._on_play_pause:
            self._on_play_pause()

    @method()
    def Stop(self) -> None:  # noqa: N802
        if self._on_stop:
            self._on_stop()

    @method()
    def Play(self) -> None:  # noqa: N802
        if self._on_play_pause:
            self._on_play_pause()

    # -- Properties (all read-only for MPRIS) --

    @dbus_property(access=PropertyAccess.READ)
    def PlaybackStatus(self) -> "s":  # noqa: N802
        return self._playback_status

    @dbus_property(access=PropertyAccess.READ)
    def LoopStatus(self) -> "s":  # noqa: N802
        return self._loop_status

    @dbus_property(access=PropertyAccess.READ)
    def Shuffle(self) -> "b":  # noqa: N802
        return self._shuffle

    @dbus_property(access=PropertyAccess.READ)
    def Metadata(self) -> "a{sv}":  # noqa: N802
        return self._metadata

    @dbus_property(access=PropertyAccess.READ)
    def Volume(self) -> "d":  # noqa: N802
        return self._volume

    @dbus_property(access=PropertyAccess.READ)
    def Position(self) -> "x":  # noqa: N802
        return self._position

    @dbus_property(access=PropertyAccess.READ)
    def Rate(self) -> "d":  # noqa: N802
        return 1.0

    @dbus_property(access=PropertyAccess.READ)
    def MinimumRate(self) -> "d":  # noqa: N802
        return 1.0

    @dbus_property(access=PropertyAccess.READ)
    def MaximumRate(self) -> "d":  # noqa: N802
        return 1.0

    @dbus_property(access=PropertyAccess.READ)
    def CanGoNext(self) -> "b":  # noqa: N802
        return True

    @dbus_property(access=PropertyAccess.READ)
    def CanGoPrevious(self) -> "b":  # noqa: N802
        return True

    @dbus_property(access=PropertyAccess.READ)
    def CanPlay(self) -> "b":  # noqa: N802
        return True

    @dbus_property(access=PropertyAccess.READ)
    def CanPause(self) -> "b":  # noqa: N802
        return True

    @dbus_property(access=PropertyAccess.READ)
    def CanSeek(self) -> "b":  # noqa: N802
        return False

    @dbus_property(access=PropertyAccess.READ)
    def CanControl(self) -> "b":  # noqa: N802
        return True

    # -- Signals --

    @signal()
    def Seeked(self) -> "x":  # noqa: N802
        return self._position

    # -- Internal state update --

    def update_state(
        self,
        state: PlayerState,
        track: Track | None = None,
        shuffle: bool = False,
        repeat_mode: RepeatMode | None = None,
    ) -> None:
        """Push new playback state to MPRIS properties.

        Only properties whose values actually changed are announced via
        PropertiesChanged, so a steady 1 Hz poll from the UI does not
        flood the bus with redundant signals.  Position is deliberately
        never emitted: the MPRIS spec has clients poll it and reserves
        the Seeked signal for jumps.

        Must run on the D-Bus event loop thread (see MprisService.update).
        """
        if state.is_playing:
            new_status = "Playing"
        elif state.video_id:
            new_status = "Paused"
        else:
            new_status = "Stopped"

        new_volume = state.volume / 100.0
        new_loop_status = _repeat_to_loop_status(repeat_mode or RepeatMode.OFF)
        new_metadata = _build_metadata(track)

        self._position = int(state.position * 1_000_000)

        changed: dict[str, Any] = {}
        if new_status != self._playback_status:
            self._playback_status = new_status
            changed["PlaybackStatus"] = new_status
        if new_volume != self._volume:
            self._volume = new_volume
            changed["Volume"] = new_volume
        if new_loop_status != self._loop_status:
            self._loop_status = new_loop_status
            changed["LoopStatus"] = new_loop_status
        if shuffle != self._shuffle:
            self._shuffle = shuffle
            changed["Shuffle"] = shuffle
        if not _metadata_equal(new_metadata, self._metadata):
            self._metadata = new_metadata
            changed["Metadata"] = new_metadata

        if changed:
            self.emit_properties_changed(changed)


# ---------------------------------------------------------------------------
# Service wrapper
# ---------------------------------------------------------------------------


class MprisService:
    """Manages the MPRIS2 D-Bus service in a background thread.

    Usage::

        mpris = MprisService()
        mpris.start(on_play_pause=..., on_next=..., on_previous=..., on_stop=...)
        mpris.update(state, track=..., shuffle=..., repeat_mode=...)
        mpris.shutdown()

    If the D-Bus connection cannot be established or dies later, the
    failure is logged and recorded in ``connection_error``; the rest of
    the application is unaffected.
    """

    def __init__(self) -> None:
        self._loop: asyncio.AbstractEventLoop | None = None
        self._thread: threading.Thread | None = None
        self._player_iface: _MediaPlayer2Player | None = None
        self._bus: MessageBus | None = None
        self._started = threading.Event()
        self._stop_event: asyncio.Event | None = None
        self._callbacks: dict[str, Callable[[], None] | None] = {}
        self.connection_error: str | None = None

    def start(
        self,
        *,
        on_play_pause: Callable[[], None] | None = None,
        on_next: Callable[[], None] | None = None,
        on_previous: Callable[[], None] | None = None,
        on_stop: Callable[[], None] | None = None,
    ) -> None:
        """Start the MPRIS service in a daemon thread."""
        self._callbacks = {
            "on_play_pause": on_play_pause,
            "on_next": on_next,
            "on_previous": on_previous,
            "on_stop": on_stop,
        }
        self._thread = threading.Thread(target=self._run, daemon=True)
        self._thread.start()

    def _run(self) -> None:
        try:
            asyncio.run(self._serve())
        except Exception as exc:
            self._record_error(exc, "MPRIS: service stopped unexpectedly")
        finally:
            self._started.set()

    async def _serve(self) -> None:
        self._stop_event = asyncio.Event()
        try:
            bus = MessageBus(bus_type=BusType.SESSION)
            await bus.connect()
        except Exception as exc:
            self._record_error(exc, "MPRIS: could not connect to session bus")
            return

        self._bus = bus
        _install_eagain_tolerant_writer(bus)

        root_iface = _MediaPlayer2()
        player_iface = _MediaPlayer2Player()
        player_iface.set_callbacks(**self._callbacks)

        bus.export(_OBJECT_PATH, root_iface)
        bus.export(_OBJECT_PATH, player_iface)
        await bus.request_name(_BUS_NAME)

        self._player_iface = player_iface
        self._loop = asyncio.get_running_loop()
        self._started.set()

        stop_task = asyncio.ensure_future(self._stop_event.wait())
        disconnect_task = asyncio.ensure_future(
            bus.wait_for_disconnect()  # type: ignore[no-untyped-call]
        )
        done, pending = await asyncio.wait(
            {stop_task, disconnect_task}, return_when=asyncio.FIRST_COMPLETED
        )
        for task in pending:
            task.cancel()
        if disconnect_task in done:
            disconnect_error = disconnect_task.exception()
            if disconnect_error is not None:
                self._record_error(disconnect_error, "MPRIS: D-Bus connection lost")
        with contextlib.suppress(Exception):
            bus.disconnect()

    def _record_error(self, exc: BaseException, message: str) -> None:
        self.connection_error = f"{type(exc).__name__}: {exc}"
        _LOGGER.warning("%s: %s", message, exc)

    def update(
        self,
        state: PlayerState,
        track: Track | None = None,
        shuffle: bool = False,
        repeat_mode: RepeatMode | None = None,
    ) -> None:
        """Thread-safe state update from the main Textual thread.

        All property mutation and signal emission happens on the D-Bus
        event loop thread, so there is no cross-thread state access.
        """
        iface = self._player_iface
        loop = self._loop
        if iface is None or loop is None:
            return
        # RuntimeError: the event loop is already closed (shutdown race).
        with contextlib.suppress(RuntimeError):
            loop.call_soon_threadsafe(iface.update_state, state, track, shuffle, repeat_mode)

    def shutdown(self) -> None:
        """Stop the MPRIS service.

        The bus disconnect itself runs on the service's event loop
        thread (in _serve) once the stop event fires.
        """
        loop = self._loop
        stop_event = self._stop_event
        if loop is None or stop_event is None:
            return
        with contextlib.suppress(RuntimeError):
            loop.call_soon_threadsafe(stop_event.set)
