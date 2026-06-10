//! Configuration management (TOML-based).
//!
//! Reads user config from `~/.config/ytmusic-tui/config.toml`, falling back
//! to the bundled `config/default.toml` embedded at compile time.  Provides
//! typed structs for each config section and a theme system with CSS custom-
//! property palettes.  Also loads the keymap from
//! `~/.config/ytmusic-tui/keymap.toml`, merged on top of
//! `config/default_keymap.toml` (embedded) and the hard-coded [`DEFAULT_KEYMAP`].
//!
//! # Compatibility contract (directive §1)
//!
//! A user's existing `config.toml` and `keymap.toml` must work unchanged.
//! This means:
//! - Same TOML key names as the Python version.
//! - Same defaults when keys / files are absent.
//! - Same theme names and hex color values.
//! - Same keymap action names.
//! - **Unknown keys and sections are silently ignored** (forward compatibility):
//!   `deny_unknown_fields` is intentionally NOT used on any serde struct.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

// ---------------------------------------------------------------------------
// Embedded defaults (compile-time)
// ---------------------------------------------------------------------------

/// Bundled `config/default.toml` content — embedded at compile time so the
/// binary works without locating the source tree at runtime.
const BUNDLED_DEFAULT_TOML: &str = include_str!("../../../config/default.toml");

/// Bundled `config/default_keymap.toml` content — same rationale.
const BUNDLED_DEFAULT_KEYMAP_TOML: &str = include_str!("../../../config/default_keymap.toml");

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors that can occur while loading configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A TOML file exists but cannot be read from disk.
    #[error("failed to read config file at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// A file (user or bundled) contains invalid TOML.
    #[error("failed to parse config TOML: {0}")]
    Parse(#[from] toml::de::Error),
}

// ---------------------------------------------------------------------------
// Raw serde structs (section-level deserialization from merged TOML)
// ---------------------------------------------------------------------------
//
// These mirror the TOML structure exactly.  Unknown keys are silently ignored
// because `deny_unknown_fields` is absent — this matches Python's tomllib
// behaviour which loads the raw dict and then only reads known keys.

#[derive(Debug, Deserialize, Default)]
struct RawAuthConfig {
    browser_auth_path: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct RawPlayerConfig {
    backend: Option<String>,
    volume: Option<i64>,
    audio_quality: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct RawUiConfig {
    theme: Option<String>,
}

/// Top-level raw config, deserialized directly from TOML.
/// All sections are optional so a partial file is accepted.
#[derive(Debug, Deserialize, Default)]
struct RawAppConfig {
    auth: Option<RawAuthConfig>,
    player: Option<RawPlayerConfig>,
    ui: Option<RawUiConfig>,
}

// ---------------------------------------------------------------------------
// Public config structs
// ---------------------------------------------------------------------------

/// Authentication-related settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthConfig {
    /// Path to the ytmusicapi browser-auth JSON file.
    /// May start with `~` (expanded at use-time).
    pub browser_auth_path: String,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            browser_auth_path: "~/.config/ytmusic-tui/browser.json".to_owned(),
        }
    }
}

/// Playback engine settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerConfig {
    /// Audio backend (currently only `"mpv"` is supported).
    pub backend: String,
    /// Initial volume, 0–100.
    pub volume: u8,
    /// Audio quality preference: `"low"` / `"normal"` / `"high"`.
    /// Passed through raw; normalization of unknown values happens in the
    /// player module (mirrors Python's pass-through behavior).
    pub audio_quality: String,
}

impl Default for PlayerConfig {
    fn default() -> Self {
        Self {
            backend: "mpv".to_owned(),
            volume: 80,
            audio_quality: "high".to_owned(),
        }
    }
}

/// User-interface settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiConfig {
    /// Name of the active color theme (e.g. `"synthwave"`).
    pub theme: String,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            theme: "synthwave".to_owned(),
        }
    }
}

/// Top-level application configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppConfig {
    pub auth: AuthConfig,
    pub player: PlayerConfig,
    pub ui: UiConfig,
}

// ---------------------------------------------------------------------------
// Config parsing helpers
// ---------------------------------------------------------------------------

/// Merge *override_raw* on top of *base_raw* using the same shallow-merge
/// semantics as Python's `_merge_dicts`: each top-level section is merged
/// key-by-key, so a partial user section (e.g. only `[ui]`) leaves other
/// sections from the bundled default intact.
fn merge_raw(base: RawAppConfig, over: RawAppConfig) -> RawAppConfig {
    let auth = merge_section(base.auth, over.auth);
    let player = merge_section(base.player, over.player);
    let ui = merge_section(base.ui, over.ui);
    RawAppConfig {
        auth: Some(auth),
        player: Some(player),
        ui: Some(ui),
    }
}

/// Merge two optional sections: override wins field-by-field (only non-None
/// override values replace base values).
fn merge_section<T: Default + MergeSection>(base: Option<T>, over: Option<T>) -> T {
    match (base, over) {
        (None, None) => T::default(),
        (Some(b), None) => b,
        (None, Some(o)) => o,
        (Some(b), Some(o)) => b.merge(o),
    }
}

/// Section-level merge: override's `Some` fields win over base fields.
trait MergeSection: Sized {
    fn merge(self, over: Self) -> Self;
}

impl MergeSection for RawAuthConfig {
    fn merge(self, over: Self) -> Self {
        Self {
            browser_auth_path: over.browser_auth_path.or(self.browser_auth_path),
        }
    }
}

impl MergeSection for RawPlayerConfig {
    fn merge(self, over: Self) -> Self {
        Self {
            backend: over.backend.or(self.backend),
            volume: over.volume.or(self.volume),
            audio_quality: over.audio_quality.or(self.audio_quality),
        }
    }
}

impl MergeSection for RawUiConfig {
    fn merge(self, over: Self) -> Self {
        Self {
            theme: over.theme.or(self.theme),
        }
    }
}

/// Build a typed [`AppConfig`] from a merged [`RawAppConfig`], applying
/// hard-coded defaults for any key that is still absent.
fn build_config(raw: RawAppConfig) -> AppConfig {
    let defaults = AppConfig::default();
    let auth_raw = raw.auth.unwrap_or_default();
    let player_raw = raw.player.unwrap_or_default();
    let ui_raw = raw.ui.unwrap_or_default();

    AppConfig {
        auth: AuthConfig {
            browser_auth_path: auth_raw
                .browser_auth_path
                .unwrap_or(defaults.auth.browser_auth_path),
        },
        player: PlayerConfig {
            backend: player_raw.backend.unwrap_or(defaults.player.backend),
            // Clamp to u8 range; any value outside 0-100 in the TOML is
            // clamped rather than rejected, matching Python's int() cast.
            // Known minor divergence: a FLOAT literal (`volume = 80.0`)
            // errors here, while Python's int(80.0) silently accepted it.
            volume: player_raw
                .volume
                .map(|v| v.clamp(0, 100) as u8)
                .unwrap_or(defaults.player.volume),
            audio_quality: player_raw
                .audio_quality
                .unwrap_or(defaults.player.audio_quality),
        },
        ui: UiConfig {
            theme: ui_raw.theme.unwrap_or(defaults.ui.theme),
        },
    }
}

// ---------------------------------------------------------------------------
// Public config loader
// ---------------------------------------------------------------------------

/// Load application configuration with user overrides on top of bundled defaults.
///
/// Resolution order (mirrors Python's `load_config`):
/// 1. Hard-coded Rust defaults (via `Default` impls)
/// 2. Bundled `config/default.toml` (embedded as `BUNDLED_DEFAULT_TOML`)
/// 3. User `~/.config/ytmusic-tui/config.toml`
///
/// # Arguments
///
/// * `user_path` – Override for the user config file path (for testing).
///   `None` → `~/.config/ytmusic-tui/config.toml`.
/// * `bundled_str` – Override for the bundled default TOML content (for testing).
///   `None` → embedded `BUNDLED_DEFAULT_TOML`.
///
/// Using string injection for the bundled content mirrors how `test_config.py`
/// passes `bundled_path` to `load_config` — the seam allows tests to exercise
/// the merge logic without relying on file paths.
pub fn load_config(
    user_path: Option<&Path>,
    bundled_str: Option<&str>,
) -> Result<AppConfig, ConfigError> {
    // Layer 1: bundled defaults.
    let bundled_content = bundled_str.unwrap_or(BUNDLED_DEFAULT_TOML);
    let base: RawAppConfig = toml::from_str(bundled_content)?;

    // Layer 2: user overrides (file may not exist — that is fine).
    let user = user_path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(default_user_config_path);
    let over: RawAppConfig = if user.is_file() {
        let content = std::fs::read_to_string(&user)
            .map_err(|source| ConfigError::Io { path: user, source })?;
        toml::from_str(&content)?
    } else {
        RawAppConfig::default()
    };

    Ok(build_config(merge_raw(base, over)))
}

/// Default path for the user config file: `~/.config/ytmusic-tui/config.toml`.
fn default_user_config_path() -> PathBuf {
    expand_tilde(Path::new("~/.config/ytmusic-tui/config.toml"))
}

// ---------------------------------------------------------------------------
// Tilde expansion (local copy — expand_tilde in ytmusic-api::auth is private)
// ---------------------------------------------------------------------------
//
// NOTE for M5: if ytmusic-api ever exposes its expand_tilde helper publicly,
// this local copy can be removed.  For now we duplicate the logic rather than
// changing ytmusic-api's public API surface.

/// Expand a leading `~` to the user's home directory (`$HOME`).
///
/// Only a bare `~` or a `~/…` prefix is expanded; other paths are returned
/// unchanged.  Falls back to the original path when `$HOME` is unavailable.
pub(crate) fn expand_tilde(path: &Path) -> PathBuf {
    expand_tilde_with(path, std::env::var_os("HOME").map(PathBuf::from))
}

fn expand_tilde_with(path: &Path, home: Option<PathBuf>) -> PathBuf {
    let Some(text) = path.to_str() else {
        return path.to_path_buf();
    };
    if text == "~" {
        if let Some(h) = home {
            return h;
        }
    } else if let Some(rest) = text.strip_prefix("~/")
        && let Some(h) = home
    {
        return h.join(rest);
    }
    path.to_path_buf()
}

// ---------------------------------------------------------------------------
// Theme system
// ---------------------------------------------------------------------------

/// A single theme palette: CSS variable name → hex color string.
///
/// Values are stored as raw strings (e.g. `"#ff77e9"`) so that the ratatui
/// color parsing in M5 can happen at the point of use without baking in a
/// ratatui dependency here.
pub type ThemePalette = HashMap<&'static str, &'static str>;

/// All four built-in themes, keyed by name.
///
/// These are the canonical color values — source of truth is `config.py`'s
/// `THEMES` dict.  The Rust version matches it exactly.
#[must_use]
pub fn themes() -> HashMap<&'static str, ThemePalette> {
    let mut map = HashMap::with_capacity(4);

    // synthwave
    let mut t = HashMap::new();
    t.insert("primary", "#ff77e9");
    t.insert("secondary", "#00e5ff");
    t.insert("accent", "#b967ff");
    t.insert("background", "#1a1025");
    t.insert("surface", "#241734");
    t.insert("primary-background", "#2d1b4e");
    t.insert("text", "#eee5f5");
    t.insert("text-muted", "#9a8aad");
    map.insert("synthwave", t);

    // nord
    let mut t = HashMap::new();
    t.insert("primary", "#88c0d0");
    t.insert("secondary", "#81a1c1");
    t.insert("accent", "#8fbcbb");
    t.insert("background", "#2e3440");
    t.insert("surface", "#3b4252");
    t.insert("primary-background", "#434c5e");
    t.insert("text", "#eceff4");
    t.insert("text-muted", "#a0a8b7");
    map.insert("nord", t);

    // gruvbox
    let mut t = HashMap::new();
    t.insert("primary", "#fe8019");
    t.insert("secondary", "#fabd2f");
    t.insert("accent", "#d65d0e");
    t.insert("background", "#282828");
    t.insert("surface", "#3c3836");
    t.insert("primary-background", "#504945");
    t.insert("text", "#ebdbb2");
    t.insert("text-muted", "#a89984");
    map.insert("gruvbox", t);

    // catppuccin
    let mut t = HashMap::new();
    t.insert("primary", "#cba6f7");
    t.insert("secondary", "#f5c2e7");
    t.insert("accent", "#b4befe");
    t.insert("background", "#1e1e2e");
    t.insert("surface", "#313244");
    t.insert("primary-background", "#45475a");
    t.insert("text", "#cdd6f4");
    t.insert("text-muted", "#a6adc8");
    map.insert("catppuccin", t);

    map
}

/// The set of CSS variable keys that every theme must define.
pub const THEME_REQUIRED_KEYS: &[&str] = &[
    "primary",
    "secondary",
    "accent",
    "background",
    "surface",
    "primary-background",
    "text",
    "text-muted",
];

/// Look up a named theme, falling back to `"synthwave"` if the name is unknown
/// (mirrors Python's `build_textual_theme` fallback behavior).
#[must_use]
pub fn get_theme_palette(name: &str) -> ThemePalette {
    let all = themes();
    all.get(name)
        .or_else(|| all.get("synthwave"))
        .cloned()
        .expect("BUG: synthwave was removed from themes(); fix get_theme_palette's fallback")
}

// ---------------------------------------------------------------------------
// Keymap
// ---------------------------------------------------------------------------

/// The hard-coded default action → key mapping (spotify_player-compatible).
///
/// This is the innermost layer: the bundled keymap.toml and the user's
/// keymap.toml both override entries in this table.
pub static DEFAULT_KEYMAP: &[(&str, &str)] = &[
    ("toggle_pause", "space"),
    ("next_track", "n"),
    ("previous_track", "p"),
    ("toggle_shuffle", "s"),
    ("cycle_repeat", "r"),
    ("volume_up", "plus,equal"),
    ("volume_down", "minus"),
    ("search", "slash"),
    ("switch_home", "g"),
    ("switch_library", "l"),
    ("switch_queue", "q"),
    ("quit", "Q"),
    ("open_current_artist", "a"),
    ("open_current_album", "A"),
    ("go_back", "escape"),
    ("open_action_popup", "full_stop"),
    ("open_theme_popup", "T"),
    ("open_lyrics", "L"),
    ("cycle_audio_quality", "b"),
];

/// Load the effective keymap, merging three layers:
///
/// 1. Hard-coded [`DEFAULT_KEYMAP`] entries.
/// 2. Bundled `config/default_keymap.toml` (overrides defaults).
/// 3. User `~/.config/ytmusic-tui/keymap.toml` (overrides both).
///
/// Unknown action names in a TOML file are accepted and stored
/// (they extend the map rather than cause an error — mirrors Python's behavior
/// where `base[action] = key` works for any `action` string).
///
/// # Arguments
///
/// * `user_path` – Override for the user keymap file path (for testing).
///   `None` → `~/.config/ytmusic-tui/keymap.toml`.
/// * `bundled_str` – Override for the bundled keymap TOML content (for testing).
///   `None` → embedded `BUNDLED_DEFAULT_KEYMAP_TOML`.
pub fn load_keymap(
    user_path: Option<&Path>,
    bundled_str: Option<&str>,
) -> Result<HashMap<String, String>, ConfigError> {
    // Start with hard-coded defaults.
    let mut keymap: HashMap<String, String> = DEFAULT_KEYMAP
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

    // Layer 1: bundled default keymap (may add or override entries).
    let bundled_content = bundled_str.unwrap_or(BUNDLED_DEFAULT_KEYMAP_TOML);
    apply_keymap_toml(&mut keymap, bundled_content)?;

    // Layer 2: user keymap file (optional).
    let user = user_path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(default_user_keymap_path);
    if user.is_file() {
        let content = std::fs::read_to_string(&user)
            .map_err(|source| ConfigError::Io { path: user, source })?;
        apply_keymap_toml(&mut keymap, &content)?;
    }

    Ok(keymap)
}

/// Parse a keymap TOML string and apply `[keybinds]` entries to *map*.
///
/// Only `String` values under `[keybinds]` are applied (mirrors Python's
/// `if isinstance(key, str)` guard).  Unknown action names are stored without
/// error (forward compat).
fn apply_keymap_toml(map: &mut HashMap<String, String>, content: &str) -> Result<(), ConfigError> {
    #[derive(Deserialize, Default)]
    struct KeymapFile {
        keybinds: Option<HashMap<String, toml::Value>>,
    }

    let parsed: KeymapFile = toml::from_str(content)?;
    if let Some(binds) = parsed.keybinds {
        for (action, val) in binds {
            if let toml::Value::String(key_str) = val {
                map.insert(action, key_str);
            }
        }
    }
    Ok(())
}

/// Default path for the user keymap file: `~/.config/ytmusic-tui/keymap.toml`.
fn default_user_keymap_path() -> PathBuf {
    expand_tilde(Path::new("~/.config/ytmusic-tui/keymap.toml"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // -----------------------------------------------------------------------
    // Theme system (ported from TestThemes in test_config.py)
    // -----------------------------------------------------------------------

    #[test]
    fn all_themes_have_required_keys() {
        let all = themes();
        let required: std::collections::HashSet<&str> =
            THEME_REQUIRED_KEYS.iter().copied().collect();
        for (name, palette) in &all {
            let palette_keys: std::collections::HashSet<&str> = palette.keys().copied().collect();
            assert_eq!(
                palette_keys,
                required,
                "Theme '{name}' is missing keys: {:?}",
                required.difference(&palette_keys).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn theme_values_are_non_empty_strings() {
        let all = themes();
        for (name, palette) in &all {
            for (key, value) in palette {
                assert!(
                    !value.is_empty(),
                    "Theme '{name}' key '{key}' has empty value"
                );
            }
        }
    }

    #[test]
    fn four_themes_exist() {
        let all = themes();
        assert_eq!(all.len(), 4);
        assert!(all.contains_key("synthwave"));
        assert!(all.contains_key("nord"));
        assert!(all.contains_key("gruvbox"));
        assert!(all.contains_key("catppuccin"));
    }

    // -----------------------------------------------------------------------
    // Config defaults (ported from TestConfigDefaults in test_config.py)
    // -----------------------------------------------------------------------

    #[test]
    fn default_app_config_has_expected_values() {
        let cfg = AppConfig::default();
        assert_eq!(
            cfg.auth.browser_auth_path,
            "~/.config/ytmusic-tui/browser.json"
        );
        assert_eq!(cfg.player.volume, 80);
        assert_eq!(cfg.player.backend, "mpv");
        assert_eq!(cfg.player.audio_quality, "high");
        assert_eq!(cfg.ui.theme, "synthwave");
    }

    // Note: Rust structs are not "frozen" in the Python sense; immutability is
    // enforced at the call-site by using non-mut bindings.  The Python
    // test_auth_config_frozen / test_player_config_frozen / test_ui_config_frozen
    // tests verify the dataclass(frozen=True) attribute — there is no direct
    // Rust equivalent to test; ownership semantics serve the same purpose.

    // -----------------------------------------------------------------------
    // Config loading (ported from TestConfigLoading in test_config.py)
    // -----------------------------------------------------------------------

    /// Write a temp file with the given content and return its path.
    fn write_temp_toml(dir: &tempfile::TempDir, name: &str, content: &str) -> PathBuf {
        let path = dir.path().join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path
    }

    #[test]
    fn load_from_bundled_default() {
        // Mirrors test_load_from_bundled_default: supply custom bundled content
        // and a nonexistent user path.
        let bundled = r#"
[auth]
browser_auth_path = "/custom/browser.json"

[player]
backend = "mpv"
volume = 75

[ui]
theme = "nord"
"#;
        let tmp = tempfile::tempdir().unwrap();
        let nonexistent = tmp.path().join("nonexistent.toml");
        let cfg = load_config(Some(&nonexistent), Some(bundled)).unwrap();
        assert_eq!(cfg.auth.browser_auth_path, "/custom/browser.json");
        assert_eq!(cfg.player.volume, 75);
        assert_eq!(cfg.ui.theme, "nord");
    }

    #[test]
    fn user_overrides_bundled() {
        let bundled = r#"
[player]
volume = 80

[ui]
theme = "synthwave"
"#;
        let user_toml = r#"
[player]
volume = 50

[ui]
theme = "gruvbox"
"#;
        let tmp = tempfile::tempdir().unwrap();
        let user_path = write_temp_toml(&tmp, "user.toml", user_toml);
        let cfg = load_config(Some(&user_path), Some(bundled)).unwrap();
        assert_eq!(cfg.player.volume, 50);
        assert_eq!(cfg.ui.theme, "gruvbox");
    }

    #[test]
    fn missing_both_files_returns_defaults() {
        // Neither user path nor any file exists; bundled_str is None so the
        // embedded BUNDLED_DEFAULT_TOML is used, which matches the hard-coded
        // defaults.
        let tmp = tempfile::tempdir().unwrap();
        let nonexistent = tmp.path().join("nope.toml");
        let cfg = load_config(Some(&nonexistent), None).unwrap();
        assert_eq!(cfg.player.volume, 80);
        assert_eq!(cfg.ui.theme, "synthwave");
    }

    #[test]
    fn partial_user_config_merges_cleanly() {
        let bundled = r#"
[auth]
browser_auth_path = "~/.config/ytmusic-tui/browser.json"

[player]
volume = 80

[ui]
theme = "synthwave"
"#;
        let user_toml = r#"
[ui]
theme = "catppuccin"
"#;
        let tmp = tempfile::tempdir().unwrap();
        let user_path = write_temp_toml(&tmp, "user.toml", user_toml);
        let cfg = load_config(Some(&user_path), Some(bundled)).unwrap();
        assert_eq!(cfg.ui.theme, "catppuccin");
        // Non-overridden sections keep their bundled values.
        assert_eq!(cfg.player.volume, 80);
    }

    #[test]
    fn load_real_bundled_default_toml() {
        // Mirrors test_load_real_bundled_default: use the embedded constant
        // directly (no user override).
        let tmp = tempfile::tempdir().unwrap();
        let nonexistent = tmp.path().join("nonexistent.toml");
        let cfg = load_config(Some(&nonexistent), None).unwrap();
        assert_eq!(cfg.player.volume, 80);
        assert_eq!(cfg.ui.theme, "synthwave");
    }

    // -----------------------------------------------------------------------
    // Additional test: bundled TOML itself parses correctly
    // -----------------------------------------------------------------------

    #[test]
    fn bundled_default_toml_parses_and_matches_hardcoded_defaults() {
        // Confirm include_str! content is valid TOML and produces the expected defaults.
        let raw: RawAppConfig = toml::from_str(BUNDLED_DEFAULT_TOML).expect("must parse");
        let cfg = build_config(merge_raw(raw, RawAppConfig::default()));
        let expected = AppConfig::default();
        assert_eq!(cfg, expected);
    }

    #[test]
    fn bundled_keymap_covers_every_default_keymap_action() {
        // Every action in DEFAULT_KEYMAP must appear in the bundled keymap.toml.
        let tmp = tempfile::tempdir().unwrap();
        let nonexistent = tmp.path().join("nonexistent.toml");
        let km = load_keymap(Some(&nonexistent), None).unwrap();
        for (action, _) in DEFAULT_KEYMAP {
            assert!(
                km.contains_key(*action),
                "action '{action}' missing from loaded keymap"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Keymap loading
    // -----------------------------------------------------------------------

    #[test]
    fn default_keymap_has_expected_entries() {
        let km: HashMap<String, String> = DEFAULT_KEYMAP
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        assert_eq!(km.get("toggle_pause").map(String::as_str), Some("space"));
        assert_eq!(km.get("quit").map(String::as_str), Some("Q"));
        assert_eq!(km.get("cycle_audio_quality").map(String::as_str), Some("b"));
    }

    #[test]
    fn user_keymap_overrides_default() {
        let user_toml = r#"
[keybinds]
quit = "ctrl+q"
toggle_pause = "p"
"#;
        let tmp = tempfile::tempdir().unwrap();
        let user_path = write_temp_toml(&tmp, "keymap.toml", user_toml);
        let km = load_keymap(Some(&user_path), None).unwrap();
        assert_eq!(km.get("quit").map(String::as_str), Some("ctrl+q"));
        assert_eq!(km.get("toggle_pause").map(String::as_str), Some("p"));
        // Non-overridden action keeps its default.
        assert_eq!(km.get("next_track").map(String::as_str), Some("n"));
    }

    #[test]
    fn unknown_action_in_keymap_is_stored_not_rejected() {
        // Forward compatibility: novel action names in the user file must not
        // cause an error (Python: `base[action] = key` for any action string).
        let user_toml = r#"
[keybinds]
future_action = "ctrl+x"
"#;
        let tmp = tempfile::tempdir().unwrap();
        let user_path = write_temp_toml(&tmp, "keymap.toml", user_toml);
        let km = load_keymap(Some(&user_path), None).unwrap();
        assert_eq!(km.get("future_action").map(String::as_str), Some("ctrl+x"));
    }

    #[test]
    fn missing_user_keymap_returns_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let nonexistent = tmp.path().join("nonexistent.toml");
        let km = load_keymap(Some(&nonexistent), None).unwrap();
        assert_eq!(km.get("toggle_pause").map(String::as_str), Some("space"));
        assert_eq!(km.get("quit").map(String::as_str), Some("Q"));
    }

    // -----------------------------------------------------------------------
    // Unknown-key tolerance
    // -----------------------------------------------------------------------

    #[test]
    fn unknown_toml_keys_are_silently_ignored() {
        // A user config.toml with unknown sections and keys must load without
        // error (forward compat — deny_unknown_fields is intentionally absent).
        let bundled = r#"
[player]
volume = 80

[ui]
theme = "synthwave"

[unknown_section]
some_key = "ignored"
"#;
        let tmp = tempfile::tempdir().unwrap();
        let nonexistent = tmp.path().join("nope.toml");
        let cfg = load_config(Some(&nonexistent), Some(bundled)).unwrap();
        assert_eq!(cfg.player.volume, 80);
    }

    // -----------------------------------------------------------------------
    // Tilde expansion
    // -----------------------------------------------------------------------

    #[test]
    fn expand_tilde_resolves_home() {
        let fake_home = Some(PathBuf::from("/tmp/fake-home"));
        assert_eq!(
            expand_tilde_with(Path::new("~"), fake_home.clone()),
            PathBuf::from("/tmp/fake-home")
        );
        assert_eq!(
            expand_tilde_with(Path::new("~/.config/x"), fake_home.clone()),
            PathBuf::from("/tmp/fake-home/.config/x")
        );
        // A path that merely contains ~ mid-string is left untouched.
        assert_eq!(
            expand_tilde_with(Path::new("/etc/~weird"), fake_home.clone()),
            PathBuf::from("/etc/~weird")
        );
        // When no home is available the path is returned unchanged.
        assert_eq!(expand_tilde_with(Path::new("~"), None), PathBuf::from("~"));
    }
}
