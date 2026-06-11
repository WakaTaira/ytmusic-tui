//! Stage-1 parser for the `browse` (history) endpoint.
//!
//! Mirrors `ytmusicapi.mixins.library.get_history`: the history page is a
//! single-column section list of dated `musicShelfRenderer` shelves ("Today",
//! "This week", …). Each shelf's rows are parsed by `parse_playlist_items` and
//! flattened into one list in shelf order (newest first). ytmusicapi tags each
//! item with `played` (the shelf title) and `feedbackToken`, but
//! `api.py::get_history` reads neither — it just runs `_dict_to_track` over the
//! flat list — so this emits the same track dicts `dict_to_track` consumes.

use serde_json::Value;

use super::stage1::parse_playlist_items;
use crate::nav::{Step, nav_array};

/// Walk a raw `FEmusic_history` response into a flat list of ytmusicapi-shaped
/// track dicts, concatenating every dated shelf in order.
///
/// Returns an empty list when the section list is absent. (ytmusicapi raises a
/// `YTMusicServerError` for a shelf-less notifier row; api.py would surface that
/// as a transport-style error. The trimmed contract here treats a missing
/// section list as "no history".)
pub(crate) fn parse_history(response: &Value) -> Vec<Value> {
    let Some(sections) = section_list(response) else {
        return Vec::new();
    };
    sections
        .iter()
        .filter_map(|section| {
            nav_array(
                section,
                &[Step::Key("musicShelfRenderer"), Step::Key("contents")],
            )
        })
        .flat_map(|shelf| parse_playlist_items(shelf, false))
        .collect()
}

/// `SINGLE_COLUMN_TAB + SECTION_LIST` — the history page's dated-shelf list.
fn section_list(response: &Value) -> Option<&Vec<Value>> {
    nav_array(
        response,
        &[
            Step::Key("contents"),
            Step::Key("singleColumnBrowseResultsRenderer"),
            Step::Key("tabs"),
            Step::Index(0),
            Step::Key("tabRenderer"),
            Step::Key("content"),
            Step::Key("sectionListRenderer"),
            Step::Key("contents"),
        ],
    )
}
