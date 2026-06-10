"""Tests for ActionPopup and ThemePopup overlay widgets."""

from __future__ import annotations

from unittest.mock import MagicMock

import pytest
from helpers import make_app as _make_app
from helpers import make_track as _make_track

from ytmusic_tui.api import AlbumInfo, PlaylistInfo
from ytmusic_tui.queue import Track
from ytmusic_tui.views.popup import (
    ActionKind,
    ActionPopup,
    PopupAction,
    ThemePopup,
    actions_for_album,
    actions_for_playlist,
    actions_for_track,
    build_actions,
)

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _make_playlist_info(n: int = 1) -> PlaylistInfo:
    """Create a dummy PlaylistInfo."""
    return PlaylistInfo(
        playlist_id=f"PL_{n}",
        title=f"Playlist {n}",
        description=f"Description {n}",
        track_count=n * 5,
    )


def _make_album_info(n: int = 1) -> AlbumInfo:
    """Create a dummy AlbumInfo."""
    return AlbumInfo(
        browse_id=f"MPREb_{n}",
        title=f"Album {n}",
        artist=f"Artist {n}",
        year=str(2020 + n),
    )


# ===================================================================
# Unit tests: action builders
# ===================================================================


class TestActionBuilders:
    """Tests for the pure action-building functions."""

    def test_actions_for_track_count(self) -> None:
        """Track should have 7 actions."""
        track = _make_track()
        actions = actions_for_track(track)
        assert len(actions) == 7

    def test_actions_for_track_kinds(self) -> None:
        """Track actions should include Play, Add to queue, etc."""
        track = _make_track()
        actions = actions_for_track(track)
        kinds = [a.kind for a in actions]
        assert ActionKind.PLAY in kinds
        assert ActionKind.ADD_TO_QUEUE in kinds
        assert ActionKind.START_RADIO in kinds
        assert ActionKind.GO_TO_ARTIST in kinds
        assert ActionKind.GO_TO_ALBUM in kinds
        assert ActionKind.ADD_TO_PLAYLIST in kinds
        assert ActionKind.TOGGLE_LIKE in kinds

    def test_actions_for_track_add_to_playlist_enabled(self) -> None:
        """Add to playlist should be enabled."""
        track = _make_track()
        actions = actions_for_track(track)
        add_pl = next(a for a in actions if a.kind is ActionKind.ADD_TO_PLAYLIST)
        assert add_pl.enabled is True

    def test_actions_for_playlist_count(self) -> None:
        """PlaylistInfo should have 2 actions."""
        playlist = _make_playlist_info()
        actions = actions_for_playlist(playlist)
        assert len(actions) == 2

    def test_actions_for_playlist_kinds(self) -> None:
        """PlaylistInfo actions should be Play all and Open."""
        playlist = _make_playlist_info()
        actions = actions_for_playlist(playlist)
        kinds = [a.kind for a in actions]
        assert ActionKind.PLAY_ALL in kinds
        assert ActionKind.OPEN in kinds

    def test_actions_for_album_count(self) -> None:
        """AlbumInfo should have 3 actions."""
        album = _make_album_info()
        actions = actions_for_album(album)
        assert len(actions) == 3

    def test_actions_for_album_kinds(self) -> None:
        """AlbumInfo actions should be Play all, Open, Go to artist."""
        album = _make_album_info()
        actions = actions_for_album(album)
        kinds = [a.kind for a in actions]
        assert ActionKind.PLAY_ALL in kinds
        assert ActionKind.OPEN in kinds
        assert ActionKind.GO_TO_ARTIST in kinds

    def test_build_actions_dispatches_to_track(self) -> None:
        """build_actions should dispatch correctly for Track."""
        track = _make_track()
        actions = build_actions(track)
        assert len(actions) == 7
        assert actions[0].kind is ActionKind.PLAY

    def test_build_actions_dispatches_to_playlist(self) -> None:
        """build_actions should dispatch correctly for PlaylistInfo."""
        playlist = _make_playlist_info()
        actions = build_actions(playlist)
        assert len(actions) == 2
        assert actions[0].kind is ActionKind.PLAY_ALL

    def test_build_actions_dispatches_to_album(self) -> None:
        """build_actions should dispatch correctly for AlbumInfo."""
        album = _make_album_info()
        actions = build_actions(album)
        assert len(actions) == 3
        assert actions[0].kind is ActionKind.PLAY_ALL


# ===================================================================
# ActionPopup widget tests
# ===================================================================


class TestActionPopup:
    """Tests for the ActionPopup overlay widget."""

    @pytest.mark.asyncio
    async def test_popup_hidden_by_default(self) -> None:
        """ActionPopup should be hidden when the app starts."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            popup = app.query_one(ActionPopup)
            assert popup.is_visible is False

    @pytest.mark.asyncio
    async def test_popup_shows_for_track(self) -> None:
        """show() with a Track should display the popup with 5 actions."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            popup = app.query_one(ActionPopup)
            track = _make_track()
            popup.show(track)
            await _pilot.pause()

            assert popup.is_visible is True
            assert len(popup.actions) == 7
            assert popup.item is track

    @pytest.mark.asyncio
    async def test_popup_shows_for_playlist(self) -> None:
        """show() with a PlaylistInfo should display 2 actions."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            popup = app.query_one(ActionPopup)
            playlist = _make_playlist_info()
            popup.show(playlist)
            await _pilot.pause()

            assert popup.is_visible is True
            assert len(popup.actions) == 2

    @pytest.mark.asyncio
    async def test_popup_shows_for_album(self) -> None:
        """show() with an AlbumInfo should display 3 actions."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            popup = app.query_one(ActionPopup)
            album = _make_album_info()
            popup.show(album)
            await _pilot.pause()

            assert popup.is_visible is True
            assert len(popup.actions) == 3

    @pytest.mark.asyncio
    async def test_popup_dismiss(self) -> None:
        """dismiss() should hide the popup."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            popup = app.query_one(ActionPopup)
            popup.show(_make_track())
            await _pilot.pause()
            assert popup.is_visible is True

            popup.dismiss()
            await _pilot.pause()
            assert popup.is_visible is False

    @pytest.mark.asyncio
    async def test_popup_escape_dismisses(self) -> None:
        """Pressing Escape on the popup should hide it."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            popup = app.query_one(ActionPopup)
            popup.show(_make_track())
            await _pilot.pause()
            assert popup.is_visible is True

            # Simulate Escape key
            mock_event = MagicMock()
            mock_event.key = "escape"
            popup.on_key(mock_event)
            await _pilot.pause()

            assert popup.is_visible is False

    @pytest.mark.asyncio
    async def test_popup_action_play_queues_and_plays(self) -> None:
        """Selecting 'Play' on a Track should queue and play it."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            track = _make_track()

            # Directly test the app handler
            action = PopupAction(kind=ActionKind.PLAY, label="Play")
            event = ActionPopup.ActionSelected(action=action, item=track)
            app.on_action_popup_action_selected(event)

            assert len(app.queue_manager.tracks) == 1
            assert app.queue_manager.current_track == track
            app.player.play.assert_called_once_with("vid_1")

    @pytest.mark.asyncio
    async def test_popup_action_add_to_queue(self) -> None:
        """Selecting 'Add to queue' should add the track without playing."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            track = _make_track()

            action = PopupAction(kind=ActionKind.ADD_TO_QUEUE, label="Add to queue")
            event = ActionPopup.ActionSelected(action=action, item=track)
            app.on_action_popup_action_selected(event)

            assert len(app.queue_manager.tracks) == 1
            # Should NOT auto-play
            app.player.play.assert_not_called()

    @pytest.mark.asyncio
    async def test_popup_disabled_action_not_executed(self) -> None:
        """Disabled actions should not emit ActionSelected."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            popup = app.query_one(ActionPopup)
            track = _make_track()
            popup.show(track)
            await _pilot.pause()

            # Manually inject a disabled action for testing
            popup._actions[4] = PopupAction(
                kind=ActionKind.ADD_TO_PLAYLIST,
                label="Add to playlist",
                enabled=False,
            )

            mock_event = MagicMock()
            mock_event.list_view = MagicMock()
            mock_event.list_view.index = 4

            original_post = popup.post_message
            messages: list[object] = []

            def capture_message(msg: object) -> None:
                messages.append(msg)
                return original_post(msg)

            popup.post_message = capture_message  # type: ignore[assignment]

            popup.on_list_view_selected(mock_event)
            await _pilot.pause()

            action_msgs = [m for m in messages if isinstance(m, ActionPopup.ActionSelected)]
            assert len(action_msgs) == 0


# ===================================================================
# ThemePopup widget tests
# ===================================================================


class TestThemePopup:
    """Tests for the ThemePopup overlay widget."""

    @pytest.mark.asyncio
    async def test_theme_popup_hidden_by_default(self) -> None:
        """ThemePopup should be hidden when the app starts."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            popup = app.query_one(ThemePopup)
            assert popup.is_visible is False

    @pytest.mark.asyncio
    async def test_theme_popup_shows_themes(self) -> None:
        """show() should display the given theme names."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            popup = app.query_one(ThemePopup)
            popup.show(["synthwave", "nord", "gruvbox"], current_theme="synthwave")
            await _pilot.pause()

            assert popup.is_visible is True
            assert popup.theme_names == ["synthwave", "nord", "gruvbox"]

    @pytest.mark.asyncio
    async def test_theme_popup_dismiss(self) -> None:
        """dismiss() should hide the theme popup."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            popup = app.query_one(ThemePopup)
            popup.show(["synthwave", "nord"])
            await _pilot.pause()
            assert popup.is_visible is True

            popup.dismiss()
            await _pilot.pause()
            assert popup.is_visible is False

    @pytest.mark.asyncio
    async def test_theme_popup_escape_dismisses(self) -> None:
        """Pressing Escape on the theme popup should hide it."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            popup = app.query_one(ThemePopup)
            popup.show(["synthwave", "nord"])
            await _pilot.pause()

            mock_event = MagicMock()
            mock_event.key = "escape"
            popup.on_key(mock_event)
            await _pilot.pause()

            assert popup.is_visible is False

    @pytest.mark.asyncio
    async def test_theme_selection_applies_theme(self) -> None:
        """Selecting a theme should trigger the app to apply it."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            # Directly test the handler
            event = ThemePopup.ThemeSelected(theme_name="nord")
            app.on_theme_popup_theme_selected(event)
            await _pilot.pause()

            # Theme name should have been applied
            assert "nord" in app.theme


# ===================================================================
# App-level popup integration
# ===================================================================


class TestPopupAppIntegration:
    """Tests for popup keybinding wiring in the app."""

    @pytest.mark.asyncio
    async def test_action_popup_binding_exists(self) -> None:
        """The full_stop key should be bound to open_action_popup."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            keys = [b.key for b in app.BINDINGS]
            assert "full_stop" in keys

    @pytest.mark.asyncio
    async def test_theme_popup_binding_exists(self) -> None:
        """The T key should be bound to open_theme_popup."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            keys = [b.key for b in app.BINDINGS]
            assert "T" in keys

    @pytest.mark.asyncio
    async def test_action_popup_no_item_does_nothing(self) -> None:
        """Opening action popup with no focused item should be a no-op."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            # Home view with no sections = no focused item
            app.action_open_action_popup()
            await _pilot.pause()

            popup = app.query_one(ActionPopup)
            assert popup.is_visible is False

    @pytest.mark.asyncio
    async def test_theme_popup_opens(self) -> None:
        """action_open_theme_popup should show the theme popup."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            app.action_open_theme_popup()
            await _pilot.pause()

            popup = app.query_one(ThemePopup)
            assert popup.is_visible is True
            assert len(popup.theme_names) >= 4  # synthwave, nord, gruvbox, catppuccin

    @pytest.mark.asyncio
    async def test_go_back_dismisses_action_popup(self) -> None:
        """Escape (go_back) should dismiss the action popup first."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            # Manually show action popup
            popup = app.query_one(ActionPopup)
            popup.show(_make_track())
            await _pilot.pause()
            assert popup.is_visible is True

            app.action_go_back()
            await _pilot.pause()
            assert popup.is_visible is False

    @pytest.mark.asyncio
    async def test_go_back_dismisses_theme_popup(self) -> None:
        """Escape (go_back) should dismiss the theme popup first."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            popup = app.query_one(ThemePopup)
            popup.show(["synthwave", "nord"])
            await _pilot.pause()
            assert popup.is_visible is True

            app.action_go_back()
            await _pilot.pause()
            assert popup.is_visible is False


# ===================================================================
# PopupAction dataclass
# ===================================================================


class TestPopupAction:
    """Tests for the PopupAction frozen dataclass."""

    def test_popup_action_immutable(self) -> None:
        """PopupAction should be frozen (immutable)."""
        action = PopupAction(kind=ActionKind.PLAY, label="Play")
        with pytest.raises(AttributeError):
            action.label = "Changed"  # type: ignore[misc]

    def test_popup_action_default_enabled(self) -> None:
        """PopupAction should default to enabled=True."""
        action = PopupAction(kind=ActionKind.PLAY, label="Play")
        assert action.enabled is True

    def test_popup_action_disabled(self) -> None:
        """PopupAction with enabled=False should be disabled."""
        action = PopupAction(kind=ActionKind.PLAY, label="Play", enabled=False)
        assert action.enabled is False


# ===================================================================
# _item_title helper
# ===================================================================


class TestItemTitle:
    """Tests for the _item_title helper function."""

    def test_track_title_with_artist(self) -> None:
        from ytmusic_tui.views.popup import _item_title

        track = _make_track()
        assert _item_title(track) == "Song 1 - Artist 1"

    def test_track_title_without_artist(self) -> None:
        from ytmusic_tui.views.popup import _item_title

        track = Track(video_id="v1", title="Instrumental", artist="")
        assert _item_title(track) == "Instrumental"

    def test_playlist_title(self) -> None:
        from ytmusic_tui.views.popup import _item_title

        pl = _make_playlist_info()
        assert _item_title(pl) == "Playlist 1"

    def test_album_title_with_artist(self) -> None:
        from ytmusic_tui.views.popup import _item_title

        album = _make_album_info()
        assert _item_title(album) == "Album 1 - Artist 1"

    def test_album_title_without_artist(self) -> None:
        from ytmusic_tui.views.popup import _item_title

        album = AlbumInfo(browse_id="b1", title="Untitled", artist="")
        assert _item_title(album) == "Untitled"
