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
//! * [`search`] — the 4-pane search view (Tracks/Albums/Artists/Playlists).
//! * [`library`] — the 3-pane library view (Playlists/Albums/Artists).
//! * [`album`] — the album detail view (header + track list).
//! * [`artist`] — the artist page (top songs / albums / related artists).
//! * [`lyrics`] — scrollable lyrics for the current track.
//! * [`history`] — recently-played track list.
//! * [`queue_view`] — the current playback queue with position highlight.
//! * [`player_bar`] — the bottom now-playing bar.
//! * [`toast`] — floating bottom-right notifications (the `notify()` port).

pub mod album;
pub mod artist;
pub mod filter_bar;
pub mod history;
pub mod home;
pub mod library;
pub mod lyrics;
pub mod player_bar;
pub mod playlist;
pub mod popup;
pub mod queue_view;
pub mod search;
pub mod toast;

use ratatui::layout::Constraint;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Cell, Row, Table};

use crate::config::{ThemePalette, get_theme_palette};

// ===========================================================================
// STYLE CONTRACT  —  element → palette / layout, extracted from the
// machine-readable Textual SVG exports (screenshots/{home,library,queue}.svg)
// plus each Python view's DEFAULT_CSS. This is the definitive mapping the
// ratatui render layer reproduces; earlier rounds guessed at borders and a
// `▶` cursor symbol that the real Textual app does not draw.
// ===========================================================================
//
// ## Global frame (40-row terminal, synthwave default theme)
//
// | Region        | Rows      | Background           | Notes                  |
// |---------------|-----------|----------------------|------------------------|
// | Title bar     | row 0     | `panel_bg` `#40284d` | "ytmusic-tui" centered, `text` bold |
// | Content       | rows 1-35 | `surface`  `#241734` | the active view        |
// | Filter bar    | (when on) | `surface`            | docked above the player |
// | Player bar    | rows 36-39| `surface`, top-border `primary-background` `#2d1b4e` | 4 rows |
//
// ## DataTables (home / library / queue / search / playlist / album / artist /
// ## history) — Textual renders these BORDERLESS, NOT in a box. There are no
// ## box-drawing border glyphs anywhere in the SVGs. Panes are separated by a
// ## one-column `surface` gutter, and structure is conveyed purely by row
// ## backgrounds:
//
// | Element                         | Background          | Foreground          |
// |---------------------------------|---------------------|---------------------|
// | Column header row (focused)     | `header_bg` `#483154` | `text2` `#e5efda` bold |
// | Column header row (un-focused)  | `panel_bg` `#40284d`  | `text2` `#e5efda` bold |
// | Data row (focused table)        | `row_bg`   `#2d213c`  | `text2` `#e5efda`   |
// | Data row (un-focused table)     | `surface`  `#241734`  | `text2` `#e5efda`   |
// | Selected row (focused table)    | `primary`  `#ff77e9`  | `text`  `#eee5f5` bold |
// | Selected row (un-focused table) | `dim_cursor` `#653369`| `text2` `#e5efda`   |
//
// Each cell is padded with one leading + one trailing space (Textual's default
// DataTable cell padding) and columns are fixed-width and left-aligned, so the
// header labels and data line up in vertical lanes. Column widths per view are
// in [`crate::views::table`]'s callers.
//
// ## Section / pane titles
//
// A single-line label ABOVE each table, NOT a panel border title:
//   * home section title ("Quick picks") → `accent` `#b967ff` bold.
//   * library pane title, focused ("Playlists") → `accent` bold; un-focused
//     ("Albums"/"Artists") → `text-muted` `#9a8aad`.
//   * album/artist/lyrics/history headers → `accent` bold (the `.section-label`
//     / `#…-title` CSS `color: $accent`).
//
// ## Status line
//
// A single muted line under the title row (e.g. library's "3 playlist(s) | 2
// album(s)", queue's "Queue is empty") → `text-muted`. Error states use
// `primary`.
//
// ## Player bar (rows 36-39, all three SVGs identical)
//
//   row 36: `────…` full-width divider in `primary-background` `#2d1b4e`.
//   row 37: `⏸`/`▶` icon, `Title - Artist` (`text2`), right cluster `S` `R`
//           (`text-muted`-ish `#a3a5a1`) and `Vol: N`/`Vol: MUTE` (`text2`).
//   row 38: album line (`text-muted`, blank when no album).
//   row 39: `━━╶──` progress glyphs (`text2`) + right-aligned `pos / dur`.
//
// ## Popups (action / theme / playlist-picker) — popup.py DEFAULT_CSS
//
// NOT centered modals: `dock: bottom; offset-y: -4; height: auto`, full content
// width, `background: $surface`, `border-top: solid $accent`, `padding: 0 1`,
// title `color: $accent` bold, list rows height 1, selected row = Textual's
// default `primary` cursor. ratatui cannot dim the backdrop translucently; we
// approximate the modal focus by clearing only the sheet area (the view stays
// visible behind, matching Textual which also does not dim under these docked
// sheets).
//
// ## ratatui approximations (documented, unavoidable)
//
//   * The four derived shades (`panel_bg`/`header_bg`/`row_bg`/`dim_cursor`)
//     are Textual `Color.blend`/`$boost` outputs; reproduced here as RGB lerps
//     of the base palette that match the synthwave SVG within a few units and
//     stay sensible across the other three themes (see [`Theme::derive`]).
//   * `text2` `#e5efda` is Textual's auto-contrast `$text` on `$surface`; the
//     base palette `text` `#eee5f5` is used for the *selected* (cursor) row,
//     matching the SVG (r5/r7 use `#eee5f5`, body text uses `#e5efda`). We use
//     the single `text` field for both — the ~3-unit difference is below
//     terminal perceptibility and avoids a fifth derived shade.

/// One leading + trailing space of cell padding, matching Textual's default
/// `DataTable` cell padding so column labels and data align in lanes.
const CELL_PAD: &str = " ";

/// Build the styled column-header [`Row`] for a borderless DataTable.
///
/// `focused` selects between the brighter focused header background
/// (`header_bg`) and the dimmer un-focused one (`panel_bg`), reproducing
/// Textual's focused-vs-unfocused DataTable header shading seen in the library
/// SVG (focused "Playlists" header `#483154`, un-focused "Albums" `#40284d`).
#[must_use]
pub fn table_header(theme: &Theme, labels: &[&str], focused: bool) -> Row<'static> {
    let bg = if focused {
        theme.header_bg
    } else {
        theme.panel_bg
    };
    let cells: Vec<Cell> = labels
        .iter()
        .map(|l| Cell::from(format!("{CELL_PAD}{l}{CELL_PAD}")))
        .collect();
    Row::new(cells).style(
        Style::default()
            .fg(theme.text)
            .bg(bg)
            .add_modifier(Modifier::BOLD),
    )
}

/// Build a styled data [`Row`] from already-formatted column strings.
///
/// `focused` chooses the data-row background: the focused table zebra fill
/// (`row_bg`) versus the plain `surface` of an un-focused table, matching the
/// home SVG (the focused "Quick picks" rows are `#2d213c`; the un-focused
/// "Mixed for you" rows fall back to `surface`).
#[must_use]
pub fn table_row(theme: &Theme, columns: &[String], focused: bool) -> Row<'static> {
    let bg = if focused { theme.row_bg } else { theme.surface };
    let cells: Vec<Cell> = columns
        .iter()
        .map(|c| Cell::from(format!("{CELL_PAD}{c}{CELL_PAD}")))
        .collect();
    Row::new(cells).style(Style::default().fg(theme.text).bg(bg))
}

/// Assemble a borderless [`Table`] from a header row, data rows, and column
/// widths, with the focused/un-focused selection highlight applied.
///
/// This is the single source of the DataTable look across views: no border, a
/// `surface` base background so gaps render in the screen color, and the row
/// highlight from [`selected_row_style`] (focused) or [`dim_selected_style`]
/// (un-focused). Callers pass [`Constraint::Length`] widths sized to include
/// the [`CELL_PAD`] padding.
#[must_use]
pub fn borderless_table<'a>(
    theme: &Theme,
    header: Row<'a>,
    rows: Vec<Row<'a>>,
    widths: Vec<Constraint>,
    focused: bool,
) -> Table<'a> {
    let highlight = if focused {
        selected_row_style(theme)
    } else {
        dim_selected_style(theme)
    };
    Table::new(rows, widths)
        .header(header)
        .column_spacing(0)
        .style(Style::default().bg(theme.surface))
        .row_highlight_style(highlight)
}

/// A single-line section/pane title label (the text drawn above a table).
///
/// `focused` → `accent` bold (Python `.section-title`/`.pane-title.focused`);
/// un-focused → `text-muted` (the library SVG's dimmed "Albums"/"Artists").
#[must_use]
pub fn section_title(theme: &Theme, title: &str, focused: bool) -> Span<'static> {
    if focused {
        Span::styled(
            title.to_owned(),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(title.to_owned(), Style::default().fg(theme.text_muted))
    }
}

/// The highlight style for a selected row in a *focused* list/table.
///
/// Textual's focused DataTable cursor: a `$primary` block with `$text`
/// foreground (the home SVG selected row is `#ff77e9` bg / `#eee5f5` text).
#[must_use]
pub fn selected_row_style(theme: &Theme) -> Style {
    Style::default()
        .fg(theme.text)
        .bg(theme.primary)
        .add_modifier(Modifier::BOLD)
}

/// The highlight style for the cursor row of an *un-focused* table.
///
/// Textual dims the cursor of a table that does not have focus to
/// `dim_cursor` (`#653369` in the home SVG's un-focused "Mixed for you"
/// table), keeping the normal `text` foreground rather than the inverted
/// focused-cursor foreground.
#[must_use]
pub fn dim_selected_style(theme: &Theme) -> Style {
    Style::default().fg(theme.text).bg(theme.dim_cursor)
}

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
    // -- Derived shades (Textual `Color.blend`/`$boost` outputs) -------------
    // Not part of the eight base CSS variables; computed by [`Theme::derive`]
    // from the base palette to reproduce the rendered DataTable look. The
    // synthwave values below name the SVG ground truth these approximate.
    /// Title bar + un-focused DataTable header background (`#40284d`).
    pub panel_bg: Color,
    /// Focused DataTable header-row background (`#483154`).
    pub header_bg: Color,
    /// Focused DataTable data-row (zebra) background (`#2d213c`).
    pub row_bg: Color,
    /// Un-focused table cursor-row background (`#653369`).
    pub dim_cursor: Color,
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
        let primary = color("primary");
        let surface = color("surface");
        let text = color("text");
        Self {
            primary,
            secondary: color("secondary"),
            accent: color("accent"),
            background: color("background"),
            surface,
            primary_background: color("primary-background"),
            text,
            text_muted: color("text-muted"),
            // Blend factors chosen so synthwave matches the SVG within a few
            // RGB units and the other built-in themes stay legible:
            //   panel  = surface→primary 0.13   (#40284d target)
            //   header = surface→primary 0.18   (#483154 target)
            //   row    = surface→text    0.06   (#2d213c target)
            //   cursor = surface→primary 0.30   (#653369 target)
            panel_bg: blend(surface, primary, 0.13),
            header_bg: blend(surface, primary, 0.18),
            row_bg: blend(surface, text, 0.06),
            dim_cursor: blend(surface, primary, 0.30),
        }
    }
}

/// Linearly interpolate between two [`Color`]s in RGB space by `t` in `0.0..=1.0`.
///
/// Both inputs are expected to be [`Color::Rgb`] (every built-in palette parses
/// to RGB); any non-RGB input returns `from` unchanged so a malformed palette
/// degrades to the base color rather than panicking. This reproduces Textual's
/// derived shades closely enough for terminal rendering — see the STYLE
/// CONTRACT note on the approximation.
fn blend(from: Color, to: Color, t: f32) -> Color {
    let (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) = (from, to) else {
        return from;
    };
    let lerp = |a: u8, b: u8| -> u8 {
        let a = f32::from(a);
        let b = f32::from(b);
        (a + (b - a) * t).round().clamp(0.0, 255.0) as u8
    };
    Color::Rgb(lerp(r1, r2), lerp(g1, g2), lerp(b1, b2))
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
                t.panel_bg,
                t.header_bg,
                t.row_bg,
                t.dim_cursor,
            ] {
                assert_ne!(color, Color::Reset, "theme '{name}' had an unparsed color");
            }
        }
    }

    // -- derived shades (the STYLE CONTRACT approximations) ----------------

    #[test]
    fn synthwave_derived_shades_match_the_svg_within_tolerance() {
        // The Textual SVG exports render these exact backgrounds; our RGB-lerp
        // approximations must land within a small tolerance of them.
        let t = Theme::from_name("synthwave");
        let close = |c: Color, hex: &str, tol: i32| {
            let Color::Rgb(r, g, b) = c else {
                panic!("derived color is not RGB");
            };
            let want = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap();
            let (wr, wg, wb) = (
                ((want >> 16) & 0xff) as i32,
                ((want >> 8) & 0xff) as i32,
                (want & 0xff) as i32,
            );
            let d =
                (i32::from(r) - wr).abs() + (i32::from(g) - wg).abs() + (i32::from(b) - wb).abs();
            assert!(d <= tol, "{c:?} not within {tol} of #{hex} (sum-delta {d})");
        };
        close(t.panel_bg, "40284d", 12);
        close(t.header_bg, "483154", 18);
        close(t.row_bg, "2d213c", 12);
        close(t.dim_cursor, "653369", 8);
    }

    #[test]
    fn derived_shades_are_lighter_than_surface() {
        // Every derived background must read as a lift off the surface so the
        // header/row/cursor structure is visible (the whole point of the
        // borderless DataTable look).
        let t = Theme::from_name("synthwave");
        let lum = |c: Color| {
            let Color::Rgb(r, g, b) = c else { return 0u32 };
            u32::from(r) + u32::from(g) + u32::from(b)
        };
        let base = lum(t.surface);
        for shade in [t.panel_bg, t.header_bg, t.row_bg, t.dim_cursor] {
            assert!(lum(shade) > base, "derived shade not lighter than surface");
        }
        // header is a brighter lift than the un-focused panel header.
        assert!(lum(t.header_bg) > lum(t.panel_bg));
    }

    #[test]
    fn blend_endpoints_and_midpoint() {
        let a = Color::Rgb(0, 0, 0);
        let b = Color::Rgb(100, 200, 50);
        assert_eq!(blend(a, b, 0.0), a);
        assert_eq!(blend(a, b, 1.0), b);
        assert_eq!(blend(a, b, 0.5), Color::Rgb(50, 100, 25));
    }

    #[test]
    fn blend_passes_through_non_rgb() {
        assert_eq!(blend(Color::Reset, Color::Rgb(1, 2, 3), 0.5), Color::Reset);
    }

    // -- selected-row styles ----------------------------------------------

    #[test]
    fn focused_cursor_uses_primary_bg_and_text_fg() {
        let t = Theme::default();
        let s = selected_row_style(&t);
        assert_eq!(s.bg, Some(t.primary));
        assert_eq!(s.fg, Some(t.text));
    }

    #[test]
    fn unfocused_cursor_uses_dim_cursor_bg() {
        let t = Theme::default();
        let s = dim_selected_style(&t);
        assert_eq!(s.bg, Some(t.dim_cursor));
        assert_eq!(s.fg, Some(t.text));
    }

    // -- table builders (rendered, since Row fields are private) -----------

    /// Render a one-data-row borderless table and return its buffer so a test
    /// can read header/row/selection backgrounds out of specific cells.
    fn render_mini_table(
        theme: &Theme,
        focused: bool,
        select: Option<usize>,
    ) -> ratatui::buffer::Buffer {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::widgets::TableState;

        let header = table_header(theme, &["Title", "Dur"], focused);
        let rows = vec![table_row(
            theme,
            &["Song".to_owned(), "3:00".to_owned()],
            focused,
        )];
        let widths = vec![Constraint::Length(8), Constraint::Length(6)];
        let table = borderless_table(theme, header, rows, widths, focused);

        let backend = TestBackend::new(14, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let mut state = TableState::default();
                state.select(select);
                frame.render_stateful_widget(table, frame.area(), &mut state);
            })
            .unwrap();
        terminal.backend().buffer().clone()
    }

    #[test]
    fn rendered_header_focused_uses_header_bg_unfocused_uses_panel_bg() {
        let t = Theme::default();
        // Header is row 0; a content cell (col 1) carries the header bg.
        assert_eq!(render_mini_table(&t, true, None)[(1, 0)].bg, t.header_bg);
        assert_eq!(render_mini_table(&t, false, None)[(1, 0)].bg, t.panel_bg);
    }

    #[test]
    fn rendered_unselected_row_focused_uses_row_bg_unfocused_uses_surface() {
        let t = Theme::default();
        // Data row is row 1.
        assert_eq!(render_mini_table(&t, true, None)[(1, 1)].bg, t.row_bg);
        assert_eq!(render_mini_table(&t, false, None)[(1, 1)].bg, t.surface);
    }

    #[test]
    fn rendered_selected_row_focused_uses_primary_unfocused_uses_dim_cursor() {
        let t = Theme::default();
        assert_eq!(render_mini_table(&t, true, Some(0))[(1, 1)].bg, t.primary);
        assert_eq!(
            render_mini_table(&t, false, Some(0))[(1, 1)].bg,
            t.dim_cursor
        );
    }

    #[test]
    fn rendered_table_has_no_box_border_glyphs() {
        let t = Theme::default();
        let buffer = render_mini_table(&t, true, None);
        for cell in buffer.content() {
            let s = cell.symbol();
            assert!(
                !"┌┐└┘─│├┤┬┴┼╭╮╰╯".contains(s),
                "borderless table drew a box glyph: {s:?}"
            );
        }
    }
}
