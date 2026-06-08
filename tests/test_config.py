"""Tests for configuration loading and theme system."""

from __future__ import annotations

import textwrap
from pathlib import Path

import pytest

from ytmusic_tui.config import (
    THEMES,
    AppConfig,
    AuthConfig,
    PlayerConfig,
    UIConfig,
    get_theme,
    load_config,
)

# ---------------------------------------------------------------------------
# Theme system
# ---------------------------------------------------------------------------


class TestThemes:
    def test_all_themes_have_required_keys(self) -> None:
        """Every theme must define the same set of CSS variables."""
        required_keys = {
            "primary",
            "secondary",
            "accent",
            "background",
            "surface",
            "primary-background",
            "text",
            "text-muted",
        }
        for name, palette in THEMES.items():
            assert set(palette.keys()) == required_keys, (
                f"Theme '{name}' is missing keys: {required_keys - set(palette.keys())}"
            )

    def test_theme_values_are_strings(self) -> None:
        """All theme values should be non-empty strings."""
        for name, palette in THEMES.items():
            for key, value in palette.items():
                assert isinstance(value, str) and value, (
                    f"Theme '{name}' key '{key}' has invalid value: {value!r}"
                )

    def test_get_theme_known(self) -> None:
        """get_theme returns the correct palette for a known theme."""
        result = get_theme("nord")
        assert result["primary"] == "#88c0d0"

    def test_get_theme_unknown_falls_back(self) -> None:
        """get_theme falls back to synthwave for unknown names."""
        result = get_theme("nonexistent")
        assert result == THEMES["synthwave"]

    def test_get_theme_returns_copy(self) -> None:
        """get_theme should return a copy, not the original dict."""
        result = get_theme("synthwave")
        result["primary"] = "modified"
        assert THEMES["synthwave"]["primary"] != "modified"

    def test_four_themes_exist(self) -> None:
        """We should have exactly four built-in themes."""
        assert len(THEMES) == 4
        assert set(THEMES.keys()) == {"synthwave", "nord", "gruvbox", "catppuccin"}


# ---------------------------------------------------------------------------
# Config dataclasses
# ---------------------------------------------------------------------------


class TestConfigDefaults:
    def test_default_app_config(self) -> None:
        """Default AppConfig should have sensible values."""
        cfg = AppConfig()
        assert cfg.auth.browser_auth_path == "~/.config/ytmusic-tui/browser.json"
        assert cfg.player.volume == 80
        assert cfg.player.backend == "mpv"
        assert cfg.player.audio_quality == "high"
        assert cfg.ui.theme == "synthwave"
        assert cfg.ui.vim_keys is True
        assert cfg.keybinds.overrides == {}

    def test_auth_config_frozen(self) -> None:
        """AuthConfig should be immutable."""
        auth = AuthConfig()
        with pytest.raises(AttributeError):
            auth.browser_auth_path = "other"  # type: ignore[misc]

    def test_player_config_frozen(self) -> None:
        """PlayerConfig should be immutable."""
        player = PlayerConfig()
        with pytest.raises(AttributeError):
            player.volume = 50  # type: ignore[misc]

    def test_ui_config_frozen(self) -> None:
        """UIConfig should be immutable."""
        ui = UIConfig()
        with pytest.raises(AttributeError):
            ui.theme = "nord"  # type: ignore[misc]


# ---------------------------------------------------------------------------
# Config loading
# ---------------------------------------------------------------------------


class TestConfigLoading:
    def test_load_from_bundled_default(self, tmp_path: Path) -> None:
        """Loading with just the bundled default should work."""
        toml_content = textwrap.dedent("""\
            [auth]
            browser_auth_path = "/custom/browser.json"

            [player]
            backend = "mpv"
            volume = 75

            [ui]
            theme = "nord"
            vim_keys = false
        """)
        bundled = tmp_path / "default.toml"
        bundled.write_text(toml_content)

        cfg = load_config(
            user_path=tmp_path / "nonexistent.toml",
            bundled_path=bundled,
        )
        assert cfg.auth.browser_auth_path == "/custom/browser.json"
        assert cfg.player.volume == 75
        assert cfg.ui.theme == "nord"
        assert cfg.ui.vim_keys is False

    def test_user_overrides_bundled(self, tmp_path: Path) -> None:
        """User config should override bundled defaults."""
        bundled = tmp_path / "default.toml"
        bundled.write_text(
            textwrap.dedent("""\
            [player]
            volume = 80

            [ui]
            theme = "synthwave"
        """)
        )

        user = tmp_path / "user.toml"
        user.write_text(
            textwrap.dedent("""\
            [player]
            volume = 50

            [ui]
            theme = "gruvbox"
        """)
        )

        cfg = load_config(user_path=user, bundled_path=bundled)
        assert cfg.player.volume == 50
        assert cfg.ui.theme == "gruvbox"

    def test_missing_both_files_returns_defaults(self, tmp_path: Path) -> None:
        """When both config files are missing, return default values."""
        cfg = load_config(
            user_path=tmp_path / "nope1.toml",
            bundled_path=tmp_path / "nope2.toml",
        )
        assert cfg.player.volume == 80
        assert cfg.ui.theme == "synthwave"

    def test_partial_user_config(self, tmp_path: Path) -> None:
        """User config with only some sections should merge cleanly."""
        bundled = tmp_path / "default.toml"
        bundled.write_text(
            textwrap.dedent("""\
            [auth]
            browser_auth_path = "~/.config/ytmusic-tui/browser.json"

            [player]
            volume = 80

            [ui]
            theme = "synthwave"
        """)
        )

        user = tmp_path / "user.toml"
        user.write_text(
            textwrap.dedent("""\
            [ui]
            theme = "catppuccin"
        """)
        )

        cfg = load_config(user_path=user, bundled_path=bundled)
        assert cfg.ui.theme == "catppuccin"
        # Non-overridden sections keep their defaults
        assert cfg.player.volume == 80

    def test_keybinds_override(self, tmp_path: Path) -> None:
        """Keybind overrides should be loaded from user config."""
        user = tmp_path / "user.toml"
        user.write_text(
            textwrap.dedent("""\
            [keybinds]
            play_pause = "p"
            quit = "ctrl+q"
        """)
        )

        cfg = load_config(
            user_path=user,
            bundled_path=tmp_path / "nonexistent.toml",
        )
        assert cfg.keybinds.overrides == {"play_pause": "p", "quit": "ctrl+q"}

    def test_load_real_bundled_default(self) -> None:
        """The real bundled default.toml should load without errors."""
        bundled = Path(__file__).parent.parent / "config" / "default.toml"
        if not bundled.exists():
            pytest.skip("Bundled default.toml not found")

        cfg = load_config(
            user_path=Path("/tmp/ytmusic-tui-test-nonexistent.toml"),
            bundled_path=bundled,
        )
        assert cfg.player.volume == 80
        assert cfg.ui.theme == "synthwave"
