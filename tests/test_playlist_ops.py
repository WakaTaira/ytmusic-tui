"""Tests for playlist mutation (create, add items) and PlaylistPickerPopup."""

from __future__ import annotations

from unittest.mock import MagicMock, patch

import pytest

from ytmusic_tui.api import PlaylistInfo
from ytmusic_tui.queue import Track
from ytmusic_tui.views.popup import PlaylistPickerPopup


def _make_track(n: int = 1) -> Track:
    return Track(
        video_id=f"vid{n}",
        title=f"Song {n}",
        artist=f"Artist {n}",
        album=f"Album {n}",
        duration_seconds=200.0,
    )


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
        from ytmusic_tui.views.popup import actions_for_queue_track, ActionKind

        track = _make_track()
        actions = actions_for_queue_track(track)
        kinds = [a.kind for a in actions]
        assert ActionKind.REMOVE_FROM_QUEUE in kinds

    def test_playlist_track_has_remove(self) -> None:
        from ytmusic_tui.views.popup import actions_for_playlist_track, ActionKind

        track = _make_track()
        actions = actions_for_playlist_track(track)
        kinds = [a.kind for a in actions]
        assert ActionKind.REMOVE_FROM_PLAYLIST in kinds

    def test_build_actions_with_queue_context(self) -> None:
        from ytmusic_tui.views.popup import build_actions, ActionKind

        track = _make_track()
        actions = build_actions(track, context="queue")
        kinds = [a.kind for a in actions]
        assert ActionKind.REMOVE_FROM_QUEUE in kinds
        assert ActionKind.ADD_TO_QUEUE not in kinds

    def test_build_actions_with_playlist_context(self) -> None:
        from ytmusic_tui.views.popup import build_actions, ActionKind

        track = _make_track()
        actions = build_actions(track, context="playlist_tracks")
        kinds = [a.kind for a in actions]
        assert ActionKind.REMOVE_FROM_PLAYLIST in kinds

    def test_build_actions_default_context(self) -> None:
        from ytmusic_tui.views.popup import build_actions, ActionKind

        track = _make_track()
        actions = build_actions(track)
        kinds = [a.kind for a in actions]
        assert ActionKind.REMOVE_FROM_QUEUE not in kinds
        assert ActionKind.REMOVE_FROM_PLAYLIST not in kinds


class TestPlaylistPickerPopup:
    def test_sentinel_value(self) -> None:
        assert PlaylistPickerPopup._NEW_PLAYLIST_SENTINEL == "__new__"

    def test_initial_state(self) -> None:
        popup = PlaylistPickerPopup()
        assert popup._playlists == []
        assert popup._track is None
