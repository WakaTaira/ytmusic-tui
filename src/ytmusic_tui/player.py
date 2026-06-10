"""mpv IPC playback controller."""

from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING

import mpv

if TYPE_CHECKING:
    from collections.abc import Callable


# YouTube Music URL template
_YTM_URL = "https://music.youtube.com/watch?v={video_id}"

# Volume boundaries
_VOL_MIN = 0
_VOL_MAX = 100


@dataclass
class PlayerState:
    """Snapshot of the current playback state."""

    is_playing: bool = False
    volume: int = 80
    position: float = 0.0
    duration: float = 0.0
    title: str = ""
    artist: str = ""
    video_id: str = ""

    @property
    def progress(self) -> float:
        """Return playback progress as a 0.0-1.0 ratio."""
        if self.duration <= 0:
            return 0.0
        return self.position / self.duration


class Player:
    """Thin wrapper around *python-mpv* for headless audio playback.

    mpv plays YouTube URLs directly via its built-in ytdl-hook,
    so no external yt-dlp subprocess is needed.
    """

    def __init__(self) -> None:
        import locale

        locale.setlocale(locale.LC_NUMERIC, "C")
        self._mpv: mpv.MPV = mpv.MPV(
            ytdl=True,
            video=False,
            terminal=False,
        )
        self._video_id: str = ""
        self.on_track_end: Callable[[], None] | None = None

        # Register end-of-file observer for queue integration
        @self._mpv.event_callback("end-file")  # type: ignore[untyped-decorator]
        def _on_end_file(event: mpv.MpvEvent) -> None:
            self._handle_end_file(event)

        self._end_file_handler = _on_end_file

    def _handle_end_file(self, event: mpv.MpvEvent) -> None:
        """Fire on_track_end only when a track finished naturally.

        mpv emits end-file for *every* reason a file stops, including
        being replaced by a new loadfile (ABORTED) — reacting to those
        would auto-advance the queue right after the user picks a track,
        playing the wrong song. ERROR is also ignored on purpose: with a
        broken stream resolver it would machine-gun through the queue.
        """
        data = getattr(event, "data", None)
        reason = getattr(data, "reason", None)
        if reason != mpv.MpvEventEndFile.EOF:
            return
        if self.on_track_end is not None:
            self.on_track_end()

    # -- Playback control --------------------------------------------------

    def play(self, video_id: str) -> None:
        """Start playback of a YouTube Music track by *video_id*."""
        self._video_id = video_id
        self._mpv.play(_YTM_URL.format(video_id=video_id))

    def toggle_pause(self) -> None:
        """Toggle between paused and playing."""
        self._mpv.pause = not self._mpv.pause

    def stop(self) -> None:
        """Stop playback and clear the current track."""
        self._mpv.stop()

    # -- Volume -------------------------------------------------------------

    def set_volume(self, vol: int) -> None:
        """Set volume, clamped to 0-100."""
        self._mpv.volume = max(_VOL_MIN, min(_VOL_MAX, vol))

    def adjust_volume(self, delta: int) -> None:
        """Adjust volume by *delta* relative to current level."""
        current = self._mpv.volume or 0
        self.set_volume(int(current) + delta)

    # -- Seeking ------------------------------------------------------------

    def seek(self, seconds: float) -> None:
        """Seek *seconds* relative to current position."""
        self._mpv.seek(seconds, "relative")

    def seek_absolute(self, position: float) -> None:
        """Seek to an absolute *position* in seconds."""
        self._mpv.seek(position, "absolute")

    # -- State introspection ------------------------------------------------

    @property
    def is_idle(self) -> bool:
        """True when mpv has no file loaded (track ended or never started)."""
        return bool(self._mpv.idle_active)

    def get_state(self) -> PlayerState:
        """Read current mpv properties and return a :class:`PlayerState`."""
        idle = self.is_idle
        pause = self._mpv.pause
        volume = self._mpv.volume
        time_pos = self._mpv.time_pos
        duration = self._mpv.duration
        title = self._mpv.media_title

        return PlayerState(
            is_playing=not idle and not pause if pause is not None else False,
            volume=int(volume) if volume is not None else 0,
            position=float(time_pos) if time_pos is not None else 0.0,
            duration=float(duration) if duration is not None else 0.0,
            title=str(title) if title is not None else "",
            artist="",  # populated by queue/API layer later
            video_id="" if idle else self._video_id,
        )

    # -- Lifecycle ----------------------------------------------------------

    def shutdown(self) -> None:
        """Release the mpv instance."""
        self._mpv.terminate()
