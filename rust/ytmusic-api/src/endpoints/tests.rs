//! Endpoint-flow tests: drive the full stage-1 → stage-2 → domain pipeline
//! against captured InnerTube fixtures, with a fake [`PostRequest`] that returns
//! the fixture instead of issuing HTTP.
//!
//! These are the M3c-DEFERRED "endpoint-flow" tests (the ones that mocked the
//! `YTMusic` client in `test_api.py`), now runnable because the client can
//! execute the real pipeline. Each expected value is the ground truth captured
//! by running the live Python pipeline (`api.py`) against the same raw fixture.

use std::sync::Mutex;

use serde_json::Value;

use super::{PostRequest, get_album, get_artist, get_playlist_tracks, search_all};
use crate::error::ApiError;

/// A fake transport that returns a fixed JSON response and records the last
/// `(endpoint, body)` it was asked to post — so flow tests can assert both the
/// parsed output and the request the flow built (params, VL prefix, ...).
struct FakePost {
    response: Value,
    last_call: Mutex<Option<(String, Value)>>,
}

impl FakePost {
    fn new(response: Value) -> Self {
        Self {
            response,
            last_call: Mutex::new(None),
        }
    }

    /// The `(endpoint, body)` of the most recent `post_request`.
    fn last(&self) -> (String, Value) {
        self.last_call
            .lock()
            .unwrap()
            .clone()
            .expect("a call was made")
    }
}

impl PostRequest for FakePost {
    async fn post_request(&self, endpoint: &str, body: Value) -> Result<Value, ApiError> {
        *self.last_call.lock().unwrap() = Some((endpoint.to_owned(), body));
        Ok(self.response.clone())
    }
}

/// A transport that always fails, to exercise error propagation through a flow.
struct FailingPost;

impl PostRequest for FailingPost {
    async fn post_request(&self, _endpoint: &str, _body: Value) -> Result<Value, ApiError> {
        Err(ApiError::Http {
            status: 500,
            message: "boom".to_owned(),
        })
    }
}

/// Load a raw InnerTube fixture from `tests/fixtures_innertube/<name>`.
fn fixture(name: &str) -> Value {
    let path = format!(
        "{}/tests/fixtures_innertube/{}",
        env!("CARGO_MANIFEST_DIR"),
        name
    );
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read fixture {name}: {e}"));
    serde_json::from_str(&content).unwrap_or_else(|e| panic!("failed to parse fixture {name}: {e}"))
}

/// Block on a future without pulling in `#[tokio::test]` for these synchronous
/// fakes (no real I/O happens, so a current-thread runtime is enough).
fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime builds")
        .block_on(fut)
}

// ---------------------------------------------------------------------------
// search — default (no filter)
// ---------------------------------------------------------------------------

#[test]
fn search_default_returns_expected_tracks() {
    let client = FakePost::new(fixture("search_default.json"));
    let results = block_on(search_all(&client, "lofi", 20, None)).expect("search ok");

    // Ground truth from the live api.py pipeline: 3 tracks (the videoId=null
    // "video" item is dropped by stage-2 dict_to_track), no albums/artists/
    // playlists in this default-search capture.
    assert_eq!(results.tracks.len(), 3);
    assert_eq!(results.albums.len(), 0);
    assert_eq!(results.artists.len(), 0);
    assert_eq!(results.playlists.len(), 0);

    let t0 = &results.tracks[0];
    assert_eq!(t0.video_id, "hVJcHYs2LgA");
    assert_eq!(t0.title, "Morning Coffee Lo-Fi Chillhop Beats for Focus");
    assert_eq!(t0.artist, "LO-FI BEATS, Lofi Chillhop");
    assert_eq!(t0.album, "");
    assert_eq!(t0.duration_seconds, 0.0);

    let t1 = &results.tracks[1];
    assert_eq!(t1.video_id, "tnlBctBzH1g");
    assert_eq!(t1.artist, "LO-FI BEATS, HIP-HOP LOFI, Lofi Anime");

    let t2 = &results.tracks[2];
    assert_eq!(t2.video_id, "ZSzwxyTuDw0");
    assert_eq!(t2.artist, "Lofi Jazz Terrace");

    // The default search sends no params and posts the `search` endpoint.
    let (endpoint, body) = client.last();
    assert_eq!(endpoint, "search");
    assert_eq!(body["query"], "lofi");
    assert!(
        body.get("params").is_none(),
        "default search sends no params"
    );
}

// ---------------------------------------------------------------------------
// search — songs filter
// ---------------------------------------------------------------------------

#[test]
fn search_songs_filter_returns_expected_tracks() {
    let client = FakePost::new(fixture("search_songs.json"));
    let results = block_on(search_all(&client, "lofi", 20, Some("songs"))).expect("search ok");

    assert_eq!(results.tracks.len(), 3);
    let expected = [
        (
            "BVDkTrlWPT0",
            "Suzume (Lo-Fi)",
            "Kei",
            "Suzume (Lo-Fi)",
            124.0,
        ),
        (
            "1MS2LCzrEdE",
            "You Know How We Do It",
            "Lofi Fruits Music, Chill Fruits Music",
            "90s Oldschool Lofi Hip Hop",
            131.0,
        ),
        (
            "iEKesTODIRY",
            "Japanese Lofi HipHop Mix",
            "Lofi Fruits Music, Chill Fruits Music",
            "Tokyo Early Morning Cafe",
            107.0,
        ),
    ];
    for (track, (vid, title, artist, album, dur)) in results.tracks.iter().zip(expected) {
        assert_eq!(track.video_id, vid);
        assert_eq!(track.title, title);
        assert_eq!(track.artist, artist);
        assert_eq!(track.album, album);
        assert_eq!(track.duration_seconds, dur);
    }

    // The songs filter sends the songs params blob.
    let (_, body) = client.last();
    assert_eq!(body["params"], "EgWKAQIIAWoMEA4QChADEAQQCRAF");
}

#[test]
fn search_limit_truncates_results() {
    let client = FakePost::new(fixture("search_songs.json"));
    let results = block_on(search_all(&client, "lofi", 2, Some("songs"))).expect("search ok");
    assert_eq!(results.tracks.len(), 2, "limit caps the track list");
}

#[test]
fn search_propagates_transport_error() {
    let err = block_on(search_all(&FailingPost, "lofi", 10, None)).expect_err("should fail");
    assert!(
        matches!(err, ApiError::Http { status: 500, .. }),
        "got: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// playlist tracks
// ---------------------------------------------------------------------------

#[test]
fn playlist_tracks_returns_expected() {
    let client = FakePost::new(fixture("playlist.json"));
    let tracks = block_on(get_playlist_tracks(&client, "PL_test")).expect("playlist ok");

    assert_eq!(tracks.len(), 3);
    assert_eq!(tracks[0].video_id, "7s4RmXxcZvM");
    assert_eq!(tracks[0].title, "「宝島」(吹奏楽) ピアノで弾きました");
    assert_eq!(tracks[0].artist, "Fujii Kaze");
    assert_eq!(tracks[0].album, "");
    assert_eq!(tracks[0].duration_seconds, 179.0);

    assert_eq!(tracks[1].video_id, "KwZFalJdsbQ");
    assert_eq!(tracks[1].artist, "dankeサン");

    assert_eq!(tracks[2].video_id, "hlr_7Za6v0M");
    assert_eq!(tracks[2].album, "Returns To Dreamland");
    assert_eq!(tracks[2].duration_seconds, 115.0);

    // The flow prefixes the playlist id with VL and posts `browse`.
    let (endpoint, body) = client.last();
    assert_eq!(endpoint, "browse");
    assert_eq!(body["browseId"], "VLPL_test");
}

#[test]
fn playlist_tracks_keeps_existing_vl_prefix() {
    let client = FakePost::new(fixture("playlist.json"));
    let _ = block_on(get_playlist_tracks(&client, "VLPL_already")).expect("ok");
    let (_, body) = client.last();
    assert_eq!(body["browseId"], "VLPL_already", "VL prefix not doubled");
}

// ---------------------------------------------------------------------------
// album
// ---------------------------------------------------------------------------

#[test]
fn album_returns_expected_info_and_tracks() {
    let client = FakePost::new(fixture("album.json"));
    let album = block_on(get_album(&client, "MPREb_test")).expect("album ok");

    // browse_id is the requested id (api.py passes it straight through).
    assert_eq!(album.browse_id, "MPREb_test");
    assert_eq!(album.title, "Morning Coffee Lo-Fi Chillhop Beats for Focus");
    assert_eq!(album.artist, "LO-FI BEATS, Lofi Chillhop");
    assert_eq!(album.year, "2025");
    assert!(
        album.thumbnail_url.starts_with("https://"),
        "thumb: {}",
        album.thumbnail_url
    );

    assert_eq!(album.tracks.len(), 3);
    // Ground truth: tracks inherit album-level artists; `album` is "" (real
    // ytmusicapi overwrites the track album with the title STRING, which
    // dict_to_album_track skips); track thumbnails are empty here.
    let expected = [
        ("hVJcHYs2LgA", "Lofi Study Chill", 121.0),
        ("OAKZj4PHGX8", "Chillhop Gentle Breeze", 240.0),
        ("RMIz-jDgjB0", "Lofi Work Mode", 153.0),
    ];
    for (track, (vid, title, dur)) in album.tracks.iter().zip(expected) {
        assert_eq!(track.video_id, vid);
        assert_eq!(track.title, title);
        assert_eq!(track.artist, "LO-FI BEATS, Lofi Chillhop");
        assert_eq!(track.album, "");
        assert_eq!(track.duration_seconds, dur);
        assert_eq!(track.thumbnail_url, "");
    }

    let (endpoint, body) = client.last();
    assert_eq!(endpoint, "browse");
    assert_eq!(body["browseId"], "MPREb_test");
}

// ---------------------------------------------------------------------------
// artist
// ---------------------------------------------------------------------------

#[test]
fn artist_returns_expected_sections() {
    let client = FakePost::new(fixture("artist.json"));
    let artist = block_on(get_artist(&client, "UC_test")).expect("artist ok");

    // channel_id is the input id, not the parsed subscriptionButton id.
    assert_eq!(artist.channel_id, "UC_test");
    assert_eq!(artist.name, "Lofi Girl");
    assert_eq!(artist.description, "");
    assert!(artist.thumbnail_url.starts_with("https://"));

    // Top songs: from the leading musicShelfRenderer; artists filled, no
    // duration in this shelf.
    assert_eq!(artist.top_songs.len(), 2);
    assert_eq!(artist.top_songs[0].video_id, "zJymZhHQmfs");
    assert_eq!(artist.top_songs[0].title, "Snowman");
    assert_eq!(artist.top_songs[0].artist, "WYS, Lofi Girl");
    assert_eq!(artist.top_songs[0].album, "Snowman");
    assert_eq!(artist.top_songs[1].video_id, "B_IrQoHbhAE");
    assert_eq!(artist.top_songs[1].artist, "Casiio, Kainbeats, Lofi Girl");

    // Albums: the "Albums" carousel only (the "Singles & EPs" carousel is the
    // ignored `singles` category).
    assert_eq!(artist.albums.len(), 2);
    assert_eq!(artist.albums[0].browse_id, "MPREb_uIxpnvfvGsl");
    assert_eq!(
        artist.albums[0].title,
        "Lofi Girl x The Sims - cozy music to feel ooh be gah!"
    );
    assert_eq!(artist.albums[0].artist, "");
    assert_eq!(artist.albums[0].year, "2026");
    assert_eq!(artist.albums[1].browse_id, "MPREb_Q4sZOGlm97U");

    // No related-artist carousel in this fixture.
    assert_eq!(artist.related_artists.len(), 0);

    // ytmusicapi strips a leading MPLA before requesting; a UC id is unchanged.
    let (endpoint, body) = client.last();
    assert_eq!(endpoint, "browse");
    assert_eq!(body["browseId"], "UC_test");
}

#[test]
fn artist_strips_mpla_prefix_in_request() {
    let client = FakePost::new(fixture("artist.json"));
    let artist = block_on(get_artist(&client, "MPLAUC_test")).expect("ok");
    // The domain channel_id keeps the original argument...
    assert_eq!(artist.channel_id, "MPLAUC_test");
    // ...but the request id has MPLA stripped.
    let (_, body) = client.last();
    assert_eq!(body["browseId"], "UC_test");
}
