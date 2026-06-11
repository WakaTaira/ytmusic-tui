//! Stage-1 parser for the `browse` (home) endpoint.
//!
//! Mirrors `ytmusicapi.parsers.browsing.parse_mixed_content` restricted to the
//! item types `parse::parse_home_sections` (stage 2) routes on. The home page is
//! a list of carousels; each carousel item is a `musicTwoRowItemRenderer`
//! (song / album / playlist / artist card), a `musicResponsiveListItemRenderer`
//! (flat song), or a `musicMultiRowListItemRenderer` (episode). This produces
//! the `[{title, contents:[...]}]` raw-sections list stage 2 consumes; the
//! per-item dicts carry exactly the shape (`videoId` / `playlistId` /
//! `browseId` / `audioPlaylistId` + the stage-2-read fields) that
//! `parse_home_sections`'s branch logic keys on.

use serde_json::{Map, Value, json};

use super::songruns::parse_song_runs;
use super::stage1::leading_count;
use crate::nav::{
    CAROUSEL_TITLE_TEXT, MMRIR, MRLIR, MTRIR, NAVIGATION_PLAYLIST_ID, NAVIGATION_VIDEO_ID,
    NAVIGATION_WATCH_PLAYLIST_ID, PLAY_BUTTON_VIDEO_ID, SUBTITLE_RUNS, Step,
    THUMBNAIL_OVERLAY_WATCH_PID, THUMBNAIL_OVERLAY_WATCH_PLAYLIST_ID, THUMBNAIL_RENDERER,
    THUMBNAILS, TITLE_RUN0_BROWSE_ID, TITLE_TEXT, nav, nav_array, nav_str,
};

/// Parse a raw `FEmusic_home` response into the ytmusicapi-shaped section list
/// `parse::parse_home_sections` consumes.
///
/// Each output section is `{ "title": <carousel title>, "contents": [<items>] }`.
/// Returns an empty list when the section list is absent.
pub(crate) fn parse_home(response: &Value) -> Vec<Value> {
    let Some(rows) = section_list(response) else {
        return Vec::new();
    };
    rows.iter().filter_map(parse_row).collect()
}

/// `SINGLE_COLUMN_TAB + SECTION_LIST` — the home page's carousel list.
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

/// Port of one `parse_mixed_content` row (the carousel branch).
///
/// The description-shelf branch is intentionally omitted: it produces a section
/// whose contents are description-run dicts that `parse_home_sections` cannot
/// route to a track/playlist/album, so it never contributes home items.
/// A row whose single renderer value has no `contents` is skipped (mirrors
/// `if "contents" not in results: continue`).
fn parse_row(row: &Value) -> Option<Value> {
    // `next(iter(row.values()))`: the single renderer payload.
    let results = row.as_object()?.values().next()?;
    let contents = results.get("contents")?.as_array()?;
    let title = nav_str(results, CAROUSEL_TITLE_TEXT).unwrap_or("");

    let items: Vec<Value> = contents.iter().filter_map(parse_carousel_item).collect();
    Some(json!({ "title": title, "contents": items }))
}

/// Dispatch one carousel item to its renderer-specific parser, mirroring the
/// MTRIR / MRLIR / MMRIR branches of `parse_mixed_content`.
fn parse_carousel_item(item: &Value) -> Option<Value> {
    if let Some(data) = item.get(MTRIR) {
        return Some(parse_two_row(data));
    }
    if let Some(data) = item.get(MRLIR) {
        return Some(parse_song_flat(data));
    }
    if item.get(MMRIR).is_some() {
        // Episodes: stage 2 has no episode branch (no videoId/playlistId/
        // browseId it routes), so emit an empty object that parse_home_sections
        // drops via dict_to_track → None. Keeps section item counts faithful to
        // the Python pipeline, which also yields nothing usable for episodes.
        return Some(Value::Object(Map::new()));
    }
    None
}

/// MTRIR dispatch by `pageType`, mirroring `parse_mixed_content`'s inner block.
fn parse_two_row(data: &Value) -> Value {
    match two_row_page_type(data) {
        // song / watch_playlist (no pageType).
        None => {
            if nav(data, NAVIGATION_WATCH_PLAYLIST_ID).is_some() {
                parse_watch_playlist_card(data)
            } else {
                parse_song(data)
            }
        }
        Some("MUSIC_PAGE_TYPE_ALBUM") | Some("MUSIC_PAGE_TYPE_AUDIOBOOK") => parse_album_card(data),
        Some("MUSIC_PAGE_TYPE_ARTIST") | Some("MUSIC_PAGE_TYPE_USER_CHANNEL") => {
            parse_related_artist_card(data)
        }
        Some("MUSIC_PAGE_TYPE_PLAYLIST") => parse_playlist_card(data),
        // Other page types (podcast show) have no stage-2 home branch.
        Some(_) => Value::Object(Map::new()),
    }
}

/// `TITLE + NAVIGATION_BROWSE + PAGE_TYPE` on a two-row card.
fn two_row_page_type(data: &Value) -> Option<&str> {
    nav_str(
        data,
        &[
            Step::Key("title"),
            Step::Key("runs"),
            Step::Index(0),
            Step::Key("navigationEndpoint"),
            Step::Key("browseEndpoint"),
            Step::Key("browseEndpointContextSupportedConfigs"),
            Step::Key("browseEndpointContextMusicConfig"),
            Step::Key("pageType"),
        ],
    )
}

/// `parse_song` (MTRIR): `{title, videoId, playlistId, thumbnails, +song_runs}`.
fn parse_song(data: &Value) -> Value {
    let mut out = Map::new();
    insert_str(&mut out, "title", nav_str(data, TITLE_TEXT));
    insert_str(&mut out, "videoId", nav_str(data, NAVIGATION_VIDEO_ID));
    insert_str(
        &mut out,
        "playlistId",
        nav_str(data, NAVIGATION_PLAYLIST_ID),
    );
    insert_thumbs(&mut out, data, THUMBNAIL_RENDERER);
    merge_object(&mut out, parse_song_runs_subtitle(data, true));
    Value::Object(out)
}

/// `parse_watch_playlist` (MTRIR): `{title, playlistId, thumbnails}` where the
/// playlistId is the `watchPlaylistEndpoint` id.
fn parse_watch_playlist_card(data: &Value) -> Value {
    let mut out = Map::new();
    insert_str(&mut out, "title", nav_str(data, TITLE_TEXT));
    insert_str(
        &mut out,
        "playlistId",
        nav_str(data, NAVIGATION_WATCH_PLAYLIST_ID),
    );
    insert_thumbs(&mut out, data, THUMBNAIL_RENDERER);
    Value::Object(out)
}

/// `parse_album` (MTRIR): `{title, artists, browseId, audioPlaylistId,
/// thumbnails, year?}` for the fields stage 2 reads.
fn parse_album_card(data: &Value) -> Value {
    let mut out = Map::new();
    insert_str(&mut out, "title", nav_str(data, TITLE_TEXT));
    out.insert("artists".to_owned(), subtitle_nav_artists(data));
    insert_str(&mut out, "browseId", nav_str(data, TITLE_RUN0_BROWSE_ID));
    if let Some(playlist_id) = audio_playlist_id(data) {
        out.insert(
            "audioPlaylistId".to_owned(),
            Value::String(playlist_id.to_owned()),
        );
    }
    insert_thumbs(&mut out, data, THUMBNAIL_RENDERER);
    // year via `_parse_album_single_subtitle`. (Stage-2's home album-like branch
    // discards year when it wraps the album as a PlaylistInfo, but the stage-1
    // dict still mirrors ytmusicapi faithfully.)
    if let Some(year) = subtitle_year(data) {
        out.insert("year".to_owned(), Value::String(year));
    }
    Value::Object(out)
}

/// `parse_playlist` (MTRIR): `{title, playlistId, thumbnails, description?,
/// count?}` for the fields stage 2 reads.
fn parse_playlist_card(data: &Value) -> Value {
    let mut out = Map::new();
    insert_str(&mut out, "title", nav_str(data, TITLE_TEXT));
    // playlistId = TITLE + NAVIGATION_BROWSE_ID with the leading "VL" dropped.
    if let Some(browse_id) = nav_str(data, TITLE_RUN0_BROWSE_ID) {
        let playlist_id = browse_id.strip_prefix("VL").unwrap_or(browse_id);
        out.insert(
            "playlistId".to_owned(),
            Value::String(playlist_id.to_owned()),
        );
    }
    insert_thumbs(&mut out, data, THUMBNAIL_RENDERER);
    // subtitle → description (joined run texts) + count (SUBTITLE2 leading num).
    if let Some(runs) = nav_array(data, SUBTITLE_RUNS) {
        let description: String = runs
            .iter()
            .filter_map(|r| r.get("text").and_then(Value::as_str))
            .collect();
        out.insert("description".to_owned(), Value::String(description));
        if runs.len() == 3
            && let Some(sub2) = runs
                .get(2)
                .and_then(|r| r.get("text"))
                .and_then(Value::as_str)
            && let Some(count) = leading_count(sub2)
        {
            out.insert("count".to_owned(), Value::String(count.to_owned()));
        }
    }
    Value::Object(out)
}

/// `parse_related_artist` (MTRIR): `{title, browseId, subscribers?, thumbnails}`.
fn parse_related_artist_card(data: &Value) -> Value {
    let mut out = Map::new();
    insert_str(&mut out, "title", nav_str(data, TITLE_TEXT));
    insert_str(&mut out, "browseId", nav_str(data, TITLE_RUN0_BROWSE_ID));
    insert_thumbs(&mut out, data, THUMBNAIL_RENDERER);
    Value::Object(out)
}

/// `parse_song_flat` (MRLIR): `{title, videoId, thumbnails, +song_runs, album?}`.
fn parse_song_flat(data: &Value) -> Value {
    let mut out = Map::new();
    // title / videoId come from flex column 0.
    let title = flex_run0_text(data, 0);
    insert_str(&mut out, "title", title);
    // flat videoId: column-0 run navigationEndpoint, else play button.
    let video_id = flex_run0_video_id(data, 0).or_else(|| nav_str(data, PLAY_BUTTON_VIDEO_ID));
    insert_str(&mut out, "videoId", video_id);
    insert_thumbs(&mut out, data, THUMBNAILS);
    // runs from flex column 1.
    if let Some(runs) = flex_runs(data, 1) {
        merge_object(&mut out, parse_song_runs(runs, true));
    }
    // album from flex column 2 when it has a navigationEndpoint.
    if let Some(album) = flat_album(data) {
        out.insert("album".to_owned(), album);
    }
    Value::Object(out)
}

// --- shared field helpers --------------------------------------------------

/// `parse_song_runs(SUBTITLE_RUNS, skip_type_spec)` as an object (or empty).
fn parse_song_runs_subtitle(data: &Value, skip_type_spec: bool) -> Value {
    match nav_array(data, SUBTITLE_RUNS) {
        Some(runs) => parse_song_runs(runs, skip_type_spec),
        None => Value::Object(Map::new()),
    }
}

/// Subtitle runs carrying a `navigationEndpoint` → `[{name, id}]`
/// (mirrors `parse_album`'s `parse_id_name` filter for clickable runs).
fn subtitle_nav_artists(data: &Value) -> Value {
    let runs = match nav_array(data, SUBTITLE_RUNS) {
        Some(r) => r,
        None => return Value::Array(Vec::new()),
    };
    let artists: Vec<Value> = runs
        .iter()
        .filter(|run| run.get("navigationEndpoint").is_some())
        .map(|run| {
            json!({
                "name": run.get("text").and_then(Value::as_str).unwrap_or(""),
                "id": nav_str(run, &[
                    Step::Key("navigationEndpoint"),
                    Step::Key("browseEndpoint"),
                    Step::Key("browseId"),
                ]),
            })
        })
        .collect();
    Value::Array(artists)
}

/// `_parse_album_single_subtitle`'s year extraction: `SUBTITLE` (run 0) when
/// numeric, else — when run 0 is a type spec — `SUBTITLE2` (run 2) when numeric.
fn subtitle_year(data: &Value) -> Option<String> {
    let sub0 = subtitle_run_text(data, 0)?;
    if is_numeric(sub0) {
        return Some(sub0.to_owned());
    }
    // run 0 was a type ("Album"/"Single"/…); the year may live in run 2.
    let sub2 = subtitle_run_text(data, 2)?;
    is_numeric(sub2).then(|| sub2.to_owned())
}

/// `subtitle.runs[index].text`.
fn subtitle_run_text(data: &Value, index: usize) -> Option<&str> {
    nav_str(
        data,
        &[
            Step::Key("subtitle"),
            Step::Key("runs"),
            Step::Index(index),
            Step::Key("text"),
        ],
    )
}

/// A non-empty all-ASCII-digit string (ytmusicapi's `str.isnumeric()` for the
/// year tokens, which are plain digit runs).
fn is_numeric(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

/// `audioPlaylistId` from the thumbnail overlay (authenticated, then fallback).
fn audio_playlist_id(data: &Value) -> Option<&str> {
    nav_str(data, THUMBNAIL_OVERLAY_WATCH_PID)
        .or_else(|| nav_str(data, THUMBNAIL_OVERLAY_WATCH_PLAYLIST_ID))
}

/// flex column `index` runs (MRLIR), reusing the shared path shape.
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

/// Text of run 0 of flex column `index`.
fn flex_run0_text(data: &Value, index: usize) -> Option<&str> {
    flex_runs(data, index)?.first()?.get("text")?.as_str()
}

/// videoId of run 0 of flex column `index` (`TEXT_RUN + NAVIGATION_VIDEO_ID`).
fn flex_run0_video_id(data: &Value, index: usize) -> Option<&str> {
    nav_str(flex_runs(data, index)?.first()?, NAVIGATION_VIDEO_ID)
}

/// `parse_song_flat`'s album block: flex column 2 run 0 `{name, id}` when it has
/// a navigationEndpoint.
fn flat_album(data: &Value) -> Option<Value> {
    let run0 = flex_runs(data, 2)?.first()?;
    run0.get("navigationEndpoint")?; // album column only when clickable
    Some(json!({
        "name": run0.get("text").and_then(Value::as_str).unwrap_or(""),
        "id": nav_str(run0, &[
            Step::Key("navigationEndpoint"),
            Step::Key("browseEndpoint"),
            Step::Key("browseId"),
        ]),
    }))
}

/// Insert `key: <string>` when present (skips `None` so the key is simply absent,
/// matching how `parse_home_sections` tests key presence).
fn insert_str(out: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(v) = value {
        out.insert(key.to_owned(), Value::String(v.to_owned()));
    }
}

/// Insert `thumbnails` from `path` when present.
fn insert_thumbs(out: &mut Map<String, Value>, data: &Value, path: &[Step]) {
    if let Some(thumbs) = nav(data, path) {
        out.insert("thumbnails".to_owned(), thumbs.clone());
    }
}

/// Merge an object's entries into `out` (the `dict.update(...)` form).
fn merge_object(out: &mut Map<String, Value>, value: Value) {
    if let Value::Object(map) = value {
        for (k, v) in map {
            out.insert(k, v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The live home fixture is all MTRIR; cover the MRLIR flat-song branch
    /// (`parse_song_flat`) and its album column directly.
    #[test]
    fn parse_song_flat_extracts_video_album_and_artists() {
        let item = json!({
            MRLIR: {
                "flexColumns": [
                    { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [
                        { "text": "Quick Pick Song",
                          "navigationEndpoint": { "watchEndpoint": { "videoId": "QP123" } } }
                    ] } } },
                    { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [
                        { "text": "Some Artist",
                          "navigationEndpoint": { "browseEndpoint": { "browseId": "UCart" } } }
                    ] } } },
                    { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [
                        { "text": "Some Album",
                          "navigationEndpoint": { "browseEndpoint": { "browseId": "MPREb_x" } } }
                    ] } } }
                ],
                "thumbnail": { "musicThumbnailRenderer": { "thumbnail": { "thumbnails": [
                    { "url": "https://t/x", "width": 60, "height": 60 }
                ] } } }
            }
        });
        let out = parse_carousel_item(&item).expect("flat song parsed");
        assert_eq!(out["videoId"], "QP123");
        assert_eq!(out["title"], "Quick Pick Song");
        assert_eq!(out["artists"][0]["name"], "Some Artist");
        assert_eq!(out["album"]["name"], "Some Album");
        // No playlistId / browseId at top level → stage 2 routes it as a Track.
        assert!(out.get("playlistId").is_none());
        assert!(out.get("browseId").is_none());
    }

    /// Cover the MTRIR album-card branch (`MUSIC_PAGE_TYPE_ALBUM`): browseId,
    /// audioPlaylistId from the thumbnail overlay, artists, and year.
    #[test]
    fn parse_album_card_extracts_browse_audio_playlist_and_year() {
        let item = json!({
            MTRIR: {
                "title": { "runs": [
                    { "text": "An Album",
                      "navigationEndpoint": { "browseEndpoint": {
                          "browseId": "MPREb_album",
                          "browseEndpointContextSupportedConfigs": {
                              "browseEndpointContextMusicConfig": {
                                  "pageType": "MUSIC_PAGE_TYPE_ALBUM" } } } } }
                ] },
                // Real album-card subtitle shape: ["Album", " • ", "2021"].
                // run0 is the type spec, run2 is the year (SUBTITLE2 branch of
                // _parse_album_single_subtitle). No clickable artist run here.
                "subtitle": { "runs": [
                    { "text": "Album" },
                    { "text": " • " },
                    { "text": "2021" }
                ] },
                "thumbnailOverlay": { "musicItemThumbnailOverlayRenderer": { "content": {
                    "musicPlayButtonRenderer": { "playNavigationEndpoint": {
                        "watchPlaylistEndpoint": { "playlistId": "OLAK5audio" } } } } } },
                "thumbnailRenderer": { "musicThumbnailRenderer": { "thumbnail": {
                    "thumbnails": [ { "url": "https://t/a", "width": 226, "height": 226 } ] } } }
            }
        });
        let out = parse_carousel_item(&item).expect("album card parsed");
        assert_eq!(out["browseId"], "MPREb_album");
        assert_eq!(out["audioPlaylistId"], "OLAK5audio");
        assert_eq!(out["year"], "2021"); // via the SUBTITLE2 branch
        assert_eq!(out["artists"], json!([])); // no clickable artist run
        // browseId present + no videoId → stage 2 album-like branch.
        assert!(out.get("videoId").is_none());
    }
}
