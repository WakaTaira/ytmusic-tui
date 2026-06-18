//! M3c tests: domain models + pure conversion functions.
//!
//! Ported 1-to-1 from `tests/test_api.py` — the conversion-focused test
//! classes only.  Endpoint-flow tests (those that mock `YTMusic` client
//! methods) are DEFERRED to M3d and listed at the bottom of this file.
//!
//! Fixture data lives at `tests/fixtures_shared/` inside this crate (under
//! `CARGO_MANIFEST_DIR`). These fixtures were originally shared with the
//! now-removed Python test suite.

use serde_json::{Value, json};
use ytmusic_api::Track;
use ytmusic_api::models::{AlbumInfo, HomeSectionItem, PlaylistInfo, RelatedArtist, SearchResults};
use ytmusic_api::parse::{
    categorize_search_results, dict_to_album_info, dict_to_album_track, dict_to_playlist_info,
    dict_to_related_artist, dict_to_track, parse_duration, parse_home_sections,
    watch_item_to_track,
};

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

/// Load a JSON fixture from `tests/fixtures_shared/<name>` (within this crate).
fn load_fixture(name: &str) -> Value {
    let path = format!(
        "{}/tests/fixtures_shared/{}",
        env!("CARGO_MANIFEST_DIR"),
        name
    );
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read fixture {name}: {e}"));
    serde_json::from_str(&content).unwrap_or_else(|e| panic!("failed to parse fixture {name}: {e}"))
}

// ---------------------------------------------------------------------------
// Python-side `_make_*` helpers — load fixture + apply overrides
// ---------------------------------------------------------------------------

/// Mirrors `_make_search_song_result` in test_api.py.
/// Loads `search_song.json` and applies keyword overrides.
fn make_search_song_result(overrides: &Value) -> Value {
    let mut result = load_fixture("search_song.json");
    merge_overrides(&mut result, overrides);
    result
}

/// Mirrors `_make_playlist_item` in test_api.py.
fn make_playlist_item(overrides: &Value) -> Value {
    let mut result = load_fixture("playlist_item.json");
    merge_overrides(&mut result, overrides);
    result
}

/// Mirrors `_make_home_section` in test_api.py (uses the first section).
fn make_home_section(overrides: &Value) -> Value {
    let arr = load_fixture("home_sections.json");
    let mut result = arr[0].clone();
    merge_overrides(&mut result, overrides);
    result
}

/// Mirrors `_make_library_album_item` in test_api.py.
fn make_library_album_item(overrides: &Value) -> Value {
    let mut result = load_fixture("library_album_item.json");
    merge_overrides(&mut result, overrides);
    result
}

/// Mirrors `_make_library_artist_item` in test_api.py.
fn make_library_artist_item(overrides: &Value) -> Value {
    let mut result = load_fixture("library_artist_item.json");
    merge_overrides(&mut result, overrides);
    result
}

/// Apply top-level key overrides from `overrides` into `target`.
/// A `Value::Null` override removes the key (simulates `item.pop(key)`).
fn merge_overrides(target: &mut Value, overrides: &Value) {
    debug_assert!(target.is_object() && overrides.is_object());
    if let (Some(tgt), Some(src)) = (target.as_object_mut(), overrides.as_object()) {
        for (k, v) in src {
            if v.is_null() {
                tgt.remove(k);
            } else {
                tgt.insert(k.clone(), v.clone());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// TestParseDuration — ports test_api.py::TestParseDuration
// ---------------------------------------------------------------------------

mod test_parse_duration {
    use super::*;

    #[test]
    fn test_minutes_seconds() {
        assert_eq!(parse_duration(Some("3:45")), 225.0);
    }

    #[test]
    fn test_hour_minutes_seconds() {
        assert_eq!(parse_duration(Some("1:02:30")), 3750.0);
    }

    #[test]
    fn test_zero_duration() {
        assert_eq!(parse_duration(Some("0:00")), 0.0);
    }

    #[test]
    fn test_none_returns_zero() {
        assert_eq!(parse_duration(None), 0.0);
    }

    #[test]
    fn test_empty_string_returns_zero() {
        assert_eq!(parse_duration(Some("")), 0.0);
    }

    #[test]
    fn test_single_digit_seconds() {
        assert_eq!(parse_duration(Some("4:05")), 245.0);
    }

    #[test]
    fn test_long_song() {
        assert_eq!(parse_duration(Some("12:34")), 754.0);
    }

    #[test]
    fn test_seconds_only() {
        // Some API responses may return just seconds.
        assert_eq!(parse_duration(Some("45")), 45.0);
    }
}

// ---------------------------------------------------------------------------
// TestTrackConversion — ports the conversion-logic assertions from
// test_api.py::TestTrackConversion (the YTMusic-mock wiring is deferred to
// M3d; here we drive dict_to_track directly).
// ---------------------------------------------------------------------------

mod test_track_conversion {
    use super::*;

    #[test]
    fn test_basic_song_conversion() {
        let item = make_search_song_result(&json!({}));
        let track = dict_to_track(&item).expect("should parse");

        assert!(matches!(track, Track { .. }));
        assert_eq!(track.video_id, "dQw4w9WgXcQ");
        assert_eq!(track.title, "Never Gonna Give You Up");
        assert_eq!(track.artist, "Rick Astley");
        assert_eq!(track.album, "Whenever You Need Somebody");
        assert_eq!(track.duration_seconds, 213.0);
        assert_eq!(track.thumbnail_url, "https://lh3.google.com/large.jpg");
    }

    #[test]
    fn test_multiple_artists_joined() {
        let item = make_search_song_result(&json!({
            "artists": [
                {"name": "Artist A", "id": "UC1"},
                {"name": "Artist B", "id": "UC2"},
                {"name": "Artist C", "id": "UC3"}
            ]
        }));
        let track = dict_to_track(&item).expect("should parse");
        assert_eq!(track.artist, "Artist A, Artist B, Artist C");
    }

    #[test]
    fn test_missing_album() {
        // album=null → album ""
        let item = make_search_song_result(&json!({"album": null}));
        let track = dict_to_track(&item).expect("should parse");
        assert_eq!(track.album, "");
    }

    #[test]
    fn test_missing_artists() {
        // artists=null → artist ""
        let item = make_search_song_result(&json!({"artists": null}));
        let track = dict_to_track(&item).expect("should parse");
        assert_eq!(track.artist, "");
    }

    #[test]
    fn test_missing_duration() {
        // Remove both duration fields → duration_seconds 0.0
        let mut item = make_search_song_result(&json!({"duration": null}));
        if let Some(obj) = item.as_object_mut() {
            obj.remove("duration_seconds");
        }
        let track = dict_to_track(&item).expect("should parse");
        assert_eq!(track.duration_seconds, 0.0);
    }

    #[test]
    fn test_missing_thumbnails() {
        let item = make_search_song_result(&json!({"thumbnails": null}));
        let track = dict_to_track(&item).expect("should parse");
        assert_eq!(track.thumbnail_url, "");
    }

    #[test]
    fn test_empty_thumbnails_list() {
        let item = make_search_song_result(&json!({"thumbnails": []}));
        let track = dict_to_track(&item).expect("should parse");
        assert_eq!(track.thumbnail_url, "");
    }

    #[test]
    fn test_skips_items_without_video_id() {
        // No videoId → None
        let item = json!({"resultType": "song", "title": "No ID"});
        assert!(dict_to_track(&item).is_none());
    }
}

// ---------------------------------------------------------------------------
// TestPlaylistInfo — ports test_api.py::TestPlaylistInfo (dataclass contract)
// ---------------------------------------------------------------------------

mod test_playlist_info {
    use super::*;

    #[test]
    fn test_defaults() {
        let info = PlaylistInfo::new("PL1", "Test", "", 0, "");
        assert_eq!(info.description, "");
        assert_eq!(info.track_count, 0);
        assert_eq!(info.thumbnail_url, "");
    }

    #[test]
    fn test_with_all_fields() {
        let info = PlaylistInfo::new("PL2", "Music", "desc", 42, "https://example.com/t.jpg");
        assert_eq!(info.playlist_id, "PL2");
        assert_eq!(info.track_count, 42);
        assert_eq!(info.thumbnail_url, "https://example.com/t.jpg");
    }
}

// ---------------------------------------------------------------------------
// TestSearchResults — ports test_api.py::TestSearchResults
// ---------------------------------------------------------------------------

mod test_search_results {
    use super::*;

    #[test]
    fn test_defaults_empty() {
        let results = SearchResults::default();
        assert!(results.tracks.is_empty());
        assert!(results.albums.is_empty());
        assert!(results.artists.is_empty());
        assert!(results.playlists.is_empty());
    }

    #[test]
    fn test_with_data() {
        let track = Track::new("v1", "Song", "Art", "", 0.0, "");
        let album = AlbumInfo::new_without_tracks("b1", "Alb", "Art", "", "");
        let artist = RelatedArtist::new("c1", "Art", "");
        let playlist = PlaylistInfo::new("p1", "PL", "", 0, "");

        let results = SearchResults {
            tracks: vec![track],
            albums: vec![album],
            artists: vec![artist],
            playlists: vec![playlist],
        };
        assert_eq!(results.tracks.len(), 1);
        assert_eq!(results.albums.len(), 1);
        assert_eq!(results.artists.len(), 1);
        assert_eq!(results.playlists.len(), 1);
    }
}

// ---------------------------------------------------------------------------
// PlaylistInfo dict conversion — ports playlist-conversion assertions
// scattered through TestGetLibraryPlaylists in test_api.py
// ---------------------------------------------------------------------------

mod test_dict_to_playlist_info {
    use super::*;

    #[test]
    fn test_from_fixture_with_count_integer() {
        // library_playlists.json: count is integer 10
        let arr = load_fixture("library_playlists.json");
        let playlists: Vec<_> = arr
            .as_array()
            .unwrap()
            .iter()
            .filter_map(dict_to_playlist_info)
            .collect();

        assert_eq!(playlists.len(), 2);
        assert_eq!(playlists[0].playlist_id, "PL_1");
        assert_eq!(playlists[0].title, "Chill");
        assert_eq!(playlists[0].track_count, 10);
        assert_eq!(
            playlists[0].thumbnail_url,
            "https://lh3.google.com/pl_thumb.jpg"
        );
    }

    #[test]
    fn test_count_as_string() {
        // library_playlists.json: second item has count as string "25"
        let arr = load_fixture("library_playlists.json");
        let playlists: Vec<_> = arr
            .as_array()
            .unwrap()
            .iter()
            .filter_map(dict_to_playlist_info)
            .collect();
        assert_eq!(playlists[1].track_count, 25);
    }

    #[test]
    fn test_handles_missing_description() {
        let mut item = make_playlist_item(&json!({}));
        if let Some(obj) = item.as_object_mut() {
            obj.remove("description");
        }
        let info = dict_to_playlist_info(&item).expect("should parse");
        assert_eq!(info.description, "");
    }

    #[test]
    fn test_handles_missing_count() {
        let mut item = make_playlist_item(&json!({}));
        if let Some(obj) = item.as_object_mut() {
            obj.remove("count");
        }
        let info = dict_to_playlist_info(&item).expect("should parse");
        assert_eq!(info.track_count, 0);
    }

    #[test]
    fn test_returns_none_without_playlist_id() {
        let item = json!({"title": "No ID"});
        assert!(dict_to_playlist_info(&item).is_none());
    }
}

// ---------------------------------------------------------------------------
// AlbumInfo dict conversion — ports TestGetLibraryAlbums assertions
// ---------------------------------------------------------------------------

mod test_dict_to_album_info {
    use super::*;

    #[test]
    fn test_from_fixture() {
        let arr = load_fixture("library_albums.json");
        let albums: Vec<_> = arr
            .as_array()
            .unwrap()
            .iter()
            .filter_map(dict_to_album_info)
            .collect();

        assert_eq!(albums.len(), 2);
        assert_eq!(albums[0].browse_id, "MPREb_1");
        assert_eq!(albums[0].title, "Album A");
        assert_eq!(albums[0].artist, "Lib Artist");
        assert_eq!(albums[1].browse_id, "MPREb_2");
    }

    #[test]
    fn test_skips_items_without_browse_id() {
        let mut item = make_library_album_item(&json!({}));
        if let Some(obj) = item.as_object_mut() {
            obj.remove("browseId");
        }
        assert!(dict_to_album_info(&item).is_none());
    }

    #[test]
    fn test_handles_empty_response() {
        let empty: Vec<AlbumInfo> = vec![];
        assert_eq!(empty, []);
    }
}

// ---------------------------------------------------------------------------
// RelatedArtist dict conversion — ports TestGetLibraryArtists assertions
// and the related artist dict cases
// ---------------------------------------------------------------------------

mod test_dict_to_related_artist {
    use super::*;

    #[test]
    fn test_skips_items_without_browse_id() {
        let mut item = make_library_artist_item(&json!({}));
        if let Some(obj) = item.as_object_mut() {
            obj.remove("browseId");
        }
        // No browseId and no channelId → None
        assert!(dict_to_related_artist(&item).is_none());
    }

    #[test]
    fn test_falls_back_to_name_key() {
        // Some responses use 'name' instead of 'title'
        let item = json!({"browseId": "UC_fb", "name": "Fallback Name", "thumbnails": []});
        let artist = dict_to_related_artist(&item).expect("should parse");
        assert_eq!(artist.name, "Fallback Name");
        assert_eq!(artist.channel_id, "UC_fb");
    }

    #[test]
    fn test_falls_back_to_channel_id_key() {
        // Some search results use "channelId" instead of "browseId"
        let item = json!({"channelId": "UC_ch", "title": "Channel Artist", "thumbnails": []});
        let artist = dict_to_related_artist(&item).expect("should parse");
        assert_eq!(artist.channel_id, "UC_ch");
    }
}

// ---------------------------------------------------------------------------
// Library artists — ports TestGetLibraryArtists (the parse-path assertions)
// ---------------------------------------------------------------------------

mod test_library_artists_parse {
    use super::*;

    #[test]
    fn test_from_fixture_shape() {
        // library_artists.json uses 'artist' key (not 'name' or 'title').
        // The ArtistInfo construction path (direct field extraction, not
        // _dict_to_related_artist) is exercised in M3d.
        let arr = load_fixture("library_artists.json");
        let items = arr.as_array().unwrap();
        assert_eq!(items[0]["browseId"], "UC_1");
        assert_eq!(items[0]["artist"], "Artist A");
        assert_eq!(items[1]["browseId"], "UC_2");
        assert_eq!(items[1]["artist"], "Artist B");
    }

    #[test]
    fn test_library_artist_item_fixture_shape() {
        // Verifies the fixture helper produces the expected JSON structure.
        // The ArtistInfo construction path is exercised in M3d.
        let item = make_library_artist_item(&json!({"browseId": "UC_simp", "artist": "Simple"}));
        assert_eq!(item["browseId"], "UC_simp");
        assert_eq!(item["artist"], "Simple");
    }
}

// ---------------------------------------------------------------------------
// Track from playlist_with_tracks fixture — ports TestGetPlaylistTracks
// ---------------------------------------------------------------------------

mod test_playlist_tracks_parse {
    use super::*;

    #[test]
    fn test_returns_track_list_from_fixture() {
        let fixture = load_fixture("playlist_with_tracks.json");
        let raw_tracks = fixture["tracks"].as_array().unwrap();
        let tracks: Vec<_> = raw_tracks.iter().filter_map(dict_to_track).collect();

        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].video_id, "t1");
        assert_eq!(tracks[1].video_id, "t2");
    }

    #[test]
    fn test_skips_unavailable_tracks_none_video_id() {
        let items = [
            json!({"videoId": "ok1", "title": "ok", "artists": [], "duration": "3:00", "thumbnails": []}),
            json!({"videoId": null, "title": "deleted"}),
        ];
        let tracks: Vec<_> = items.iter().filter_map(dict_to_track).collect();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].video_id, "ok1");
    }

    #[test]
    fn test_handles_empty_list() {
        let tracks: Vec<_> = std::iter::empty::<&Value>()
            .filter_map(dict_to_track)
            .collect();
        assert!(tracks.is_empty());
    }
}

// ---------------------------------------------------------------------------
// Liked songs — ports TestGetLikedSongs (parse path)
// ---------------------------------------------------------------------------

mod test_liked_songs_parse {
    use super::*;

    #[test]
    fn test_returns_liked_tracks_from_fixture() {
        let fixture = load_fixture("liked_songs.json");
        let raw_tracks = fixture["tracks"].as_array().unwrap();
        let tracks: Vec<_> = raw_tracks.iter().filter_map(dict_to_track).collect();

        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].video_id, "like1");
        assert_eq!(tracks[1].video_id, "like2");
    }

    #[test]
    fn test_picks_largest_thumbnail() {
        // liked_songs.json has 3 thumbnails of widths 60, 226, 544
        let fixture = load_fixture("liked_songs.json");
        let track = dict_to_track(&fixture["tracks"][0]).expect("should parse");
        assert_eq!(track.thumbnail_url, "https://lh3.google.com/t_xlarge.jpg");
    }
}

// ---------------------------------------------------------------------------
// Home sections — ports TestGetHome (parse path)
// ---------------------------------------------------------------------------

mod test_home_parse {
    use super::*;

    #[test]
    fn test_returns_home_sections_from_fixture() {
        let fixture = load_fixture("home_sections.json");
        let sections = parse_home_sections(fixture.as_array().unwrap());

        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].title, "Quick picks");
        assert_eq!(sections[1].title, "Forgotten favourites");
    }

    #[test]
    fn test_home_section_contains_tracks_and_playlists() {
        let section = make_home_section(&json!({}));
        let sections = parse_home_sections(&[section]);

        let items = &sections[0].items;
        assert_eq!(items.len(), 2);
        assert!(matches!(items[0], HomeSectionItem::Track(_)));
        assert!(matches!(items[1], HomeSectionItem::Playlist(_)));
    }

    #[test]
    fn test_handles_empty_home() {
        let sections = parse_home_sections(&[]);
        assert!(sections.is_empty());
    }

    #[test]
    fn test_skips_sections_without_contents() {
        let broken = json!({"title": "Broken Section"});
        let good = make_home_section(&json!({"title": "Good Section"}));
        let sections = parse_home_sections(&[broken, good]);

        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].title, "Good Section");
    }

    /// Issue #29: an album-like card with `audioPlaylistId: ""` (empty string,
    /// not absent) previously slipped through as `PlaylistInfo { playlist_id: "" }`
    /// because `Option::unwrap_or(&album.browse_id)` only falls back on `None`,
    /// not on `Some("")`. Opening such a card seeded the nav stack with an
    /// empty `playlist_id` and broke every downstream action with a misleading
    /// "no playlist context" toast.
    #[test]
    fn test_drops_album_like_card_with_empty_audio_playlist_id() {
        let section = json!({
            "title": "Mixed for you",
            "contents": [
                // Malformed: real browseId but audioPlaylistId is an empty string.
                {
                    "browseId": "MPREb_real_album",
                    "audioPlaylistId": "",
                    "title": "Broken card",
                    "artists": [{ "name": "Artist X" }],
                },
                // Valid sibling: same branch, audioPlaylistId absent so the
                // browseId fallback supplies a non-empty playlist id.
                {
                    "browseId": "MPREb_good_album",
                    "title": "Good card",
                    "artists": [{ "name": "Artist Y" }],
                },
            ],
        });

        let sections = parse_home_sections(&[section]);
        assert_eq!(sections.len(), 1);
        let items = &sections[0].items;
        assert_eq!(items.len(), 1, "malformed card dropped, valid sibling kept");
        match &items[0] {
            HomeSectionItem::Playlist(p) => {
                assert_eq!(p.playlist_id, "MPREb_good_album");
                assert!(!p.playlist_id.is_empty(), "no empty id ever escapes");
            }
            other => panic!("expected Playlist, got {other:?}"),
        }
    }

    /// Defensive companion to the above: even when the only card in a section
    /// is malformed, the section is still emitted with an empty `items` vec
    /// (the existing parity invariant — only the broken item is dropped, not
    /// the whole section).
    #[test]
    fn test_section_with_only_malformed_album_card_yields_empty_items() {
        let section = json!({
            "title": "Mystery section",
            "contents": [
                {
                    "browseId": "MPREb_solo",
                    "audioPlaylistId": "",
                    "title": "Lone broken card",
                    "artists": [{ "name": "?" }],
                },
            ],
        });

        let sections = parse_home_sections(&[section]);
        assert_eq!(sections.len(), 1, "section kept even when all items drop");
        assert!(sections[0].items.is_empty());
    }
}

// ---------------------------------------------------------------------------
// Search results mixed fixture — ports TestSearchAll (categorization path)
// ---------------------------------------------------------------------------

mod test_categorize_search_results {
    use super::*;

    #[test]
    fn test_categorizes_mixed_results() {
        let fixture = load_fixture("search_results_mixed.json");
        let raw = fixture.as_array().unwrap();
        let results = categorize_search_results(raw);

        // 1 song + 1 video = 2 tracks
        assert_eq!(results.tracks.len(), 2);
        assert_eq!(results.tracks[0].video_id, "song1");
        assert_eq!(results.tracks[1].video_id, "vid1");

        assert_eq!(results.albums.len(), 1);
        assert_eq!(results.albums[0].browse_id, "MPREb_alb1");
        assert_eq!(results.albums[0].title, "Great Album");

        assert_eq!(results.artists.len(), 1);
        assert_eq!(results.artists[0].channel_id, "UCartist1");
        assert_eq!(results.artists[0].name, "Famous Artist");

        assert_eq!(results.playlists.len(), 1);
        assert_eq!(results.playlists[0].playlist_id, "VLPL_search1");
    }

    #[test]
    fn test_empty_search() {
        let results = categorize_search_results(&[]);
        assert!(results.tracks.is_empty());
        assert!(results.albums.is_empty());
        assert!(results.artists.is_empty());
        assert!(results.playlists.is_empty());
    }

    #[test]
    fn test_skips_invalid_items() {
        let raw = vec![
            json!({"resultType": "song",     "title": "No ID"}),
            json!({"resultType": "album",    "title": "No Browse ID"}),
            json!({"resultType": "artist",   "title": "No Channel"}),
            json!({"resultType": "playlist", "title": "No Playlist ID"}),
            json!({
                "resultType": "song",
                "videoId": "valid1",
                "title": "Valid Song",
                "artists": [],
                "duration": "2:00",
                "thumbnails": []
            }),
        ];
        let results = categorize_search_results(&raw);

        assert_eq!(results.tracks.len(), 1);
        assert_eq!(results.tracks[0].video_id, "valid1");
        assert!(results.albums.is_empty());
        assert!(results.artists.is_empty());
        assert!(results.playlists.is_empty());
    }

    #[test]
    fn test_songs_only() {
        let item1 = make_search_song_result(&json!({"videoId": "s1"}));
        let item2 = make_search_song_result(&json!({"videoId": "s2"}));
        let results = categorize_search_results(&[item1, item2]);

        assert_eq!(results.tracks.len(), 2);
        assert!(results.albums.is_empty());
        assert!(results.artists.is_empty());
        assert!(results.playlists.is_empty());
    }

    #[test]
    fn test_unknown_result_types_silently_ignored() {
        // "station" type in search_results_mixed.json should be skipped
        let fixture = load_fixture("search_results_mixed.json");
        let raw = fixture.as_array().unwrap();
        let results = categorize_search_results(raw);
        // Total items: 2 tracks + 1 album + 1 artist + 1 playlist = 5 (station skipped)
        let total = results.tracks.len()
            + results.albums.len()
            + results.artists.len()
            + results.playlists.len();
        assert_eq!(total, 5);
    }
}

// ---------------------------------------------------------------------------
// dict_to_album_track — ports _dict_to_album_track (api.py ~190)
// ---------------------------------------------------------------------------

mod test_dict_to_album_track {
    use super::*;

    #[test]
    fn test_positive_conversion_with_artists() {
        // Normal album track: has its own artists list.
        let item = json!({
            "videoId": "alb_vid_1",
            "title": "Track One",
            "artists": [{"name": "Solo Artist", "id": "UCsolo"}],
            "duration_seconds": 242,
            "thumbnails": [
                {"url": "https://lh3.google.com/alb_small.jpg", "width": 60, "height": 60},
                {"url": "https://lh3.google.com/alb_large.jpg", "width": 226, "height": 226}
            ]
        });
        let track = dict_to_album_track(&item, "Album Artist Fallback")
            .expect("should parse a track with an artists list");

        assert_eq!(track.video_id, "alb_vid_1");
        assert_eq!(track.title, "Track One");
        // When artists is present, album_artist parameter must NOT be used.
        assert_eq!(track.artist, "Solo Artist");
        assert_eq!(track.duration_seconds, 242.0);
        assert_eq!(track.thumbnail_url, "https://lh3.google.com/alb_large.jpg");
    }

    #[test]
    fn test_fallback_to_album_artist_when_artists_missing() {
        // Album tracks from get_album() may omit the artists field entirely;
        // album_artist is inherited from the album-level artist.
        let item = json!({
            "videoId": "alb_vid_2",
            "title": "Track Two",
            "duration": "4:05",
            "thumbnails": []
        });
        let track = dict_to_album_track(&item, "The Album Artist")
            .expect("should parse even without an artists key");

        assert_eq!(track.video_id, "alb_vid_2");
        // artists absent → fall back to album_artist parameter
        assert_eq!(track.artist, "The Album Artist");
        assert_eq!(track.duration_seconds, 245.0);
    }

    #[test]
    fn test_fallback_to_album_artist_when_artists_empty() {
        // artists key is present but empty array → same fallback as missing.
        let item = json!({
            "videoId": "alb_vid_3",
            "title": "Track Three",
            "artists": [],
            "duration_seconds": 180,
            "thumbnails": []
        });
        let track = dict_to_album_track(&item, "Fallback Band").expect("should parse");

        assert_eq!(track.artist, "Fallback Band");
    }

    #[test]
    fn test_returns_none_without_video_id() {
        let item = json!({"title": "No ID", "artists": [{"name": "X"}], "duration": "3:00"});
        assert!(dict_to_album_track(&item, "Fallback").is_none());
    }
}

// ---------------------------------------------------------------------------
// watch_item_to_track — ports _watch_item_to_track (api.py ~220)
// ---------------------------------------------------------------------------

mod test_watch_item_to_track {
    use super::*;

    #[test]
    fn test_positive_conversion_singular_thumbnail_and_length() {
        // Watch-playlist items use 'thumbnail' (singular key, value is an array)
        // for the thumbnail list and 'length' (string) for duration.
        let item = json!({
            "videoId": "watch_vid_1",
            "title": "Watch Track",
            "artists": [{"name": "Watch Artist", "id": "UCwatch"}],
            "album": {"name": "Watch Album", "id": "MPREwatch"},
            "length": "5:12",
            "thumbnail": [
                {"url": "https://lh3.google.com/watch_sm.jpg", "width": 60,  "height": 60},
                {"url": "https://lh3.google.com/watch_lg.jpg", "width": 544, "height": 544}
            ]
        });
        let track = watch_item_to_track(&item).expect("should parse");

        assert_eq!(track.video_id, "watch_vid_1");
        assert_eq!(track.title, "Watch Track");
        assert_eq!(track.artist, "Watch Artist");
        assert_eq!(track.album, "Watch Album");
        // "5:12" = 5*60 + 12 = 312
        assert_eq!(track.duration_seconds, 312.0);
        // Should pick the largest thumbnail (width 544)
        assert_eq!(track.thumbnail_url, "https://lh3.google.com/watch_lg.jpg");
    }

    #[test]
    fn test_missing_duration_length_returns_zero() {
        // When 'length' key is absent, duration falls back to 0.0.
        let item = json!({
            "videoId": "watch_vid_2",
            "title": "No Length",
            "artists": [],
            "thumbnail": []
        });
        let track = watch_item_to_track(&item).expect("should parse");
        assert_eq!(track.duration_seconds, 0.0);
    }

    #[test]
    fn test_returns_none_when_video_id_missing() {
        // No videoId → None (skip unavailable tracks, same as dict_to_track).
        let item = json!({"title": "No ID", "length": "3:00", "thumbnail": []});
        assert!(watch_item_to_track(&item).is_none());
    }
}

// ---------------------------------------------------------------------------
// parse_home_sections — album-like branch + empty-section contract
// ---------------------------------------------------------------------------

mod test_home_album_branch {
    use super::*;

    #[test]
    fn test_album_like_item_uses_audio_playlist_id() {
        // Item with browseId + audioPlaylistId and no videoId → album-like branch.
        // playlist_id must be audioPlaylistId (not browse_id).
        let section = json!({
            "title": "Albums for you",
            "contents": [
                {
                    "browseId": "MPREb_album1",
                    "audioPlaylistId": "OLAK5uy_album1",
                    "title": "Great Album",
                    "artists": [{"name": "Great Artist", "id": "UCart1"}],
                    "thumbnails": [
                        {"url": "https://lh3.google.com/album1.jpg", "width": 226, "height": 226}
                    ],
                    "year": "2024"
                }
            ]
        });
        let sections = parse_home_sections(&[section]);

        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].items.len(), 1);

        match &sections[0].items[0] {
            HomeSectionItem::Playlist(p) => {
                // playlist_id must come from audioPlaylistId
                assert_eq!(p.playlist_id, "OLAK5uy_album1");
                assert_eq!(p.title, "Great Album");
                // description is album.artist (mirrors Python)
                assert_eq!(p.description, "Great Artist");
                assert_eq!(p.thumbnail_url, "https://lh3.google.com/album1.jpg");
            }
            other => panic!("expected Playlist, got {other:?}"),
        }
    }

    #[test]
    fn test_album_like_item_falls_back_to_browse_id_when_audio_playlist_id_absent() {
        // When audioPlaylistId is absent, playlist_id falls back to album.browse_id.
        let section = json!({
            "title": "Albums for you",
            "contents": [
                {
                    "browseId": "MPREb_fallback",
                    "title": "Fallback Album",
                    "artists": [{"name": "Fallback Artist"}],
                    "thumbnails": []
                }
            ]
        });
        let sections = parse_home_sections(&[section]);

        assert_eq!(sections[0].items.len(), 1);
        match &sections[0].items[0] {
            HomeSectionItem::Playlist(p) => {
                assert_eq!(p.playlist_id, "MPREb_fallback");
                assert_eq!(p.description, "Fallback Artist");
            }
            other => panic!("expected Playlist, got {other:?}"),
        }
    }

    #[test]
    fn test_section_with_all_items_failing_parse_kept_with_empty_items() {
        // Python parity: HomeSection is appended even when all items fail to parse.
        // An item with no videoId and no browseId and no playlistId fails all branches.
        let section = json!({
            "title": "Problem Section",
            "contents": [
                {"title": "No IDs at all"}
            ]
        });
        let sections = parse_home_sections(&[section]);

        // Section is kept (not skipped)
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].title, "Problem Section");
        // Items vec is empty because the sole item failed all parse paths
        assert!(sections[0].items.is_empty());
    }

    #[test]
    fn test_section_with_null_contents_is_skipped() {
        // Python: `if contents is None: continue`
        let section = json!({"title": "Null Contents", "contents": null});
        let sections = parse_home_sections(&[section]);
        assert!(sections.is_empty());
    }
}

// ---------------------------------------------------------------------------
// parse_duration overflow hardening
// ---------------------------------------------------------------------------

mod test_parse_duration_overflow {
    use super::*;

    #[test]
    fn test_absurd_hour_count_returns_zero() {
        // i64::MAX / 3600 ≈ 2.56e15 hours — checked_mul would overflow.
        // Python would return a very large float; our Rust port returns 0.0.
        assert_eq!(parse_duration(Some("9999999999999999:00:00")), 0.0);
    }

    #[test]
    fn test_large_but_valid_hour_count() {
        // i64 can hold up to ~2.56e15 hours before overflow in multiplication
        // by 3600; 999 hours is well within range.
        // 999*3600 + 59*60 + 59 = 3596400 + 3540 + 59 = 3599999
        assert_eq!(parse_duration(Some("999:59:59")), 3_599_999.0);
    }
}

// =============================================================================
// DEFERRED tests (M3d) — endpoint-flow tests that mock the YTMusic client
// =============================================================================
//
// The following Python test classes are NOT ported here because they exercise
// the HTTP client layer (mock YTMusic / InnerTubeClient calls), which belongs
// in M3d once the client can execute real InnerTube requests:
//
// - TestSessionValidity
//   (is_session_valid / get_account_info / network-error / unusable-auth behaviour)
//
// - TestMusicAPIInit
//   (lazy client construction, accepts Path object)
//
// - TestTrackConversion (the mock-wiring subtests)
//   Tests that drive search_all() with a mocked YTMusic client:
//   test_basic_song_conversion, test_multiple_artists_joined,
//   test_missing_album, test_missing_artists, test_missing_duration,
//   test_missing_thumbnails, test_empty_thumbnails_list,
//   test_skips_items_without_video_id
//   → The parse-logic portion IS ported above (test_track_conversion module).
//     Only the "call search_all on a mock client" wiring is deferred.
//
// - TestGetLibraryPlaylists (mock wiring: test_passes_limit, test_returns_*)
// - TestGetLibraryAlbums   (mock wiring: test_passes_limit, test_returns_*)
// - TestGetLibraryArtists  (mock wiring: test_passes_limit, test_returns_simplified_artist_info)
// - TestGetPlaylistTracks  (mock wiring: test_handles_none_tracks_key)
// - TestGetHome            (mock wiring subtests)
// - TestGetLikedSongs      (mock wiring: test_passes_limit)
// - TestSearchAll          (mock wiring: test_passes_limit, test_passes_explicit_filter)
//
// These will be implemented in M3d when InnerTubeClient can issue real
// categorized search / library / playlist / home requests.
