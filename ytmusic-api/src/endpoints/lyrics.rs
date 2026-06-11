//! Stage-1 parsers for the two-call lyrics flow.
//!
//! `api.py::get_lyrics` is two requests: `get_watch_playlist(videoId)` (the
//! `next` endpoint) to discover the lyrics browse id, then `get_lyrics(id)` (a
//! `browse`) for the text. This module provides the two pure extractors:
//!
//! * [`lyrics_browse_id`] — mirrors `get_tab_browse_ids`: the tab whose
//!   `pageType` is `MUSIC_PAGE_TYPE_TRACK_LYRICS`.
//! * [`lyrics_text`] — mirrors `get_lyrics`'s non-timestamped branch:
//!   `contents → SECTION_LIST_ITEM → musicDescriptionShelfRenderer → description
//!   → runs[0].text`.
//!
//! Absence is a value, never an error (battle lesson): a missing tab or empty
//! description yields `None`, which the flow surfaces as "this track has no
//! lyrics".

use serde_json::Value;

use crate::nav::{Step, nav_array, nav_str};

/// The `MUSIC_PAGE_TYPE_TRACK_LYRICS` tab's browse id from a watch (`next`)
/// response, or `None` when the track exposes no lyrics tab.
///
/// Mirrors `get_tab_browse_ids`: walk the watchNext tabs, skip `unselectable`
/// ones, read each selectable tab's `endpoint.browseEndpoint`, and key it by its
/// `pageType`.
pub(crate) fn lyrics_browse_id(watch_response: &Value) -> Option<&str> {
    let tabs = nav_array(
        watch_response,
        &[
            Step::Key("contents"),
            Step::Key("singleColumnMusicWatchNextResultsRenderer"),
            Step::Key("tabbedRenderer"),
            Step::Key("watchNextTabbedResultsRenderer"),
            Step::Key("tabs"),
        ],
    )?;

    for tab in tabs {
        let Some(renderer) = tab.get("tabRenderer") else {
            continue;
        };
        // ytmusicapi: `if "unselectable" in tab["tabRenderer"]: continue`.
        if renderer.get("unselectable").is_some() {
            continue;
        }
        let Some(browse) = nav_browse_endpoint(renderer) else {
            continue;
        };
        if page_type(browse) == Some("MUSIC_PAGE_TYPE_TRACK_LYRICS") {
            return browse.get("browseId").and_then(Value::as_str);
        }
    }
    None
}

/// The lyrics text from a lyrics `browse` response, or `None` when absent.
///
/// Mirrors `get_lyrics`'s untimed branch:
/// `contents.sectionListRenderer.contents[0]
///  .musicDescriptionShelfRenderer.description.runs[0].text`.
pub(crate) fn lyrics_text(lyrics_response: &Value) -> Option<String> {
    nav_str(
        lyrics_response,
        &[
            Step::Key("contents"),
            Step::Key("sectionListRenderer"),
            Step::Key("contents"),
            Step::Index(0),
            Step::Key("musicDescriptionShelfRenderer"),
            Step::Key("description"),
            Step::Key("runs"),
            Step::Index(0),
            Step::Key("text"),
        ],
    )
    .filter(|s| !s.is_empty())
    .map(str::to_owned)
}

/// `tabRenderer.endpoint.browseEndpoint`.
fn nav_browse_endpoint(renderer: &Value) -> Option<&Value> {
    renderer.get("endpoint")?.get("browseEndpoint")
}

/// The `pageType` of a browseEndpoint.
fn page_type(browse: &Value) -> Option<&str> {
    nav_str(
        browse,
        &[
            Step::Key("browseEndpointContextSupportedConfigs"),
            Step::Key("browseEndpointContextMusicConfig"),
            Step::Key("pageType"),
        ],
    )
}
