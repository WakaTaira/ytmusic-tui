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
