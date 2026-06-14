//! Application runtime plumbing: the command/event channels, the dedicated
//! tokio runtime thread, and the player-event fan-out forwarder.
//!
//! # Architecture (the M5a fixed decision)
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────────────┐
//! │ main thread: synchronous ratatui + crossterm render/input loop     │
//! │   • poll crossterm events (with timeout) → send AppCommand          │
//! │   • drain the AppEvent receiver (try_recv) each tick → mutate state │
//! │   • render the state                                                │
//! └────────┬──────────────────────────────────────────▲───────────────┘
//!          │ AppCommand                                │ AppEvent
//!          │ (tokio mpsc, UI→runtime)                  │ (std mpsc, →UI)
//!          ▼                                           │
//! ┌────────────────────────────────────────────────────┴──────────────┐
//! │ dedicated std::thread hosting a tokio Runtime (block_on)            │
//! │   owns InnerTubeClient + Player                                     │
//! │   command loop: cmd_rx.recv().await                                 │
//! │     FetchHome → client.get_home() → HomeLoaded / ApiError → ev_tx   │
//! │     Play / TogglePause / SetVolume / AdjustVolume → player ops      │
//! │   (M6) MPRIS server tasks join this same runtime                    │
//! └──────────────────────────────────────────────▲────────────────────┘
//!          ▲ PlayerEvent (std mpsc, from mpv's event thread)            │
//!          │                                                            │
//! ┌────────┴────────────────────────────────────────────────────────┐  │
//! │ player fan-out forwarder: a dedicated std::thread                 │  │
//! │   loop { player_events.recv() → map to AppEvent::Player* → ev_tx }│──┘
//! │   ← M6 adds a second sink (the MPRIS update channel) right here    │
//! └───────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Queue ownership and the auto-advance flow (the M5b decision)
//!
//! The `QueueManager` lives **inside the runtime thread** alongside the
//! `Player` — a single owner, no shared mutable state, no locks. Playback
//! commands ([`AppCommand::Play`], `PlayPlaylist`, `NextTrack`, ...) mutate the
//! queue and then drive the player; whenever the current track or the
//! shuffle/repeat modes change the runtime emits an [`AppEvent::NowPlaying`] so
//! the UI's player bar can update its metadata.
//!
//! Auto-advance is the one subtle flow. A natural end-of-file arrives on the
//! **forwarder** thread (not the runtime), which only holds the UI sink. Rather
//! than give the forwarder a back-channel into the queue — which would mean
//! sharing the queue across threads — the forwarder forwards `TrackEnded` to
//! the UI as today, and the UI loop responds by sending
//! [`AppCommand::NextTrack`] back to the runtime. The runtime then advances the
//! queue and plays the next track (or goes idle), emitting `NowPlaying`. This
//! keeps the queue single-owned; the extra hop is one UI tick (<~60 ms) and is
//! invisible. Crucially, `TrackError` is forwarded but the UI does **not** send
//! `NextTrack` for it — a broken resolver must never machine-gun the queue (the
//! end-file battle lesson, asserted in the UI-layer tests in `main.rs`).
//!
//! ## Why these channel types
//!
//! * **AppCommand uses a `tokio::sync::mpsc`** because the consumer is an async
//!   task inside the runtime that `recv().await`s. The producer (the UI thread)
//!   calls the non-async [`UnboundedSender::send`], which works from any thread
//!   without an async context.
//! * **AppEvent uses a `std::sync::mpsc`** because the consumer is the
//!   synchronous UI loop, which drains it with non-blocking `try_recv` once per
//!   tick. Every producer (the runtime tasks, the forwarder thread) calls the
//!   non-async `std` `Sender::send`.
//!
//! ## The player-event fan-out shape (resolves the M2 single-consumer TODO)
//!
//! `Player` hands out its `PlayerEvent` receiver exactly once via
//! [`Player::take_events`](crate::player::Player::take_events) — it is a
//! single-consumer `mpsc::Receiver` (`!Clone`). The forwarder thread is that
//! single owner: it blocks on `recv()` and **re-publishes** each event as an
//! [`AppEvent`] onto the (clonable-by-construction, multi-producer) UI event
//! channel. This is a hand-rolled fan-out rather than a `broadcast` channel
//! because today there is exactly one downstream (the UI). M6 adds MPRIS as a
//! second downstream by handing the forwarder a second sink (see
//! [`spawn_player_forwarder`]); no `broadcast` dependency is pulled in until a
//! second consumer actually exists (YAGNI).

use std::sync::mpsc::Sender as StdSender;
use std::thread::{self, JoinHandle};

use tokio::runtime::Builder;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use ytmusic_api::{
    AlbumInfo, ArtistInfo, HomeSection, InnerTubeClient, PlaylistInfo, SearchResults, Track,
};

use crate::views::queue_view::QueueSnapshot;

use crate::mpris::{self, MprisHandle, MprisState, control_to_command};
use crate::player::{Player, PlayerEvent};
use crate::queue::{QueueManager, RepeatMode};

/// A command sent from the UI thread to the runtime thread.
///
/// The UI never blocks on the result; the runtime replies (when there is
/// anything to reply) by emitting an [`AppEvent`].
#[derive(Debug, Clone, PartialEq)]
pub enum AppCommand {
    /// Fetch the home page recommendations. Replies with
    /// [`AppEvent::HomeLoaded`] or [`AppEvent::ApiError`].
    FetchHome,
    /// Fetch the user's library playlists (for the playlist view's level-1
    /// list). Replies with [`AppEvent::LibraryPlaylistsLoaded`] or
    /// [`AppEvent::ApiError`].
    FetchLibraryPlaylists,
    /// Fetch a single playlist's tracks (the playlist view's level-2 list).
    /// `playlist_id` identifies the playlist; `title` is echoed back so the UI
    /// can label the track list without a second lookup. Replies with
    /// [`AppEvent::PlaylistTracksLoaded`] or [`AppEvent::ApiError`].
    FetchPlaylistTracks { playlist_id: String, title: String },
    /// Run a search (the search view's Enter-confirm). `query` is the text to
    /// search; `filter` restricts the result type when a `#category:` prefix was
    /// used (`"songs"` / `"albums"` / `"artists"` / `"playlists"`), or `None`
    /// for an all-category search. Replies with [`AppEvent::SearchLoaded`] or
    /// [`AppEvent::ApiError`].
    Search {
        query: String,
        filter: Option<String>,
    },
    /// Fetch the user's library albums (the library view's albums pane). Replies
    /// with [`AppEvent::LibraryAlbumsLoaded`] or [`AppEvent::ApiError`].
    FetchLibraryAlbums,
    /// Fetch the user's library artists (the library view's artists pane).
    /// Replies with [`AppEvent::LibraryArtistsLoaded`] or [`AppEvent::ApiError`].
    FetchLibraryArtists,
    /// Play a single track immediately: the queue is replaced with `[track]`
    /// (spotify_player "play this one" semantics). Emits [`AppEvent::NowPlaying`].
    Play(Track),
    /// Play a playlist starting at `start_index`, queueing the rest (the
    /// spotify_player "queue the remainder" rule). Emits [`AppEvent::NowPlaying`].
    PlayPlaylist {
        tracks: Vec<Track>,
        start_index: usize,
    },
    /// Advance to the next queued track (auto-advance on EOF, or the `n` key).
    /// Plays the new current track, or goes idle when the queue is exhausted.
    NextTrack,
    /// Go back one track (the `p` key). At the start of the queue this stays
    /// on the first track and replays it (mirrors Python `action_previous_track`).
    PreviousTrack,
    /// Toggle shuffle on the queue (the `s` key). Emits [`AppEvent::NowPlaying`]
    /// so the bar's shuffle label updates.
    ToggleShuffle,
    /// Cycle the repeat mode Off → All → One (the `r` key). Emits
    /// [`AppEvent::NowPlaying`] so the bar's repeat label updates.
    CycleRepeat,
    /// Toggle between paused and playing.
    TogglePause,
    /// Set the absolute volume (clamped to 0–100 by the player).
    SetVolume(i64),
    /// Adjust the volume by a relative delta.
    AdjustVolume(i64),
    /// Seek forward by [`SEEK_STEP_SECONDS`] (the `>` key). A no-op when nothing
    /// is playing; a failed seek (the stream not yet seekable while the
    /// ytdl-hook resolves) is swallowed, mirroring Python's suppressed seek.
    SeekForward,
    /// Seek backward by [`SEEK_STEP_SECONDS`] (the `<` key). Same no-op / error
    /// semantics as [`AppCommand::SeekForward`].
    SeekBackward,
    /// Seek to the start of the current track (the `^` key). A no-op when
    /// nothing is playing.
    SeekToStart,
    /// Toggle audio mute (the `_` key). The player's mute observation
    /// ([`AppEvent::PlayerMute`]) folds the new state into the bar.
    ToggleMute,
    /// Cycle the audio quality low → normal → high → low (the `b` key). The
    /// change applies from the next track (ytdl-format is read at loadfile
    /// time). Replies with [`AppEvent::AudioQualityChanged`] carrying the new
    /// level so the UI can toast it.
    CycleAudioQuality,
    /// Fetch a single album's tracks. `browse_id` identifies the album.
    /// Replies with [`AppEvent::AlbumLoaded`] or [`AppEvent::ApiError`].
    FetchAlbum(String),
    /// Fetch an artist's page (top songs / albums / related artists).
    /// `channel_id` identifies the artist. Replies with
    /// [`AppEvent::ArtistLoaded`] or [`AppEvent::ApiError`].
    FetchArtist(String),
    /// Fetch lyrics for the given `video_id`. Replies with
    /// [`AppEvent::LyricsLoaded`] or [`AppEvent::ApiError`].
    FetchLyrics(String),
    /// Fetch the user's listening history. Replies with
    /// [`AppEvent::HistoryLoaded`] or [`AppEvent::ApiError`].
    FetchHistory,
    /// Request a snapshot of the current queue state (for the queue view).
    /// Replies immediately with [`AppEvent::QueueSnapshot`] — no API call
    /// needed.
    FetchQueue,
    /// Validate the auth session (the "logged-out HTTP 200" canary). Replies
    /// with [`AppEvent::SessionInvalid`] only when the session is *not* valid;
    /// a valid session produces no event (the UI assumes valid by default).
    CheckSession,
    /// Append a single track to the end of the queue (the action popup's "Add to
    /// queue"). No playback change; emits an updated [`AppEvent::QueueSnapshot`]
    /// so an open queue view reflects the addition.
    AddToQueue(Track),
    /// Start a radio seeded by `video_id` (the `R` key / "Start radio" action).
    /// Fetches the radio tracks and plays them as a fresh queue. Replies with
    /// [`AppEvent::NowPlaying`] on success or [`AppEvent::ApiError`] on failure.
    StartRadio(String),
    /// Resolve an artist by `name` via search and open their page (the `a` key /
    /// "Go to artist"). The runtime searches `name` with the `artists` filter,
    /// takes the first hit's `channel_id`, and replies with
    /// [`AppEvent::ArtistLoaded`] (chaining the fetch). An empty result or an
    /// API error replies with [`AppEvent::ApiError`]. Mirrors Python's
    /// `_lookup_and_open_artist`.
    SearchAndOpenArtist(String),
    /// Resolve an album by `name` (optionally disambiguated by `artist`) via
    /// search and open it (the `A` key / "Go to album"). The runtime searches
    /// `"{name} {artist}"` with the `albums` filter, takes the first hit's
    /// `browse_id`, and replies with [`AppEvent::AlbumLoaded`]. Mirrors Python's
    /// `_lookup_and_open_album`.
    SearchAndOpenAlbum { name: String, artist: String },
    /// Toggle the like state of `video_id` (the `f` key / "Like / Unlike"). Reads
    /// the current status then flips it. Replies with [`AppEvent::ActionResult`]
    /// (a toast) or [`AppEvent::ApiError`].
    ToggleLike(String),
    /// Save / remove an album or playlist from the user's library (issue #12,
    /// the album / playlist popup's "Save to library" / "Remove from library"
    /// actions). Mirrors [`AppCommand::ToggleLike`] but targets a
    /// `playlistId` instead of a `videoId`: `status` is `"LIKE"` to save or
    /// `"INDIFFERENT"` to remove. Replies with [`AppEvent::ActionResult`] on
    /// success or [`AppEvent::ApiError`] on failure.
    RatePlaylist {
        /// The album's `audioPlaylistId` (`OLAK5uy_...`) or the playlist's
        /// id (`PL...` / `RD...`). Any leading `VL` prefix is stripped
        /// runtime-side by the API layer.
        playlist_id: String,
        /// `"LIKE"` to save or `"INDIFFERENT"` to remove.
        status: String,
    },
    /// Add `video_id` to the playlist `playlist_id` (the "Add to playlist"
    /// action after picking an existing playlist). Replies with
    /// [`AppEvent::ActionResult`] or [`AppEvent::ApiError`].
    AddToPlaylist {
        playlist_id: String,
        video_id: String,
    },
    /// Create a new playlist titled `title` and add `video_id` to it (the
    /// picker's "New playlist…" choice). Replies with [`AppEvent::ActionResult`]
    /// or [`AppEvent::ApiError`].
    CreatePlaylistAndAdd { title: String, video_id: String },
    /// Remove the queue track with `video_id` (the queue action popup's "Remove
    /// from queue"). The runtime finds the index by id and removes it, then
    /// emits an updated [`AppEvent::QueueSnapshot`]. A no-op (with a toast) when
    /// the track is no longer in the queue. Mirrors Python's `_remove_from_queue`
    /// (look up by `video_id`, then `queue_manager.remove(i)`).
    RemoveFromQueue(String),
    /// Remove `video_id` from the playlist `playlist_id` (the playlist-track
    /// action popup's "Remove from playlist"). Replies with
    /// [`AppEvent::ActionResult`] or [`AppEvent::ApiError`]. Mirrors Python's
    /// `_remove_from_playlist` (`remove_playlist_items(playlist_id, [video_id])`).
    RemoveFromPlaylist {
        playlist_id: String,
        video_id: String,
    },
    /// Subscribe to (follow) an artist (issue #11 / "Follow artist" action).
    /// The string is the artist's display name — the album popup is the only
    /// caller today and `AlbumInfo` carries only the artist name, not a
    /// channel id, so the runtime first resolves the name via the search
    /// endpoint (mirroring `SearchAndOpenArtist`) and then subscribes to the
    /// resolved channel id. Replies with [`AppEvent::ActionResult`] on success
    /// or [`AppEvent::ApiError`] on failure (including "artist not found").
    FollowArtist(String),
    /// Unsubscribe from (unfollow) an artist (issue #11 / "Unfollow artist"
    /// action). Same name-based resolution as [`AppCommand::FollowArtist`].
    UnfollowArtist(String),
    /// Shut the runtime down cleanly; the command loop exits after this.
    Quit,
}

/// Now-playing metadata, emitted by the runtime whenever the queue's current
/// track or modes change.
///
/// This is the Rust replacement for Python's 1 Hz player-bar poll
/// (`views/player.py::_poll_player_state`), which enriched the bar from
/// `queue.current_track`. Here the queue lives in the runtime thread, so the
/// runtime pushes a snapshot to the UI on every queue mutation rather than the
/// UI polling. The progress/duration *ticks* still arrive via the player's own
/// [`AppEvent::PlayerProgress`] / [`AppEvent::PlayerDuration`] feed; this event
/// carries the static-per-track metadata (title/artist/album) plus the queue
/// modes and the API duration fallback.
#[derive(Debug, Clone, PartialEq)]
pub struct NowPlaying {
    /// The current track's title, or empty when the queue is idle.
    pub title: String,
    /// The current track's artist.
    pub artist: String,
    /// The current track's album (the bar's dimmed second row).
    pub album: String,
    /// The current track's `video_id` (empty when idle).
    pub video_id: String,
    /// The track's duration from the API, used as the bar's fallback while mpv
    /// still reports 0 (the ytdl-hook resolves the real duration lazily).
    pub duration_seconds: f64,
    /// Whether shuffle is enabled on the queue.
    pub shuffle: bool,
    /// The queue's repeat mode.
    pub repeat: RepeatMode,
}

/// An event sent from the runtime / forwarder threads to the UI thread.
///
/// The UI drains these with `try_recv` once per render tick and folds them into
/// its state.
#[derive(Debug, Clone, PartialEq)]
pub enum AppEvent {
    /// Home recommendations finished loading.
    HomeLoaded(Vec<HomeSection>),
    /// The user's library playlists finished loading (playlist view level 1).
    LibraryPlaylistsLoaded(Vec<PlaylistInfo>),
    /// A playlist's tracks finished loading (playlist view level 2). `title` is
    /// the playlist name echoed from the request so the view can label the list.
    PlaylistTracksLoaded { title: String, tracks: Vec<Track> },
    /// A search finished, carrying the categorized results for all four panes.
    SearchLoaded(SearchResults),
    /// The library albums finished loading (library view's albums pane).
    LibraryAlbumsLoaded(Vec<AlbumInfo>),
    /// The library artists finished loading (library view's artists pane).
    LibraryArtistsLoaded(Vec<ArtistInfo>),
    /// An API call failed; the string is a user-facing, classified message.
    ApiError(String),
    /// The now-playing metadata changed (a new current track, or a mode toggle).
    /// Feeds the static-per-track half of the player bar; see [`NowPlaying`].
    NowPlaying(NowPlaying),
    /// A `time-pos` tick from the player (seconds). Feeds the player bar.
    PlayerProgress(f64),
    /// A `duration` observation from the player (seconds). Feeds the player bar.
    PlayerDuration(f64),
    /// A `volume` observation from the player (0–100). Corrects the bar's
    /// optimistic volume after mpv applies (or clamps) a change.
    PlayerVolume(i64),
    /// A `mute` observation from the player. Folds into the bar so it shows
    /// `Vol: MUTE` while muted (the `_` key).
    PlayerMute(bool),
    /// A `pause` observation from the player (`true` = paused, `false` =
    /// playing). Folds into the bar so the ▶ / ⏸ icon tracks every pause
    /// toggle: the space key, MPRIS PlayPause, and auto-advance all go through
    /// mpv, so this single event covers all paths.
    PlayerPaused(bool),
    /// The audio quality was cycled; the string is the new level (`"low"` /
    /// `"normal"` / `"high"`). The UI toasts it on the status line. Applies from
    /// the next track.
    AudioQualityChanged(String),
    /// A non-playback mutation (like/unlike, add-to-queue, add-to-playlist)
    /// succeeded; the string is a short user-facing confirmation toast.
    ActionResult(String),
    /// The current track started loading in mpv. This collapses both mpv
    /// `start-file` and `file-loaded` (see [`translate_player_event`]); M6's
    /// MPRIS may need to split them for metadata publishing.
    PlayerStarted,
    /// The current track ended naturally; the queue should advance. The UI
    /// reacts by sending [`AppCommand::NextTrack`] back to the runtime (the
    /// queue lives runtime-side; see the module docs on the advance flow).
    TrackEnded,
    /// The current stream failed; the string is a short description.
    TrackError(String),
    /// A search resolved a `channel_id` for the `a` / "Go to artist" lookup. The
    /// UI navigates to the artist page (switch view + push nav) and the fetch is
    /// chained by the follow-up [`AppCommand::FetchArtist`] the fold returns —
    /// reusing the normal open-artist flow. Emitted by
    /// [`AppCommand::SearchAndOpenArtist`] on a hit.
    ArtistResolved(String),
    /// A search resolved a `browse_id` for the `A` / "Go to album" lookup. The
    /// UI navigates to the album page and the fetch is chained by the follow-up
    /// [`AppCommand::FetchAlbum`] the fold returns. Emitted by
    /// [`AppCommand::SearchAndOpenAlbum`] on a hit.
    AlbumResolved(String),
    /// An album's tracks finished loading.
    AlbumLoaded(AlbumInfo),
    /// An artist's page data finished loading.
    ArtistLoaded(ArtistInfo),
    /// Lyrics for the current track finished loading. `None` means the API
    /// confirmed no lyrics are available for this track (not an error).
    LyricsLoaded(Option<String>),
    /// The user's listening history finished loading.
    HistoryLoaded(Vec<Track>),
    /// A snapshot of the current queue state (response to
    /// [`AppCommand::FetchQueue`], or emitted after any queue mutation so the
    /// queue view stays current).
    QueueSnapshot(QueueSnapshot),
    /// The auth session is invalid (YouTube served a logged-out page with HTTP
    /// 200). The UI renders a one-line warning prompting `ytmusic-tui auth`.
    /// Only emitted in response to [`AppCommand::CheckSession`] when the canary
    /// fails — a valid session is silent.
    SessionInvalid,
}

/// Handles for the spawned background threads, joined on shutdown.
///
/// Dropping this does **not** join — call [`RuntimeHandle::shutdown`] so the UI
/// can tear the runtime down deterministically (send [`AppCommand::Quit`],
/// then join both threads).
pub struct RuntimeHandle {
    commands: UnboundedSender<AppCommand>,
    runtime_thread: Option<JoinHandle<()>>,
    forwarder_thread: Option<JoinHandle<()>>,
}

impl RuntimeHandle {
    /// Send a command to the runtime thread.
    ///
    /// Returns `false` if the runtime thread has already exited (its receiver
    /// is gone), so the caller can stop trying.
    pub fn send(&self, command: AppCommand) -> bool {
        self.commands.send(command).is_ok()
    }

    /// Create a stub `RuntimeHandle` for unit tests.
    ///
    /// Returns `(handle, receiver)` — `handle` can be passed to any method
    /// that takes `&RuntimeHandle`; `receiver` lets the test drain and inspect
    /// the commands the code under test sent. The stub handle has no background
    /// threads; calling `shutdown` on it is a no-op.
    #[doc(hidden)]
    pub fn stub() -> (Self, tokio::sync::mpsc::UnboundedReceiver<AppCommand>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let handle = Self {
            commands: tx,
            runtime_thread: None,
            forwarder_thread: None,
        };
        (handle, rx)
    }

    /// Shut the runtime down: send [`AppCommand::Quit`], then join both
    /// threads. Idempotent — a second call is a no-op.
    ///
    /// The forwarder thread exits on its own once the player is dropped (its
    /// `recv()` returns `Err` when the player's event-thread sender is gone),
    /// which happens when the runtime thread drops the `Player` it owns.
    pub fn shutdown(&mut self) {
        // Best-effort: if the runtime already exited, the send fails and the
        // join below still cleans up.
        let _ = self.commands.send(AppCommand::Quit);
        if let Some(handle) = self.runtime_thread.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.forwarder_thread.take() {
            let _ = handle.join();
        }
    }
}

/// Spawn the runtime thread and the player-event forwarder thread.
///
/// * `client` is the InnerTube client, or `None` when auth could not be loaded
///   (a [`AppCommand::FetchHome`] then replies with [`AppEvent::ApiError`]
///   rather than panicking — mirrors the Python "empty library" degradation).
/// * `player` is moved into the runtime thread, which owns it for its lifetime
///   and applies playback commands to it.
/// * `player_events` is the receiver taken from the player via
///   [`Player::take_events`]; the forwarder thread drains it.
/// * `events` is the UI-bound sink; both the runtime tasks and the forwarder
///   publish [`AppEvent`]s to it.
///
/// Returns a [`RuntimeHandle`] for sending commands and shutting down.
#[must_use]
pub fn spawn_runtime(
    client: Option<InnerTubeClient>,
    player: Player,
    player_events: std::sync::mpsc::Receiver<PlayerEvent>,
    events: StdSender<AppEvent>,
) -> RuntimeHandle {
    // Unbounded by design: the only sender is the UI thread at ~1 command per
    // keypress, so the queue cannot grow large in practice, and the
    // synchronous UI thread must never block on a full buffer.
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<AppCommand>();

    // The forwarder owns the single player-event receiver and re-publishes onto
    // the UI channel. Spawned first so it is already draining before playback
    // can start.
    let forwarder_events = events.clone();
    let forwarder_thread =
        thread::spawn(move || run_player_forwarder(player_events, &forwarder_events));

    let runtime_thread = thread::spawn(move || run_runtime(client, player, cmd_rx, &events));

    RuntimeHandle {
        commands: cmd_tx,
        runtime_thread: Some(runtime_thread),
        forwarder_thread: Some(forwarder_thread),
    }
}

/// The player-event fan-out forwarder body (the M2 single-consumer resolution).
///
/// Blocks on the player's `PlayerEvent` receiver and re-publishes each event as
/// an [`AppEvent`] onto the UI sink. Exits when the player's sender is dropped
/// (`recv` returns `Err`) — i.e. when the runtime thread drops the `Player`.
///
/// # Adding a second sink (M6)
///
/// MPRIS needs the same event stream. The minimal change is to give this
/// function a second `sink` parameter (e.g. an `mpsc::Sender<MprisUpdate>`) and,
/// for each translated event, send to both. Because the translation is a pure
/// `PlayerEvent -> AppEvent` map (see [`translate_player_event`]), fanning out
/// is a second `send` per event; no `broadcast` channel is required until the
/// shapes diverge.
fn run_player_forwarder(
    player_events: std::sync::mpsc::Receiver<PlayerEvent>,
    ui_sink: &StdSender<AppEvent>,
) {
    while let Ok(event) = player_events.recv() {
        let app_event = translate_player_event(event);
        // M6: a second sink (MPRIS) is fed here, alongside the UI sink.
        if ui_sink.send(app_event).is_err() {
            // UI receiver gone — the app is shutting down; stop forwarding.
            break;
        }
    }
}

/// Pure map from a player-layer [`PlayerEvent`] to a UI-layer [`AppEvent`].
///
/// Kept as a free function so the fan-out translation is unit-testable without
/// threads, and so M6's second sink reuses the exact same mapping.
fn translate_player_event(event: PlayerEvent) -> AppEvent {
    match event {
        PlayerEvent::Progress(secs) => AppEvent::PlayerProgress(secs),
        PlayerEvent::Duration(secs) => AppEvent::PlayerDuration(secs),
        PlayerEvent::Volume(vol) => AppEvent::PlayerVolume(vol),
        PlayerEvent::Mute(muted) => AppEvent::PlayerMute(muted),
        PlayerEvent::Pause(paused) => AppEvent::PlayerPaused(paused),
        PlayerEvent::TrackEnded => AppEvent::TrackEnded,
        PlayerEvent::TrackError(detail) => AppEvent::TrackError(detail),
        PlayerEvent::Started => AppEvent::PlayerStarted,
        // `Loaded` (mpv file-loaded) has no distinct UI consumer yet; collapse
        // it onto PlayerStarted so the bar can react to "a track is now active".
        // TODO(M5b+): split if a view ever needs start-of-load vs loaded.
        PlayerEvent::Loaded => AppEvent::PlayerStarted,
    }
}

/// The runtime thread body: build a tokio runtime and run the command loop.
///
/// Owns the `Player`, the `QueueManager`, and the optional `InnerTubeClient`
/// for the whole session. The `Player` is dropped when this function returns,
/// which stops its event thread and, in turn, ends the forwarder thread.
fn run_runtime(
    client: Option<InnerTubeClient>,
    mut player: Player,
    mut commands: UnboundedReceiver<AppCommand>,
    events: &StdSender<AppEvent>,
) {
    let runtime = match Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            let _ = events.send(AppEvent::ApiError(format!("runtime init failed: {e}")));
            return;
        }
    };

    // The queue lives here, single-owned alongside the player (see module docs).
    let mut queue = QueueManager::new();

    runtime.block_on(async move {
        // Spawn the MPRIS server on THIS runtime (the single-executor rule from
        // the spike). The inbound-control channel relays D-Bus method calls
        // (PlayPause/Next/Previous/Stop) back here; the select loop below maps
        // them to commands. A `None` handle means MPRIS init failed (no session
        // bus, name taken) — already toasted once, the app runs without it.
        let (control_tx, mut control_rx) = mpsc::unbounded_channel::<mpris::MprisControl>();
        let mpris = mpris::spawn_mpris(control_tx, events.clone()).await;
        // Publish the initial (idle) state so probes have something to read.
        push_mpris_state(mpris.as_ref(), &queue, &player);

        loop {
            // Concurrently await a UI command or an inbound MPRIS control. The
            // control is mapped to the same AppCommand the keymap would produce,
            // so both paths converge on one match.
            let command = tokio::select! {
                cmd = commands.recv() => match cmd {
                    Some(cmd) => cmd,
                    // The UI command sender is gone — the app is exiting.
                    None => break,
                },
                control = control_rx.recv() => match control {
                    Some(control) => control_to_command(control),
                    // The control channel only closes when the MPRIS interface
                    // (and its server) is dropped; keep serving UI commands.
                    None => continue,
                },
            };
            match command {
                AppCommand::Quit => break,
                AppCommand::FetchHome => handle_fetch_home(client.as_ref(), events).await,
                AppCommand::FetchLibraryPlaylists => {
                    handle_fetch_library_playlists(client.as_ref(), events).await;
                }
                AppCommand::FetchPlaylistTracks { playlist_id, title } => {
                    handle_fetch_playlist_tracks(client.as_ref(), &playlist_id, title, events)
                        .await;
                }
                AppCommand::Search { query, filter } => {
                    handle_search(client.as_ref(), &query, filter.as_deref(), events).await;
                }
                AppCommand::FetchLibraryAlbums => {
                    handle_fetch_library_albums(client.as_ref(), events).await;
                }
                AppCommand::FetchLibraryArtists => {
                    handle_fetch_library_artists(client.as_ref(), events).await;
                }
                AppCommand::FetchAlbum(browse_id) => {
                    handle_fetch_album(client.as_ref(), &browse_id, events).await;
                }
                AppCommand::FetchArtist(channel_id) => {
                    handle_fetch_artist(client.as_ref(), &channel_id, events).await;
                }
                AppCommand::FetchLyrics(video_id) => {
                    handle_fetch_lyrics(client.as_ref(), &video_id, events).await;
                }
                AppCommand::FetchHistory => {
                    handle_fetch_history(client.as_ref(), events).await;
                }
                AppCommand::FetchQueue => emit_queue_snapshot(&queue, events),
                AppCommand::CheckSession => handle_check_session(client.as_ref(), events).await,
                AppCommand::Play(track) => {
                    play_single(&mut queue, &mut player, track, events);
                }
                AppCommand::PlayPlaylist {
                    tracks,
                    start_index,
                } => {
                    play_playlist(&mut queue, &mut player, tracks, start_index, events);
                }
                AppCommand::NextTrack => advance_queue(&mut queue, &mut player, events),
                AppCommand::PreviousTrack => rewind_queue(&mut queue, &mut player, events),
                AppCommand::ToggleShuffle => {
                    queue.toggle_shuffle();
                    emit_now_playing(&queue, events);
                }
                AppCommand::CycleRepeat => {
                    queue.cycle_repeat();
                    emit_now_playing(&queue, events);
                }
                AppCommand::TogglePause => {
                    report_player_result(player.toggle_pause(), events);
                }
                AppCommand::SetVolume(vol) => {
                    report_player_result(player.set_volume(vol), events);
                }
                AppCommand::AdjustVolume(delta) => {
                    report_player_result(player.adjust_volume(delta), events);
                }
                AppCommand::SeekForward => seek_relative(&player, SEEK_STEP_SECONDS),
                AppCommand::SeekBackward => seek_relative(&player, -SEEK_STEP_SECONDS),
                AppCommand::SeekToStart => seek_to_start(&player),
                AppCommand::ToggleMute => {
                    report_player_result(player.toggle_mute(), events);
                }
                AppCommand::CycleAudioQuality => match player.cycle_audio_quality() {
                    Ok(quality) => {
                        let _ = events.send(AppEvent::AudioQualityChanged(quality.to_owned()));
                    }
                    Err(err) => {
                        let _ = events.send(AppEvent::TrackError(err.to_string()));
                    }
                },
                AppCommand::AddToQueue(track) => {
                    let title = track.title.clone();
                    queue.add(track);
                    // Reflect the addition in an open queue view.
                    emit_queue_snapshot(&queue, events);
                    let _ = events.send(AppEvent::ActionResult(format!("Added to queue: {title}")));
                }
                AppCommand::StartRadio(video_id) => {
                    handle_start_radio(client.as_ref(), &video_id, &mut queue, &mut player, events)
                        .await;
                }
                AppCommand::SearchAndOpenArtist(name) => {
                    handle_search_and_open_artist(client.as_ref(), &name, events).await;
                }
                AppCommand::SearchAndOpenAlbum { name, artist } => {
                    handle_search_and_open_album(client.as_ref(), &name, &artist, events).await;
                }
                AppCommand::ToggleLike(video_id) => {
                    handle_toggle_like(client.as_ref(), &video_id, events).await;
                }
                AppCommand::RatePlaylist {
                    playlist_id,
                    status,
                } => {
                    handle_rate_playlist(client.as_ref(), &playlist_id, &status, events).await;
                }
                AppCommand::AddToPlaylist {
                    playlist_id,
                    video_id,
                } => {
                    handle_add_to_playlist(client.as_ref(), &playlist_id, &video_id, events).await;
                }
                AppCommand::CreatePlaylistAndAdd { title, video_id } => {
                    handle_create_playlist_and_add(client.as_ref(), &title, &video_id, events)
                        .await;
                }
                AppCommand::RemoveFromQueue(video_id) => {
                    remove_from_queue(&mut queue, &video_id, events);
                }
                AppCommand::RemoveFromPlaylist {
                    playlist_id,
                    video_id,
                } => {
                    handle_remove_from_playlist(client.as_ref(), &playlist_id, &video_id, events)
                        .await;
                }
                AppCommand::FollowArtist(name) => {
                    handle_follow_artist(client.as_ref(), &name, events).await;
                }
                AppCommand::UnfollowArtist(name) => {
                    handle_unfollow_artist(client.as_ref(), &name, events).await;
                }
            }
            // After every command, push a fresh MPRIS snapshot built from the
            // queue + the live player state. The MPRIS task diffs it against the
            // last and emits ONLY changed properties, so pushing unconditionally
            // here is cheap (a no-op when nothing playback-relevant changed) and
            // keeps the wiring to a single call site instead of one per arm.
            push_mpris_state(mpris.as_ref(), &queue, &player);
        }
        // The MPRIS tasks die via RAII when this block_on returns and the
        // runtime drops (task abort closes the zbus connection, releasing the
        // bus name before `player` drops below). The Shutdown message is a
        // best-effort nudge; the abort usually wins the race — both are clean.
        if let Some(handle) = mpris.as_ref() {
            handle.shutdown();
        }
    });
}

/// Build an [`MprisState`] from the queue + live player state and push it to the
/// MPRIS task (a no-op when MPRIS is unavailable).
///
/// The queue supplies the static-per-track metadata (title/artist/album/art/
/// duration) and the modes (shuffle/repeat); the player supplies the live flags
/// (is_playing, volume, position). This is the Rust analogue of Python's
/// `MprisService.update(state, track, shuffle, repeat_mode)` — called from the
/// runtime side with full state, never from the raw mpv event feed.
fn push_mpris_state(mpris: Option<&MprisHandle>, queue: &QueueManager, player: &Player) {
    let Some(handle) = mpris else {
        return;
    };
    let player_state = player.get_state();
    let snapshot = match queue.current_track() {
        Some(track) => MprisState::from_now_playing(
            player_state.is_playing,
            track.video_id.clone(),
            track.title.clone(),
            track.artist.clone(),
            track.album.clone(),
            track.thumbnail_url.clone(),
            track.duration_seconds,
            player_state.volume,
            queue.repeat_mode(),
            queue.shuffle(),
            player_state.position,
        ),
        None => MprisState::from_now_playing(
            false,
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            0.0,
            player_state.volume,
            queue.repeat_mode(),
            queue.shuffle(),
            0.0,
        ),
    };
    handle.send(snapshot);
}

// ---------------------------------------------------------------------------
// Queue-driven playback (pure-ish helpers; the queue logic itself is in queue.rs)
// ---------------------------------------------------------------------------

/// Build a [`NowPlaying`] snapshot from the queue's current state.
///
/// Returns the idle snapshot (empty strings, the queue's modes) when no track
/// is selected, so the bar clears its metadata at end-of-queue. Mirrors the
/// enrichment Python's `_poll_player_state` did from `queue.current_track`.
fn now_playing_snapshot(queue: &QueueManager) -> NowPlaying {
    match queue.current_track() {
        Some(track) => NowPlaying {
            title: track.title.clone(),
            artist: track.artist.clone(),
            album: track.album.clone(),
            video_id: track.video_id.clone(),
            duration_seconds: track.duration_seconds,
            shuffle: queue.shuffle(),
            repeat: queue.repeat_mode(),
        },
        None => NowPlaying {
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            video_id: String::new(),
            duration_seconds: 0.0,
            shuffle: queue.shuffle(),
            repeat: queue.repeat_mode(),
        },
    }
}

/// Emit the queue's current now-playing snapshot to the UI.
fn emit_now_playing(queue: &QueueManager, events: &StdSender<AppEvent>) {
    let _ = events.send(AppEvent::NowPlaying(now_playing_snapshot(queue)));
}

/// Emit a snapshot of the current queue state (the response to
/// [`AppCommand::FetchQueue`], and after any queue-mutating command). The
/// current track's index is derived from the queue's `current_track()`, which
/// already handles the -1 sentinel.
fn emit_queue_snapshot(queue: &QueueManager, events: &StdSender<AppEvent>) {
    let tracks = queue.tracks();
    let current_index = queue
        .current_track()
        .and_then(|ct| tracks.iter().position(|t| t == ct));
    let _ = events.send(AppEvent::QueueSnapshot(QueueSnapshot {
        tracks,
        current_index,
    }));
}

/// Remove the queue track with `video_id` and emit an updated snapshot.
///
/// Mirrors Python's `_remove_from_queue`: find the *first* matching track by
/// `video_id` and remove it (the index is recomputed runtime-side rather than
/// passed from the UI, so a stale queue-view snapshot can never remove the wrong
/// row). A missing track is a no-op confirmation toast; a successful removal
/// emits a fresh [`AppEvent::QueueSnapshot`] so an open queue view re-renders.
fn remove_from_queue(queue: &mut QueueManager, video_id: &str, events: &StdSender<AppEvent>) {
    let index = queue.tracks().iter().position(|t| t.video_id == video_id);
    match index {
        Some(i) => {
            // `remove` only errors on an out-of-range index, which `position`
            // cannot produce; ignore the Result rather than toast a can't-happen.
            let _ = queue.remove(i);
            emit_queue_snapshot(queue, events);
            let _ = events.send(AppEvent::ActionResult("Removed from queue".to_owned()));
        }
        None => {
            let _ = events.send(AppEvent::ActionResult("Track not in the queue".to_owned()));
        }
    }
}

/// Play a single track: replace the queue with `[track]` and start it.
fn play_single(
    queue: &mut QueueManager,
    player: &mut Player,
    track: Track,
    events: &StdSender<AppEvent>,
) {
    queue.set_playlist(vec![track], 0);
    play_current(queue, player, events);
}

/// Play a playlist from `start_index`, queueing the remainder (spotify_player).
fn play_playlist(
    queue: &mut QueueManager,
    player: &mut Player,
    tracks: Vec<Track>,
    start_index: usize,
    events: &StdSender<AppEvent>,
) {
    queue.set_playlist(tracks, start_index);
    play_current(queue, player, events);
}

/// Advance to the next track and play it, or go idle when the queue is
/// exhausted. The auto-advance target (also the `n` key). Mirrors Python
/// `action_next_track` / `_on_track_end`: only plays when `next_track` returns
/// a track.
///
/// Note on `RepeatMode::One`: `next_track` returns the *current* track again
/// (the queue keeps its index), so this re-issues `play_current` for the same
/// `video_id`. That is the intended "repeat one" behaviour and is a faithful
/// 1:1 port of Python's `_on_track_end` → `player.play(next.video_id)`: a fresh
/// `loadfile replace` replays the track. Because EOF only fires at the *end* of
/// a track, this is one reload per playthrough — not a tight loop — and matches
/// the Python client exactly.
fn advance_queue(queue: &mut QueueManager, player: &mut Player, events: &StdSender<AppEvent>) {
    if queue.next_track().is_some() {
        play_current(queue, player, events);
    } else {
        // Queue exhausted (repeat Off at the end): the UI's TrackEnded fold
        // already returned the bar to idle; emit an idle NowPlaying so the
        // metadata (title/artist/album) clears too.
        emit_now_playing(queue, events);
    }
}

/// Go back one track and play it (the `p` key). Mirrors Python
/// `action_previous_track`: `previous_track` clamps at the start.
fn rewind_queue(queue: &mut QueueManager, player: &mut Player, events: &StdSender<AppEvent>) {
    if queue.previous_track().is_some() {
        play_current(queue, player, events);
    }
}

/// Play whatever the queue currently points at and announce it.
///
/// A `None` current track (empty queue) is a no-op. A failed `player.play`
/// surfaces a [`AppEvent::TrackError`] toast but the `NowPlaying` is still
/// emitted, so the bar shows the track the user *meant* to hear next to the
/// error (matching Python, where the bar updates from the queue regardless of
/// the mpv result).
fn play_current(queue: &QueueManager, player: &mut Player, events: &StdSender<AppEvent>) {
    let Some(video_id) = queue.current_track().map(|t| t.video_id.clone()) else {
        return;
    };
    emit_now_playing(queue, events);
    report_player_result(player.play(&video_id), events);
}

/// Fetch the home page and emit the result (or a classified error).
async fn handle_fetch_home(client: Option<&InnerTubeClient>, events: &StdSender<AppEvent>) {
    let Some(client) = client else {
        let _ = events.send(AppEvent::ApiError(
            "Not signed in — run: ytmusic-tui auth".to_owned(),
        ));
        return;
    };
    let event = match client.get_home().await {
        Ok(sections) => AppEvent::HomeLoaded(sections),
        Err(err) => AppEvent::ApiError(ytmusic_api::classify_api_error(&err)),
    };
    let _ = events.send(event);
}

/// Default page size for the library-playlists fetch, matching Python's
/// `get_library_playlists(limit=50)` call in `_show_playlist_picker` and the
/// playlist view's library list.
const LIBRARY_PLAYLISTS_LIMIT: usize = 50;

/// Fetch the user's library playlists and emit the result (or a classified
/// error). The playlist view's level-1 list.
async fn handle_fetch_library_playlists(
    client: Option<&InnerTubeClient>,
    events: &StdSender<AppEvent>,
) {
    let Some(client) = client else {
        let _ = events.send(AppEvent::ApiError(
            "Not signed in — run: ytmusic-tui auth".to_owned(),
        ));
        return;
    };
    let event = match client.get_library_playlists(LIBRARY_PLAYLISTS_LIMIT).await {
        Ok(playlists) => AppEvent::LibraryPlaylistsLoaded(playlists),
        Err(err) => AppEvent::ApiError(ytmusic_api::classify_api_error(&err)),
    };
    let _ = events.send(event);
}

/// Fetch a single playlist's tracks and emit the result (or a classified
/// error). `title` is echoed back so the view can label the list. The playlist
/// view's level-2 list.
async fn handle_fetch_playlist_tracks(
    client: Option<&InnerTubeClient>,
    playlist_id: &str,
    title: String,
    events: &StdSender<AppEvent>,
) {
    let Some(client) = client else {
        let _ = events.send(AppEvent::ApiError(
            "Not signed in — run: ytmusic-tui auth".to_owned(),
        ));
        return;
    };
    let event = match client.get_playlist_tracks(playlist_id).await {
        Ok(tracks) => AppEvent::PlaylistTracksLoaded { title, tracks },
        Err(err) => AppEvent::ApiError(ytmusic_api::classify_api_error(&err)),
    };
    let _ = events.send(event);
}

/// Per-category result cap for a search, matching Python's
/// `search_all(query, limit=20, ...)` in `SearchView._run_search`.
const SEARCH_LIMIT: usize = 20;

/// Default page size for the library albums/artists fetches.
/// Mirrors the Python view's `get_library_*` calls (ytmusicapi defaults to 25
/// for these); a generous cap keeps the panes populated without paging.
const LIBRARY_LIMIT: usize = 50;

/// Run a search and emit the categorized results (or a classified error).
///
/// `filter` is the optional `#category:` restriction (`"songs"` / `"albums"` /
/// `"artists"` / `"playlists"`); `None` searches across all categories. The
/// search view fills all four panes from the [`SearchResults`].
async fn handle_search(
    client: Option<&InnerTubeClient>,
    query: &str,
    filter: Option<&str>,
    events: &StdSender<AppEvent>,
) {
    let Some(client) = client else {
        let _ = events.send(AppEvent::ApiError(
            "Not signed in — run: ytmusic-tui auth".to_owned(),
        ));
        return;
    };
    let event = match client.search_all(query, SEARCH_LIMIT, filter).await {
        Ok(results) => AppEvent::SearchLoaded(results),
        Err(err) => AppEvent::ApiError(ytmusic_api::classify_api_error(&err)),
    };
    let _ = events.send(event);
}

/// Fetch the user's library albums and emit the result (or a classified error).
async fn handle_fetch_library_albums(
    client: Option<&InnerTubeClient>,
    events: &StdSender<AppEvent>,
) {
    let Some(client) = client else {
        let _ = events.send(AppEvent::ApiError(
            "Not signed in — run: ytmusic-tui auth".to_owned(),
        ));
        return;
    };
    let event = match client.get_library_albums(LIBRARY_LIMIT).await {
        Ok(albums) => AppEvent::LibraryAlbumsLoaded(albums),
        Err(err) => AppEvent::ApiError(ytmusic_api::classify_api_error(&err)),
    };
    let _ = events.send(event);
}

/// Fetch the user's library artists and emit the result (or a classified error).
async fn handle_fetch_library_artists(
    client: Option<&InnerTubeClient>,
    events: &StdSender<AppEvent>,
) {
    let Some(client) = client else {
        let _ = events.send(AppEvent::ApiError(
            "Not signed in — run: ytmusic-tui auth".to_owned(),
        ));
        return;
    };
    let event = match client.get_library_artists(LIBRARY_LIMIT).await {
        Ok(artists) => AppEvent::LibraryArtistsLoaded(artists),
        Err(err) => AppEvent::ApiError(ytmusic_api::classify_api_error(&err)),
    };
    let _ = events.send(event);
}

/// Run the session canary and emit [`AppEvent::SessionInvalid`] only on failure.
///
/// A missing client (auth not loaded) is treated as invalid, so the UI shows
/// the same "sign in" prompt whether the auth file was absent or expired. A
/// valid session is silent — the UI assumes validity unless told otherwise,
/// matching `is_session_valid`'s "assume valid on transient error" contract.
async fn handle_check_session(client: Option<&InnerTubeClient>, events: &StdSender<AppEvent>) {
    let valid = match client {
        Some(client) => client.is_session_valid().await,
        None => false,
    };
    if !valid {
        let _ = events.send(AppEvent::SessionInvalid);
    }
}

/// Surface a failed player operation as an [`AppEvent::TrackError`] toast.
///
/// A successful operation produces no event (the resulting state change arrives
/// via the player's own event stream).
fn report_player_result(
    result: Result<(), crate::player::PlayerError>,
    events: &StdSender<AppEvent>,
) {
    if let Err(err) = result {
        let _ = events.send(AppEvent::TrackError(err.to_string()));
    }
}

/// Seek step in seconds for the `>` / `<` keys, matching Python's `_SEEK_STEP`.
const SEEK_STEP_SECONDS: f64 = 5.0;

/// Seek `seconds` relative to the current position, mirroring Python's
/// `_seek_relative`: a no-op when nothing is playing, and a swallowed error
/// otherwise.
///
/// The stream may not be seekable yet while the ytdl-hook is still resolving the
/// URL; mpv rejects that seek with `MPV_ERROR_COMMAND`. Python wrapped the seek
/// in `contextlib.suppress(Exception)` precisely for this transient case, so the
/// error is dropped rather than toasted (it is not a user-actionable failure).
fn seek_relative(player: &Player, seconds: f64) {
    if player.get_state().video_id.is_empty() {
        return;
    }
    let _ = player.seek(seconds);
}

/// Seek to the start of the current track, mirroring Python's `action_seek_start`
/// (`seek_absolute(0.0)` behind the same `video_id` guard + error suppression).
fn seek_to_start(player: &Player) {
    if player.get_state().video_id.is_empty() {
        return;
    }
    let _ = player.seek_absolute(0.0);
}

/// Fetch a single album by `browse_id` and emit the result (or a classified
/// error). Mirrors Python `api.get_album(browse_id)`.
async fn handle_fetch_album(
    client: Option<&InnerTubeClient>,
    browse_id: &str,
    events: &StdSender<AppEvent>,
) {
    let Some(client) = client else {
        let _ = events.send(AppEvent::ApiError(
            "Not signed in — run: ytmusic-tui auth".to_owned(),
        ));
        return;
    };
    let event = match client.get_album(browse_id).await {
        Ok(album) => AppEvent::AlbumLoaded(album),
        Err(err) => AppEvent::ApiError(ytmusic_api::classify_api_error(&err)),
    };
    let _ = events.send(event);
}

/// Fetch an artist page by `channel_id` and emit the result (or a classified
/// error). Mirrors Python `api.get_artist(channel_id)`.
async fn handle_fetch_artist(
    client: Option<&InnerTubeClient>,
    channel_id: &str,
    events: &StdSender<AppEvent>,
) {
    let Some(client) = client else {
        let _ = events.send(AppEvent::ApiError(
            "Not signed in — run: ytmusic-tui auth".to_owned(),
        ));
        return;
    };
    let event = match client.get_artist(channel_id).await {
        Ok(artist) => AppEvent::ArtistLoaded(artist),
        Err(err) => AppEvent::ApiError(ytmusic_api::classify_api_error(&err)),
    };
    let _ = events.send(event);
}

/// Fetch lyrics for `video_id` and emit the result (or a classified error).
/// `None` from the API means "no lyrics available" — a valid loaded state, not
/// an error. Mirrors Python `api.get_lyrics(video_id)`.
async fn handle_fetch_lyrics(
    client: Option<&InnerTubeClient>,
    video_id: &str,
    events: &StdSender<AppEvent>,
) {
    let Some(client) = client else {
        let _ = events.send(AppEvent::ApiError(
            "Not signed in — run: ytmusic-tui auth".to_owned(),
        ));
        return;
    };
    let event = match client.get_lyrics(video_id).await {
        Ok(lyrics) => AppEvent::LyricsLoaded(lyrics),
        Err(err) => AppEvent::ApiError(ytmusic_api::classify_api_error(&err)),
    };
    let _ = events.send(event);
}

/// Fetch the user's listening history and emit the result (or a classified
/// error). Mirrors Python `api.get_history()`.
async fn handle_fetch_history(client: Option<&InnerTubeClient>, events: &StdSender<AppEvent>) {
    let Some(client) = client else {
        let _ = events.send(AppEvent::ApiError(
            "Not signed in — run: ytmusic-tui auth".to_owned(),
        ));
        return;
    };
    let event = match client.get_history().await {
        Ok(tracks) => AppEvent::HistoryLoaded(tracks),
        Err(err) => AppEvent::ApiError(ytmusic_api::classify_api_error(&err)),
    };
    let _ = events.send(event);
}

/// Default radio length, matching Python's `get_radio(video_id)` (ytmusicapi
/// returns ~25 watch-playlist tracks by default).
const RADIO_LIMIT: usize = 25;

/// Start a radio seeded by `video_id`: fetch the radio tracks and play them as a
/// fresh queue. Mirrors Python's `_start_radio` → `_queue_and_play`. An empty
/// result is a warning toast, not an error.
async fn handle_start_radio(
    client: Option<&InnerTubeClient>,
    video_id: &str,
    queue: &mut QueueManager,
    player: &mut Player,
    events: &StdSender<AppEvent>,
) {
    let Some(client) = client else {
        let _ = events.send(AppEvent::ApiError(
            "Not signed in — run: ytmusic-tui auth".to_owned(),
        ));
        return;
    };
    match client.get_radio(video_id, RADIO_LIMIT).await {
        Ok(tracks) if tracks.is_empty() => {
            let _ = events.send(AppEvent::ActionResult(
                "Radio returned no tracks".to_owned(),
            ));
        }
        Ok(tracks) => {
            play_playlist(queue, player, tracks, 0, events);
            let _ = events.send(AppEvent::ActionResult("Started radio".to_owned()));
        }
        Err(err) => {
            let _ = events.send(AppEvent::ApiError(ytmusic_api::classify_api_error(&err)));
        }
    }
}

/// Per-lookup search cap for the `a` / `A` resolution, matching Python's
/// `search_all(name, limit=5, filter=...)` in `_lookup_and_open_*`.
const LOOKUP_SEARCH_LIMIT: usize = 5;

/// Resolve an artist by `name` and emit [`AppEvent::ArtistResolved`] with the
/// first hit's `channel_id`. Mirrors Python's `_lookup_and_open_artist`: search
/// the name with the `artists` filter, take the first result that carries a
/// `channel_id`, and let the UI navigate. An empty result is a warning toast;
/// an API error is classified. The actual artist fetch is chained UI-side (the
/// `ArtistResolved` fold returns `FetchArtist`).
async fn handle_search_and_open_artist(
    client: Option<&InnerTubeClient>,
    name: &str,
    events: &StdSender<AppEvent>,
) {
    let Some(client) = client else {
        let _ = events.send(AppEvent::ApiError(
            "Not signed in — run: ytmusic-tui auth".to_owned(),
        ));
        return;
    };
    match client
        .search_all(name, LOOKUP_SEARCH_LIMIT, Some("artists"))
        .await
    {
        Ok(results) => {
            match results
                .artists
                .into_iter()
                .find(|a| !a.channel_id.is_empty())
            {
                Some(artist) => {
                    let _ = events.send(AppEvent::ArtistResolved(artist.channel_id));
                }
                None => {
                    let _ = events.send(AppEvent::ApiError(format!("Artist not found: {name}")));
                }
            }
        }
        Err(err) => {
            let _ = events.send(AppEvent::ApiError(ytmusic_api::classify_api_error(&err)));
        }
    }
}

/// Resolve an album by `name` (optionally disambiguated by `artist`) and emit
/// [`AppEvent::AlbumResolved`] with the first hit's `browse_id`. Mirrors
/// Python's `_lookup_and_open_album`: search `"{name} {artist}"` with the
/// `albums` filter, take the first result that carries a `browse_id`.
async fn handle_search_and_open_album(
    client: Option<&InnerTubeClient>,
    name: &str,
    artist: &str,
    events: &StdSender<AppEvent>,
) {
    let Some(client) = client else {
        let _ = events.send(AppEvent::ApiError(
            "Not signed in — run: ytmusic-tui auth".to_owned(),
        ));
        return;
    };
    let query = format!("{name} {artist}");
    let query = query.trim();
    match client
        .search_all(query, LOOKUP_SEARCH_LIMIT, Some("albums"))
        .await
    {
        Ok(results) => match results.albums.into_iter().find(|a| !a.browse_id.is_empty()) {
            Some(album) => {
                let _ = events.send(AppEvent::AlbumResolved(album.browse_id));
            }
            None => {
                let _ = events.send(AppEvent::ApiError(format!("Album not found: {name}")));
            }
        },
        Err(err) => {
            let _ = events.send(AppEvent::ApiError(ytmusic_api::classify_api_error(&err)));
        }
    }
}

/// Toggle the like state of `video_id`: read the current status and flip it.
/// Mirrors Python's `_toggle_like` (`INDIFFERENT if status == "LIKE" else
/// "LIKE"`).
async fn handle_toggle_like(
    client: Option<&InnerTubeClient>,
    video_id: &str,
    events: &StdSender<AppEvent>,
) {
    let Some(client) = client else {
        let _ = events.send(AppEvent::ApiError(
            "Not signed in — run: ytmusic-tui auth".to_owned(),
        ));
        return;
    };
    let status = match client.get_like_status(video_id).await {
        Ok(status) => status,
        Err(err) => {
            let _ = events.send(AppEvent::ApiError(ytmusic_api::classify_api_error(&err)));
            return;
        }
    };
    let new_status = if status.as_deref() == Some("LIKE") {
        "INDIFFERENT"
    } else {
        "LIKE"
    };
    match client.rate_track(video_id, new_status).await {
        Ok(()) => {
            let toast = if new_status == "LIKE" {
                "Liked"
            } else {
                "Like removed"
            };
            let _ = events.send(AppEvent::ActionResult(toast.to_owned()));
        }
        Err(err) => {
            let _ = events.send(AppEvent::ApiError(ytmusic_api::classify_api_error(&err)));
        }
    }
}

/// Save / remove `playlist_id` from the user's library (issue #12).
///
/// Mirrors [`handle_toggle_like`] but for albums / playlists: the same
/// `like/{like,removelike}` endpoint family, with `{target: {playlistId}}`
/// instead of `{target: {videoId}}`. Unlike the track toggle there is no
/// `get_like_status` equivalent for playlists, so `status` is passed in by
/// the caller (the popup surfaces both "Save" and "Remove" so the user
/// picks the direction).
async fn handle_rate_playlist(
    client: Option<&InnerTubeClient>,
    playlist_id: &str,
    status: &str,
    events: &StdSender<AppEvent>,
) {
    let Some(client) = client else {
        let _ = events.send(AppEvent::ApiError(
            "Not signed in — run: ytmusic-tui auth".to_owned(),
        ));
        return;
    };
    match client.rate_playlist(playlist_id, status).await {
        Ok(()) => {
            let toast = if status == "LIKE" {
                "Saved to library"
            } else {
                "Removed from library"
            };
            let _ = events.send(AppEvent::ActionResult(toast.to_owned()));
        }
        Err(err) => {
            let _ = events.send(AppEvent::ApiError(ytmusic_api::classify_api_error(&err)));
        }
    }
}

/// Add `video_id` to the playlist `playlist_id`. Mirrors Python's
/// `add_playlist_items(playlist_id, [video_id])`.
async fn handle_add_to_playlist(
    client: Option<&InnerTubeClient>,
    playlist_id: &str,
    video_id: &str,
    events: &StdSender<AppEvent>,
) {
    let Some(client) = client else {
        let _ = events.send(AppEvent::ApiError(
            "Not signed in — run: ytmusic-tui auth".to_owned(),
        ));
        return;
    };
    match client
        .add_playlist_items(playlist_id, &[video_id.to_owned()])
        .await
    {
        Ok(()) => {
            let _ = events.send(AppEvent::ActionResult("Added to playlist".to_owned()));
        }
        Err(err) => {
            let _ = events.send(AppEvent::ApiError(ytmusic_api::classify_api_error(&err)));
        }
    }
}

/// Remove `video_id` from the playlist `playlist_id`. Mirrors Python's
/// `_remove_from_playlist` (`remove_playlist_items(playlist_id, [video_id])`).
async fn handle_remove_from_playlist(
    client: Option<&InnerTubeClient>,
    playlist_id: &str,
    video_id: &str,
    events: &StdSender<AppEvent>,
) {
    let Some(client) = client else {
        let _ = events.send(AppEvent::ApiError(
            "Not signed in — run: ytmusic-tui auth".to_owned(),
        ));
        return;
    };
    match client
        .remove_playlist_items(playlist_id, &[video_id.to_owned()])
        .await
    {
        Ok(()) => {
            let _ = events.send(AppEvent::ActionResult("Removed from playlist".to_owned()));
        }
        Err(err) => {
            let _ = events.send(AppEvent::ApiError(ytmusic_api::classify_api_error(&err)));
        }
    }
}

/// Whether [`resolve_artist_for_subscription`] should follow or unfollow.
enum SubscriptionOp {
    Follow,
    Unfollow,
}

/// Resolve an artist by display `name` to a channel id via search.
///
/// Mirrors the resolution half of [`handle_search_and_open_artist`]: search
/// `name` with the `artists` filter, take the first result that carries a
/// non-empty `channel_id`. Returns `None` after emitting the appropriate
/// [`AppEvent::ApiError`] toast on any failure path (missing client, empty
/// result, transport error).
async fn resolve_artist_channel_id(
    client: Option<&InnerTubeClient>,
    name: &str,
    events: &StdSender<AppEvent>,
) -> Option<String> {
    let client = match client {
        Some(c) => c,
        None => {
            let _ = events.send(AppEvent::ApiError(
                "Not signed in — run: ytmusic-tui auth".to_owned(),
            ));
            return None;
        }
    };
    match client
        .search_all(name, LOOKUP_SEARCH_LIMIT, Some("artists"))
        .await
    {
        Ok(results) => match results
            .artists
            .into_iter()
            .find(|a| !a.channel_id.is_empty())
        {
            Some(artist) => Some(artist.channel_id),
            None => {
                let _ = events.send(AppEvent::ApiError(format!("Artist not found: {name}")));
                None
            }
        },
        Err(err) => {
            let _ = events.send(AppEvent::ApiError(ytmusic_api::classify_api_error(&err)));
            None
        }
    }
}

/// Resolve `name` to a channel id and run the requested subscription op,
/// emitting an [`AppEvent::ActionResult`] toast on success or an
/// [`AppEvent::ApiError`] on failure.
async fn resolve_artist_for_subscription(
    client: Option<&InnerTubeClient>,
    name: &str,
    op: SubscriptionOp,
    events: &StdSender<AppEvent>,
) {
    let Some(channel_id) = resolve_artist_channel_id(client, name, events).await else {
        return;
    };
    // `resolve_artist_channel_id` returns Some only when `client` was Some, so
    // the next unwrap-via-guard cannot fail.
    let Some(client) = client else {
        return;
    };
    let ids = [channel_id];
    let result = match op {
        SubscriptionOp::Follow => client.subscribe_artists(&ids).await,
        SubscriptionOp::Unfollow => client.unsubscribe_artists(&ids).await,
    };
    match result {
        Ok(()) => {
            let toast = match op {
                SubscriptionOp::Follow => format!("Following {name}"),
                SubscriptionOp::Unfollow => format!("Unfollowed {name}"),
            };
            let _ = events.send(AppEvent::ActionResult(toast));
        }
        Err(err) => {
            let _ = events.send(AppEvent::ApiError(ytmusic_api::classify_api_error(&err)));
        }
    }
}

/// Follow (subscribe to) an artist by display `name`. Mirrors issue #11's
/// "Follow artist" action: resolve the name to a channel id via search, then
/// call `subscribe_artists`.
async fn handle_follow_artist(
    client: Option<&InnerTubeClient>,
    name: &str,
    events: &StdSender<AppEvent>,
) {
    resolve_artist_for_subscription(client, name, SubscriptionOp::Follow, events).await;
}

/// Unfollow (unsubscribe from) an artist by display `name`. Mirrors issue
/// #11's "Unfollow artist" action: resolve the name to a channel id via
/// search, then call `unsubscribe_artists`.
async fn handle_unfollow_artist(
    client: Option<&InnerTubeClient>,
    name: &str,
    events: &StdSender<AppEvent>,
) {
    resolve_artist_for_subscription(client, name, SubscriptionOp::Unfollow, events).await;
}

/// Privacy level for playlists created from the picker's "New playlist…" choice,
/// matching Python's default `create_playlist(..., "PRIVATE")`.
const NEW_PLAYLIST_PRIVACY: &str = "PRIVATE";

/// Create a playlist titled `title` and add `video_id` to it. Mirrors Python's
/// create-then-add flow for the picker's "New playlist…" choice.
async fn handle_create_playlist_and_add(
    client: Option<&InnerTubeClient>,
    title: &str,
    video_id: &str,
    events: &StdSender<AppEvent>,
) {
    let Some(client) = client else {
        let _ = events.send(AppEvent::ApiError(
            "Not signed in — run: ytmusic-tui auth".to_owned(),
        ));
        return;
    };
    let playlist_id = match client
        .create_playlist(title, "", NEW_PLAYLIST_PRIVACY)
        .await
    {
        Ok(id) => id,
        Err(err) => {
            let _ = events.send(AppEvent::ApiError(ytmusic_api::classify_api_error(&err)));
            return;
        }
    };
    match client
        .add_playlist_items(&playlist_id, &[video_id.to_owned()])
        .await
    {
        Ok(()) => {
            let _ = events.send(AppEvent::ActionResult(format!(
                "Created playlist '{title}' and added track"
            )));
        }
        Err(err) => {
            let _ = events.send(AppEvent::ApiError(ytmusic_api::classify_api_error(&err)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- now_playing_snapshot (queue → bar metadata, pure) -----------------

    fn track(id: &str, title: &str, artist: &str, album: &str, dur: f64) -> Track {
        Track::new(id, title, artist, album, dur, "")
    }

    #[test]
    fn now_playing_snapshot_of_empty_queue_is_idle() {
        let queue = QueueManager::new();
        let snap = now_playing_snapshot(&queue);
        assert_eq!(snap.title, "");
        assert_eq!(snap.artist, "");
        assert_eq!(snap.album, "");
        assert_eq!(snap.video_id, "");
        assert_eq!(snap.duration_seconds, 0.0);
        assert!(!snap.shuffle);
        assert_eq!(snap.repeat, RepeatMode::Off);
    }

    #[test]
    fn now_playing_snapshot_carries_current_track_metadata() {
        let mut queue = QueueManager::new();
        queue.set_playlist(vec![track("v1", "Song", "Band", "LP", 200.0)], 0);
        let snap = now_playing_snapshot(&queue);
        assert_eq!(snap.title, "Song");
        assert_eq!(snap.artist, "Band");
        assert_eq!(snap.album, "LP");
        assert_eq!(snap.video_id, "v1");
        assert_eq!(snap.duration_seconds, 200.0);
    }

    #[test]
    fn now_playing_snapshot_reflects_queue_modes() {
        let mut queue = QueueManager::new();
        queue.set_playlist(vec![track("v1", "S", "A", "", 10.0)], 0);
        queue.toggle_shuffle();
        queue.cycle_repeat(); // Off -> All
        let snap = now_playing_snapshot(&queue);
        assert!(snap.shuffle);
        assert_eq!(snap.repeat, RepeatMode::All);
    }

    #[test]
    fn now_playing_snapshot_after_advance_points_at_next_track() {
        let mut queue = QueueManager::new();
        queue.set_playlist(
            vec![
                track("v1", "First", "A", "", 10.0),
                track("v2", "Second", "B", "", 20.0),
            ],
            0,
        );
        queue.next_track();
        let snap = now_playing_snapshot(&queue);
        assert_eq!(snap.video_id, "v2");
        assert_eq!(snap.title, "Second");
    }

    // -- emit_now_playing / advance emission (real channel, no Player) -----

    #[test]
    fn remove_from_queue_removes_by_id_and_emits_snapshot() {
        let (tx, rx) = std::sync::mpsc::channel::<AppEvent>();
        let mut queue = QueueManager::new();
        queue.set_playlist(
            vec![
                track("v1", "First", "A", "", 10.0),
                track("v2", "Second", "B", "", 20.0),
            ],
            0,
        );
        remove_from_queue(&mut queue, "v2", &tx);
        // The track is gone from the queue.
        assert_eq!(queue.tracks().len(), 1);
        assert_eq!(queue.tracks()[0].video_id, "v1");
        // A snapshot (reflecting the removal) and a confirmation toast were sent.
        let mut saw_snapshot = false;
        let mut saw_toast = false;
        while let Ok(ev) = rx.try_recv() {
            match ev {
                AppEvent::QueueSnapshot(s) => {
                    assert_eq!(s.tracks.len(), 1);
                    saw_snapshot = true;
                }
                AppEvent::ActionResult(msg) => {
                    assert!(msg.contains("Removed"));
                    saw_toast = true;
                }
                _ => {}
            }
        }
        assert!(saw_snapshot && saw_toast);
    }

    #[test]
    fn remove_from_queue_with_duplicate_id_removes_the_first_instance() {
        // M5d review leftover: when the same video_id appears twice in the
        // queue, removal must drop the FIRST occurrence (Python's
        // `_remove_from_queue` finds the index by id, then removes that index;
        // `position` returns the first match). After removing, exactly one copy
        // of "dup" remains and it is the one that was originally second — proven
        // by the surrounding distinct tracks keeping their relative order.
        let (tx, _rx) = std::sync::mpsc::channel::<AppEvent>();
        let mut queue = QueueManager::new();
        queue.set_playlist(
            vec![
                track("dup", "First Copy", "A", "", 10.0),
                track("mid", "Middle", "B", "", 20.0),
                track("dup", "Second Copy", "C", "", 30.0),
            ],
            0,
        );
        remove_from_queue(&mut queue, "dup", &tx);
        let remaining = queue.tracks();
        // One "dup" left, and the FIRST instance (title "First Copy") is gone.
        assert_eq!(remaining.len(), 2);
        let dup_titles: Vec<&str> = remaining
            .iter()
            .filter(|t| t.video_id == "dup")
            .map(|t| t.title.as_str())
            .collect();
        assert_eq!(
            dup_titles,
            vec!["Second Copy"],
            "the FIRST 'dup' instance must be the one removed"
        );
        // The middle distinct track is untouched and still ordered before the
        // surviving duplicate.
        assert_eq!(remaining[0].video_id, "mid");
        assert_eq!(remaining[1].video_id, "dup");
        assert_eq!(remaining[1].title, "Second Copy");
    }

    #[test]
    fn remove_from_queue_missing_id_is_a_noop_toast() {
        let (tx, rx) = std::sync::mpsc::channel::<AppEvent>();
        let mut queue = QueueManager::new();
        queue.set_playlist(vec![track("v1", "First", "A", "", 10.0)], 0);
        remove_from_queue(&mut queue, "absent", &tx);
        // Nothing removed.
        assert_eq!(queue.tracks().len(), 1);
        // A "not in the queue" toast, no snapshot.
        match rx.recv().unwrap() {
            AppEvent::ActionResult(msg) => assert!(msg.contains("not in the queue")),
            other => panic!("expected a toast, got {other:?}"),
        }
    }

    #[test]
    fn emit_now_playing_sends_a_now_playing_event() {
        let (tx, rx) = std::sync::mpsc::channel::<AppEvent>();
        let mut queue = QueueManager::new();
        queue.set_playlist(vec![track("v1", "Song", "Band", "LP", 200.0)], 0);
        emit_now_playing(&queue, &tx);
        match rx.recv().unwrap() {
            AppEvent::NowPlaying(snap) => {
                assert_eq!(snap.video_id, "v1");
                assert_eq!(snap.title, "Song");
            }
            other => panic!("expected NowPlaying, got {other:?}"),
        }
    }

    #[test]
    fn repeat_one_advance_stays_on_same_track() {
        // The repeat-one auto-advance contract: next_track returns the same
        // track, so the runtime replays it (a faithful port of Python's
        // _on_track_end). The snapshot still names that track.
        let mut queue = QueueManager::new();
        queue.set_playlist(
            vec![
                track("v1", "First", "A", "", 10.0),
                track("v2", "Second", "B", "", 20.0),
            ],
            0,
        );
        queue.cycle_repeat(); // Off -> All
        queue.cycle_repeat(); // All -> One
        assert!(queue.next_track().is_some(), "repeat-one yields a track");
        let snap = now_playing_snapshot(&queue);
        assert_eq!(snap.video_id, "v1", "repeat-one replays the current track");
        assert_eq!(snap.repeat, RepeatMode::One);
    }

    #[test]
    fn exhausted_queue_advance_decision_is_idle() {
        // The advance decision: when next_track() returns None (repeat Off at
        // the end) the snapshot is idle. This is the pure half of advance_queue
        // (the player.play half needs mpv and is covered by the player tests).
        let mut queue = QueueManager::new();
        queue.set_playlist(vec![track("v1", "Only", "A", "", 10.0)], 0);
        assert!(queue.next_track().is_none(), "single-track queue exhausts");
        let snap = now_playing_snapshot(&queue);
        // current_track stays on the last track after an exhausting next_track
        // (queue.rs keeps the index), but the bar's TrackEnded fold has already
        // cleared the position; the NowPlaying still names the last track. What
        // matters is no panic and a coherent snapshot.
        assert_eq!(snap.video_id, "v1");
    }

    // -- translate_player_event (the fan-out mapping) ----------------------

    #[test]
    fn progress_maps_to_player_progress() {
        assert_eq!(
            translate_player_event(PlayerEvent::Progress(12.5)),
            AppEvent::PlayerProgress(12.5)
        );
    }

    #[test]
    fn duration_maps_to_player_duration() {
        assert_eq!(
            translate_player_event(PlayerEvent::Duration(200.0)),
            AppEvent::PlayerDuration(200.0)
        );
    }

    #[test]
    fn volume_maps_to_player_volume() {
        assert_eq!(
            translate_player_event(PlayerEvent::Volume(72)),
            AppEvent::PlayerVolume(72)
        );
    }

    #[test]
    fn track_ended_maps_to_track_ended() {
        assert_eq!(
            translate_player_event(PlayerEvent::TrackEnded),
            AppEvent::TrackEnded
        );
    }

    #[test]
    fn track_error_preserves_detail() {
        assert_eq!(
            translate_player_event(PlayerEvent::TrackError("boom".to_owned())),
            AppEvent::TrackError("boom".to_owned())
        );
    }

    #[test]
    fn started_maps_to_player_started() {
        assert_eq!(
            translate_player_event(PlayerEvent::Started),
            AppEvent::PlayerStarted
        );
    }

    #[test]
    fn loaded_collapses_to_player_started() {
        assert_eq!(
            translate_player_event(PlayerEvent::Loaded),
            AppEvent::PlayerStarted
        );
    }

    #[test]
    fn pause_maps_to_player_paused() {
        assert_eq!(
            translate_player_event(PlayerEvent::Pause(true)),
            AppEvent::PlayerPaused(true)
        );
        assert_eq!(
            translate_player_event(PlayerEvent::Pause(false)),
            AppEvent::PlayerPaused(false)
        );
    }

    // -- forwarder thread end-to-end (real channels, no mpv) ---------------

    #[test]
    fn forwarder_republishes_events_to_ui_sink() {
        let (player_tx, player_rx) = std::sync::mpsc::channel::<PlayerEvent>();
        let (ui_tx, ui_rx) = std::sync::mpsc::channel::<AppEvent>();

        let handle = thread::spawn(move || run_player_forwarder(player_rx, &ui_tx));

        player_tx.send(PlayerEvent::Progress(1.0)).unwrap();
        player_tx.send(PlayerEvent::Duration(180.0)).unwrap();
        player_tx.send(PlayerEvent::TrackEnded).unwrap();

        assert_eq!(ui_rx.recv().unwrap(), AppEvent::PlayerProgress(1.0));
        assert_eq!(ui_rx.recv().unwrap(), AppEvent::PlayerDuration(180.0));
        assert_eq!(ui_rx.recv().unwrap(), AppEvent::TrackEnded);

        // Dropping the player sender ends the forwarder loop.
        drop(player_tx);
        handle.join().unwrap();
    }

    #[test]
    fn forwarder_stops_when_ui_sink_closes() {
        let (player_tx, player_rx) = std::sync::mpsc::channel::<PlayerEvent>();
        let (ui_tx, ui_rx) = std::sync::mpsc::channel::<AppEvent>();

        let handle = thread::spawn(move || run_player_forwarder(player_rx, &ui_tx));

        // Close the UI side first: the next forwarded event fails to send and
        // the loop breaks even though the player sender is still alive.
        drop(ui_rx);
        let _ = player_tx.send(PlayerEvent::Started);

        handle.join().unwrap();
        // The player sender outlives the forwarder; sending now is harmless.
        let _ = player_tx.send(PlayerEvent::TrackEnded);
    }
}
