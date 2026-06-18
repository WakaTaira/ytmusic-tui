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
//!
//! # Continuations (issue #26)
//!
//! Each list endpoint paginates: the initial response carries the first ~25–50
//! items plus a continuation marker, and the flow walks the chain until the
//! caller's `limit` is reached. Two shapes are tolerated:
//!
//! * **Modern**: a `continuationItemRenderer` sits at the tail of the items
//!   array (`gridRenderer.items` for playlists/albums, `musicShelfRenderer.contents`
//!   for artists). Continuation responses arrive under
//!   `onResponseReceivedActions[0].appendContinuationItemsAction.continuationItems`.
//! * **Legacy**: the token lives on the renderer itself at
//!   `…continuations[0].nextContinuationData.continuation`; continuation
//!   responses use `continuationContents.{gridContinuation,musicShelfContinuation}`.
//!
//! Both initial-page parsers and both continuation walkers try modern first
//! and fall back to legacy. Previously `get_library_*` ignored continuations
//! entirely, capping users with >25–50 saved items at the first page.

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

// ---------------------------------------------------------------------------
// Continuations (issue #26)
//
// All three list endpoints share the same modern-first / legacy-fallback
// strategy. The grid- and shelf-flavoured helpers below are exported to
// `mod.rs` so the wrapper flows can drive a uniform paging loop.
// ---------------------------------------------------------------------------

/// Extract the initial-response next-page token from a `FEmusic_liked_*` GRID
/// playlist/album response.
///
/// Prefers the modern `continuationItemRenderer` sitting at the tail of
/// `gridRenderer.items`; falls back to the legacy
/// `gridRenderer.continuations[0].nextContinuationData.continuation` path.
pub(crate) fn parse_grid_initial_continuation_token(response: &Value) -> Option<String> {
    if let Some(items) = library_grid_items(response)
        && let Some(token) = continuation_marker_token(items)
    {
        return Some(token);
    }
    let section = library_first_section(response)?;
    nav_str(section, GRID_RENDERER_LEGACY_TOKEN).map(str::to_owned)
}

/// Extract the next-page token from a GRID continuation response.
pub(crate) fn parse_grid_continuation_next_token(response: &Value) -> Option<String> {
    if let Some(items) = nav_array(response, MODERN_CONTINUATION_ITEMS)
        && let Some(token) = continuation_marker_token(items)
    {
        return Some(token);
    }
    nav_str(response, GRID_CONTINUATION_LEGACY_NEXT_TOKEN).map(str::to_owned)
}

/// Parse a GRID continuation response into [`PlaylistInfo`]s (no `skip(1)` —
/// the "New playlist" pseudo-item only ships on the initial page).
pub(crate) fn parse_library_continuation_playlists(response: &Value) -> Vec<PlaylistInfo> {
    grid_continuation_items(response)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get(MTRIR))
        .map(parse_playlist_card)
        .filter_map(|dict| dict_to_playlist_info(&dict))
        .collect()
}

/// Parse a GRID continuation response into [`AlbumInfo`]s.
pub(crate) fn parse_library_continuation_albums(response: &Value) -> Vec<AlbumInfo> {
    grid_continuation_items(response)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get(MTRIR))
        .map(parse_album_card)
        .filter_map(|dict| dict_to_album_info(&dict))
        .collect()
}

/// Extract the initial-response next-page token from a
/// `FEmusic_library_corpus_track_artists` MUSIC_SHELF response.
pub(crate) fn parse_shelf_initial_continuation_token(response: &Value) -> Option<String> {
    if let Some(rows) = library_shelf_contents(response)
        && let Some(token) = continuation_marker_token(rows)
    {
        return Some(token);
    }
    let section = library_first_section(response)?;
    nav_str(section, MUSIC_SHELF_LEGACY_TOKEN).map(str::to_owned)
}

/// Extract the next-page token from a MUSIC_SHELF continuation response.
pub(crate) fn parse_shelf_continuation_next_token(response: &Value) -> Option<String> {
    if let Some(rows) = nav_array(response, MODERN_CONTINUATION_ITEMS)
        && let Some(token) = continuation_marker_token(rows)
    {
        return Some(token);
    }
    nav_str(response, SHELF_CONTINUATION_LEGACY_NEXT_TOKEN).map(str::to_owned)
}

/// Parse a MUSIC_SHELF continuation response into [`ArtistInfo`]s.
pub(crate) fn parse_library_continuation_artists(response: &Value) -> Vec<ArtistInfo> {
    shelf_continuation_rows(response)
        .into_iter()
        .flatten()
        .filter_map(|row| row.get(MRLIR))
        .filter_map(parse_library_artist_row)
        .collect()
}

/// Resolve the items array on a GRID continuation response, preferring the
/// modern `appendContinuationItemsAction` shape.
fn grid_continuation_items(response: &Value) -> Option<&Vec<Value>> {
    nav_array(response, MODERN_CONTINUATION_ITEMS)
        .or_else(|| nav_array(response, GRID_LEGACY_CONTINUATION_ITEMS))
}

/// Resolve the rows array on a MUSIC_SHELF continuation response, preferring
/// the modern `appendContinuationItemsAction` shape.
fn shelf_continuation_rows(response: &Value) -> Option<&Vec<Value>> {
    nav_array(response, MODERN_CONTINUATION_ITEMS)
        .or_else(|| nav_array(response, SHELF_LEGACY_CONTINUATION_CONTENTS))
}

/// When the last element of `items` is a `continuationItemRenderer`, return
/// its `continuationEndpoint.continuationCommand.token`; otherwise `None`.
fn continuation_marker_token(items: &[Value]) -> Option<String> {
    let last = items.last()?;
    nav_str(last, CONTINUATION_ITEM_TOKEN).map(str::to_owned)
}

/// Modern continuation response items — same path as for playlists.
const MODERN_CONTINUATION_ITEMS: &[Step] = &[
    Step::Key("onResponseReceivedActions"),
    Step::Index(0),
    Step::Key("appendContinuationItemsAction"),
    Step::Key("continuationItems"),
];

/// Token extraction on a `continuationItemRenderer` row.
const CONTINUATION_ITEM_TOKEN: &[Step] = &[
    Step::Key("continuationItemRenderer"),
    Step::Key("continuationEndpoint"),
    Step::Key("continuationCommand"),
    Step::Key("token"),
];

/// Legacy initial-response token on the GRID renderer:
/// `gridRenderer.continuations[0].nextContinuationData.continuation` (rooted
/// at the section returned by [`library_first_section`]).
const GRID_RENDERER_LEGACY_TOKEN: &[Step] = &[
    Step::Key("gridRenderer"),
    Step::Key("continuations"),
    Step::Index(0),
    Step::Key("nextContinuationData"),
    Step::Key("continuation"),
];

/// Legacy GRID continuation response items.
const GRID_LEGACY_CONTINUATION_ITEMS: &[Step] = &[
    Step::Key("continuationContents"),
    Step::Key("gridContinuation"),
    Step::Key("items"),
];

/// Legacy GRID continuation response next-page token.
const GRID_CONTINUATION_LEGACY_NEXT_TOKEN: &[Step] = &[
    Step::Key("continuationContents"),
    Step::Key("gridContinuation"),
    Step::Key("continuations"),
    Step::Index(0),
    Step::Key("nextContinuationData"),
    Step::Key("continuation"),
];

/// Legacy initial-response token on the MUSIC_SHELF renderer (rooted at the
/// section returned by [`library_first_section`]).
const MUSIC_SHELF_LEGACY_TOKEN: &[Step] = &[
    Step::Key("musicShelfRenderer"),
    Step::Key("continuations"),
    Step::Index(0),
    Step::Key("nextContinuationData"),
    Step::Key("continuation"),
];

/// Legacy MUSIC_SHELF continuation response rows.
const SHELF_LEGACY_CONTINUATION_CONTENTS: &[Step] = &[
    Step::Key("continuationContents"),
    Step::Key("musicShelfContinuation"),
    Step::Key("contents"),
];

/// Legacy MUSIC_SHELF continuation response next-page token.
const SHELF_CONTINUATION_LEGACY_NEXT_TOKEN: &[Step] = &[
    Step::Key("continuationContents"),
    Step::Key("musicShelfContinuation"),
    Step::Key("continuations"),
    Step::Index(0),
    Step::Key("nextContinuationData"),
    Step::Key("continuation"),
];

/// Defensive upper bound on continuation pages for a single library load.
///
/// Library pages run ~25–50 items each; capping at 50 covers ~2500 items per
/// list — well past any realistic user — while preventing a runaway loop if
/// the server returns a continuation token that does not advance. If a user
/// genuinely needs more, paging this up is one constant edit away.
pub(super) const MAX_LIBRARY_CONTINUATION_PAGES: usize = 50;
