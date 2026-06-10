//! The typed endpoint seam: raw InnerTube JSON → ytmusicapi-shaped `Value`
//! (stage 1) → [`crate::parse`] converters (stage 2) → domain models.
//!
//! # Architecture
//!
//! Each endpoint is a two-stage pipeline:
//!
//! 1. **Stage 1** (this module's submodules): a pure function over the raw
//!    InnerTube `serde_json::Value` that reproduces the relevant slice of the
//!    matching `ytmusicapi` parser, emitting a `Value` shaped exactly like
//!    `ytmusicapi`'s public return (only the fields stage 2 reads).
//! 2. **Stage 2** ([`crate::parse`], committed in M3c): the existing
//!    `dict_to_*` / `categorize_*` converters that turn that ytmusicapi-shaped
//!    `Value` into domain types.
//!
//! # The transport seam
//!
//! Flow functions depend on the [`PostRequest`] trait, not on
//! [`InnerTubeClient`] directly. This lets endpoint-flow tests drive the full
//! pipeline against captured fixtures without HTTP (see the fixture tests in
//! `tests/`). [`InnerTubeClient`] implements [`PostRequest`] by delegating to
//! its real signed `post`; the public methods on [`InnerTubeClient`] (added at
//! the bottom of this module) are thin wrappers over the flow functions.
//!
//! M3d-2 (home / library / liked / history / radio / lyrics) and M3d-3
//! (mutations) extend the same seam: add a stage-1 submodule and a flow
//! function, then expose a wrapper method.

mod album;
mod artist;
mod history;
mod home;
mod library;
mod lyrics;
mod mutations;
mod playlist;
mod radio;
mod search;
mod songruns;
mod stage1;

#[cfg(test)]
mod tests;

use serde_json::{Value, json};

use crate::client::InnerTubeClient;
use crate::error::ApiError;
use crate::models::{AlbumInfo, ArtistInfo, HomeSection, PlaylistInfo, SearchResults, Track};
use crate::parse;

/// The transport abstraction the endpoint flows depend on.
///
/// One method: POST a body to a `youtubei/v1` endpoint and return the parsed
/// JSON. [`InnerTubeClient`] implements it with its signed transport; tests
/// implement it with a fixture-returning fake.
///
/// `Send + Sync` supertraits: the TUI (M5) shares one client across spawned
/// tokio tasks; constraining the trait here surfaces a non-Send impl at its
/// definition instead of at a distant spawn site.
pub(crate) trait PostRequest: Send + Sync {
    /// POST `body` to `endpoint` (e.g. `"search"`, `"browse"`) and return the
    /// parsed response, or an [`ApiError`] on transport / HTTP / parse failure.
    fn post_request(
        &self,
        endpoint: &str,
        body: Value,
    ) -> impl std::future::Future<Output = Result<Value, ApiError>> + Send;
}

impl PostRequest for InnerTubeClient {
    async fn post_request(&self, endpoint: &str, body: Value) -> Result<Value, ApiError> {
        self.post(endpoint, body).await
    }
}

// ---------------------------------------------------------------------------
// Flow functions (transport-agnostic)
// ---------------------------------------------------------------------------

/// Search across all categories and return categorized results.
///
/// Mirrors `api.py::search_all`: POST `search` with `{query, [params]}`, run
/// the stage-1 search parser, then categorize via stage 2.
///
/// `filter` is passed through to ytmusicapi's category restriction
/// (`"songs"`, `"albums"`, `"artists"`, `"playlists"`); `None` is a default
/// search across all types.
pub(crate) async fn search_all(
    client: &impl PostRequest,
    query: &str,
    limit: usize,
    filter: Option<&str>,
) -> Result<SearchResults, ApiError> {
    let mut body = json!({ "query": query });
    if let Some(params) = search_params(filter) {
        body["params"] = Value::String(params.to_owned());
    }
    // `limit` shapes continuation depth in ytmusicapi; the trimmed single-page
    // contract here returns the first shelf. Bound the result to `limit` to keep
    // the wrapper's contract (api.py passes limit straight through).
    let response = client.post_request("search", body).await?;
    let raw_items = search::parse_search_response(&response, filter);
    let mut results = parse::categorize_search_results(&raw_items);
    truncate_results(&mut results, limit);
    Ok(results)
}

/// Truncate each category of a [`SearchResults`] to at most `limit` items.
///
/// ytmusicapi enforces `limit` via continuation paging; on a single trimmed
/// page we simply cap each list so the wrapper never returns more than asked.
fn truncate_results(results: &mut SearchResults, limit: usize) {
    results.tracks.truncate(limit);
    results.albums.truncate(limit);
    results.artists.truncate(limit);
    results.playlists.truncate(limit);
}

/// Get all tracks in a playlist.
///
/// Mirrors `api.py::get_playlist_tracks`: prefix the id with `VL` (unless
/// already prefixed), POST `browse`, walk the track shelf (stage 1), and
/// convert each via stage 2's `dict_to_track`, skipping unavailable items.
pub(crate) async fn get_playlist_tracks(
    client: &impl PostRequest,
    playlist_id: &str,
) -> Result<Vec<Track>, ApiError> {
    let browse_id = if playlist_id.starts_with("VL") {
        playlist_id.to_owned()
    } else {
        format!("VL{playlist_id}")
    };
    let response = client
        .post_request("browse", json!({ "browseId": browse_id }))
        .await?;
    let raw_tracks = playlist::parse_playlist_tracks(&response);
    Ok(raw_tracks.iter().filter_map(parse::dict_to_track).collect())
}

/// Get the user's liked ("thumbs up") songs.
///
/// Mirrors `api.py::get_liked_songs`, which delegates to ytmusicapi's
/// `get_liked_songs(limit)` = `get_playlist("LM", limit)`. The `"LM"` playlist
/// uses the identical track-shelf layout as any playlist, so this reuses the
/// playlist machinery verbatim (the flow prefixes `"LM"` with `VL` → `"VLLM"`).
///
/// `limit` bounds the returned list to match the wrapper contract (ytmusicapi
/// pages to `limit`; the trimmed single page is capped here).
pub(crate) async fn get_liked_songs(
    client: &impl PostRequest,
    limit: usize,
) -> Result<Vec<Track>, ApiError> {
    let mut tracks = get_playlist_tracks(client, "LM").await?;
    tracks.truncate(limit);
    Ok(tracks)
}

/// Get the user's library playlists.
///
/// Mirrors `api.py::get_library_playlists`: POST `browse FEmusic_liked_playlists`,
/// run the stage-1 GRID parser (skipping the "New playlist" pseudo-item), then
/// convert via `dict_to_playlist_info`. `limit` caps the trimmed single page.
pub(crate) async fn get_library_playlists(
    client: &impl PostRequest,
    limit: usize,
) -> Result<Vec<PlaylistInfo>, ApiError> {
    let response = client
        .post_request("browse", json!({ "browseId": "FEmusic_liked_playlists" }))
        .await?;
    let mut playlists = library::parse_library_playlists(&response);
    playlists.truncate(limit);
    Ok(playlists)
}

/// Get the user's library albums.
///
/// Mirrors `api.py::get_library_albums`: POST `browse FEmusic_liked_albums`, run
/// the stage-1 GRID album parser, then convert via `dict_to_album_info`.
pub(crate) async fn get_library_albums(
    client: &impl PostRequest,
    limit: usize,
) -> Result<Vec<AlbumInfo>, ApiError> {
    let response = client
        .post_request("browse", json!({ "browseId": "FEmusic_liked_albums" }))
        .await?;
    let mut albums = library::parse_library_albums(&response);
    albums.truncate(limit);
    Ok(albums)
}

/// Get the user's library artists.
///
/// Mirrors `api.py::get_library_artists`: POST
/// `browse FEmusic_library_corpus_track_artists`, run the stage-1 MUSIC_SHELF
/// parser, building [`ArtistInfo`]s straight from the `artist`/`browseId` row
/// keys (no `dict_to_related_artist` — gotcha M3d-2/4).
pub(crate) async fn get_library_artists(
    client: &impl PostRequest,
    limit: usize,
) -> Result<Vec<ArtistInfo>, ApiError> {
    let response = client
        .post_request(
            "browse",
            json!({ "browseId": "FEmusic_library_corpus_track_artists" }),
        )
        .await?;
    let mut artists = library::parse_library_artists(&response);
    artists.truncate(limit);
    Ok(artists)
}

/// Get the home page recommendations as a list of titled sections.
///
/// Mirrors `api.py::get_home`: POST `browse FEmusic_home`, run the stage-1
/// `parse_mixed_content` port to build the raw `[{title, contents}]` sections,
/// then convert via `parse::parse_home_sections` (which routes each item to a
/// [`Track`] or [`PlaylistInfo`] by its `videoId`/`playlistId`/`browseId` shape).
pub(crate) async fn get_home(client: &impl PostRequest) -> Result<Vec<HomeSection>, ApiError> {
    let response = client
        .post_request("browse", json!({ "browseId": "FEmusic_home" }))
        .await?;
    let raw_sections = home::parse_home(&response);
    Ok(parse::parse_home_sections(&raw_sections))
}

/// Get the play history (newest first), flattened across dated shelves.
///
/// Mirrors `api.py::get_history`: POST `browse FEmusic_history`, flatten the
/// dated `musicShelfRenderer` shelves into one list (stage 1), then convert each
/// via `dict_to_track`, skipping unavailable items. (`api.py` ignores the
/// per-item `played` shelf label, so the flow does too.) No `limit`: history
/// returns whatever the single page holds, matching the Python wrapper.
pub(crate) async fn get_history(client: &impl PostRequest) -> Result<Vec<Track>, ApiError> {
    let response = client
        .post_request("browse", json!({ "browseId": "FEmusic_history" }))
        .await?;
    let raw_tracks = history::parse_history(&response);
    Ok(raw_tracks.iter().filter_map(parse::dict_to_track).collect())
}

/// Get album metadata and tracks.
///
/// Mirrors `api.py::get_album`: POST `browse` with the `MPRE` browse id, run
/// the stage-1 album parser (header + post-processed tracks), then build
/// [`AlbumInfo`] with `dict_to_album_track` (album-artist fallback).
pub(crate) async fn get_album(
    client: &impl PostRequest,
    browse_id: &str,
) -> Result<AlbumInfo, ApiError> {
    let response = client
        .post_request("browse", json!({ "browseId": browse_id }))
        .await?;
    Ok(album::parse_album(&response, browse_id))
}

/// Get an artist page: top songs, albums, and related artists.
///
/// Mirrors `api.py::get_artist`: POST `browse` with the channel id, run the
/// stage-1 artist parser, then assemble [`ArtistInfo`]. The returned
/// `channel_id` is the *input* id (matching api.py, which ignores the parsed
/// `subscriptionButton.channelId`).
pub(crate) async fn get_artist(
    client: &impl PostRequest,
    channel_id: &str,
) -> Result<ArtistInfo, ApiError> {
    // ytmusicapi strips a leading "MPLA" before requesting.
    let request_id = channel_id.strip_prefix("MPLA").unwrap_or(channel_id);
    let response = client
        .post_request("browse", json!({ "browseId": request_id }))
        .await?;
    Ok(artist::parse_artist(&response, channel_id))
}

/// Get a radio queue seeded by `video_id` (the seed track first).
///
/// Mirrors `api.py::get_radio`, which calls ytmusicapi's
/// `get_watch_playlist(videoId, radio=True, limit)`: POST `next` with the
/// persistent-panel body and the `"wAEB"` radio params, walk the
/// `playlistPanelRenderer` queue (stage 1), then convert each via
/// `watch_item_to_track`. `limit` caps the trimmed single page.
pub(crate) async fn get_radio(
    client: &impl PostRequest,
    video_id: &str,
    limit: usize,
) -> Result<Vec<Track>, ApiError> {
    // ytmusicapi defaults playlistId to "RDAMVM" + videoId for a video seed and
    // sets params="wAEB" when radio=True. The persistent-panel flags match
    // `get_watch_playlist`'s base body.
    let body = json!({
        "enablePersistentPlaylistPanel": true,
        "isAudioOnly": true,
        "tunerSettingValue": "AUTOMIX_SETTING_NORMAL",
        "videoId": video_id,
        "playlistId": format!("RDAMVM{video_id}"),
        "params": "wAEB",
    });
    let response = client.post_request("next", body).await?;
    let raw_tracks = radio::parse_radio(&response);
    let mut tracks: Vec<Track> = raw_tracks
        .iter()
        .filter_map(parse::watch_item_to_track)
        .collect();
    tracks.truncate(limit);
    Ok(tracks)
}

/// Fetch a track's lyrics, or `None` when the track has no lyrics.
///
/// Mirrors `api.py::get_lyrics`: first `get_watch_playlist(video_id)` (a `next`
/// request) to read the lyrics browse id from the watch panel's tabs, then — only
/// if present — a `browse` for the lyrics text.
///
/// `None` means exactly "this track has no lyrics" — a value, not an error
/// (battle lesson). Transport / HTTP failures still propagate as [`ApiError`] so
/// callers can tell "no lyrics" from "could not reach YouTube Music".
pub(crate) async fn get_lyrics(
    client: &impl PostRequest,
    video_id: &str,
) -> Result<Option<String>, ApiError> {
    // get_watch_playlist(video_id) base body: persistent panel + the ATV music
    // config (non-radio) + the default RDAMVM playlist seed.
    let watch_body = json!({
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
    let watch = client.post_request("next", watch_body).await?;
    let Some(browse_id) = lyrics::lyrics_browse_id(&watch).map(str::to_owned) else {
        return Ok(None); // no lyrics tab → the track simply has no lyrics
    };

    let lyrics_response = client
        .post_request("browse", json!({ "browseId": browse_id }))
        .await?;
    Ok(lyrics::lyrics_text(&lyrics_response))
}

/// Rate a track (thumbs up / down / remove).
///
/// Mirrors `api.py::rate_track`: `status` must be `"LIKE"`, `"INDIFFERENT"`,
/// or `"DISLIKE"`. Returns `Ok(())` on success; transport / auth / parse errors
/// propagate as [`ApiError`].
pub(crate) async fn rate_track(
    client: &impl PostRequest,
    video_id: &str,
    status: &str,
) -> Result<(), ApiError> {
    mutations::rate_track(client, video_id, status).await
}

/// Return the like status of a track.
///
/// Mirrors `api.py::get_like_status`. Returns `None` when the status cannot
/// be determined (video not in the watch panel or `likeStatus` is null).
pub(crate) async fn get_like_status(
    client: &impl PostRequest,
    video_id: &str,
) -> Result<Option<String>, ApiError> {
    mutations::get_like_status(client, video_id).await
}

/// Create a new playlist and return its ID.
///
/// Mirrors `api.py::create_playlist`. `privacy` is one of `"PUBLIC"`,
/// `"PRIVATE"`, `"UNLISTED"`. Fails with
/// [`ApiError::MutationFailed`]`("Playlist was not created")` when the API
/// does not return a `playlistId`.
pub(crate) async fn create_playlist(
    client: &impl PostRequest,
    title: &str,
    description: &str,
    privacy: &str,
) -> Result<String, ApiError> {
    mutations::create_playlist(client, title, description, privacy).await
}

/// Add tracks to an existing playlist.
///
/// Mirrors `api.py::add_playlist_items`. Fails with
/// [`ApiError::MutationFailed`]`("Tracks were not added to the playlist")`
/// when the service does not confirm `STATUS_SUCCEEDED`.
pub(crate) async fn add_playlist_items(
    client: &impl PostRequest,
    playlist_id: &str,
    video_ids: &[String],
) -> Result<(), ApiError> {
    mutations::add_playlist_items(client, playlist_id, video_ids).await
}

/// Remove tracks from a playlist.
///
/// Two-step flow: fetch the playlist to resolve `setVideoId`s, then send the
/// remove actions. Fails with [`ApiError::MutationFailed`] when no target
/// `videoId` was found in the playlist or the service rejects the request.
pub(crate) async fn remove_playlist_items(
    client: &impl PostRequest,
    playlist_id: &str,
    video_ids: &[String],
) -> Result<(), ApiError> {
    mutations::remove_playlist_items(client, playlist_id, video_ids).await
}

/// Delete a playlist.
///
/// Mirrors `ytmusicapi.mixins.playlists.delete_playlist`: POST
/// `playlist/delete` with `{playlistId}` (VL prefix stripped).
///
/// Success predicate mirrors the Python source exactly:
/// - If the response has a `"status"` key, it must contain `"SUCCEEDED"`.
/// - If there is no `"status"` key, the full-response shape (which YouTube
///   returns for newer API versions as a `command` object) is treated as
///   success — matching Python's `return response["status"] if "status" in
///   response else response` (a truthy dict = success).
///
/// Fails with [`ApiError::MutationFailed`]`("Playlist was not deleted")`
/// only when `"status"` is present and does NOT contain `"SUCCEEDED"`.
///
/// Used primarily by the integration-test lifecycle cleanup; exposed here so
/// the TUI layer (M5) can offer a delete-playlist action.
pub(crate) async fn delete_playlist(
    client: &impl PostRequest,
    playlist_id: &str,
) -> Result<(), ApiError> {
    let bare_id = playlist_id.strip_prefix("VL").unwrap_or(playlist_id);
    let response = client
        .post_request("playlist/delete", json!({ "playlistId": bare_id }))
        .await?;
    match response.get("status").and_then(Value::as_str) {
        // Explicit SUCCEEDED status — always ok.
        Some(s) if s.contains("SUCCEEDED") => Ok(()),
        // No "status" key → newer command-object response shape; treat as ok.
        None => Ok(()),
        // "status" present but not SUCCEEDED → genuine failure.
        Some(_) => Err(ApiError::MutationFailed(
            "Playlist was not deleted".to_owned(),
        )),
    }
}

/// The InnerTube `params` blob for a search filter, mirroring the non-spelling
/// branch of `get_search_params` for the four routed filters.
///
/// `None` (default search) and unrecognized filters send no params. The values
/// are the fixed protobuf-encoded tokens ytmusicapi hardcodes; they are not
/// secrets. Traced to ytmusicapi's `get_search_params` (parsers/search.py) —
/// when a filtered search breaks after a protocol change, diff against it first.
fn search_params(filter: Option<&str>) -> Option<&'static str> {
    match filter? {
        "songs" => Some("EgWKAQIIAWoMEA4QChADEAQQCRAF"),
        // Pre-wired for a future "videos" filter; api.py's search_all does not
        // expose it (its surface is songs/albums/artists/playlists).
        "videos" => Some("EgWKAQIQAWoMEA4QChADEAQQCRAF"),
        "albums" => Some("EgWKAQIYAWoMEA4QChADEAQQCRAF"),
        "artists" => Some("EgWKAQIgAWoMEA4QChADEAQQCRAF"),
        "playlists" => Some("Eg-KAQwIABAAGAAgACgBMABqChAEEAMQCRAFEAo%3D"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Public client surface
// ---------------------------------------------------------------------------

impl InnerTubeClient {
    /// Search across all categories. See [`search_all`].
    pub async fn search_all(
        &self,
        query: &str,
        limit: usize,
        filter: Option<&str>,
    ) -> Result<SearchResults, ApiError> {
        search_all(self, query, limit, filter).await
    }

    /// Get all tracks in a playlist. See [`get_playlist_tracks`].
    pub async fn get_playlist_tracks(&self, playlist_id: &str) -> Result<Vec<Track>, ApiError> {
        get_playlist_tracks(self, playlist_id).await
    }

    /// Get the user's liked songs. See [`get_liked_songs`].
    pub async fn get_liked_songs(&self, limit: usize) -> Result<Vec<Track>, ApiError> {
        get_liked_songs(self, limit).await
    }

    /// Get the user's library playlists. See [`get_library_playlists`].
    pub async fn get_library_playlists(&self, limit: usize) -> Result<Vec<PlaylistInfo>, ApiError> {
        get_library_playlists(self, limit).await
    }

    /// Get the user's library albums. See [`get_library_albums`].
    pub async fn get_library_albums(&self, limit: usize) -> Result<Vec<AlbumInfo>, ApiError> {
        get_library_albums(self, limit).await
    }

    /// Get the user's library artists. See [`get_library_artists`].
    pub async fn get_library_artists(&self, limit: usize) -> Result<Vec<ArtistInfo>, ApiError> {
        get_library_artists(self, limit).await
    }

    /// Get the play history (newest first). See [`get_history`].
    ///
    /// Deliberately takes no `limit` (unlike the other list endpoints): the
    /// Python wrapper returns whatever the single history page holds.
    pub async fn get_history(&self) -> Result<Vec<Track>, ApiError> {
        get_history(self).await
    }

    /// Get the home page sections. See [`get_home`].
    pub async fn get_home(&self) -> Result<Vec<HomeSection>, ApiError> {
        get_home(self).await
    }

    /// Get a radio queue seeded by `video_id`. See [`get_radio`].
    pub async fn get_radio(&self, video_id: &str, limit: usize) -> Result<Vec<Track>, ApiError> {
        get_radio(self, video_id, limit).await
    }

    /// Fetch a track's lyrics (`None` = no lyrics). See [`get_lyrics`].
    pub async fn get_lyrics(&self, video_id: &str) -> Result<Option<String>, ApiError> {
        get_lyrics(self, video_id).await
    }

    /// Get album metadata and tracks. See [`get_album`].
    pub async fn get_album(&self, browse_id: &str) -> Result<AlbumInfo, ApiError> {
        get_album(self, browse_id).await
    }

    /// Get an artist page. See [`get_artist`].
    pub async fn get_artist(&self, channel_id: &str) -> Result<ArtistInfo, ApiError> {
        get_artist(self, channel_id).await
    }

    /// Rate a track. See [`rate_track`].
    pub async fn rate_track(&self, video_id: &str, status: &str) -> Result<(), ApiError> {
        rate_track(self, video_id, status).await
    }

    /// Get the like status of a track. See [`get_like_status`].
    pub async fn get_like_status(&self, video_id: &str) -> Result<Option<String>, ApiError> {
        get_like_status(self, video_id).await
    }

    /// Create a playlist and return its ID. See [`create_playlist`].
    pub async fn create_playlist(
        &self,
        title: &str,
        description: &str,
        privacy: &str,
    ) -> Result<String, ApiError> {
        create_playlist(self, title, description, privacy).await
    }

    /// Add tracks to a playlist. See [`add_playlist_items`].
    pub async fn add_playlist_items(
        &self,
        playlist_id: &str,
        video_ids: &[String],
    ) -> Result<(), ApiError> {
        add_playlist_items(self, playlist_id, video_ids).await
    }

    /// Remove tracks from a playlist. See [`remove_playlist_items`].
    pub async fn remove_playlist_items(
        &self,
        playlist_id: &str,
        video_ids: &[String],
    ) -> Result<(), ApiError> {
        remove_playlist_items(self, playlist_id, video_ids).await
    }

    /// Delete a playlist. See [`delete_playlist`].
    pub async fn delete_playlist(&self, playlist_id: &str) -> Result<(), ApiError> {
        delete_playlist(self, playlist_id).await
    }
}
