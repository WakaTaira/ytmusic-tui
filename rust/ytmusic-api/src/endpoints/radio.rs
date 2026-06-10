//! Stage-1 parser for the `next` (watch / radio) endpoint.
//!
//! Mirrors `ytmusicapi.parsers.watch.parse_watch_playlist` + `parse_watch_track`
//! restricted to the fields the stage-2 `parse::watch_item_to_track` reads:
//! `videoId`, `title`, `length` (string), `thumbnail` (SINGULAR — `["thumbnail",
//! "thumbnails"]`), and the `artists`/`album` produced by
//! `parse_song_runs(longBylineText.runs)`. The watch dict shape deliberately
//! differs from the playlist/search dicts (gotcha M3d-2/c): duration lives in
//! `length`, not `duration_seconds`, and thumbnails are under the singular key.

use serde_json::{Map, Value};

use super::songruns::parse_song_runs;
use crate::nav::{Step, TITLE_TEXT, nav, nav_array, nav_str};

/// Walk a raw `next` (radio) response into a list of watch-shaped track dicts.
///
/// The queue lives under
/// `contents.singleColumnMusicWatchNextResultsRenderer.tabbedRenderer
///  .watchNextTabbedResultsRenderer.tabs[0].tabRenderer.content
///  .musicQueueRenderer.content.playlistPanelRenderer.contents`.
/// Returns an empty list when that panel is absent. (ytmusicapi raises a
/// `YTMusicServerError` for a missing panel; the trimmed contract here yields an
/// empty queue, and `api.py::get_radio` already tolerates a non-dict result.)
pub(crate) fn parse_radio(response: &Value) -> Vec<Value> {
    let Some(items) = panel_contents(response) else {
        return Vec::new();
    };
    items.iter().filter_map(parse_watch_item).collect()
}

/// `playlistPanelRenderer.contents`.
fn panel_contents(response: &Value) -> Option<&Vec<Value>> {
    nav_array(
        response,
        &[
            Step::Key("contents"),
            Step::Key("singleColumnMusicWatchNextResultsRenderer"),
            Step::Key("tabbedRenderer"),
            Step::Key("watchNextTabbedResultsRenderer"),
            Step::Key("tabs"),
            Step::Index(0),
            Step::Key("tabRenderer"),
            Step::Key("content"),
            Step::Key("musicQueueRenderer"),
            Step::Key("content"),
            Step::Key("playlistPanelRenderer"),
            Step::Key("contents"),
        ],
    )
}

/// Port of `parse_watch_playlist`'s per-row handling: unwrap the wrapper's
/// `primaryRenderer`, skip non-`playlistPanelVideoRenderer` rows (ads) and
/// `unplayableText` rows, then parse the track.
fn parse_watch_item(result: &Value) -> Option<Value> {
    let data = if let Some(wrapper) = result.get("playlistPanelVideoWrapperRenderer") {
        wrapper.get("primaryRenderer")?.get(PPVR)?
    } else {
        result.get(PPVR)?
    };
    if data.get("unplayableText").is_some() {
        return None;
    }
    Some(parse_watch_track(data))
}

/// Port of `parse_watch_track` for `watch_item_to_track`'s reads.
///
/// `videoId`/`title`/`length`/`thumbnail` direct; `artists`/`album` from
/// `parse_song_runs(longBylineText.runs)` (no `skip_type_spec`, matching
/// ytmusicapi). `likeStatus`, `videoType`, and the menu-derived fields are
/// dropped — stage 2 never reads them.
fn parse_watch_track(data: &Value) -> Value {
    let mut out = Map::new();

    if let Some(video_id) = data.get("videoId").and_then(Value::as_str) {
        out.insert("videoId".to_owned(), Value::String(video_id.to_owned()));
    }
    if let Some(title) = nav_str(data, TITLE_TEXT) {
        out.insert("title".to_owned(), Value::String(title.to_owned()));
    }
    // length = lengthText.runs[0].text (a "M:SS" clock string).
    if let Some(length) = nav_str(
        data,
        &[
            Step::Key("lengthText"),
            Step::Key("runs"),
            Step::Index(0),
            Step::Key("text"),
        ],
    ) {
        out.insert("length".to_owned(), Value::String(length.to_owned()));
    }
    // thumbnail (SINGULAR): ["thumbnail", "thumbnails"].
    if let Some(thumbs) = nav(data, &[Step::Key("thumbnail"), Step::Key("thumbnails")]) {
        out.insert("thumbnail".to_owned(), thumbs.clone());
    }
    // artists / album / (year) from the byline runs.
    if let Some(runs) = nav_array(data, &[Step::Key("longBylineText"), Step::Key("runs")]) {
        let parsed = parse_song_runs(runs, false);
        if let Value::Object(map) = parsed {
            for (k, v) in map {
                out.insert(k, v);
            }
        }
    }

    Value::Object(out)
}

/// `playlistPanelVideoRenderer`.
const PPVR: &str = "playlistPanelVideoRenderer";
