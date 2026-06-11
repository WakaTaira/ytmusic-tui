//! Stage-1 + assembly for the `browse` (album) endpoint.
//!
//! Mirrors `ytmusicapi.mixins.browsing.get_album` (header via
//! `parse_album_header_2024`, tracks via `parse_playlist_items(is_album=True)`,
//! then the post-processing loop) followed by `api.py::get_album`'s assembly
//! (`_join_artists`, `_dict_to_album_track`, `_pick_largest_thumbnail`).
//!
//! The post-processing is faithful: ytmusicapi overwrites each track's `album`
//! with the album *title string* and fills empty `artists` from the album-level
//! artists. The stage-2 `dict_to_album_track` then sees a string `album` (→ "")
//! and a populated `artists` list, producing the identical domain `Track` the
//! Python pipeline yields.

use serde_json::Value;

use super::songruns::parse_song_runs;
use super::stage1::{artists_from_runs, parse_playlist_items};
use crate::models::{AlbumInfo, Track};
use crate::nav::{Step, THUMBNAILS, nav, nav_array, nav_str};
use crate::parse::{dict_to_album_track, join_artists, pick_largest_thumbnail};

/// Parse a raw album `browse` response into an [`AlbumInfo`].
///
/// `browse_id` is the requested album id, used verbatim as `AlbumInfo.browse_id`
/// (matching `api.py`, which passes the argument straight through).
pub(crate) fn parse_album(response: &Value, browse_id: &str) -> AlbumInfo {
    let header = album_header(response);

    let title = header
        .and_then(|h| nav_str(h, crate::nav::TITLE_TEXT))
        .unwrap_or("")
        .to_owned();

    // artists: parse_artists_runs(straplineTextOne.runs) → [{name, id}].
    let artists_value = header.and_then(strapline_artists).unwrap_or(Value::Null);
    let artist = join_artists(&artists_value);

    // year: parse_song_runs(subtitle.runs[2:]) → the "year" token.
    let year = header
        .and_then(|h| nav_array(h, &[Step::Key("subtitle"), Step::Key("runs")]))
        .map(|runs| year_from_subtitle(runs))
        .unwrap_or_default();

    let thumbnail_url = header
        .and_then(|h| nav(h, THUMBNAILS))
        .map(pick_largest_thumbnail)
        .unwrap_or_default();

    // Tracks: the album shelf, post-processed exactly like the mixin.
    let raw_tracks = album_tracks(response);
    let tracks: Vec<Track> = raw_tracks
        .iter()
        .filter_map(|t| dict_to_album_track(t, &artist))
        .collect();

    AlbumInfo::new(browse_id, title, artist, year, tracks, thumbnail_url)
}

/// The `musicResponsiveHeaderRenderer` block, mirroring `parse_album_header_2024`'s
/// `nav(response, [...TWO_COLUMN..., RESPONSIVE_HEADER])`.
fn album_header(response: &Value) -> Option<&Value> {
    nav(
        response,
        &[
            Step::Key("contents"),
            Step::Key("twoColumnBrowseResultsRenderer"),
            Step::Key("tabs"),
            Step::Index(0),
            Step::Key("tabRenderer"),
            Step::Key("content"),
            Step::Key("sectionListRenderer"),
            Step::Key("contents"),
            Step::Index(0),
            Step::Key("musicResponsiveHeaderRenderer"),
        ],
    )
}

/// The album track shelf, post-processed: `album` overwritten with the title
/// string and empty `artists` filled from the album-level artists.
fn album_tracks(response: &Value) -> Vec<Value> {
    let Some(shelf) = album_track_shelf(response) else {
        return Vec::new();
    };
    let title = album_header(response)
        .and_then(|h| nav_str(h, crate::nav::TITLE_TEXT))
        .unwrap_or("")
        .to_owned();
    let album_artists = album_header(response)
        .and_then(strapline_artists)
        .unwrap_or(Value::Null);

    let mut tracks = parse_playlist_items(shelf, true);
    for track in &mut tracks {
        if let Some(obj) = track.as_object_mut() {
            // album → album title string (ytmusicapi post-processing).
            obj.insert("album".to_owned(), Value::String(title.clone()));
            // artists → existing, else album-level.
            let empty = obj
                .get("artists")
                .map(|a| a.as_array().map(|v| v.is_empty()).unwrap_or(true))
                .unwrap_or(true);
            if empty {
                obj.insert("artists".to_owned(), album_artists.clone());
            }
        }
    }
    tracks
}

/// `secondaryContents.sectionListRenderer.contents[0].musicShelfRenderer.contents`.
fn album_track_shelf(response: &Value) -> Option<&Vec<Value>> {
    nav_array(
        response,
        &[
            Step::Key("contents"),
            Step::Key("twoColumnBrowseResultsRenderer"),
            Step::Key("secondaryContents"),
            Step::Key("sectionListRenderer"),
            Step::Key("contents"),
            Step::Index(0),
            Step::Key("musicShelfRenderer"),
            Step::Key("contents"),
        ],
    )
}

/// `parse_artists_runs(straplineTextOne.runs)` → `[{name, id}]`.
fn strapline_artists(header: &Value) -> Option<Value> {
    let runs = nav_array(header, &[Step::Key("straplineTextOne"), Step::Key("runs")])?;
    Some(artists_from_runs(runs))
}

/// Extract the `year` token from `subtitle.runs` via the song-run classifier
/// (mirrors `parse_song_runs(header["subtitle"]["runs"][2:])`).
fn year_from_subtitle(runs: &[Value]) -> String {
    // ytmusicapi slices off the first two runs (type spec + separator) before
    // classifying.
    let tail = if runs.len() > 2 { &runs[2..] } else { &[][..] };
    let parsed = parse_song_runs(tail, false);
    parsed
        .get("year")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned()
}
