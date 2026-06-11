//! Terminal-UI views and the rendering theme.
//!
//! This module hosts the ratatui render layer ported from
//! `src/ytmusic_tui/views/`. Each view is a plain value that renders itself
//! into a [`ratatui::Frame`] — there is no widget-tree retention as in Textual;
//! the synchronous main loop redraws every tick (see [`crate::app`] for the
//! threading model that feeds these views).
//!
//! # Contents
//!
//! * [`PageState`] — the fetch-state enum every API-backed view holds, the
//!   ratatui equivalent of Python's `FetchView` (loading → loaded → error).
//! * [`Theme`] — the palette parsed once at startup from
//!   [`crate::config::get_theme_palette`] hex strings into [`ratatui::Color`]s.
//! * [`home`] — the home recommendations view.
//! * [`playlist`] — the two-level playlist browser.
//! * [`player_bar`] — the bottom now-playing bar.

pub mod home;
pub mod player_bar;
pub mod playlist;

use ratatui::style::Color;

use crate::config::{ThemePalette, get_theme_palette};

// ---------------------------------------------------------------------------
// PageState — the FetchView equivalent
// ---------------------------------------------------------------------------

/// The fetch state of an API-backed view.
///
/// This is the Rust port of Python's `FetchView` lifecycle
/// (`src/ytmusic_tui/views/base.py`): a worker fetches in the background, and
/// until it returns the view shows a "Loading…" status; on success the data
/// renders; on failure a classified error message is shown (the string is what
/// `classify_api_error` produced). Modeling the three states as one enum makes
/// the illegal "loaded but also errored" combination unrepresentable.
///
/// `T` is the loaded payload (e.g. `Vec<HomeSection>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageState<T> {
    /// The fetch is in flight; render the loading label.
    Loading,
    /// The fetch succeeded; render `T`.
    Loaded(T),
    /// The fetch failed; the string is the user-facing, classified message.
    Error(String),
}

impl<T> Default for PageState<T> {
    /// A fresh view starts in [`PageState::Loading`] (its worker has been
    /// dispatched but has not replied yet), mirroring the Python view that
    /// renders "Loading…" in `compose` before the worker delivers.
    fn default() -> Self {
        PageState::Loading
    }
}

/// The label text shown while a fetch is in flight.
///
/// Matches the Python "Loading..." status label (an ASCII triple-dot, not the
/// `…` ellipsis, so it is byte-identical to the Python string the tests assert).
pub const LOADING_LABEL: &str = "Loading...";

impl<T> PageState<T> {
    /// The single-line status text for the non-data states.
    ///
    /// * [`PageState::Loading`] → the [`LOADING_LABEL`].
    /// * [`PageState::Error`] → the classified message.
    /// * [`PageState::Loaded`] → `None` (the data renders instead of a status
    ///   line), matching Python clearing the status label on success.
    #[must_use]
    pub fn status_line(&self) -> Option<&str> {
        match self {
            PageState::Loading => Some(LOADING_LABEL),
            PageState::Error(msg) => Some(msg),
            PageState::Loaded(_) => None,
        }
    }

    /// The loaded payload, or `None` while loading or on error.
    #[must_use]
    pub fn loaded(&self) -> Option<&T> {
        match self {
            PageState::Loaded(data) => Some(data),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Theme — palette hex → ratatui Color, parsed once at startup
// ---------------------------------------------------------------------------

/// The active color theme, resolved from a named palette into ratatui colors.
///
/// Built once with [`Theme::from_name`] at startup. The config layer stores
/// palettes as `"#rrggbb"` hex strings (so it carries no ratatui dependency —
/// see `config.rs`); this struct does the one-time parse into [`Color::Rgb`].
///
/// Fields map 1-to-1 to the eight CSS custom properties every theme defines
/// (`config::THEME_REQUIRED_KEYS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub primary: Color,
    pub secondary: Color,
    pub accent: Color,
    pub background: Color,
    pub surface: Color,
    pub primary_background: Color,
    pub text: Color,
    pub text_muted: Color,
}

impl Theme {
    /// Resolve a named theme into ratatui colors.
    ///
    /// Unknown names fall back to `synthwave` via
    /// [`crate::config::get_theme_palette`], so this never fails. A malformed
    /// hex value in a palette falls back to [`Color::Reset`] for that single
    /// entry rather than panicking (the built-in palettes are all well-formed;
    /// this only guards a future hand-edited palette).
    #[must_use]
    pub fn from_name(name: &str) -> Self {
        Self::from_palette(&get_theme_palette(name))
    }

    /// Build a [`Theme`] from an already-resolved palette map.
    #[must_use]
    pub fn from_palette(palette: &ThemePalette) -> Self {
        let color = |key: &str| palette.get(key).map_or(Color::Reset, |hex| parse_hex(hex));
        Self {
            primary: color("primary"),
            secondary: color("secondary"),
            accent: color("accent"),
            background: color("background"),
            surface: color("surface"),
            primary_background: color("primary-background"),
            text: color("text"),
            text_muted: color("text-muted"),
        }
    }
}

impl Default for Theme {
    /// The default theme is `synthwave`, matching `config::UiConfig::default`.
    fn default() -> Self {
        Self::from_name("synthwave")
    }
}

/// Parse a `#rrggbb` hex string into [`Color::Rgb`].
///
/// Returns [`Color::Reset`] for any malformed input (wrong length, non-hex
/// digits, missing `#`). The six-digit form is the only one the bundled
/// palettes use; three-digit shorthand is intentionally not accepted because
/// none of the themes use it.
fn parse_hex(hex: &str) -> Color {
    let digits = hex.strip_prefix('#').unwrap_or(hex);
    if digits.len() != 6 {
        return Color::Reset;
    }
    match u32::from_str_radix(digits, 16) {
        Ok(rgb) => Color::from_u32(rgb),
        Err(_) => Color::Reset,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- PageState ---------------------------------------------------------

    #[test]
    fn default_page_state_is_loading() {
        let state: PageState<Vec<u8>> = PageState::default();
        assert_eq!(state, PageState::Loading);
    }

    #[test]
    fn loading_status_line_is_loading_label() {
        let state: PageState<u8> = PageState::Loading;
        assert_eq!(state.status_line(), Some("Loading..."));
    }

    #[test]
    fn error_status_line_is_the_message() {
        let state: PageState<u8> = PageState::Error("network down".to_owned());
        assert_eq!(state.status_line(), Some("network down"));
    }

    #[test]
    fn loaded_has_no_status_line_and_exposes_payload() {
        let state = PageState::Loaded(42_u8);
        assert_eq!(state.status_line(), None);
        assert_eq!(state.loaded(), Some(&42));
    }

    #[test]
    fn loading_and_error_have_no_payload() {
        let loading: PageState<u8> = PageState::Loading;
        let errored: PageState<u8> = PageState::Error("x".to_owned());
        assert_eq!(loading.loaded(), None);
        assert_eq!(errored.loaded(), None);
    }

    // -- Theme / hex parsing -----------------------------------------------

    #[test]
    fn parse_hex_with_hash_yields_rgb() {
        // synthwave primary "#ff77e9".
        assert_eq!(parse_hex("#ff77e9"), Color::Rgb(0xff, 0x77, 0xe9));
    }

    #[test]
    fn parse_hex_without_hash_yields_rgb() {
        assert_eq!(parse_hex("00e5ff"), Color::Rgb(0x00, 0xe5, 0xff));
    }

    #[test]
    fn parse_hex_rejects_bad_length() {
        assert_eq!(parse_hex("#fff"), Color::Reset);
        assert_eq!(parse_hex("#ff77e9aa"), Color::Reset);
    }

    #[test]
    fn parse_hex_rejects_non_hex_digits() {
        assert_eq!(parse_hex("#gggggg"), Color::Reset);
    }

    #[test]
    fn synthwave_theme_maps_known_colors() {
        let theme = Theme::from_name("synthwave");
        assert_eq!(theme.primary, Color::Rgb(0xff, 0x77, 0xe9));
        assert_eq!(theme.secondary, Color::Rgb(0x00, 0xe5, 0xff));
        assert_eq!(theme.accent, Color::Rgb(0xb9, 0x67, 0xff));
        assert_eq!(theme.background, Color::Rgb(0x1a, 0x10, 0x25));
        assert_eq!(theme.text, Color::Rgb(0xee, 0xe5, 0xf5));
    }

    #[test]
    fn unknown_theme_falls_back_to_synthwave() {
        let unknown = Theme::from_name("does-not-exist");
        let synthwave = Theme::from_name("synthwave");
        assert_eq!(unknown, synthwave);
    }

    #[test]
    fn default_theme_is_synthwave() {
        assert_eq!(Theme::default(), Theme::from_name("synthwave"));
    }

    #[test]
    fn all_builtin_themes_parse_without_reset() {
        // Every built-in palette is well-formed six-digit hex, so no field
        // should fall back to Color::Reset.
        for name in ["synthwave", "nord", "gruvbox", "catppuccin"] {
            let t = Theme::from_name(name);
            for color in [
                t.primary,
                t.secondary,
                t.accent,
                t.background,
                t.surface,
                t.primary_background,
                t.text,
                t.text_muted,
            ] {
                assert_ne!(color, Color::Reset, "theme '{name}' had an unparsed color");
            }
        }
    }
}
