//! Faithful port of ytmusicapi's run classification (`parsers/songs.py`).
//!
//! The InnerTube "runs" arrays interleave artist/album/duration/view/year
//! fragments separated by `" • "` dots. ytmusicapi's `parse_song_run` /
//! `parse_song_runs` classify each even-indexed run and accumulate the typed
//! pieces. The stage-1 parsers in this module's siblings rebuild the
//! ytmusicapi-shaped `{artists, album, duration, duration_seconds, ...}` dict
//! from these runs, so the stage-2 `parse::dict_to_*` helpers can consume them
//! exactly as they consume real ytmusicapi output.

use serde_json::{Map, Value, json};

use crate::nav::{NAVIGATION_BROWSE_ID, nav_str};

/// The dot separator run ytmusicapi compares against: `{"text": " • "}`.
/// (`ytmusicapi.parsers.constants.DOT_SEPARATOR_RUN`.)
const DOT_SEPARATOR: &str = " \u{2022} ";

/// One classified run, mirroring the `{"type": ..., "data": ...}` dicts
/// returned by `parse_song_run`.
enum SongRun {
    /// An album reference (`navigationEndpoint` browseId starts with `MPRE`
    /// or contains `release_detail`). Carries the album name.
    Album { name: String },
    /// An artist reference (a run with a non-album `navigationEndpoint`, or any
    /// text run that is not a views/duration/year token). Carries the name.
    Artist { name: String },
    /// A views token (e.g. `"576K plays"` / `"1.4M"`); the data is the leading
    /// magnitude word. Unused by stage-2 but classified for parity.
    Views,
    /// A duration token matching `^(\d+:)*\d+:\d+$`.
    Duration { text: String },
    /// A four-digit year token.
    Year { text: String },
}

/// Classify a single run, mirroring `parse_song_run`.
fn parse_song_run(run: &Value) -> SongRun {
    let text = run.get("text").and_then(Value::as_str).unwrap_or("");

    if run.get("navigationEndpoint").is_some() {
        // Artist or album: distinguished by the browseId prefix.
        let id = nav_str(run, NAVIGATION_BROWSE_ID).unwrap_or("");
        if id.starts_with("MPRE") || id.contains("release_detail") {
            return SongRun::Album {
                name: text.to_owned(),
            };
        }
        return SongRun::Artist {
            name: text.to_owned(),
        };
    }

    // No navigationEndpoint: classify by text pattern.
    if is_views(text) {
        SongRun::Views
    } else if is_duration(text) {
        SongRun::Duration {
            text: text.to_owned(),
        }
    } else if is_year(text) {
        SongRun::Year {
            text: text.to_owned(),
        }
    } else {
        // Artist without an id.
        SongRun::Artist {
            name: text.to_owned(),
        }
    }
}

/// Build the ytmusicapi-shaped song dict from a runs array, mirroring
/// `parse_song_runs`.
///
/// The returned object carries any of `artists` (array of `{name}`), `album`
/// (`{name}`), `duration` (string) + `duration_seconds` (int), and `year`
/// (string) that were present — exactly the keys stage-2 reads. `views` is
/// classified but not emitted (stage-2 ignores it).
///
/// `skip_type_spec` mirrors the ytmusicapi flag: when the first run is an
/// artist-typed type specifier (e.g. "Song") followed by `" • "` and another
/// artist run, drop the leading two runs so the specifier is not mistaken for
/// an artist.
pub(crate) fn parse_song_runs(runs: &[Value], skip_type_spec: bool) -> Value {
    let runs = maybe_skip_type_spec(runs, skip_type_spec);

    let mut artists: Vec<Value> = Vec::new();
    let mut out = Map::new();

    for (i, run) in runs.iter().enumerate() {
        // Odd-indexed runs are always separators.
        if i % 2 == 1 {
            continue;
        }
        match parse_song_run(run) {
            SongRun::Album { name } => {
                out.insert("album".to_owned(), json!({ "name": name }));
            }
            SongRun::Artist { name } => artists.push(json!({ "name": name })),
            SongRun::Views => {}
            SongRun::Duration { text } => {
                let seconds = parse_duration_seconds(&text);
                out.insert("duration".to_owned(), Value::String(text));
                out.insert(
                    "duration_seconds".to_owned(),
                    seconds.map(Value::from).unwrap_or(Value::Null),
                );
            }
            SongRun::Year { text } => {
                out.insert("year".to_owned(), Value::String(text));
            }
        }
    }

    if !artists.is_empty() {
        out.insert("artists".to_owned(), Value::Array(artists));
    }

    Value::Object(out)
}

/// Implement the `skip_type_spec` head-trim from `parse_song_runs`.
fn maybe_skip_type_spec(runs: &[Value], skip_type_spec: bool) -> &[Value] {
    if skip_type_spec
        && runs.len() > 2
        && matches!(parse_song_run(&runs[0]), SongRun::Artist { .. })
        && runs[1].get("text").and_then(Value::as_str) == Some(DOT_SEPARATOR)
        && matches!(parse_song_run(&runs[2]), SongRun::Artist { .. })
    {
        &runs[2..]
    } else {
        runs
    }
}

/// `^\d([^ ])* [^ ]*$` — a number-magnitude views token (e.g. "576K plays").
///
/// ytmusicapi uses `re.match`, which is anchored at the start only; the trailing
/// `$` in the pattern anchors the end. Equivalent here: first char is a digit,
/// exactly one space, and neither the pre-space nor post-space segment contains
/// a further space.
fn is_views(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.first().is_none_or(|c| !c.is_ascii_digit()) {
        return false;
    }
    let mut parts = text.splitn(2, ' ');
    let first = parts.next().unwrap_or("");
    let Some(rest) = parts.next() else {
        return false; // no space at all
    };
    // The first segment must have no interior space (guaranteed by splitn on the
    // first space) and the remainder must contain no space.
    !first.contains(' ') && !rest.contains(' ')
}

/// `^(\d+:)*\d+:\d+$` — a clock duration like "2:04" or "1:02:30".
///
/// `pub(super)` so the shared stage-1 helpers (`stage1`) reuse this single
/// definition instead of re-porting the regex (de-dups the former
/// `playlist::is_duration_text`).
pub(super) fn is_duration(text: &str) -> bool {
    let segments: Vec<&str> = text.split(':').collect();
    // Need at least two colon-separated groups (so "45" alone is NOT a duration,
    // matching the regex which requires at least one ':').
    if segments.len() < 2 {
        return false;
    }
    segments
        .iter()
        .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
}

/// `^\d{4}$` — a four-digit year.
fn is_year(text: &str) -> bool {
    text.len() == 4 && text.bytes().all(|b| b.is_ascii_digit())
}

/// Parse a clock duration into seconds, mirroring `_utils.parse_duration`.
///
/// Returns `None` for falsy / non-digit input (ytmusicapi returns `None`); the
/// stage-2 layer treats a `duration_seconds` of `null` by falling back to the
/// `duration` string, so the contract is preserved either way.
///
/// `pub(super)` so the shared stage-1 helpers reuse this single definition
/// (the playlist parser previously carried a byte-identical copy).
pub(super) fn parse_duration_seconds(text: &str) -> Option<i64> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut seconds: i64 = 0;
    // ytmusicapi zips multipliers [1, 60, 3600] with the reversed segments.
    for (mult, seg) in [1_i64, 60, 3600].into_iter().zip(trimmed.split(':').rev()) {
        if seg.is_empty() || !seg.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let n: i64 = seg.parse().ok()?;
        seconds = seconds.checked_add(mult.checked_mul(n)?)?;
    }
    Some(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classifies_album_by_mpre_prefix() {
        let run = json!({
            "text": "Suzume (Lo-Fi)",
            "navigationEndpoint": {"browseEndpoint": {"browseId": "MPREb_fkEvgwHHmzQ"}}
        });
        assert!(matches!(parse_song_run(&run), SongRun::Album { .. }));
    }

    #[test]
    fn classifies_artist_by_non_album_browse_id() {
        let run = json!({
            "text": "Kei",
            "navigationEndpoint": {"browseEndpoint": {"browseId": "UCTr7n4uWRyzCkJA-6cJIXcw"}}
        });
        assert!(matches!(parse_song_run(&run), SongRun::Artist { .. }));
    }

    #[test]
    fn classifies_plain_text_artist_without_id() {
        let run = json!({"text": "Mr Goofy Guy"});
        match parse_song_run(&run) {
            SongRun::Artist { name } => assert_eq!(name, "Mr Goofy Guy"),
            _ => panic!("expected artist"),
        }
    }

    #[test]
    fn views_token_detected() {
        assert!(is_views("576K plays"));
        assert!(is_views("1.4M views"));
        assert!(!is_views("2:04"));
        assert!(!is_views("Lofi Beats Music")); // two spaces → not views
        assert!(!is_views("plays")); // no leading digit
    }

    #[test]
    fn duration_token_detected() {
        assert!(is_duration("2:04"));
        assert!(is_duration("1:02:30"));
        assert!(!is_duration("45")); // needs a colon
        assert!(!is_duration("2,343")); // non-digit
    }

    #[test]
    fn year_token_detected() {
        assert!(is_year("2025"));
        assert!(!is_year("25"));
        assert!(!is_year("20256"));
    }

    #[test]
    fn duration_seconds_matches_python() {
        assert_eq!(parse_duration_seconds("2:04"), Some(124));
        assert_eq!(parse_duration_seconds("1:02:30"), Some(3750));
        assert_eq!(parse_duration_seconds("0:00"), Some(0));
        assert_eq!(parse_duration_seconds(" "), None);
        assert_eq!(parse_duration_seconds("2,343"), None);
    }

    #[test]
    fn parse_song_runs_extracts_artist_album_duration() {
        // Mirrors a search-song flex column 1 + appended views column.
        let runs = vec![
            json!({"text": "Kei", "navigationEndpoint": {"browseEndpoint": {"browseId": "UCxxxx"}}}),
            json!({"text": " \u{2022} "}),
            json!({"text": "Suzume", "navigationEndpoint": {"browseEndpoint": {"browseId": "MPREb_z"}}}),
            json!({"text": " \u{2022} "}),
            json!({"text": "2:04"}),
        ];
        let out = parse_song_runs(&runs, true);
        assert_eq!(out["artists"][0]["name"], "Kei");
        assert_eq!(out["album"]["name"], "Suzume");
        assert_eq!(out["duration"], "2:04");
        assert_eq!(out["duration_seconds"], 124);
    }

    #[test]
    fn parse_song_runs_skips_type_specifier() {
        // "Song" • "Artist" → with skip_type_spec, "Song" is dropped.
        let runs = vec![
            json!({"text": "Song"}),
            json!({"text": " \u{2022} "}),
            json!({"text": "Real Artist"}),
        ];
        let out = parse_song_runs(&runs, true);
        let artists = out["artists"].as_array().unwrap();
        assert_eq!(artists.len(), 1);
        assert_eq!(artists[0]["name"], "Real Artist");
    }
}
