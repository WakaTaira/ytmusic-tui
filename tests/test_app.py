"""Tests for the TUI application skeleton."""

from __future__ import annotations

from typing import TYPE_CHECKING
from unittest.mock import MagicMock, patch

import pytest
from helpers import make_app as _make_app
from textual.widgets import ContentSwitcher, Header, Static

from ytmusic_tui.player import PlayerState
from ytmusic_tui.queue import RepeatMode, Track
from ytmusic_tui.views.home import HomeView
from ytmusic_tui.views.library import LibraryView
from ytmusic_tui.views.player import (
    PlayerBar,
    format_modes,
    format_repeat_icon,
    format_shuffle_icon,
)
from ytmusic_tui.views.playlist import PlaylistView
from ytmusic_tui.views.queue import QueueView
from ytmusic_tui.views.search import SearchView

if TYPE_CHECKING:
    from pathlib import Path

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


# ===================================================================
# App construction
# ===================================================================


class TestAppConstruction:
    @patch("ytmusic_tui.app.MusicAPI")
    @patch("ytmusic_tui.app.Player")
    def test_app_can_be_constructed(
        self, mock_player_cls: MagicMock, mock_api_cls: MagicMock
    ) -> None:
        from ytmusic_tui.app import YtMusicTui

        app = YtMusicTui(auth_path="/fake/auth.json")
        assert app.title == "ytmusic-tui"

    @patch("ytmusic_tui.app.MusicAPI")
    @patch("ytmusic_tui.app.Player")
    def test_default_auth_path(self, mock_player_cls: MagicMock, mock_api_cls: MagicMock) -> None:
        from ytmusic_tui.app import YtMusicTui

        YtMusicTui()
        mock_api_cls.assert_called_once()
        # The default path should contain browser.json
        call_args = mock_api_cls.call_args[0][0]
        assert "browser.json" in str(call_args)

    @patch("ytmusic_tui.app.MusicAPI")
    @patch("ytmusic_tui.app.Player")
    def test_custom_auth_path(self, mock_player_cls: MagicMock, mock_api_cls: MagicMock) -> None:
        from ytmusic_tui.app import YtMusicTui

        YtMusicTui(auth_path="/custom/path.json")
        mock_api_cls.assert_called_once_with("/custom/path.json")


# ===================================================================
# App compose
# ===================================================================


class TestAppCompose:
    @pytest.mark.asyncio
    async def test_compose_yields_expected_widgets(self) -> None:
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            # Header should exist
            assert app.query_one(Header) is not None

            # PlayerBar should exist
            assert app.query_one(PlayerBar) is not None

            # ContentSwitcher should exist
            switcher = app.query_one(ContentSwitcher)
            assert switcher is not None

    @pytest.mark.asyncio
    async def test_home_view_is_default(self) -> None:
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            switcher = app.query_one(ContentSwitcher)
            assert switcher.current == "home"

    @pytest.mark.asyncio
    async def test_all_views_exist(self) -> None:
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            assert app.query_one(HomeView) is not None
            assert app.query_one(SearchView) is not None
            assert app.query_one(PlaylistView) is not None
            assert app.query_one(LibraryView) is not None
            assert app.query_one(QueueView) is not None


# ===================================================================
# PlayerBar
# ===================================================================


class TestPlayerBar:
    @pytest.mark.asyncio
    async def test_initial_display(self) -> None:
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            bar = app.query_one(PlayerBar)
            assert bar is not None

    @pytest.mark.asyncio
    async def test_update_state_with_playing_track(self) -> None:
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            bar = app.query_one(PlayerBar)
            state = PlayerState(
                is_playing=True,
                volume=75,
                position=90.0,
                duration=240.0,
                title="Test Song",
                artist="Test Artist",
            )
            bar.update_state(state)
            # Check that the title/artist label updated
            title_text = bar.query_one("#player-track-info", Static).content
            assert "Test Song" in title_text
            assert "Test Artist" in title_text

    @pytest.mark.asyncio
    async def test_update_state_paused(self) -> None:
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            bar = app.query_one(PlayerBar)
            state = PlayerState(
                is_playing=False,
                title="Paused Track",
                artist="Artist",
            )
            bar.update_state(state)
            play_icon = bar.query_one("#player-play-icon", Static).content
            # Should show play icon (not pause)
            assert "▶" in play_icon

    @pytest.mark.asyncio
    async def test_update_state_playing_icon(self) -> None:
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            bar = app.query_one(PlayerBar)
            state = PlayerState(is_playing=True, title="Song", artist="Art")
            bar.update_state(state)
            play_icon = bar.query_one("#player-play-icon", Static).content
            # Should show pause icon
            assert "⏸" in play_icon

    @pytest.mark.asyncio
    async def test_update_state_time_display(self) -> None:
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            bar = app.query_one(PlayerBar)
            state = PlayerState(
                is_playing=True,
                position=65.0,
                duration=200.0,
                title="Song",
                artist="Art",
            )
            bar.update_state(state)
            time_text = bar.query_one("#player-time", Static).content
            # 65s = 1:05, 200s = 3:20
            assert "1:05" in time_text
            assert "3:20" in time_text

    @pytest.mark.asyncio
    async def test_update_state_no_track(self) -> None:
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            bar = app.query_one(PlayerBar)
            state = PlayerState()
            bar.update_state(state)
            title_text = bar.query_one("#player-track-info", Static).content
            assert "No track" in title_text


# ===================================================================
# Stub views
# ===================================================================


class TestStubViews:
    @pytest.mark.asyncio
    async def test_search_view_placeholder(self) -> None:
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            view = app.query_one(SearchView)
            assert view is not None

    @pytest.mark.asyncio
    async def test_library_view_placeholder(self) -> None:
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            view = app.query_one(LibraryView)
            assert view is not None

    @pytest.mark.asyncio
    async def test_playlist_view_placeholder(self) -> None:
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            view = app.query_one(PlaylistView)
            assert view is not None

    @pytest.mark.asyncio
    async def test_queue_view_placeholder(self) -> None:
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            view = app.query_one(QueueView)
            assert view is not None


# ===================================================================
# Key bindings
# ===================================================================


class TestKeyBindings:
    @pytest.mark.asyncio
    async def test_quit_binding(self) -> None:
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            # App should have a quit binding
            binding_keys = [b.key for b in app.BINDINGS]
            assert "q" in binding_keys

    @pytest.mark.asyncio
    async def test_seek_bindings_exist(self) -> None:
        """> / < should be bound to seek actions (spotify_player style)."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            actions = {b.key: b.action for b in app.BINDINGS}
            assert actions.get("greater_than_sign") == "seek_forward"
            assert actions.get("less_than_sign") == "seek_backward"

    def test_seek_actions_remappable_via_keymap(self) -> None:
        """seek_forward / seek_backward must be exposed to keymap.toml.

        Remappability is expressed by a Binding carrying an ``id`` equal
        to the keymap action name (set_keymap overrides by binding id).
        """
        from textual.binding import Binding

        from ytmusic_tui.app import YtMusicTui

        binding_ids = {b.id for b in YtMusicTui.BINDINGS if isinstance(b, Binding)}
        assert "seek_forward" in binding_ids
        assert "seek_backward" in binding_ids

    def test_search_action_has_renamed_id(self) -> None:
        """The ``/`` filter binding must expose the ``search`` keymap id.

        Renamed from ``focus_search`` to match spotify_player's "Search".
        """
        from textual.binding import Binding

        from ytmusic_tui.app import YtMusicTui

        actions = {b.id: b.action for b in YtMusicTui.BINDINGS if isinstance(b, Binding)}
        assert actions.get("search") == "toggle_filter"
        assert "focus_search" not in actions

    def test_search_page_default_unbound(self) -> None:
        """search_page ships without a key: no binding and not in defaults."""
        from textual.binding import Binding

        from ytmusic_tui.app import YtMusicTui
        from ytmusic_tui.config import DEFAULT_KEYMAP

        assert "search_page" not in DEFAULT_KEYMAP
        binding_keys = [b.key for b in YtMusicTui.BINDINGS if isinstance(b, Binding)]
        binding_actions = [b.action for b in YtMusicTui.BINDINGS if isinstance(b, Binding)]
        # No compiled-in binding maps to the search_page action.
        assert "search_page" not in binding_actions
        # The example key in default_keymap.toml is not compiled in either.
        assert "ctrl+s" not in binding_keys

    @pytest.mark.asyncio
    async def test_search_page_bindable_via_keymap(self, tmp_path: Path) -> None:
        """A keymap.toml search_page entry binds the key to the search page."""
        from textual.widgets import ContentSwitcher, Input

        keymap_file = tmp_path / "keymap.toml"
        keymap_file.write_text('[keybinds]\nsearch_page = "ctrl+s"\n')

        app = _make_app(keymap_path=keymap_file)
        async with app.run_test(size=(120, 40)) as pilot:
            await pilot.press("ctrl+s")
            # Two pumps: one for on_show, one for the deferred focus_input
            # scheduled via call_after_refresh.
            await pilot.pause()
            await pilot.pause()

            switcher = app.query_one(ContentSwitcher)
            assert switcher.current == "search"
            search_input = app.query_one("#search-input", Input)
            assert app.focused is search_input

    @pytest.mark.asyncio
    async def test_search_page_unbound_without_keymap_entry(self) -> None:
        """Without a keymap entry, no key triggers the search page action."""
        from textual.binding import Binding

        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            # The live binding table carries no search_page action because
            # _apply_keymap only binds it when keymap.toml opts in.
            active_actions = {
                b.action
                for bindings in app._bindings.key_to_bindings.values()
                for b in bindings
                if isinstance(b, Binding)
            }
            assert "search_page" not in active_actions


# ===================================================================
# Session probe (stale cookies served as logged-out pages)
# ===================================================================


class TestSessionProbe:
    @pytest.mark.asyncio
    async def test_probe_warns_when_signed_out(self) -> None:
        from helpers import capture_notifications

        app = _make_app()
        app.music_api.is_session_valid.return_value = False
        async with app.run_test(size=(120, 40)) as pilot:
            captured = capture_notifications(app)
            app._probe_session()
            await app.workers.wait_for_complete()
            await pilot.pause()

        assert any(
            "signed out" in message and severity == "warning" for message, severity in captured
        )

    @pytest.mark.asyncio
    async def test_probe_silent_when_session_valid(self) -> None:
        from helpers import capture_notifications

        app = _make_app()
        app.music_api.is_session_valid.return_value = True
        async with app.run_test(size=(120, 40)) as pilot:
            captured = capture_notifications(app)
            app._probe_session()
            await app.workers.wait_for_complete()
            await pilot.pause()

        assert captured == []


# ===================================================================
# Playback error surfacing
# ===================================================================


class TestPlaybackError:
    @pytest.mark.asyncio
    async def test_error_notifies_with_track_title(self) -> None:
        from helpers import capture_notifications

        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            captured = capture_notifications(app)
            app.queue_manager.set_playlist(
                [Track(video_id="vid", title="Dead Stream", artist="X")]
            )
            app._on_playback_error()

        assert any(
            "Dead Stream" in message and "press n to skip" in message and severity == "error"
            for message, severity in captured
        )

    @pytest.mark.asyncio
    async def test_error_notifies_without_track(self) -> None:
        from helpers import capture_notifications

        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            captured = capture_notifications(app)
            app._on_playback_error()

        assert captured == [("Playback failed", "error")]

    @pytest.mark.asyncio
    async def test_error_appends_description(self) -> None:
        from helpers import capture_notifications

        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            captured = capture_notifications(app)
            app.queue_manager.set_playlist(
                [Track(video_id="vid", title="Dead Stream", artist="X")]
            )
            app._on_playback_error("loading failed")

        assert any(
            "(loading failed)" in message and severity == "error" for message, severity in captured
        )

    @pytest.mark.asyncio
    async def test_error_does_not_advance_queue(self) -> None:
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            app.queue_manager.set_playlist(
                [
                    Track(video_id="a", title="A", artist="X"),
                    Track(video_id="b", title="B", artist="X"),
                ]
            )
            app._on_playback_error("loading failed")

        # Surfacing the error must not skip to the next track.
        current = app.queue_manager.current_track
        assert current is not None
        assert current.video_id == "a"


# ===================================================================
# Seek actions
# ===================================================================


class TestSeekActions:
    @pytest.mark.asyncio
    async def test_seek_forward_seeks_5s(self) -> None:
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            app.player.get_state.return_value = PlayerState(video_id="active")
            app.action_seek_forward()
            app.player.seek.assert_called_once_with(5.0)

    @pytest.mark.asyncio
    async def test_seek_backward_seeks_minus_5s(self) -> None:
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            app.player.get_state.return_value = PlayerState(video_id="active")
            app.action_seek_backward()
            app.player.seek.assert_called_once_with(-5.0)

    @pytest.mark.asyncio
    async def test_seek_ignored_when_nothing_loaded(self) -> None:
        """Seeking with no track loaded must be a no-op, not a crash."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            app.player.get_state.return_value = PlayerState()
            app.action_seek_forward()
            app.action_seek_backward()
            app.player.seek.assert_not_called()

    @pytest.mark.asyncio
    async def test_seek_error_is_swallowed(self) -> None:
        """A not-yet-seekable stream (ytdl still resolving) must not crash."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            app.player.get_state.return_value = PlayerState(video_id="active")
            app.player.seek.side_effect = SystemError("seek failed")
            app.action_seek_forward()  # must not raise

    @pytest.mark.asyncio
    async def test_seek_works_while_paused(self) -> None:
        """Seeking while paused (video loaded, not playing) is allowed."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            app.player.get_state.return_value = PlayerState(is_playing=False, video_id="active")
            app.action_seek_forward()
            app.player.seek.assert_called_once_with(5.0)


# ===================================================================
# Duration display (Bug 1)
# ===================================================================


class TestDurationDisplay:
    @pytest.mark.asyncio
    async def test_duration_shown_when_video_id_present(self) -> None:
        """When a video is loaded (video_id set), duration 0.0 should show '0:00'
        instead of the dash character."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            bar = app.query_one(PlayerBar)
            state = PlayerState(
                is_playing=True,
                position=0.0,
                duration=0.0,
                title="Loading Track",
                artist="Artist",
                video_id="abc123",
            )
            bar.update_state(state)
            time_text = bar.query_one("#player-time", Static).content
            # Should show "0:00 / 0:00" not "0:00 / —"
            assert "—" not in time_text  # no em-dash
            assert "0:00 / 0:00" in time_text

    @pytest.mark.asyncio
    async def test_duration_dash_when_no_video(self) -> None:
        """With no video loaded, duration 0.0 should still show the dash."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            bar = app.query_one(PlayerBar)
            state = PlayerState()
            bar.update_state(state)
            time_text = bar.query_one("#player-time", Static).content
            assert "—" in time_text  # em-dash present

    @pytest.mark.asyncio
    async def test_duration_shows_real_time_when_loaded(self) -> None:
        """Once mpv reports a real duration, it should display correctly."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            bar = app.query_one(PlayerBar)
            state = PlayerState(
                is_playing=True,
                position=30.0,
                duration=180.0,
                title="Real Track",
                artist="Artist",
                video_id="xyz789",
            )
            bar.update_state(state)
            time_text = bar.query_one("#player-time", Static).content
            assert "0:30" in time_text
            assert "3:00" in time_text


# ===================================================================
# Toggle pause with idle player (Bug 4)
# ===================================================================


class TestTogglePauseIdlePlayer:
    @pytest.mark.asyncio
    async def test_toggle_pause_starts_playback_when_idle_with_queue(self) -> None:
        """If no track is loaded but queue has a track, toggle_pause should
        start playing instead of just toggling pause."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from ytmusic_tui.queue import Track

            track = Track(video_id="vid_1", title="Song", artist="Art")
            app.queue_manager.add(track)
            # Ensure player reports no video loaded
            app.player.get_state.return_value = PlayerState()
            app.action_toggle_pause()
            app.player.play.assert_called_with("vid_1")

    @pytest.mark.asyncio
    async def test_toggle_pause_normal_when_playing(self) -> None:
        """If a track is already loaded, toggle_pause should just toggle."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            app.player.get_state.return_value = PlayerState(video_id="active")
            app.action_toggle_pause()
            app.player.toggle_pause.assert_called_once()


# ===================================================================
# Duration fallback from track metadata (Bug 1 & 2)
# ===================================================================


class TestDurationFallback:
    @pytest.mark.asyncio
    async def test_duration_uses_track_metadata_when_mpv_zero(self) -> None:
        """PlayerBar should show track.duration_seconds when mpv returns 0."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from ytmusic_tui.queue import Track

            track = Track(
                video_id="abc123",
                title="Loading Track",
                artist="Artist",
                duration_seconds=210.0,
            )
            app.queue_manager.set_playlist([track])
            # mpv reports 0.0 duration (still loading ytdl-hook)
            app.player.get_state.return_value = PlayerState(
                is_playing=True,
                position=0.0,
                duration=0.0,
                title="Loading Track",
                video_id="abc123",
            )

            bar = app.query_one(PlayerBar)
            bar._poll_player_state()
            await _pilot.pause()

            time_text = bar.query_one("#player-time", Static).content
            # Should show 3:30 (from track metadata) not 0:00
            assert "3:30" in time_text

    @pytest.mark.asyncio
    async def test_duration_prefers_mpv_when_nonzero(self) -> None:
        """When mpv provides a real duration, it should take priority."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from ytmusic_tui.queue import Track

            track = Track(
                video_id="abc123",
                title="Track",
                artist="Artist",
                duration_seconds=210.0,
            )
            app.queue_manager.set_playlist([track])
            # mpv now reports real duration (different from metadata)
            app.player.get_state.return_value = PlayerState(
                is_playing=True,
                position=30.0,
                duration=215.0,
                title="Track",
                video_id="abc123",
            )

            bar = app.query_one(PlayerBar)
            bar._poll_player_state()
            await _pilot.pause()

            time_text = bar.query_one("#player-time", Static).content
            # Should use mpv's 215s = 3:35
            assert "3:35" in time_text


# ===================================================================
# Mode icon formatting (unit tests, no app needed)
# ===================================================================


class TestModeIconFormatting:
    def test_shuffle_icon_on(self) -> None:
        result = format_shuffle_icon(True)
        assert "bold green" in result
        assert "S" in result

    def test_shuffle_icon_off(self) -> None:
        result = format_shuffle_icon(False)
        assert "dim" in result
        assert "S" in result

    def test_repeat_icon_all(self) -> None:
        result = format_repeat_icon(RepeatMode.ALL)
        assert "bold green" in result
        assert "R:all" in result

    def test_repeat_icon_one(self) -> None:
        result = format_repeat_icon(RepeatMode.ONE)
        assert "bold green" in result
        assert "R:one" in result

    def test_repeat_icon_off(self) -> None:
        result = format_repeat_icon(RepeatMode.OFF)
        assert "dim" in result
        assert "R" in result

    def test_format_modes_shuffle_on_repeat_all(self) -> None:
        result = format_modes(True, RepeatMode.ALL)
        assert "R:all" in result
        # Shuffle ON uses bold green S
        assert "bold green" in result

    def test_format_modes_shuffle_off_repeat_off(self) -> None:
        result = format_modes(False, RepeatMode.OFF)
        # Both off — should contain dim markers, no bold green
        assert "R:all" not in result
        assert "R:one" not in result

    def test_format_modes_shuffle_on_repeat_one(self) -> None:
        result = format_modes(True, RepeatMode.ONE)
        assert "R:one" in result
        assert "bold green" in result


# ===================================================================
# PlayerBar: album display
# ===================================================================


class TestPlayerBarAlbum:
    @pytest.mark.asyncio
    async def test_album_displayed_when_provided(self) -> None:
        """Album name should appear in the player-album widget."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            bar = app.query_one(PlayerBar)
            state = PlayerState(
                is_playing=True,
                title="Song",
                artist="Artist",
            )
            bar.update_state(state, album="Great Album")
            album_text = bar.query_one("#player-album", Static).content
            assert "Great Album" in album_text

    @pytest.mark.asyncio
    async def test_album_empty_when_not_provided(self) -> None:
        """Album widget should be empty when no album info is available."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            bar = app.query_one(PlayerBar)
            state = PlayerState(is_playing=True, title="Song", artist="Art")
            bar.update_state(state)
            album_text = bar.query_one("#player-album", Static).content
            assert album_text == ""

    @pytest.mark.asyncio
    async def test_album_from_queue_via_poll(self) -> None:
        """Polling should populate album from queue's current track."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            track = Track(
                video_id="vid_1",
                title="Song",
                artist="Artist",
                album="From Queue",
                duration_seconds=180.0,
            )
            app.queue_manager.set_playlist([track])
            app.player.get_state.return_value = PlayerState(
                is_playing=True,
                position=10.0,
                duration=180.0,
                title="Song",
                video_id="vid_1",
            )

            bar = app.query_one(PlayerBar)
            bar._poll_player_state()
            await _pilot.pause()

            album_text = bar.query_one("#player-album", Static).content
            assert "From Queue" in album_text


# ===================================================================
# PlayerBar: shuffle/repeat icons
# ===================================================================


class TestPlayerBarModes:
    @pytest.mark.asyncio
    async def test_modes_show_shuffle_on(self) -> None:
        """Modes widget should show active shuffle indicator when enabled."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            bar = app.query_one(PlayerBar)
            state = PlayerState(is_playing=True, title="S", artist="A")
            bar.update_state(state, shuffle=True)
            modes_text = bar.query_one("#player-modes", Static).content
            assert "bold green" in modes_text
            assert "S" in modes_text

    @pytest.mark.asyncio
    async def test_modes_show_repeat_all(self) -> None:
        """Modes widget should show repeat-all indicator."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            bar = app.query_one(PlayerBar)
            state = PlayerState(is_playing=True, title="S", artist="A")
            bar.update_state(state, repeat_mode=RepeatMode.ALL)
            modes_text = bar.query_one("#player-modes", Static).content
            assert "R:all" in modes_text

    @pytest.mark.asyncio
    async def test_modes_show_repeat_one(self) -> None:
        """Modes widget should show repeat-one indicator."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            bar = app.query_one(PlayerBar)
            state = PlayerState(is_playing=True, title="S", artist="A")
            bar.update_state(state, repeat_mode=RepeatMode.ONE)
            modes_text = bar.query_one("#player-modes", Static).content
            assert "R:one" in modes_text

    @pytest.mark.asyncio
    async def test_modes_dimmed_when_both_off(self) -> None:
        """Modes widget should show dimmed indicators when both off."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            bar = app.query_one(PlayerBar)
            state = PlayerState(is_playing=True, title="S", artist="A")
            bar.update_state(state, shuffle=False, repeat_mode=RepeatMode.OFF)
            modes_text = bar.query_one("#player-modes", Static).content
            # Should contain dimmed S and R, not active variants
            assert "R:all" not in modes_text
            assert "R:one" not in modes_text
            assert "dim" in modes_text

    @pytest.mark.asyncio
    async def test_modes_from_queue_via_poll(self) -> None:
        """Polling should populate modes from queue state."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            track = Track(video_id="v1", title="S", artist="A")
            app.queue_manager.set_playlist([track])
            app.queue_manager.toggle_shuffle()
            app.queue_manager.cycle_repeat()  # OFF -> ALL
            app.player.get_state.return_value = PlayerState(
                is_playing=True,
                title="S",
                video_id="v1",
            )

            bar = app.query_one(PlayerBar)
            bar._poll_player_state()
            await _pilot.pause()

            modes_text = bar.query_one("#player-modes", Static).content
            assert "bold green" in modes_text  # shuffle on
            assert "R:all" in modes_text  # repeat all


# ===================================================================
# PlayerBar: layout structure
# ===================================================================


class TestPlayerBarLayout:
    @pytest.mark.asyncio
    async def test_has_album_widget(self) -> None:
        """PlayerBar should contain a #player-album Static widget."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            bar = app.query_one(PlayerBar)
            album_widget = bar.query_one("#player-album", Static)
            assert album_widget is not None

    @pytest.mark.asyncio
    async def test_progress_and_time_on_same_row(self) -> None:
        """Progress bar and time display should both be in player-row-bottom."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            bar = app.query_one(PlayerBar)
            row_bottom = bar.query_one("#player-row-bottom")
            # Both progress and time should be children of row-bottom
            progress = row_bottom.query_one("#player-progress", Static)
            time_widget = row_bottom.query_one("#player-time", Static)
            assert progress is not None
            assert time_widget is not None

    @pytest.mark.asyncio
    async def test_album_on_middle_row(self) -> None:
        """Album widget should be in player-row-middle."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            bar = app.query_one(PlayerBar)
            row_middle = bar.query_one("#player-row-middle")
            album = row_middle.query_one("#player-album", Static)
            assert album is not None


# ===================================================================
# CLI entry point dispatch
# ===================================================================


class TestMainDispatch:
    def test_auth_subcommand_runs_setup_not_tui(self) -> None:
        from ytmusic_tui import app as app_module

        with (
            patch.object(app_module, "YtMusicTui") as mock_app_cls,
            patch("ytmusic_tui.auth.run_auth_setup", return_value=0) as mock_setup,
            patch("sys.argv", ["ytmusic-tui", "auth"]),
            pytest.raises(SystemExit) as excinfo,
        ):
            app_module.main()

        assert excinfo.value.code == 0
        mock_setup.assert_called_once_with()
        mock_app_cls.assert_not_called()
