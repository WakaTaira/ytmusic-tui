"""Tests for Sprint 7 features: like, radio, history, mute, seek-to-start."""

from __future__ import annotations

from types import SimpleNamespace
from unittest.mock import MagicMock, patch

import pytest
from helpers import capture_notifications, make_app, make_track, make_tracks

from ytmusic_tui.api import MusicAPI, _watch_item_to_track
from ytmusic_tui.player import PlayerState

# ---------------------------------------------------------------------------
# Watch-playlist item conversion
# ---------------------------------------------------------------------------


def _watch_item(**overrides: object) -> dict:
    item: dict = {
        "videoId": "vid_w1",
        "title": "Watch Song",
        "artists": [{"name": "Artist W", "id": "UC1"}],
        "album": {"name": "Album W", "id": "MPREb_w"},
        "length": "4:48",
        "thumbnail": [{"url": "https://img/wt.jpg", "width": 226, "height": 226}],
        "likeStatus": "INDIFFERENT",
    }
    item.update(overrides)
    return item


class TestWatchItemConversion:
    def test_basic(self) -> None:
        track = _watch_item_to_track(_watch_item())
        assert track is not None
        assert track.video_id == "vid_w1"
        assert track.title == "Watch Song"
        assert track.artist == "Artist W"
        assert track.album == "Album W"
        assert track.duration_seconds == 288.0
        assert track.thumbnail_url == "https://img/wt.jpg"

    def test_missing_video_id(self) -> None:
        assert _watch_item_to_track(_watch_item(videoId=None)) is None

    def test_album_none(self) -> None:
        track = _watch_item_to_track(_watch_item(album=None))
        assert track is not None
        assert track.album == ""


# ---------------------------------------------------------------------------
# API: radio / history / likes
# ---------------------------------------------------------------------------


class TestGetRadio:
    @patch("ytmusic_tui.api.YTMusic")
    def test_parses_and_filters(self, mock_cls: MagicMock) -> None:
        client = MagicMock()
        client.get_watch_playlist.return_value = {
            "tracks": [_watch_item(), _watch_item(videoId=None)]
        }
        mock_cls.return_value = client

        tracks = MusicAPI("/fake").get_radio("seed", limit=10)

        client.get_watch_playlist.assert_called_once_with("seed", radio=True, limit=10)
        assert len(tracks) == 1
        assert tracks[0].video_id == "vid_w1"

    @patch("ytmusic_tui.api.YTMusic")
    def test_non_dict_response_returns_empty(self, mock_cls: MagicMock) -> None:
        client = MagicMock()
        client.get_watch_playlist.return_value = "error page"
        mock_cls.return_value = client

        assert MusicAPI("/fake").get_radio("seed") == []


class TestGetHistory:
    @patch("ytmusic_tui.api.YTMusic")
    def test_parses_and_filters(self, mock_cls: MagicMock) -> None:
        client = MagicMock()
        client.get_history.return_value = [
            {
                "videoId": "h1",
                "title": "Hist 1",
                "artists": [{"name": "A"}],
                "duration": "3:00",
            },
            {"title": "no video id"},
        ]
        mock_cls.return_value = client

        tracks = MusicAPI("/fake").get_history()

        assert [t.video_id for t in tracks] == ["h1"]
        assert tracks[0].duration_seconds == 180.0


class TestLikeStatus:
    @patch("ytmusic_tui.api.YTMusic")
    def test_finds_seed_status(self, mock_cls: MagicMock) -> None:
        client = MagicMock()
        client.get_watch_playlist.return_value = {
            "tracks": [_watch_item(videoId="seed", likeStatus="LIKE")]
        }
        mock_cls.return_value = client

        assert MusicAPI("/fake").get_like_status("seed") == "LIKE"

    @patch("ytmusic_tui.api.YTMusic")
    def test_unknown_track_returns_none(self, mock_cls: MagicMock) -> None:
        client = MagicMock()
        client.get_watch_playlist.return_value = {"tracks": [_watch_item(videoId="other")]}
        mock_cls.return_value = client

        assert MusicAPI("/fake").get_like_status("seed") is None

    @patch("ytmusic_tui.api.YTMusic")
    def test_rate_track_success(self, mock_cls: MagicMock) -> None:
        client = MagicMock()
        mock_cls.return_value = client

        # Success returns None; the call reaching the client is the contract.
        assert MusicAPI("/fake").rate_track("v", "LIKE") is None
        client.rate_song.assert_called_once_with("v", "LIKE")

    @patch("ytmusic_tui.api.YTMusic")
    def test_rate_track_propagates_error(self, mock_cls: MagicMock) -> None:
        """A transport/auth error must propagate so the worker can
        classify it, instead of being swallowed into a False sentinel."""
        client = MagicMock()
        client.rate_song.side_effect = Exception("api error")
        mock_cls.return_value = client

        with pytest.raises(Exception, match="api error"):
            MusicAPI("/fake").rate_track("v", "LIKE")


# ---------------------------------------------------------------------------
# Player: mute
# ---------------------------------------------------------------------------


class TestMute:
    @patch("ytmusic_tui.player.mpv.MPV")
    def test_toggle_mute_flips(self, mock_mpv_cls: MagicMock) -> None:
        from ytmusic_tui.player import Player

        mock_mpv = mock_mpv_cls.return_value
        mock_mpv.mute = False
        player = Player()
        player.toggle_mute()
        assert mock_mpv.mute is True
        player.shutdown()

    def test_state_default_not_muted(self) -> None:
        assert PlayerState().is_muted is False


# ---------------------------------------------------------------------------
# App actions: mute / seek-to-start / like / radio
# ---------------------------------------------------------------------------


class TestMuteSeekActions:
    @pytest.mark.asyncio
    async def test_toggle_mute_action(self) -> None:
        app = make_app()
        async with app.run_test(size=(120, 40)):
            app.action_toggle_mute()
            app.player.toggle_mute.assert_called_once()

    @pytest.mark.asyncio
    async def test_seek_start_action(self) -> None:
        app = make_app()
        async with app.run_test(size=(120, 40)):
            app.player.get_state.return_value = PlayerState(video_id="active")
            app.action_seek_start()
            app.player.seek_absolute.assert_called_once_with(0.0)

    @pytest.mark.asyncio
    async def test_seek_start_noop_when_idle(self) -> None:
        app = make_app()
        async with app.run_test(size=(120, 40)):
            app.player.get_state.return_value = PlayerState()
            app.action_seek_start()
            app.player.seek_absolute.assert_not_called()


class TestLikeAction:
    @pytest.mark.asyncio
    async def test_like_current_track(self) -> None:
        app = make_app()  # get_like_status=INDIFFERENT, rate_track=True
        async with app.run_test(size=(120, 40)) as pilot:
            captured = capture_notifications(app)
            app.queue_manager.add(make_track())
            app.action_toggle_like()
            await app.workers.wait_for_complete()
            await pilot.pause()

        assert ("Liked", "information") in captured
        app.music_api.rate_track.assert_called_once_with("vid_1", "LIKE")

    @pytest.mark.asyncio
    async def test_unlike_when_already_liked(self) -> None:
        app = make_app(
            configure_api=lambda api: setattr(api.get_like_status, "return_value", "LIKE")
        )
        async with app.run_test(size=(120, 40)) as pilot:
            captured = capture_notifications(app)
            app.queue_manager.add(make_track())
            app.action_toggle_like()
            await app.workers.wait_for_complete()
            await pilot.pause()

        assert ("Like removed", "information") in captured
        app.music_api.rate_track.assert_called_once_with("vid_1", "INDIFFERENT")

    @pytest.mark.asyncio
    async def test_like_with_no_track_is_noop(self) -> None:
        app = make_app()
        async with app.run_test(size=(120, 40)) as pilot:
            app.action_toggle_like()
            await app.workers.wait_for_complete()
            await pilot.pause()

        app.music_api.rate_track.assert_not_called()

    @pytest.mark.asyncio
    async def test_like_failure_notifies_classified_error(self) -> None:
        """A rate failure now propagates as an exception, so the toast is
        the classified message (auth-aware), not a generic "Could not
        update like"."""

        def _configure(api: MagicMock) -> None:
            api.rate_track.side_effect = Exception("403 Forbidden")

        app = make_app(configure_api=_configure)
        async with app.run_test(size=(120, 40)) as pilot:
            captured = capture_notifications(app)
            app.queue_manager.add(make_track())
            app.action_toggle_like()
            await app.workers.wait_for_complete()
            await pilot.pause()

        assert any(
            "Auth expired" in message and severity == "error" for message, severity in captured
        )


class TestRadioAction:
    @pytest.mark.asyncio
    async def test_radio_from_current_track(self) -> None:
        radio_tracks = make_tracks(3)
        app = make_app(
            configure_api=lambda api: setattr(api.get_radio, "return_value", radio_tracks)
        )
        async with app.run_test(size=(120, 40)) as pilot:
            app.queue_manager.add(make_track(9))
            app.action_start_radio()
            await app.workers.wait_for_complete()
            await pilot.pause()

            app.music_api.get_radio.assert_called_once_with("vid_9")
            app.player.play.assert_called_with("vid_1")
            assert app.queue_manager.tracks == radio_tracks

    @pytest.mark.asyncio
    async def test_radio_empty_warns(self) -> None:
        app = make_app()  # get_radio=[]
        async with app.run_test(size=(120, 40)) as pilot:
            captured = capture_notifications(app)
            app.queue_manager.add(make_track())
            app.action_start_radio()
            await app.workers.wait_for_complete()
            await pilot.pause()

        assert ("Radio returned no tracks", "warning") in captured


# ---------------------------------------------------------------------------
# History view
# ---------------------------------------------------------------------------


class TestHistoryView:
    @pytest.mark.asyncio
    async def test_switch_populates_table(self) -> None:
        from textual.widgets import DataTable

        from ytmusic_tui.views.history import HistoryView

        history = make_tracks(4)
        app = make_app(configure_api=lambda api: setattr(api.get_history, "return_value", history))
        async with app.run_test(size=(120, 40)) as pilot:
            app.action_switch_view("history")
            await app.workers.wait_for_complete()
            await pilot.pause()

            view = app.query_one(HistoryView)
            table = view.query_one("#history-table", DataTable)
            assert table.row_count == 4

    @pytest.mark.asyncio
    async def test_enter_queues_from_position(self) -> None:
        from ytmusic_tui.views.history import HistoryView

        history = make_tracks(5)
        app = make_app()
        async with app.run_test(size=(120, 40)) as pilot:
            view = app.query_one(HistoryView)
            view._populate(history)
            await pilot.pause()

            view.on_data_table_row_selected(SimpleNamespace(cursor_row=2))  # type: ignore[arg-type]

            app.player.play.assert_called_once_with("vid_3")
            assert app.queue_manager.current_track == history[2]

    @pytest.mark.asyncio
    async def test_get_focused_item_empty(self) -> None:
        from ytmusic_tui.views.history import HistoryView

        app = make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            view = app.query_one(HistoryView)
            assert view.get_focused_item() is None


class TestNewBindings:
    @pytest.mark.asyncio
    async def test_all_new_bindings_present(self) -> None:
        app = make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            actions = {b.key: b.action for b in app.BINDINGS}
            assert actions.get("f") == "toggle_like"
            assert actions.get("R") == "start_radio"
            assert actions.get("underscore") == "toggle_mute"
            assert actions.get("circumflex_accent") == "seek_start"
            assert actions.get("H") == "switch_view('history')"

    def test_new_actions_remappable_via_keymap(self) -> None:
        from ytmusic_tui.app import YtMusicTui

        for name in ("toggle_like", "start_radio", "toggle_mute", "seek_start"):
            assert YtMusicTui._ACTION_TO_TEXTUAL[name] == name
        assert YtMusicTui._ACTION_TO_TEXTUAL["switch_history"] == "switch_view('history')"
