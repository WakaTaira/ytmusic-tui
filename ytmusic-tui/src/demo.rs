//! Screenshot "demo mode" for CI-generated README screenshots.
//!
//! When the environment variable `YTMUSIC_TUI_DEMO` is set, `main.rs` branches
//! into `run_demo` instead of the real startup path. No real network calls,
//! mpv, or MPRIS are initialised.
//!
//! # Env-var contract
//!
//! | Variable                      | Values                                          | Default  |
//! |-------------------------------|------------------------------------------------|----------|
//! | `YTMUSIC_TUI_DEMO`            | any non-empty string → demo mode active        | (unset)  |
//! | `YTMUSIC_TUI_DEMO_SCREEN`     | `home` `search` `library` `queue` `player` `popup` | `home` |
//! | `YTMUSIC_TUI_DEMO_THEME`      | any theme name known to `Theme::from_name`     | (config) |
//! | `YTMUSIC_TUI_DEMO_INTERACTIVE`| any non-empty string → interactive loop mode   | (unset)  |

use ytmusic_api::{
    AlbumInfo, ArtistInfo, HomeSection, HomeSectionItem, PlaylistInfo, RelatedArtist,
    SearchResults, Track,
};

use crate::app::{AppCommand, AppEvent, NowPlaying};
use crate::queue::RepeatMode;
use crate::views::queue_view::QueueSnapshot;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Returns `true` when `YTMUSIC_TUI_DEMO` is set to any non-empty value.
pub fn is_demo() -> bool {
    std::env::var_os("YTMUSIC_TUI_DEMO").is_some_and(|v| !v.is_empty())
}

/// Returns `true` when both `YTMUSIC_TUI_DEMO` and `YTMUSIC_TUI_DEMO_INTERACTIVE`
/// are set to any non-empty values.
pub fn is_interactive() -> bool {
    is_demo() && std::env::var_os("YTMUSIC_TUI_DEMO_INTERACTIVE").is_some_and(|v| !v.is_empty())
}

// ---------------------------------------------------------------------------
// Command simulator (interactive demo runtime substitute)
// ---------------------------------------------------------------------------

/// Respond to an [`AppCommand`] from the UI with fake [`AppEvent`]s.
///
/// This is the interactive-demo substitute for the real runtime thread. The UI
/// sends commands exactly as in production; this function synthesises realistic
/// events so the model updates as if a real backend replied. Commands that have
/// no meaningful stub reply (seek, volume, mute, etc.) return an empty `Vec`.
///
/// The scripted catalog is the same one used by [`scripted_events`] so the demo
/// looks coherent across all interactions.
pub fn respond(cmd: &AppCommand) -> Vec<AppEvent> {
    match cmd {
        AppCommand::FetchHome => home_events(),

        AppCommand::Search { .. } => search_events(),

        AppCommand::Play(track) => play_events_for_track(track),

        AppCommand::PlayPlaylist {
            tracks,
            start_index,
        } => {
            if let Some(track) = tracks.get(*start_index).or_else(|| tracks.first()) {
                play_events_for_track(track)
            } else {
                vec![]
            }
        }

        AppCommand::FetchQueue => queue_events(),

        AppCommand::FetchLibraryPlaylists => {
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
            ];
            vec![AppEvent::LibraryPlaylistsLoaded(playlists)]
        }

        AppCommand::FetchLibraryAlbums => {
            let albums = vec![
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
            ];
            vec![AppEvent::LibraryAlbumsLoaded(albums)]
        }

        AppCommand::FetchLibraryArtists => {
            let artists = vec![
                ArtistInfo::new_minimal("demo-ch-001", "Neon Cascade", ""),
                ArtistInfo::new_minimal("demo-ch-002", "Midnight Drive", ""),
            ];
            vec![AppEvent::LibraryArtistsLoaded(artists)]
        }

        AppCommand::FetchPlaylistTracks { title, .. } => {
            let tracks = all_tracks();
            vec![AppEvent::PlaylistTracksLoaded {
                title: title.clone(),
                tracks: tracks[0..8].to_vec(),
            }]
        }

        AppCommand::TogglePause => {
            // The demo has no live player state to flip, so report "playing"
            // (unpaused) after every toggle — the bar icon cycles visually.
            vec![AppEvent::PlayerPaused(false)]
        }

        AppCommand::AdjustVolume(delta) => {
            // Clamp the stub volume within 0–100 and echo it back.
            let new_vol = (80_i64 + delta).clamp(0, 100);
            vec![AppEvent::PlayerVolume(new_vol)]
        }

        AppCommand::ToggleShuffle => {
            // Re-emit the now-playing snapshot with shuffle toggled (stub always
            // reports shuffle=true after the first toggle for a visible effect).
            let tracks = all_tracks();
            let now = &tracks[3];
            vec![AppEvent::NowPlaying(NowPlaying {
                title: now.title.clone(),
                artist: now.artist.clone(),
                album: now.album.clone(),
                video_id: now.video_id.clone(),
                duration_seconds: now.duration_seconds,
                shuffle: true,
                repeat: RepeatMode::Off,
            })]
        }

        AppCommand::CycleRepeat => {
            let tracks = all_tracks();
            let now = &tracks[3];
            vec![AppEvent::NowPlaying(NowPlaying {
                title: now.title.clone(),
                artist: now.artist.clone(),
                album: now.album.clone(),
                video_id: now.video_id.clone(),
                duration_seconds: now.duration_seconds,
                shuffle: false,
                repeat: RepeatMode::All,
            })]
        }

        AppCommand::NextTrack | AppCommand::PreviousTrack => {
            // Advance/rewind: pick a different demo track to make the change
            // visible in the player bar.
            let tracks = all_tracks();
            let next = if matches!(cmd, AppCommand::NextTrack) {
                &tracks[4]
            } else {
                &tracks[2]
            };
            play_events_for_track(next)
        }

        AppCommand::AddToQueue(track) => {
            let tracks = all_tracks();
            let mut queue_tracks = tracks[0..5].to_vec();
            queue_tracks.push(track.clone());
            vec![
                AppEvent::QueueSnapshot(QueueSnapshot {
                    tracks: queue_tracks,
                    current_index: Some(3),
                }),
                AppEvent::ActionResult(format!("Added to queue: {}", track.title)),
            ]
        }

        AppCommand::ToggleLike(_) => {
            vec![AppEvent::ActionResult("Liked!".to_owned())]
        }

        AppCommand::CycleAudioQuality => {
            vec![AppEvent::AudioQualityChanged("high".to_owned())]
        }

        // Commands that have no visible effect in demo mode.
        AppCommand::Quit
        | AppCommand::SeekForward
        | AppCommand::SeekBackward
        | AppCommand::SeekToStart
        | AppCommand::ToggleMute
        | AppCommand::CheckSession
        | AppCommand::StartRadio(_)
        | AppCommand::SetVolume(_)
        | AppCommand::FetchAlbum(_)
        | AppCommand::FetchArtist(_)
        | AppCommand::FetchLyrics(_)
        | AppCommand::FetchHistory
        | AppCommand::AddToPlaylist { .. }
        | AppCommand::CreatePlaylistAndAdd { .. }
        | AppCommand::RemoveFromQueue(_)
        | AppCommand::RemoveFromPlaylist { .. }
        | AppCommand::SearchAndOpenArtist(_)
        | AppCommand::SearchAndOpenAlbum { .. } => vec![],
    }
}

/// Build NowPlaying + player-state events for a specific track.
///
/// Used by both the `Play` and `PlayPlaylist` arms of [`respond`], and also by
/// `NextTrack` / `PreviousTrack`. Mirrors the real runtime's `play_single` /
/// `emit_now_playing` + the player-event fan-out.
fn play_events_for_track(track: &Track) -> Vec<AppEvent> {
    let tracks = all_tracks();
    // Build a tiny queue snapshot so the queue view reflects the new track.
    let queue_snapshot = QueueSnapshot {
        tracks: tracks[0..5].to_vec(),
        current_index: Some(0),
    };
    vec![
        AppEvent::NowPlaying(NowPlaying {
            title: track.title.clone(),
            artist: track.artist.clone(),
            album: track.album.clone(),
            video_id: track.video_id.clone(),
            duration_seconds: track.duration_seconds,
            shuffle: false,
            repeat: RepeatMode::Off,
        }),
        AppEvent::PlayerStarted,
        AppEvent::PlayerPaused(false),
        AppEvent::PlayerProgress(0.0),
        AppEvent::PlayerDuration(track.duration_seconds),
        AppEvent::PlayerVolume(80),
        AppEvent::QueueSnapshot(queue_snapshot),
    ]
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

    // -- respond() tests -------------------------------------------------------

    /// Searching returns a SearchLoaded event.
    #[test]
    fn respond_search_returns_search_loaded() {
        let cmd = AppCommand::Search {
            query: "neon".to_owned(),
            filter: None,
        };
        let events = respond(&cmd);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AppEvent::SearchLoaded(_))),
            "expected SearchLoaded in response to Search command"
        );
    }

    /// Playing a track returns NowPlaying among its events.
    #[test]
    fn respond_play_returns_now_playing() {
        let t = track(
            "demo-test-001",
            "Test Track",
            "Test Artist",
            "Test Album",
            180.0,
        );
        let events = respond(&AppCommand::Play(t.clone()));
        let has_now_playing = events.iter().any(|e| match e {
            AppEvent::NowPlaying(np) => np.title == t.title && np.artist == t.artist,
            _ => false,
        });
        assert!(has_now_playing, "expected NowPlaying for the played track");
    }

    /// Playing a track also returns PlayerStarted.
    #[test]
    fn respond_play_returns_player_started() {
        let t = track("demo-test-002", "Track B", "Artist B", "Album B", 200.0);
        let events = respond(&AppCommand::Play(t));
        assert!(
            events.contains(&AppEvent::PlayerStarted),
            "expected PlayerStarted in response to Play"
        );
    }

    /// FetchHome returns at least one HomeLoaded event.
    #[test]
    fn respond_fetch_home_returns_home_loaded() {
        let events = respond(&AppCommand::FetchHome);
        assert!(
            events.iter().any(|e| matches!(e, AppEvent::HomeLoaded(_))),
            "expected HomeLoaded in response to FetchHome"
        );
    }

    /// FetchQueue returns a QueueSnapshot event.
    #[test]
    fn respond_fetch_queue_returns_queue_snapshot() {
        let events = respond(&AppCommand::FetchQueue);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AppEvent::QueueSnapshot(_))),
            "expected QueueSnapshot in response to FetchQueue"
        );
    }

    /// Every no-op arm of respond() returns an empty event list.
    ///
    /// Table-driven: covers all variants listed in the no-op match arm so that
    /// adding a new variant forces an explicit decision here.
    #[test]
    fn respond_unsupported_commands_return_empty() {
        let cmds: &[AppCommand] = &[
            AppCommand::Quit,
            AppCommand::SeekForward,
            AppCommand::SeekBackward,
            AppCommand::SeekToStart,
            AppCommand::ToggleMute,
            AppCommand::CheckSession,
            AppCommand::StartRadio("vid-001".to_owned()),
            AppCommand::SetVolume(50),
            AppCommand::FetchAlbum("browse-001".to_owned()),
            AppCommand::FetchArtist("ch-001".to_owned()),
            AppCommand::FetchLyrics("vid-001".to_owned()),
            AppCommand::FetchHistory,
            AppCommand::AddToPlaylist {
                playlist_id: "pl-001".to_owned(),
                video_id: "vid-001".to_owned(),
            },
            AppCommand::CreatePlaylistAndAdd {
                title: "My Playlist".to_owned(),
                video_id: "vid-001".to_owned(),
            },
            AppCommand::RemoveFromQueue("vid-001".to_owned()),
            AppCommand::RemoveFromPlaylist {
                playlist_id: "pl-001".to_owned(),
                video_id: "vid-001".to_owned(),
            },
            AppCommand::SearchAndOpenArtist("Artist Name".to_owned()),
            AppCommand::SearchAndOpenAlbum {
                name: "Album Name".to_owned(),
                artist: "Artist Name".to_owned(),
            },
        ];
        for cmd in cmds {
            let events = respond(cmd);
            assert!(
                events.is_empty(),
                "expected empty response for {cmd:?}, got {events:?}"
            );
        }
    }

    /// PlayPlaylist with a valid start_index returns NowPlaying for that track.
    #[test]
    fn respond_play_playlist_returns_now_playing_for_start_track() {
        let tracks = all_tracks();
        let start_index = 2;
        let cmd = AppCommand::PlayPlaylist {
            tracks: tracks.clone(),
            start_index,
        };
        let events = respond(&cmd);
        let has_now_playing = events.iter().any(|e| match e {
            AppEvent::NowPlaying(np) => np.title == tracks[start_index].title,
            _ => false,
        });
        assert!(
            has_now_playing,
            "expected NowPlaying for track at start_index"
        );
    }

    /// is_interactive returns false when the env vars are unset.
    ///
    /// Uses a process-wide mutex so parallel test threads cannot observe each
    /// other's temporary env-var mutations.
    #[test]
    fn is_interactive_false_when_unset() {
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        // Save and clear both vars so the assertion is unconditional.
        let saved_demo = std::env::var_os("YTMUSIC_TUI_DEMO");
        let saved_interactive = std::env::var_os("YTMUSIC_TUI_DEMO_INTERACTIVE");
        unsafe {
            std::env::remove_var("YTMUSIC_TUI_DEMO");
            std::env::remove_var("YTMUSIC_TUI_DEMO_INTERACTIVE");
        }

        let result = is_interactive();

        // Restore original values before asserting so a failure doesn't leave
        // the environment dirty for other tests.
        unsafe {
            match saved_demo {
                Some(v) => std::env::set_var("YTMUSIC_TUI_DEMO", v),
                None => std::env::remove_var("YTMUSIC_TUI_DEMO"),
            }
            match saved_interactive {
                Some(v) => std::env::set_var("YTMUSIC_TUI_DEMO_INTERACTIVE", v),
                None => std::env::remove_var("YTMUSIC_TUI_DEMO_INTERACTIVE"),
            }
        }

        assert!(
            !result,
            "is_interactive() must be false when env vars are absent"
        );
    }

    // -- scripted_events() tests -----------------------------------------------

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
