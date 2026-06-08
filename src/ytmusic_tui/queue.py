"""Playback queue management."""

from __future__ import annotations

import random
from dataclasses import dataclass
from enum import Enum


class RepeatMode(Enum):
    """Repeat behaviour for the queue."""

    OFF = "off"
    ALL = "all"
    ONE = "one"


# Cycle order used by QueueManager.cycle_repeat()
_REPEAT_CYCLE: list[RepeatMode] = [RepeatMode.OFF, RepeatMode.ALL, RepeatMode.ONE]


@dataclass(frozen=True)
class Track:
    """Immutable representation of a single music track."""

    video_id: str
    title: str
    artist: str
    album: str = ""
    duration_seconds: float = 0.0
    thumbnail_url: str = ""


class QueueManager:
    """Manages an ordered playback queue with shuffle and repeat support.

    Design decisions
    ----------------
    * Selecting a song from a playlist queues all remaining songs
      (spotify_player style).
    * Search results queue a single song.
    * Shuffle reorders the tracks after the current position;
      unshuffling restores the original order while keeping the
      current track in place.
    * Repeat modes: OFF (stop at end), ALL (wrap), ONE (loop current).
    """

    def __init__(self) -> None:
        self._tracks: list[Track] = []
        self._current_index: int = -1
        self._shuffle: bool = False
        self._repeat_mode: RepeatMode = RepeatMode.OFF
        self._exhausted: bool = False

        # Original order snapshot used for unshuffle
        self._original_tracks: list[Track] | None = None

    # ------------------------------------------------------------------
    # Properties
    # ------------------------------------------------------------------

    @property
    def current_track(self) -> Track | None:
        """Return the currently selected track, or ``None``."""
        if not self._tracks or self._current_index < 0:
            return None
        return self._tracks[self._current_index]

    @property
    def tracks(self) -> list[Track]:
        """Return a read-only copy of the queue."""
        return list(self._tracks)

    @property
    def shuffle(self) -> bool:
        """Whether shuffle is currently enabled."""
        return self._shuffle

    @property
    def repeat_mode(self) -> RepeatMode:
        """Current repeat mode."""
        return self._repeat_mode

    @repeat_mode.setter
    def repeat_mode(self, value: RepeatMode) -> None:
        self._repeat_mode = value

    # ------------------------------------------------------------------
    # Queue mutation
    # ------------------------------------------------------------------

    def add(self, track: Track) -> None:
        """Append a single track to the end of the queue.

        If the queue was empty (``current_index == -1``), exhausted
        (``_exhausted`` flag set by ``next_track``), or the index is
        past the end, the index is reset so that ``current_track``
        returns a valid track.
        """
        was_empty_or_exhausted = (
            self._current_index < 0 or self._current_index >= len(self._tracks) or self._exhausted
        )
        insert_pos = len(self._tracks)
        self._tracks.append(track)
        if was_empty_or_exhausted:
            self._current_index = insert_pos
            self._exhausted = False

    def add_many(self, tracks: list[Track]) -> None:
        """Append multiple tracks to the end of the queue.

        If the queue was empty or exhausted, the index is reset to the
        first newly added track so that ``current_track`` is valid.
        """
        if not tracks:
            return
        was_empty_or_exhausted = (
            self._current_index < 0 or self._current_index >= len(self._tracks) or self._exhausted
        )
        insert_pos = len(self._tracks)
        self._tracks.extend(tracks)
        if was_empty_or_exhausted:
            self._current_index = insert_pos
            self._exhausted = False

    def set_playlist(self, tracks: list[Track], start_index: int = 0) -> None:
        """Replace the entire queue and set the current position.

        If *start_index* exceeds the length of *tracks* it is clamped
        to the last valid position.
        """
        self._tracks = list(tracks)
        self._shuffle = False
        self._original_tracks = None
        self._exhausted = False

        if not self._tracks:
            self._current_index = -1
            return

        self._current_index = min(start_index, len(self._tracks) - 1)

    # ------------------------------------------------------------------
    # Navigation
    # ------------------------------------------------------------------

    def next_track(self) -> Track | None:
        """Advance to the next track respecting the current repeat mode.

        Returns the new current track, or ``None`` when playback should
        stop (repeat OFF and already at the end).
        """
        if not self._tracks:
            return None

        if self._repeat_mode is RepeatMode.ONE:
            # Stay on current track
            return self.current_track

        next_index = self._current_index + 1

        if next_index >= len(self._tracks):
            if self._repeat_mode is RepeatMode.ALL:
                self._current_index = 0
                return self.current_track
            # RepeatMode.OFF - end of queue
            self._exhausted = True
            return None

        self._exhausted = False
        self._current_index = next_index
        return self.current_track

    def previous_track(self) -> Track | None:
        """Go back one track.

        At the beginning of the queue the position stays at 0.
        Returns the (possibly unchanged) current track, or ``None``
        if the queue is empty.
        """
        if not self._tracks:
            return None

        self._exhausted = False
        self._current_index = max(0, self._current_index - 1)
        return self.current_track

    # ------------------------------------------------------------------
    # Remove / clear
    # ------------------------------------------------------------------

    def remove(self, index: int) -> None:
        """Remove the track at *index*, adjusting the current position.

        Raises :class:`IndexError` for out-of-range or negative indices.
        """
        if index < 0 or index >= len(self._tracks):
            raise IndexError(f"Queue index out of range: {index}")

        self._tracks.pop(index)

        if not self._tracks:
            self._current_index = -1
            return

        if index < self._current_index:
            # Removed before current - shift left
            self._current_index -= 1
        elif index == self._current_index and self._current_index >= len(self._tracks):
            # Removed the current track and it was the last element - fall back
            self._current_index = len(self._tracks) - 1

    def clear(self) -> None:
        """Empty the queue and reset all state."""
        self._tracks.clear()
        self._current_index = -1
        self._shuffle = False
        self._exhausted = False
        self._original_tracks = None

    # ------------------------------------------------------------------
    # Shuffle
    # ------------------------------------------------------------------

    def toggle_shuffle(self) -> None:
        """Toggle shuffle mode.

        When enabling, only tracks *after* the current position are
        shuffled.  The current track and everything before it stay in
        place.

        When disabling, the original order is restored while keeping
        the current track's identity (the index is updated so that the
        same :class:`Track` object remains selected).
        """
        if self._shuffle:
            self._unshuffle()
        else:
            self._enable_shuffle()

    def _enable_shuffle(self) -> None:
        """Shuffle remaining tracks (after current)."""
        self._shuffle = True

        if not self._tracks:
            return

        # Snapshot original order before mutating
        self._original_tracks = list(self._tracks)

        split = self._current_index + 1
        remaining = self._tracks[split:]
        random.shuffle(remaining)
        self._tracks[split:] = remaining

    def _unshuffle(self) -> None:
        """Restore original order, keeping current track selected."""
        self._shuffle = False

        if self._original_tracks is None:
            return

        current = self.current_track
        self._tracks = list(self._original_tracks)
        self._original_tracks = None

        # Re-locate the current track in the restored order
        if current is not None:
            try:
                self._current_index = self._tracks.index(current)
            except ValueError:
                # Defensive: track was removed while shuffled
                self._current_index = 0

    # ------------------------------------------------------------------
    # Repeat
    # ------------------------------------------------------------------

    def cycle_repeat(self) -> None:
        """Cycle through repeat modes: OFF -> ALL -> ONE -> OFF."""
        idx = _REPEAT_CYCLE.index(self._repeat_mode)
        self._repeat_mode = _REPEAT_CYCLE[(idx + 1) % len(_REPEAT_CYCLE)]

    # ------------------------------------------------------------------
    # Move
    # ------------------------------------------------------------------

    def move(self, from_idx: int, to_idx: int) -> None:
        """Move the track at *from_idx* to *to_idx*.

        The current-track pointer follows if the moved track is the
        current one, and adjusts when a move shifts the current
        position.

        Raises :class:`IndexError` for out-of-range indices.
        """
        length = len(self._tracks)
        if from_idx < 0 or from_idx >= length or to_idx < 0 or to_idx >= length:
            raise IndexError(
                f"Move indices out of range: from={from_idx}, to={to_idx}, len={length}"
            )

        if from_idx == to_idx:
            return

        track = self._tracks.pop(from_idx)

        # Adjust current_index for the pop
        new_current = self._current_index
        if from_idx == self._current_index:
            # We are moving the current track - will fix after insert
            new_current = -1  # sentinel
        elif from_idx < self._current_index:
            new_current -= 1

        self._tracks.insert(to_idx, track)

        # Adjust current_index for the insert
        if new_current == -1:
            # The moved track IS the current track
            new_current = to_idx
        elif to_idx <= new_current:
            new_current += 1

        self._current_index = new_current
