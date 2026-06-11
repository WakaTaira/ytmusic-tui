//! The `org.mpris.MediaPlayer2{,.Player}` D-Bus interface implementation.
//!
//! Ported from `spikes/mpris_spike/src/player_impl.rs` and extended to full
//! Python parity (`src/ytmusic_tui/mpris.py`): album + artUrl metadata,
//! `LoopStatus`, `Shuffle`, and the read-only property surface.
//!
//! We deliberately use the LOW-LEVEL [`mpris_server::Server`] + manual trait
//! impls (rather than the ready-made `mpris_server::Player`) for two reasons,
//! both established in the M0 spike:
//!
//! 1. The M6 design needs explicit control over WHICH properties get emitted
//!    (only-changed-props — the waybar lesson), which the low-level `Server`'s
//!    `properties_changed([Property::...])` gives us directly.
//! 2. The ready-made `Player` helper is `!Send`, so it cannot be shared across
//!    a multi-thread runtime; the manual `RootInterface`/`PlayerInterface` impl
//!    on a plain struct IS `Send + Sync`, composing cleanly with our dedicated
//!    runtime thread (FINDINGS.md §"Threading / executor model").
//!
//! Threading model (mirrors the Python fix): the UI/runtime thread never touches
//! D-Bus state directly. It pushes [`MprisState`](super::MprisState) snapshots
//! over a channel; the single MPRIS task owns the `Server` and applies them.
//! Inbound control (Play/Pause/Next from playerctl) is relayed back out over a
//! second channel so the runtime can turn it into an [`AppCommand`](crate::app::AppCommand).
//! This is the Rust analogue of Python's `call_soon_threadsafe` hand-off.

use std::sync::Arc;

use mpris_server::{
    LoopStatus, Metadata, PlaybackRate, PlaybackStatus, PlayerInterface, RootInterface, Time,
    TrackId, Volume, zbus::fdo,
};
use tokio::sync::{Mutex, mpsc};

use super::{MprisControl, MprisState, PlaybackStatusKind};
use crate::mpris::trackid::youtube_trackid;

/// The application name surfaced as `Identity` and used to build the bus-name
/// suffix (`org.mpris.MediaPlayer2.ytmusic_tui`). Mirrors Python's `_BUS_NAME`.
pub(super) const APP_NAME: &str = "ytmusic-tui";

/// The bus-name suffix `mpris_server::Server::new` appends to
/// `org.mpris.MediaPlayer2.`. The dot in `ytmusic-tui` is illegal in a bus-name
/// element, so we use an underscore — matching Python's
/// `org.mpris.MediaPlayer2.ytmusic_tui`.
pub(super) const BUS_SUFFIX: &str = "ytmusic_tui";

/// The object that implements the MPRIS D-Bus interfaces. Holds the shared
/// last-known state (read by the property getters) plus a sender that forwards
/// inbound control requests to the runtime.
pub struct YtmusicPlayer {
    state: Arc<Mutex<MprisState>>,
    control_tx: mpsc::UnboundedSender<MprisControl>,
}

impl YtmusicPlayer {
    /// Build the interface object over the shared state and the control relay.
    pub fn new(
        state: Arc<Mutex<MprisState>>,
        control_tx: mpsc::UnboundedSender<MprisControl>,
    ) -> Self {
        Self { state, control_tx }
    }

    /// Build the current [`Metadata`] from the shared state.
    ///
    /// Mirrors Python's `_build_metadata`: trackid (encoded), title, artist
    /// (always an array, possibly empty), album, length (only when > 0), and
    /// artUrl (only when a thumbnail is present). An idle state (no `video_id`)
    /// yields the `NO_TRACK` trackid and otherwise-empty metadata.
    pub async fn current_metadata(&self) -> Metadata {
        let st = self.state.lock().await;
        build_metadata(&st)
    }

    /// Relay an inbound control request to the runtime. A dead receiver (app
    /// shutting down) is silently dropped — exactly Python's behaviour where a
    /// missing callback is a no-op.
    fn relay(&self, control: MprisControl) {
        let _ = self.control_tx.send(control);
    }
}

/// Build the MPRIS metadata dictionary from a state snapshot (pure; testable).
///
/// Faithful port of Python `_build_metadata`. An idle snapshot (empty
/// `video_id`) maps to `mpris:trackid = NO_TRACK` with no other fields, which is
/// the spec-clean "no track" value (Python emitted an empty dict; `NO_TRACK` is
/// the typed equivalent the crate forces).
pub(super) fn build_metadata(state: &MprisState) -> Metadata {
    let mut m = Metadata::new();
    m.set_trackid(Some(youtube_trackid(&state.video_id)));
    if state.video_id.is_empty() {
        return m;
    }
    m.set_title(Some(state.title.clone()));
    // xesam:artist is always an array; an empty artist becomes an empty array
    // (Python: `[track.artist] if track.artist else []`).
    if state.artist.is_empty() {
        m.set_artist(Some(Vec::<String>::new()));
    } else {
        m.set_artist(Some([state.artist.clone()]));
    }
    if !state.album.is_empty() {
        m.set_album(Some(state.album.clone()));
    }
    if state.length_secs > 0 {
        m.set_length(Some(Time::from_secs(state.length_secs)));
    }
    if !state.art_url.is_empty() {
        m.set_art_url(Some(state.art_url.clone()));
    }
    m
}

// --- org.mpris.MediaPlayer2 (the "Root" interface) --------------------------
//
// Property surface mirrors Python's `_MediaPlayer2`: Identity = "ytmusic-tui",
// CanQuit = true, CanRaise = false, HasTrackList = false, empty URI/MIME lists.

impl RootInterface for YtmusicPlayer {
    async fn raise(&self) -> fdo::Result<()> {
        Ok(())
    }
    async fn quit(&self) -> fdo::Result<()> {
        Ok(())
    }
    async fn can_quit(&self) -> fdo::Result<bool> {
        // Python `CanQuit` returns True.
        Ok(true)
    }
    async fn fullscreen(&self) -> fdo::Result<bool> {
        Ok(false)
    }
    async fn set_fullscreen(&self, _fullscreen: bool) -> mpris_server::zbus::Result<()> {
        Ok(())
    }
    async fn can_set_fullscreen(&self) -> fdo::Result<bool> {
        Ok(false)
    }
    async fn can_raise(&self) -> fdo::Result<bool> {
        Ok(false)
    }
    async fn has_track_list(&self) -> fdo::Result<bool> {
        Ok(false)
    }
    async fn identity(&self) -> fdo::Result<String> {
        Ok(APP_NAME.to_owned())
    }
    async fn desktop_entry(&self) -> fdo::Result<String> {
        Ok(APP_NAME.to_owned())
    }
    async fn supported_uri_schemes(&self) -> fdo::Result<Vec<String>> {
        Ok(vec![])
    }
    async fn supported_mime_types(&self) -> fdo::Result<Vec<String>> {
        Ok(vec![])
    }
}

// --- org.mpris.MediaPlayer2.Player ------------------------------------------
//
// Methods relay to the runtime (Python's callbacks). Properties read the shared
// state or return constants matching Python's `_MediaPlayer2Player`.

impl PlayerInterface for YtmusicPlayer {
    async fn next(&self) -> fdo::Result<()> {
        self.relay(MprisControl::Next);
        Ok(())
    }
    async fn previous(&self) -> fdo::Result<()> {
        self.relay(MprisControl::Previous);
        Ok(())
    }
    async fn pause(&self) -> fdo::Result<()> {
        // Python `Pause` calls `on_play_pause` (it has no separate pause action).
        self.relay(MprisControl::PlayPause);
        Ok(())
    }
    async fn play_pause(&self) -> fdo::Result<()> {
        self.relay(MprisControl::PlayPause);
        Ok(())
    }
    async fn stop(&self) -> fdo::Result<()> {
        self.relay(MprisControl::Stop);
        Ok(())
    }
    async fn play(&self) -> fdo::Result<()> {
        // Python `Play` also routes to `on_play_pause`.
        self.relay(MprisControl::PlayPause);
        Ok(())
    }
    async fn seek(&self, _offset: Time) -> fdo::Result<()> {
        // CanSeek is false (Python parity); ignore.
        Ok(())
    }
    async fn set_position(&self, _track_id: TrackId, _position: Time) -> fdo::Result<()> {
        Ok(())
    }
    async fn open_uri(&self, _uri: String) -> fdo::Result<()> {
        Ok(())
    }

    async fn playback_status(&self) -> fdo::Result<PlaybackStatus> {
        Ok(self.state.lock().await.status.into())
    }
    async fn loop_status(&self) -> fdo::Result<LoopStatus> {
        Ok(self.state.lock().await.loop_status)
    }
    async fn set_loop_status(&self, _loop_status: LoopStatus) -> mpris_server::zbus::Result<()> {
        // Read-only in Python (no setter callback wired); accept-and-ignore.
        Ok(())
    }
    async fn rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }
    async fn set_rate(&self, _rate: PlaybackRate) -> mpris_server::zbus::Result<()> {
        Ok(())
    }
    async fn shuffle(&self) -> fdo::Result<bool> {
        Ok(self.state.lock().await.shuffle)
    }
    async fn set_shuffle(&self, _shuffle: bool) -> mpris_server::zbus::Result<()> {
        Ok(())
    }
    async fn metadata(&self) -> fdo::Result<Metadata> {
        Ok(self.current_metadata().await)
    }
    async fn volume(&self) -> fdo::Result<Volume> {
        Ok(self.state.lock().await.volume)
    }
    async fn set_volume(&self, _volume: Volume) -> mpris_server::zbus::Result<()> {
        // Python exposes Volume read-only (no setter); accept-and-ignore.
        Ok(())
    }
    async fn position(&self) -> fdo::Result<Time> {
        // Position is a plain getter. The crate declares it
        // `emits_changed_signal = "false"`, so it is NEVER pushed through
        // PropertiesChanged — the library-level guarantee behind the "never spam
        // Position" / waybar lesson (FINDINGS.md §"Only-changed-properties").
        Ok(Time::from_micros(self.state.lock().await.position_micros))
    }
    async fn minimum_rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }
    async fn maximum_rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }
    async fn can_go_next(&self) -> fdo::Result<bool> {
        Ok(true)
    }
    async fn can_go_previous(&self) -> fdo::Result<bool> {
        Ok(true)
    }
    async fn can_play(&self) -> fdo::Result<bool> {
        Ok(true)
    }
    async fn can_pause(&self) -> fdo::Result<bool> {
        Ok(true)
    }
    async fn can_seek(&self) -> fdo::Result<bool> {
        // Python `CanSeek` returns False.
        Ok(false)
    }
    async fn can_control(&self) -> fdo::Result<bool> {
        Ok(true)
    }
}

impl From<PlaybackStatusKind> for PlaybackStatus {
    fn from(k: PlaybackStatusKind) -> Self {
        match k {
            PlaybackStatusKind::Playing => PlaybackStatus::Playing,
            PlaybackStatusKind::Paused => PlaybackStatus::Paused,
            PlaybackStatusKind::Stopped => PlaybackStatus::Stopped,
        }
    }
}
