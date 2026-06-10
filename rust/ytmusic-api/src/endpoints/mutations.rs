//! Stage-1 flows for the mutation endpoints.
//!
//! All five mutation flows mirror the corresponding `api.py` methods and the
//! ytmusicapi mixins they delegate to (`mixins/library.py` for `rate_track`;
//! `mixins/playlists.py` for the playlist mutations).
//!
//! # Body shapes (from ytmusicapi source)
//!
//! | Endpoint              | Body keys                                        |
//! |-----------------------|--------------------------------------------------|
//! | `like/like`           | `{target: {videoId}}`                            |
//! | `like/dislike`        | `{target: {videoId}}`                            |
//! | `like/removelike`     | `{target: {videoId}}`                            |
//! | `playlist/create`     | `{title, description, privacyStatus}`            |
//! | `browse/edit_playlist`| `{playlistId, actions:[...]}`                    |
//!   — add items:        `actions: [{action:"ACTION_ADD_VIDEO", addedVideoId}]` |
//!   — remove items:     `actions: [{action:"ACTION_REMOVE_VIDEO", setVideoId, removedVideoId}]` |
//!
//! # Success predicates (from api.py)
//!
//! * `rate_track`: no logical-failure dimension; transport/auth errors propagate.
//! * `create_playlist`: response must contain `"playlistId"` key (string).
//! * `add_playlist_items`: `response["status"]` must contain `"SUCCEEDED"`.
//! * `remove_playlist_items`: same as add; plus at least one `setVideoId` must
//!   be resolved from the playlist items.
//!
//! # get_like_status flow
//!
//! Mirrors `api.py::get_like_status`: POST `next` (the watch-playlist endpoint)
//! with `limit=1`, then walk the returned tracks list looking for the exact
//! `videoId` match and reading its `likeStatus` key (threaded through
//! `radio::parse_watch_track` in M3d-3).

use serde_json::{Value, json};

use super::PostRequest;
use super::radio::parse_radio;
use crate::error::ApiError;

// ---------------------------------------------------------------------------
// rate_track
// ---------------------------------------------------------------------------

/// Rate a track ("thumbs up" / "thumbs down").
///
/// Mirrors `api.py::rate_track`, which calls `ytmusicapi::rate_song`.
///
/// `status` must be one of `"LIKE"`, `"INDIFFERENT"`, or `"DISLIKE"` —
/// the same strings `api.py` accepts. The endpoint is chosen via
/// `prepare_like_endpoint` (ytmusicapi `mixins/_utils.py`):
/// - `"LIKE"` → `like/like`
/// - `"DISLIKE"` → `like/dislike`
/// - `"INDIFFERENT"` → `like/removelike`
///
/// There is no logical-failure dimension: transport, auth, and HTTP errors
/// propagate as `ApiError` for the caller to classify. An unrecognised
/// `status` string is rejected with `ApiError::Parse`.
pub(crate) async fn rate_track(
    client: &impl PostRequest,
    video_id: &str,
    status: &str,
) -> Result<(), ApiError> {
    let endpoint = like_endpoint(status).ok_or_else(|| {
        ApiError::Parse(format!(
            "invalid rating '{status}'; expected LIKE, INDIFFERENT, or DISLIKE"
        ))
    })?;
    let body = json!({ "target": { "videoId": video_id } });
    client.post_request(endpoint, body).await?;
    Ok(())
}

/// Map a rating string to the InnerTube endpoint, mirroring
/// `prepare_like_endpoint` in ytmusicapi `mixins/_utils.py`.
fn like_endpoint(status: &str) -> Option<&'static str> {
    match status {
        "LIKE" => Some("like/like"),
        "DISLIKE" => Some("like/dislike"),
        "INDIFFERENT" => Some("like/removelike"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// get_like_status
// ---------------------------------------------------------------------------

/// Return the like status (`"LIKE"`, `"INDIFFERENT"`, etc.) of a track, or
/// `None` when the status cannot be determined.
///
/// Mirrors `api.py::get_like_status`: POST `next` (the watch-playlist body
/// with `limit=1`, no radio params so it returns the ATV watch panel), walk
/// the `tracks` list for the exact `videoId` match, read `likeStatus`.
///
/// `None` is returned when:
/// - The watch playlist returns no tracks.
/// - No track in the list matches `video_id`.
/// - The matching track has a `null` / absent `likeStatus`.
///
/// Transport / auth errors propagate as `ApiError`.
pub(crate) async fn get_like_status(
    client: &impl PostRequest,
    video_id: &str,
) -> Result<Option<String>, ApiError> {
    // api.py: `self._client.get_watch_playlist(video_id, limit=1)`
    // Non-radio body (same as get_lyrics): no `params`, includes the ATV
    // watchEndpointMusicConfig so YTM returns the full persistent panel.
    let body = json!({
        "enablePersistentPlaylistPanel": true,
        "isAudioOnly": true,
        "tunerSettingValue": "AUTOMIX_SETTING_NORMAL",
        "videoId": video_id,
        "playlistId": format!("RDAMVM{video_id}"),
        "watchEndpointMusicSupportedConfigs": {
            "watchEndpointMusicConfig": {
                "hasPersistentPlaylistPanel": true,
                "musicVideoType": "MUSIC_VIDEO_TYPE_ATV",
            }
        },
    });
    let response = client.post_request("next", body).await?;

    // parse_radio walks the same playlistPanelRenderer contents as
    // get_watch_playlist; it now emits `likeStatus` for each item.
    let raw_tracks = parse_radio(&response);

    // api.py: `for item in watch.get("tracks") or []: if item.get("videoId") == video_id`
    for raw in &raw_tracks {
        if raw.get("videoId").and_then(Value::as_str) == Some(video_id) {
            let status = raw.get("likeStatus").and_then(Value::as_str);
            return Ok(status.map(str::to_owned));
        }
    }

    // videoId was not in the returned panel (e.g. the seed was redirected).
    Ok(None)
}

// ---------------------------------------------------------------------------
// create_playlist
// ---------------------------------------------------------------------------

/// Create a new playlist and return its ID.
///
/// Mirrors `api.py::create_playlist`, which delegates to ytmusicapi
/// `create_playlist(title, description, privacy_status=privacy)`.
///
/// The InnerTube endpoint is `playlist/create`; the request body is
/// `{title, description, privacyStatus}`.
///
/// Succeeds when `response["playlistId"]` is a non-empty string.
/// Fails with `ApiError::MutationFailed("Playlist was not created")` when the
/// key is absent — mirroring api.py's `raise MutationFailedError("Playlist was
/// not created")`.
pub(crate) async fn create_playlist(
    client: &impl PostRequest,
    title: &str,
    description: &str,
    privacy: &str,
) -> Result<String, ApiError> {
    let body = json!({
        "title": title,
        "description": description,
        "privacyStatus": privacy,
    });
    let response = client.post_request("playlist/create", body).await?;
    match response.get("playlistId").and_then(Value::as_str) {
        Some(id) if !id.is_empty() => Ok(id.to_owned()),
        _ => Err(ApiError::MutationFailed(
            "Playlist was not created".to_owned(),
        )),
    }
}

// ---------------------------------------------------------------------------
// add_playlist_items
// ---------------------------------------------------------------------------

/// Add tracks to an existing playlist.
///
/// Mirrors `api.py::add_playlist_items`, which calls ytmusicapi
/// `add_playlist_items(playlist_id, video_ids)`.
///
/// The InnerTube endpoint is `browse/edit_playlist`; the request body is
/// `{playlistId, actions: [{action: "ACTION_ADD_VIDEO", addedVideoId}…]}`.
///
/// The success predicate mirrors api.py: `response["status"]` must contain
/// the substring `"SUCCEEDED"` (ytmusicapi checks `"SUCCEEDED" in status`).
/// On failure → `ApiError::MutationFailed("Tracks were not added to the
/// playlist")`.
pub(crate) async fn add_playlist_items(
    client: &impl PostRequest,
    playlist_id: &str,
    video_ids: &[String],
) -> Result<(), ApiError> {
    let actions: Vec<Value> = video_ids
        .iter()
        .map(|vid| json!({ "action": "ACTION_ADD_VIDEO", "addedVideoId": vid }))
        .collect();
    let body = json!({
        "playlistId": strip_vl(playlist_id),
        "actions": actions,
    });
    let response = client.post_request("browse/edit_playlist", body).await?;
    if succeeded(&response) {
        return Ok(());
    }
    Err(ApiError::MutationFailed(
        "Tracks were not added to the playlist".to_owned(),
    ))
}

// ---------------------------------------------------------------------------
// remove_playlist_items
// ---------------------------------------------------------------------------

/// Remove tracks from a playlist.
///
/// Two-step flow mirroring `api.py::remove_playlist_items`:
///
/// 1. Fetch the playlist via `browse/VL{playlist_id}` and resolve each
///    `video_id` to its `setVideoId` (the unique per-item token the remove
///    endpoint requires).
/// 2. POST `browse/edit_playlist` with `ACTION_REMOVE_VIDEO` actions carrying
///    both `setVideoId` and `removedVideoId`.
///
/// `setVideoId` comes from the stage-1 playlist parser (threaded through
/// `stage1::parse_playlist_item`'s menu scan in M3d-3).
///
/// Fails with `ApiError::MutationFailed("Track was not found in the playlist")`
/// when none of `video_ids` map to a `setVideoId` in the fetched playlist —
/// mirrors api.py's `raise MutationFailedError("Track was not found in the
/// playlist")`.
///
/// Fails with `ApiError::MutationFailed("Tracks were not removed from the
/// playlist")` when the edit endpoint returns a non-succeeded status.
pub(crate) async fn remove_playlist_items(
    client: &impl PostRequest,
    playlist_id: &str,
    video_ids: &[String],
) -> Result<(), ApiError> {
    use super::playlist::parse_playlist_tracks;

    // Step 1: fetch playlist and resolve videoId → setVideoId.
    let browse_id = if playlist_id.starts_with("VL") {
        playlist_id.to_owned()
    } else {
        format!("VL{playlist_id}")
    };
    let playlist_response = client
        .post_request("browse", json!({ "browseId": browse_id }))
        .await?;

    let raw_tracks = parse_playlist_tracks(&playlist_response);
    let target_set: std::collections::HashSet<&str> =
        video_ids.iter().map(|s| s.as_str()).collect();

    let mut to_remove: Vec<Value> = Vec::new();
    let mut resolved: std::collections::HashSet<&str> = std::collections::HashSet::new();

    for raw in &raw_tracks {
        let vid = raw.get("videoId").and_then(Value::as_str).unwrap_or("");
        if target_set.contains(vid)
            && !resolved.contains(vid)
            && let Some(set_vid) = raw.get("setVideoId").and_then(Value::as_str)
        {
            to_remove.push(json!({
                "setVideoId": set_vid,
                "removedVideoId": vid,
                "action": "ACTION_REMOVE_VIDEO",
            }));
            resolved.insert(vid);
        }
        if resolved.len() == target_set.len() {
            break;
        }
    }

    if to_remove.is_empty() {
        return Err(ApiError::MutationFailed(
            "Track was not found in the playlist".to_owned(),
        ));
    }

    // Step 2: send the remove edit.
    let body = json!({
        "playlistId": strip_vl(playlist_id),
        "actions": to_remove,
    });
    let response = client.post_request("browse/edit_playlist", body).await?;
    if succeeded(&response) {
        return Ok(());
    }
    Err(ApiError::MutationFailed(
        "Tracks were not removed from the playlist".to_owned(),
    ))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Strip a leading `"VL"` prefix from a playlist ID (ytmusicapi's
/// `validate_playlist_id`): `browse/edit_playlist` wants the bare ID.
fn strip_vl(playlist_id: &str) -> &str {
    playlist_id.strip_prefix("VL").unwrap_or(playlist_id)
}

/// Success predicate for playlist edit responses.
///
/// Mirrors api.py / ytmusicapi: a response `status` field that contains the
/// substring `"SUCCEEDED"` is treated as success. Both `str` and dict shapes
/// are handled (ytmusicapi returns either depending on the endpoint version).
fn succeeded(response: &Value) -> bool {
    match response.get("status").and_then(Value::as_str) {
        Some(s) => s.contains("SUCCEEDED"),
        None => false,
    }
}
