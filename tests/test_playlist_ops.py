"""Tests for playlist mutation (create, add items) and PlaylistPickerPopup."""

from __future__ import annotations

from unittest.mock import MagicMock, patch

import pytest
from helpers import capture_notifications as _capture_notifications
from helpers import make_app
from helpers import make_track as _make_track

from ytmusic_tui.views.popup import NEW_PLAYLIST_SENTINEL, PlaylistPickerPopup

# ---------------------------------------------------------------------------
# API methods
# ---------------------------------------------------------------------------


class TestCreatePlaylist:
    @patch("ytmusic_tui.api.YTMusic")
    def test_create_playlist_success(self, mock_cls: MagicMock) -> None:
        from ytmusic_tui.api import MusicAPI

        mock_client = mock_cls.return_value
        mock_client.create_playlist.return_value = "PLnew123"

        api = MusicAPI("/fake/path")
        result = api.create_playlist("My Playlist", "desc")
        assert result == "PLnew123"
        mock_client.create_playlist.assert_called_once_with(
            "My Playlist", "desc", privacy_status="PRIVATE"
        )

    @patch("ytmusic_tui.api.YTMusic")
    def test_create_playlist_failure(self, mock_cls: MagicMock) -> None:
        from ytmusic_tui.api import MusicAPI

        mock_client = mock_cls.return_value
        mock_client.create_playlist.side_effect = Exception("API error")

        api = MusicAPI("/fake/path")
        result = api.create_playlist("Fail")
        assert result is None

    @patch("ytmusic_tui.api.YTMusic")
    def test_create_playlist_non_string_result(self, mock_cls: MagicMock) -> None:
        from ytmusic_tui.api import MusicAPI

        mock_client = mock_cls.return_value
        mock_client.create_playlist.return_value = {"error": "bad"}

        api = MusicAPI("/fake/path")
        result = api.create_playlist("Weird")
        assert result is None


class TestAddPlaylistItems:
    @patch("ytmusic_tui.api.YTMusic")
    def test_add_items_success_dict(self, mock_cls: MagicMock) -> None:
        from ytmusic_tui.api import MusicAPI

        mock_client = mock_cls.return_value
        mock_client.add_playlist_items.return_value = {"status": "STATUS_SUCCEEDED"}

        api = MusicAPI("/fake/path")
        result = api.add_playlist_items("PL123", ["vid1", "vid2"])
        assert result is True

    @patch("ytmusic_tui.api.YTMusic")
    def test_add_items_success_string(self, mock_cls: MagicMock) -> None:
        from ytmusic_tui.api import MusicAPI

        mock_client = mock_cls.return_value
        mock_client.add_playlist_items.return_value = "STATUS_SUCCEEDED"

        api = MusicAPI("/fake/path")
        result = api.add_playlist_items("PL123", ["vid1"])
        assert result is True

    @patch("ytmusic_tui.api.YTMusic")
    def test_add_items_failure(self, mock_cls: MagicMock) -> None:
        from ytmusic_tui.api import MusicAPI

        mock_client = mock_cls.return_value
        mock_client.add_playlist_items.return_value = {"status": "STATUS_FAILED"}

        api = MusicAPI("/fake/path")
        result = api.add_playlist_items("PL123", ["vid1"])
        assert result is False

    @patch("ytmusic_tui.api.YTMusic")
    def test_add_items_exception(self, mock_cls: MagicMock) -> None:
        from ytmusic_tui.api import MusicAPI

        mock_client = mock_cls.return_value
        mock_client.add_playlist_items.side_effect = Exception("Network")

        api = MusicAPI("/fake/path")
        result = api.add_playlist_items("PL123", ["vid1"])
        assert result is False


# ---------------------------------------------------------------------------
# PlaylistPickerPopup unit tests
# ---------------------------------------------------------------------------


class TestRemovePlaylistItems:
    @patch("ytmusic_tui.api.YTMusic")
    def test_remove_items_success(self, mock_cls: MagicMock) -> None:
        from ytmusic_tui.api import MusicAPI

        mock_client = mock_cls.return_value
        mock_client.get_playlist.return_value = {
            "tracks": [
                {"videoId": "vid1", "setVideoId": "set1"},
                {"videoId": "vid2", "setVideoId": "set2"},
            ]
        }
        mock_client.remove_playlist_items.return_value = "STATUS_SUCCEEDED"

        api = MusicAPI("/fake/path")
        result = api.remove_playlist_items("PL123", ["vid1"])
        assert result is True
        mock_client.remove_playlist_items.assert_called_once_with(
            "PL123", [{"videoId": "vid1", "setVideoId": "set1"}]
        )

    @patch("ytmusic_tui.api.YTMusic")
    def test_remove_items_video_not_found(self, mock_cls: MagicMock) -> None:
        from ytmusic_tui.api import MusicAPI

        mock_client = mock_cls.return_value
        mock_client.get_playlist.return_value = {"tracks": []}

        api = MusicAPI("/fake/path")
        result = api.remove_playlist_items("PL123", ["vid_missing"])
        assert result is False

    @patch("ytmusic_tui.api.YTMusic")
    def test_remove_items_exception(self, mock_cls: MagicMock) -> None:
        from ytmusic_tui.api import MusicAPI

        mock_client = mock_cls.return_value
        mock_client.get_playlist.side_effect = Exception("API error")

        api = MusicAPI("/fake/path")
        result = api.remove_playlist_items("PL123", ["vid1"])
        assert result is False


# ---------------------------------------------------------------------------
# Context-aware actions
# ---------------------------------------------------------------------------


class TestContextActions:
    def test_queue_track_has_remove(self) -> None:
        from ytmusic_tui.views.popup import ActionKind, actions_for_queue_track

        track = _make_track()
        actions = actions_for_queue_track(track)
        kinds = [a.kind for a in actions]
        assert ActionKind.REMOVE_FROM_QUEUE in kinds

    def test_playlist_track_has_remove(self) -> None:
        from ytmusic_tui.views.popup import ActionKind, actions_for_playlist_track

        track = _make_track()
        actions = actions_for_playlist_track(track)
        kinds = [a.kind for a in actions]
        assert ActionKind.REMOVE_FROM_PLAYLIST in kinds

    def test_build_actions_with_queue_context(self) -> None:
        from ytmusic_tui.views.popup import ActionKind, build_actions

        track = _make_track()
        actions = build_actions(track, context="queue")
        kinds = [a.kind for a in actions]
        assert ActionKind.REMOVE_FROM_QUEUE in kinds
        assert ActionKind.ADD_TO_QUEUE not in kinds

    def test_build_actions_with_playlist_context(self) -> None:
        from ytmusic_tui.views.popup import ActionKind, build_actions

        track = _make_track()
        actions = build_actions(track, context="playlist_tracks")
        kinds = [a.kind for a in actions]
        assert ActionKind.REMOVE_FROM_PLAYLIST in kinds

    def test_build_actions_default_context(self) -> None:
        from ytmusic_tui.views.popup import ActionKind, build_actions

        track = _make_track()
        actions = build_actions(track)
        kinds = [a.kind for a in actions]
        assert ActionKind.REMOVE_FROM_QUEUE not in kinds
        assert ActionKind.REMOVE_FROM_PLAYLIST not in kinds


class TestPlaylistPickerPopup:
    def test_sentinel_value(self) -> None:
        assert NEW_PLAYLIST_SENTINEL == "__new__"

    def test_initial_state(self) -> None:
        popup = PlaylistPickerPopup()
        assert popup._playlists == []
        assert popup._track is None


# ---------------------------------------------------------------------------
# User feedback for playlist operations (worker -> notify)
# ---------------------------------------------------------------------------


class TestPlaylistOpsFeedback:
    """Playlist mutations must give visible success/failure feedback
    instead of silently swallowing errors."""

    @pytest.mark.asyncio
    async def test_add_to_existing_notifies_success(self) -> None:
        app = make_app(
            configure_api=lambda api: setattr(api.add_playlist_items, "return_value", True)
        )
        async with app.run_test(size=(120, 40)) as pilot:
            captured = _capture_notifications(app)
            app._add_to_existing_playlist("PL123", _make_track())
            await app.workers.wait_for_complete()
            await pilot.pause()

        assert ("Added to playlist", "information") in captured

    @pytest.mark.asyncio
    async def test_add_to_existing_notifies_failure(self) -> None:
        app = make_app(
            configure_api=lambda api: setattr(api.add_playlist_items, "return_value", False)
        )
        async with app.run_test(size=(120, 40)) as pilot:
            captured = _capture_notifications(app)
            app._add_to_existing_playlist("PL123", _make_track())
            await app.workers.wait_for_complete()
            await pilot.pause()

        assert ("Could not add to playlist", "error") in captured

    @pytest.mark.asyncio
    async def test_add_to_existing_classifies_auth_error(self) -> None:
        def _configure(api) -> None:
            api.add_playlist_items.side_effect = Exception("401 Unauthorized")

        app = make_app(configure_api=_configure)
        async with app.run_test(size=(120, 40)) as pilot:
            captured = _capture_notifications(app)
            app._add_to_existing_playlist("PL123", _make_track())
            await app.workers.wait_for_complete()
            await pilot.pause()

        assert any(
            "Auth expired" in message and severity == "error" for message, severity in captured
        )

    @pytest.mark.asyncio
    async def test_create_and_add_notifies_success(self) -> None:
        def _configure(api) -> None:
            api.create_playlist.return_value = "PLnew"
            api.add_playlist_items.return_value = True

        app = make_app(configure_api=_configure)
        async with app.run_test(size=(120, 40)) as pilot:
            captured = _capture_notifications(app)
            app._create_and_add(_make_track())
            await app.workers.wait_for_complete()
            await pilot.pause()

        assert ("Created playlist and added track", "information") in captured

    @pytest.mark.asyncio
    async def test_create_and_add_notifies_create_failure(self) -> None:
        def _configure(api) -> None:
            api.create_playlist.return_value = None

        app = make_app(configure_api=_configure)
        async with app.run_test(size=(120, 40)) as pilot:
            captured = _capture_notifications(app)
            app._create_and_add(_make_track())
            await app.workers.wait_for_complete()
            await pilot.pause()

        assert ("Could not create playlist", "error") in captured

    @pytest.mark.asyncio
    async def test_play_all_playlist_notifies_on_api_error(self) -> None:
        def _configure(api) -> None:
            api.get_playlist_tracks.side_effect = Exception("Connection refused")

        app = make_app(configure_api=_configure)
        async with app.run_test(size=(120, 40)) as pilot:
            captured = _capture_notifications(app)
            app._play_all_playlist("PL123")
            await app.workers.wait_for_complete()
            await pilot.pause()

        assert any(
            "Network error" in message and severity == "error" for message, severity in captured
        )
