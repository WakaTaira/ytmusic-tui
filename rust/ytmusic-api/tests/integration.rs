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

// ---------------------------------------------------------------------------
// M3d-3 mutation smoke tests against the live API.
//
// Protocol:
//  (a) get_like_status — read-only, no state change.
//  (b) rate_track — read current status first, then set the SAME value.
//  (c) Full playlist lifecycle in ONE test: create → add → remove → delete.
//      The account is left clean after the test.
//
// These never print cookie material.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "hits the live YouTube Music API; requires real browser.json"]
async fn get_like_status_against_real_api() {
    let client = live_client();
    // "Never Gonna Give You Up" — a stable public videoId.
    // The status may be any of LIKE / INDIFFERENT / DISLIKE (or None).
    let status = client
        .get_like_status("dQw4w9WgXcQ")
        .await
        .expect("get_like_status succeeds");
    eprintln!("like_status[dQw4w9WgXcQ]: {status:?}");
    // The value is optional but the call must succeed.
    match &status {
        Some(s) => assert!(
            matches!(s.as_str(), "LIKE" | "INDIFFERENT" | "DISLIKE"),
            "unexpected likeStatus value: {s:?}"
        ),
        None => eprintln!("  (status not available — watch panel did not return it)"),
    }
}

#[tokio::test]
#[ignore = "hits the live YouTube Music API; requires real browser.json"]
async fn rate_track_idempotent_against_real_api() {
    let client = live_client();
    let video_id = "dQw4w9WgXcQ";

    // Step 1: read current status.
    let current = client
        .get_like_status(video_id)
        .await
        .expect("get_like_status succeeds");
    eprintln!("rate_track: current likeStatus = {current:?}");

    // Step 2: set the same status (idempotent — no observable state change).
    // If status is None, default to INDIFFERENT (safest no-op).
    let status_to_set = current.as_deref().unwrap_or("INDIFFERENT");
    client
        .rate_track(video_id, status_to_set)
        .await
        .expect("rate_track succeeds");
    eprintln!("rate_track: set status={status_to_set:?} on {video_id} (idempotent)");

    // Step 3: confirm the status is still the same.
    let after = client
        .get_like_status(video_id)
        .await
        .expect("get_like_status after rate_track succeeds");
    eprintln!("rate_track: after likeStatus = {after:?}");
    assert_eq!(
        after.as_deref(),
        Some(status_to_set),
        "status changed unexpectedly after idempotent set"
    );
}

#[tokio::test]
#[ignore = "hits the live YouTube Music API; requires real browser.json; MUTATES then restores account state"]
async fn playlist_lifecycle_against_real_api() {
    // Use the audio-native track (lYBUbBu4W08), NOT the music video (dQw4w9WgXcQ).
    // YouTube Music silently substitutes music-video IDs with their audio
    // counterpart when adding to a playlist, so the videoId that appears in
    // `get_playlist_tracks` will be lYBUbBu4W08 regardless of which one was
    // requested.  Using the audio track directly keeps the poll and remove logic
    // straightforward — the same videoId that was added is the one visible in
    // the playlist and the one remove_playlist_items searches for.
    const TEST_VIDEO_ID: &str = "lYBUbBu4W08"; // Rick Astley — Never Gonna Give You Up (audio)

    let client = live_client();

    // --- Step 1: create a temporary playlist. ---
    let title = "rust-spike-tmp-M3d3-lifecycle";
    let playlist_id = client
        .create_playlist(title, "Temporary test playlist — safe to delete", "PRIVATE")
        .await
        .expect("create_playlist succeeds");
    assert!(
        !playlist_id.is_empty(),
        "create_playlist returned empty playlistId"
    );
    eprintln!("playlist_lifecycle: created playlist {playlist_id:?}");

    // Run add → poll → remove in a block so we can always delete afterwards.
    // YouTube Music's eventual consistency means add_playlist_items succeeds
    // before the track is visible to get_playlist_tracks; poll until it appears.
    let video_ids = vec![TEST_VIDEO_ID.to_owned()];
    let middle_result: Result<(), String> = async {
        // --- Step 2: add one item. ---
        client
            .add_playlist_items(&playlist_id, &video_ids)
            .await
            .map_err(|e| format!("add_playlist_items failed: {e}"))?;
        eprintln!("playlist_lifecycle: added {TEST_VIDEO_ID} to {playlist_id}");

        // --- Step 2b: poll until the added track is visible. ---
        // YouTube Music has multi-second eventual consistency on newly-created
        // playlists: the track may not appear immediately after add_playlist_items
        // reports success. Poll with a generous budget.
        const MAX_ATTEMPTS: u32 = 10;
        const POLL_SLEEP_SECS: u64 = 3;
        let mut appeared = false;
        for attempt in 1..=MAX_ATTEMPTS {
            tokio::time::sleep(tokio::time::Duration::from_secs(POLL_SLEEP_SECS)).await;
            let tracks = client
                .get_playlist_tracks(&playlist_id)
                .await
                .map_err(|e| format!("get_playlist_tracks (poll attempt {attempt}) failed: {e}"))?;
            if tracks.iter().any(|t| t.video_id == TEST_VIDEO_ID) {
                eprintln!(
                    "playlist_lifecycle: track appeared after attempt {attempt} / {MAX_ATTEMPTS}"
                );
                appeared = true;
                break;
            }
            eprintln!(
                "playlist_lifecycle: track not yet visible, {} tracks returned \
                 (attempt {attempt}/{MAX_ATTEMPTS}), waiting {POLL_SLEEP_SECS}s …",
                tracks.len()
            );
        }
        if !appeared {
            return Err(format!(
                "added track {TEST_VIDEO_ID} never appeared in playlist {playlist_id} \
                 after {MAX_ATTEMPTS} attempts ({} s total)",
                MAX_ATTEMPTS * POLL_SLEEP_SECS as u32
            ));
        }

        // --- Step 3: remove the item. ---
        client
            .remove_playlist_items(&playlist_id, &video_ids)
            .await
            .map_err(|e| format!("remove_playlist_items failed: {e}"))?;
        eprintln!("playlist_lifecycle: removed {TEST_VIDEO_ID} from {playlist_id}");

        Ok(())
    }
    .await;

    // --- Step 4: ALWAYS delete the playlist (leave account clean). ---
    let delete_result = client.delete_playlist(&playlist_id).await;
    match &delete_result {
        Ok(()) => eprintln!("playlist_lifecycle: deleted {playlist_id}"),
        Err(e) => {
            eprintln!("playlist_lifecycle: WARNING — delete_playlist({playlist_id}) failed: {e}")
        }
    }

    // Now surface any failure from the middle steps.
    if let Err(msg) = middle_result {
        panic!("playlist_lifecycle failed (playlist {playlist_id} was still deleted): {msg}");
    }
    delete_result.expect("delete_playlist succeeds");
    eprintln!("playlist_lifecycle: account left clean — test complete");
}
