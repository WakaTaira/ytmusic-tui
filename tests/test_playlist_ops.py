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
    def test_create_playlist_propagates_transport_error(self, mock_cls: MagicMock) -> None:
        """A transport/auth error must propagate, not collapse to None."""
        from ytmusic_tui.api import MusicAPI

        mock_client = mock_cls.return_value
        mock_client.create_playlist.side_effect = Exception("API error")

        api = MusicAPI("/fake/path")
        with pytest.raises(Exception, match="API error"):
            api.create_playlist("Fail")

    @patch("ytmusic_tui.api.YTMusic")
    def test_create_playlist_non_string_result_raises(self, mock_cls: MagicMock) -> None:
        """A completed call that returns an error dict (no playlist id)
        is a logical failure -> MutationFailedError."""
        from ytmusic_tui.api import MusicAPI, MutationFailedError

        mock_client = mock_cls.return_value
        mock_client.create_playlist.return_value = {"error": "bad"}

        api = MusicAPI("/fake/path")
        with pytest.raises(MutationFailedError):
            api.create_playlist("Weird")


class TestAddPlaylistItems:
    @patch("ytmusic_tui.api.YTMusic")
    def test_add_items_success_dict(self, mock_cls: MagicMock) -> None:
        from ytmusic_tui.api import MusicAPI

        mock_client = mock_cls.return_value
        mock_client.add_playlist_items.return_value = {"status": "STATUS_SUCCEEDED"}

        api = MusicAPI("/fake/path")
        # Success returns None; absence of an exception is the contract.
        assert api.add_playlist_items("PL123", ["vid1", "vid2"]) is None

    @patch("ytmusic_tui.api.YTMusic")
    def test_add_items_success_string(self, mock_cls: MagicMock) -> None:
        from ytmusic_tui.api import MusicAPI

        mock_client = mock_cls.return_value
        mock_client.add_playlist_items.return_value = "STATUS_SUCCEEDED"

        api = MusicAPI("/fake/path")
        assert api.add_playlist_items("PL123", ["vid1"]) is None

    @patch("ytmusic_tui.api.YTMusic")
    def test_add_items_logical_failure_raises(self, mock_cls: MagicMock) -> None:
        """HTTP succeeded but status != STATUS_SUCCEEDED -> MutationFailedError."""
        from ytmusic_tui.api import MusicAPI, MutationFailedError

        mock_client = mock_cls.return_value
        mock_client.add_playlist_items.return_value = {"status": "STATUS_FAILED"}

        api = MusicAPI("/fake/path")
        with pytest.raises(MutationFailedError):
            api.add_playlist_items("PL123", ["vid1"])

    @patch("ytmusic_tui.api.YTMusic")
    def test_add_items_propagates_transport_error(self, mock_cls: MagicMock) -> None:
        """A transport/auth error must propagate, not collapse to False."""
        from ytmusic_tui.api import MusicAPI

        mock_client = mock_cls.return_value
        mock_client.add_playlist_items.side_effect = Exception("Network")

        api = MusicAPI("/fake/path")
        with pytest.raises(Exception, match="Network"):
            api.add_playlist_items("PL123", ["vid1"])


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
        # Success returns None; absence of an exception is the contract.
        assert api.remove_playlist_items("PL123", ["vid1"]) is None
        mock_client.remove_playlist_items.assert_called_once_with(
            "PL123", [{"videoId": "vid1", "setVideoId": "set1"}]
        )

    @patch("ytmusic_tui.api.YTMusic")
    def test_remove_items_video_not_found_raises(self, mock_cls: MagicMock) -> None:
        """No matching setVideoId found is a logical failure -> MutationFailedError."""
        from ytmusic_tui.api import MusicAPI, MutationFailedError

        mock_client = mock_cls.return_value
        mock_client.get_playlist.return_value = {"tracks": []}

        api = MusicAPI("/fake/path")
        with pytest.raises(MutationFailedError):
            api.remove_playlist_items("PL123", ["vid_missing"])
        mock_client.remove_playlist_items.assert_not_called()

    @patch("ytmusic_tui.api.YTMusic")
    def test_remove_items_logical_failure_raises(self, mock_cls: MagicMock) -> None:
        """Found the track but the service reported non-success -> MutationFailedError."""
        from ytmusic_tui.api import MusicAPI, MutationFailedError

        mock_client = mock_cls.return_value
        mock_client.get_playlist.return_value = {
            "tracks": [{"videoId": "vid1", "setVideoId": "set1"}]
        }
        mock_client.remove_playlist_items.return_value = {"status": "STATUS_FAILED"}

        api = MusicAPI("/fake/path")
        with pytest.raises(MutationFailedError):
            api.remove_playlist_items("PL123", ["vid1"])

    @patch("ytmusic_tui.api.YTMusic")
    def test_remove_items_propagates_transport_error(self, mock_cls: MagicMock) -> None:
        """A transport/auth error must propagate, not collapse to False."""
        from ytmusic_tui.api import MusicAPI

        mock_client = mock_cls.return_value
        mock_client.get_playlist.side_effect = Exception("API error")

        api = MusicAPI("/fake/path")
        with pytest.raises(Exception, match="API error"):
            api.remove_playlist_items("PL123", ["vid1"])


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
    async def test_add_to_existing_notifies_logical_failure(self) -> None:
        """A logical failure (service rejected the add) surfaces the
        MutationFailedError message verbatim, which is more specific than the
        old generic "Could not add to playlist"."""
        from ytmusic_tui.api import MutationFailedError

        def _configure(api) -> None:
            api.add_playlist_items.side_effect = MutationFailedError(
                "Tracks were not added to the playlist"
            )

        app = make_app(configure_api=_configure)
        async with app.run_test(size=(120, 40)) as pilot:
            captured = _capture_notifications(app)
            app._add_to_existing_playlist("PL123", _make_track())
            await app.workers.wait_for_complete()
            await pilot.pause()

        assert ("Tracks were not added to the playlist", "error") in captured

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
            api.add_playlist_items.return_value = None

        app = make_app(configure_api=_configure)
        async with app.run_test(size=(120, 40)) as pilot:
            captured = _capture_notifications(app)
            app._create_and_add(_make_track())
            await app.workers.wait_for_complete()
            await pilot.pause()

        assert ("Created playlist and added track", "information") in captured

    @pytest.mark.asyncio
    async def test_create_and_add_notifies_create_failure(self) -> None:
        """A create that the service rejects surfaces the MutationFailedError
        message verbatim."""
        from ytmusic_tui.api import MutationFailedError

        def _configure(api) -> None:
            api.create_playlist.side_effect = MutationFailedError("Playlist was not created")

        app = make_app(configure_api=_configure)
        async with app.run_test(size=(120, 40)) as pilot:
            captured = _capture_notifications(app)
            app._create_and_add(_make_track())
            await app.workers.wait_for_complete()
            await pilot.pause()

        assert ("Playlist was not created", "error") in captured

    @pytest.mark.asyncio
    async def test_create_and_add_notifies_partial_failure(self) -> None:
        """Create succeeds but add fails: the message must say the playlist
        now exists (empty) and why the track was not added."""
        from ytmusic_tui.api import MutationFailedError

        def _configure(api) -> None:
            api.create_playlist.return_value = "PLnew"
            api.add_playlist_items.side_effect = MutationFailedError(
                "Tracks were not added to the playlist"
            )

        app = make_app(configure_api=_configure)
        async with app.run_test(size=(120, 40)) as pilot:
            captured = _capture_notifications(app)
            app._create_and_add(_make_track())
            await app.workers.wait_for_complete()
            await pilot.pause()

        assert any(
            message.startswith("Playlist created, but adding the track failed")
            and "Tracks were not added" in message
            and severity == "error"
            for message, severity in captured
        )

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
