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
mod playlist;
mod search;
mod songruns;

#[cfg(test)]
mod tests;

use serde_json::{Value, json};

use crate::client::InnerTubeClient;
use crate::error::ApiError;
use crate::models::{AlbumInfo, ArtistInfo, SearchResults, Track};
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

    /// Get album metadata and tracks. See [`get_album`].
    pub async fn get_album(&self, browse_id: &str) -> Result<AlbumInfo, ApiError> {
        get_album(self, browse_id).await
    }

    /// Get an artist page. See [`get_artist`].
    pub async fn get_artist(&self, channel_id: &str) -> Result<ArtistInfo, ApiError> {
        get_artist(self, channel_id).await
    }
}
