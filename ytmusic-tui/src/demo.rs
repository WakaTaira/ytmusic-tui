//! Screenshot "demo mode" for CI-generated README screenshots.
//!
//! When the environment variable `YTMUSIC_TUI_DEMO` is set, `main.rs` branches
//! into `run_demo` instead of the real startup path. No real network calls,
//! mpv, or MPRIS are initialised.
//!
//! # Env-var contract
//!
//! | Variable                 | Values                                          | Default  |
//! |--------------------------|------------------------------------------------|----------|
//! | `YTMUSIC_TUI_DEMO`       | any non-empty string → demo mode active        | (unset)  |
//! | `YTMUSIC_TUI_DEMO_SCREEN`| `home` `search` `library` `queue` `player` `popup` | `home` |
//! | `YTMUSIC_TUI_DEMO_THEME` | any theme name known to `Theme::from_name`     | (config) |

use ytmusic_api::{
    AlbumInfo, ArtistInfo, HomeSection, HomeSectionItem, PlaylistInfo, RelatedArtist,
    SearchResults, Track,
};

use crate::app::{AppEvent, NowPlaying};
use crate::queue::RepeatMode;
use crate::views::queue_view::QueueSnapshot;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Returns `true` when `YTMUSIC_TUI_DEMO` is set to any non-empty value.
pub fn is_demo() -> bool {
    std::env::var_os("YTMUSIC_TUI_DEMO").is_some_and(|v| !v.is_empty())
}

/// Which UI screen the demo session should show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemoScreen {
    Home,
    Search,
    Library,
    Queue,
    Player,
    ActionPopup,
}

impl DemoScreen {
    /// Parse a screen variant from a string slice.
    /// Accepts "home", "search", "library", "queue", "player", "popup" (case-insensitive).
    /// Unknown values fall back to `Home`.
    fn from_str(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "search" => Self::Search,
            "library" => Self::Library,
            "queue" => Self::Queue,
            "player" => Self::Player,
            "popup" => Self::ActionPopup,
            _ => Self::Home,
        }
    }

    /// Parse from the `YTMUSIC_TUI_DEMO_SCREEN` environment variable.
    /// Unknown values fall back to `Home`.
    pub fn from_env() -> Self {
        let raw = std::env::var("YTMUSIC_TUI_DEMO_SCREEN").unwrap_or_default();
        Self::from_str(&raw)
    }
}

/// Build the batch of `AppEvent`s that populate a demo screen.
///
/// Every batch includes player-state events so the player bar always shows a
/// realistic "playing" state.  Screen-specific data events follow.
pub fn scripted_events(screen: DemoScreen) -> Vec<AppEvent> {
    let mut events = player_events();
    match screen {
        DemoScreen::Home | DemoScreen::Player => {
            events.extend(home_events());
        }
        DemoScreen::Search => {
            events.extend(search_events());
        }
        DemoScreen::Library => {
            events.extend(library_events());
        }
        DemoScreen::Queue => {
            events.extend(queue_events());
        }
        DemoScreen::ActionPopup => {
            // Populate the home view so the popup has a non-empty selected row.
            events.extend(home_events());
        }
    }
    events
}

// ---------------------------------------------------------------------------
// Shared fake catalog  (~15-20 tracks reused across screens)
// ---------------------------------------------------------------------------

/// All demo tracks. Reused across every screen so data looks coherent.
fn all_tracks() -> Vec<Track> {
    vec![
        track(
            "demo-video-001",
            "Phosphene Dream",
            "Neon Cascade",
            "Chrome Sunset",
            237.0,
        ),
        track(
            "demo-video-002",
            "Late Orbit",
            "Midnight Drive",
            "Cassette Horizons",
            194.0,
        ),
        track(
            "demo-video-003",
            "Vapour Meridian",
            "Neon Cascade",
            "Chrome Sunset",
            312.0,
        ),
        track(
            "demo-video-004",
            "Midnight Overture",
            "Circuit Bloom",
            "Neon Architecture",
            258.0,
        ),
        track(
            "demo-video-005",
            "Glass Tropics",
            "Pulse Array",
            "Open Water EP",
            183.0,
        ),
        track(
            "demo-video-006",
            "Digital Monsoon",
            "Midnight Drive",
            "Cassette Horizons",
            271.0,
        ),
        track(
            "demo-video-007",
            "Static Garden",
            "Neon Cascade",
            "Drift Index",
            209.0,
        ),
        track(
            "demo-video-008",
            "Thermal Coast",
            "Velvet Frequency",
            "Ultraviolet",
            245.0,
        ),
        track(
            "demo-video-009",
            "Crestline",
            "Circuit Bloom",
            "Neon Architecture",
            188.0,
        ),
        track(
            "demo-video-010",
            "Subterranean Light",
            "Pulse Array",
            "Open Water EP",
            326.0,
        ),
        track(
            "demo-video-011",
            "Echo Lattice",
            "Velvet Frequency",
            "Ultraviolet",
            221.0,
        ),
        track(
            "demo-video-012",
            "Quantum Drift",
            "Midnight Drive",
            "Signal Bloom",
            197.0,
        ),
        track(
            "demo-video-013",
            "Solar Descent",
            "Neon Cascade",
            "Drift Index",
            288.0,
        ),
        track(
            "demo-video-014",
            "Spectral Forest",
            "Circuit Bloom",
            "Signal Bloom",
            234.0,
        ),
        track(
            "demo-video-015",
            "Lucid Tide",
            "Velvet Frequency",
            "Ultraviolet",
            265.0,
        ),
        track(
            "demo-video-016",
            "Afterglow Requiem",
            "Pulse Array",
            "Open Water EP",
            309.0,
        ),
        track(
            "demo-video-017",
            "Holocene Static",
            "Midnight Drive",
            "Signal Bloom",
            176.0,
        ),
        track(
            "demo-video-018",
            "Chrome Synthesis",
            "Neon Cascade",
            "Chrome Sunset",
            248.0,
        ),
        track(
            "demo-video-019",
            "Refraction Loop",
            "Circuit Bloom",
            "Neon Architecture",
            203.0,
        ),
        track(
            "demo-video-020",
            "Dusk Protocol",
            "Velvet Frequency",
            "Chromatic Drift",
            291.0,
        ),
    ]
}

/// Construct a demo `Track` with dummy thumbnail URL.
fn track(id: &str, title: &str, artist: &str, album: &str, duration_seconds: f64) -> Track {
    Track::new(id, title, artist, album, duration_seconds, "")
}

// ---------------------------------------------------------------------------
// Player bar events (always included so the bar shows a frozen playing state)
// ---------------------------------------------------------------------------

fn player_events() -> Vec<AppEvent> {
    let tracks = all_tracks();
    // Track 4 (index 3) is the "currently playing" track in all screens.
    let now = &tracks[3];
    vec![
        AppEvent::NowPlaying(NowPlaying {
            title: now.title.clone(),
            artist: now.artist.clone(),
            album: now.album.clone(),
            video_id: now.video_id.clone(),
            duration_seconds: now.duration_seconds,
            shuffle: false,
            repeat: RepeatMode::Off,
        }),
        AppEvent::PlayerStarted,
        AppEvent::PlayerPaused(false),   // is_playing = true
        AppEvent::PlayerProgress(97.0),  // ~1:37 into the track
        AppEvent::PlayerDuration(258.0), // full duration visible
        AppEvent::PlayerVolume(80),
        AppEvent::PlayerMute(false),
    ]
}

// ---------------------------------------------------------------------------
// Screen-specific event batches
// ---------------------------------------------------------------------------

fn home_events() -> Vec<AppEvent> {
    let tracks = all_tracks();

    let section_quick_picks = HomeSection {
        title: "Quick picks".to_owned(),
        items: tracks[0..6]
            .iter()
            .map(|t| HomeSectionItem::Track(t.clone()))
            .collect(),
    };

    let section_mixes = HomeSection {
        title: "Mixed for you".to_owned(),
        items: vec![
            HomeSectionItem::Playlist(PlaylistInfo::new(
                "demo-pl-001",
                "Late Night Coding",
                "Curated for focus sessions",
                28,
                "",
            )),
            HomeSectionItem::Playlist(PlaylistInfo::new(
                "demo-pl-002",
                "Synthwave Essentials",
                "The classic selection",
                42,
                "",
            )),
            HomeSectionItem::Playlist(PlaylistInfo::new(
                "demo-pl-003",
                "Neon Drive Playlist",
                "Dark, fast, electric",
                37,
                "",
            )),
        ],
    };

    let section_liked = HomeSection {
        title: "Recently liked".to_owned(),
        items: tracks[6..12]
            .iter()
            .map(|t| HomeSectionItem::Track(t.clone()))
            .collect(),
    };

    let section_new = HomeSection {
        title: "New releases".to_owned(),
        items: tracks[12..18]
            .iter()
            .map(|t| HomeSectionItem::Track(t.clone()))
            .collect(),
    };

    vec![AppEvent::HomeLoaded(vec![
        section_quick_picks,
        section_mixes,
        section_liked,
        section_new,
    ])]
}

fn search_events() -> Vec<AppEvent> {
    let tracks = all_tracks();

    let results = SearchResults {
        tracks: tracks[0..8].to_vec(),
        albums: vec![
            AlbumInfo::new_without_tracks(
                "demo-al-001",
                "Chrome Sunset",
                "Neon Cascade",
                "2024",
                "",
            ),
            AlbumInfo::new_without_tracks(
                "demo-al-002",
                "Cassette Horizons",
                "Midnight Drive",
                "2023",
                "",
            ),
            AlbumInfo::new_without_tracks(
                "demo-al-003",
                "Neon Architecture",
                "Circuit Bloom",
                "2024",
                "",
            ),
            AlbumInfo::new_without_tracks(
                "demo-al-004",
                "Open Water EP",
                "Pulse Array",
                "2022",
                "",
            ),
            AlbumInfo::new_without_tracks(
                "demo-al-005",
                "Ultraviolet",
                "Velvet Frequency",
                "2023",
                "",
            ),
            AlbumInfo::new_without_tracks("demo-al-006", "Drift Index", "Neon Cascade", "2021", ""),
        ],
        artists: vec![
            RelatedArtist::new("demo-ch-001", "Neon Cascade", ""),
            RelatedArtist::new("demo-ch-002", "Midnight Drive", ""),
            RelatedArtist::new("demo-ch-003", "Circuit Bloom", ""),
            RelatedArtist::new("demo-ch-004", "Pulse Array", ""),
            RelatedArtist::new("demo-ch-005", "Velvet Frequency", ""),
        ],
        playlists: vec![
            PlaylistInfo::new("demo-pl-001", "Late Night Coding", "Deep focus mix", 28, ""),
            PlaylistInfo::new(
                "demo-pl-002",
                "Synthwave Essentials",
                "Genre highlights",
                42,
                "",
            ),
            PlaylistInfo::new(
                "demo-pl-004",
                "Neon Drive Playlist",
                "Dark, fast, electric",
                37,
                "",
            ),
            PlaylistInfo::new(
                "demo-pl-005",
                "Deep Space Radio",
                "Ambient synthwave",
                19,
                "",
            ),
        ],
    };

    vec![AppEvent::SearchLoaded(results)]
}

fn library_events() -> Vec<AppEvent> {
    let tracks = all_tracks();

    let playlists = vec![
        PlaylistInfo::new("demo-pl-001", "Late Night Coding", "Focus sessions", 28, ""),
        PlaylistInfo::new(
            "demo-pl-002",
            "Synthwave Essentials",
            "Genre highlights",
            42,
            "",
        ),
        PlaylistInfo::new(
            "demo-pl-003",
            "Neon Drive Playlist",
            "Dark, fast, electric",
            37,
            "",
        ),
        PlaylistInfo::new(
            "demo-pl-004",
            "Deep Space Radio",
            "Ambient synthwave",
            19,
            "",
        ),
        PlaylistInfo::new("demo-pl-005", "Morning Pulse", "Upbeat opener", 24, ""),
        PlaylistInfo::new(
            "demo-pl-006",
            "Chromatic Archive",
            "Curated archive",
            56,
            "",
        ),
    ];

    let albums = vec![
        AlbumInfo::new_without_tracks("demo-al-001", "Chrome Sunset", "Neon Cascade", "2024", ""),
        AlbumInfo::new_without_tracks(
            "demo-al-002",
            "Cassette Horizons",
            "Midnight Drive",
            "2023",
            "",
        ),
        AlbumInfo::new_without_tracks(
            "demo-al-003",
            "Neon Architecture",
            "Circuit Bloom",
            "2024",
            "",
        ),
        AlbumInfo::new_without_tracks("demo-al-004", "Open Water EP", "Pulse Array", "2022", ""),
        AlbumInfo::new_without_tracks("demo-al-005", "Ultraviolet", "Velvet Frequency", "2023", ""),
    ];

    let artists = vec![
        ArtistInfo::new_minimal("demo-ch-001", "Neon Cascade", ""),
        ArtistInfo::new_minimal("demo-ch-002", "Midnight Drive", ""),
        ArtistInfo::new_minimal("demo-ch-003", "Circuit Bloom", ""),
        ArtistInfo::new_minimal("demo-ch-004", "Pulse Array", ""),
        ArtistInfo::new_minimal("demo-ch-005", "Velvet Frequency", ""),
    ];

    vec![
        AppEvent::LibraryPlaylistsLoaded(playlists),
        AppEvent::LibraryAlbumsLoaded(albums),
        AppEvent::LibraryArtistsLoaded(artists),
        AppEvent::LikedSongsLoaded(tracks[0..10].to_vec()),
    ]
}

fn queue_events() -> Vec<AppEvent> {
    let tracks = all_tracks();
    // Display ~10 tracks with track index 3 as the currently playing one.
    let queue_tracks = tracks[0..10].to_vec();

    vec![AppEvent::QueueSnapshot(QueueSnapshot {
        tracks: queue_tracks,
        current_index: Some(3),
    })]
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Every screen's event batch must be non-empty.
    #[test]
    fn scripted_events_non_empty_for_every_screen() {
        for screen in [
            DemoScreen::Home,
            DemoScreen::Search,
            DemoScreen::Library,
            DemoScreen::Queue,
            DemoScreen::Player,
            DemoScreen::ActionPopup,
        ] {
            let events = scripted_events(screen);
            assert!(
                !events.is_empty(),
                "scripted_events returned empty batch for {screen:?}"
            );
        }
    }

    /// No track in the demo catalog has an empty title or empty artist.
    #[test]
    fn demo_tracks_have_non_empty_titles_and_artists() {
        for t in all_tracks() {
            assert!(!t.title.is_empty(), "empty title for id={}", t.video_id);
            assert!(!t.artist.is_empty(), "empty artist for id={}", t.video_id);
        }
    }

    /// `DemoScreen::from_str` parses all documented values correctly and handles case-insensitivity.
    #[test]
    fn demo_screen_from_str_valid_values() {
        // Valid lowercase values
        assert_eq!(DemoScreen::from_str("home"), DemoScreen::Home);
        assert_eq!(DemoScreen::from_str("search"), DemoScreen::Search);
        assert_eq!(DemoScreen::from_str("library"), DemoScreen::Library);
        assert_eq!(DemoScreen::from_str("queue"), DemoScreen::Queue);
        assert_eq!(DemoScreen::from_str("player"), DemoScreen::Player);
        assert_eq!(DemoScreen::from_str("popup"), DemoScreen::ActionPopup);
    }

    /// `DemoScreen::from_str` is case-insensitive.
    #[test]
    fn demo_screen_from_str_case_insensitive() {
        // Uppercase
        assert_eq!(DemoScreen::from_str("HOME"), DemoScreen::Home);
        assert_eq!(DemoScreen::from_str("SEARCH"), DemoScreen::Search);
        assert_eq!(DemoScreen::from_str("LIBRARY"), DemoScreen::Library);
        assert_eq!(DemoScreen::from_str("QUEUE"), DemoScreen::Queue);
        assert_eq!(DemoScreen::from_str("PLAYER"), DemoScreen::Player);
        assert_eq!(DemoScreen::from_str("POPUP"), DemoScreen::ActionPopup);

        // Mixed case
        assert_eq!(DemoScreen::from_str("Home"), DemoScreen::Home);
        assert_eq!(DemoScreen::from_str("SeArCh"), DemoScreen::Search);
        assert_eq!(DemoScreen::from_str("LIBRARY"), DemoScreen::Library);
    }

    /// `DemoScreen::from_str` falls back to `Home` for unknown values.
    #[test]
    fn demo_screen_from_str_unknown_falls_back_to_home() {
        assert_eq!(DemoScreen::from_str("unknown"), DemoScreen::Home);
        assert_eq!(DemoScreen::from_str("invalid"), DemoScreen::Home);
        assert_eq!(DemoScreen::from_str(""), DemoScreen::Home);
        assert_eq!(DemoScreen::from_str("playlist"), DemoScreen::Home); // similar but wrong
        assert_eq!(DemoScreen::from_str("UNKNOWN"), DemoScreen::Home);
    }

    /// `DemoScreen::from_env` returns `Home` when the environment variable is unset.
    #[test]
    fn demo_screen_from_env_unset() {
        // When YTMUSIC_TUI_DEMO_SCREEN is not set, from_env uses unwrap_or_default()
        // which returns an empty string, and from_str("") falls back to Home.
        assert_eq!(DemoScreen::from_env(), DemoScreen::Home);
    }
}
