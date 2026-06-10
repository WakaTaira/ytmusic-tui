//! Minimal `org.mpris.MediaPlayer2{,.Player}` implementation for the spike.
//!
//! We deliberately use the LOW-LEVEL `Server` + manual trait impls (rather than
//! the ready-made `mpris_server::Player`) because the M6 design needs explicit
//! control over WHICH properties get emitted (only-changed-props) and we want
//! to show the trait surface the real port will implement.
//!
//! Threading model mirrored from the Python fix: the UI/player thread never
//! touches D-Bus state directly. It sends [`Command`]s over an mpsc channel;
//! the single MPRIS task owns the `Server` and applies them. Inbound control
//! (Play/Pause/Next from playerctl) is forwarded back out over a second
//! channel so the (future) ratatui loop can react. This is the Rust analogue
//! of Python's `call_soon_threadsafe` / `call_from_thread` hand-off.

use std::sync::Arc;

use mpris_server::{
    LoopStatus, Metadata, PlaybackRate, PlaybackStatus, PlayerInterface, RootInterface, Time,
    TrackId, Volume, zbus::fdo,
};
use tokio::sync::{Mutex, mpsc};

use crate::trackid::youtube_trackid;

/// Control requests that originate from D-Bus clients (playerctl, KDE, waybar
/// buttons) and must be relayed to the application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Control {
    PlayPause,
    Play,
    Pause,
    Stop,
    Next,
    Previous,
}

/// A single track's user-visible metadata.
#[derive(Debug, Clone, Default)]
pub struct TrackInfo {
    pub video_id: String,
    pub title: String,
    pub artist: String,
    pub length_secs: u64,
}

/// Mutable player state shared between the `Server` (read side, for property
/// getters) and the command applier (write side).
#[derive(Debug, Default)]
pub struct PlayerState {
    pub status: PlaybackStatusKind,
    pub track: TrackInfo,
    pub volume: f64,
}

/// Local mirror of `PlaybackStatus` so `Default` is available.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackStatusKind {
    Playing,
    Paused,
    #[default]
    Stopped,
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

/// The object that implements the MPRIS D-Bus interfaces. Holds shared state
/// plus a sender that forwards inbound control requests to the application.
pub struct YtmusicPlayer {
    state: Arc<Mutex<PlayerState>>,
    control_tx: mpsc::UnboundedSender<Control>,
}

impl YtmusicPlayer {
    pub fn new(state: Arc<Mutex<PlayerState>>, control_tx: mpsc::UnboundedSender<Control>) -> Self {
        Self { state, control_tx }
    }

    /// Build the current `Metadata` from shared state. This is where the
    /// trackid encoding actually gets exercised on a real `Metadata`.
    pub async fn current_metadata(&self) -> Metadata {
        let st = self.state.lock().await;
        let mut m = Metadata::new();
        m.set_trackid(Some(youtube_trackid(&st.track.video_id)));
        m.set_title(Some(st.track.title.clone()));
        m.set_artist(Some([st.track.artist.clone()]));
        if st.track.length_secs > 0 {
            m.set_length(Some(Time::from_secs(st.track.length_secs as i64)));
        }
        m
    }

    fn relay(&self, c: Control) {
        // If the receiver is gone (app shutting down) we simply drop it.
        let _ = self.control_tx.send(c);
    }
}

// --- org.mpris.MediaPlayer2 (the "Root" interface) --------------------------

impl RootInterface for YtmusicPlayer {
    async fn raise(&self) -> fdo::Result<()> {
        Ok(())
    }
    async fn quit(&self) -> fdo::Result<()> {
        Ok(())
    }
    async fn can_quit(&self) -> fdo::Result<bool> {
        Ok(false)
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
        Ok("ytmusic-tui (spike)".to_owned())
    }
    async fn desktop_entry(&self) -> fdo::Result<String> {
        Ok("ytmusic-tui".to_owned())
    }
    async fn supported_uri_schemes(&self) -> fdo::Result<Vec<String>> {
        Ok(vec![])
    }
    async fn supported_mime_types(&self) -> fdo::Result<Vec<String>> {
        Ok(vec![])
    }
}

// --- org.mpris.MediaPlayer2.Player ------------------------------------------

impl PlayerInterface for YtmusicPlayer {
    async fn next(&self) -> fdo::Result<()> {
        self.relay(Control::Next);
        Ok(())
    }
    async fn previous(&self) -> fdo::Result<()> {
        self.relay(Control::Previous);
        Ok(())
    }
    async fn pause(&self) -> fdo::Result<()> {
        self.relay(Control::Pause);
        Ok(())
    }
    async fn play_pause(&self) -> fdo::Result<()> {
        self.relay(Control::PlayPause);
        Ok(())
    }
    async fn stop(&self) -> fdo::Result<()> {
        self.relay(Control::Stop);
        Ok(())
    }
    async fn play(&self) -> fdo::Result<()> {
        self.relay(Control::Play);
        Ok(())
    }
    async fn seek(&self, _offset: Time) -> fdo::Result<()> {
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
        Ok(LoopStatus::None)
    }
    async fn set_loop_status(&self, _loop_status: LoopStatus) -> mpris_server::zbus::Result<()> {
        Ok(())
    }
    async fn rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }
    async fn set_rate(&self, _rate: PlaybackRate) -> mpris_server::zbus::Result<()> {
        Ok(())
    }
    async fn shuffle(&self) -> fdo::Result<bool> {
        Ok(false)
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
        Ok(())
    }
    async fn position(&self) -> fdo::Result<Time> {
        // NOTE: Position is intentionally a plain getter. The crate declares it
        // `emits_changed_signal = "false"`, so it is NEVER pushed through
        // PropertiesChanged. This is the library-level guarantee behind the
        // "never spam Position" / waybar lesson.
        Ok(Time::ZERO)
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
        Ok(false)
    }
    async fn can_control(&self) -> fdo::Result<bool> {
        Ok(true)
    }
}
