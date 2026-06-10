"""Tests for the playback queue."""

from __future__ import annotations

import pytest
from helpers import make_track as _make_track
from helpers import make_tracks as _make_tracks

from ytmusic_tui.queue import QueueManager, RepeatMode, Track

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


# ===================================================================
# Track dataclass
# ===================================================================


class TestTrack:
    def test_creation_with_defaults(self) -> None:
        t = Track(video_id="abc", title="Test", artist="Art")
        assert t.video_id == "abc"
        assert t.title == "Test"
        assert t.artist == "Art"
        assert t.album == ""
        assert t.duration_seconds == 0.0
        assert t.thumbnail_url == ""

    def test_creation_with_all_fields(self) -> None:
        t = Track(
            video_id="xyz",
            title="Full",
            artist="Band",
            album="LP",
            duration_seconds=240.5,
            thumbnail_url="https://img.example.com/thumb.jpg",
        )
        assert t.duration_seconds == 240.5
        assert t.thumbnail_url == "https://img.example.com/thumb.jpg"

    def test_frozen(self) -> None:
        t = _make_track(1)
        with pytest.raises(AttributeError):
            t.title = "Changed"  # type: ignore[misc]

    def test_equality(self) -> None:
        a = Track(video_id="same", title="T", artist="A")
        b = Track(video_id="same", title="T", artist="A")
        assert a == b

    def test_inequality(self) -> None:
        a = _make_track(1)
        b = _make_track(2)
        assert a != b


# ===================================================================
# QueueManager - basic state
# ===================================================================


class TestQueueManagerInit:
    def test_initial_state(self) -> None:
        q = QueueManager()
        assert q.current_track is None
        assert q.tracks == []
        assert q.shuffle is False
        assert q.repeat_mode is RepeatMode.OFF

    def test_tracks_returns_copy(self) -> None:
        q = QueueManager()
        q.add(_make_track(1))
        copy = q.tracks
        copy.append(_make_track(99))
        assert len(q.tracks) == 1


# ===================================================================
# QueueManager - add / add_many
# ===================================================================


class TestQueueAdd:
    def test_add_single(self) -> None:
        q = QueueManager()
        t = _make_track(1)
        q.add(t)
        assert q.tracks == [t]

    def test_add_many(self) -> None:
        q = QueueManager()
        tracks = _make_tracks(3)
        q.add_many(tracks)
        assert q.tracks == tracks

    def test_add_many_appends(self) -> None:
        q = QueueManager()
        q.add(_make_track(1))
        q.add_many(_make_tracks(2))
        assert len(q.tracks) == 3


# ===================================================================
# QueueManager - set_playlist
# ===================================================================


class TestSetPlaylist:
    def test_replaces_queue(self) -> None:
        q = QueueManager()
        q.add(_make_track(99))
        tracks = _make_tracks(3)
        q.set_playlist(tracks)
        assert q.tracks == tracks
        assert q.current_track == tracks[0]

    def test_start_index(self) -> None:
        q = QueueManager()
        tracks = _make_tracks(5)
        q.set_playlist(tracks, start_index=2)
        assert q.current_track == tracks[2]

    def test_start_index_out_of_range_clamps(self) -> None:
        q = QueueManager()
        tracks = _make_tracks(3)
        q.set_playlist(tracks, start_index=10)
        assert q.current_track == tracks[-1]

    def test_empty_playlist(self) -> None:
        q = QueueManager()
        q.add(_make_track(1))
        q.set_playlist([])
        assert q.current_track is None
        assert q.tracks == []


# ===================================================================
# QueueManager - navigation (next / previous)
# ===================================================================


class TestNavigation:
    def test_next_track_advances(self) -> None:
        q = QueueManager()
        tracks = _make_tracks(3)
        q.set_playlist(tracks)
        assert q.current_track == tracks[0]
        nxt = q.next_track()
        assert nxt == tracks[1]
        assert q.current_track == tracks[1]

    def test_next_at_end_repeat_off(self) -> None:
        q = QueueManager()
        q.set_playlist(_make_tracks(2))
        q.next_track()  # -> track 2
        result = q.next_track()  # at end
        assert result is None

    def test_next_at_end_repeat_all(self) -> None:
        q = QueueManager()
        q.set_playlist(_make_tracks(2))
        q.repeat_mode = RepeatMode.ALL
        q.next_track()  # -> track 2
        result = q.next_track()  # wraps to track 1
        assert result == _make_track(1)
        assert q.current_track == _make_track(1)

    def test_next_repeat_one(self) -> None:
        q = QueueManager()
        tracks = _make_tracks(3)
        q.set_playlist(tracks)
        q.repeat_mode = RepeatMode.ONE
        result = q.next_track()
        assert result == tracks[0]  # stays on current
        assert q.current_track == tracks[0]

    def test_previous_goes_back(self) -> None:
        q = QueueManager()
        tracks = _make_tracks(3)
        q.set_playlist(tracks)
        q.next_track()  # -> 2
        q.next_track()  # -> 3
        prev = q.previous_track()
        assert prev == tracks[1]

    def test_previous_at_start(self) -> None:
        q = QueueManager()
        tracks = _make_tracks(3)
        q.set_playlist(tracks)
        prev = q.previous_track()
        assert prev == tracks[0]  # stays at start

    def test_next_on_empty(self) -> None:
        q = QueueManager()
        assert q.next_track() is None

    def test_previous_on_empty(self) -> None:
        q = QueueManager()
        assert q.previous_track() is None


# ===================================================================
# QueueManager - remove
# ===================================================================


class TestRemove:
    def test_remove_after_current(self) -> None:
        q = QueueManager()
        tracks = _make_tracks(3)
        q.set_playlist(tracks)
        q.remove(2)  # remove last track
        assert len(q.tracks) == 2
        assert q.current_track == tracks[0]

    def test_remove_before_current(self) -> None:
        q = QueueManager()
        tracks = _make_tracks(4)
        q.set_playlist(tracks, start_index=2)
        assert q.current_track == tracks[2]
        q.remove(0)  # remove track before current
        # current should still point to same track
        assert q.current_track == tracks[2]

    def test_remove_current_track(self) -> None:
        q = QueueManager()
        tracks = _make_tracks(3)
        q.set_playlist(tracks, start_index=1)
        assert q.current_track == tracks[1]
        q.remove(1)  # remove current
        # current advances to next (which was tracks[2], now at index 1)
        assert q.current_track == tracks[2]

    def test_remove_current_last_track(self) -> None:
        q = QueueManager()
        tracks = _make_tracks(3)
        q.set_playlist(tracks, start_index=2)
        q.remove(2)  # remove current which is last
        # should fall back to new last
        assert q.current_track == tracks[1]

    def test_remove_only_track(self) -> None:
        q = QueueManager()
        q.set_playlist([_make_track(1)])
        q.remove(0)
        assert q.current_track is None
        assert q.tracks == []

    def test_remove_invalid_index(self) -> None:
        q = QueueManager()
        q.set_playlist(_make_tracks(2))
        with pytest.raises(IndexError):
            q.remove(5)

    def test_remove_negative_index(self) -> None:
        q = QueueManager()
        q.set_playlist(_make_tracks(2))
        with pytest.raises(IndexError):
            q.remove(-1)


# ===================================================================
# QueueManager - clear
# ===================================================================


class TestClear:
    def test_clear(self) -> None:
        q = QueueManager()
        q.set_playlist(_make_tracks(5), start_index=3)
        q.clear()
        assert q.tracks == []
        assert q.current_track is None

    def test_clear_resets_shuffle(self) -> None:
        q = QueueManager()
        q.set_playlist(_make_tracks(5))
        q.toggle_shuffle()
        q.clear()
        assert q.shuffle is False


# ===================================================================
# QueueManager - shuffle
# ===================================================================


class TestShuffle:
    def test_toggle_on(self) -> None:
        q = QueueManager()
        tracks = _make_tracks(10)
        q.set_playlist(tracks)
        q.next_track()  # current = track 2 (index 1)
        q.toggle_shuffle()
        assert q.shuffle is True
        # Current track should not change
        assert q.current_track == tracks[1]

    def test_shuffle_preserves_current(self) -> None:
        q = QueueManager()
        tracks = _make_tracks(10)
        q.set_playlist(tracks)
        q.toggle_shuffle()
        assert q.current_track == tracks[0]

    def test_shuffle_only_remaining(self) -> None:
        q = QueueManager()
        tracks = _make_tracks(10)
        q.set_playlist(tracks, start_index=3)
        q.toggle_shuffle()
        # Tracks before and including current should be untouched
        assert q.tracks[:4] == tracks[:4]
        # Remaining are a permutation of the originals
        remaining = set(q.tracks[4:])
        assert remaining == set(tracks[4:])

    def test_unshuffle_restores_order(self) -> None:
        q = QueueManager()
        tracks = _make_tracks(10)
        q.set_playlist(tracks, start_index=2)
        q.toggle_shuffle()  # shuffle on
        assert q.shuffle is True
        q.toggle_shuffle()  # shuffle off
        assert q.shuffle is False
        assert q.tracks == tracks
        assert q.current_track == tracks[2]

    def test_toggle_shuffle_empty(self) -> None:
        q = QueueManager()
        q.toggle_shuffle()  # should not raise
        assert q.shuffle is True


# ===================================================================
# QueueManager - repeat mode cycling
# ===================================================================


class TestRepeatCycle:
    def test_cycle_order(self) -> None:
        q = QueueManager()
        assert q.repeat_mode is RepeatMode.OFF
        q.cycle_repeat()
        assert q.repeat_mode is RepeatMode.ALL
        q.cycle_repeat()
        assert q.repeat_mode is RepeatMode.ONE
        q.cycle_repeat()
        assert q.repeat_mode is RepeatMode.OFF


# ===================================================================
# QueueManager - move
# ===================================================================


class TestMove:
    def test_move_forward(self) -> None:
        q = QueueManager()
        tracks = _make_tracks(5)
        q.set_playlist(tracks)
        q.move(1, 3)
        assert q.tracks[3] == tracks[1]
        assert q.tracks[1] == tracks[2]

    def test_move_backward(self) -> None:
        q = QueueManager()
        tracks = _make_tracks(5)
        q.set_playlist(tracks)
        q.move(3, 1)
        assert q.tracks[1] == tracks[3]
        assert q.tracks[2] == tracks[1]

    def test_move_current_track(self) -> None:
        q = QueueManager()
        tracks = _make_tracks(5)
        q.set_playlist(tracks, start_index=1)
        q.move(1, 3)
        # current_track should follow the moved track
        assert q.current_track == tracks[1]

    def test_move_same_position(self) -> None:
        q = QueueManager()
        tracks = _make_tracks(3)
        q.set_playlist(tracks)
        q.move(1, 1)  # no-op
        assert q.tracks == tracks

    def test_move_invalid_index(self) -> None:
        q = QueueManager()
        q.set_playlist(_make_tracks(3))
        with pytest.raises(IndexError):
            q.move(0, 5)

    def test_move_updates_current_index_when_affected(self) -> None:
        """When moving a track across the current index, current must adjust."""
        q = QueueManager()
        tracks = _make_tracks(5)
        q.set_playlist(tracks, start_index=2)
        # Move track from before current to after current
        q.move(0, 4)
        # Current track should still be the same Track object
        assert q.current_track == tracks[2]


# ===================================================================
# QueueManager - edge cases
# ===================================================================


class TestEdgeCases:
    def test_single_track_next_repeat_off(self) -> None:
        q = QueueManager()
        q.set_playlist([_make_track(1)])
        result = q.next_track()
        assert result is None

    def test_single_track_next_repeat_all(self) -> None:
        q = QueueManager()
        q.set_playlist([_make_track(1)])
        q.repeat_mode = RepeatMode.ALL
        result = q.next_track()
        assert result == _make_track(1)

    def test_single_track_next_repeat_one(self) -> None:
        q = QueueManager()
        q.set_playlist([_make_track(1)])
        q.repeat_mode = RepeatMode.ONE
        result = q.next_track()
        assert result == _make_track(1)

    def test_navigation_through_full_queue(self) -> None:
        """Walk forward through entire queue then back."""
        q = QueueManager()
        tracks = _make_tracks(4)
        q.set_playlist(tracks)
        for i in range(1, 4):
            assert q.next_track() == tracks[i]
        assert q.next_track() is None  # end, repeat OFF
        # Walk back
        for i in range(2, -1, -1):
            assert q.previous_track() == tracks[i]

    def test_repeat_all_full_cycle(self) -> None:
        q = QueueManager()
        tracks = _make_tracks(3)
        q.set_playlist(tracks)
        q.repeat_mode = RepeatMode.ALL
        # Go through all tracks and wrap
        q.next_track()  # 2
        q.next_track()  # 3
        result = q.next_track()  # wraps to 1
        assert result == tracks[0]
        result = q.next_track()  # 2 again
        assert result == tracks[1]


# ===================================================================
# QueueManager - add to empty queue sets current_index (Bug 3)
# ===================================================================


class TestAddToEmptyQueue:
    def test_add_to_empty_queue_sets_current_index(self) -> None:
        """Adding a track to an empty queue should make it the current track."""
        q = QueueManager()
        assert q.current_track is None
        t = _make_track(1)
        q.add(t)
        assert q.current_track == t

    def test_add_many_to_empty_queue_sets_current_index(self) -> None:
        """Adding tracks to an empty queue should set the first as current."""
        q = QueueManager()
        tracks = _make_tracks(3)
        q.add_many(tracks)
        assert q.current_track == tracks[0]

    def test_add_to_nonempty_queue_preserves_current(self) -> None:
        """Adding a track to a non-empty queue should not change current."""
        q = QueueManager()
        tracks = _make_tracks(3)
        q.set_playlist(tracks, start_index=1)
        q.add(_make_track(99))
        assert q.current_track == tracks[1]

    def test_add_many_to_nonempty_queue_preserves_current(self) -> None:
        """add_many on a non-empty queue should not move the index."""
        q = QueueManager()
        tracks = _make_tracks(3)
        q.set_playlist(tracks, start_index=2)
        q.add_many(_make_tracks(2))
        assert q.current_track == tracks[2]

    def test_add_many_empty_list_is_noop(self) -> None:
        """add_many([]) should not change state."""
        q = QueueManager()
        q.add_many([])
        assert q.current_track is None


# ===================================================================
# QueueManager - add after queue exhaustion (Bug 4)
# ===================================================================


class TestAddAfterExhaustion:
    def test_add_after_queue_exhausted_resets_index(self) -> None:
        """Adding tracks after the queue ran out should set current to the new track."""
        q = QueueManager()
        q.set_playlist(_make_tracks(2))
        q.next_track()  # -> track 2
        result = q.next_track()  # -> None (end, repeat OFF)
        assert result is None
        # Queue is exhausted; adding a track should make it current.
        new_track = _make_track(99)
        q.add(new_track)
        assert len(q.tracks) == 3
        assert q.current_track == new_track

    def test_add_many_after_queue_exhausted_resets_index(self) -> None:
        """Adding tracks via add_many after exhaustion should set first as current."""
        q = QueueManager()
        q.set_playlist(_make_tracks(2))
        q.next_track()  # -> track 2
        q.next_track()  # -> None (end)
        new_tracks = [_make_track(90), _make_track(91)]
        q.add_many(new_tracks)
        assert q.current_track == new_tracks[0]

    def test_add_after_clear_and_exhaust(self) -> None:
        """After clear(), add() should set current to the new track."""
        q = QueueManager()
        q.set_playlist(_make_tracks(2))
        q.clear()
        assert q.current_track is None
        t = _make_track(42)
        q.add(t)
        assert q.current_track == t

    def test_add_many_after_clear(self) -> None:
        """After clear(), add_many() should set current to the first new track."""
        q = QueueManager()
        q.set_playlist(_make_tracks(3))
        q.clear()
        new_tracks = _make_tracks(2)
        q.add_many(new_tracks)
        assert q.current_track == new_tracks[0]

    def test_previous_clears_exhausted_flag(self) -> None:
        """Going back with previous_track after exhaustion should clear the flag."""
        q = QueueManager()
        tracks = _make_tracks(3)
        q.set_playlist(tracks)
        q.next_track()  # -> track 2
        q.next_track()  # -> track 3
        q.next_track()  # -> None (exhausted)
        prev = q.previous_track()
        assert prev == tracks[1]
        # Adding after previous should NOT reset to end of queue
        q.add(_make_track(99))
        assert q.current_track == tracks[1]  # unchanged

    def test_exhausted_flag_reset_on_set_playlist(self) -> None:
        """set_playlist should clear the exhausted flag."""
        q = QueueManager()
        q.set_playlist(_make_tracks(1))
        q.next_track()  # -> None (exhausted)
        new_tracks = _make_tracks(3)
        q.set_playlist(new_tracks)
        # Should not be exhausted; add should not reset
        q.add(_make_track(99))
        assert q.current_track == new_tracks[0]
