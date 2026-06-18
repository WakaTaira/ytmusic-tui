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
//! Playlists with more than one shelf-page (~100 tracks for YTM) expose the
//! next-page continuation token in one of two shapes — both are tolerated:
//!
//! * **Modern** (current InnerTube surface): the last element of
//!   `musicPlaylistShelfRenderer.contents` is a `continuationItemRenderer`
//!   carrying the token at
//!   `continuationEndpoint.continuationCommand.token`. The continuation
//!   response replaces `continuationContents` with
//!   `onResponseReceivedActions[0].appendContinuationItemsAction.continuationItems`,
//!   whose last entry is again a `continuationItemRenderer`.
//! * **Legacy** (older cohort): the initial shelf carries the token at
//!   `continuations[0].nextContinuationData.continuation`; continuation
//!   responses use `continuationContents.musicPlaylistShelfContinuation`.
//!
//! Issue #26: the legacy-only parser silently capped playlists at one page
//! (~96 visible tracks) because YTM moved everything to the modern shape.
//! Both shapes coexist across cohorts, so the parsers try modern first and
//! fall back to legacy.

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
///
/// The trailing `continuationItemRenderer` (modern shape) is filtered out by
/// `parse_playlist_items` because it carries no MRLIR key — no extra branch
/// here.
pub(crate) fn parse_playlist_tracks(response: &Value) -> Vec<Value> {
    let Some(shelf) = nav_array(response, SHELF_CONTENTS) else {
        return Vec::new();
    };
    parse_playlist_items(shelf, false)
}

/// Extract the next-page continuation token from an initial playlist `browse`
/// response, or `None` when the response carries no further pages.
///
/// Tries the modern `continuationItemRenderer` at the tail of
/// `musicPlaylistShelfRenderer.contents` first, then falls back to the legacy
/// `continuations[0].nextContinuationData.continuation` path.
pub(crate) fn parse_continuation_token(response: &Value) -> Option<String> {
    if let Some(token) = modern_continuation_token(nav_array(response, SHELF_CONTENTS)?) {
        return Some(token);
    }
    nav_str(response, INITIAL_LEGACY_TOKEN).map(str::to_owned)
}

/// Walk a continuation `browse` response into a list of ytmusicapi-shaped
/// track dicts.
///
/// Continuation responses come in two shapes; both are recognised:
/// * modern — `onResponseReceivedActions[0].appendContinuationItemsAction
///   .continuationItems`
/// * legacy — `continuationContents.musicPlaylistShelfContinuation.contents`
///
/// The inner row layout (`musicResponsiveListItemRenderer`) is identical, so we
/// reuse `parse_playlist_items` either way (it filters out the trailing
/// `continuationItemRenderer`).
pub(crate) fn parse_continuation_tracks(response: &Value) -> Vec<Value> {
    let Some(shelf) = continuation_shelf(response) else {
        return Vec::new();
    };
    parse_playlist_items(shelf, false)
}

/// Extract the next-page continuation token from a continuation response, or
/// `None` when the chain is exhausted.
///
/// Same modern-first / legacy-fallback strategy as
/// [`parse_continuation_token`].
pub(crate) fn parse_continuation_next_token(response: &Value) -> Option<String> {
    if let Some(items) = nav_array(response, MODERN_CONTINUATION_ITEMS)
        && let Some(token) = modern_continuation_token(items)
    {
        return Some(token);
    }
    nav_str(response, LEGACY_CONTINUATION_NEXT_TOKEN).map(str::to_owned)
}

/// Resolve the row array on a continuation response, preferring the modern
/// `appendContinuationItemsAction` shape over the legacy
/// `musicPlaylistShelfContinuation` wrapper.
fn continuation_shelf(response: &Value) -> Option<&Vec<Value>> {
    nav_array(response, MODERN_CONTINUATION_ITEMS)
        .or_else(|| nav_array(response, LEGACY_CONTINUATION_CONTENTS))
}

/// When the last element of `items` is a `continuationItemRenderer`, return
/// its `continuationEndpoint.continuationCommand.token`; otherwise `None`.
fn modern_continuation_token(items: &[Value]) -> Option<String> {
    let last = items.last()?;
    nav_str(last, CONTINUATION_ITEM_TOKEN).map(str::to_owned)
}

/// `musicPlaylistShelfRenderer.contents` on an initial playlist response.
const SHELF_CONTENTS: &[Step] = &[
    Step::Key("contents"),
    Step::Key("twoColumnBrowseResultsRenderer"),
    Step::Key("secondaryContents"),
    Step::Key("sectionListRenderer"),
    Step::Key("contents"),
    Step::Index(0),
    Step::Key("musicPlaylistShelfRenderer"),
    Step::Key("contents"),
];

/// Modern continuation response rows:
/// `onResponseReceivedActions[0].appendContinuationItemsAction.continuationItems`.
const MODERN_CONTINUATION_ITEMS: &[Step] = &[
    Step::Key("onResponseReceivedActions"),
    Step::Index(0),
    Step::Key("appendContinuationItemsAction"),
    Step::Key("continuationItems"),
];

/// Token extraction path on a `continuationItemRenderer` row:
/// `.continuationEndpoint.continuationCommand.token`.
const CONTINUATION_ITEM_TOKEN: &[Step] = &[
    Step::Key("continuationItemRenderer"),
    Step::Key("continuationEndpoint"),
    Step::Key("continuationCommand"),
    Step::Key("token"),
];

/// Legacy initial-response token path.
const INITIAL_LEGACY_TOKEN: &[Step] = &[
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

/// Legacy continuation-response row array.
const LEGACY_CONTINUATION_CONTENTS: &[Step] = &[
    Step::Key("continuationContents"),
    Step::Key("musicPlaylistShelfContinuation"),
    Step::Key("contents"),
];

/// Legacy continuation-response token path.
const LEGACY_CONTINUATION_NEXT_TOKEN: &[Step] = &[
    Step::Key("continuationContents"),
    Step::Key("musicPlaylistShelfContinuation"),
    Step::Key("continuations"),
    Step::Index(0),
    Step::Key("nextContinuationData"),
    Step::Key("continuation"),
];
