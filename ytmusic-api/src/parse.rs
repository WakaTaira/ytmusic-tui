//! Pure conversion functions: raw ytmusicapi JSON → typed domain models.
//!
//! This is a 1-to-1 port of the helper functions in `src/ytmusic_tui/api.py`:
//! `parse_duration`, `_pick_largest_thumbnail`, `_join_artists`,
//! `_extract_duration`, `_dict_to_track`, `_dict_to_album_track`,
//! `_watch_item_to_track`, `_dict_to_album_info`, `_dict_to_related_artist`,
//! `_dict_to_playlist_info`.
//!
//! All functions take a `&serde_json::Value` (the raw ytmusicapi payload) and
//! return `Option<T>` where the Python returns `T | None`, or `T` with the
//! same fallback semantics (empty string / 0.0 / 0 on missing fields).
//!
//! # Porting seam
//!
//! The raw `dict_to_*` helpers below are the porting seam that mirrors
//! `api.py`'s private `_dict_to_*` functions. They are intentionally `pub` so
//! that fixture-driven unit tests (in `tests/parse_tests.rs`) can drive them
//! directly without going through the HTTP client. The stable consumer surface
//! will be the typed client methods added in M3d; callers should prefer those
//! over calling these helpers directly.

use serde_json::Value;

use crate::models::{
    AlbumInfo, HomeSection, HomeSectionItem, PlaylistInfo, RelatedArtist, SearchResults, Track,
};

// ---------------------------------------------------------------------------
// parse_duration — public, mirrors the Python top-level helper
// ---------------------------------------------------------------------------

/// Parse a duration string into seconds.
///
/// Accepts "M:SS", "H:MM:SS", or "SS" formats.
/// Returns `0.0` for `None`, empty string, unparseable input, or arithmetic
/// overflow (mirrors Python's `ValueError → 0.0` contract; overflow cannot
/// occur in Python because Python integers are unbounded, but absurdly large
/// hour counts would produce an astronomically large float — we return `0.0`
/// on overflow to keep the contract simple and consistent).
///
/// Python equivalent: `parse_duration(raw: str | None) -> float`.
pub fn parse_duration(raw: Option<&str>) -> f64 {
    let s = match raw {
        Some(s) if !s.is_empty() => s,
        _ => return 0.0,
    };

    let parts: Vec<&str> = s.split(':').collect();

    // Every segment must be a valid integer (Python raises ValueError otherwise → 0.0)
    let int_parts: Vec<i64> = match parts.iter().map(|p| p.parse::<i64>()).collect() {
        Ok(v) => v,
        Err(_) => return 0.0,
    };

    // Use checked arithmetic so absurd inputs (e.g. "9999999999:00:00") do not
    // silently wrap; any overflow → 0.0.
    let seconds: Option<i64> = match int_parts.as_slice() {
        [sec] => Some(*sec),
        [min, sec] => min.checked_mul(60).and_then(|m| m.checked_add(*sec)),
        [hr, min, sec] => hr
            .checked_mul(3600)
            .and_then(|h| min.checked_mul(60).and_then(|m| h.checked_add(m)))
            .and_then(|hm| hm.checked_add(*sec)),
        _ => None,
    };

    match seconds {
        Some(s) => s as f64,
        None => 0.0,
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Return the URL of the largest thumbnail (by width), or `""`.
///
/// Python equivalent: `_pick_largest_thumbnail`.
/// Selects the thumbnail with the highest `width` value. If `thumbnails` is
/// absent, null, or an empty array, returns an empty string.
pub(crate) fn pick_largest_thumbnail(thumbnails: &Value) -> String {
    let arr = match thumbnails.as_array() {
        Some(a) if !a.is_empty() => a,
        _ => return String::new(),
    };

    let best = arr
        .iter()
        .max_by_key(|t| t.get("width").and_then(Value::as_u64).unwrap_or(0));

    best.and_then(|t| t.get("url"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned()
}

/// Join artist names with `", "`.
///
/// Python equivalent: `_join_artists`.
/// Returns `""` if the field is absent, null, or an empty array.
pub(crate) fn join_artists(artists: &Value) -> String {
    let arr = match artists.as_array() {
        Some(a) if !a.is_empty() => a,
        _ => return String::new(),
    };

    arr.iter()
        .map(|a| a.get("name").and_then(Value::as_str).unwrap_or(""))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Extract duration in seconds from an API item.
///
/// Prefers the numeric `duration_seconds` field, falls back to parsing the
/// `duration` string.
///
/// Python equivalent: `_extract_duration`.
pub(crate) fn extract_duration(item: &Value) -> f64 {
    // Prefer numeric duration_seconds when present and coercible to f64
    if let Some(sec) = item.get("duration_seconds") {
        if let Some(n) = sec.as_f64() {
            return n;
        }
        // String representation (e.g. "213") — try parsing as i64 first
        if let Some(s) = sec.as_str()
            && let Ok(n) = s.parse::<f64>()
        {
            return n;
        }
    }
    parse_duration(item.get("duration").and_then(Value::as_str))
}

// ---------------------------------------------------------------------------
// Public conversion functions
// ---------------------------------------------------------------------------

/// Convert a raw ytmusicapi song/video dict into a [`Track`].
///
/// Returns `None` if `videoId` is absent or null (skip unavailable tracks).
///
/// Python equivalent: `_dict_to_track`.
pub fn dict_to_track(item: &Value) -> Option<Track> {
    let video_id = item.get("videoId").and_then(Value::as_str)?;
    if video_id.is_empty() {
        return None;
    }

    let album_name = item
        .get("album")
        .and_then(|a| a.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("");

    Some(Track::new(
        video_id,
        item.get("title").and_then(Value::as_str).unwrap_or(""),
        join_artists(item.get("artists").unwrap_or(&Value::Null)),
        album_name,
        extract_duration(item),
        pick_largest_thumbnail(item.get("thumbnails").unwrap_or(&Value::Null)),
    ))
}

/// Convert a raw album-track dict into a [`Track`].
///
/// Album tracks from `get_album()` may omit `artists`; `album_artist` is used
/// as the fallback (inherited from the album-level artist field).
///
/// Python equivalent: `_dict_to_album_track`.
pub fn dict_to_album_track(item: &Value, album_artist: &str) -> Option<Track> {
    let video_id = item.get("videoId").and_then(Value::as_str)?;
    if video_id.is_empty() {
        return None;
    }

    let album_name = item
        .get("album")
        .and_then(|a| a.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("");

    let artist_joined = join_artists(item.get("artists").unwrap_or(&Value::Null));
    let artist = if artist_joined.is_empty() {
        album_artist.to_owned()
    } else {
        artist_joined
    };

    Some(Track::new(
        video_id,
        item.get("title").and_then(Value::as_str).unwrap_or(""),
        artist,
        album_name,
        extract_duration(item),
        pick_largest_thumbnail(item.get("thumbnails").unwrap_or(&Value::Null)),
    ))
}

/// Convert a `get_watch_playlist` track dict into a [`Track`].
///
/// Watch-playlist items differ: duration is in `length` (string) and
/// thumbnails are under `thumbnail` (singular, not array).
///
/// Python equivalent: `_watch_item_to_track`.
pub fn watch_item_to_track(item: &Value) -> Option<Track> {
    let video_id = item.get("videoId").and_then(Value::as_str)?;
    if video_id.is_empty() {
        return None;
    }

    let album_name = item
        .get("album")
        .and_then(|a| a.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("");

    Some(Track::new(
        video_id,
        item.get("title").and_then(Value::as_str).unwrap_or(""),
        join_artists(item.get("artists").unwrap_or(&Value::Null)),
        album_name,
        // Watch items use "length" string, not duration_seconds
        parse_duration(item.get("length").and_then(Value::as_str)),
        // Watch items use singular "thumbnail", not "thumbnails"
        pick_largest_thumbnail(item.get("thumbnail").unwrap_or(&Value::Null)),
    ))
}

/// Convert a raw album dict (from artist page or library) into an [`AlbumInfo`].
///
/// Returns `None` if `browseId` is absent or null.
///
/// Python equivalent: `_dict_to_album_info`.
pub fn dict_to_album_info(item: &Value) -> Option<AlbumInfo> {
    let browse_id = item.get("browseId").and_then(Value::as_str)?;
    if browse_id.is_empty() {
        return None;
    }

    let year = item
        .get("year")
        .map(|y| {
            // year may be a string or integer in the API response
            if let Some(s) = y.as_str() {
                s.to_owned()
            } else if let Some(n) = y.as_u64() {
                n.to_string()
            } else {
                String::new()
            }
        })
        .unwrap_or_default();

    Some(AlbumInfo::new_without_tracks(
        browse_id,
        item.get("title").and_then(Value::as_str).unwrap_or(""),
        join_artists(item.get("artists").unwrap_or(&Value::Null)),
        year,
        pick_largest_thumbnail(item.get("thumbnails").unwrap_or(&Value::Null)),
    ))
}

/// Convert a raw artist dict from a `related` section into a [`RelatedArtist`].
///
/// Returns `None` if neither `browseId` nor `channelId` is present.
///
/// Python equivalent: `_dict_to_related_artist`.
/// Note: Python checks `browseId` OR `channelId`, and `title` OR `name`.
pub fn dict_to_related_artist(item: &Value) -> Option<RelatedArtist> {
    // Python: channel_id = item.get("browseId") or item.get("channelId")
    let channel_id = item
        .get("browseId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            item.get("channelId")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
        })?;

    // Python: name = item.get("title", "") or item.get("name", "")
    let name = item
        .get("title")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            item.get("name")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
        })
        .unwrap_or("");

    Some(RelatedArtist::new(
        channel_id,
        name,
        pick_largest_thumbnail(item.get("thumbnails").unwrap_or(&Value::Null)),
    ))
}

/// Convert a raw ytmusicapi dict into a [`PlaylistInfo`].
///
/// Returns `None` if `playlistId` is absent or null.
///
/// Python equivalent: `_dict_to_playlist_info`.
/// `count` can be an integer, a string like `"25"`, or absent (→ 0).
/// `description` falling back through `None` and `""` both produce `""`.
pub fn dict_to_playlist_info(item: &Value) -> Option<PlaylistInfo> {
    let playlist_id = item.get("playlistId").and_then(Value::as_str)?;
    if playlist_id.is_empty() {
        return None;
    }

    // Python: int(count_raw) if count_raw is not None else 0; ValueError → 0
    let track_count: u32 = item
        .get("count")
        .and_then(|c| {
            c.as_u64()
                .map(|n| n as u32)
                .or_else(|| c.as_str().and_then(|s| s.parse().ok()))
        })
        .unwrap_or(0);

    // Python: item.get("description", "") or ""  (treats None and "" equally)
    let description = item
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("");

    Some(PlaylistInfo::new(
        playlist_id,
        item.get("title").and_then(Value::as_str).unwrap_or(""),
        description,
        track_count,
        pick_largest_thumbnail(item.get("thumbnails").unwrap_or(&Value::Null)),
    ))
}

// ---------------------------------------------------------------------------
// Higher-level collection converters (used by the future client layer, M3d)
// ---------------------------------------------------------------------------

/// Categorize a mixed list of search-result items into a [`SearchResults`].
///
/// Mirrors the loop inside `MusicAPI.search_all()`: items are routed by
/// `resultType` ("song"/"video" → tracks; "album" → albums; "artist" →
/// artists; "playlist" → playlists). Unknown result types are silently ignored.
pub fn categorize_search_results(raw: &[Value]) -> SearchResults {
    let mut results = SearchResults::default();

    for item in raw {
        match item.get("resultType").and_then(Value::as_str) {
            Some("song") | Some("video") => {
                if let Some(t) = dict_to_track(item) {
                    results.tracks.push(t);
                }
            }
            Some("album") => {
                if let Some(a) = dict_to_album_info(item) {
                    results.albums.push(a);
                }
            }
            Some("artist") => {
                if let Some(a) = dict_to_related_artist(item) {
                    results.artists.push(a);
                }
            }
            Some("playlist") => {
                if let Some(p) = dict_to_playlist_info(item) {
                    results.playlists.push(p);
                }
            }
            _ => {}
        }
    }

    results
}

/// Parse a list of raw ytmusicapi `get_home()` sections into typed [`HomeSection`]s.
///
/// Mirrors `MusicAPI.get_home()`. Three item branches (same as Python):
/// 1. `resultType == "playlist"` OR (`playlistId` present AND `videoId` absent)
///    → [`PlaylistInfo`] via `dict_to_playlist_info`.
/// 2. `browseId` present AND `videoId` absent (album-like: album shelf item)
///    → `dict_to_album_info`, then wrapped as `PlaylistInfo` with
///    `playlist_id = audioPlaylistId` (falling back to `album.browse_id` when absent).
/// 3. Otherwise → [`Track`] via `dict_to_track`.
///
/// # Empty-section contract
///
/// Sections whose `contents` key is `null` or missing are **skipped entirely**
/// (Python parity: `if contents is None: continue`).
/// Sections whose items ALL fail to parse are kept with an empty `items` vec
/// (Python parity: `HomeSection` is appended unconditionally after the loop,
/// even when all items were skipped — the TUI layer decides how to render
/// empty sections).
pub fn parse_home_sections(raw: &[Value]) -> Vec<HomeSection> {
    raw.iter()
        .filter_map(|section| {
            let contents = section.get("contents")?.as_array()?;
            let title = section.get("title").and_then(Value::as_str).unwrap_or("");

            let items: Vec<HomeSectionItem> = contents
                .iter()
                .filter_map(|item| {
                    let result_type = item.get("resultType").and_then(Value::as_str).unwrap_or("");
                    let has_playlist_id = item.get("playlistId").is_some();
                    let has_video_id = item.get("videoId").is_some();
                    let has_browse_id = item.get("browseId").is_some();

                    if result_type == "playlist" || (has_playlist_id && !has_video_id) {
                        dict_to_playlist_info(item).map(HomeSectionItem::Playlist)
                    } else if has_browse_id && !has_video_id {
                        // Album-like item: browseId + audioPlaylistId, no videoId
                        let album = dict_to_album_info(item)?;
                        // Issue #29: many home album cards arrive with
                        // `audioPlaylistId: ""` (empty string, not absent).
                        // `Option::unwrap_or` only falls back on `None`, so
                        // `Some("")` slipped through and produced
                        // `PlaylistInfo { playlist_id: "" }`, which seeded the
                        // nav stack with an empty id and broke every downstream
                        // action ("no playlist context" on Remove from playlist
                        // was the visible symptom). Treat `Some("")` like
                        // `None` so the `album.browse_id` fallback engages —
                        // `dict_to_album_info` already rejects empty
                        // `browse_id` upstream, so the resolved id is always
                        // non-empty by the time we get here.
                        let playlist_id = item
                            .get("audioPlaylistId")
                            .and_then(Value::as_str)
                            .filter(|s| !s.is_empty())
                            .unwrap_or(&album.browse_id)
                            .to_owned();
                        Some(HomeSectionItem::Playlist(PlaylistInfo::new(
                            playlist_id,
                            &album.title,
                            &album.artist,
                            0,
                            &album.thumbnail_url,
                        )))
                    } else {
                        dict_to_track(item).map(HomeSectionItem::Track)
                    }
                })
                .collect();

            Some(HomeSection {
                title: title.to_owned(),
                items,
            })
        })
        .collect()
}
