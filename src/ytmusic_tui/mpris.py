"""MPRIS2 D-Bus interface for desktop integration.

Exposes playback state over the org.mpris.MediaPlayer2 interface so
that tools like playerctl, waybar, and KDE Connect can display and
control ytmusic-tui.

Runs its own asyncio event loop in a daemon thread so it does not
block the Textual event loop.
"""

import asyncio
import threading
from typing import Any

from dbus_next import BusType, PropertyAccess, Variant
from dbus_next.aio import MessageBus
from dbus_next.service import ServiceInterface, dbus_property, method, signal

from ytmusic_tui.player import PlayerState
from ytmusic_tui.queue import RepeatMode, Track


_BUS_NAME = "org.mpris.MediaPlayer2.ytmusic_tui"
_OBJECT_PATH = "/org/mpris/MediaPlayer2"


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
    def CanQuit(self) -> "b":  # type: ignore[override]  # noqa: N802
        return True

    @dbus_property(access=PropertyAccess.READ)
    def CanRaise(self) -> "b":  # type: ignore[override]  # noqa: N802
        return False

    @dbus_property(access=PropertyAccess.READ)
    def HasTrackList(self) -> "b":  # type: ignore[override]  # noqa: N802
        return False

    @dbus_property(access=PropertyAccess.READ)
    def Identity(self) -> "s":  # type: ignore[override]  # noqa: N802
        return "ytmusic-tui"

    @dbus_property(access=PropertyAccess.READ)
    def SupportedUriSchemes(self) -> "as":  # type: ignore[override]  # noqa: N802
        return []

    @dbus_property(access=PropertyAccess.READ)
    def SupportedMimeTypes(self) -> "as":  # type: ignore[override]  # noqa: N802
        return []


def _repeat_to_loop_status(repeat_mode: RepeatMode) -> str:
    if repeat_mode is RepeatMode.ALL:
        return "Playlist"
    if repeat_mode is RepeatMode.ONE:
        return "Track"
    return "None"


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

        self._on_play_pause: Any = None
        self._on_next: Any = None
        self._on_previous: Any = None
        self._on_stop: Any = None

    def set_callbacks(
        self,
        *,
        on_play_pause: Any = None,
        on_next: Any = None,
        on_previous: Any = None,
        on_stop: Any = None,
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
    def PlaybackStatus(self) -> "s":  # type: ignore[override]  # noqa: N802
        return self._playback_status

    @dbus_property(access=PropertyAccess.READ)
    def LoopStatus(self) -> "s":  # type: ignore[override]  # noqa: N802
        return self._loop_status

    @dbus_property(access=PropertyAccess.READ)
    def Shuffle(self) -> "b":  # type: ignore[override]  # noqa: N802
        return self._shuffle

    @dbus_property(access=PropertyAccess.READ)
    def Metadata(self) -> "a{sv}":  # type: ignore[override]  # noqa: N802
        return self._metadata

    @dbus_property(access=PropertyAccess.READ)
    def Volume(self) -> "d":  # type: ignore[override]  # noqa: N802
        return self._volume

    @dbus_property(access=PropertyAccess.READ)
    def Position(self) -> "x":  # type: ignore[override]  # noqa: N802
        return self._position

    @dbus_property(access=PropertyAccess.READ)
    def Rate(self) -> "d":  # type: ignore[override]  # noqa: N802
        return 1.0

    @dbus_property(access=PropertyAccess.READ)
    def MinimumRate(self) -> "d":  # type: ignore[override]  # noqa: N802
        return 1.0

    @dbus_property(access=PropertyAccess.READ)
    def MaximumRate(self) -> "d":  # type: ignore[override]  # noqa: N802
        return 1.0

    @dbus_property(access=PropertyAccess.READ)
    def CanGoNext(self) -> "b":  # type: ignore[override]  # noqa: N802
        return True

    @dbus_property(access=PropertyAccess.READ)
    def CanGoPrevious(self) -> "b":  # type: ignore[override]  # noqa: N802
        return True

    @dbus_property(access=PropertyAccess.READ)
    def CanPlay(self) -> "b":  # type: ignore[override]  # noqa: N802
        return True

    @dbus_property(access=PropertyAccess.READ)
    def CanPause(self) -> "b":  # type: ignore[override]  # noqa: N802
        return True

    @dbus_property(access=PropertyAccess.READ)
    def CanSeek(self) -> "b":  # type: ignore[override]  # noqa: N802
        return False

    @dbus_property(access=PropertyAccess.READ)
    def CanControl(self) -> "b":  # type: ignore[override]  # noqa: N802
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
        """Push new playback state to MPRIS properties."""
        if state.is_playing:
            self._playback_status = "Playing"
        elif state.video_id:
            self._playback_status = "Paused"
        else:
            self._playback_status = "Stopped"

        self._volume = state.volume / 100.0
        self._position = int(state.position * 1_000_000)
        self._shuffle = shuffle
        self._loop_status = _repeat_to_loop_status(repeat_mode or RepeatMode.OFF)

        metadata: dict[str, Variant] = {}
        if track is not None:
            track_id = f"/org/mpris/MediaPlayer2/Track/{track.video_id}"
            metadata["mpris:trackid"] = Variant("s", track_id)
            metadata["xesam:title"] = Variant("s", track.title)
            metadata["xesam:artist"] = Variant("as", [track.artist] if track.artist else [])
            metadata["xesam:album"] = Variant("s", track.album)
            if track.duration_seconds > 0:
                metadata["mpris:length"] = Variant(
                    "x", int(track.duration_seconds * 1_000_000)
                )
            if track.thumbnail_url:
                metadata["mpris:artUrl"] = Variant("s", track.thumbnail_url)
        self._metadata = metadata

        self.emit_properties_changed(
            {
                "PlaybackStatus": self._playback_status,
                "Metadata": self._metadata,
                "Volume": self._volume,
                "Position": self._position,
                "LoopStatus": self._loop_status,
                "Shuffle": self._shuffle,
            }
        )


class MprisService:
    """Manages the MPRIS2 D-Bus service in a background thread.

    Usage::

        mpris = MprisService()
        mpris.start(on_play_pause=..., on_next=..., on_previous=..., on_stop=...)
        mpris.update(state, track=..., shuffle=..., repeat_mode=...)
        mpris.shutdown()
    """

    def __init__(self) -> None:
        self._loop: asyncio.AbstractEventLoop | None = None
        self._thread: threading.Thread | None = None
        self._player_iface: _MediaPlayer2Player | None = None
        self._bus: MessageBus | None = None
        self._started = threading.Event()
        self._stop_event: asyncio.Event | None = None

    def start(
        self,
        *,
        on_play_pause: Any = None,
        on_next: Any = None,
        on_previous: Any = None,
        on_stop: Any = None,
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
        async def serve() -> None:
            self._stop_event = asyncio.Event()
            try:
                self._bus = await MessageBus(bus_type=BusType.SESSION).connect()
            except Exception:
                self._started.set()
                return

            root_iface = _MediaPlayer2()
            player_iface = _MediaPlayer2Player()
            player_iface.set_callbacks(**self._callbacks)

            self._bus.export(_OBJECT_PATH, root_iface)
            self._bus.export(_OBJECT_PATH, player_iface)

            await self._bus.request_name(_BUS_NAME)
            self._player_iface = player_iface
            self._loop = asyncio.get_running_loop()
            self._started.set()
            await self._stop_event.wait()

        asyncio.run(serve())

    def update(
        self,
        state: PlayerState,
        track: Track | None = None,
        shuffle: bool = False,
        repeat_mode: RepeatMode | None = None,
    ) -> None:
        """Thread-safe state update from the main Textual thread."""
        iface = self._player_iface
        if iface is None:
            return

        # Update internal state directly (simple attribute assignments)
        if state.is_playing:
            iface._playback_status = "Playing"
        elif state.video_id:
            iface._playback_status = "Paused"
        else:
            iface._playback_status = "Stopped"

        iface._volume = state.volume / 100.0
        iface._position = int(state.position * 1_000_000)
        iface._shuffle = shuffle
        iface._loop_status = _repeat_to_loop_status(repeat_mode or RepeatMode.OFF)

        metadata: dict[str, Variant] = {}
        if track is not None:
            track_id = f"/org/mpris/MediaPlayer2/Track/{track.video_id}"
            metadata["mpris:trackid"] = Variant("s", track_id)
            metadata["xesam:title"] = Variant("s", track.title)
            metadata["xesam:artist"] = Variant("as", [track.artist] if track.artist else [])
            metadata["xesam:album"] = Variant("s", track.album)
            if track.duration_seconds > 0:
                metadata["mpris:length"] = Variant("x", int(track.duration_seconds * 1_000_000))
            if track.thumbnail_url:
                metadata["mpris:artUrl"] = Variant("s", track.thumbnail_url)
        iface._metadata = metadata

        # Emit property changes on the D-Bus event loop thread
        if self._loop is not None:
            try:
                self._loop.call_soon_threadsafe(
                    iface.emit_properties_changed,
                    {
                        "PlaybackStatus": iface._playback_status,
                        "Metadata": iface._metadata,
                        "Volume": iface._volume,
                    },
                )
            except RuntimeError:
                pass

    def shutdown(self) -> None:
        """Stop the MPRIS service."""
        if self._stop_event is not None and self._loop is not None:
            self._loop.call_soon_threadsafe(self._stop_event.set)
        if self._bus is not None:
            try:
                self._bus.disconnect()
            except Exception:
                pass
