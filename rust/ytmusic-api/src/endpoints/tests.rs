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
    PostRequest, get_album, get_artist, get_history, get_home, get_library_albums,
    get_library_artists, get_library_playlists, get_liked_songs, get_lyrics, get_playlist_tracks,
    get_radio, search_all,
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
    assert_eq!(tracks[1].artist, "dankeサン");
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
    assert_eq!(tracks[0].artist, "Lana Nealsen, 6.7M views");
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
    assert_eq!(tracks[2].artist, "Sun Levi");
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
