//! The InnerTube request context.
//!
//! Every `youtubei/v1` call carries a `context.client` block identifying the
//! caller as the YouTube Music web client (`WEB_REMIX`). Mirrors
//! `ytmusicapi.helpers.initialize_context`.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

/// The InnerTube client name ytmusicapi reports. Constant across requests.
pub const CLIENT_NAME: &str = "WEB_REMIX";

/// Build the `context` object merged into every request body.
///
/// ytmusicapi sets `clientVersion` to `"1.{YYYYMMDD}.01.00"` using the current
/// UTC date, and `hl` (interface language) to `"en"`. The `user` object is sent
/// empty (no brand-account impersonation).
pub fn build_context() -> Value {
    json!({
        "context": {
            "client": {
                "clientName": CLIENT_NAME,
                "clientVersion": client_version(),
                "hl": "en",
            },
            "user": {},
        }
    })
}

/// Compute the `"1.{YYYYMMDD}.01.00"` client version for today (UTC).
///
/// ytmusicapi uses `time.strftime("%Y%m%d", time.gmtime())`; this derives the
/// same UTC calendar date from the Unix epoch without pulling in a date crate.
fn client_version() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (year, month, day) = ymd_from_unix(secs);
    format!("1.{year:04}{month:02}{day:02}.01.00")
}

/// Convert a Unix timestamp (UTC) to a `(year, month, day)` calendar date.
///
/// Days-since-epoch are converted with a civil-calendar algorithm
/// (Howard Hinnant's `civil_from_days`), valid for all dates in range.
fn ymd_from_unix(secs: u64) -> (i64, u32, u32) {
    let days = (secs / 86_400) as i64;
    // Shift epoch to 0000-03-01 so leap days fall at the end of the cycle.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11], March-based
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_known_unix_dates() {
        // 2021-01-01 00:00:00 UTC
        assert_eq!(ymd_from_unix(1_609_459_200), (2021, 1, 1));
        // 2023-11-14 22:13:20 UTC (the vector-1 timestamp)
        assert_eq!(ymd_from_unix(1_700_000_000), (2023, 11, 14));
        // Unix epoch
        assert_eq!(ymd_from_unix(0), (1970, 1, 1));
        // A leap day: 2024-02-29 12:00:00 UTC
        assert_eq!(ymd_from_unix(1_709_208_000), (2024, 2, 29));
    }

    #[test]
    fn client_version_has_expected_shape() {
        let v = client_version();
        // "1." + 8 digits + ".01.00"
        assert!(v.starts_with("1."), "version: {v}");
        assert!(v.ends_with(".01.00"), "version: {v}");
        let date_part = v
            .strip_prefix("1.")
            .and_then(|s| s.strip_suffix(".01.00"))
            .unwrap();
        assert_eq!(date_part.len(), 8, "date part: {date_part}");
        assert!(date_part.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn context_carries_web_remix_client() {
        let ctx = build_context();
        let client = &ctx["context"]["client"];
        assert_eq!(client["clientName"], "WEB_REMIX");
        assert_eq!(client["hl"], "en");
        assert!(ctx["context"]["user"].is_object());
    }
}
