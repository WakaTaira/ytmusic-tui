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

use serde_json::Value;

use super::stage1::parse_playlist_items;
use crate::nav::{Step, nav_array};

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

/// Resolve the `musicPlaylistShelfRenderer.contents` array.
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
