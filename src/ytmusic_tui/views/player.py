"""Player bar view (now playing, progress, volume).

Layout mirrors spotify_player's playback window:

    Row 1:  play_icon  Track - Artist              shuffle repeat  Vol: 80
    Row 2:             Album Name
    Row 3:  progress_bar                                   1:23 / 3:45
"""

from __future__ import annotations

from typing import TYPE_CHECKING

from textual.containers import Horizontal
from textual.reactive import reactive
from textual.widgets import Static

if TYPE_CHECKING:
    from textual.app import ComposeResult

from ytmusic_tui.formatting import format_duration
from ytmusic_tui.player import PlayerState
from ytmusic_tui.queue import RepeatMode

# Interval (seconds) between player state polls
_POLL_INTERVAL_S = 1.0

# Mode display labels — always visible, dimmed when inactive.
# Rich markup is used so textual renders styling correctly.
_SHUFFLE_ON = "[bold green]S[/]"
_SHUFFLE_OFF = "[dim]S[/]"
_REPEAT_ALL = "[bold green]R:all[/]"
_REPEAT_ONE = "[bold green]R:one[/]"
_REPEAT_OFF = "[dim]R[/]"


def _format_time(seconds: float) -> str:
    """Format seconds for the player bar position display.

    Unlike ``format_duration`` this always shows ``0:00`` for zero because
    the *position* counter genuinely starts at zero.  It delegates to
    ``format_duration`` for positive values.
    """
    total = int(max(0, seconds))
    if total == 0:
        return "0:00"
    return format_duration(seconds)


def format_shuffle_icon(shuffle: bool) -> str:
    """Return the shuffle status icon."""
    return _SHUFFLE_ON if shuffle else _SHUFFLE_OFF


def format_repeat_icon(repeat_mode: RepeatMode) -> str:
    """Return the repeat status icon for the given mode."""
    if repeat_mode is RepeatMode.ALL:
        return _REPEAT_ALL
    if repeat_mode is RepeatMode.ONE:
        return _REPEAT_ONE
    return _REPEAT_OFF


def format_modes(shuffle: bool, repeat_mode: RepeatMode) -> str:
    """Return a combined shuffle + repeat display string."""
    return f"{format_shuffle_icon(shuffle)} {format_repeat_icon(repeat_mode)}"


class PlayerBar(Static):
    """Fixed footer bar showing playback state.

    Layout:
        Row 1: play/pause icon | track - artist | shuffle repeat | volume
        Row 2: (spacer)        | album name (dimmed)
        Row 3: progress bar                                 | time display
    """

    DEFAULT_CSS = """
    PlayerBar {
        dock: bottom;
        height: 4;
        background: $surface;
        border-top: solid $primary-background;
    }
    PlayerBar Horizontal {
        width: 100%;
        height: 1;
        align: left middle;
    }
    PlayerBar #player-row-top {
        height: 1;
        padding: 0 1;
    }
    PlayerBar #player-row-middle {
        height: 1;
        padding: 0 1;
    }
    PlayerBar #player-row-bottom {
        height: 1;
        padding: 0 1;
    }
    PlayerBar #player-play-icon {
        width: 4;
        padding-right: 1;
        content-align: center middle;
    }
    PlayerBar #player-track-info {
        width: 1fr;
        content-align: left middle;
    }
    PlayerBar #player-album-spacer {
        width: 4;
    }
    PlayerBar #player-album {
        width: 1fr;
        content-align: left middle;
        color: $text-muted;
    }
    PlayerBar #player-progress {
        width: 1fr;
        content-align: left middle;
    }
    PlayerBar #player-time {
        width: 14;
        content-align: right middle;
    }
    PlayerBar #player-volume {
        width: 10;
        content-align: right middle;
    }
    PlayerBar #player-modes {
        width: 12;
        content-align: center middle;
    }
    """

    _current_state: reactive[PlayerState] = reactive(PlayerState, init=False)

    def compose(self) -> ComposeResult:
        """Build the player bar layout."""
        # Row 1: play icon | track - artist | modes | volume
        with Horizontal(id="player-row-top"):
            yield Static("▶", id="player-play-icon")
            yield Static("No track", id="player-track-info")
            yield Static("", id="player-modes")
            yield Static("Vol: 80", id="player-volume")
        # Row 2: spacer | album name (dimmed)
        with Horizontal(id="player-row-middle"):
            yield Static("", id="player-album-spacer")
            yield Static("", id="player-album")
        # Row 3: progress bar (text-based) | time display
        with Horizontal(id="player-row-bottom"):
            yield Static("", id="player-progress")
            yield Static("—", id="player-time")

    def update_state(
        self,
        state: PlayerState,
        *,
        album: str = "",
        shuffle: bool = False,
        repeat_mode: RepeatMode = RepeatMode.OFF,
    ) -> None:
        """Refresh all sub-widgets from a PlayerState snapshot.

        Parameters
        ----------
        state:
            Current playback state from the player / queue enrichment.
        album:
            Album name for the current track (empty when unknown).
        shuffle:
            Whether shuffle mode is enabled.
        repeat_mode:
            Current repeat mode of the queue.
        """
        # Play/pause icon
        icon = "⏸" if state.is_playing else "▶"
        self.query_one("#player-play-icon", Static).update(icon)

        # Track info
        if state.title:
            info = f"{state.title} - {state.artist}" if state.artist else state.title
        else:
            info = "No track"
        self.query_one("#player-track-info", Static).update(info)

        # Album (dimmed second row)
        self.query_one("#player-album", Static).update(album)

        # Shuffle / repeat mode icons
        modes_text = format_modes(shuffle, repeat_mode)
        self.query_one("#player-modes", Static).update(modes_text)

        # Progress bar (text-based)
        term_width = self.size.width - 18
        bar_width = max(10, term_width)
        filled = int(bar_width * state.progress)
        empty = bar_width - filled
        bar_text = "━" * filled + "╶" + "─" * max(0, empty - 1)
        self.query_one("#player-progress", Static).update(bar_text)

        # Time display -- during active playback (video_id is set),
        # use _format_time so the duration shows "0:00" while mpv loads
        # rather than a misleading "--".  Only show "--" when there is
        # genuinely no track loaded.
        pos_str = _format_time(state.position)
        if state.video_id:
            dur_str = _format_time(state.duration)
        else:
            dur_str = format_duration(state.duration)
        self.query_one("#player-time", Static).update(f"{pos_str} / {dur_str}")

        # Volume
        volume_text = "Vol: MUTE" if state.is_muted else f"Vol: {state.volume}"
        self.query_one("#player-volume", Static).update(volume_text)

    def on_mount(self) -> None:
        """Start the periodic state-polling timer."""
        self.set_interval(_POLL_INTERVAL_S, self._poll_player_state)

    def _poll_player_state(self) -> None:
        """Request a state update from the app-level player."""
        try:
            app = self.app
            # The app stores the player reference; poll it
            player = getattr(app, "player", None)
            queue = getattr(app, "queue_manager", None)
            if player is not None:
                state = player.get_state()
                album = ""
                shuffle = False
                repeat_mode = RepeatMode.OFF

                # Enrich artist, album, and duration from queue if available.
                # mpv returns duration=None (mapped to 0.0) while the
                # ytdl-hook is still resolving the YouTube URL, so we
                # fall back to the track metadata duration which the API
                # already provides.
                if queue is not None:
                    shuffle = queue.shuffle
                    repeat_mode = queue.repeat_mode
                    if queue.current_track is not None:
                        track = queue.current_track
                        album = track.album
                        duration = state.duration
                        if duration <= 0 and track.duration_seconds > 0:
                            duration = track.duration_seconds
                        state = PlayerState(
                            is_playing=state.is_playing,
                            volume=state.volume,
                            position=state.position,
                            duration=duration,
                            title=state.title or track.title,
                            artist=track.artist,
                            video_id=state.video_id,
                        )
                self.update_state(
                    state,
                    album=album,
                    shuffle=shuffle,
                    repeat_mode=repeat_mode,
                )

                # Push state to MPRIS if available
                mpris = getattr(app, "_mpris", None)
                if mpris is not None:
                    current_track = queue.current_track if queue else None
                    mpris.update(
                        state,
                        track=current_track,
                        shuffle=shuffle,
                        repeat_mode=repeat_mode,
                    )
        except Exception:
            # Swallow errors during polling to avoid crashing the timer
            pass
