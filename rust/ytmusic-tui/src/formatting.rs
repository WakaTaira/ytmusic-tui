//! Shared formatting utilities.
//!
//! 1:1 port of the Python `ytmusic_tui.formatting` module.

/// Format `seconds` as `M:SS` or `H:MM:SS`.
///
/// When `seconds` is zero or negative the track has no known duration (e.g.
/// YouTube Music `get_home()` omits the field), so this returns a dash instead
/// of a misleading `0:00`.
#[must_use]
pub fn format_duration(seconds: f64) -> String {
    let total = seconds as i64;
    if total <= 0 {
        return "—".to_owned();
    }
    let hours = total / 3600;
    let remainder = total % 3600;
    let minutes = remainder / 60;
    let secs = remainder % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{secs:02}")
    } else {
        format!("{minutes}:{secs:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_seconds_is_dash() {
        assert_eq!(format_duration(0.0), "—");
    }

    #[test]
    fn negative_seconds_is_dash() {
        assert_eq!(format_duration(-5.0), "—");
    }

    #[test]
    fn sub_minute_formats_as_m_ss() {
        assert_eq!(format_duration(5.0), "0:05");
    }

    #[test]
    fn minutes_and_seconds_zero_pad_seconds() {
        assert_eq!(format_duration(83.0), "1:23");
    }

    #[test]
    fn exact_minute_boundary() {
        assert_eq!(format_duration(60.0), "1:00");
    }

    #[test]
    fn hours_format_as_h_mm_ss() {
        // 1 hour, 1 minute, 5 seconds
        assert_eq!(format_duration(3665.0), "1:01:05");
    }

    #[test]
    fn exact_hour_boundary_zero_pads_minutes() {
        assert_eq!(format_duration(3600.0), "1:00:00");
    }

    #[test]
    fn fractional_seconds_truncate_toward_zero() {
        // 83.9 -> 83 -> 1:23 (int() truncation, matching Python int(seconds)).
        assert_eq!(format_duration(83.9), "1:23");
    }
}
