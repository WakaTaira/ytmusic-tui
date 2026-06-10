//! Stage-1 + assembly for the `browse` (artist) endpoint.
//!
//! Mirrors `ytmusicapi.mixins.browsing.get_artist` (name from the immersive
//! header, top songs from the leading `musicShelfRenderer`, albums/related from
//! `parse_channel_contents`' carousels) followed by `api.py::get_artist`'s
//! assembly. The returned `channel_id` is the *input* id, matching `api.py`
//! (which ignores the parsed `subscriptionButton.channelId`).
//!
//! `api.py` consumes only `raw["songs"]["results"]`, `raw["albums"]["results"]`,
//! and `raw["related"]["results"]`; the `singles` carousel (a sibling of
//! `albums`) is intentionally ignored, matching the Python wrapper.

use serde_json::{Value, json};

use super::playlist::parse_playlist_items;
use crate::models::{AlbumInfo, ArtistInfo, RelatedArtist, Track};
use crate::nav::{
    MTRIR, NAVIGATION_BROWSE_ID, Step, THUMBNAIL_RENDERER, THUMBNAILS, TITLE_TEXT, nav, nav_array,
    nav_str,
};
use crate::parse::{
    dict_to_album_info, dict_to_related_artist, dict_to_track, pick_largest_thumbnail,
};

/// Parse a raw artist `browse` response into an [`ArtistInfo`].
pub(crate) fn parse_artist(response: &Value, channel_id: &str) -> ArtistInfo {
    let header = immersive_header(response);
    let name = header
        .and_then(|h| nav_str(h, TITLE_TEXT))
        .unwrap_or("")
        .to_owned();
    let thumbnail_url = header
        .and_then(|h| nav(h, THUMBNAILS))
        .map(pick_largest_thumbnail)
        .unwrap_or_default();

    let sections = section_list(response);

    // description: api.py reads raw["description"], built from the description
    // shelf. Absent in the common case → "". (M3d keeps this minimal; the
    // description shelf parse can be added when a fixture exercises it.)
    let description = description_text(sections);

    let top_songs = parse_top_songs(sections);
    let albums = parse_albums(sections);
    let related_artists = parse_related(sections);

    ArtistInfo::new(
        channel_id,
        name,
        description,
        top_songs,
        albums,
        related_artists,
        thumbnail_url,
    )
}

/// `header.musicImmersiveHeaderRenderer`.
fn immersive_header(response: &Value) -> Option<&Value> {
    nav(
        response,
        &[
            Step::Key("header"),
            Step::Key("musicImmersiveHeaderRenderer"),
        ],
    )
}

/// `SINGLE_COLUMN_TAB + SECTION_LIST` — the artist page's section list.
fn section_list(response: &Value) -> &[Value] {
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
    .map(Vec::as_slice)
    .unwrap_or(&[])
}

/// Top songs: the leading `musicShelfRenderer.contents`, parsed as playlist
/// items (non-album), then converted via stage-2 `dict_to_track`.
fn parse_top_songs(sections: &[Value]) -> Vec<Track> {
    let Some(first) = sections.first() else {
        return Vec::new();
    };
    let Some(shelf) = nav_array(
        first,
        &[Step::Key("musicShelfRenderer"), Step::Key("contents")],
    ) else {
        return Vec::new();
    };
    parse_playlist_items(shelf, false)
        .iter()
        .filter_map(dict_to_track)
        .collect()
}

/// Albums: the carousel whose (lowercased) title is exactly "albums", parsed as
/// MTRIR album cards, then converted via stage-2 `dict_to_album_info`.
///
/// The "singles & eps" carousel is a separate `parse_channel_contents` category
/// (`singles`) that `api.py` ignores, so the strict title match excludes it.
fn parse_albums(sections: &[Value]) -> Vec<AlbumInfo> {
    let Some(carousel) = carousel_by_title(sections, "albums") else {
        return Vec::new();
    };
    let Some(contents) = carousel.get("contents").and_then(Value::as_array) else {
        return Vec::new();
    };
    contents
        .iter()
        .filter_map(|item| item.get(MTRIR))
        .map(parse_album_card)
        .filter_map(|card| dict_to_album_info(&card))
        .collect()
}

/// Related artists: the "related" carousel parsed as MTRIR cards, then
/// converted via stage-2 `dict_to_related_artist`.
fn parse_related(sections: &[Value]) -> Vec<RelatedArtist> {
    let Some(carousel) = carousel_by_title(sections, "related") else {
        return Vec::new();
    };
    let Some(contents) = carousel.get("contents").and_then(Value::as_array) else {
        return Vec::new();
    };
    contents
        .iter()
        .filter_map(|item| item.get(MTRIR))
        .map(parse_related_card)
        .filter_map(|card| dict_to_related_artist(&card))
        .collect()
}

/// Find the `musicCarouselShelfRenderer` whose basic-header title (lowercased)
/// equals `title_lower`, mirroring `parse_channel_contents`' category match.
fn carousel_by_title<'a>(sections: &'a [Value], title_lower: &str) -> Option<&'a Value> {
    sections
        .iter()
        .filter_map(|s| s.get("musicCarouselShelfRenderer"))
        .find(|carousel| {
            carousel_title(carousel)
                .map(|t| t.to_lowercase() == title_lower)
                .unwrap_or(false)
        })
}

/// `header.musicCarouselShelfBasicHeaderRenderer.title.runs[0].text`.
fn carousel_title(carousel: &Value) -> Option<&str> {
    nav_str(
        carousel,
        &[
            Step::Key("header"),
            Step::Key("musicCarouselShelfBasicHeaderRenderer"),
            Step::Key("title"),
            Step::Key("runs"),
            Step::Index(0),
            Step::Key("text"),
        ],
    )
}

/// Build a ytmusicapi-shaped album dict from an MTRIR album card, mirroring
/// `parse_album` for the fields `_dict_to_album_info` reads (`browseId`,
/// `title`, `artists`, `year`, `thumbnails`).
fn parse_album_card(card: &Value) -> Value {
    let title = nav_str(card, TITLE_TEXT).unwrap_or("");
    let browse_id = nav_str(card, TITLE_RUN0_BROWSE_ID);

    // artists: subtitle runs that carry a navigationEndpoint → {id, name}.
    let artists: Vec<Value> = nav_array(card, &[Step::Key("subtitle"), Step::Key("runs")])
        .map(|runs| {
            runs.iter()
                .filter(|run| run.get("navigationEndpoint").is_some())
                .map(|run| {
                    json!({
                        "name": run.get("text").and_then(Value::as_str).unwrap_or(""),
                        "id": nav_str(run, NAVIGATION_BROWSE_ID),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    // year: subtitle run 0 text when numeric (mirrors _parse_album_single_subtitle).
    let year = subtitle_year(card);

    json!({
        "browseId": browse_id,
        "title": title,
        "artists": artists,
        "year": year,
        "thumbnails": nav(card, THUMBNAIL_RENDERER).cloned().unwrap_or(Value::Null),
    })
}

/// Build a ytmusicapi-shaped artist dict from an MTRIR related-artist card,
/// mirroring `parse_related_artist` (`browseId`, `title`, `thumbnails`).
fn parse_related_card(card: &Value) -> Value {
    json!({
        "browseId": nav_str(card, TITLE_RUN0_BROWSE_ID),
        "title": nav_str(card, TITLE_TEXT).unwrap_or(""),
        "thumbnails": nav(card, THUMBNAIL_RENDERER).cloned().unwrap_or(Value::Null),
    })
}

/// `subtitle.runs[0].text` when numeric, else `""`, mirroring
/// `_parse_album_single_subtitle`'s year extraction for the year-only case.
fn subtitle_year(card: &Value) -> String {
    nav_str(
        card,
        &[
            Step::Key("subtitle"),
            Step::Key("runs"),
            Step::Index(0),
            Step::Key("text"),
        ],
    )
    .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
    .unwrap_or("")
    .to_owned()
}

/// description text: the description shelf is absent in the minimal artist
/// response, so this returns "". (Parity with `api.py`'s `description or ""`.)
fn description_text(_sections: &[Value]) -> String {
    String::new()
}

/// `["title", "runs", 0, "navigationEndpoint", "browseEndpoint", "browseId"]` —
/// the album/artist card's browse id (ytmusicapi's `TITLE + NAVIGATION_BROWSE_ID`).
const TITLE_RUN0_BROWSE_ID: &[Step] = &[
    Step::Key("title"),
    Step::Key("runs"),
    Step::Index(0),
    Step::Key("navigationEndpoint"),
    Step::Key("browseEndpoint"),
    Step::Key("browseId"),
];
