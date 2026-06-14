//! Stage-1 parser for the `browse` (playlist) endpoint.
//!
//! Mirrors `ytmusicapi.parsers.playlists.parse_playlist_item` for the fields
//! `api.py::get_playlist_tracks` consumes via stage-2 `dict_to_track`:
//! `videoId`, `title`, `artists[].name`, `album.name`, `duration`,
//! `duration_seconds`, `thumbnails`.
//!
//! The flex-column resolution and the `parse_playlist_items` row walker were
//! promoted to [`super::stage1`] (M3d-2) so the album, artist, library, and
//! history parsers share one definition.
//!
//! # Continuations
//!
//! Playlists with more than one shelf-page (~100 tracks for YTM) carry a
//! `continuations[0].nextContinuationData.continuation` token next to the
//! `contents` array. The flow function in [`super`] walks these tokens to load
//! the rest of the playlist; [`parse_continuation_token`] extracts it and
//! [`parse_continuation_tracks`] parses the resulting page (which lives at a
//! different JSON path than the initial response).

use serde_json::Value;

use super::stage1::parse_playlist_items;
use crate::nav::{Step, nav_array, nav_str};

/// Walk a raw playlist `browse` response into a list of ytmusicapi-shaped track
/// dicts (the `tracks` list `api.py` reads).
///
/// The track shelf lives under
/// `contents.twoColumnBrowseResultsRenderer.secondaryContents
///  .sectionListRenderer.contents[0].musicPlaylistShelfRenderer.contents`.
/// Returns an empty list when the shelf is absent (mirrors
/// `api.py`'s `raw_playlist.get("tracks") or []`).
pub(crate) fn parse_playlist_tracks(response: &Value) -> Vec<Value> {
    let Some(shelf) = playlist_shelf(response) else {
        return Vec::new();
    };
    parse_playlist_items(shelf, false)
}

/// Extract the next-page continuation token from a playlist `browse` response,
/// or `None` when the response carries no further pages.
///
/// Mirrors ytmusicapi's `get_continuation_token`: the token lives at
/// `…musicPlaylistShelfRenderer.continuations[0].nextContinuationData
///  .continuation`.
pub(crate) fn parse_continuation_token(response: &Value) -> Option<String> {
    nav_str(response, INITIAL_CONTINUATION_TOKEN).map(str::to_owned)
}

/// Walk a continuation `browse` response into a list of ytmusicapi-shaped
/// track dicts.
///
/// Continuation responses replace the initial `twoColumnBrowseResultsRenderer`
/// wrapper with `continuationContents.musicPlaylistShelfContinuation`, but the
/// inner row layout (`contents[].musicResponsiveListItemRenderer`) is identical
/// — so we reuse the same row parser.
pub(crate) fn parse_continuation_tracks(response: &Value) -> Vec<Value> {
    let Some(shelf) = continuation_shelf(response) else {
        return Vec::new();
    };
    parse_playlist_items(shelf, false)
}

/// Extract the next-page continuation token from a continuation response, or
/// `None` when the chain is exhausted. The path differs from the initial
/// response (continuation vs. shelf renderer).
pub(crate) fn parse_continuation_next_token(response: &Value) -> Option<String> {
    nav_str(response, CONTINUATION_NEXT_TOKEN).map(str::to_owned)
}

/// Resolve the `musicPlaylistShelfRenderer.contents` array on an initial
/// playlist response.
fn playlist_shelf(response: &Value) -> Option<&Vec<Value>> {
    nav_array(
        response,
        &[
            Step::Key("contents"),
            Step::Key("twoColumnBrowseResultsRenderer"),
            Step::Key("secondaryContents"),
            Step::Key("sectionListRenderer"),
            Step::Key("contents"),
            Step::Index(0),
            Step::Key("musicPlaylistShelfRenderer"),
            Step::Key("contents"),
        ],
    )
}

/// Resolve the row array on a `musicPlaylistShelfContinuation` response.
fn continuation_shelf(response: &Value) -> Option<&Vec<Value>> {
    nav_array(
        response,
        &[
            Step::Key("continuationContents"),
            Step::Key("musicPlaylistShelfContinuation"),
            Step::Key("contents"),
        ],
    )
}

/// Path to the next-page token on an initial playlist response.
const INITIAL_CONTINUATION_TOKEN: &[Step] = &[
    Step::Key("contents"),
    Step::Key("twoColumnBrowseResultsRenderer"),
    Step::Key("secondaryContents"),
    Step::Key("sectionListRenderer"),
    Step::Key("contents"),
    Step::Index(0),
    Step::Key("musicPlaylistShelfRenderer"),
    Step::Key("continuations"),
    Step::Index(0),
    Step::Key("nextContinuationData"),
    Step::Key("continuation"),
];

/// Path to the next-page token on a continuation response.
const CONTINUATION_NEXT_TOKEN: &[Step] = &[
    Step::Key("continuationContents"),
    Step::Key("musicPlaylistShelfContinuation"),
    Step::Key("continuations"),
    Step::Index(0),
    Step::Key("nextContinuationData"),
    Step::Key("continuation"),
];
