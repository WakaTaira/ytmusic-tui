//! Stage-1 + assembly for the library `browse` endpoints.
//!
//! Mirrors `ytmusicapi.mixins.library.get_library_{playlists,albums,artists}`
//! and `parsers/library.py` for the fields the stage-2 converters read:
//!
//! * playlists — `FEmusic_liked_playlists`, GRID, `parse_playlist` over
//!   `items[1:]` (the `items[0]` "New playlist" pseudo-item is skipped, like
//!   ytmusicapi). Stage 2 is `dict_to_playlist_info`.
//! * albums — `FEmusic_liked_albums`, GRID, `parse_albums` over all items.
//!   Stage 2 is `dict_to_album_info`.
//! * artists — `FEmusic_library_corpus_track_artists`, MUSIC_SHELF,
//!   `parse_artists` over the shelf rows. api.py reads the `artist` + `browseId`
//!   keys directly and builds `ArtistInfo::new_minimal`; it does NOT route
//!   through `dict_to_related_artist` (gotcha M3d-2/4), so [`parse_library_artists`]
//!   returns the typed [`ArtistInfo`] list rather than a ytmusicapi-shaped dict.

use serde_json::{Value, json};

use super::songruns::parse_song_runs;
use super::stage1::{item_text, leading_count};
use crate::models::{AlbumInfo, ArtistInfo, PlaylistInfo};
use crate::nav::{
    MRLIR, MTRIR, NAVIGATION_BROWSE_ID, Step, THUMBNAIL_RENDERER, THUMBNAILS, nav, nav_array,
    nav_str,
};
use crate::parse::{dict_to_album_info, dict_to_playlist_info, pick_largest_thumbnail};

/// Parse a `FEmusic_liked_playlists` response into [`PlaylistInfo`]s.
///
/// GRID items, skipping `items[0]` (the "New playlist" card), each parsed by
/// `parse_playlist` and converted via `dict_to_playlist_info`.
pub(crate) fn parse_library_playlists(response: &Value) -> Vec<PlaylistInfo> {
    let Some(items) = library_grid_items(response) else {
        return Vec::new();
    };
    items
        .iter()
        .skip(1) // ytmusicapi: results["items"][1:]
        .filter_map(|item| item.get(MTRIR))
        .map(parse_playlist_card)
        .filter_map(|dict| dict_to_playlist_info(&dict))
        .collect()
}

/// Parse a `FEmusic_liked_albums` response into [`AlbumInfo`]s (no track list).
///
/// GRID items (no skip), each MTRIR parsed by `parse_albums` and converted via
/// `dict_to_album_info`.
pub(crate) fn parse_library_albums(response: &Value) -> Vec<AlbumInfo> {
    let Some(items) = library_grid_items(response) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| item.get(MTRIR))
        .map(parse_album_card)
        .filter_map(|dict| dict_to_album_info(&dict))
        .collect()
}

/// Parse a `FEmusic_library_corpus_track_artists` response into [`ArtistInfo`]s.
///
/// MUSIC_SHELF rows, each MRLIR read for `browseId` + `artist` (the row's text)
/// — exactly the two keys `api.py::get_library_artists` consumes. Rows without a
/// browseId are skipped (api.py: `if not channel_id: continue`).
pub(crate) fn parse_library_artists(response: &Value) -> Vec<ArtistInfo> {
    let Some(rows) = library_shelf_contents(response) else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|row| row.get(MRLIR))
        .filter_map(parse_library_artist_row)
        .collect()
}

/// Build an [`ArtistInfo`] from one MRLIR library-artist row.
///
/// `browseId` = `NAVIGATION_BROWSE_ID`; `artist` = flex column 0 text. Mirrors
/// `parse_artists` for those two keys, then `api.py`'s `new_minimal` assembly.
fn parse_library_artist_row(data: &Value) -> Option<ArtistInfo> {
    let channel_id = nav_str(data, NAVIGATION_BROWSE_ID).filter(|s| !s.is_empty())?;
    let name = item_text(data, 0, 0).unwrap_or("");
    let thumbnail_url = pick_largest_thumbnail(nav(data, THUMBNAILS).unwrap_or(&Value::Null));
    Some(ArtistInfo::new_minimal(channel_id, name, thumbnail_url))
}

/// `parse_playlist` restricted to `dict_to_playlist_info`'s reads.
///
/// `playlistId` = `title.runs[0].navigationEndpoint.browseEndpoint.browseId[2:]`
/// (drops the leading `VL`); `count` from the `SUBTITLE2` "<n> tracks" token when
/// the subtitle has the recognised 3-run shape; `description` = the concatenated
/// subtitle run texts.
fn parse_playlist_card(card: &Value) -> Value {
    let title = nav_str(card, crate::nav::TITLE_TEXT).unwrap_or("");
    let browse_id = nav_str(card, TITLE_RUN0_BROWSE_ID).unwrap_or("");
    let playlist_id = browse_id.strip_prefix("VL").unwrap_or(browse_id);

    let mut out = json!({
        "playlistId": playlist_id,
        "title": title,
        "thumbnails": nav(card, THUMBNAIL_RENDERER).cloned().unwrap_or(Value::Null),
    });

    // subtitle → description (joined run texts) + optional count.
    if let Some(runs) = nav_array(card, &[Step::Key("subtitle"), Step::Key("runs")]) {
        let description: String = runs
            .iter()
            .filter_map(|r| r.get("text").and_then(Value::as_str))
            .collect();
        out["description"] = Value::String(description);

        // ytmusicapi: when len(runs)==3 and SUBTITLE2 matches r"\d+ ", the count
        // is the leading number of run[2].
        if runs.len() == 3
            && let Some(sub2) = runs
                .get(2)
                .and_then(|r| r.get("text"))
                .and_then(Value::as_str)
            && let Some(count) = leading_count(sub2)
        {
            out["count"] = Value::String(count.to_owned());
        }
    }
    out
}

/// `parse_albums` restricted to `dict_to_album_info`'s reads.
///
/// `browseId` = `TITLE + NAVIGATION_BROWSE_ID`; `title`/`thumbnails` direct; and
/// when the subtitle has `runs`, `parse_song_runs(subtitle.runs[2:])` supplies
/// the `artists` and `year` (the leading two runs are the type spec + separator).
fn parse_album_card(card: &Value) -> Value {
    let mut out = json!({
        "browseId": nav_str(card, TITLE_RUN0_BROWSE_ID),
        "title": nav_str(card, crate::nav::TITLE_TEXT).unwrap_or(""),
        "thumbnails": nav(card, THUMBNAIL_RENDERER).cloned().unwrap_or(Value::Null),
    });

    if let Some(runs) = nav_array(card, &[Step::Key("subtitle"), Step::Key("runs")]) {
        let tail = if runs.len() > 2 { &runs[2..] } else { &[][..] };
        let parsed = parse_song_runs(tail, false);
        if let Value::Object(map) = parsed {
            for (k, v) in map {
                // dict_to_album_info reads only `artists` and `year` from here.
                if k == "artists" || k == "year" {
                    out[k] = v;
                }
            }
        }
    }
    out
}

/// The GRID `items` array, mirroring `get_library_contents(response, GRID)` for
/// the modern (itemSection-less) response: descend to the section list's first
/// renderer and read `gridRenderer.items`.
fn library_grid_items(response: &Value) -> Option<&Vec<Value>> {
    let section = library_first_section(response)?;
    nav_array(section, &[Step::Key("gridRenderer"), Step::Key("items")])
}

/// The MUSIC_SHELF `contents`, the artists-list analogue of [`library_grid_items`].
fn library_shelf_contents(response: &Value) -> Option<&Vec<Value>> {
    let section = library_first_section(response)?;
    nav_array(
        section,
        &[Step::Key("musicShelfRenderer"), Step::Key("contents")],
    )
}

/// The first section under the single-column library tab.
///
/// Mirrors `get_library_contents`: prefer an `itemSectionRenderer` wrapper when
/// present (older responses), otherwise the section list's first entry directly
/// (the modern response, observed live, lists the grid/shelf renderer at
/// `sectionListRenderer.contents[0]`).
fn library_first_section(response: &Value) -> Option<&Value> {
    let sections = nav_array(
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
    )?;
    // Prefer the itemSectionRenderer's first content if any section carries one.
    for section in sections {
        if let Some(item_section) = section.get("itemSectionRenderer")
            && let Some(first) = item_section
                .get("contents")
                .and_then(Value::as_array)
                .and_then(|c| c.first())
        {
            return Some(first);
        }
    }
    sections.first()
}

/// `title.runs[0].navigationEndpoint.browseEndpoint.browseId` — the card's
/// browse id (ytmusicapi's `TITLE + NAVIGATION_BROWSE_ID`).
const TITLE_RUN0_BROWSE_ID: &[Step] = &[
    Step::Key("title"),
    Step::Key("runs"),
    Step::Index(0),
    Step::Key("navigationEndpoint"),
    Step::Key("browseEndpoint"),
    Step::Key("browseId"),
];
