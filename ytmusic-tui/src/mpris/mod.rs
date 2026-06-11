//! MPRIS2 D-Bus integration (milestone M6).
//!
//! Exposes playback state over `org.mpris.MediaPlayer2` so playerctl, waybar,
//! and KDE Connect can display and control ytmusic-tui. This is the Rust port of
//! `src/ytmusic_tui/mpris.py`, redesigned around the M0 spike's findings
//! (`spikes/mpris_spike/FINDINGS.md`): the low-level [`mpris_server::Server`] on
//! the runtime thread's tokio runtime, only-changed-property emission, and
//! never emitting `Position` through `PropertiesChanged`.
//!
//! # Why this exists / what the Python bug was
//!
//! The Python build hung playerctl because dbus-fast treated `EAGAIN`
//! (`BlockingIOError`) from a full kernel send buffer as fatal and silently
//! deregistered the writer, leaving the bus name registered but deaf. In zbus
//! 5.x the write path matches `WouldBlock` explicitly, yields `Poll::Pending`,
//! and re-registers the waker — backpressure is structurally non-fatal, proven
//! at 39k emits/s under 30 concurrent probes in the spike. So none of Python's
//! `_eagain_tolerant_write_callback` machinery is ported (it does not exist in
//! this model); the Python tests for it are excluded with that reason.
//!
//! # Architecture (the outbound-state plumbing choice)
//!
//! The directive offered two seams for outbound state:
//!
//! 1. extend the player-event forwarder with a second sink, or
//! 2. have the runtime push [`MprisUpdate`] messages into a tokio mpsc consumed
//!    by the MPRIS task.
//!
//! **We chose (2).** MPRIS's `update_state` needs the full track metadata
//! (title/artist/album/art/length) plus the queue modes (shuffle/repeat) and the
//! playing/paused/idle status. The forwarder only sees metadata-less
//! [`PlayerEvent`](crate::player::PlayerEvent)s (Progress/Duration/Volume/Mute/
//! TrackEnded/Started) — it would have to re-derive metadata it does not have.
//! The **runtime thread already owns** the queue and the player and already
//! builds [`NowPlaying`](crate::app::NowPlaying) on every transition, so it is
//! the natural producer of the rich MPRIS snapshot. This mirrors Python, where
//! `MprisService.update(state, track, shuffle, repeat)` is called from the
//! app/runtime side with full state — never from the raw mpv event feed. The
//! forwarder's documented "second sink" seam therefore stays unused; see
//! [`crate::app`]'s `run_player_forwarder` doc note.
//!
//! ```text
//! runtime thread (tokio runtime, owns Player+Queue+Client)
//!   • on NowPlaying / pause / volume / mute change:
//!       mpris_tx.send(MprisUpdate::State(snapshot))      ── tokio mpsc ──┐
//!                                                                         ▼
//!   • spawn_mpris task (same runtime):                                   │
//!       loop { mpris_rx.recv() → diff vs last → Server::properties_changed(only-changed) }
//!       Server owns the zbus Connection (Send+Sync)                      │
//!       inbound: PlayerInterface methods → control_tx ── tokio mpsc ──┐  │
//!                                                                      ▼  │
//!   • runtime drains control_rx → maps MprisControl → AppCommand (own command channel)
//!       connection-death watch (MessageStream + monitor_activity) → AppEvent::toast (once)
//! ```

mod player_impl;
mod trackid;

use std::sync::Arc;
use std::sync::mpsc::Sender as StdSender;

use futures_util::StreamExt;
use mpris_server::{LoopStatus, Metadata, PlaybackStatus, Property, Server};
use tokio::sync::{Mutex, mpsc};

use crate::app::{AppCommand, AppEvent};
use crate::queue::RepeatMode;

pub use player_impl::YtmusicPlayer;

/// The warning surfaced (once) when MPRIS init fails or the bus connection dies.
/// Mirrors Python's `on_mount` "MPRIS unavailable — desktop controls disabled"
/// notify, kept non-fatal: the rest of the app runs unaffected.
pub const MPRIS_UNAVAILABLE_WARNING: &str = "MPRIS unavailable — desktop controls disabled";

/// An inbound control request from a D-Bus client (playerctl, KDE, waybar
/// buttons), relayed by the [`YtmusicPlayer`] interface to the runtime.
///
/// Maps 1:1 to the methods Python honored: Next/Previous, and PlayPause (which
/// Python's `Play`/`Pause`/`PlayPause` all funnel into `on_play_pause`), plus
/// `Stop`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MprisControl {
    /// `PlayPause` / `Play` / `Pause` — Python routed all three to the single
    /// play-pause toggle.
    PlayPause,
    /// `Next`.
    Next,
    /// `Previous`.
    Previous,
    /// `Stop`.
    Stop,
}

/// Map an inbound [`MprisControl`] to the runtime [`AppCommand`] it triggers.
///
/// Pure and total so the mapping is unit-tested without a runtime. `Stop` maps
/// to [`AppCommand::TogglePause`] as the closest available action: the Rust
/// runtime has no dedicated "stop" command (the Python `on_stop` callback was
/// wired to the same play-pause action in practice — there is no stop action in
/// either client), so pausing is the faithful, side-effect-free behaviour.
#[must_use]
pub fn control_to_command(control: MprisControl) -> AppCommand {
    match control {
        MprisControl::PlayPause | MprisControl::Stop => AppCommand::TogglePause,
        MprisControl::Next => AppCommand::NextTrack,
        MprisControl::Previous => AppCommand::PreviousTrack,
    }
}

/// Local mirror of [`PlaybackStatus`] that has a `Default`, so [`MprisState`]
/// derives `Default` cleanly (the crate's `PlaybackStatus` has none).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackStatusKind {
    /// Actively playing.
    Playing,
    /// A track is loaded but paused.
    Paused,
    /// Nothing loaded / idle.
    #[default]
    Stopped,
}

/// The full MPRIS-visible state snapshot.
///
/// This is the read side for the D-Bus property getters and the value the
/// only-changed-props diff compares against. Built by the runtime from its
/// [`NowPlaying`](crate::app::NowPlaying) + player state on every transition
/// (see [`MprisState::from_now_playing`]).
///
/// `Default` is hand-written (not derived) because [`LoopStatus`] has no
/// `Default` impl; the default is the idle state (Stopped, `LoopStatus::None`),
/// matching Python's `_MediaPlayer2Player` initial values.
#[derive(Debug, Clone, PartialEq)]
pub struct MprisState {
    /// The playing/paused/idle status.
    pub status: PlaybackStatusKind,
    /// The current track's `video_id` (empty when idle). Drives the encoded
    /// `mpris:trackid`.
    pub video_id: String,
    /// `xesam:title`.
    pub title: String,
    /// `xesam:artist` (wrapped into a single-element array, or empty).
    pub artist: String,
    /// `xesam:album`.
    pub album: String,
    /// `mpris:artUrl` (empty when absent).
    pub art_url: String,
    /// `mpris:length` seconds (0 = omit the field).
    pub length_secs: i64,
    /// `Volume`, 0.0–1.0 (MPRIS scale; the UI volume is 0–100).
    pub volume: f64,
    /// `LoopStatus`, derived from the queue's repeat mode.
    pub loop_status: LoopStatus,
    /// `Shuffle`.
    pub shuffle: bool,
    /// `Position` in microseconds — a getter only, NEVER emitted through
    /// `PropertiesChanged` (the crate forbids it; the waybar lesson).
    pub position_micros: i64,
}

impl Default for MprisState {
    fn default() -> Self {
        Self {
            status: PlaybackStatusKind::default(),
            video_id: String::new(),
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            art_url: String::new(),
            length_secs: 0,
            volume: 0.0,
            loop_status: LoopStatus::None,
            shuffle: false,
            position_micros: 0,
        }
    }
}

/// Map the queue's [`RepeatMode`] to the MPRIS [`LoopStatus`]. Mirrors Python's
/// `_repeat_to_loop_status` (Off→None, All→Playlist, One→Track).
#[must_use]
pub fn repeat_to_loop_status(repeat: RepeatMode) -> LoopStatus {
    match repeat {
        RepeatMode::Off => LoopStatus::None,
        RepeatMode::All => LoopStatus::Playlist,
        RepeatMode::One => LoopStatus::Track,
    }
}

impl MprisState {
    /// Build a snapshot from the runtime's now-playing fields + the live player
    /// flags. `is_playing` and `is_muted` come from the player; the rest from
    /// the [`NowPlaying`](crate::app::NowPlaying) the runtime already built.
    ///
    /// Status mapping mirrors Python's `update_state`: playing → `Playing`; a
    /// loaded-but-not-playing track (non-empty `video_id`) → `Paused`; no track
    /// → `Stopped`.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn from_now_playing(
        is_playing: bool,
        video_id: String,
        title: String,
        artist: String,
        album: String,
        art_url: String,
        duration_seconds: f64,
        volume_0_100: i64,
        repeat: RepeatMode,
        shuffle: bool,
        position_seconds: f64,
    ) -> Self {
        let status = if is_playing {
            PlaybackStatusKind::Playing
        } else if !video_id.is_empty() {
            PlaybackStatusKind::Paused
        } else {
            PlaybackStatusKind::Stopped
        };
        Self {
            status,
            video_id,
            title,
            artist,
            album,
            art_url,
            length_secs: seconds_to_whole(duration_seconds),
            volume: volume_0_100 as f64 / 100.0,
            loop_status: repeat_to_loop_status(repeat),
            shuffle,
            position_micros: seconds_to_micros(position_seconds),
        }
    }
}

/// Convert a fractional-second duration to whole seconds for `mpris:length`,
/// clamping negatives to 0 (Python used `int(... )` on a non-negative value).
fn seconds_to_whole(secs: f64) -> i64 {
    if secs <= 0.0 { 0 } else { secs as i64 }
}

/// Convert a fractional-second position to microseconds for the `Position`
/// getter (Python: `int(state.position * 1_000_000)`).
fn seconds_to_micros(secs: f64) -> i64 {
    if secs <= 0.0 {
        0
    } else {
        (secs * 1_000_000.0) as i64
    }
}

/// Compute the set of `Property` values to emit for a state transition.
///
/// PURE and total over `(old, new)` — the testable core of the only-changed
/// emission rule (the waybar lesson). Returns ONLY the properties whose value
/// actually changed, mirroring Python's `update_state` change-detection:
/// `PlaybackStatus`, `Volume`, `LoopStatus`, `Shuffle`, and `Metadata`.
/// `Position` is structurally excluded — it has no [`Property`] variant in the
/// crate, so it can never be emitted here.
///
/// An empty result means "nothing changed, emit nothing" — the steady-state for
/// a Position-only tick, which keeps the 1 Hz UI feed from flooding the bus.
#[must_use]
pub fn changed_properties(old: &MprisState, new: &MprisState) -> Vec<Property> {
    let mut changed = Vec::new();
    if old.status != new.status {
        changed.push(Property::PlaybackStatus(PlaybackStatus::from(new.status)));
    }
    // Compare on the f64 volume; the runtime quantizes from an integer 0–100 so
    // there is no float-jitter risk (each step is exactly k/100).
    if old.volume != new.volume {
        changed.push(Property::Volume(new.volume));
    }
    if old.loop_status != new.loop_status {
        changed.push(Property::LoopStatus(new.loop_status));
    }
    if old.shuffle != new.shuffle {
        changed.push(Property::Shuffle(new.shuffle));
    }
    // Metadata is compared by value (Metadata derives PartialEq). Any change to
    // trackid/title/artist/album/length/artUrl re-emits the whole dict, exactly
    // as Python's `_metadata_equal` gate did.
    let old_meta = build_metadata_for(old);
    let new_meta = build_metadata_for(new);
    if old_meta != new_meta {
        changed.push(Property::Metadata(new_meta));
    }
    changed
}

/// Build the MPRIS [`Metadata`] dict for a snapshot (delegates to the interface
/// impl's pure builder so the diff and the live getter agree exactly).
fn build_metadata_for(state: &MprisState) -> Metadata {
    player_impl::build_metadata(state)
}

/// A message pushed from the runtime to the MPRIS task.
#[derive(Debug, Clone)]
pub enum MprisUpdate {
    /// A new full state snapshot; the task diffs it against the last and emits
    /// only the changed properties.
    State(MprisState),
    /// Tear the MPRIS task down (sent on app shutdown).
    Shutdown,
}

/// Handle the runtime keeps for the MPRIS task: the update sink plus the join
/// handle of the spawned task.
pub struct MprisHandle {
    update_tx: mpsc::UnboundedSender<MprisUpdate>,
}

impl MprisHandle {
    /// Push a state snapshot to the MPRIS task. A dead task (init failed, or
    /// already shut down) makes this a silent no-op — MPRIS is best-effort.
    pub fn send(&self, state: MprisState) {
        let _ = self.update_tx.send(MprisUpdate::State(state));
    }

    /// Ask the MPRIS task to shut down. Best-effort.
    pub fn shutdown(&self) {
        let _ = self.update_tx.send(MprisUpdate::Shutdown);
    }
}

/// Spawn the MPRIS server task on the current tokio runtime.
///
/// Must be called from within the runtime thread's tokio context (the runtime
/// already runs `block_on`, so a `tokio::spawn` here joins that runtime — the
/// spike-verified single-executor rule).
///
/// * `control_tx` is the runtime's own inbound-control sink: the MPRIS interface
///   relays D-Bus method calls (PlayPause/Next/Previous/Stop) onto it, and the
///   runtime turns them into [`AppCommand`]s via [`control_to_command`].
/// * `events` is the UI-bound [`AppEvent`] sink, used ONLY to surface the
///   one-shot connection-failure / death warning toast.
///
/// On any failure to register the server (no session bus, name taken, build
/// error) this surfaces ONE warning toast and returns `None` — the app keeps
/// running with no desktop integration (Python parity: a `try/except` in
/// `on_mount` that leaves `_mpris = None`). Returns `Some(MprisHandle)` on
/// success.
pub async fn spawn_mpris(
    control_tx: mpsc::UnboundedSender<MprisControl>,
    events: StdSender<AppEvent>,
) -> Option<MprisHandle> {
    let state = Arc::new(Mutex::new(MprisState::default()));
    let imp = YtmusicPlayer::new(Arc::clone(&state), control_tx);

    let server = match Server::new(player_impl::BUS_SUFFIX, imp).await {
        Ok(server) => server,
        Err(err) => {
            // Init failure is non-fatal — one warning, continue without MPRIS.
            warn_once(&events, &format!("{MPRIS_UNAVAILABLE_WARNING} ({err})"));
            return None;
        }
    };

    let (update_tx, update_rx) = mpsc::unbounded_channel::<MprisUpdate>();

    // Arm the connection-death watch before serving (FINDINGS.md §"Connection-
    // death detection"): a MessageStream over the server's connection yields
    // Some(Err)/None when the socket dies. We surface ONE warning toast and let
    // the app keep running (the positive lesson from the Python incident).
    spawn_death_watch(&server, events.clone());

    // The emission task owns the Server and the shared state, drains updates,
    // diffs, and emits only-changed properties.
    tokio::spawn(run_emit_loop(server, state, update_rx));

    Some(MprisHandle { update_tx })
}

/// Surface a single warning toast on the UI event channel. A dead UI receiver
/// (app shutting down) is ignored.
///
/// The once-semantics live at the call sites (init failure returns early;
/// the death watch breaks after its first send) — this function itself is a
/// plain send, not a gate.
fn warn_once(events: &StdSender<AppEvent>, message: &str) {
    let _ = events.send(AppEvent::ActionResult(message.to_owned()));
}

/// Spawn the connection-death watcher. On the first death signal it surfaces a
/// single warning toast; it never tears the app down.
fn spawn_death_watch(server: &Server<YtmusicPlayer>, events: StdSender<AppEvent>) {
    let conn = server.connection().clone();
    tokio::spawn(async move {
        let mut stream = zbus::MessageStream::from(conn);
        // The stream yields Some(Ok(_)) for normal traffic, then Some(Err)/None
        // exactly once when the connection dies. We only care about the death.
        loop {
            match stream.next().await {
                Some(Ok(_)) => continue,
                Some(Err(_)) | None => {
                    warn_once(&events, MPRIS_UNAVAILABLE_WARNING);
                    break;
                }
            }
        }
    });
}

/// The emission loop: drain [`MprisUpdate`]s, diff each new state against the
/// last, and emit only the changed properties. Holds the [`Server`] (and the
/// shared state the property getters read) for its lifetime.
async fn run_emit_loop(
    server: Server<YtmusicPlayer>,
    state: Arc<Mutex<MprisState>>,
    mut updates: mpsc::UnboundedReceiver<MprisUpdate>,
) {
    while let Some(update) = updates.recv().await {
        let new_state = match update {
            MprisUpdate::State(s) => s,
            MprisUpdate::Shutdown => break,
        };
        let changed = {
            // Hold the lock only to read the old value and swap in the new one,
            // so the property getters always see a consistent snapshot.
            let mut guard = state.lock().await;
            let changed = changed_properties(&guard, &new_state);
            *guard = new_state;
            changed
        };
        if !changed.is_empty() {
            // A transient emit error (e.g. mid-teardown) is non-fatal; the next
            // tick retries. zbus backpressure never errors here (it parks).
            let _ = server.properties_changed(changed).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playing_state() -> MprisState {
        MprisState::from_now_playing(
            true,
            "vid1".to_owned(),
            "Song 1".to_owned(),
            "Artist 1".to_owned(),
            "Album 1".to_owned(),
            "http://art".to_owned(),
            181.0,
            80,
            RepeatMode::All,
            true,
            60.0,
        )
    }

    // -- status mapping (playing / paused / idle) --------------------------

    #[test]
    fn status_is_playing_when_playing() {
        let s = MprisState::from_now_playing(
            true,
            "v".to_owned(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            0.0,
            50,
            RepeatMode::Off,
            false,
            0.0,
        );
        assert_eq!(s.status, PlaybackStatusKind::Playing);
    }

    #[test]
    fn status_is_paused_with_track_but_not_playing() {
        // Mirrors Python: a loaded-but-paused track (non-empty video_id) → Paused.
        let s = MprisState::from_now_playing(
            false,
            "v".to_owned(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            0.0,
            50,
            RepeatMode::Off,
            false,
            0.0,
        );
        assert_eq!(s.status, PlaybackStatusKind::Paused);
    }

    #[test]
    fn status_is_stopped_when_idle() {
        let s = MprisState::from_now_playing(
            false,
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            0.0,
            50,
            RepeatMode::Off,
            false,
            0.0,
        );
        assert_eq!(s.status, PlaybackStatusKind::Stopped);
    }

    // -- repeat → loop status ----------------------------------------------

    #[test]
    fn repeat_off_maps_to_loop_none() {
        assert_eq!(repeat_to_loop_status(RepeatMode::Off), LoopStatus::None);
    }

    #[test]
    fn repeat_all_maps_to_loop_playlist() {
        assert_eq!(repeat_to_loop_status(RepeatMode::All), LoopStatus::Playlist);
    }

    #[test]
    fn repeat_one_maps_to_loop_track() {
        assert_eq!(repeat_to_loop_status(RepeatMode::One), LoopStatus::Track);
    }

    // -- field mapping ------------------------------------------------------

    #[test]
    fn volume_is_scaled_to_unit_range() {
        assert_eq!(playing_state().volume, 0.8);
    }

    #[test]
    fn duration_maps_to_whole_length_seconds() {
        assert_eq!(playing_state().length_secs, 181);
    }

    #[test]
    fn position_maps_to_microseconds() {
        assert_eq!(playing_state().position_micros, 60_000_000);
    }

    #[test]
    fn zero_duration_omits_length() {
        let s = MprisState::from_now_playing(
            true,
            "v".to_owned(),
            "t".to_owned(),
            "a".to_owned(),
            String::new(),
            String::new(),
            0.0,
            80,
            RepeatMode::Off,
            false,
            0.0,
        );
        assert_eq!(s.length_secs, 0);
        let meta = build_metadata_for(&s);
        // No mpris:length field present when length is 0 (Python parity).
        assert!(meta.get_value("mpris:length").is_none());
    }

    // -- control → command mapping -----------------------------------------

    #[test]
    fn playpause_control_maps_to_toggle_pause() {
        assert_eq!(
            control_to_command(MprisControl::PlayPause),
            AppCommand::TogglePause
        );
    }

    #[test]
    fn next_control_maps_to_next_track() {
        assert_eq!(
            control_to_command(MprisControl::Next),
            AppCommand::NextTrack
        );
    }

    #[test]
    fn previous_control_maps_to_previous_track() {
        assert_eq!(
            control_to_command(MprisControl::Previous),
            AppCommand::PreviousTrack
        );
    }

    #[test]
    fn stop_control_maps_to_toggle_pause() {
        // No dedicated stop command in the runtime; pause is the faithful action
        // (Python wired on_stop to the same play-pause toggle in practice).
        assert_eq!(
            control_to_command(MprisControl::Stop),
            AppCommand::TogglePause
        );
    }

    // -- only-changed-props diff (the waybar lesson, as a pure function) ----

    #[test]
    fn first_transition_from_default_emits_status_and_metadata() {
        // Default (Stopped, idle) → playing track: status + volume + loop +
        // shuffle + metadata all differ from the Stopped default.
        let changed = changed_properties(&MprisState::default(), &playing_state());
        assert!(
            changed
                .iter()
                .any(|p| matches!(p, Property::PlaybackStatus(_)))
        );
        assert!(changed.iter().any(|p| matches!(p, Property::Metadata(_))));
        // Position is never an emittable property (no Property::Position variant).
    }

    #[test]
    fn identical_state_emits_nothing() {
        let s = playing_state();
        assert!(changed_properties(&s, &s).is_empty());
    }

    #[test]
    fn only_status_change_emits_only_playback_status() {
        let old = playing_state();
        let mut new = old.clone();
        new.status = PlaybackStatusKind::Paused;
        let changed = changed_properties(&old, &new);
        assert_eq!(changed.len(), 1);
        assert!(matches!(changed[0], Property::PlaybackStatus(_)));
    }

    #[test]
    fn position_only_change_emits_nothing() {
        // The core waybar rule: a Position-only delta produces NO emission.
        let old = playing_state();
        let mut new = old.clone();
        new.position_micros = 120_000_000;
        assert!(changed_properties(&old, &new).is_empty());
    }

    #[test]
    fn volume_change_emits_only_volume() {
        let old = playing_state();
        let mut new = old.clone();
        new.volume = 0.5;
        let changed = changed_properties(&old, &new);
        assert_eq!(changed.len(), 1);
        assert!(matches!(changed[0], Property::Volume(v) if v == 0.5));
    }

    #[test]
    fn track_change_emits_metadata() {
        let old = playing_state();
        let mut new = old.clone();
        new.video_id = "vid2".to_owned();
        new.title = "Song 2".to_owned();
        let changed = changed_properties(&old, &new);
        assert!(changed.iter().any(|p| matches!(p, Property::Metadata(_))));
    }

    #[test]
    fn loop_status_change_emits_only_loop_status() {
        let old = playing_state();
        let mut new = old.clone();
        new.loop_status = LoopStatus::Track;
        let changed = changed_properties(&old, &new);
        assert_eq!(changed.len(), 1);
        assert!(matches!(
            changed[0],
            Property::LoopStatus(LoopStatus::Track)
        ));
    }

    #[test]
    fn shuffle_change_emits_only_shuffle() {
        let old = playing_state();
        let mut new = old.clone();
        new.shuffle = false;
        let changed = changed_properties(&old, &new);
        assert_eq!(changed.len(), 1);
        assert!(matches!(changed[0], Property::Shuffle(false)));
    }

    // -- metadata field mapping (build_metadata parity with Python) ---------

    #[test]
    fn metadata_carries_title_artist_album_length_arturl() {
        let meta = build_metadata_for(&playing_state());
        assert!(meta.get_value("xesam:title").is_some());
        assert!(meta.get_value("xesam:artist").is_some());
        assert!(meta.get_value("xesam:album").is_some());
        assert!(meta.get_value("mpris:length").is_some());
        assert!(meta.get_value("mpris:artUrl").is_some());
        assert!(meta.get_value("mpris:trackid").is_some());
    }

    #[test]
    fn idle_metadata_has_only_no_track_trackid() {
        let meta = build_metadata_for(&MprisState::default());
        assert!(meta.get_value("mpris:trackid").is_some());
        assert!(meta.get_value("xesam:title").is_none());
        assert!(meta.get_value("mpris:length").is_none());
    }

    #[test]
    fn empty_artist_omits_no_field_but_is_empty_array() {
        // Python emits xesam:artist = [] for an empty artist (field present,
        // empty list) — distinct from omitting the key.
        let mut s = playing_state();
        s.artist = String::new();
        let meta = build_metadata_for(&s);
        assert!(meta.get_value("xesam:artist").is_some());
    }

    #[test]
    fn missing_art_url_omits_the_field() {
        let mut s = playing_state();
        s.art_url = String::new();
        let meta = build_metadata_for(&s);
        assert!(meta.get_value("mpris:artUrl").is_none());
    }

    // -- live session-bus probe (ignored by default) -----------------------
    //
    // Requires a real D-Bus session bus and `busctl` on PATH. Run locally with:
    //   cargo test -p ytmusic-tui --lib mpris::tests::live_ -- --ignored --nocapture
    // It is `#[ignore]` so CI (no session bus) stays green; this is the Rust
    // analogue of the spike's live serve/probe run.

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires a live D-Bus session bus + busctl (run locally)"]
    async fn live_registers_bus_name_and_serves_metadata() {
        use std::process::Command;

        let (control_tx, _control_rx) = mpsc::unbounded_channel::<MprisControl>();
        let (event_tx, _event_rx) = std::sync::mpsc::channel::<AppEvent>();

        let handle = spawn_mpris(control_tx, event_tx)
            .await
            .expect("spawn_mpris should register on a live session bus");

        // Push a playing track so a probe has real metadata to read.
        handle.send(playing_state());

        // Give the emission task a moment to apply the update.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Probe the served PlaybackStatus via busctl.
        let out = Command::new("busctl")
            .args([
                "--user",
                "get-property",
                "org.mpris.MediaPlayer2.ytmusic_tui",
                "/org/mpris/MediaPlayer2",
                "org.mpris.MediaPlayer2.Player",
                "PlaybackStatus",
            ])
            .output()
            .expect("busctl must be on PATH for the live probe");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("Playing"),
            "expected PlaybackStatus=Playing from busctl, got: {stdout:?} / stderr {:?}",
            String::from_utf8_lossy(&out.stderr)
        );

        handle.shutdown();
    }
}
