//! Integration tests against the live YouTube Music API.
//!
//! These are `#[ignore]` by default — they require a valid
//! `~/.config/ytmusic-tui/browser.json` and network access. Run them with:
//!
//! ```sh
//! cargo test -p ytmusic-api -- --ignored
//! ```
//!
//! They never print cookie material.

use std::path::PathBuf;

use ytmusic_api::InnerTubeClient;

/// Resolve the default browser-auth path (`~/.config/ytmusic-tui/browser.json`).
fn default_auth_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config/ytmusic-tui/browser.json"))
}

#[tokio::test]
#[ignore = "hits the live YouTube Music API; requires real browser.json"]
async fn is_session_valid_against_real_api() {
    let path = default_auth_path().expect("HOME is set");
    assert!(
        path.is_file(),
        "expected browser.json at {} — run `ytmusic-tui auth`",
        path.display()
    );

    let client = InnerTubeClient::from_auth_path(&path).expect("client builds from auth file");

    // The canary contract: returns a bool, never panics for auth rot.
    let valid = client.is_session_valid().await;
    // Print only the boolean — never any header/cookie material.
    eprintln!("is_session_valid -> {valid}");
    assert!(
        valid,
        "session reported invalid — cookies may be stale; re-run `ytmusic-tui auth`"
    );
}

#[tokio::test]
#[ignore = "hits the live YouTube Music API; requires real browser.json"]
async fn get_account_info_against_real_api() {
    let path = default_auth_path().expect("HOME is set");
    assert!(
        path.is_file(),
        "expected browser.json at {}",
        path.display()
    );

    let client = InnerTubeClient::from_auth_path(&path).expect("client builds");
    match client.get_account_info().await {
        Ok(info) => {
            // The account name confirms a signed-in session. We deliberately do
            // not assert its value (it is the user's real name); just that the
            // canary parsed a non-empty signed-in structure.
            assert!(!info.account_name.is_empty(), "account name was empty");
            eprintln!("account_info parsed: signed in = true");
        }
        Err(e) => panic!("get_account_info failed: {e}"),
    }
}

/// Build a live client, or skip-by-panic with an actionable message.
fn live_client() -> InnerTubeClient {
    let path = default_auth_path().expect("HOME is set");
    assert!(
        path.is_file(),
        "expected browser.json at {} — run `ytmusic-tui auth`",
        path.display()
    );
    InnerTubeClient::from_auth_path(&path).expect("client builds from auth file")
}

// ---------------------------------------------------------------------------
// M3d-1 endpoint smoke tests against the live API.
//
// These print only public identifiers and titles — never header/cookie material.
// Each asserts structural soundness (non-empty ids/titles), not specific values,
// since the live catalogue changes over time.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "hits the live YouTube Music API; requires real browser.json"]
async fn search_all_against_real_api() {
    let client = live_client();
    let results = client
        .search_all("lofi", 5, None)
        .await
        .expect("default search succeeds");

    let total = results.tracks.len()
        + results.albums.len()
        + results.artists.len()
        + results.playlists.len();
    assert!(total > 0, "default search returned nothing");
    eprintln!(
        "search(default): {} tracks, {} albums, {} artists, {} playlists",
        results.tracks.len(),
        results.albums.len(),
        results.artists.len(),
        results.playlists.len()
    );
    if let Some(t) = results.tracks.first() {
        assert!(!t.video_id.is_empty(), "track videoId empty");
        assert!(!t.title.is_empty(), "track title empty");
        eprintln!("  first track: videoId={} title={:?}", t.video_id, t.title);
    }

    // Filtered search returns only the matching category.
    let songs = client
        .search_all("lofi", 5, Some("songs"))
        .await
        .expect("songs search succeeds");
    assert!(!songs.tracks.is_empty(), "songs filter returned no tracks");
    assert!(songs.albums.is_empty() && songs.artists.is_empty() && songs.playlists.is_empty());
    eprintln!("search(songs): {} tracks", songs.tracks.len());
}

#[tokio::test]
#[ignore = "hits the live YouTube Music API; requires real browser.json"]
async fn get_album_against_real_api() {
    let client = live_client();
    // Discover a real album browseId via search, then fetch it.
    let results = client
        .search_all("lofi", 10, Some("albums"))
        .await
        .expect("albums search succeeds");
    let Some(album_ref) = results.albums.first() else {
        panic!("albums search returned no albums to fetch");
    };
    assert!(
        album_ref.browse_id.starts_with("MPRE"),
        "unexpected album id"
    );

    let album = client
        .get_album(&album_ref.browse_id)
        .await
        .expect("get_album succeeds");
    assert_eq!(album.browse_id, album_ref.browse_id);
    assert!(!album.title.is_empty(), "album title empty");
    assert!(!album.tracks.is_empty(), "album has no tracks");
    for t in &album.tracks {
        assert!(!t.video_id.is_empty(), "album track videoId empty");
    }
    eprintln!(
        "album {}: title={:?} artist={:?} year={:?} tracks={}",
        album.browse_id,
        album.title,
        album.artist,
        album.year,
        album.tracks.len()
    );
}

#[tokio::test]
#[ignore = "hits the live YouTube Music API; requires real browser.json"]
async fn get_artist_against_real_api() {
    let client = live_client();
    // Discover a real channelId via search, then fetch the artist page.
    let results = client
        .search_all("lofi", 10, Some("artists"))
        .await
        .expect("artists search succeeds");
    let Some(artist_ref) = results.artists.first() else {
        panic!("artists search returned no artists to fetch");
    };
    assert!(!artist_ref.channel_id.is_empty(), "artist channelId empty");

    let artist = client
        .get_artist(&artist_ref.channel_id)
        .await
        .expect("get_artist succeeds");
    assert_eq!(artist.channel_id, artist_ref.channel_id);
    assert!(!artist.name.is_empty(), "artist name empty");
    eprintln!(
        "artist {}: name={:?} top_songs={} albums={} related={}",
        artist.channel_id,
        artist.name,
        artist.top_songs.len(),
        artist.albums.len(),
        artist.related_artists.len()
    );
}

#[tokio::test]
#[ignore = "hits the live YouTube Music API; requires real browser.json"]
async fn get_liked_songs_against_real_api() {
    let client = live_client();
    let tracks = client
        .get_liked_songs(10)
        .await
        .expect("get_liked_songs succeeds");
    eprintln!("liked_songs: {} tracks", tracks.len());
    if let Some(t) = tracks.first() {
        assert!(!t.video_id.is_empty(), "liked track videoId empty");
        eprintln!("  first liked: videoId={} title={:?}", t.video_id, t.title);
    }
    assert!(tracks.len() <= 10, "limit exceeded");
}

#[tokio::test]
#[ignore = "hits the live YouTube Music API; requires real browser.json"]
async fn get_playlist_tracks_against_real_api() {
    let client = live_client();
    // Discover a real playlistId via search, then fetch its tracks.
    let results = client
        .search_all("lofi", 10, Some("playlists"))
        .await
        .expect("playlists search succeeds");
    let Some(playlist_ref) = results.playlists.first() else {
        panic!("playlists search returned no playlists to fetch");
    };
    assert!(!playlist_ref.playlist_id.is_empty(), "playlistId empty");

    let tracks = client
        .get_playlist_tracks(&playlist_ref.playlist_id)
        .await
        .expect("get_playlist_tracks succeeds");
    // A non-empty playlist should yield tracks; print the count and first id.
    eprintln!(
        "playlist {}: {} tracks",
        playlist_ref.playlist_id,
        tracks.len()
    );
    if let Some(t) = tracks.first() {
        assert!(!t.video_id.is_empty(), "playlist track videoId empty");
        eprintln!("  first track: videoId={} title={:?}", t.video_id, t.title);
    }
}

// ---------------------------------------------------------------------------
// M3d-2 endpoint smoke tests against the live API (library / history / home /
// radio / lyrics). Public ids/titles only — never header/cookie material.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "hits the live YouTube Music API; requires real browser.json"]
async fn get_library_playlists_against_real_api() {
    let client = live_client();
    let playlists = client
        .get_library_playlists(25)
        .await
        .expect("get_library_playlists succeeds");
    eprintln!("library_playlists: {}", playlists.len());
    for p in playlists.iter().take(3) {
        assert!(!p.playlist_id.is_empty(), "playlistId empty");
        eprintln!("  {} ({} tracks)", p.title, p.track_count);
    }
}

#[tokio::test]
#[ignore = "hits the live YouTube Music API; requires real browser.json"]
async fn get_library_albums_against_real_api() {
    let client = live_client();
    let albums = client
        .get_library_albums(25)
        .await
        .expect("get_library_albums succeeds");
    eprintln!("library_albums: {}", albums.len());
    for a in albums.iter().take(3) {
        assert!(
            a.browse_id.starts_with("MPRE"),
            "album id {:?}",
            a.browse_id
        );
        eprintln!("  {} — {} ({})", a.title, a.artist, a.year);
    }
}

#[tokio::test]
#[ignore = "hits the live YouTube Music API; requires real browser.json"]
async fn get_library_artists_against_real_api() {
    let client = live_client();
    let artists = client
        .get_library_artists(25)
        .await
        .expect("get_library_artists succeeds");
    eprintln!("library_artists: {}", artists.len());
    for a in artists.iter().take(3) {
        assert!(!a.channel_id.is_empty(), "channelId empty");
        assert!(!a.name.is_empty(), "artist name empty");
        eprintln!("  {} [{}]", a.name, a.channel_id);
    }
}

#[tokio::test]
#[ignore = "hits the live YouTube Music API; requires real browser.json"]
async fn get_history_against_real_api() {
    let client = live_client();
    let tracks = client.get_history().await.expect("get_history succeeds");
    eprintln!("history: {} tracks", tracks.len());
    for t in tracks.iter().take(3) {
        assert!(!t.video_id.is_empty(), "history track videoId empty");
        eprintln!("  videoId={} title={:?}", t.video_id, t.title);
    }
}

#[tokio::test]
#[ignore = "hits the live YouTube Music API; requires real browser.json"]
async fn get_home_against_real_api() {
    use ytmusic_api::HomeSectionItem;
    let client = live_client();
    let sections = client.get_home().await.expect("get_home succeeds");
    assert!(!sections.is_empty(), "home returned no sections");
    eprintln!("home: {} sections", sections.len());
    for s in &sections {
        let (tracks, playlists) =
            s.items
                .iter()
                .fold((0usize, 0usize), |(t, p), item| match item {
                    HomeSectionItem::Track(_) => (t + 1, p),
                    HomeSectionItem::Playlist(_) => (t, p + 1),
                });
        eprintln!(
            "  {:?}: {} items ({} tracks, {} playlists)",
            s.title,
            s.items.len(),
            tracks,
            playlists
        );
    }
}

#[tokio::test]
#[ignore = "hits the live YouTube Music API; requires real browser.json"]
async fn get_radio_against_real_api() {
    let client = live_client();
    // A stable public seed (Rick Astley — "Never Gonna Give You Up").
    let tracks = client
        .get_radio("dQw4w9WgXcQ", 10)
        .await
        .expect("get_radio succeeds");
    assert!(!tracks.is_empty(), "radio returned no tracks");
    assert_eq!(
        tracks[0].video_id, "dQw4w9WgXcQ",
        "seed track should be first"
    );
    eprintln!("radio[dQw4w9WgXcQ]: {} tracks", tracks.len());
    eprintln!("  seed: {:?} — {:?}", tracks[0].title, tracks[0].artist);
    assert!(tracks.len() <= 10, "limit exceeded");
}

#[tokio::test]
#[ignore = "hits the live YouTube Music API; requires real browser.json"]
async fn get_lyrics_against_real_api() {
    let client = live_client();
    // "Bohemian Rhapsody" (bSnlKl_PoQU) has a stable public lyrics page.
    let lyrics = client
        .get_lyrics("bSnlKl_PoQU")
        .await
        .expect("get_lyrics succeeds")
        .expect("Bohemian Rhapsody has lyrics");
    assert!(
        lyrics.starts_with("Is this the real life?"),
        "unexpected lyrics opener"
    );
    eprintln!("lyrics[bSnlKl_PoQU]: {} chars", lyrics.chars().count());

    // A track with no lyrics yields Ok(None), never an error (absence is a value).
    let none = client
        .get_lyrics("dQw4w9WgXcQ")
        .await
        .expect("no-lyrics is not an error");
    eprintln!("lyrics[dQw4w9WgXcQ]: present = {}", none.is_some());
}
