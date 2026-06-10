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

# yt-dlp format selectors for each audio quality level.
#
# YouTube Music serves:
#   - opus 251 ~160 kbps
#   - opus 250 ~70 kbps
#   - opus 249 ~50 kbps
#   - AAC 140 ~128 kbps
#
# Every entry falls back to "bestaudio/best" so an over-narrow filter
# can never yield "no formats" (the ytdl-hook would fail silently).
AUDIO_QUALITY_FORMATS: dict[str, str] = {
    "low": "bestaudio[abr<=70]/bestaudio/best",
    "normal": "bestaudio[abr<=131]/bestaudio/best",
    "high": "bestaudio/best",
}

# Ordered cycle used by cycle_audio_quality.
_QUALITY_CYCLE = ["low", "normal", "high"]


@dataclass
class PlayerState:
    """Snapshot of the current playback state."""

    is_playing: bool = False
    volume: int = 80
    is_muted: bool = False
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

    Args:
        audio_quality: Initial quality level — "low", "normal", or "high".
            Unknown values are silently normalised to "high" (config typos
            degrade gracefully to the default; the ``b`` key in the TUI
            reveals the active value).
    """

    def __init__(self, audio_quality: str = "high") -> None:
        import locale

        locale.setlocale(locale.LC_NUMERIC, "C")
        # Normalise unknown values to the safe default.
        quality = audio_quality if audio_quality in AUDIO_QUALITY_FORMATS else "high"
        self._audio_quality: str = quality

        # python-mpv translates constructor kwargs: underscores → dashes
        # (e.g. ytdl_format → ytdl-format).  See mpv.MPV.__init__.
        self._mpv: mpv.MPV = mpv.MPV(
            ytdl=True,
            video=False,
            terminal=False,
            ytdl_format=AUDIO_QUALITY_FORMATS[quality],
        )
        self._video_id: str = ""
        self.on_track_end: Callable[[], None] | None = None
        self.on_track_error: Callable[[str], None] | None = None

        # Register end-of-file observer for queue integration.
        # python-mpv's event_callback decorator is untyped.
        @self._mpv.event_callback("end-file")  # type: ignore[untyped-decorator]
        def _on_end_file(event: mpv.MpvEvent) -> None:
            self._handle_end_file(event)

        self._end_file_handler = _on_end_file

    def _handle_end_file(self, event: mpv.MpvEvent) -> None:
        """Route an mpv end-file event to the right callback.

        mpv emits end-file for *every* reason a file stops:

        * ``EOF`` — the track finished naturally; fire ``on_track_end`` so
          the queue advances.
        * ``ERROR`` — the stream could not be played (e.g. a stale resolver
          facing YouTube's EJS challenges). We deliberately do **not**
          advance the queue, because a broken resolver would machine-gun
          through every track. Instead we fire ``on_track_error`` with a
          short description so the user gets visible feedback rather than
          silence. The description comes from mpv's integer error code
          (``event.data.error``) via :meth:`mpv.ErrorCode.human_readable`;
          python-mpv 1.0.8 exposes no human-readable ``file_error`` string,
          so we translate the code ourselves and fall back to an empty
          string if it is unavailable.
        * Any other reason (ABORTED on loadfile replacement, QUIT,
          REDIRECT, ...) is ignored: reacting to ABORTED auto-advanced the
          queue right after the user picked a track, playing the wrong song.
        """
        data = getattr(event, "data", None)
        reason = getattr(data, "reason", None)
        if reason == mpv.MpvEventEndFile.EOF:
            if self.on_track_end is not None:
                self.on_track_end()
            return
        if reason == mpv.MpvEventEndFile.ERROR:
            if self.on_track_error is not None:
                self.on_track_error(self._end_file_error(data))
            return

    @staticmethod
    def _end_file_error(data: object) -> str:
        """Translate an end-file event's mpv error code to a short string.

        Returns the empty string when the code is missing, zero, or cannot
        be translated, so callers can treat the description as optional.
        """
        code = getattr(data, "error", None)
        if not isinstance(code, int) or code == 0:
            return ""
        try:
            return str(mpv.ErrorCode.human_readable(code))
        except Exception:
            return ""

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

    def toggle_mute(self) -> None:
        """Toggle audio mute."""
        self._mpv.mute = not self._mpv.mute

    # -- Seeking ------------------------------------------------------------

    def seek(self, seconds: float) -> None:
        """Seek *seconds* relative to current position."""
        self._mpv.seek(seconds, "relative")

    def seek_absolute(self, position: float) -> None:
        """Seek to an absolute *position* in seconds."""
        self._mpv.seek(position, "absolute")

    # -- Audio quality ------------------------------------------------------

    @property
    def audio_quality(self) -> str:
        """Current audio quality level ("low", "normal", or "high")."""
        return self._audio_quality

    def set_audio_quality(self, quality: str) -> None:
        """Set the yt-dlp format selector for a new quality level.

        Unknown values are normalised to "high".  The change applies from
        the *next* track: ytdl-format is evaluated by the ytdl-hook at
        loadfile time, so the currently-playing stream is unaffected.

        Uses dict-style option write (``self._mpv["ytdl-format"] = ...``),
        which maps to mpv's ``options/`` property namespace — the same
        mechanism as MPV.__setitem__ in python-mpv 1.0.8.
        """
        quality = quality if quality in AUDIO_QUALITY_FORMATS else "high"
        self._audio_quality = quality
        self._mpv["ytdl-format"] = AUDIO_QUALITY_FORMATS[quality]

    def cycle_audio_quality(self) -> str:
        """Advance quality low → normal → high → low and return the new value."""
        idx = _QUALITY_CYCLE.index(self._audio_quality)
        new_quality = _QUALITY_CYCLE[(idx + 1) % len(_QUALITY_CYCLE)]
        self.set_audio_quality(new_quality)
        return self._audio_quality

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
        muted = self._mpv.mute
        time_pos = self._mpv.time_pos
        duration = self._mpv.duration
        title = self._mpv.media_title

        return PlayerState(
            is_playing=not idle and not pause if pause is not None else False,
            volume=int(volume) if volume is not None else 0,
            is_muted=bool(muted),
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
