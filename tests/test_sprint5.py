"""Tests for Sprint 5: Responsive layout and keymap.toml support."""

from __future__ import annotations

import textwrap
from pathlib import Path
from unittest.mock import patch

import pytest

from ytmusic_tui.config import DEFAULT_KEYMAP, load_keymap
from ytmusic_tui.layout import Orientation, detect_orientation
from ytmusic_tui.player import PlayerState

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _make_app(keymap_path: Path | None = None):
    """Create a YtMusicTui app with mocked dependencies."""
    with (
        patch("ytmusic_tui.app.MusicAPI") as mock_api_cls,
        patch("ytmusic_tui.app.Player") as mock_player_cls,
    ):
        mock_api = mock_api_cls.return_value
        mock_api.get_home.return_value = []
        mock_api.search.return_value = []
        mock_api.get_library_playlists.return_value = []
        mock_api.get_library_albums.return_value = []
        mock_api.get_library_artists.return_value = []
        mock_api.get_playlist_tracks.return_value = []
        mock_api.get_liked_songs.return_value = []

        mock_player = mock_player_cls.return_value
        mock_player.get_state.return_value = PlayerState()

        from ytmusic_tui.app import YtMusicTui

        app = YtMusicTui(auth_path="/fake/auth.json", keymap_path=keymap_path)
        return app


# ===================================================================
# detect_orientation
# ===================================================================


class TestDetectOrientation:
    def test_wide_terminal_is_horizontal(self) -> None:
        """A wide terminal (ratio > 2.3) should return HORIZONTAL."""
        assert detect_orientation(240, 40) is Orientation.HORIZONTAL

    def test_narrow_terminal_is_vertical(self) -> None:
        """A narrow terminal (ratio <= 2.3) should return VERTICAL."""
        assert detect_orientation(80, 40) is Orientation.VERTICAL

    def test_exact_threshold_is_vertical(self) -> None:
        """Exactly 2.3 ratio should return VERTICAL (not strictly greater)."""
        # 230 / 100 = 2.3 exactly
        assert detect_orientation(230, 100) is Orientation.VERTICAL

    def test_just_above_threshold_is_horizontal(self) -> None:
        """Just above 2.3 should return HORIZONTAL."""
        # 231 / 100 = 2.31
        assert detect_orientation(231, 100) is Orientation.HORIZONTAL

    def test_zero_rows_returns_vertical(self) -> None:
        """Zero rows should not crash; max(rows, 1) prevents division by zero."""
        result = detect_orientation(120, 0)
        assert result is Orientation.HORIZONTAL  # 120 / 1 = 120

    def test_one_row_wide(self) -> None:
        """Single-row terminal should be HORIZONTAL (ratio = columns)."""
        assert detect_orientation(100, 1) is Orientation.HORIZONTAL

    def test_typical_80x24(self) -> None:
        """Typical 80x24 terminal: 80/24 = 3.33 -> HORIZONTAL."""
        assert detect_orientation(80, 24) is Orientation.HORIZONTAL

    def test_tall_40x60(self) -> None:
        """A tall 40x60 terminal: 40/60 = 0.67 -> VERTICAL."""
        assert detect_orientation(40, 60) is Orientation.VERTICAL

    def test_square_terminal(self) -> None:
        """Square terminal: 100/100 = 1.0 -> VERTICAL."""
        assert detect_orientation(100, 100) is Orientation.VERTICAL


# ===================================================================
# load_keymap
# ===================================================================


class TestLoadKeymap:
    def test_default_keymap_when_no_file(self, tmp_path: Path) -> None:
        """When no keymap file exists, all defaults should be returned."""
        result = load_keymap(
            keymap_path=tmp_path / "nonexistent.toml",
            bundled_path=tmp_path / "also_nonexistent.toml",
        )
        assert result == DEFAULT_KEYMAP

    def test_load_from_bundled_keymap(self, tmp_path: Path) -> None:
        """Bundled keymap should override compiled-in defaults."""
        bundled = tmp_path / "default_keymap.toml"
        bundled.write_text(
            textwrap.dedent("""\
            [keybinds]
            quit = "ctrl+q"
        """)
        )

        result = load_keymap(
            keymap_path=tmp_path / "nonexistent.toml",
            bundled_path=bundled,
        )
        assert result["quit"] == "ctrl+q"
        # Other defaults preserved
        assert result["toggle_pause"] == "space"

    def test_user_keymap_overrides_bundled(self, tmp_path: Path) -> None:
        """User keymap should override bundled keymap."""
        bundled = tmp_path / "default_keymap.toml"
        bundled.write_text(
            textwrap.dedent("""\
            [keybinds]
            quit = "ctrl+q"
            next_track = "N"
        """)
        )

        user = tmp_path / "keymap.toml"
        user.write_text(
            textwrap.dedent("""\
            [keybinds]
            quit = "ctrl+shift+q"
        """)
        )

        result = load_keymap(keymap_path=user, bundled_path=bundled)
        # User overrides bundled
        assert result["quit"] == "ctrl+shift+q"
        # Bundled overrides compiled-in default
        assert result["next_track"] == "N"
        # Compiled-in default preserved
        assert result["toggle_pause"] == "space"

    def test_partial_user_keymap(self, tmp_path: Path) -> None:
        """User keymap with only some keys preserves other defaults."""
        user = tmp_path / "keymap.toml"
        user.write_text(
            textwrap.dedent("""\
            [keybinds]
            volume_up = "ctrl+up"
            volume_down = "ctrl+down"
        """)
        )

        result = load_keymap(
            keymap_path=user,
            bundled_path=tmp_path / "none.toml",
        )
        assert result["volume_up"] == "ctrl+up"
        assert result["volume_down"] == "ctrl+down"
        assert result["quit"] == "Q"  # default preserved
        assert result["toggle_pause"] == "space"

    def test_non_string_values_ignored(self, tmp_path: Path) -> None:
        """Non-string values in the keymap should be silently ignored."""
        user = tmp_path / "keymap.toml"
        user.write_text(
            textwrap.dedent("""\
            [keybinds]
            quit = 42
            toggle_pause = "x"
        """)
        )

        result = load_keymap(
            keymap_path=user,
            bundled_path=tmp_path / "none.toml",
        )
        # Non-string quit value ignored, keeps default
        assert result["quit"] == "Q"
        # String value applied
        assert result["toggle_pause"] == "x"

    def test_unknown_actions_preserved(self, tmp_path: Path) -> None:
        """Unknown action names are kept in the returned dict."""
        user = tmp_path / "keymap.toml"
        user.write_text(
            textwrap.dedent("""\
            [keybinds]
            custom_action = "ctrl+x"
        """)
        )

        result = load_keymap(
            keymap_path=user,
            bundled_path=tmp_path / "none.toml",
        )
        assert result["custom_action"] == "ctrl+x"
        # Defaults still present
        assert result["quit"] == "Q"

    def test_real_bundled_keymap_loads(self) -> None:
        """The real bundled default_keymap.toml should load without errors."""
        bundled = Path(__file__).parent.parent / "config" / "default_keymap.toml"
        if not bundled.exists():
            pytest.skip("Bundled default_keymap.toml not found")

        result = load_keymap(
            keymap_path=Path("/tmp/ytmusic-tui-test-nonexistent-keymap.toml"),
            bundled_path=bundled,
        )
        assert result["quit"] == "Q"
        assert result["toggle_pause"] == "space"

    def test_empty_keybinds_section(self, tmp_path: Path) -> None:
        """An empty [keybinds] section should not alter defaults."""
        user = tmp_path / "keymap.toml"
        user.write_text("[keybinds]\n")

        result = load_keymap(
            keymap_path=user,
            bundled_path=tmp_path / "none.toml",
        )
        assert result == DEFAULT_KEYMAP

    def test_missing_keybinds_section(self, tmp_path: Path) -> None:
        """A TOML file without [keybinds] should not alter defaults."""
        user = tmp_path / "keymap.toml"
        user.write_text("[other_section]\nfoo = 'bar'\n")

        result = load_keymap(
            keymap_path=user,
            bundled_path=tmp_path / "none.toml",
        )
        assert result == DEFAULT_KEYMAP


# ===================================================================
# App keymap integration
# ===================================================================


class TestAppKeymapIntegration:
    @pytest.mark.asyncio
    async def test_default_keymap_applied(self) -> None:
        """Default keymap should leave all bindings at their compiled-in keys."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            # The quit binding should be at "Q" by default
            found = False
            for key, bindings in app._bindings.key_to_bindings.items():
                for binding in bindings:
                    if binding.action == "quit":
                        assert key == "Q"
                        found = True
            assert found, "quit binding not found"

    @pytest.mark.asyncio
    async def test_custom_keymap_overrides_binding(self, tmp_path: Path) -> None:
        """Custom keymap should change the binding key."""
        keymap_file = tmp_path / "keymap.toml"
        keymap_file.write_text(
            textwrap.dedent("""\
            [keybinds]
            quit = "ctrl+q"
        """)
        )

        app = _make_app(keymap_path=keymap_file)
        async with app.run_test(size=(120, 40)) as _pilot:
            # Should find quit at ctrl+q
            found = False
            for key, bindings in app._bindings.key_to_bindings.items():
                for binding in bindings:
                    if binding.action == "quit":
                        assert key == "ctrl+q"
                        found = True
            assert found, "quit binding not found at ctrl+q"

    @pytest.mark.asyncio
    async def test_missing_keymap_uses_defaults(self) -> None:
        """Missing keymap file should fall back to defaults cleanly."""
        fake_path = Path("/tmp/ytmusic-tui-nonexistent-keymap-test.toml")
        app = _make_app(keymap_path=fake_path)
        async with app.run_test(size=(120, 40)) as _pilot:
            # All default bindings should be present
            binding_actions = set()
            for bindings in app._bindings.key_to_bindings.values():
                for binding in bindings:
                    binding_actions.add(binding.action)
            assert "quit" in binding_actions
            assert "toggle_pause" in binding_actions
            assert "next_track" in binding_actions

    @pytest.mark.asyncio
    async def test_keymap_stored_on_app(self) -> None:
        """The loaded keymap should be accessible as app._keymap."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            assert isinstance(app._keymap, dict)
            assert "quit" in app._keymap
            assert "toggle_pause" in app._keymap


# ===================================================================
# Responsive layout in app
# ===================================================================


class TestAppResponsiveLayout:
    @pytest.mark.asyncio
    async def test_initial_orientation_is_horizontal(self) -> None:
        """App should start with HORIZONTAL orientation."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            assert app._orientation is Orientation.HORIZONTAL

    @pytest.mark.asyncio
    async def test_resize_to_narrow_switches_to_vertical(self) -> None:
        """Resizing to narrow terminal should switch to VERTICAL."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            # Simulate a resize to a narrow terminal
            await _pilot.resize_terminal(60, 40)
            await _pilot.pause()
            # 60/40 = 1.5 -> VERTICAL
            assert app._orientation is Orientation.VERTICAL

    @pytest.mark.asyncio
    async def test_resize_to_wide_switches_to_horizontal(self) -> None:
        """Resizing to wide terminal should switch to HORIZONTAL."""
        app = _make_app()
        async with app.run_test(size=(60, 40)) as _pilot:
            # Start narrow: 60/40 = 1.5 -> VERTICAL (after resize event)
            await _pilot.resize_terminal(60, 40)
            await _pilot.pause()

            # Now go wide
            await _pilot.resize_terminal(200, 40)
            await _pilot.pause()
            # 200/40 = 5.0 -> HORIZONTAL
            assert app._orientation is Orientation.HORIZONTAL

    @pytest.mark.asyncio
    async def test_same_orientation_no_change(self) -> None:
        """Resizing within the same orientation should not trigger updates."""
        app = _make_app()
        async with app.run_test(size=(200, 40)) as _pilot:
            # 200/40 = 5.0 -> HORIZONTAL
            assert app._orientation is Orientation.HORIZONTAL

            # Still wide, same orientation
            await _pilot.resize_terminal(190, 40)
            await _pilot.pause()
            assert app._orientation is Orientation.HORIZONTAL


# ===================================================================
# LibraryView responsive layout
# ===================================================================


class TestLibraryViewOrientation:
    @pytest.mark.asyncio
    async def test_library_default_orientation(self) -> None:
        """LibraryView should start in HORIZONTAL orientation."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from ytmusic_tui.views.library import LibraryView

            view = app.query_one(LibraryView)
            assert view._orientation is Orientation.HORIZONTAL

    @pytest.mark.asyncio
    async def test_library_switch_to_vertical(self) -> None:
        """update_orientation(VERTICAL) should add vertical-layout class."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from ytmusic_tui.views.library import LibraryView

            view = app.query_one(LibraryView)
            view.update_orientation(Orientation.VERTICAL)
            await _pilot.pause()

            panes = view.query_one("#library-panes")
            assert "vertical-layout" in panes.classes
            assert view._orientation is Orientation.VERTICAL

    @pytest.mark.asyncio
    async def test_library_switch_back_to_horizontal(self) -> None:
        """Switching back to HORIZONTAL should remove vertical-layout class."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from ytmusic_tui.views.library import LibraryView

            view = app.query_one(LibraryView)
            view.update_orientation(Orientation.VERTICAL)
            await _pilot.pause()

            view.update_orientation(Orientation.HORIZONTAL)
            await _pilot.pause()

            panes = view.query_one("#library-panes")
            assert "vertical-layout" not in panes.classes

    @pytest.mark.asyncio
    async def test_library_same_orientation_noop(self) -> None:
        """update_orientation with same orientation should not re-process."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from ytmusic_tui.views.library import LibraryView

            view = app.query_one(LibraryView)
            # Already HORIZONTAL, calling HORIZONTAL again should be fine
            view.update_orientation(Orientation.HORIZONTAL)
            await _pilot.pause()

            panes = view.query_one("#library-panes")
            assert "vertical-layout" not in panes.classes


# ===================================================================
# SearchView responsive layout
# ===================================================================


class TestSearchViewOrientation:
    @pytest.mark.asyncio
    async def test_search_default_orientation(self) -> None:
        """SearchView should start in HORIZONTAL orientation."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from ytmusic_tui.views.search import SearchView

            view = app.query_one(SearchView)
            assert view._orientation is Orientation.HORIZONTAL

    @pytest.mark.asyncio
    async def test_search_switch_to_vertical(self) -> None:
        """update_orientation(VERTICAL) should add vertical-layout class."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from ytmusic_tui.views.search import SearchView

            view = app.query_one(SearchView)
            view.update_orientation(Orientation.VERTICAL)
            await _pilot.pause()

            grid = view.query_one("#search-grid")
            assert "vertical-layout" in grid.classes
            assert view._orientation is Orientation.VERTICAL

    @pytest.mark.asyncio
    async def test_search_switch_back_to_horizontal(self) -> None:
        """Switching back to HORIZONTAL should remove vertical-layout class."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from ytmusic_tui.views.search import SearchView

            view = app.query_one(SearchView)
            view.update_orientation(Orientation.VERTICAL)
            await _pilot.pause()

            view.update_orientation(Orientation.HORIZONTAL)
            await _pilot.pause()

            grid = view.query_one("#search-grid")
            assert "vertical-layout" not in grid.classes

    @pytest.mark.asyncio
    async def test_search_same_orientation_noop(self) -> None:
        """update_orientation with same orientation should not re-process."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            from ytmusic_tui.views.search import SearchView

            view = app.query_one(SearchView)
            view.update_orientation(Orientation.HORIZONTAL)
            await _pilot.pause()

            grid = view.query_one("#search-grid")
            assert "vertical-layout" not in grid.classes
