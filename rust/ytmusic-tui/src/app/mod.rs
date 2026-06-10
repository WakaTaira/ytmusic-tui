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
use ytmusic_api::{HomeSection, InnerTubeClient};

use crate::player::{Player, PlayerEvent};

/// A command sent from the UI thread to the runtime thread.
///
/// The UI never blocks on the result; the runtime replies (when there is
/// anything to reply) by emitting an [`AppEvent`].
#[derive(Debug, Clone, PartialEq)]
pub enum AppCommand {
    /// Fetch the home page recommendations. Replies with
    /// [`AppEvent::HomeLoaded`] or [`AppEvent::ApiError`].
    FetchHome,
    /// Start playback of a track by its YouTube `video_id`.
    Play(String),
    /// Toggle between paused and playing.
    TogglePause,
    /// Set the absolute volume (clamped to 0–100 by the player).
    SetVolume(i64),
    /// Adjust the volume by a relative delta.
    AdjustVolume(i64),
    /// Shut the runtime down cleanly; the command loop exits after this.
    Quit,
}

/// An event sent from the runtime / forwarder threads to the UI thread.
///
/// The UI drains these with `try_recv` once per render tick and folds them into
/// its state.
#[derive(Debug, Clone, PartialEq)]
pub enum AppEvent {
    /// Home recommendations finished loading.
    HomeLoaded(Vec<HomeSection>),
    /// An API call failed; the string is a user-facing, classified message.
    ApiError(String),
    /// A `time-pos` tick from the player (seconds). Feeds the player bar.
    PlayerProgress(f64),
    /// A `duration` observation from the player (seconds). Feeds the player bar.
    PlayerDuration(f64),
    /// The current track started loading in mpv (start-file).
    PlayerStarted,
    /// The current track ended naturally; the queue should advance.
    TrackEnded,
    /// The current stream failed; the string is a short description.
    TrackError(String),
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
/// Owns the `Player` and the optional `InnerTubeClient` for the whole session.
/// The `Player` is dropped when this function returns, which stops its event
/// thread and, in turn, ends the forwarder thread.
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

    runtime.block_on(async move {
        while let Some(command) = commands.recv().await {
            match command {
                AppCommand::Quit => break,
                AppCommand::FetchHome => handle_fetch_home(client.as_ref(), events).await,
                AppCommand::Play(video_id) => {
                    report_player_result(player.play(&video_id), events);
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
