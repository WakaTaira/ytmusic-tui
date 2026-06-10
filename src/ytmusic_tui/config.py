"""Configuration management (TOML-based).

Reads user config from ~/.config/ytmusic-tui/config.toml,
falling back to the bundled config/default.toml.  Provides
typed dataclasses for each config section and a theme system
with CSS custom property palettes.
"""

from __future__ import annotations

import tomllib
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from textual.theme import Theme

# ---------------------------------------------------------------------------
# Config dataclasses
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class AuthConfig:
    """Authentication-related settings."""

    browser_auth_path: str = "~/.config/ytmusic-tui/browser.json"


@dataclass(frozen=True)
class PlayerConfig:
    """Playback engine settings."""

    backend: str = "mpv"
    volume: int = 80
    audio_quality: str = "high"


@dataclass(frozen=True)
class UIConfig:
    """User-interface settings."""

    theme: str = "synthwave"


@dataclass(frozen=True)
class AppConfig:
    """Top-level application configuration."""

    auth: AuthConfig = field(default_factory=AuthConfig)
    player: PlayerConfig = field(default_factory=PlayerConfig)
    ui: UIConfig = field(default_factory=UIConfig)


# ---------------------------------------------------------------------------
# Theme system
# ---------------------------------------------------------------------------

# Each theme maps Textual CSS variable names to colour values.
# These are applied via App.set_css_variables() at startup.

THEMES: dict[str, dict[str, str]] = {
    "synthwave": {
        "primary": "#ff77e9",
        "secondary": "#00e5ff",
        "accent": "#b967ff",
        "background": "#1a1025",
        "surface": "#241734",
        "primary-background": "#2d1b4e",
        "text": "#eee5f5",
        "text-muted": "#9a8aad",
    },
    "nord": {
        "primary": "#88c0d0",
        "secondary": "#81a1c1",
        "accent": "#8fbcbb",
        "background": "#2e3440",
        "surface": "#3b4252",
        "primary-background": "#434c5e",
        "text": "#eceff4",
        "text-muted": "#a0a8b7",
    },
    "gruvbox": {
        "primary": "#fe8019",
        "secondary": "#fabd2f",
        "accent": "#d65d0e",
        "background": "#282828",
        "surface": "#3c3836",
        "primary-background": "#504945",
        "text": "#ebdbb2",
        "text-muted": "#a89984",
    },
    "catppuccin": {
        "primary": "#cba6f7",
        "secondary": "#f5c2e7",
        "accent": "#b4befe",
        "background": "#1e1e2e",
        "surface": "#313244",
        "primary-background": "#45475a",
        "text": "#cdd6f4",
        "text-muted": "#a6adc8",
    },
}


def build_textual_theme(name: str) -> Theme:
    """Build a Textual :class:`Theme` from a named palette.

    Falls back to ``"synthwave"`` if *name* is unknown.
    """
    palette = THEMES.get(name, THEMES["synthwave"])
    # Pass the extra keys (surface, primary-background, etc.) as
    # custom CSS variables that Textual will make available in TCSS.
    variables: dict[str, str] = {}
    for key in ("primary-background", "text", "text-muted"):
        if key in palette:
            variables[key] = palette[key]

    return Theme(
        name=f"ytm-{name}",
        primary=palette["primary"],
        secondary=palette.get("secondary", palette["primary"]),
        accent=palette.get("accent", palette["primary"]),
        background=palette.get("background"),
        surface=palette.get("surface"),
        dark=True,
        variables=variables,
    )


# ---------------------------------------------------------------------------
# Config loading
# ---------------------------------------------------------------------------

# Bundled default config that ships with the package
_BUNDLED_DEFAULT = Path(__file__).resolve().parent.parent.parent / "config" / "default.toml"

# User config location
_USER_CONFIG = Path.home() / ".config" / "ytmusic-tui" / "config.toml"


def _merge_dicts(base: dict[str, Any], override: dict[str, Any]) -> dict[str, Any]:
    """Shallow-merge *override* into *base*, section by section.

    Only top-level sections are merged; nested keys within each
    section are replaced entirely by the override.
    """
    merged: dict[str, Any] = {}
    all_keys = set(base) | set(override)
    for key in all_keys:
        base_val = base.get(key, {})
        over_val = override.get(key)
        if over_val is None:
            merged[key] = base_val
        elif isinstance(base_val, dict) and isinstance(over_val, dict):
            merged[key] = {**base_val, **over_val}
        else:
            merged[key] = over_val
    return merged


def _parse_config(data: dict[str, Any]) -> AppConfig:
    """Build an :class:`AppConfig` from a parsed TOML dict."""
    auth_raw = data.get("auth", {})
    player_raw = data.get("player", {})
    ui_raw = data.get("ui", {})

    return AppConfig(
        auth=AuthConfig(
            browser_auth_path=str(auth_raw.get("browser_auth_path", AuthConfig.browser_auth_path)),
        ),
        player=PlayerConfig(
            backend=str(player_raw.get("backend", PlayerConfig.backend)),
            volume=int(player_raw.get("volume", PlayerConfig.volume)),
            audio_quality=str(player_raw.get("audio_quality", PlayerConfig.audio_quality)),
        ),
        ui=UIConfig(
            theme=str(ui_raw.get("theme", UIConfig.theme)),
        ),
    )


def load_config(
    user_path: Path | None = None,
    bundled_path: Path | None = None,
) -> AppConfig:
    """Load configuration with user overrides on top of defaults.

    Resolution order:
    1. Bundled ``config/default.toml``
    2. User ``~/.config/ytmusic-tui/config.toml`` (overrides)

    Args:
        user_path: Override for the user config file (testing).
        bundled_path: Override for the bundled default (testing).

    Returns:
        A fully populated :class:`AppConfig` instance.
    """
    bundled = bundled_path or _BUNDLED_DEFAULT
    user = user_path or _USER_CONFIG

    base: dict[str, Any] = {}
    if bundled.is_file():
        with bundled.open("rb") as f:
            base = tomllib.load(f)

    override: dict[str, Any] = {}
    if user.is_file():
        with user.open("rb") as f:
            override = tomllib.load(f)

    merged = _merge_dicts(base, override)
    return _parse_config(merged)


# ---------------------------------------------------------------------------
# Keymap loading
# ---------------------------------------------------------------------------

# Default keymap file location
_USER_KEYMAP = Path.home() / ".config" / "ytmusic-tui" / "keymap.toml"

# Bundled default keymap that ships with the package
_BUNDLED_KEYMAP = Path(__file__).resolve().parent.parent.parent / "config" / "default_keymap.toml"

# Canonical action name -> default key mapping.
# These are the spotify_player-compatible defaults used when no
# keymap.toml is found.
DEFAULT_KEYMAP: dict[str, str] = {
    "toggle_pause": "space",
    "next_track": "n",
    "previous_track": "p",
    "toggle_shuffle": "s",
    "cycle_repeat": "r",
    "volume_up": "plus,equal",
    "volume_down": "minus",
    "search": "slash",
    "switch_home": "g",
    "switch_library": "l",
    "switch_queue": "q",
    "quit": "Q",
    "open_current_artist": "a",
    "open_current_album": "A",
    "go_back": "escape",
    "open_action_popup": "full_stop",
    "open_theme_popup": "T",
    "open_lyrics": "L",
}


def load_keymap(
    keymap_path: Path | None = None,
    bundled_path: Path | None = None,
) -> dict[str, str]:
    """Load a keymap from a TOML file, falling back to defaults.

    The TOML file is expected to have a ``[keybinds]`` section
    mapping action names to key strings::

        [keybinds]
        quit = "ctrl+q"
        toggle_pause = "space"

    Actions not present in the file keep their default binding.

    Args:
        keymap_path: Override for the user keymap file (testing).
        bundled_path: Override for the bundled default keymap (testing).

    Returns:
        A dict mapping action names to key strings.
    """
    base = dict(DEFAULT_KEYMAP)

    # Layer 1: bundled default keymap
    bundled = bundled_path or _BUNDLED_KEYMAP
    if bundled.is_file():
        with bundled.open("rb") as f:
            data = tomllib.load(f)
        keybinds = data.get("keybinds", {})
        for action, key in keybinds.items():
            if isinstance(key, str):
                base[action] = key

    # Layer 2: user keymap overrides
    user = keymap_path or _USER_KEYMAP
    if user.is_file():
        with user.open("rb") as f:
            data = tomllib.load(f)
        keybinds = data.get("keybinds", {})
        for action, key in keybinds.items():
            if isinstance(key, str):
                base[action] = key

    return base
