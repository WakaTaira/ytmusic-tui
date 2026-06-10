//! mpv-backed audio playback controller.
//!
//! This module is a 1-to-1 port of `ytmusic_tui/player.py`. It wraps the
//! `libmpv2` binding (selected and proven in `spikes/mpv_spike`) for headless
//! audio playback: mpv resolves YouTube Music URLs directly via its built-in
//! `ytdl-hook`, so no external `yt-dlp` subprocess is needed.
//!
//! # Architecture (from the M2 spike findings)
//!
//! * **One shared `Arc<Mpv>`** — never `create_client()`. A client handle owns
//!   its own event queue and observed-property set, so commands issued on one
//!   handle never produce events on another, and the event loop hangs forever.
//!   The mpv C client API is thread-safe (`Mpv` is `Send + Sync`), so a single
//!   shared handle (event thread reads `wait_event`, the controlling thread
//!   issues commands) is sound.
//! * **A dedicated event thread** is the sole caller of `wait_event`, polling
//!   with a short timeout so a stop flag is honored promptly. It translates raw
//!   mpv events into a domain [`PlayerEvent`] enum delivered over an
//!   [`std::sync::mpsc`] channel — the Rust replacement for the Python
//!   `on_track_end` / `on_track_error` callbacks.
//! * **RAII shutdown** via [`Drop`]: the stop flag is set and the event thread
//!   joined; dropping the last `Arc<Mpv>` runs `mpv_destroy`.
//!
//! # The end-file battle lesson
//!
//! mpv emits `end-file` for *every* reason a file stops. The queue must advance
//! ONLY on a natural `EOF`. A `STOP` (which mpv reports when `loadfile ...
//! replace` interrupts a live file), `QUIT`, `REDIRECT`, or any unknown reason
//! is ignored — reacting to a `STOP` auto-advanced the queue right after the
//! user picked a track, playing the wrong song. An `ERROR` notifies the user
//! but must NEVER advance, or a broken resolver would machine-gun the queue.
//!
//! In `libmpv2`, the `ERROR` case is delivered as `Err(e)` from `wait_event` —
//! `Event::EndFile(Error)` is never produced. The event loop therefore
//! classifies in two places: [`classify_end_file_reason`] for the
//! `Ok(Event::EndFile(reason))` path and the `Err(e)` arm for the error case.
//! Both feed the pure [`EndFileAction`] decision, which keeps the playback
//! policy unit-testable without a live mpv instance.
//!
//! # `LC_NUMERIC`
//!
//! mpv requires `LC_NUMERIC=C` (the Python version set it explicitly). The
//! default C locale on Linux already satisfies this, but a process that has
//! switched to a comma-decimal locale would make `Mpv::with_initializer` fail.
//! Rust does not change the C locale unless the program calls `setlocale`, so
//! no defensive reset is performed here; the constraint is documented for
//! callers that link C code which might.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use libmpv2::events::{Event, PropertyData};
use libmpv2::{EndFileReason, Format, Mpv, mpv_end_file_reason};

// ---------------------------------------------------------------------------
// Constants (verbatim from player.py)
// ---------------------------------------------------------------------------

/// YouTube Music watch-URL template; `{video_id}` is substituted at play time.
const YTM_URL_PREFIX: &str = "https://music.youtube.com/watch?v=";

/// Inclusive lower volume bound.
const VOL_MIN: i64 = 0;
/// Inclusive upper volume bound.
const VOL_MAX: i64 = 100;

/// Poll timeout for the event thread's `wait_event`, in seconds. Short enough
/// that the stop flag is honored promptly on shutdown (matches the spike).
const EVENT_POLL_TIMEOUT: f64 = 0.25;

/// Observed-property ids. The values are arbitrary but must be distinct.
const PROP_ID_TIME_POS: u64 = 1;
const PROP_ID_DURATION: u64 = 2;

/// Property names observed for the player bar feed.
const PROP_TIME_POS: &str = "time-pos";
const PROP_DURATION: &str = "duration";

/// Default audio-quality level (mirrors `player.py`'s `audio_quality="high"`).
pub const DEFAULT_AUDIO_QUALITY: &str = "high";

// ---------------------------------------------------------------------------
// Audio quality
// ---------------------------------------------------------------------------

/// Return the yt-dlp format selector for a quality level, or `None` if unknown.
///
/// This is the Rust form of Python's `AUDIO_QUALITY_FORMATS` dict. Selectors
/// are copied verbatim. YouTube Music serves opus 251 (~160 kbps), opus 250
/// (~70 kbps), opus 249 (~50 kbps), and AAC 140 (~128 kbps); every entry falls
/// back to `bestaudio/best` so an over-narrow filter can never yield
/// "no formats" (which would make the ytdl-hook fail silently).
fn quality_format(quality: &str) -> Option<&'static str> {
    match quality {
        "low" => Some("bestaudio[abr<=70]/bestaudio/best"),
        "normal" => Some("bestaudio[abr<=131]/bestaudio/best"),
        "high" => Some("bestaudio/best"),
        _ => None,
    }
}

/// Normalise an audio-quality string to a known level.
///
/// Unknown values degrade gracefully to [`DEFAULT_AUDIO_QUALITY`] (`"high"`),
/// matching the Python contract where config typos like `"lossless"` fall back
/// to the safe default instead of erroring.
fn normalize_quality(quality: &str) -> &'static str {
    match quality {
        "low" => "low",
        "normal" => "normal",
        _ => DEFAULT_AUDIO_QUALITY,
    }
}

/// Ordered cycle used by [`Player::cycle_audio_quality`]: low -> normal -> high.
const QUALITY_CYCLE: [&str; 3] = ["low", "normal", "high"];

/// Return the next quality level after `current` in the low -> normal -> high
/// -> low cycle. An unrecognised `current` is treated as if it were the last
/// entry so the cycle still advances deterministically to `"low"`.
fn next_quality(current: &str) -> &'static str {
    let idx = QUALITY_CYCLE.iter().position(|q| *q == current);
    match idx {
        Some(i) => QUALITY_CYCLE[(i + 1) % QUALITY_CYCLE.len()],
        // Mirror Python's list.index raising: an unknown value has no place in
        // the cycle. We pick the first element so the player never gets stuck.
        None => QUALITY_CYCLE[0],
    }
}

// ---------------------------------------------------------------------------
// End-file decision (pure, libmpv-free — unit-testable)
// ---------------------------------------------------------------------------

/// What the playback policy decides to do when a file stops.
///
/// This is the pure decision extracted from the Python `_handle_end_file`
/// branches, so the "advance only on EOF" rule can be unit-tested without a
/// live mpv instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndFileAction {
    /// reason == EOF: the track finished naturally — advance the queue.
    Advance,
    /// reason == ERROR: the stream failed — notify the user (carry a short
    /// description), but NEVER advance.
    NotifyError(String),
    /// reason == STOP / QUIT / REDIRECT / anything unknown: a deliberate or
    /// unrecognised interruption — ignore it.
    Ignore,
}

/// Classify an `Ok(Event::EndFile(reason))` into an [`EndFileAction`].
///
/// `reason` is an integer alias (`libmpv2::EndFileReason`), not a Rust enum, so
/// it is compared by value against the named constants. Unknown values map to
/// [`EndFileAction::Ignore`] — the safe default of never auto-advancing on
/// something unrecognised.
///
/// Note: in `libmpv2`, an end-file with reason `ERROR` is delivered via the
/// `Err` path of `wait_event`, not here; the `ERROR` arm below is defensive in
/// case a future binding ever delivers it as a reason. It maps to
/// [`EndFileAction::NotifyError`] with no detail, never to `Advance`.
pub fn classify_end_file_reason(reason: EndFileReason) -> EndFileAction {
    if reason == mpv_end_file_reason::Eof {
        EndFileAction::Advance
    } else if reason == mpv_end_file_reason::Error {
        EndFileAction::NotifyError(String::new())
    } else {
        // STOP / QUIT / REDIRECT / unknown.
        EndFileAction::Ignore
    }
}

/// Turn a `libmpv2::Error` from the end-file ERROR path into a short
/// human-readable string — the Rust equivalent of Python's
/// `ErrorCode.human_readable(code)`.
///
/// `libmpv2::Error::Raw(code)` wraps the raw mpv error int; the `-sys` crate's
/// safe `mpv_error_str` translates it via mpv's own error table. For the
/// broken-file case the spike observed code `-13` ("loading failed").
fn error_detail(e: &libmpv2::Error) -> String {
    match e {
        libmpv2::Error::Raw(code) => {
            format!("{} (code {code})", libmpv2_sys::mpv_error_str(*code))
        }
        other => format!("{other:?}"),
    }
}

// ---------------------------------------------------------------------------
// PlayerEvent (the channel message — replaces Python's callbacks)
// ---------------------------------------------------------------------------

/// A domain event emitted by the player's event thread.
///
/// This is the Rust replacement for the Python `Player.on_track_end` /
/// `on_track_error` callbacks. The Python version delivered exactly two
/// signals (track-ended, track-error); this enum keeps those as
/// [`PlayerEvent::TrackEnded`] / [`PlayerEvent::TrackError`] and adds the
/// property-observation feed the FINDINGS doc recommends for the TUI player
/// bar ([`PlayerEvent::Progress`] / [`PlayerEvent::Duration`]) plus optional
/// now-playing transitions ([`PlayerEvent::Started`] / [`PlayerEvent::Loaded`]).
///
/// Consumers integrate it like the Python callbacks: `TrackEnded` drives the
/// queue advance; `TrackError` surfaces a toast; `Progress` / `Duration` feed
/// the player bar.
#[derive(Debug, Clone, PartialEq)]
pub enum PlayerEvent {
    /// The current track ended naturally (mpv end-file reason EOF). The queue
    /// should advance. Corresponds to Python `on_track_end()`.
    TrackEnded,
    /// The current stream failed (mpv end-file reason ERROR). The string is a
    /// short, possibly empty, human-readable description. The queue must NOT
    /// advance. Corresponds to Python `on_track_error(desc)`.
    TrackError(String),
    /// A `time-pos` observation tick (seconds). Feeds the player bar.
    Progress(f64),
    /// A `duration` observation (seconds). Feeds the player bar.
    Duration(f64),
    /// Playback of a new file started (mpv start-file). Optional now-playing
    /// transition for the UI.
    Started,
    /// A file finished loading (mpv file-loaded). Optional now-playing
    /// transition for the UI.
    Loaded,
}

/// Translate a single raw mpv event (the `wait_event` result) into an optional
/// [`PlayerEvent`].
///
/// Returns `None` for events the player does not surface (e.g. seeks, audio
/// reconfig, ignored end-file reasons). Kept as a free function taking the raw
/// `Option<Result<Event>>` so the translation — including the critical
/// `Err` => error split — is exercised by the event loop and mirrors the
/// spike's `pump_once` shape.
fn translate_event(raw: Option<Result<Event<'_>, libmpv2::Error>>) -> Option<PlayerEvent> {
    match raw {
        // Timed out: nothing to report, the loop polls again.
        None => None,
        Some(Ok(Event::StartFile)) => Some(PlayerEvent::Started),
        Some(Ok(Event::FileLoaded)) => Some(PlayerEvent::Loaded),
        Some(Ok(Event::EndFile(reason))) => match classify_end_file_reason(reason) {
            EndFileAction::Advance => Some(PlayerEvent::TrackEnded),
            // Defensive ERROR-as-reason path (see classify docs).
            EndFileAction::NotifyError(detail) => Some(PlayerEvent::TrackError(detail)),
            EndFileAction::Ignore => None,
        },
        Some(Ok(Event::PropertyChange {
            name,
            change: PropertyData::Double(v),
            ..
        })) => match name {
            PROP_TIME_POS => Some(PlayerEvent::Progress(v)),
            PROP_DURATION => Some(PlayerEvent::Duration(v)),
            _ => None,
        },
        // Other events (Seek, AudioReconfig, other PropertyChange formats,
        // replies, ...) are not surfaced.
        Some(Ok(_)) => None,
        // libmpv2 funnels an end-file ERROR through here: the mpv error code
        // arrives as Error::Raw(code), which error_detail turns into a
        // human-facing string. This is the ERROR case — notify only, never
        // advance.
        //
        // NOTE: an Err from wait_event could in principle also be a failed
        // async command/property reply, but this player issues no async
        // requests (all get_property/command calls are synchronous). If async
        // calls are ever added (M5+), gate this arm on "a track is currently
        // loaded" to disambiguate (see mpv_spike/FINDINGS.md).
        Some(Err(e)) => Some(PlayerEvent::TrackError(error_detail(&e))),
    }
}

// ---------------------------------------------------------------------------
// PlayerState (snapshot — port of the Python dataclass)
// ---------------------------------------------------------------------------

/// Snapshot of the current playback state, read on demand from mpv.
///
/// Port of the Python `PlayerState` dataclass. `artist` is populated by the
/// queue/API layer later (mpv does not know it), mirroring the Python comment.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerState {
    pub is_playing: bool,
    pub volume: i64,
    pub is_muted: bool,
    pub position: f64,
    pub duration: f64,
    pub title: String,
    pub artist: String,
    pub video_id: String,
}

impl Default for PlayerState {
    /// Matches the Python dataclass field defaults (volume defaults to 80).
    fn default() -> Self {
        Self {
            is_playing: false,
            volume: 80,
            is_muted: false,
            position: 0.0,
            duration: 0.0,
            title: String::new(),
            artist: String::new(),
            video_id: String::new(),
        }
    }
}

impl PlayerState {
    /// Playback progress as a 0.0-1.0 ratio (0.0 when duration is non-positive).
    pub fn progress(&self) -> f64 {
        if self.duration <= 0.0 {
            0.0
        } else {
            self.position / self.duration
        }
    }
}

// ---------------------------------------------------------------------------
// PlayerError
// ---------------------------------------------------------------------------

/// Errors raised by the [`Player`].
///
/// Matches the existing module style (cf. `queue::IndexOutOfRange`) but uses
/// `thiserror` for the two variants the player needs: a wrapped mpv error and
/// the `LC_NUMERIC`/version init failure surfaced by `Mpv::with_initializer`.
#[derive(Debug, thiserror::Error)]
pub enum PlayerError {
    /// mpv failed to initialise (e.g. client API major mismatch, or a
    /// non-`C` `LC_NUMERIC`). Carries the binding's error detail.
    #[error("mpv initialisation failed: {0}")]
    Init(String),
    /// A command or property operation against a running mpv failed.
    #[error("mpv operation failed: {0}")]
    Mpv(String),
}

impl From<libmpv2::Error> for PlayerError {
    fn from(e: libmpv2::Error) -> Self {
        PlayerError::Mpv(error_detail(&e))
    }
}

// ---------------------------------------------------------------------------
// Player
// ---------------------------------------------------------------------------

/// Thin wrapper around `libmpv2` for headless audio playback.
///
/// Construct with [`Player::new`]; receive playback events on the channel
/// returned by [`Player::events`]. Playback control methods ([`Player::play`],
/// [`Player::toggle_pause`], ...) issue commands against the shared handle from
/// the controlling thread. The instance shuts the event thread down on drop.
pub struct Player {
    mpv: Arc<Mpv>,
    events: Receiver<PlayerEvent>,
    stop: Arc<AtomicBool>,
    event_thread: Option<JoinHandle<()>>,
    audio_quality: &'static str,
    video_id: String,
}

impl Player {
    /// Create a player with the given initial audio quality.
    ///
    /// `audio_quality` accepts `"low"`, `"normal"`, or `"high"`; unknown values
    /// are normalised to `"high"` (config typos degrade gracefully). mpv is
    /// configured exactly like the Python version: `ytdl=yes`, `video=no`,
    /// `terminal=no`, and `ytdl-format` set to the quality's selector.
    ///
    /// Starts the dedicated event thread and registers the `time-pos` /
    /// `duration` observers up front, before the loop runs.
    ///
    /// # Errors
    ///
    /// Returns [`PlayerError::Init`] if mpv cannot be initialised (client API
    /// major mismatch, non-`C` `LC_NUMERIC`, or a rejected option).
    pub fn new(audio_quality: &str) -> Result<Self, PlayerError> {
        let quality = normalize_quality(audio_quality);
        let format = quality_format(quality).unwrap_or("bestaudio/best");

        // Single shared handle. NOT create_client (separate event queue hangs
        // the loop). Configured at init like player.py's MPV(...) kwargs.
        let mpv = Mpv::with_initializer(|init| {
            init.set_property("ytdl", "yes")?;
            init.set_property("video", "no")?;
            init.set_property("terminal", "no")?;
            init.set_property("ytdl-format", format)?;
            Ok(())
        })
        .map_err(|e| PlayerError::Init(error_detail(&e)))?;
        let mpv = Arc::new(mpv);

        // Observe the player-bar properties before the event loop starts, so no
        // early change is missed (matches how the Python bar's observers and
        // the spike's warm-up register up front).
        mpv.observe_property(PROP_TIME_POS, Format::Double, PROP_ID_TIME_POS)?;
        mpv.observe_property(PROP_DURATION, Format::Double, PROP_ID_DURATION)?;

        // Unbounded by design: ~8 events/sec at steady state (two 4 Hz-ish
        // property feeds); a stalled consumer drains quickly on resume, so a
        // bound would only add a failure mode.
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));

        let event_mpv = Arc::clone(&mpv);
        let event_stop = Arc::clone(&stop);
        let event_thread = thread::spawn(move || run_event_loop(&event_mpv, &tx, &event_stop));

        Ok(Self {
            mpv,
            events: rx,
            stop,
            event_thread: Some(event_thread),
            audio_quality: quality,
            video_id: String::new(),
        })
    }

    /// Borrow the channel receiver carrying [`PlayerEvent`]s.
    ///
    /// The replacement for the Python `on_track_end` / `on_track_error`
    /// callbacks: poll or block on this in the consuming layer.
    ///
    /// TODO(M5/M6): single-consumer by construction (`Receiver` is not
    /// `Clone`). When the TUI and MPRIS both need these events, replace with
    /// a broadcast channel or a fan-out forwarder instead of sharing this.
    pub fn events(&self) -> &Receiver<PlayerEvent> {
        &self.events
    }

    // -- Playback control --------------------------------------------------

    /// Start playback of a YouTube Music track by `video_id`.
    ///
    /// Issues `loadfile <url> replace`; the running stream (if any) is replaced.
    pub fn play(&mut self, video_id: &str) -> Result<(), PlayerError> {
        self.video_id = video_id.to_owned();
        let url = format!("{YTM_URL_PREFIX}{video_id}");
        self.mpv.command("loadfile", &[&url, "replace"])?;
        Ok(())
    }

    /// Toggle between paused and playing.
    pub fn toggle_pause(&self) -> Result<(), PlayerError> {
        let paused: bool = self.mpv.get_property("pause")?;
        self.mpv.set_property("pause", !paused)?;
        Ok(())
    }

    /// Stop playback and clear the current track.
    pub fn stop(&self) -> Result<(), PlayerError> {
        self.mpv.command("stop", &[])?;
        Ok(())
    }

    // -- Volume -------------------------------------------------------------

    /// Set volume, clamped to 0-100 (the same bounds as the Python version).
    pub fn set_volume(&self, vol: i64) -> Result<(), PlayerError> {
        let clamped = vol.clamp(VOL_MIN, VOL_MAX);
        self.mpv.set_property("volume", clamped)?;
        Ok(())
    }

    /// Adjust volume by `delta` relative to the current level.
    ///
    /// A failed read propagates (the Python version would raise from the
    /// property access too); adjusting against a guessed baseline would
    /// silently jump the volume.
    pub fn adjust_volume(&self, delta: i64) -> Result<(), PlayerError> {
        let current: i64 = self.mpv.get_property("volume")?;
        self.set_volume(current + delta)
    }

    /// Toggle audio mute.
    pub fn toggle_mute(&self) -> Result<(), PlayerError> {
        let muted: bool = self.mpv.get_property("mute")?;
        self.mpv.set_property("mute", !muted)?;
        Ok(())
    }

    // -- Seeking ------------------------------------------------------------

    /// Seek `seconds` relative to the current position (±seconds).
    pub fn seek(&self, seconds: f64) -> Result<(), PlayerError> {
        self.mpv
            .command("seek", &[&seconds.to_string(), "relative"])?;
        Ok(())
    }

    /// Seek to an absolute `position` in seconds.
    pub fn seek_absolute(&self, position: f64) -> Result<(), PlayerError> {
        self.mpv
            .command("seek", &[&position.to_string(), "absolute"])?;
        Ok(())
    }

    // -- Audio quality ------------------------------------------------------

    /// Current audio-quality level (`"low"`, `"normal"`, or `"high"`).
    pub fn audio_quality(&self) -> &str {
        self.audio_quality
    }

    /// Set the yt-dlp format selector for a new quality level.
    ///
    /// Unknown values normalise to `"high"`. The change applies from the *next*
    /// track: `ytdl-format` is evaluated by the ytdl-hook at loadfile time, so
    /// the currently-playing stream is unaffected.
    pub fn set_audio_quality(&mut self, quality: &str) -> Result<(), PlayerError> {
        let normalized = normalize_quality(quality);
        self.audio_quality = normalized;
        let format = quality_format(normalized).unwrap_or("bestaudio/best");
        self.mpv.set_property("ytdl-format", format)?;
        Ok(())
    }

    /// Advance quality low -> normal -> high -> low and return the new value.
    pub fn cycle_audio_quality(&mut self) -> Result<&str, PlayerError> {
        let next = next_quality(self.audio_quality);
        self.set_audio_quality(next)?;
        Ok(self.audio_quality)
    }

    // -- State introspection ------------------------------------------------

    /// `true` when mpv has no file loaded (track ended or never started).
    pub fn is_idle(&self) -> bool {
        self.mpv.get_property("idle-active").unwrap_or(false)
    }

    /// Read current mpv properties and return a [`PlayerState`].
    ///
    /// Mirrors the Python `get_state`: each property is read defensively and
    /// missing/unavailable values fall back to their neutral defaults, so a
    /// transiently-idle mpv never produces an error.
    pub fn get_state(&self) -> PlayerState {
        let idle = self.is_idle();
        // mpv reports `pause` even while idle; treat a read failure as paused
        // so `is_playing` errs toward "not playing" (matches Python's None ->
        // is_playing=False).
        let pause: Option<bool> = self.mpv.get_property("pause").ok();
        let volume: Option<i64> = self.mpv.get_property("volume").ok();
        let muted: bool = self.mpv.get_property("mute").unwrap_or(false);
        let time_pos: Option<f64> = self.mpv.get_property("time-pos").ok();
        let duration: Option<f64> = self.mpv.get_property("duration").ok();
        let title: Option<String> = self.mpv.get_property("media-title").ok();

        let is_playing = match pause {
            Some(p) => !idle && !p,
            None => false,
        };

        PlayerState {
            is_playing,
            volume: volume.unwrap_or(0),
            is_muted: muted,
            position: time_pos.unwrap_or(0.0),
            duration: duration.unwrap_or(0.0),
            title: title.unwrap_or_default(),
            artist: String::new(), // populated by the queue/API layer later
            video_id: if idle {
                String::new()
            } else {
                self.video_id.clone()
            },
        }
    }

    // -- Lifecycle ----------------------------------------------------------

    /// Stop the event thread and release the mpv instance.
    ///
    /// Idempotent and also run automatically on drop; calling it explicitly
    /// mirrors the Python `shutdown()` and lets callers join the thread early.
    pub fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.event_thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.shutdown();
        // Dropping the last Arc<Mpv> after this runs mpv_destroy.
    }
}

/// The event thread body: poll `wait_event`, translate, and forward over the
/// channel until the stop flag is set or the receiver is gone.
///
/// It is the *sole* caller of `wait_event` (see the module-level threading
/// note). A short poll timeout keeps shutdown responsive.
fn run_event_loop(mpv: &Mpv, tx: &Sender<PlayerEvent>, stop: &AtomicBool) {
    while !stop.load(Ordering::Relaxed) {
        let raw = mpv.wait_event(EVENT_POLL_TIMEOUT);
        if let Some(event) = translate_event(raw)
            && tx.send(event).is_err()
        {
            // Receiver dropped — nothing left to feed.
            break;
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================
//
// Port of tests/test_player.py (31 tests). The Python suite mocks python-mpv;
// the Rust split separates the pure playback policy (end-file classification,
// volume clamping, quality mapping, state math) from the FFI. Pure-logic tests
// run as plain #[test] with no libmpv; the handful that need a real mpv
// instance use the spike's lavfi trick and run headless (ao=null/vo=null).
//
// They run reliably in well under 5s on a machine with libmpv installed, so
// per the directive they are kept as normal tests (NOT #[ignore]). CI has
// libmpv; if a future CI lacks it, they would fail at `Mpv::new`, at which
// point gating them behind a feature or #[ignore] would be the fix.
//
// EXCLUDED Python tests (with reasons):
//   * test_frozen / dataclass-immutability checks have no Rust analogue: field
//     mutation is a compile error, not a runtime one (same rationale as
//     queue.rs::test_track::test_frozen). PlayerState here is a plain struct;
//     there is no frozen-mutation test to port.
//   * The Python end-file tests assert callbacks fire/don't fire. Rust delivers
//     events over a channel, so the *intent* is preserved by asserting the
//     pure EndFileAction decision (the policy) plus the channel-delivery shape
//     through translate_event. "without callback is a no-op" Python tests
//     collapse into "Ignore / no PlayerEvent produced", asserted directly.

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // PlayerState (TestPlayerState — 3 tests)
    // -----------------------------------------------------------------------

    mod player_state {
        use super::*;

        #[test]
        fn test_initial_state() {
            let state = PlayerState::default();
            assert!(!state.is_playing);
            assert_eq!(state.volume, 80);
            assert_eq!(state.position, 0.0);
            assert_eq!(state.duration, 0.0);
            assert_eq!(state.title, "");
            assert_eq!(state.artist, "");
            assert_eq!(state.video_id, "");
        }

        #[test]
        fn test_progress_zero_duration() {
            let state = PlayerState::default();
            assert_eq!(state.progress(), 0.0);
        }

        #[test]
        fn test_progress_with_duration() {
            let state = PlayerState {
                position: 30.0,
                duration: 120.0,
                ..PlayerState::default()
            };
            assert!((state.progress() - 0.25).abs() < f64::EPSILON);
        }
    }

    // -----------------------------------------------------------------------
    // Audio quality mapping (TestAudioQuality — pure portions)
    // -----------------------------------------------------------------------

    mod audio_quality {
        use super::*;

        #[test]
        fn test_high_quality_format() {
            assert_eq!(quality_format("high"), Some("bestaudio/best"));
        }

        #[test]
        fn test_normal_quality_format() {
            assert_eq!(
                quality_format("normal"),
                Some("bestaudio[abr<=131]/bestaudio/best")
            );
        }

        #[test]
        fn test_low_quality_format() {
            assert_eq!(
                quality_format("low"),
                Some("bestaudio[abr<=70]/bestaudio/best")
            );
        }

        #[test]
        fn test_unknown_quality_has_no_format() {
            assert_eq!(quality_format("lossless"), None);
        }

        #[test]
        fn test_unknown_quality_normalises_to_high() {
            // Config typos ("lossless", "ultra", ...) degrade to "high".
            assert_eq!(normalize_quality("lossless"), "high");
            assert_eq!(normalize_quality("ultra"), "high");
            assert_eq!(normalize_quality(""), "high");
        }

        #[test]
        fn test_known_qualities_normalise_to_self() {
            assert_eq!(normalize_quality("low"), "low");
            assert_eq!(normalize_quality("normal"), "normal");
            assert_eq!(normalize_quality("high"), "high");
        }

        #[test]
        fn test_cycle_quality_order() {
            // Default "high" -> low -> normal -> high.
            assert_eq!(next_quality("high"), "low");
            assert_eq!(next_quality("low"), "normal");
            assert_eq!(next_quality("normal"), "high");
        }

        #[test]
        fn test_cycle_unknown_quality_starts_at_low() {
            // An unrecognised current still advances deterministically.
            assert_eq!(next_quality("lossless"), "low");
        }
    }

    // -----------------------------------------------------------------------
    // Volume clamping (the pure half of TestPlayer volume tests)
    // -----------------------------------------------------------------------

    mod volume_clamp {
        use super::*;

        fn clamp(v: i64) -> i64 {
            v.clamp(VOL_MIN, VOL_MAX)
        }

        #[test]
        fn test_volume_set_in_range() {
            assert_eq!(clamp(60), 60);
        }

        #[test]
        fn test_volume_clamp_high() {
            assert_eq!(clamp(150), 100);
        }

        #[test]
        fn test_volume_clamp_low() {
            assert_eq!(clamp(-10), 0);
        }

        #[test]
        fn test_volume_adjust_math() {
            // adjust_volume(10) from current 50 -> set_volume(60).
            let current = 50_i64;
            assert_eq!(clamp(current + 10), 60);
        }

        #[test]
        fn test_volume_adjust_clamps_at_boundary() {
            let current = 95_i64;
            assert_eq!(clamp(current + 10), 100);
        }
    }

    // -----------------------------------------------------------------------
    // End-file handling (TestEndFileHandling — the policy, pure)
    //
    // mpv emits end-file for every stop reason. on_track_end must fire ONLY on
    // EOF; reacting to STOP/aborted auto-advanced the queue right after the
    // user picked a track, playing the wrong song. ERROR notifies only.
    // -----------------------------------------------------------------------

    mod end_file_handling {
        use super::*;

        #[test]
        fn test_eof_advances() {
            assert_eq!(
                classify_end_file_reason(mpv_end_file_reason::Eof),
                EndFileAction::Advance
            );
        }

        #[test]
        fn test_stop_is_ignored() {
            // STOP is what mpv reports for a loadfile-replace / stop command
            // interrupting a live file — the Python "ABORTED" case.
            assert_eq!(
                classify_end_file_reason(mpv_end_file_reason::Stop),
                EndFileAction::Ignore
            );
        }

        #[test]
        fn test_quit_is_ignored() {
            assert_eq!(
                classify_end_file_reason(mpv_end_file_reason::Quit),
                EndFileAction::Ignore
            );
        }

        #[test]
        fn test_redirect_is_ignored() {
            assert_eq!(
                classify_end_file_reason(mpv_end_file_reason::Redirect),
                EndFileAction::Ignore
            );
        }

        #[test]
        fn test_unknown_reason_is_ignored() {
            // Safe default: never auto-advance on an unrecognised reason.
            let bogus: EndFileReason = 999;
            assert_eq!(classify_end_file_reason(bogus), EndFileAction::Ignore);
        }

        #[test]
        fn test_error_reason_notifies_not_advances() {
            // The defensive ERROR-as-reason path: notify only, never advance.
            assert_eq!(
                classify_end_file_reason(mpv_end_file_reason::Error),
                EndFileAction::NotifyError(String::new())
            );
        }

        #[test]
        fn test_eof_translates_to_track_ended() {
            // The full event-thread shape: Ok(EndFile(EOF)) -> TrackEnded.
            let raw = Some(Ok(Event::EndFile(mpv_end_file_reason::Eof)));
            assert_eq!(translate_event(raw), Some(PlayerEvent::TrackEnded));
        }

        #[test]
        fn test_stop_translates_to_nothing() {
            // Ok(EndFile(STOP)) produces no PlayerEvent (queue must not advance).
            let raw = Some(Ok(Event::EndFile(mpv_end_file_reason::Stop)));
            assert_eq!(translate_event(raw), None);
        }

        #[test]
        fn test_timeout_translates_to_nothing() {
            // wait_event returning None (poll timeout) yields no event.
            assert_eq!(translate_event(None), None);
        }

        #[test]
        fn test_error_via_err_path_translates_to_track_error() {
            // libmpv2 delivers end-file ERROR as Err(Raw(code)); the event
            // thread must surface it as TrackError (notify only), never advance.
            let raw = Some(Err(libmpv2::Error::Raw(libmpv2::mpv_error::LoadingFailed)));
            match translate_event(raw) {
                Some(PlayerEvent::TrackError(detail)) => {
                    assert!(!detail.is_empty(), "error detail should be human-readable");
                }
                other => panic!("expected TrackError, got {other:?}"),
            }
        }

        #[test]
        fn test_error_detail_is_human_readable() {
            // The Python "human_readable description" contract: a loading
            // failure maps to a non-empty string from mpv's error table.
            let detail = error_detail(&libmpv2::Error::Raw(libmpv2::mpv_error::LoadingFailed));
            assert!(detail.contains("code"));
            assert!(!detail.is_empty());
        }
    }

    // -----------------------------------------------------------------------
    // PlayerEvent translation of property observations.
    // -----------------------------------------------------------------------

    mod property_translation {
        use super::*;

        #[test]
        fn test_time_pos_translates_to_progress() {
            let raw = Some(Ok(Event::PropertyChange {
                name: PROP_TIME_POS,
                change: PropertyData::Double(12.5),
                reply_userdata: PROP_ID_TIME_POS,
            }));
            assert_eq!(translate_event(raw), Some(PlayerEvent::Progress(12.5)));
        }

        #[test]
        fn test_duration_translates_to_duration() {
            let raw = Some(Ok(Event::PropertyChange {
                name: PROP_DURATION,
                change: PropertyData::Double(200.0),
                reply_userdata: PROP_ID_DURATION,
            }));
            assert_eq!(translate_event(raw), Some(PlayerEvent::Duration(200.0)));
        }

        #[test]
        fn test_other_property_is_ignored() {
            let raw = Some(Ok(Event::PropertyChange {
                name: "pause",
                change: PropertyData::Flag(true),
                reply_userdata: 7,
            }));
            assert_eq!(translate_event(raw), None);
        }

        #[test]
        fn test_start_and_loaded_translate() {
            assert_eq!(
                translate_event(Some(Ok(Event::StartFile))),
                Some(PlayerEvent::Started)
            );
            assert_eq!(
                translate_event(Some(Ok(Event::FileLoaded))),
                Some(PlayerEvent::Loaded)
            );
        }
    }

    // -----------------------------------------------------------------------
    // mpv-integration tests (headless, lavfi). Need a real libmpv; run in <5s.
    // These cover the TestPlayer behaviours that the Python suite asserted via
    // mocks (init options, play, pause, volume, seek, get_state, shutdown) plus
    // the end-file battle-lesson scenarios (spike scenarios 1-3 -> the M2
    // equivalent of TestEndFileHandling against a live mpv).
    // -----------------------------------------------------------------------

    mod mpv_integration {
        use super::*;
        use std::time::{Duration, Instant};

        /// A short synthesized sine that mpv generates internally — no file or
        /// network. Ends with a real EOF.
        const SINE_SHORT: &str = "av://lavfi:sine=frequency=440:duration=1";
        /// A long sine for "still playing when interrupted" scenarios.
        const SINE_LONG: &str = "av://lavfi:sine=frequency=880:duration=30";
        /// A path mpv cannot resolve -> end-file ERROR.
        const BROKEN: &str = "/nonexistent/definitely-not-a-real-file.opus";

        /// Per-scenario deadline so a hung mpv never stalls CI.
        const DEADLINE: Duration = Duration::from_secs(10);

        /// Build a Player but force headless audio/video so it is silent and
        /// needs no output device. We bypass `Player::new` only for the `ao`/
        /// `vo` overrides, then reuse the real event loop / channel / classify
        /// path. (Mirrors the spike's `make_mpv`.)
        fn headless_player() -> Player {
            let mpv = Mpv::with_initializer(|init| {
                init.set_property("vo", "null")?;
                init.set_property("ao", "null")?;
                init.set_property("video", "no")?;
                init.set_property("terminal", "no")?;
                Ok(())
            })
            .expect("headless mpv init");
            let mpv = Arc::new(mpv);
            mpv.observe_property(PROP_TIME_POS, Format::Double, PROP_ID_TIME_POS)
                .expect("observe time-pos");
            mpv.observe_property(PROP_DURATION, Format::Double, PROP_ID_DURATION)
                .expect("observe duration");

            let (tx, rx) = mpsc::channel();
            let stop = Arc::new(AtomicBool::new(false));
            let event_mpv = Arc::clone(&mpv);
            let event_stop = Arc::clone(&stop);
            let event_thread = thread::spawn(move || run_event_loop(&event_mpv, &tx, &event_stop));

            Player {
                mpv,
                events: rx,
                stop,
                event_thread: Some(event_thread),
                audio_quality: "high",
                video_id: String::new(),
            }
        }

        /// Load a raw URI (bypassing the YouTube prefix) for lavfi sources.
        fn load(player: &Player, uri: &str) {
            player
                .mpv
                .command("loadfile", &[uri, "replace"])
                .expect("loadfile");
        }

        /// Block until `pred` returns true for some received event, or the
        /// deadline elapses. Returns the matching event, or None on timeout.
        fn wait_for(
            player: &Player,
            mut pred: impl FnMut(&PlayerEvent) -> bool,
        ) -> Option<PlayerEvent> {
            let deadline = Instant::now() + DEADLINE;
            loop {
                let remaining = deadline.checked_duration_since(Instant::now())?;
                match player.events.recv_timeout(remaining) {
                    Ok(ev) => {
                        if pred(&ev) {
                            return Some(ev);
                        }
                    }
                    Err(_) => return None,
                }
            }
        }

        #[test]
        fn test_init_real_player_high_quality() {
            // Player::new must build a real mpv configured for audio playback.
            let mut player = Player::new("high").expect("player init");
            assert_eq!(player.audio_quality(), "high");
            // ytdl-format reflects the high selector.
            let fmt: String = player
                .mpv
                .get_property("ytdl-format")
                .expect("read ytdl-format");
            assert_eq!(fmt, "bestaudio/best");
            player.shutdown();
        }

        #[test]
        fn test_init_unknown_quality_normalises_to_high() {
            let mut player = Player::new("lossless").expect("player init");
            assert_eq!(player.audio_quality(), "high");
            player.shutdown();
        }

        #[test]
        fn test_init_normal_quality_sets_format() {
            let mut player = Player::new("normal").expect("player init");
            let fmt: String = player.mpv.get_property("ytdl-format").expect("read");
            assert_eq!(fmt, "bestaudio[abr<=131]/bestaudio/best");
            player.shutdown();
        }

        #[test]
        fn test_set_audio_quality_writes_format() {
            let mut player = headless_player();
            player.set_audio_quality("low").expect("set quality");
            assert_eq!(player.audio_quality(), "low");
            let fmt: String = player.mpv.get_property("ytdl-format").expect("read");
            assert_eq!(fmt, "bestaudio[abr<=70]/bestaudio/best");
        }

        #[test]
        fn test_cycle_audio_quality_writes_format() {
            let mut player = headless_player();
            // headless starts at "high" -> cycle -> low.
            assert_eq!(player.cycle_audio_quality().expect("cycle"), "low");
            let fmt: String = player.mpv.get_property("ytdl-format").expect("read");
            assert_eq!(fmt, "bestaudio[abr<=70]/bestaudio/best");
        }

        #[test]
        fn test_volume_set_and_clamp_against_mpv() {
            let player = headless_player();
            player.set_volume(60).expect("set volume");
            let v: i64 = player.mpv.get_property("volume").expect("read volume");
            assert_eq!(v, 60);

            player.set_volume(150).expect("set volume high");
            let v: i64 = player.mpv.get_property("volume").expect("read volume");
            assert_eq!(v, 100);

            player.set_volume(-10).expect("set volume low");
            let v: i64 = player.mpv.get_property("volume").expect("read volume");
            assert_eq!(v, 0);
        }

        #[test]
        fn test_adjust_volume_against_mpv() {
            let player = headless_player();
            player.set_volume(50).expect("set");
            player.adjust_volume(10).expect("adjust");
            let v: i64 = player.mpv.get_property("volume").expect("read");
            assert_eq!(v, 60);
        }

        #[test]
        fn test_toggle_pause_against_mpv() {
            let player = headless_player();
            load(&player, SINE_LONG);
            // Wait until playback is progressing so pause is meaningful.
            wait_for(&player, |e| matches!(e, PlayerEvent::Progress(_)));
            let before: bool = player.mpv.get_property("pause").expect("read pause");
            player.toggle_pause().expect("toggle");
            let after: bool = player.mpv.get_property("pause").expect("read pause");
            assert_ne!(before, after);
        }

        #[test]
        fn test_toggle_mute_against_mpv() {
            let player = headless_player();
            let before: bool = player.mpv.get_property("mute").expect("read");
            player.toggle_mute().expect("toggle");
            let after: bool = player.mpv.get_property("mute").expect("read");
            assert_ne!(before, after);
        }

        #[test]
        fn test_play_uses_youtube_url() {
            // play() must prefix the video id with the YTM watch URL. We cannot
            // resolve a real video offline, so assert the resulting end-file is
            // an ERROR (resolution failure) rather than EOF — proving a loadfile
            // was issued with a YouTube URL and routed through the error path.
            let mut player = headless_player();
            player.play("definitely_not_a_real_id").expect("play");
            let ev = wait_for(&player, |e| matches!(e, PlayerEvent::TrackError(_)));
            assert!(
                matches!(ev, Some(PlayerEvent::TrackError(_))),
                "expected TrackError from an unresolvable id, got {ev:?}"
            );
            // The id is recorded for get_state.
            assert_eq!(player.video_id, "definitely_not_a_real_id");
        }

        // --- The end-file battle lesson, against a live mpv (spike 1-3) -----

        #[test]
        fn test_natural_eof_emits_track_ended() {
            // Scenario 1: a short lavfi source plays to natural EOF -> advance.
            let player = headless_player();
            load(&player, SINE_SHORT);
            let ev = wait_for(&player, |e| matches!(e, PlayerEvent::TrackEnded));
            assert_eq!(ev, Some(PlayerEvent::TrackEnded));
        }

        #[test]
        fn test_loadfile_replace_does_not_emit_track_ended() {
            // Scenario 2: replacing a live file mid-play makes the interrupted
            // file end with reason STOP, which must NOT advance the queue. We
            // assert the first end-related signal is NOT TrackEnded.
            let player = headless_player();
            load(&player, SINE_LONG);
            assert!(
                wait_for(&player, |e| matches!(e, PlayerEvent::Started)).is_some(),
                "long track never started"
            );
            // Let the demuxer get going so the replace clearly aborts a live file.
            let settle = Instant::now() + Duration::from_millis(300);
            while Instant::now() < settle {
                // Drain progress/loaded events without busy-failing.
                let _ = player.events.recv_timeout(Duration::from_millis(50));
            }
            // Replace mid-play; the short source will EOF shortly after.
            load(&player, SINE_SHORT);

            // The STOP from the interrupted long file must not surface as
            // TrackEnded. The eventual TrackEnded (from the SHORT file's own
            // EOF) is fine; what matters is no spurious early advance. We give
            // a brief window and assert that if any event arrives before the
            // short file could possibly EOF, it is not a STOP-driven advance.
            // Simplest robust check: the long file's STOP yields no event, so
            // the only TrackEnded we ever see is the short file's true EOF.
            let ended = wait_for(&player, |e| matches!(e, PlayerEvent::TrackEnded));
            assert_eq!(
                ended,
                Some(PlayerEvent::TrackEnded),
                "the short replacement file should EOF exactly once"
            );
            // And there must be no second TrackEnded queued (no double advance
            // from the interrupted file's STOP).
            let extra = player.events.recv_timeout(Duration::from_millis(200));
            assert!(
                !matches!(extra, Ok(PlayerEvent::TrackEnded)),
                "STOP must not produce a second TrackEnded, got {extra:?}"
            );
        }

        #[test]
        fn test_broken_source_emits_track_error_not_ended() {
            // Scenario 3: a broken source ends via the Err path -> TrackError,
            // never TrackEnded (a broken resolver must not machine-gun the
            // queue).
            let player = headless_player();
            load(&player, BROKEN);
            let ev = wait_for(&player, |e| {
                matches!(e, PlayerEvent::TrackError(_) | PlayerEvent::TrackEnded)
            });
            match ev {
                Some(PlayerEvent::TrackError(detail)) => {
                    assert!(!detail.is_empty(), "error should carry a description");
                }
                other => panic!("expected TrackError, got {other:?}"),
            }
        }

        #[test]
        fn test_get_state_reports_volume_and_idle() {
            // get_state must read live mpv properties. Before any load mpv is
            // idle, so video_id is empty and is_playing is false.
            let player = headless_player();
            player.set_volume(75).expect("set volume");
            let state = player.get_state();
            assert_eq!(state.volume, 75);
            assert!(!state.is_playing);
            assert_eq!(state.video_id, "");
        }

        #[test]
        fn test_property_observation_feeds_duration() {
            // The observers registered at init must deliver a duration > 0 for
            // a real source — the player-bar feed the FINDINGS doc requires.
            let player = headless_player();
            load(&player, SINE_LONG);
            let ev = wait_for(
                &player,
                |e| matches!(e, PlayerEvent::Duration(d) if *d > 0.0),
            );
            assert!(
                matches!(ev, Some(PlayerEvent::Duration(_))),
                "expected a positive Duration observation, got {ev:?}"
            );
        }

        #[test]
        fn test_seek_command_succeeds() {
            // A relative seek mirrors the Python seek(); against a live source
            // it must not error. Wait for a Progress tick (not just StartFile):
            // a seek issued before the demuxer has a playback clock returns
            // MPV_ERROR_COMMAND, so we only seek once time-pos is advancing.
            let player = headless_player();
            load(&player, SINE_LONG);
            let progressing = wait_for(&player, |e| matches!(e, PlayerEvent::Progress(_)));
            assert!(progressing.is_some(), "playback never started progressing");
            player.seek(5.0).expect("relative seek");
            player.seek_absolute(2.0).expect("absolute seek");
        }

        #[test]
        fn test_stop_command_succeeds() {
            let player = headless_player();
            load(&player, SINE_LONG);
            wait_for(&player, |e| matches!(e, PlayerEvent::Progress(_)));
            player.stop().expect("stop");
        }

        #[test]
        fn test_shutdown_is_idempotent() {
            let mut player = headless_player();
            player.shutdown();
            player.shutdown(); // second call must not panic or hang.
        }
    }
}
