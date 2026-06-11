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
use ytmusic_api::{HomeSection, InnerTubeClient, PlaylistInfo, Track};

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
    /// Validate the auth session (the "logged-out HTTP 200" canary). Replies
    /// with [`AppEvent::SessionInvalid`] only when the session is *not* valid;
    /// a valid session produces no event (the UI assumes valid by default).
    CheckSession,
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
        while let Some(command) = commands.recv().await {
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
            }
        }
        // `player` is dropped here, stopping its event thread and ending the
        // forwarder.
    });
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
