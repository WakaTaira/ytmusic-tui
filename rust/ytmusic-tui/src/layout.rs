//! Responsive layout orientation detection.
//!
//! Determines whether the terminal is wide enough for a horizontal
//! (side-by-side) pane layout or should fall back to a vertical (stacked)
//! layout.  Uses the same heuristic as spotify_player: `columns / rows > 2.3`
//! selects horizontal.
//!
//! 1:1 port of the Python `ytmusic_tui.layout` module.

/// Layout orientation for multi-pane views.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Horizontal,
    Vertical,
}

/// spotify_player uses 2.3 as the aspect-ratio threshold.
const ASPECT_THRESHOLD: f64 = 2.3;

/// Return the recommended layout orientation for the given terminal size.
///
/// `columns` is the terminal width in characters and `rows` the height in
/// lines.  Returns [`Orientation::Horizontal`] when the terminal is wide
/// enough for side-by-side panes, [`Orientation::Vertical`] otherwise.
///
/// `rows` is clamped to a minimum of 1 to avoid a division by zero, matching
/// the Python `max(rows, 1)`.
#[must_use]
pub fn detect_orientation(columns: u16, rows: u16) -> Orientation {
    let cols = f64::from(columns);
    let rows = f64::from(rows.max(1));
    if cols / rows > ASPECT_THRESHOLD {
        Orientation::Horizontal
    } else {
        Orientation::Vertical
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_terminal_is_horizontal() {
        // 120 / 40 = 3.0 > 2.3
        assert_eq!(detect_orientation(120, 40), Orientation::Horizontal);
    }

    #[test]
    fn tall_terminal_is_vertical() {
        // 80 / 40 = 2.0 < 2.3
        assert_eq!(detect_orientation(80, 40), Orientation::Vertical);
    }

    #[test]
    fn exactly_at_threshold_is_vertical() {
        // 23 / 10 = 2.3, strictly-greater test fails -> vertical.
        assert_eq!(detect_orientation(23, 10), Orientation::Vertical);
    }

    #[test]
    fn just_above_threshold_is_horizontal() {
        // 24 / 10 = 2.4 > 2.3
        assert_eq!(detect_orientation(24, 10), Orientation::Horizontal);
    }

    #[test]
    fn zero_rows_does_not_panic_and_is_horizontal() {
        // rows clamped to 1: 80 / 1 = 80 > 2.3
        assert_eq!(detect_orientation(80, 0), Orientation::Horizontal);
    }

    #[test]
    fn zero_columns_is_vertical() {
        // 0 / 40 = 0 < 2.3
        assert_eq!(detect_orientation(0, 40), Orientation::Vertical);
    }
}
