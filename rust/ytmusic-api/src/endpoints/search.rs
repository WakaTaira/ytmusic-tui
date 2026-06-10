//! Stage-1 parser for the `search` endpoint.
//!
//! Mirrors `ytmusicapi.mixins.search.search` + `parsers/search.py`, restricted
//! to the result types `api.py::search_all` routes on (`song`, `video`,
//! `album`, `artist`, `playlist`) and the fields the stage-2
//! `parse::categorize_search_results` reads. Output is a `Vec<Value>` of
//! ytmusicapi-shaped result dicts, fed directly into stage-2.

use serde_json::{Map, Value, json};

use super::songruns::parse_song_runs;
use crate::nav::{
    MRLIR, NAVIGATION_BROWSE_ID, PLAY_BUTTON_VIDEO_ID, Step, THUMBNAILS, nav, nav_array, nav_str,
};

/// Walk a raw `search` InnerTube response into a flat list of ytmusicapi-shaped
/// result dicts.
///
/// `filter` is the wrapper-level category restriction (`"songs"`, `"albums"`,
/// `"artists"`, `"playlists"`, or `None` for a default search). When set, it
/// fixes the `result_type` for every parsed item exactly as the Python mixin
/// does (`internal_filter[:-1]`); when `None`, the type is derived per item.
pub(crate) fn parse_search_response(response: &Value, filter: Option<&str>) -> Vec<Value> {
    let Some(section_list) = section_list(response) else {
        return Vec::new();
    };

    // ytmusicapi: a single itemSectionRenderer section means "no results".
    if section_list.len() == 1 && section_list[0].get("itemSectionRenderer").is_some() {
        // Only bail if that section is the "did you mean" placeholder, i.e. its
        // first content is not an MRLIR. A genuine single-result default search
        // also has one itemSectionRenderer, so guard on the MRLIR presence.
        let first_is_mrlir = nav(&section_list[0], &[Step::Key("itemSectionRenderer")])
            .and_then(|s| s.get("contents"))
            .and_then(|c| c.get(0))
            .map(|item| item.get(MRLIR).is_some())
            .unwrap_or(false);
        if !first_is_mrlir {
            return Vec::new();
        }
    }

    // Per the mixin: a filtered search fixes result_type to filter[:-1].
    let result_type: Option<String> = filter.map(|f| {
        let normalized = if f.contains("playlists") {
            "playlists"
        } else {
            f
        };
        normalized.trim_end_matches('s').to_lowercase()
    });

    let mut results: Vec<Value> = Vec::new();
    for section in section_list {
        let shelf_contents = if let Some(shelf) = section.get("musicShelfRenderer") {
            shelf.get("contents").and_then(Value::as_array)
        } else if let Some(item_section) = section.get("itemSectionRenderer") {
            item_section.get("contents").and_then(Value::as_array)
        } else {
            None
        };

        let Some(shelf_contents) = shelf_contents else {
            continue;
        };

        for entry in shelf_contents {
            let Some(data) = entry.get(MRLIR) else {
                continue;
            };
            if let Some(parsed) = parse_search_result(data, result_type.as_deref()) {
                results.push(parsed);
            }
        }
    }

    results
}

/// Resolve the section list, handling both the tabbed (default search) and the
/// plain (filtered search) response envelopes.
fn section_list(response: &Value) -> Option<&Vec<Value>> {
    let contents = response.get("contents")?;

    let results = if let Some(tabbed) = contents.get("tabbedSearchResultsRenderer") {
        // Default search: tabs[0].tabRenderer.content
        tabbed
            .get("tabs")?
            .get(0)?
            .get("tabRenderer")?
            .get("content")?
    } else {
        contents
    };

    results
        .get("sectionListRenderer")?
        .get("contents")?
        .as_array()
}

/// Parse one MRLIR `data` block into a ytmusicapi-shaped result dict, mirroring
/// `parse_search_result` for the routed result types.
fn parse_search_result(data: &Value, result_type: Option<&str>) -> Option<Value> {
    let video_type = nav_str(data, PLAY_BUTTON_VIDEO_TYPE);

    // Derive the result type when the filter did not fix it.
    let result_type: String = match result_type {
        Some(rt) => rt.to_owned(),
        None => derive_result_type(data, video_type)?,
    };

    let mut out = Map::new();
    out.insert("resultType".to_owned(), Value::String(result_type.clone()));

    match result_type.as_str() {
        "artist" => parse_artist_result(data, &mut out),
        "album" => parse_album_result(data, &mut out),
        "playlist" => parse_playlist_result(data, &mut out),
        "song" | "video" => parse_song_result(data, &mut out),
        // Other types are not routed by api.py; emit the bare resultType so the
        // stage-2 categorizer ignores them.
        _ => {}
    }

    Some(Value::Object(out))
}

/// Derive the result type from browseId / videoType, mirroring the
/// `if not result_type` block of `parse_search_result`.
fn derive_result_type(data: &Value, video_type: Option<&str>) -> Option<String> {
    if let Some(browse_id) = nav_str(data, NAVIGATION_BROWSE_ID) {
        let mapped = [
            ("VM", "playlist"),
            ("RD", "playlist"),
            ("VL", "playlist"),
            ("MPLA", "artist"),
            ("MPRE", "album"),
            ("MPSP", "podcast"),
            ("MPED", "episode"),
            ("UC", "artist"),
        ]
        .into_iter()
        .find(|(prefix, _)| browse_id.starts_with(prefix))
        .map(|(_, ty)| ty.to_owned());
        // ytmusicapi yields None (and thus no resultType) when no prefix matches.
        mapped
    } else {
        Some(
            match video_type.unwrap_or("") {
                "MUSIC_VIDEO_TYPE_ATV" => "song",
                "MUSIC_VIDEO_TYPE_PODCAST_EPISODE" => "episode",
                _ => "video",
            }
            .to_owned(),
        )
    }
}

/// Song / video branch: title, videoId, and the song-run fields (artists,
/// album, duration). `album` is forced to `null` first (ytmusicapi sets
/// `search_result["album"] = None` for songs before the run merge).
fn parse_song_result(data: &Value, out: &mut Map<String, Value>) {
    if let Some(title) = item_text(data, 0, 0) {
        out.insert("title".to_owned(), Value::String(title.to_owned()));
    }
    // Songs default album to null; the run merge overrides it when an album run
    // is present.
    out.insert("album".to_owned(), Value::Null);

    out.insert(
        "videoId".to_owned(),
        nav_str(data, PLAY_BUTTON_VIDEO_ID)
            .map(|s| Value::String(s.to_owned()))
            .unwrap_or(Value::Null),
    );

    merge_song_runs(data, out);
}

/// Build the combined runs (flex column 1, plus flex column 2 with a dummy
/// separator) and merge `parse_song_runs(skip_type_spec=True)` into `out`.
fn merge_song_runs(data: &Value, out: &mut Map<String, Value>) {
    let mut runs: Vec<Value> = flex_runs(data, 1).cloned().unwrap_or_default();
    if let Some(extra) = flex_runs(data, 2) {
        runs.push(json!({ "text": "" }));
        runs.extend(extra.iter().cloned());
    }
    let parsed = parse_song_runs(&runs, true);
    if let Value::Object(map) = parsed {
        for (k, v) in map {
            out.insert(k, v);
        }
    }
}

/// Artist branch: `artist` name (flex column 0) plus `browseId`. api.py's
/// `_dict_to_related_artist` reads `browseId`/`channelId` and `title`/`name`;
/// ytmusicapi stores the artist name under `artist`, so we additionally mirror
/// that into `name` (which `_dict_to_related_artist` falls back to) — but the
/// real ytmusicapi key is `artist`, and stage-2 reads `title or name`. So set
/// `name` to the artist text.
fn parse_artist_result(data: &Value, out: &mut Map<String, Value>) {
    if let Some(name) = item_text(data, 0, 0) {
        // Real ytmusicapi sets "artist"; stage-2 reads title|name, so expose the
        // name under "name" for the fallback path.
        out.insert("artist".to_owned(), Value::String(name.to_owned()));
        out.insert("name".to_owned(), Value::String(name.to_owned()));
    }
    insert_browse_id(data, out);
    insert_thumbnails(data, out);
}

/// Album branch: title (flex column 0) plus `browseId`.
fn parse_album_result(data: &Value, out: &mut Map<String, Value>) {
    if let Some(title) = item_text(data, 0, 0) {
        out.insert("title".to_owned(), Value::String(title.to_owned()));
    }
    insert_browse_id(data, out);
    // Albums also carry artist/year runs in flex column 1; api.py's
    // _dict_to_album_info reads artists + year, so merge the song runs (which
    // classify artist/year tokens).
    merge_song_runs(data, out);
    insert_thumbnails(data, out);
}

/// Playlist branch: title (flex column 0) plus `browseId` (a `VL`-prefixed id).
/// api.py routes playlists via `_dict_to_playlist_info`, which reads
/// `playlistId`. ytmusicapi search exposes the playlist's `browseId`
/// (`VL`+playlistId), so derive `playlistId` from it.
fn parse_playlist_result(data: &Value, out: &mut Map<String, Value>) {
    if let Some(title) = item_text(data, 0, 0) {
        out.insert("title".to_owned(), Value::String(title.to_owned()));
    }
    let browse_id = nav_str(data, NAVIGATION_BROWSE_ID);
    if let Some(browse_id) = browse_id {
        out.insert("browseId".to_owned(), Value::String(browse_id.to_owned()));
        // playlistId = browseId without the leading "VL".
        let playlist_id = browse_id.strip_prefix("VL").unwrap_or(browse_id);
        out.insert(
            "playlistId".to_owned(),
            Value::String(playlist_id.to_owned()),
        );
    }
    insert_thumbnails(data, out);
}

/// Insert `browseId` from the item's navigationEndpoint when present.
fn insert_browse_id(data: &Value, out: &mut Map<String, Value>) {
    if let Some(browse_id) = nav_str(data, NAVIGATION_BROWSE_ID) {
        out.insert("browseId".to_owned(), Value::String(browse_id.to_owned()));
    }
}

/// Insert the `thumbnails` array (ytmusicapi-shaped `[{url,width,height}]`).
fn insert_thumbnails(data: &Value, out: &mut Map<String, Value>) {
    if let Some(thumbs) = nav(data, THUMBNAILS) {
        out.insert("thumbnails".to_owned(), thumbs.clone());
    }
}

/// `get_item_text(item, index, run_index)` — the text of run `run_index` in
/// flex column `index`.
fn item_text(data: &Value, index: usize, run_index: usize) -> Option<&str> {
    flex_runs(data, index)?
        .get(run_index)?
        .get("text")?
        .as_str()
}

/// The runs array of flex column `index`, mirroring `get_flex_column_item`.
fn flex_runs(data: &Value, index: usize) -> Option<&Vec<Value>> {
    nav_array(
        data,
        &[
            Step::Key("flexColumns"),
            Step::Index(index),
            Step::Key("musicResponsiveListItemFlexColumnRenderer"),
            Step::Key("text"),
            Step::Key("runs"),
        ],
    )
}

/// `[*PLAY_BUTTON, "playNavigationEndpoint", *NAVIGATION_VIDEO_TYPE]` — the
/// play-button's `musicVideoType`, used to disambiguate song vs video.
const PLAY_BUTTON_VIDEO_TYPE: &[Step] = &[
    Step::Key("overlay"),
    Step::Key("musicItemThumbnailOverlayRenderer"),
    Step::Key("content"),
    Step::Key("musicPlayButtonRenderer"),
    Step::Key("playNavigationEndpoint"),
    Step::Key("watchEndpoint"),
    Step::Key("watchEndpointMusicSupportedConfigs"),
    Step::Key("watchEndpointMusicConfig"),
    Step::Key("musicVideoType"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A filtered search response uses the NON-tabbed envelope: `contents` holds
    /// the `sectionListRenderer` directly (no `tabbedSearchResultsRenderer`). The
    /// committed fixtures are all tabbed, so this negative fixture covers the
    /// `section_list` else-arm (review-debt from M3d-1).
    #[test]
    fn parses_non_tabbed_filtered_envelope() {
        let response = json!({
            "contents": {
                "sectionListRenderer": { "contents": [
                    { "musicShelfRenderer": { "contents": [
                        { "musicResponsiveListItemRenderer": {
                            "flexColumns": [
                                { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [
                                    { "text": "Filtered Song" }
                                ] } } }
                            ],
                            "overlay": { "musicItemThumbnailOverlayRenderer": { "content": {
                                "musicPlayButtonRenderer": { "playNavigationEndpoint": {
                                    "watchEndpoint": { "videoId": "VID12345678" } } } } } }
                        } }
                    ] } }
                ] }
            }
        });

        // filter="songs" fixes the result_type to "song" for the non-tabbed path.
        let items = parse_search_response(&response, Some("songs"));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["resultType"], "song");
        assert_eq!(items[0]["title"], "Filtered Song");
        assert_eq!(items[0]["videoId"], "VID12345678");
    }

    /// The tabbed envelope (default search) still resolves through the if-arm.
    /// Guards against a regression that would route everything to the else-arm.
    #[test]
    fn parses_tabbed_default_envelope() {
        let response = json!({
            "contents": { "tabbedSearchResultsRenderer": { "tabs": [
                { "tabRenderer": { "content": { "sectionListRenderer": { "contents": [
                    { "musicShelfRenderer": { "contents": [
                        { "musicResponsiveListItemRenderer": {
                            "flexColumns": [
                                { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [
                                    { "text": "Tabbed Song",
                                      "navigationEndpoint": { "watchEndpoint": {
                                          "videoId": "TAB12345678" } } }
                                ] } } }
                            ],
                            "overlay": { "musicItemThumbnailOverlayRenderer": { "content": {
                                "musicPlayButtonRenderer": { "playNavigationEndpoint": {
                                    "watchEndpoint": { "videoId": "TAB12345678",
                                        "watchEndpointMusicSupportedConfigs": {
                                            "watchEndpointMusicConfig": {
                                                "musicVideoType": "MUSIC_VIDEO_TYPE_ATV" } } } } } } } }
                        } }
                    ] } }
                ] } } } }
            ] } }
        });

        // No filter: the result type is derived per item (ATV videoType → "song").
        let items = parse_search_response(&response, None);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["resultType"], "song");
        assert_eq!(items[0]["videoId"], "TAB12345678");
    }
}
