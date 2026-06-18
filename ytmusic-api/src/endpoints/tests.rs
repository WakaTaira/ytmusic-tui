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

use super::{
    PostRequest, add_playlist_items, create_playlist, get_album, get_artist, get_history, get_home,
    get_library_albums, get_library_artists, get_library_playlists, get_like_status,
    get_liked_songs, get_lyrics, get_playlist_tracks, get_radio, rate_playlist, rate_track,
    remove_playlist_items, search_all, subscribe_artists, unsubscribe_artists,
};
use crate::error::ApiError;
use crate::models::HomeSectionItem;

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

/// A fake transport that returns a response chosen by endpoint, and records the
/// ordered list of `(endpoint, body)` calls — for the multi-request flows
/// (lyrics: `next` then `browse`).
struct MapPost {
    by_endpoint: std::collections::HashMap<String, Value>,
    calls: Mutex<Vec<(String, Value)>>,
}

impl MapPost {
    fn new(pairs: &[(&str, Value)]) -> Self {
        Self {
            by_endpoint: pairs
                .iter()
                .map(|(e, v)| ((*e).to_owned(), v.clone()))
                .collect(),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<(String, Value)> {
        self.calls.lock().unwrap().clone()
    }
}

impl PostRequest for MapPost {
    async fn post_request(&self, endpoint: &str, body: Value) -> Result<Value, ApiError> {
        self.calls.lock().unwrap().push((endpoint.to_owned(), body));
        match self.by_endpoint.get(endpoint) {
            Some(v) => Ok(v.clone()),
            None => Err(ApiError::Http {
                status: 404,
                message: format!("no fixture for endpoint {endpoint}"),
            }),
        }
    }
}

/// A fake transport that returns a queued sequence of responses for repeated
/// calls to the same endpoint — used by the continuation-paging tests where
/// each `browse` POST resolves a different page of the same playlist.
struct SequencePost {
    responses: Mutex<std::collections::VecDeque<Value>>,
    calls: Mutex<Vec<(String, Value)>>,
}

impl SequencePost {
    fn new(responses: Vec<Value>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().collect()),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<(String, Value)> {
        self.calls.lock().unwrap().clone()
    }
}

impl PostRequest for SequencePost {
    async fn post_request(&self, endpoint: &str, body: Value) -> Result<Value, ApiError> {
        self.calls.lock().unwrap().push((endpoint.to_owned(), body));
        match self.responses.lock().unwrap().pop_front() {
            Some(v) => Ok(v),
            None => Err(ApiError::Http {
                status: 500,
                message: "SequencePost: response queue exhausted".to_owned(),
            }),
        }
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
    assert_eq!(tracks[1].artist, "Test Uploader C");

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
// playlist tracks — continuation paging (issue #6)
// ---------------------------------------------------------------------------

/// Build a minimal initial playlist response with `videoIds` as rows, optionally
/// carrying a `continuations[0].nextContinuationData.continuation` token.
fn make_initial_playlist_page(video_ids: &[&str], next_token: Option<&str>) -> Value {
    let contents: Vec<Value> = video_ids.iter().map(|v| playlist_row(v)).collect();
    let mut shelf = serde_json::json!({
        "musicPlaylistShelfRenderer": { "contents": contents },
    });
    if let Some(token) = next_token {
        shelf["musicPlaylistShelfRenderer"]["continuations"] = serde_json::json!([
            { "nextContinuationData": { "continuation": token, "clickTrackingParams": "ctp" } }
        ]);
    }
    serde_json::json!({
        "contents": {
            "twoColumnBrowseResultsRenderer": {
                "secondaryContents": {
                    "sectionListRenderer": { "contents": [shelf] }
                }
            }
        }
    })
}

/// Build a minimal continuation playlist response with `videoIds` as rows,
/// optionally carrying the next-page token.
fn make_continuation_playlist_page(video_ids: &[&str], next_token: Option<&str>) -> Value {
    let contents: Vec<Value> = video_ids.iter().map(|v| playlist_row(v)).collect();
    let mut shelf = serde_json::json!({
        "musicPlaylistShelfContinuation": { "contents": contents },
    });
    if let Some(token) = next_token {
        shelf["musicPlaylistShelfContinuation"]["continuations"] = serde_json::json!([
            { "nextContinuationData": { "continuation": token, "clickTrackingParams": "ctp" } }
        ]);
    }
    serde_json::json!({ "continuationContents": shelf })
}

/// Build a minimal MRLIR row carrying just the fields stage-2 `dict_to_track`
/// needs (videoId + a title flex column with the play-button overlay).
fn playlist_row(video_id: &str) -> Value {
    serde_json::json!({
        "musicResponsiveListItemRenderer": {
            "flexColumns": [
                { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [
                    { "text": format!("Track {video_id}"),
                      "navigationEndpoint": {
                        "watchEndpoint": { "videoId": video_id }
                      }
                    }
                ] } } }
            ],
            "fixedColumns": [
                { "musicResponsiveListItemFixedColumnRenderer": {
                    "text": { "runs": [{ "text": "3:00" }] }
                } }
            ],
            "overlay": {
                "musicItemThumbnailOverlayRenderer": {
                    "content": {
                        "musicPlayButtonRenderer": {
                            "playNavigationEndpoint": {
                                "watchEndpoint": { "videoId": video_id }
                            }
                        }
                    }
                }
            }
        }
    })
}

#[test]
fn playlist_tracks_walks_continuations_across_pages() {
    // Two-page playlist: page 1 has 2 tracks + a continuation token; page 2 has
    // 2 more tracks + no token. The flow should load all 4 tracks.
    let page1 = make_initial_playlist_page(&["v1", "v2"], Some("TOKEN_PAGE_2"));
    let page2 = make_continuation_playlist_page(&["v3", "v4"], None);
    let client = SequencePost::new(vec![page1, page2]);

    let tracks = block_on(get_playlist_tracks(&client, "PL_multi")).expect("ok");
    assert_eq!(tracks.len(), 4, "all pages loaded");
    let ids: Vec<&str> = tracks.iter().map(|t| t.video_id.as_str()).collect();
    assert_eq!(ids, vec!["v1", "v2", "v3", "v4"]);

    // The second call must carry the continuation token in the body, not a
    // browseId — that is the distinguishing wire shape vs. the initial fetch.
    let calls = client.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].0, "browse");
    assert_eq!(calls[0].1["browseId"], "VLPL_multi");
    assert!(
        calls[0].1.get("continuation").is_none(),
        "initial call sends no continuation key"
    );
    assert_eq!(calls[1].0, "browse");
    assert_eq!(calls[1].1["continuation"], "TOKEN_PAGE_2");
    assert!(
        calls[1].1.get("browseId").is_none(),
        "continuation call sends no browseId"
    );
}

#[test]
fn playlist_tracks_walks_three_pages() {
    // Three-page chain: page 1 → token A → page 2 → token B → page 3 (no token).
    let page1 = make_initial_playlist_page(&["a1", "a2"], Some("TOKEN_A"));
    let page2 = make_continuation_playlist_page(&["b1", "b2"], Some("TOKEN_B"));
    let page3 = make_continuation_playlist_page(&["c1"], None);
    let client = SequencePost::new(vec![page1, page2, page3]);

    let tracks = block_on(get_playlist_tracks(&client, "PL_three")).expect("ok");
    assert_eq!(tracks.len(), 5, "three pages flattened");
    let ids: Vec<&str> = tracks.iter().map(|t| t.video_id.as_str()).collect();
    assert_eq!(ids, vec!["a1", "a2", "b1", "b2", "c1"]);

    let calls = client.calls();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[1].1["continuation"], "TOKEN_A");
    assert_eq!(calls[2].1["continuation"], "TOKEN_B");
}

#[test]
fn playlist_tracks_single_page_makes_one_call() {
    // The existing single-page fixture has no continuations; the flow must
    // exit after one call (no extra browse posts).
    let client = FakePost::new(fixture("playlist.json"));
    let tracks = block_on(get_playlist_tracks(&client, "PL_test")).expect("ok");
    assert_eq!(tracks.len(), 3, "single-page fixture unchanged");
    // FakePost only remembers the last call; we rely on the SequencePost test
    // above for ordering. Here we just confirm the call shape: no continuation.
    let (_, body) = client.last();
    assert!(
        body.get("continuation").is_none(),
        "single-page flow sends no continuation key"
    );
}

#[test]
fn playlist_tracks_respects_50_page_bound() {
    // A pathological server that never stops returning continuation tokens:
    // build 60 continuation pages, all with the same "next" token. The flow
    // must terminate at MAX_PLAYLIST_CONTINUATION_PAGES (50) total
    // continuation calls + 1 initial = 51 calls.
    let mut pages: Vec<Value> = Vec::new();
    pages.push(make_initial_playlist_page(&["v0"], Some("LOOP")));
    for i in 0..60 {
        // Each page carries one unique videoId so we can count rows; the
        // continuation token always points to another loop page.
        pages.push(make_continuation_playlist_page(
            &[&format!("v{}", i + 1)],
            Some("LOOP"),
        ));
    }
    let client = SequencePost::new(pages);

    let tracks = block_on(get_playlist_tracks(&client, "PL_runaway")).expect("ok");
    // 1 initial page (1 track) + 50 continuation pages (1 track each) = 51.
    assert_eq!(tracks.len(), 51, "bound caps continuation walking at 50");
    assert_eq!(
        client.calls().len(),
        51,
        "exactly 1 initial + 50 continuation calls"
    );
}

#[test]
fn playlist_tracks_continuation_propagates_transport_error() {
    // Page 1 succeeds and carries a continuation token; page 2 fails. The
    // error must propagate — partial tracks must NOT be returned silently.
    struct FailOnContinuation {
        first_response: Value,
        calls: Mutex<usize>,
    }
    impl PostRequest for FailOnContinuation {
        async fn post_request(&self, _endpoint: &str, _body: Value) -> Result<Value, ApiError> {
            let mut n = self.calls.lock().unwrap();
            *n += 1;
            if *n == 1 {
                Ok(self.first_response.clone())
            } else {
                Err(ApiError::Http {
                    status: 502,
                    message: "boom".to_owned(),
                })
            }
        }
    }

    let client = FailOnContinuation {
        first_response: make_initial_playlist_page(&["v1"], Some("TOKEN")),
        calls: Mutex::new(0),
    };
    let err = block_on(get_playlist_tracks(&client, "PL")).expect_err("should fail");
    assert!(
        matches!(err, ApiError::Http { status: 502, .. }),
        "got: {err:?}"
    );
}

#[test]
fn playlist_tracks_empty_continuation_response_terminates() {
    // Page 1 carries a token but page 2 has no shelf at all (e.g. server
    // returned an empty `continuationContents`). The flow must stop cleanly
    // and return whatever it has so far — no panic, no extra calls.
    let page1 = make_initial_playlist_page(&["v1"], Some("TOKEN"));
    let empty_page = serde_json::json!({ "continuationContents": {} });
    let client = SequencePost::new(vec![page1, empty_page]);

    let tracks = block_on(get_playlist_tracks(&client, "PL")).expect("ok");
    assert_eq!(tracks.len(), 1, "only page 1 contributed");
    assert_eq!(
        client.calls().len(),
        2,
        "one initial + one continuation call"
    );
}

// ---------------------------------------------------------------------------
// playlist tracks — modern continuation shape (issue #26)
//
// YouTube Music moved the next-page token out of
// `musicPlaylistShelfRenderer.continuations[0].nextContinuationData` and into
// a `continuationItemRenderer` sitting at the tail of the same
// `contents` array; continuation responses now arrive under
// `onResponseReceivedActions[0].appendContinuationItemsAction.continuationItems`.
// The legacy-only parser silently capped playlists at one page (~96 visible
// tracks after the unavailable-track filter). These tests lock the modern
// shape and the modern↔legacy fallback chain.
// ---------------------------------------------------------------------------

/// A `continuationItemRenderer` carrying the next-page token under the modern
/// `continuationEndpoint.continuationCommand` wrapper.
fn continuation_item_renderer(token: &str) -> Value {
    serde_json::json!({
        "continuationItemRenderer": {
            "continuationEndpoint": {
                "continuationCommand": {
                    "token": token,
                    "request": "CONTINUATION_REQUEST_TYPE_BROWSE",
                }
            }
        }
    })
}

/// Build a minimal modern initial playlist response: track rows live in
/// `musicPlaylistShelfRenderer.contents` and the continuation marker (if any)
/// is appended as the trailing element.
fn make_modern_initial_playlist_page(video_ids: &[&str], next_token: Option<&str>) -> Value {
    let mut contents: Vec<Value> = video_ids.iter().map(|v| playlist_row(v)).collect();
    if let Some(token) = next_token {
        contents.push(continuation_item_renderer(token));
    }
    serde_json::json!({
        "contents": {
            "twoColumnBrowseResultsRenderer": {
                "secondaryContents": {
                    "sectionListRenderer": {
                        "contents": [{
                            "musicPlaylistShelfRenderer": { "contents": contents }
                        }]
                    }
                }
            }
        }
    })
}

/// Build a minimal modern continuation playlist response: rows + trailing
/// `continuationItemRenderer` live in
/// `onResponseReceivedActions[0].appendContinuationItemsAction.continuationItems`.
fn make_modern_continuation_playlist_page(video_ids: &[&str], next_token: Option<&str>) -> Value {
    let mut items: Vec<Value> = video_ids.iter().map(|v| playlist_row(v)).collect();
    if let Some(token) = next_token {
        items.push(continuation_item_renderer(token));
    }
    serde_json::json!({
        "onResponseReceivedActions": [{
            "appendContinuationItemsAction": { "continuationItems": items }
        }]
    })
}

#[test]
fn playlist_tracks_walks_modern_continuation_shape() {
    let page1 = make_modern_initial_playlist_page(&["m1", "m2"], Some("MODERN_TOKEN_A"));
    let page2 = make_modern_continuation_playlist_page(&["m3", "m4"], None);
    let client = SequencePost::new(vec![page1, page2]);

    let tracks = block_on(get_playlist_tracks(&client, "PL_modern")).expect("ok");
    assert_eq!(tracks.len(), 4, "modern shape paged through both pages");
    let ids: Vec<&str> = tracks.iter().map(|t| t.video_id.as_str()).collect();
    assert_eq!(ids, vec!["m1", "m2", "m3", "m4"]);

    let calls = client.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].0, "browse");
    assert_eq!(calls[1].1["continuation"], "MODERN_TOKEN_A");
    assert!(
        calls[1].1.get("browseId").is_none(),
        "modern continuation call still sends no browseId"
    );
}

#[test]
fn playlist_tracks_walks_modern_three_pages() {
    let page1 = make_modern_initial_playlist_page(&["a1"], Some("TOK_A"));
    let page2 = make_modern_continuation_playlist_page(&["b1"], Some("TOK_B"));
    let page3 = make_modern_continuation_playlist_page(&["c1"], None);
    let client = SequencePost::new(vec![page1, page2, page3]);

    let tracks = block_on(get_playlist_tracks(&client, "PL_modern3")).expect("ok");
    assert_eq!(tracks.len(), 3, "three modern pages flattened");
    let ids: Vec<&str> = tracks.iter().map(|t| t.video_id.as_str()).collect();
    assert_eq!(ids, vec!["a1", "b1", "c1"]);

    let calls = client.calls();
    assert_eq!(calls[1].1["continuation"], "TOK_A");
    assert_eq!(calls[2].1["continuation"], "TOK_B");
}

#[test]
fn playlist_tracks_modern_trailing_marker_not_returned_as_track() {
    // The continuationItemRenderer sits in the same contents array as real
    // rows. parse_playlist_tracks must filter it out via the MRLIR check —
    // otherwise a junk "track" would surface on every page.
    let page1 = make_modern_initial_playlist_page(&["only_one"], Some("X"));
    let page2 = make_modern_continuation_playlist_page(&[], None);
    let client = SequencePost::new(vec![page1, page2]);

    let tracks = block_on(get_playlist_tracks(&client, "PL_marker")).expect("ok");
    assert_eq!(tracks.len(), 1, "trailing continuation marker filtered");
    assert_eq!(tracks[0].video_id, "only_one");
}

#[test]
fn playlist_tracks_mixed_modern_initial_legacy_continuation() {
    // YT cohorts may serve a modern initial response followed by a legacy
    // continuation (or vice versa). The fallback chain absorbs either.
    let page1 = make_modern_initial_playlist_page(&["x1"], Some("MIX_TOK"));
    let page2 = make_continuation_playlist_page(&["x2"], None); // legacy continuation shape
    let client = SequencePost::new(vec![page1, page2]);

    let tracks = block_on(get_playlist_tracks(&client, "PL_mix")).expect("ok");
    assert_eq!(tracks.len(), 2, "modern→legacy chain still walks");
    let ids: Vec<&str> = tracks.iter().map(|t| t.video_id.as_str()).collect();
    assert_eq!(ids, vec!["x1", "x2"]);
}

#[test]
fn playlist_tracks_mixed_legacy_initial_modern_continuation() {
    let page1 = make_initial_playlist_page(&["y1"], Some("MIX2_TOK")); // legacy initial
    let page2 = make_modern_continuation_playlist_page(&["y2"], None); // modern continuation
    let client = SequencePost::new(vec![page1, page2]);

    let tracks = block_on(get_playlist_tracks(&client, "PL_mix2")).expect("ok");
    assert_eq!(tracks.len(), 2, "legacy→modern chain still walks");
    let ids: Vec<&str> = tracks.iter().map(|t| t.video_id.as_str()).collect();
    assert_eq!(ids, vec!["y1", "y2"]);
}

// ---------------------------------------------------------------------------
// liked songs (get_playlist("LM"))
// ---------------------------------------------------------------------------

#[test]
fn liked_songs_returns_expected() {
    let client = FakePost::new(fixture("liked_songs.json"));
    let tracks = block_on(get_liked_songs(&client, 100)).expect("liked ok");

    // Ground truth from the live api.py pipeline over liked_songs.json.
    assert_eq!(tracks.len(), 3);
    assert_eq!(tracks[0].video_id, "7s4RmXxcZvM");
    assert_eq!(tracks[0].title, "「宝島」(吹奏楽) ピアノで弾きました");
    assert_eq!(tracks[0].artist, "Fujii Kaze");
    assert_eq!(tracks[0].album, "");
    assert_eq!(tracks[0].duration_seconds, 179.0);

    assert_eq!(tracks[1].video_id, "KwZFalJdsbQ");
    assert_eq!(tracks[1].artist, "Test Uploader C");
    assert_eq!(tracks[1].duration_seconds, 193.0);

    assert_eq!(tracks[2].video_id, "hlr_7Za6v0M");
    assert_eq!(tracks[2].album, "Returns To Dreamland");
    assert_eq!(tracks[2].duration_seconds, 115.0);

    // get_liked_songs reuses the playlist flow: "LM" → "VLLM" via browse.
    let (endpoint, body) = client.last();
    assert_eq!(endpoint, "browse");
    assert_eq!(body["browseId"], "VLLM");
}

#[test]
fn liked_songs_respects_limit() {
    let client = FakePost::new(fixture("liked_songs.json"));
    let tracks = block_on(get_liked_songs(&client, 2)).expect("liked ok");
    assert_eq!(tracks.len(), 2, "limit caps the liked list");
}

// ---------------------------------------------------------------------------
// library playlists
// ---------------------------------------------------------------------------

#[test]
fn library_playlists_returns_expected() {
    let client = FakePost::new(fixture("library_playlists.json"));
    let playlists = block_on(get_library_playlists(&client, 25)).expect("ok");

    // Ground truth (api.py over library_playlists.json): the "New playlist"
    // pseudo-item is skipped, leaving 3 playlists.
    assert_eq!(playlists.len(), 3);
    assert_eq!(playlists[0].playlist_id, "LM");
    assert_eq!(playlists[0].title, "Liked Music");
    assert_eq!(playlists[0].description, "Auto playlist");
    assert_eq!(playlists[0].track_count, 0);

    assert_eq!(
        playlists[1].playlist_id,
        "PLv_jJ3zdS10pmtSFiE8PgXa7v0OWhN8yA"
    );
    assert_eq!(playlists[1].title, "GT");
    assert_eq!(playlists[1].track_count, 2); // "TestUser • 2 tracks"

    assert_eq!(playlists[2].title, "2025 Recap");
    assert_eq!(playlists[2].track_count, 0); // "Made for TestUser • 99 songs" (not a 3-run count)

    let (endpoint, body) = client.last();
    assert_eq!(endpoint, "browse");
    assert_eq!(body["browseId"], "FEmusic_liked_playlists");
}

#[test]
fn library_playlists_respects_limit() {
    let client = FakePost::new(fixture("library_playlists.json"));
    let playlists = block_on(get_library_playlists(&client, 2)).expect("ok");
    assert_eq!(playlists.len(), 2);
}

// ---------------------------------------------------------------------------
// library albums
// ---------------------------------------------------------------------------

#[test]
fn library_albums_returns_expected() {
    let client = FakePost::new(fixture("library_albums.json"));
    let albums = block_on(get_library_albums(&client, 25)).expect("ok");

    assert_eq!(albums.len(), 3);
    let expected = [
        ("MPREb_6Hlu7bz5KzT", "Die Lit", "Playboi Carti", "2018"),
        (
            "MPREb_ixfbA4zK0Nj",
            "MUSIC - SORRY 4 DA WAIT",
            "Playboi Carti",
            "2025",
        ),
        ("MPREb_Zfk2NiycExr", "MUSIC", "Playboi Carti", "2025"),
    ];
    for (album, (bid, title, artist, year)) in albums.iter().zip(expected) {
        assert_eq!(album.browse_id, bid);
        assert_eq!(album.title, title);
        assert_eq!(album.artist, artist);
        assert_eq!(album.year, year);
        assert!(
            album.tracks.is_empty(),
            "library albums carry no track list"
        );
    }

    let (endpoint, body) = client.last();
    assert_eq!(endpoint, "browse");
    assert_eq!(body["browseId"], "FEmusic_liked_albums");
}

// ---------------------------------------------------------------------------
// library artists
// ---------------------------------------------------------------------------

#[test]
fn library_artists_returns_expected() {
    let client = FakePost::new(fixture("library_artists.json"));
    let artists = block_on(get_library_artists(&client, 25)).expect("ok");

    assert_eq!(artists.len(), 3);
    let expected = [
        ("MPLAUCRB-a6u9flpg0xuBqCf9QlQ", "Playboi Carti"),
        ("MPLAUCf_gP4AMRSgAfyzbkeS9k4g", "Travis Scott"),
        ("MPLAUC1_liDR4fRFJgH4HoJeV8cw", "Future"),
    ];
    for (artist, (cid, name)) in artists.iter().zip(expected) {
        assert_eq!(artist.channel_id, cid);
        assert_eq!(artist.name, name);
        // new_minimal: identity-only, no sections.
        assert_eq!(artist.description, "");
        assert!(artist.top_songs.is_empty());
        assert!(artist.albums.is_empty());
        assert!(artist.related_artists.is_empty());
    }

    let (endpoint, body) = client.last();
    assert_eq!(endpoint, "browse");
    assert_eq!(body["browseId"], "FEmusic_library_corpus_track_artists");
}

// ---------------------------------------------------------------------------
// library continuations (issue #26)
//
// All three list endpoints (playlists / albums / artists) now walk
// continuation chains, both shapes:
// * modern — `continuationItemRenderer` at the tail + `appendContinuationItemsAction`
// * legacy — `…continuations[0].nextContinuationData.continuation` + `gridContinuation`
//            / `musicShelfContinuation` wrappers
// These tests lock both shapes against synthetic fixtures (no live YTM
// dependency); the limit / page-bound behavior is covered by the playlist
// tests since the loop topology is identical.
// ---------------------------------------------------------------------------

/// Build a minimal MTRIR playlist card (the keys `parse_playlist_card` reads).
fn library_playlist_card(playlist_id: &str, title: &str, track_count: usize) -> Value {
    serde_json::json!({
        "musicTwoRowItemRenderer": {
            "title": { "runs": [{
                "text": title,
                "navigationEndpoint": {
                    "browseEndpoint": { "browseId": format!("VL{playlist_id}") }
                }
            }]},
            "subtitle": { "runs": [
                { "text": "Owner" },
                { "text": " • " },
                { "text": format!("{track_count} tracks") },
            ]}
        }
    })
}

/// Build a minimal MTRIR album card (the keys `parse_album_card` reads).
fn library_album_card(browse_id: &str, title: &str, artist: &str, year: &str) -> Value {
    serde_json::json!({
        "musicTwoRowItemRenderer": {
            "title": { "runs": [{
                "text": title,
                "navigationEndpoint": {
                    "browseEndpoint": { "browseId": browse_id }
                }
            }]},
            "subtitle": { "runs": [
                { "text": "Album" },
                { "text": " • " },
                { "text": artist },
                { "text": " • " },
                { "text": year },
            ]}
        }
    })
}

/// Build a minimal MRLIR artist row (the keys `parse_library_artist_row` reads).
fn library_artist_row(channel_id: &str, name: &str) -> Value {
    serde_json::json!({
        "musicResponsiveListItemRenderer": {
            "navigationEndpoint": {
                "browseEndpoint": { "browseId": channel_id }
            },
            "flexColumns": [{
                "musicResponsiveListItemFlexColumnRenderer": {
                    "text": { "runs": [{ "text": name }] }
                }
            }]
        }
    })
}

/// Same continuation marker shape as the playlist tests use.
fn library_continuation_marker(token: &str) -> Value {
    serde_json::json!({
        "continuationItemRenderer": {
            "continuationEndpoint": {
                "continuationCommand": {
                    "token": token,
                    "request": "CONTINUATION_REQUEST_TYPE_BROWSE",
                }
            }
        }
    })
}

/// Wrap GRID items into the library `singleColumnBrowseResultsRenderer` shell
/// `parse_library_*` walks. `items[0]` is the "New playlist" pseudo-item that
/// the parser skips — caller supplies real items only.
fn library_grid_response(real_items: Vec<Value>, tail_marker: Option<Value>) -> Value {
    let mut items = vec![
        serde_json::json!({ "musicTwoRowItemRenderer": { "title": { "runs": [
            { "text": "New playlist" }
        ]}}}),
    ];
    items.extend(real_items);
    if let Some(marker) = tail_marker {
        items.push(marker);
    }
    serde_json::json!({
        "contents": {
            "singleColumnBrowseResultsRenderer": {
                "tabs": [{ "tabRenderer": { "content": {
                    "sectionListRenderer": { "contents": [
                        { "gridRenderer": { "items": items } }
                    ]}
                }}}]
            }
        }
    })
}

/// Same as [`library_grid_response`] but for the MUSIC_SHELF (artists) layout.
fn library_shelf_response(rows: Vec<Value>, tail_marker: Option<Value>) -> Value {
    let mut contents = rows;
    if let Some(marker) = tail_marker {
        contents.push(marker);
    }
    serde_json::json!({
        "contents": {
            "singleColumnBrowseResultsRenderer": {
                "tabs": [{ "tabRenderer": { "content": {
                    "sectionListRenderer": { "contents": [
                        { "musicShelfRenderer": { "contents": contents } }
                    ]}
                }}}]
            }
        }
    })
}

/// Build a modern continuation response carrying `items` (plus an optional
/// trailing marker). Shape matches both grid- and shelf-flavoured continuations
/// because the modern envelope is universal.
fn library_modern_continuation_response(items: Vec<Value>, tail_marker: Option<Value>) -> Value {
    let mut all = items;
    if let Some(marker) = tail_marker {
        all.push(marker);
    }
    serde_json::json!({
        "onResponseReceivedActions": [{
            "appendContinuationItemsAction": { "continuationItems": all }
        }]
    })
}

#[test]
fn library_playlists_walks_modern_continuation() {
    // Page 1: 2 playlist cards + continuation marker.
    let page1 = library_grid_response(
        vec![
            library_playlist_card("PL_a", "Alpha", 10),
            library_playlist_card("PL_b", "Beta", 20),
        ],
        Some(library_continuation_marker("LIB_TOK_A")),
    );
    // Page 2: 2 more playlist cards, no marker (chain ends).
    let page2 = library_modern_continuation_response(
        vec![
            library_playlist_card("PL_c", "Gamma", 30),
            library_playlist_card("PL_d", "Delta", 40),
        ],
        None,
    );
    let client = SequencePost::new(vec![page1, page2]);

    let playlists = block_on(get_library_playlists(&client, 25)).expect("ok");
    assert_eq!(
        playlists.len(),
        4,
        "all pages flattened past the legacy 1-page cap"
    );
    let titles: Vec<&str> = playlists.iter().map(|p| p.title.as_str()).collect();
    assert_eq!(titles, vec!["Alpha", "Beta", "Gamma", "Delta"]);

    let calls = client.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].1["continuation"], "LIB_TOK_A");
}

#[test]
fn library_playlists_respects_limit_with_continuation() {
    // limit=3 with 2 cards/page: page 1 fills 2, page 2 fills the rest; loop
    // exits after the second call. Limit truncates the final 1 extra.
    let page1 = library_grid_response(
        vec![
            library_playlist_card("PL_a", "A", 1),
            library_playlist_card("PL_b", "B", 1),
        ],
        Some(library_continuation_marker("X")),
    );
    let page2 = library_modern_continuation_response(
        vec![
            library_playlist_card("PL_c", "C", 1),
            library_playlist_card("PL_d", "D", 1),
        ],
        Some(library_continuation_marker("Y")), // unused — loop stops on limit
    );
    let client = SequencePost::new(vec![page1, page2]);

    let playlists = block_on(get_library_playlists(&client, 3)).expect("ok");
    assert_eq!(playlists.len(), 3, "limit honored");
    let calls = client.calls();
    assert_eq!(calls.len(), 2, "no third fetch once limit reached");
}

#[test]
fn library_playlists_walks_legacy_continuation() {
    // Legacy initial shape: gridRenderer.continuations[0].nextContinuationData
    // sits on the grid renderer itself (no tail marker).
    let page1 = serde_json::json!({
        "contents": { "singleColumnBrowseResultsRenderer": {
            "tabs": [{ "tabRenderer": { "content": { "sectionListRenderer": { "contents": [{
                "gridRenderer": {
                    "items": [
                        serde_json::json!({ "musicTwoRowItemRenderer": { "title": { "runs": [{ "text": "New playlist" }]}}}),
                        library_playlist_card("PL_legacy_a", "L_A", 3)
                    ],
                    "continuations": [{
                        "nextContinuationData": { "continuation": "LEGACY_TOK", "clickTrackingParams": "ctp" }
                    }]
                }
            }]}}}}]
        }}
    });
    // Legacy continuation response: continuationContents.gridContinuation
    let page2 = serde_json::json!({
        "continuationContents": {
            "gridContinuation": {
                "items": [library_playlist_card("PL_legacy_b", "L_B", 5)]
            }
        }
    });
    let client = SequencePost::new(vec![page1, page2]);

    let playlists = block_on(get_library_playlists(&client, 25)).expect("ok");
    assert_eq!(playlists.len(), 2);
    let titles: Vec<&str> = playlists.iter().map(|p| p.title.as_str()).collect();
    assert_eq!(titles, vec!["L_A", "L_B"]);
    assert_eq!(client.calls()[1].1["continuation"], "LEGACY_TOK");
}

#[test]
fn library_albums_walks_modern_continuation() {
    let page1 = library_grid_response(
        vec![library_album_card("MPRE_a", "Album A", "Artist A", "2020")],
        Some(library_continuation_marker("ALB_TOK")),
    );
    let page2 = library_modern_continuation_response(
        vec![library_album_card("MPRE_b", "Album B", "Artist B", "2021")],
        None,
    );
    let client = SequencePost::new(vec![page1, page2]);

    let albums = block_on(get_library_albums(&client, 25)).expect("ok");
    assert_eq!(albums.len(), 2, "modern continuation walked for albums");
    let titles: Vec<&str> = albums.iter().map(|a| a.title.as_str()).collect();
    assert_eq!(titles, vec!["Album A", "Album B"]);
    assert_eq!(client.calls()[1].1["continuation"], "ALB_TOK");
}

#[test]
fn library_artists_walks_modern_continuation() {
    let page1 = library_shelf_response(
        vec![library_artist_row("MPLAUC_a", "Artist A")],
        Some(library_continuation_marker("ART_TOK")),
    );
    let page2 = library_modern_continuation_response(
        vec![library_artist_row("MPLAUC_b", "Artist B")],
        None,
    );
    let client = SequencePost::new(vec![page1, page2]);

    let artists = block_on(get_library_artists(&client, 25)).expect("ok");
    assert_eq!(
        artists.len(),
        2,
        "musicShelfContinuation walked for artists"
    );
    let names: Vec<&str> = artists.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, vec!["Artist A", "Artist B"]);
    assert_eq!(client.calls()[1].1["continuation"], "ART_TOK");
}

#[test]
fn library_artists_walks_legacy_continuation() {
    // Legacy shape: musicShelfRenderer.continuations + musicShelfContinuation.
    let page1 = serde_json::json!({
        "contents": { "singleColumnBrowseResultsRenderer": {
            "tabs": [{ "tabRenderer": { "content": { "sectionListRenderer": { "contents": [{
                "musicShelfRenderer": {
                    "contents": [library_artist_row("MPLAUC_la", "Legacy A")],
                    "continuations": [{
                        "nextContinuationData": { "continuation": "LEGACY_ART_TOK", "clickTrackingParams": "ctp" }
                    }]
                }
            }]}}}}]
        }}
    });
    let page2 = serde_json::json!({
        "continuationContents": {
            "musicShelfContinuation": {
                "contents": [library_artist_row("MPLAUC_lb", "Legacy B")]
            }
        }
    });
    let client = SequencePost::new(vec![page1, page2]);

    let artists = block_on(get_library_artists(&client, 25)).expect("ok");
    assert_eq!(artists.len(), 2);
    let names: Vec<&str> = artists.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, vec!["Legacy A", "Legacy B"]);
    assert_eq!(client.calls()[1].1["continuation"], "LEGACY_ART_TOK");
}

// ---------------------------------------------------------------------------
// history
// ---------------------------------------------------------------------------

#[test]
fn history_flattens_dated_shelves() {
    let client = FakePost::new(fixture("history.json"));
    let tracks = block_on(get_history(&client)).expect("history ok");

    // Ground truth (api.py over history.json): two dated shelves, 2 tracks each,
    // flattened in order. History rows are video-style: the trailing view-count
    // run ("6.7M views") classifies as artist text (no album/clickable artist),
    // so it is joined into `artist` exactly as the Python pipeline does.
    assert_eq!(tracks.len(), 4);

    assert_eq!(tracks[0].video_id, "tR2oqBzMwcE");
    assert_eq!(
        tracks[0].title,
        "[Armored Core Ⅵ] Mechanized Memories  - in the end -  機械化された記憶　lyrics 和訳"
    );
    assert_eq!(tracks[0].artist, "Test Uploader A, 6.7M views");
    assert_eq!(tracks[0].album, "");
    assert_eq!(tracks[0].duration_seconds, 329.0);

    // A real-artist row carries an album and a clean artist (no view-count run).
    assert_eq!(tracks[1].video_id, "wzKviWpfgS0");
    assert_eq!(tracks[1].artist, "Yuki Chiba");
    assert_eq!(tracks[1].album, "Okuman Choja");
    assert_eq!(tracks[1].duration_seconds, 168.0);

    // Second shelf flattened after the first.
    assert_eq!(tracks[2].video_id, "OeAxWQI8hng");
    assert_eq!(tracks[2].artist, "XXXTENTACION, 2.7M views");
    assert_eq!(tracks[3].video_id, "-7HMCgJpXjM");
    assert_eq!(tracks[3].album, "MUSIC");
    assert_eq!(tracks[3].duration_seconds, 152.0);

    let (endpoint, body) = client.last();
    assert_eq!(endpoint, "browse");
    assert_eq!(body["browseId"], "FEmusic_history");
}

// ---------------------------------------------------------------------------
// home
// ---------------------------------------------------------------------------

#[test]
fn home_returns_mixed_sections() {
    let client = FakePost::new(fixture("home.json"));
    let sections = block_on(get_home(&client)).expect("home ok");

    // Ground truth (api.py over home.json): 2 carousels — a song carousel
    // (3 Tracks) and a recap-playlist carousel (3 PlaylistInfos).
    assert_eq!(sections.len(), 2);

    // Section 0: "Listen again" — three song cards → Track items.
    assert_eq!(sections[0].title, "Listen again");
    assert_eq!(sections[0].items.len(), 3);
    let HomeSectionItem::Track(t0) = &sections[0].items[0] else {
        panic!("expected a Track");
    };
    assert_eq!(t0.video_id, "wzKviWpfgS0");
    assert_eq!(t0.title, "まずはイメージ - Mazu Wa Image");
    assert_eq!(t0.artist, "Yuki Chiba");
    let HomeSectionItem::Track(t1) = &sections[0].items[1] else {
        panic!("expected a Track");
    };
    assert_eq!(t1.video_id, "NBHr-EnB4oU");
    assert_eq!(t1.artist, "farmhouse, Kee Rooz, RhymeTube");
    let HomeSectionItem::Track(t2) = &sections[0].items[2] else {
        panic!("expected a Track");
    };
    assert_eq!(t2.video_id, "pagEova9QDU");

    // Section 1: "Recaps" — three playlist cards → PlaylistInfo items.
    assert_eq!(sections[1].title, "Recaps");
    assert_eq!(sections[1].items.len(), 3);
    let HomeSectionItem::Playlist(p0) = &sections[1].items[0] else {
        panic!("expected a PlaylistInfo");
    };
    assert_eq!(p0.playlist_id, "LRSRX_C6NActwVAgHdqS087Aj05fk-3ErGguA");
    assert_eq!(p0.title, "March-May Recap '26");
    let HomeSectionItem::Playlist(p2) = &sections[1].items[2] else {
        panic!("expected a PlaylistInfo");
    };
    assert_eq!(p2.title, "2025 Recap");

    let (endpoint, body) = client.last();
    assert_eq!(endpoint, "browse");
    assert_eq!(body["browseId"], "FEmusic_home");
}

// ---------------------------------------------------------------------------
// radio (next, radio=True)
// ---------------------------------------------------------------------------

#[test]
fn radio_returns_seeded_queue() {
    let client = FakePost::new(fixture("watch_radio.json"));
    let tracks = block_on(get_radio(&client, "dQw4w9WgXcQ", 50)).expect("radio ok");

    // Ground truth (api.py over watch_radio.json): the seed track first, then
    // the radio continuation. Watch items use `length` + singular `thumbnail`.
    assert_eq!(tracks.len(), 3);
    assert_eq!(tracks[0].video_id, "dQw4w9WgXcQ");
    assert_eq!(tracks[0].title, "Never Gonna Give You Up (7'' Mix)");
    assert_eq!(tracks[0].artist, "Rick Astley");
    assert_eq!(tracks[0].album, "");
    assert_eq!(tracks[0].duration_seconds, 214.0);

    assert_eq!(tracks[1].video_id, "rZlQ28OeGMI");
    assert_eq!(tracks[1].artist, "Rick Astley");
    assert_eq!(tracks[1].duration_seconds, 195.0);

    assert_eq!(tracks[2].video_id, "9SJG2LKGspI");
    assert_eq!(tracks[2].artist, "Test Uploader D");
    assert_eq!(tracks[2].duration_seconds, 150.0);

    // The radio flow posts `next` with the seed videoId, derived playlistId, and
    // the "wAEB" radio params.
    let (endpoint, body) = client.last();
    assert_eq!(endpoint, "next");
    assert_eq!(body["videoId"], "dQw4w9WgXcQ");
    assert_eq!(body["playlistId"], "RDAMVMdQw4w9WgXcQ");
    assert_eq!(body["params"], "wAEB");
}

#[test]
fn radio_respects_limit() {
    let client = FakePost::new(fixture("watch_radio.json"));
    let tracks = block_on(get_radio(&client, "dQw4w9WgXcQ", 2)).expect("radio ok");
    assert_eq!(tracks.len(), 2, "limit caps the radio queue");
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

// ---------------------------------------------------------------------------
// lyrics (two-call: next → browse)
// ---------------------------------------------------------------------------

#[test]
fn lyrics_returns_text_via_two_calls() {
    // First the watch panel (`next`) supplies the lyrics browse id; then the
    // lyrics `browse` supplies the text.
    let client = MapPost::new(&[
        ("next", fixture("lyrics_watch.json")),
        ("browse", fixture("lyrics_browse.json")),
    ]);
    let lyrics = block_on(get_lyrics(&client, "bSnlKl_PoQU")).expect("lyrics ok");

    let lyrics = lyrics.expect("Bohemian Rhapsody has lyrics");
    // Ground truth (api.py over the same fixtures): 1905 chars, known opener.
    assert_eq!(lyrics.chars().count(), 1905);
    assert!(
        lyrics.starts_with("Is this the real life? Is this just fantasy?"),
        "unexpected opener: {:?}",
        &lyrics[..lyrics
            .char_indices()
            .nth(45)
            .map_or(lyrics.len(), |(i, _)| i)]
    );

    // The flow posts `next` (with the lyrics browse id discovered there) then a
    // `browse` for that id.
    let calls = client.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].0, "next");
    assert_eq!(calls[0].1["videoId"], "bSnlKl_PoQU");
    assert_eq!(calls[1].0, "browse");
    assert!(
        calls[1].1["browseId"]
            .as_str()
            .is_some_and(|id| id.starts_with("MPLY")),
        "second call should browse the lyrics id, got {:?}",
        calls[1].1["browseId"]
    );
}

#[test]
fn lyrics_absent_is_none_not_error() {
    // A watch panel whose only tab is the non-lyrics "Up next" tab: the flow must
    // return Ok(None) — absence is a value — and must NOT make a second request.
    let watch = serde_json::json!({
        "contents": { "singleColumnMusicWatchNextResultsRenderer": { "tabbedRenderer": {
            "watchNextTabbedResultsRenderer": { "tabs": [
                { "tabRenderer": {
                    "title": "Up next",
                    "endpoint": { "browseEndpoint": {
                        "browseId": "FEmusic_whatever",
                        "browseEndpointContextSupportedConfigs": {
                            "browseEndpointContextMusicConfig": {
                                "pageType": "MUSIC_PAGE_TYPE_TRACK_RELATED" } } } }
                } }
            ] } } } }
    });
    let client = MapPost::new(&[("next", watch)]);
    let lyrics = block_on(get_lyrics(&client, "novid")).expect("no-lyrics is not an error");
    assert_eq!(lyrics, None, "missing lyrics tab → None");

    // Only the watch request was made (no browse for a non-existent lyrics id).
    let calls = client.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "next");
}

// ---------------------------------------------------------------------------
// get_like_status (watch fixture with likeStatus in item 0)
// ---------------------------------------------------------------------------

#[test]
fn get_like_status_returns_status_for_matching_video() {
    // The watch_radio fixture (now extended with a LIKE toggleMenuServiceItem
    // on item 0 / dQw4w9WgXcQ) is reused: parse_like_status maps
    // switch_to=INDIFFERENT → current=LIKE.
    let client = FakePost::new(fixture("watch_radio.json"));
    let status = block_on(get_like_status(&client, "dQw4w9WgXcQ")).expect("ok");
    assert_eq!(status, Some("LIKE".to_owned()), "seed track should be LIKE");

    // The flow posts `next` with the watch body (non-radio, includes ATV config).
    let (endpoint, body) = client.last();
    assert_eq!(endpoint, "next");
    assert_eq!(body["videoId"], "dQw4w9WgXcQ");
    // Non-radio body includes watchEndpointMusicSupportedConfigs.
    assert!(
        body.get("watchEndpointMusicSupportedConfigs").is_some(),
        "non-radio watch body required"
    );
    // No radio params.
    assert!(
        body.get("params").is_none(),
        "get_like_status must not send radio params"
    );
}

#[test]
fn get_like_status_returns_none_for_unknown_video() {
    let client = FakePost::new(fixture("watch_radio.json"));
    let status = block_on(get_like_status(&client, "not_in_fixture")).expect("ok");
    assert_eq!(status, None, "videoId not in panel → None");
}

#[test]
fn get_like_status_returns_null_when_menu_empty() {
    // Items 1 and 2 in the fixture have an empty menu → likeStatus is null →
    // we only get None when the videoId actually matches but status is null.
    let client = FakePost::new(fixture("watch_radio.json"));
    let status = block_on(get_like_status(&client, "rZlQ28OeGMI")).expect("ok");
    assert_eq!(status, None, "item with no toggle menu → None likeStatus");
}

// ---------------------------------------------------------------------------
// rate_track
// ---------------------------------------------------------------------------

#[test]
fn rate_track_like_posts_correct_endpoint_and_body() {
    let client = FakePost::new(serde_json::json!({}));
    block_on(rate_track(&client, "dQw4w9WgXcQ", "LIKE")).expect("rate ok");
    let (endpoint, body) = client.last();
    assert_eq!(endpoint, "like/like");
    assert_eq!(body["target"]["videoId"], "dQw4w9WgXcQ");
}

#[test]
fn rate_track_dislike_posts_correct_endpoint() {
    let client = FakePost::new(serde_json::json!({}));
    block_on(rate_track(&client, "vid123", "DISLIKE")).expect("ok");
    let (endpoint, _) = client.last();
    assert_eq!(endpoint, "like/dislike");
}

#[test]
fn rate_track_indifferent_posts_removelike() {
    let client = FakePost::new(serde_json::json!({}));
    block_on(rate_track(&client, "vid123", "INDIFFERENT")).expect("ok");
    let (endpoint, _) = client.last();
    assert_eq!(endpoint, "like/removelike");
}

#[test]
fn rate_track_invalid_status_returns_parse_error() {
    let client = FakePost::new(serde_json::json!({}));
    let err = block_on(rate_track(&client, "vid", "THUMBSUP")).expect_err("should fail");
    assert!(
        matches!(err, ApiError::Parse(_)),
        "invalid status → Parse error, got {err:?}"
    );
}

#[test]
fn rate_track_propagates_transport_error() {
    let err = block_on(rate_track(&FailingPost, "vid", "LIKE")).expect_err("should fail");
    assert!(matches!(err, ApiError::Http { status: 500, .. }));
}

// ---------------------------------------------------------------------------
// rate_playlist (save / remove album or playlist from the library — issue #12)
// ---------------------------------------------------------------------------

#[test]
fn rate_playlist_like_posts_correct_endpoint_and_body() {
    // The save path targets `like/like` with `{target: {playlistId}}` — the
    // playlist-id form of the same endpoint `rate_track` uses for video ids.
    let client = FakePost::new(serde_json::json!({}));
    block_on(rate_playlist(&client, "OLAK5uy_album_audio_id", "LIKE")).expect("rate ok");
    let (endpoint, body) = client.last();
    assert_eq!(endpoint, "like/like");
    assert_eq!(body["target"]["playlistId"], "OLAK5uy_album_audio_id");
    assert!(
        body["target"].get("videoId").is_none(),
        "playlist-rate body must not carry a videoId"
    );
}

#[test]
fn rate_playlist_unlike_posts_removelike() {
    let client = FakePost::new(serde_json::json!({}));
    block_on(rate_playlist(&client, "PL_user_playlist", "INDIFFERENT")).expect("ok");
    let (endpoint, body) = client.last();
    assert_eq!(endpoint, "like/removelike");
    assert_eq!(body["target"]["playlistId"], "PL_user_playlist");
}

#[test]
fn rate_playlist_strips_vl_prefix() {
    // ytmusicapi's playlist mutations strip the leading `VL` (a browse-id
    // artifact); the like endpoints want the bare id. Mirror that here so a
    // caller wiring a raw browse id straight through still hits the right row.
    let client = FakePost::new(serde_json::json!({}));
    block_on(rate_playlist(&client, "VLPL_with_prefix", "LIKE")).expect("ok");
    let (_, body) = client.last();
    assert_eq!(body["target"]["playlistId"], "PL_with_prefix");
}

#[test]
fn rate_playlist_invalid_status_returns_parse_error() {
    let client = FakePost::new(serde_json::json!({}));
    let err = block_on(rate_playlist(&client, "PL_test", "SAVE")).expect_err("should fail");
    assert!(
        matches!(err, ApiError::Parse(_)),
        "invalid status → Parse error, got {err:?}"
    );
}

#[test]
fn rate_playlist_propagates_transport_error() {
    let err = block_on(rate_playlist(&FailingPost, "PL_test", "LIKE")).expect_err("should fail");
    assert!(
        matches!(err, ApiError::Http { status: 500, .. }),
        "got: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// create_playlist
// ---------------------------------------------------------------------------

#[test]
fn create_playlist_returns_playlist_id() {
    let response = serde_json::json!({ "playlistId": "PLnewtest123" });
    let client = FakePost::new(response);
    let id = block_on(create_playlist(&client, "Test", "desc", "PRIVATE")).expect("ok");
    assert_eq!(id, "PLnewtest123");

    let (endpoint, body) = client.last();
    assert_eq!(endpoint, "playlist/create");
    assert_eq!(body["title"], "Test");
    assert_eq!(body["description"], "desc");
    assert_eq!(body["privacyStatus"], "PRIVATE");
}

#[test]
fn create_playlist_mutation_failed_when_no_playlist_id() {
    let response = serde_json::json!({ "error": "something went wrong" });
    let client = FakePost::new(response);
    let err = block_on(create_playlist(&client, "T", "", "PRIVATE")).expect_err("should fail");
    assert!(
        matches!(&err, ApiError::MutationFailed(msg) if msg == "Playlist was not created"),
        "got: {err:?}"
    );
}

#[test]
fn create_playlist_mutation_failed_on_empty_playlist_id() {
    let response = serde_json::json!({ "playlistId": "" });
    let client = FakePost::new(response);
    let err = block_on(create_playlist(&client, "T", "", "PUBLIC")).expect_err("should fail");
    assert!(
        matches!(&err, ApiError::MutationFailed(msg) if msg == "Playlist was not created"),
        "got: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// add_playlist_items
// ---------------------------------------------------------------------------

#[test]
fn add_playlist_items_posts_correct_actions() {
    let response = serde_json::json!({ "status": "STATUS_SUCCEEDED" });
    let client = FakePost::new(response);
    let ids = vec!["vid1".to_owned(), "vid2".to_owned()];
    block_on(add_playlist_items(&client, "PLtest", &ids)).expect("ok");

    let (endpoint, body) = client.last();
    assert_eq!(endpoint, "browse/edit_playlist");
    assert_eq!(body["playlistId"], "PLtest");

    let actions = body["actions"].as_array().expect("actions array");
    assert_eq!(actions.len(), 2);
    assert_eq!(actions[0]["action"], "ACTION_ADD_VIDEO");
    assert_eq!(actions[0]["addedVideoId"], "vid1");
    assert_eq!(actions[1]["addedVideoId"], "vid2");
}

#[test]
fn add_playlist_items_strips_vl_prefix() {
    let response = serde_json::json!({ "status": "STATUS_SUCCEEDED" });
    let client = FakePost::new(response);
    let ids = vec!["v".to_owned()];
    block_on(add_playlist_items(&client, "VLPLwith_prefix", &ids)).expect("ok");

    let (_, body) = client.last();
    assert_eq!(
        body["playlistId"], "PLwith_prefix",
        "VL prefix must be stripped"
    );
}

#[test]
fn add_playlist_items_mutation_failed_on_non_succeeded() {
    let response = serde_json::json!({ "status": "STATUS_FAILED" });
    let client = FakePost::new(response);
    let ids = vec!["v".to_owned()];
    let err = block_on(add_playlist_items(&client, "PL", &ids)).expect_err("should fail");
    assert!(
        matches!(&err, ApiError::MutationFailed(msg) if msg == "Tracks were not added to the playlist"),
        "got: {err:?}"
    );
}

#[test]
fn add_playlist_items_mutation_failed_when_no_status() {
    let response = serde_json::json!({});
    let client = FakePost::new(response);
    let ids = vec!["v".to_owned()];
    let err = block_on(add_playlist_items(&client, "PL", &ids)).expect_err("should fail");
    assert!(
        matches!(&err, ApiError::MutationFailed(msg) if msg == "Tracks were not added to the playlist"),
        "got: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// remove_playlist_items (two-call flow with MapPost)
// ---------------------------------------------------------------------------

/// Build a minimal playlist browse response containing items with setVideoId
/// embedded in the menu, for the remove_playlist_items fixture.
fn make_playlist_with_set_video_ids() -> serde_json::Value {
    // The fixture needs to match the path stage-1 parse_playlist_tracks walks:
    // twoColumnBrowseResultsRenderer.secondaryContents.sectionListRenderer
    //   .contents[0].musicPlaylistShelfRenderer.contents[].MRLIR
    //
    // Each MRLIR needs:
    //  - flexColumns (for title/videoId resolution via the play button)
    //  - overlay.musicItemThumbnailOverlayRenderer.content.musicPlayButtonRenderer
    //      .playNavigationEndpoint.watchEndpoint.videoId  (PLAY_BUTTON_VIDEO_ID)
    //  - menu.menuRenderer.items[].menuServiceItemRenderer.serviceEndpoint
    //      .playlistEditEndpoint.actions[0].setVideoId
    let make_item = |video_id: &str, set_video_id: &str| {
        serde_json::json!({
            "musicResponsiveListItemRenderer": {
                "flexColumns": [
                    { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [
                        { "text": "Track Title",
                          "navigationEndpoint": {
                            "watchEndpoint": { "videoId": video_id }
                          }
                        }
                    ] } } }
                ],
                "fixedColumns": [
                    { "musicResponsiveListItemFixedColumnRenderer": {
                        "text": { "runs": [{ "text": "3:00" }] }
                    }}
                ],
                "overlay": {
                    "musicItemThumbnailOverlayRenderer": {
                        "content": {
                            "musicPlayButtonRenderer": {
                                "playNavigationEndpoint": {
                                    "watchEndpoint": { "videoId": video_id }
                                }
                            }
                        }
                    }
                },
                "menu": {
                    "menuRenderer": {
                        "items": [
                            {
                                "menuServiceItemRenderer": {
                                    "serviceEndpoint": {
                                        "playlistEditEndpoint": {
                                            "actions": [
                                                {
                                                    "action": "ACTION_REMOVE_VIDEO",
                                                    "removedVideoId": video_id,
                                                    "setVideoId": set_video_id
                                                }
                                            ]
                                        }
                                    }
                                }
                            }
                        ]
                    }
                },
                "thumbnail": {
                    "musicThumbnailRenderer": {
                        "thumbnail": {
                            "thumbnails": [{"url": "https://example.com/thumb.jpg", "width": 60, "height": 60}]
                        }
                    }
                }
            }
        })
    };

    serde_json::json!({
        "contents": {
            "twoColumnBrowseResultsRenderer": {
                "secondaryContents": {
                    "sectionListRenderer": {
                        "contents": [{
                            "musicPlaylistShelfRenderer": {
                                "contents": [
                                    make_item("vid_A", "setVid_AA"),
                                    make_item("vid_B", "setVid_BB"),
                                ]
                            }
                        }]
                    }
                }
            }
        }
    })
}

#[test]
fn remove_playlist_items_two_call_flow() {
    let playlist_response = make_playlist_with_set_video_ids();
    let edit_response = serde_json::json!({ "status": "STATUS_SUCCEEDED" });

    // First call: browse (playlist fetch); second call: browse/edit_playlist
    let client = MapPost::new(&[
        ("browse", playlist_response),
        ("browse/edit_playlist", edit_response),
    ]);

    let ids = vec!["vid_A".to_owned()];
    block_on(remove_playlist_items(&client, "PLtest", &ids)).expect("remove ok");

    let calls = client.calls();
    assert_eq!(calls.len(), 2);

    // Call 1: browse to fetch playlist items.
    assert_eq!(calls[0].0, "browse");
    assert_eq!(calls[0].1["browseId"], "VLPLtest");

    // Call 2: browse/edit_playlist with ACTION_REMOVE_VIDEO.
    assert_eq!(calls[1].0, "browse/edit_playlist");
    assert_eq!(calls[1].1["playlistId"], "PLtest");
    let actions = calls[1].1["actions"].as_array().expect("actions");
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0]["action"], "ACTION_REMOVE_VIDEO");
    assert_eq!(actions[0]["removedVideoId"], "vid_A");
    assert_eq!(actions[0]["setVideoId"], "setVid_AA");
}

#[test]
fn remove_playlist_items_mutation_failed_when_not_found() {
    let playlist_response = make_playlist_with_set_video_ids();
    let client = MapPost::new(&[("browse", playlist_response)]);

    let ids = vec!["vid_NOT_PRESENT".to_owned()];
    let err = block_on(remove_playlist_items(&client, "PL", &ids)).expect_err("should fail");
    assert!(
        matches!(&err, ApiError::MutationFailed(msg) if msg == "Track was not found in the playlist"),
        "got: {err:?}"
    );
}

#[test]
fn remove_playlist_items_mutation_failed_on_edit_failure() {
    let playlist_response = make_playlist_with_set_video_ids();
    let edit_response = serde_json::json!({ "status": "STATUS_FAILED" });
    let client = MapPost::new(&[
        ("browse", playlist_response),
        ("browse/edit_playlist", edit_response),
    ]);
    let ids = vec!["vid_A".to_owned()];
    let err = block_on(remove_playlist_items(&client, "PL", &ids)).expect_err("should fail");
    assert!(
        matches!(&err, ApiError::MutationFailed(msg) if msg == "Tracks were not removed from the playlist"),
        "got: {err:?}"
    );
}

/// Build a multi-page playlist row with embedded setVideoId, for the
/// continuation-aware remove flow test.
fn make_playlist_row_with_set_video_id(video_id: &str, set_video_id: &str) -> serde_json::Value {
    serde_json::json!({
        "musicResponsiveListItemRenderer": {
            "flexColumns": [
                { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [
                    { "text": "Track Title",
                      "navigationEndpoint": {
                        "watchEndpoint": { "videoId": video_id }
                      }
                    }
                ] } } }
            ],
            "fixedColumns": [
                { "musicResponsiveListItemFixedColumnRenderer": {
                    "text": { "runs": [{ "text": "3:00" }] }
                }}
            ],
            "overlay": {
                "musicItemThumbnailOverlayRenderer": {
                    "content": {
                        "musicPlayButtonRenderer": {
                            "playNavigationEndpoint": {
                                "watchEndpoint": { "videoId": video_id }
                            }
                        }
                    }
                }
            },
            "menu": {
                "menuRenderer": {
                    "items": [
                        {
                            "menuServiceItemRenderer": {
                                "serviceEndpoint": {
                                    "playlistEditEndpoint": {
                                        "actions": [
                                            {
                                                "action": "ACTION_REMOVE_VIDEO",
                                                "removedVideoId": video_id,
                                                "setVideoId": set_video_id
                                            }
                                        ]
                                    }
                                }
                            }
                        }
                    ]
                }
            }
        }
    })
}

#[test]
fn remove_playlist_items_finds_track_on_second_page() {
    // Target track sits on the continuation page, not the initial page. The
    // remove flow must walk the continuation chain to resolve setVideoId
    // before posting the edit, otherwise it would falsely report "not found".
    let page1_rows = vec![make_playlist_row_with_set_video_id(
        "vid_first",
        "setVid_FF",
    )];
    let page2_rows = vec![make_playlist_row_with_set_video_id(
        "vid_target",
        "setVid_TT",
    )];

    let mut page1 = serde_json::json!({
        "contents": {
            "twoColumnBrowseResultsRenderer": {
                "secondaryContents": {
                    "sectionListRenderer": {
                        "contents": [{
                            "musicPlaylistShelfRenderer": {
                                "contents": page1_rows,
                                "continuations": [
                                    { "nextContinuationData": {
                                        "continuation": "TOKEN_PAGE_2",
                                        "clickTrackingParams": "ctp"
                                    }}
                                ]
                            }
                        }]
                    }
                }
            }
        }
    });
    // Drop the placeholder None so the type checker keeps the same shape as
    // make_initial_playlist_page when continuations are present.
    let _ = page1["contents"].as_object_mut();
    let page2 = serde_json::json!({
        "continuationContents": {
            "musicPlaylistShelfContinuation": { "contents": page2_rows }
        }
    });
    let edit_response = serde_json::json!({ "status": "STATUS_SUCCEEDED" });
    let client = SequencePost::new(vec![page1, page2, edit_response]);

    let ids = vec!["vid_target".to_owned()];
    block_on(remove_playlist_items(&client, "PLmulti", &ids)).expect("remove ok");

    let calls = client.calls();
    assert_eq!(calls.len(), 3, "initial + continuation + edit");

    // Call 1: initial browse for the playlist.
    assert_eq!(calls[0].0, "browse");
    assert_eq!(calls[0].1["browseId"], "VLPLmulti");

    // Call 2: continuation browse carrying the page-2 token.
    assert_eq!(calls[1].0, "browse");
    assert_eq!(calls[1].1["continuation"], "TOKEN_PAGE_2");

    // Call 3: the edit, with the setVideoId resolved from page 2.
    assert_eq!(calls[2].0, "browse/edit_playlist");
    let actions = calls[2].1["actions"].as_array().expect("actions");
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0]["removedVideoId"], "vid_target");
    assert_eq!(actions[0]["setVideoId"], "setVid_TT");
}

#[test]
fn remove_playlist_items_skips_continuation_when_target_on_page_one() {
    // Optimisation guarantee: when every target videoId is resolved on the
    // initial page, no continuation call is issued — the happy path for small
    // playlists must stay fast.
    let page1_response = make_playlist_with_set_video_ids();
    // Add a continuation token so the loader *would* page if it did not
    // early-exit.
    let mut page1_with_token = page1_response;
    page1_with_token["contents"]["twoColumnBrowseResultsRenderer"]["secondaryContents"]["sectionListRenderer"]
        ["contents"][0]["musicPlaylistShelfRenderer"]["continuations"] = serde_json::json!([
        { "nextContinuationData": { "continuation": "WOULD_BE_FETCHED", "clickTrackingParams": "x" } }
    ]);
    let edit_response = serde_json::json!({ "status": "STATUS_SUCCEEDED" });
    let client = SequencePost::new(vec![page1_with_token, edit_response]);

    let ids = vec!["vid_A".to_owned()];
    block_on(remove_playlist_items(&client, "PLtest", &ids)).expect("ok");

    let calls = client.calls();
    assert_eq!(
        calls.len(),
        2,
        "initial + edit only — no continuation fetched"
    );
    assert_eq!(calls[0].0, "browse");
    assert_eq!(calls[1].0, "browse/edit_playlist");
}

#[test]
fn remove_playlist_items_strips_vl_from_edit_body() {
    let playlist_response = make_playlist_with_set_video_ids();
    let edit_response = serde_json::json!({ "status": "STATUS_SUCCEEDED" });
    let client = MapPost::new(&[
        ("browse", playlist_response),
        ("browse/edit_playlist", edit_response),
    ]);
    let ids = vec!["vid_B".to_owned()];
    block_on(remove_playlist_items(&client, "VLPLtest", &ids)).expect("ok");

    let calls = client.calls();
    // Browse body keeps the VL prefix (needed for browse endpoint).
    assert_eq!(calls[0].1["browseId"], "VLPLtest");
    // Edit body has VL stripped.
    assert_eq!(calls[1].1["playlistId"], "PLtest");
}

// ---------------------------------------------------------------------------
// subscribe_artists / unsubscribe_artists
// ---------------------------------------------------------------------------

#[test]
fn subscribe_artists_posts_correct_endpoint_and_body() {
    let client = FakePost::new(serde_json::json!({}));
    let ids = vec!["UCRB-a6u9flpg0xuBqCf9QlQ".to_owned()];
    block_on(subscribe_artists(&client, &ids)).expect("subscribe ok");
    let (endpoint, body) = client.last();
    assert_eq!(endpoint, "subscription/subscribe");
    let channel_ids = body["channelIds"].as_array().expect("channelIds array");
    assert_eq!(channel_ids.len(), 1);
    assert_eq!(channel_ids[0], "UCRB-a6u9flpg0xuBqCf9QlQ");
}

#[test]
fn subscribe_artists_strips_mpla_prefix() {
    let client = FakePost::new(serde_json::json!({}));
    // Library-artist channel ids carry the MPLA prefix; the request must use
    // the bare UC... form.
    let ids = vec!["MPLAUCRB-a6u9flpg0xuBqCf9QlQ".to_owned()];
    block_on(subscribe_artists(&client, &ids)).expect("subscribe ok");
    let (_, body) = client.last();
    assert_eq!(
        body["channelIds"][0], "UCRB-a6u9flpg0xuBqCf9QlQ",
        "MPLA prefix must be stripped"
    );
}

#[test]
fn subscribe_artists_supports_multiple_channels() {
    let client = FakePost::new(serde_json::json!({}));
    let ids = vec![
        "UCRB-a6u9flpg0xuBqCf9QlQ".to_owned(),
        "MPLAUCf_gP4AMRSgAfyzbkeS9k4g".to_owned(),
    ];
    block_on(subscribe_artists(&client, &ids)).expect("subscribe ok");
    let (_, body) = client.last();
    let channel_ids = body["channelIds"].as_array().expect("channelIds array");
    assert_eq!(channel_ids.len(), 2);
    assert_eq!(channel_ids[0], "UCRB-a6u9flpg0xuBqCf9QlQ");
    // The second id had its MPLA prefix stripped.
    assert_eq!(channel_ids[1], "UCf_gP4AMRSgAfyzbkeS9k4g");
}

#[test]
fn subscribe_artists_propagates_transport_error() {
    let ids = vec!["UC123".to_owned()];
    let err = block_on(subscribe_artists(&FailingPost, &ids)).expect_err("should fail");
    assert!(
        matches!(err, ApiError::Http { status: 500, .. }),
        "got: {err:?}"
    );
}

#[test]
fn unsubscribe_artists_posts_correct_endpoint_and_body() {
    let client = FakePost::new(serde_json::json!({}));
    let ids = vec!["UCRB-a6u9flpg0xuBqCf9QlQ".to_owned()];
    block_on(unsubscribe_artists(&client, &ids)).expect("unsubscribe ok");
    let (endpoint, body) = client.last();
    assert_eq!(endpoint, "subscription/unsubscribe");
    let channel_ids = body["channelIds"].as_array().expect("channelIds array");
    assert_eq!(channel_ids.len(), 1);
    assert_eq!(channel_ids[0], "UCRB-a6u9flpg0xuBqCf9QlQ");
}

#[test]
fn unsubscribe_artists_strips_mpla_prefix() {
    let client = FakePost::new(serde_json::json!({}));
    let ids = vec!["MPLAUCRB-a6u9flpg0xuBqCf9QlQ".to_owned()];
    block_on(unsubscribe_artists(&client, &ids)).expect("unsubscribe ok");
    let (_, body) = client.last();
    assert_eq!(body["channelIds"][0], "UCRB-a6u9flpg0xuBqCf9QlQ");
}

#[test]
fn unsubscribe_artists_propagates_transport_error() {
    let ids = vec!["UC123".to_owned()];
    let err = block_on(unsubscribe_artists(&FailingPost, &ids)).expect_err("should fail");
    assert!(matches!(err, ApiError::Http { status: 500, .. }));
}
