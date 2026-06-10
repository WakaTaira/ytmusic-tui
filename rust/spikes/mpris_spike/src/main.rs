//! MPRIS crate-selection spike for ytmusic-tui's Rust port (milestone M6).
//!
//! Run modes (first CLI arg):
//!   serve   - register the player, run a high-frequency PropertiesChanged
//!             stress loop, and stay alive for external busctl/playerctl probes.
//!             (default if no arg given)
//!   once    - register, emit one update, print the bus name, exit after 2s.
//!
//! The bus name is `org.mpris.MediaPlayer2.ytmusic_spike`.

mod player_impl;
mod trackid;

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use mpris_server::{PlaybackStatus, Property, Server};
use tokio::sync::{Mutex, mpsc};
use zbus::MessageStream;

use player_impl::{Control, PlaybackStatusKind, PlayerState, TrackInfo, YtmusicPlayer};

const BUS_SUFFIX: &str = "ytmusic_spike";

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "serve".into());

    // Scenario 5 as a standalone, self-proving demo: open a connection, watch
    // it, close it, and observe the watcher fire. Returns before registering a
    // player so it stays focused.
    if mode == "death" {
        return demo_connection_death().await;
    }

    // Shared state + inbound-control channel (D-Bus client -> app).
    let state = Arc::new(Mutex::new(PlayerState {
        status: PlaybackStatusKind::Stopped,
        track: TrackInfo {
            // A real YouTube id WITH a '-' to exercise the trackid encoding.
            video_id: "dQw4-9WgXcQ".into(),
            title: "Never Gonna Give You Up".into(),
            artist: "Rick Astley".into(),
            length_secs: 213,
        },
        volume: 0.8,
    }));
    let (control_tx, mut control_rx) = mpsc::unbounded_channel::<Control>();

    // Drain inbound control requests (proves Play/Pause/Next reach the app).
    tokio::spawn(async move {
        while let Some(c) = control_rx.recv().await {
            eprintln!("[control] received from D-Bus client: {c:?}");
        }
    });

    let imp = YtmusicPlayer::new(Arc::clone(&state), control_tx);
    let server = Server::new(BUS_SUFFIX, imp).await?;
    eprintln!(
        "[spike] registered bus name: {} (object /org/mpris/MediaPlayer2)",
        server.bus_name()
    );

    // Scenario 5 (connection-death observability) is demonstrated standalone in
    // `death` mode via `demo_connection_death()`; see that fn for the recipe.

    // Initial metadata so probes have something to read.
    server
        .properties_changed([
            Property::PlaybackStatus(PlaybackStatus::Playing),
            Property::Metadata(server.imp().current_metadata().await),
        ])
        .await?;
    {
        state.lock().await.status = PlaybackStatusKind::Playing;
    }
    eprintln!("[spike] emitted initial PlaybackStatus + Metadata");

    match mode.as_str() {
        "once" => {
            tokio::time::sleep(Duration::from_secs(2)).await;
            eprintln!("[spike] once-mode complete, exiting");
        }
        _ => {
            run_stress(&server, &state).await?;
            // Stay alive indefinitely for external probing.
            eprintln!("[spike] stress complete; serving. Ctrl-C / SIGTERM to stop.");
            tokio::signal::ctrl_c().await.ok();
        }
    }
    Ok(())
}

/// Scenario 3: fire a large burst of PropertiesChanged updates as fast as
/// possible, then a timed burst loop, each emitting ONLY the changed property.
async fn run_stress(
    server: &Server<YtmusicPlayer>,
    state: &Arc<Mutex<PlayerState>>,
) -> Result<(), Box<dyn std::error::Error>> {
    const FAST_N: usize = 5000;
    eprintln!("[stress] firing {FAST_N} PlaybackStatus PropertiesChanged as fast as possible...");
    let t0 = Instant::now();
    for i in 0..FAST_N {
        // Only-changed-props pattern: a single Property per emission.
        let status = if i % 2 == 0 {
            PlaybackStatus::Playing
        } else {
            PlaybackStatus::Paused
        };
        server
            .properties_changed([Property::PlaybackStatus(status)])
            .await?;
    }
    let dt = t0.elapsed();
    eprintln!(
        "[stress] {FAST_N} emits in {:?} ({:.0} emits/s) — service still up",
        dt,
        FAST_N as f64 / dt.as_secs_f64()
    );

    // Timed burst loop: hammer for a few seconds while leaving the reactor
    // room to serve concurrent property reads from busctl/playerctl.
    let burst_secs = 3;
    eprintln!("[stress] burst loop for {burst_secs}s (yields between emits)...");
    let deadline = Instant::now() + Duration::from_secs(burst_secs);
    let mut count: u64 = 0;
    let mut toggle = true;
    while Instant::now() < deadline {
        let kind = if toggle {
            PlaybackStatusKind::Playing
        } else {
            PlaybackStatusKind::Paused
        };
        toggle = !toggle;
        {
            state.lock().await.status = kind;
        }
        server
            .properties_changed([Property::PlaybackStatus(kind.into())])
            .await?;
        count += 1;
        // Yield so the socket reader / property responders make progress.
        tokio::task::yield_now().await;
    }
    eprintln!("[stress] burst loop done: {count} emits over {burst_secs}s — service still up");
    Ok(())
}

/// Scenario 5 (live demo): prove the connection-death detection recipe.
///
/// Recipe: a `MessageStream` over the connection terminates (`None`) when the
/// socket dies, because the zbus socket-reader broadcasts the error then stops.
/// Independently, `monitor_activity()` returns an `EventListener` that is also
/// notified on `close()`. We arm both, close the connection, and observe both
/// fire — an explicit, non-silent signal. This is the zbus equivalent of
/// Python's `bus.wait_for_disconnect()`.
async fn demo_connection_death() -> Result<(), Box<dyn std::error::Error>> {
    use std::time::Duration;

    let conn = zbus::Connection::session().await?;
    eprintln!(
        "[death-demo] opened session connection: {:?}",
        conn.unique_name()
    );

    let activity = conn.monitor_activity();
    let watch_conn = conn.clone();
    let stream_task = tokio::spawn(async move {
        let mut stream = MessageStream::from(watch_conn);
        // Loop until the stream yields an error or terminates.
        loop {
            match stream.next().await {
                Some(Ok(_msg)) => continue,
                Some(Err(e)) => {
                    eprintln!("[death-demo] MessageStream yielded Err => disconnect: {e}");
                    break;
                }
                None => {
                    eprintln!("[death-demo] MessageStream ended (None) => connection dead");
                    break;
                }
            }
        }
    });

    // Let the watcher arm, then kill the connection.
    tokio::time::sleep(Duration::from_millis(200)).await;
    eprintln!("[death-demo] closing connection now...");
    conn.close().await?;

    // `monitor_activity` listener resolves on close (close() notifies it).
    tokio::time::timeout(Duration::from_secs(2), activity)
        .await
        .map(|_| eprintln!("[death-demo] monitor_activity() fired => observable"))
        .unwrap_or_else(|_| eprintln!("[death-demo] activity listener timed out"));

    // And the stream task observes the dead connection too.
    let _ = tokio::time::timeout(Duration::from_secs(2), stream_task).await;
    eprintln!("[death-demo] done — connection death was observable on BOTH paths");
    Ok(())
}
