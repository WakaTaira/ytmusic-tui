//! Shared stage-1 primitives reused across the `browse`/`next` endpoints.
//!
//! These were promoted out of `playlist.rs` (M3d-1) so the library, history,
//! home, and radio parsers reuse one definition instead of re-porting the
//! flex-column resolution and the `parse_playlist_items` row walker (gotcha a).
//!
//! Everything here mirrors `ytmusicapi.parsers.playlists` /
//! `ytmusicapi.parsers.songs` restricted to the fields the stage-2
//! `parse::dict_to_*` converters read: `videoId`, `title`, `artists[].name`,
//! `album.name`, `duration`, `duration_seconds`, `thumbnails`.

use serde_json::{Map, Value, json};

use super::songruns::{is_duration, parse_duration_seconds};
use crate::nav::{
    MRLIR, NAVIGATION_BROWSE_ID, PLAY_BUTTON_VIDEO_ID, Step, THUMBNAILS, nav, nav_array, nav_str,
};

/// Port of `parse_playlist_items`: convert each MRLIR row into a track dict.
///
/// `is_album` selects the preset-column behavior album track lists require.
/// Reused by the playlist, album, artist top-songs, library-songs (none yet),
/// and history parsers.
pub(super) fn parse_playlist_items(results: &[Value], is_album: bool) -> Vec<Value> {
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
                if is_duration(text) {
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
///
/// `pub(super)` so the album/artist strapline/related parsers reuse it.
pub(super) fn parse_artists_runs(data: &Value, index: usize) -> Option<Value> {
    let runs = flex_runs(data, index)?;
    // `parse_artists_runs` walks even-indexed runs (skipping " • " separators)
    // and keeps each as an artist with its browseId. We only need the names.
    Some(artists_from_runs(runs))
}

/// Build a `[{name, id}]` array from a raw runs slice by stepping over the
/// `" • "` separators (the run-list form of `parse_artists_runs`).
///
/// `pub(super)` so the album strapline / related-card parsers reuse the exact
/// even-index walk instead of re-implementing it.
pub(super) fn artists_from_runs(runs: &[Value]) -> Value {
    let artists: Vec<Value> = runs
        .iter()
        .step_by(2)
        .map(|run| {
            let name = run.get("text").and_then(Value::as_str).unwrap_or("");
            let id = nav_str(run, NAVIGATION_BROWSE_ID);
            json!({ "name": name, "id": id })
        })
        .collect();
    Value::Array(artists)
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
///
/// `pub(super)` so sibling parsers that read a specific flex run reuse it.
pub(super) fn flex_run(data: &Value, index: usize, run_index: usize) -> Option<&Value> {
    flex_runs(data, index)?.get(run_index)
}

/// The runs array of flex column `index`.
///
/// `pub(super)` so the search/library parsers reuse the same path.
pub(super) fn flex_runs(data: &Value, index: usize) -> Option<&Vec<Value>> {
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
///
/// `pub(super)` so the library-artists parser reads column text directly.
pub(super) fn item_text(data: &Value, index: usize, run_index: usize) -> Option<&str> {
    flex_run(data, index, run_index)?.get("text")?.as_str()
}
