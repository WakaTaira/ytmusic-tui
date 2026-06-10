//! Stage-1 parser for the `browse` (playlist) endpoint.
//!
//! Mirrors `ytmusicapi.parsers.playlists.parse_playlist_item` for the fields
//! `api.py::get_playlist_tracks` consumes via stage-2 `dict_to_track`:
//! `videoId`, `title`, `artists[].name`, `album.name`, `duration`,
//! `duration_seconds`, `thumbnails`.
//!
//! The shared flex-column index resolution lives in [`parse_playlist_item`] and
//! is reused by the album and artist parsers (album tracks use preset indexes;
//! artist top-songs reuse the playlist-item path).

use serde_json::{Map, Value, json};

use crate::nav::{
    MRLIR, NAVIGATION_BROWSE_ID, PLAY_BUTTON_VIDEO_ID, Step, THUMBNAILS, nav, nav_array, nav_str,
};

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

/// Port of `parse_playlist_items`: convert each MRLIR row into a track dict.
///
/// `is_album` selects the preset-column behavior album track lists require.
pub(crate) fn parse_playlist_items(results: &[Value], is_album: bool) -> Vec<Value> {
    results
        .iter()
        .filter_map(|result| result.get(MRLIR))
        .filter_map(|data| parse_playlist_item(data, is_album))
        .collect()
}

/// Port of `parse_playlist_item` restricted to the stage-2-relevant fields.
///
/// Returns `None` for a "Song deleted" placeholder (mirrors ytmusicapi).
/// `videoId` may be `null` for unplayable items (stage 2 then skips them).
fn parse_playlist_item(data: &Value, is_album: bool) -> Option<Value> {
    let flex_columns = data.get("flexColumns").and_then(Value::as_array)?;

    // For album track lists, indexes are preset because the flex meaning is not
    // reliably derivable from navigationEndpoints (mirrors `use_preset_columns`).
    let mut title_index: Option<usize> = if is_album { Some(0) } else { None };
    let mut artist_index: Option<usize> = if is_album { Some(1) } else { None };
    let mut album_index: Option<usize> = if is_album { Some(2) } else { None };
    let mut duration_index: Option<usize> = None;
    let mut user_channel_indexes: Vec<usize> = Vec::new();
    let mut unrecognized_index: Option<usize> = None;

    for index in 0..flex_columns.len() {
        let Some(run0) = flex_run(data, index, 0) else {
            continue;
        };
        let navigation_endpoint = run0.get("navigationEndpoint");

        let Some(navigation_endpoint) = navigation_endpoint else {
            // No navigation: a duration token sets duration_index; otherwise the
            // first such column is remembered as the unrecognized (artist?) one.
            if let Some(text) = run0.get("text").and_then(Value::as_str) {
                if is_duration_text(text) {
                    duration_index = Some(index);
                } else if unrecognized_index.is_none() {
                    unrecognized_index = Some(index);
                }
            }
            continue;
        };

        if navigation_endpoint.get("watchEndpoint").is_some() {
            title_index = Some(index);
        } else if navigation_endpoint.get("browseEndpoint").is_some() {
            match page_type(navigation_endpoint) {
                "MUSIC_PAGE_TYPE_ARTIST" | "MUSIC_PAGE_TYPE_UNKNOWN" => artist_index = Some(index),
                "MUSIC_PAGE_TYPE_ALBUM" | "MUSIC_PAGE_TYPE_AUDIOBOOK" => album_index = Some(index),
                "MUSIC_PAGE_TYPE_USER_CHANNEL" => user_channel_indexes.push(index),
                "MUSIC_PAGE_TYPE_NON_MUSIC_AUDIO_TRACK_PAGE" => title_index = Some(index),
                _ => {}
            }
        }
    }

    // Fallbacks for non-clickable / video-style artist columns.
    if artist_index.is_none() {
        artist_index = unrecognized_index.or_else(|| user_channel_indexes.last().copied());
    }

    let title = title_index.and_then(|i| item_text(data, i, 0));
    if title == Some("Song deleted") {
        return None;
    }

    let mut out = Map::new();

    // videoId: prefer the play-button watch endpoint.
    out.insert(
        "videoId".to_owned(),
        nav_str(data, PLAY_BUTTON_VIDEO_ID)
            .map(|s| Value::String(s.to_owned()))
            .unwrap_or(Value::Null),
    );

    out.insert(
        "title".to_owned(),
        title
            .map(|t| Value::String(t.to_owned()))
            .unwrap_or(Value::Null),
    );

    // artists: parse_song_artists → parse_artists_runs over the flex column runs.
    if let Some(artists) = artist_index.and_then(|ai| parse_artists_runs(data, ai)) {
        out.insert("artists".to_owned(), artists);
    }

    // album: parse_song_album → {name, id} from the flex column.
    out.insert(
        "album".to_owned(),
        album_index
            .and_then(|i| parse_song_album(data, i))
            .unwrap_or(Value::Null),
    );

    // duration: fixedColumns[0] wins (clock), else the duration flex column.
    let duration = fixed_column_duration(data).or_else(|| {
        duration_index
            .and_then(|i| item_text(data, i, 0))
            .map(str::to_owned)
    });
    if let Some(d) = duration {
        let seconds = parse_duration_seconds(&d);
        out.insert("duration".to_owned(), Value::String(d));
        out.insert(
            "duration_seconds".to_owned(),
            seconds.map(Value::from).unwrap_or(Value::Null),
        );
    }

    // thumbnails.
    if let Some(thumbs) = nav(data, THUMBNAILS) {
        out.insert("thumbnails".to_owned(), thumbs.clone());
    }

    Some(Value::Object(out))
}

/// `parse_song_artists(data, index)` → `parse_artists_runs(runs)`.
///
/// Returns a `[{name, id}]` array (ids may be `null`). The stage-2 `_join_artists`
/// reads `name` only. Returns `None` if the flex column is absent.
fn parse_artists_runs(data: &Value, index: usize) -> Option<Value> {
    let runs = flex_runs(data, index)?;
    // `parse_artists_runs` walks even-indexed runs (skipping " • " separators)
    // and keeps each as an artist with its browseId. We only need the names.
    let artists: Vec<Value> = runs
        .iter()
        .step_by(2)
        .map(|run| {
            let name = run.get("text").and_then(Value::as_str).unwrap_or("");
            let id = nav_str(run, NAVIGATION_BROWSE_ID);
            json!({ "name": name, "id": id })
        })
        .collect();
    Some(Value::Array(artists))
}

/// `parse_song_album(data, index)` → `{name, id}` or `None`.
fn parse_song_album(data: &Value, index: usize) -> Option<Value> {
    let name = item_text(data, index, 0)?;
    let id = flex_run(data, index, 0).and_then(|run| nav_str(run, NAVIGATION_BROWSE_ID));
    Some(json!({ "name": name, "id": id }))
}

/// The clock string in `fixedColumns[0]`, handling both `simpleText` and `runs`
/// shapes (mirrors the `fixedColumns` branch of `parse_playlist_item`).
fn fixed_column_duration(data: &Value) -> Option<String> {
    let fixed = nav(
        data,
        &[
            Step::Key("fixedColumns"),
            Step::Index(0),
            Step::Key("musicResponsiveListItemFixedColumnRenderer"),
            Step::Key("text"),
        ],
    )?;
    if let Some(simple) = fixed.get("simpleText").and_then(Value::as_str) {
        return Some(simple.to_owned());
    }
    fixed
        .get("runs")?
        .get(0)?
        .get("text")?
        .as_str()
        .map(str::to_owned)
}

/// The `pageType` of a browse navigationEndpoint, or `""`.
fn page_type(navigation_endpoint: &Value) -> &str {
    nav_str(
        navigation_endpoint,
        &[
            Step::Key("browseEndpoint"),
            Step::Key("browseEndpointContextSupportedConfigs"),
            Step::Key("browseEndpointContextMusicConfig"),
            Step::Key("pageType"),
        ],
    )
    .unwrap_or("")
}

/// Run `run_index` of flex column `index`, mirroring `get_flex_column_item`.
fn flex_run(data: &Value, index: usize, run_index: usize) -> Option<&Value> {
    flex_runs(data, index)?.get(run_index)
}

/// The runs array of flex column `index`.
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

/// `get_item_text(item, index, run_index)`.
fn item_text(data: &Value, index: usize, run_index: usize) -> Option<&str> {
    flex_run(data, index, run_index)?.get("text")?.as_str()
}

/// `^(\d+:)*\d+:\d+$` — a clock duration (reused from the song-run classifier
/// semantics; duplicated here to keep the playlist parser self-contained).
/// TODO(M3d-2): de-duplicate into songruns (or a shared stage-1 utils module).
fn is_duration_text(text: &str) -> bool {
    let segments: Vec<&str> = text.split(':').collect();
    segments.len() >= 2
        && segments
            .iter()
            .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
}

/// Parse a clock duration into seconds (ytmusicapi `_utils.parse_duration`).
fn parse_duration_seconds(text: &str) -> Option<i64> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut seconds: i64 = 0;
    for (mult, seg) in [1_i64, 60, 3600].into_iter().zip(trimmed.split(':').rev()) {
        if seg.is_empty() || !seg.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let n: i64 = seg.parse().ok()?;
        seconds = seconds.checked_add(mult.checked_mul(n)?)?;
    }
    Some(seconds)
}
