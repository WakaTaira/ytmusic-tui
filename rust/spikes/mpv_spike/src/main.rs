//! Crate-selection spike for the Rust port's M2 (audio playback).
//!
//! This binary exercises the `libmpv2` 6.0.0 binding against the system
//! libmpv to prove four things before we commit to it for `player.rs`:
//!
//!   1. Natural end of a track yields `EndFile(Eof)`            -> queue advances.
//!   2. `loadfile <other> replace` mid-play yields `EndFile(Stop)` for the
//!      interrupted file, *discriminable from* `Eof`             -> ignored.
//!   3. A broken/nonexistent source yields an end-file ERROR    -> notify only.
//!   4. The M2 property/command surface works: time-pos/duration observation,
//!      pause toggle, relative seek, volume set.
//!
//! The battle lesson (carried from the Python version's production bug): mpv
//! emits `end-file` for *every* stop reason. Auto-advancing on anything but
//! `EOF` skips the user's pick when a file is replaced, and reacting to ERROR
//! machine-guns a broken queue. The fix is to advance ONLY on `Eof`, notify on
//! ERROR, and ignore the rest (`Stop`/`Quit`/`Redirect`).
//!
//! Headless: all scenarios force `vo=null`, `video=no`, `ao=null` and use
//! lavfi-synthesized media, so there are zero external/network dependencies.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use libmpv2::events::Event;
use libmpv2::{Format, Mpv, mpv_end_file_reason};

/// A short synthesized sine source. `av://lavfi:` lets mpv generate media
/// internally with no file or network access. Two distinct frequencies make
/// it easy to tell "first track" from "replacement track" by ear if a human
/// ever runs this with a real `ao`.
const SINE_2S: &str = "av://lavfi:sine=frequency=440:duration=2";
const SINE_OTHER: &str = "av://lavfi:sine=frequency=880:duration=30";
/// A path mpv cannot resolve -> end-file ERROR.
const BROKEN: &str = "/nonexistent/definitely-not-a-real-file.opus";

/// How long any single scenario may run before we declare it FAILED. Keeps the
/// spike from hanging CI if mpv never delivers the expected event.
const SCENARIO_TIMEOUT: Duration = Duration::from_secs(20);

/// What the event loop decided to do with an end-file, mirroring the three
/// branches of the Python `Player._handle_end_file`.
#[derive(Debug, PartialEq, Eq)]
enum EndOutcome {
    /// reason == EOF: natural finish -> the queue should advance.
    Advance,
    /// reason == ERROR: the stream failed -> notify the user, do NOT advance.
    /// Carries the human-facing error string (mpv error code -> text).
    Error(String),
    /// reason == STOP/QUIT/REDIRECT: a deliberate interruption -> ignore.
    Ignore(String),
}

/// Message pushed from the mpv event thread to the main thread.
///
/// This is the exact channel pattern recommended for M2's `player.rs`: a
/// dedicated thread owns `wait_event` and translates raw mpv events into
/// domain messages; the rest of the app never touches the mpv handle for
/// reads of the event queue.
#[derive(Debug)]
enum PlayerMsg {
    Started,
    EndFile(EndOutcome),
    /// A `time-pos` / `duration` observation tick (property name, value).
    Property(String, f64),
}

/// Build a headless mpv configured the way M2 will configure it.
///
/// `ytdl=yes, video=no` + a quality-mapped `ytdl-format` is what production
/// uses; here we keep `ytdl` off because lavfi sources need no resolver, but
/// we still prove the option-set path works (see [`scenario_warmup`]).
///
/// Returns an `Arc<Mpv>` so the single handle can be shared between the
/// command thread and the event-loop thread. We deliberately do NOT use
/// `create_client`: a client handle has *its own event queue* and would not
/// receive end-file/property events for files loaded on a different handle.
/// `Mpv` is `Send + Sync` and the mpv C API is thread-safe, so one shared
/// handle (event thread reads `wait_event`, others issue commands) is sound
/// and matches the binding's own test suite.
fn make_mpv() -> Arc<Mpv> {
    let mpv = Mpv::with_initializer(|init| {
        // Headless: no window, no audio device, no video decode.
        init.set_property("vo", "null")?;
        init.set_property("ao", "null")?;
        init.set_property("video", "no")?;
        // Quiet the library's own logging to stderr.
        init.set_property("terminal", "no")?;
        Ok(())
    })
    .expect("mpv init");
    Arc::new(mpv)
}

/// Turn a `libmpv2::Error` from the end-file ERROR path into a human-readable
/// string. When it is `Error::Raw(code)`, we translate `code` via the -sys
/// crate's `mpv_error_str` (mpv's own error table) — the Rust equivalent of
/// the Python version's `ErrorCode.human_readable(code)`.
fn error_detail(e: &libmpv2::Error) -> String {
    match e {
        libmpv2::Error::Raw(code) => {
            // e.g. code -13 -> "Failed to open or to recognize as a known format"
            format!("{} (code {code})", libmpv2_sys::mpv_error_str(*code))
        }
        other => format!("{other:?}"),
    }
}

/// Classify an end-file into the M2 decision.
///
/// `reason` is `Some(_)` when the binding delivered `Ok(Event::EndFile(reason))`.
/// `error` is `Some(_)` when the binding delivered the end-file ERROR via its
/// `Err(..)` path instead (which is how libmpv2 reports `reason == ERROR`).
fn classify(reason: Option<libmpv2::EndFileReason>, error: Option<&libmpv2::Error>) -> EndOutcome {
    if let Some(e) = error {
        return EndOutcome::Error(error_detail(e));
    }
    match reason {
        Some(r) if r == mpv_end_file_reason::Eof => EndOutcome::Advance,
        Some(r) if r == mpv_end_file_reason::Error => {
            // Defensive: if a future libmpv2 ever delivered ERROR as a reason
            // instead of via Err, still treat it as notify-only.
            EndOutcome::Error("end-file ERROR (no detail)".to_owned())
        }
        Some(r) if r == mpv_end_file_reason::Stop => EndOutcome::Ignore("STOP".to_owned()),
        Some(r) if r == mpv_end_file_reason::Quit => EndOutcome::Ignore("QUIT".to_owned()),
        Some(r) if r == mpv_end_file_reason::Redirect => EndOutcome::Ignore("REDIRECT".to_owned()),
        other => EndOutcome::Ignore(format!("unknown({other:?})")),
    }
}

/// Run one iteration of the event pump: read one event and forward a message.
/// Returns `false` when the channel receiver is gone (caller should stop).
fn pump_once(mpv: &Mpv, tx: &Sender<PlayerMsg>) -> bool {
    // 0.25s timeout: poll often so the stop flag is honored promptly.
    match mpv.wait_event(0.25) {
        None => true, // timed out; loop again
        Some(Ok(Event::StartFile)) => tx.send(PlayerMsg::Started).is_ok(),
        Some(Ok(Event::EndFile(reason))) => tx
            .send(PlayerMsg::EndFile(classify(Some(reason), None)))
            .is_ok(),
        // We only observe doubles (time-pos / duration) here; other property
        // formats are not requested, so we ignore them.
        Some(Ok(Event::PropertyChange {
            name,
            change: libmpv2::events::PropertyData::Double(v),
            ..
        })) => tx.send(PlayerMsg::Property(name.to_owned(), v)).is_ok(),
        Some(Ok(_)) => true, // AudioReconfig, FileLoaded, Seek, other PropertyChange: ignore
        Some(Err(e)) => {
            // libmpv2 funnels an end-file ERROR through here: the mpv error
            // code arrives as `Error::Raw(code)`, which `error_detail` turns
            // into a human-facing string — exactly the "error detail" M2
            // surfaces to the user. Tagged as an Error outcome (notify only,
            // never advance).
            tx.send(PlayerMsg::EndFile(classify(None, Some(&e))))
                .is_ok()
        }
    }
}

/// A self-contained player harness: one shared `Mpv` handle, a dedicated event
/// thread translating mpv events to [`PlayerMsg`] over a channel, and RAII
/// shutdown. This mirrors the structure `player.rs` will take in M2.
struct Player {
    mpv: Arc<Mpv>,
    rx: Receiver<PlayerMsg>,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Player {
    /// Build a player. `observe` runs against the handle *before* the event
    /// thread starts, so property observations are registered up front
    /// (matching how M2 will set up the player bar's observers at init).
    fn new(observe: impl FnOnce(&Mpv)) -> Self {
        let mpv = make_mpv();
        observe(&mpv);

        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));

        let ev_mpv = Arc::clone(&mpv);
        let ev_stop = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            while !ev_stop.load(Ordering::Relaxed) {
                if !pump_once(&ev_mpv, &tx) {
                    break; // receiver dropped
                }
            }
        });

        Self {
            mpv,
            rx,
            stop,
            handle: Some(handle),
        }
    }

    /// `loadfile <uri> <mode>` (mode is "replace" or "append-play").
    fn loadfile(&self, uri: &str, mode: &str) {
        self.mpv
            .command("loadfile", &[uri, mode])
            .expect("loadfile");
    }

    /// Wait for a message matching `pred`, up to the scenario deadline.
    fn wait_for<T>(&self, pred: impl FnMut(&PlayerMsg) -> Option<T>) -> Option<T> {
        wait_for(&self.rx, pred)
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Block until `pred` returns `Some(result)` for some received message, or the
/// scenario deadline elapses. Drains and ignores non-matching messages.
fn wait_for<T>(
    rx: &Receiver<PlayerMsg>,
    mut pred: impl FnMut(&PlayerMsg) -> Option<T>,
) -> Option<T> {
    let deadline = Instant::now() + SCENARIO_TIMEOUT;
    loop {
        let remaining = deadline.checked_duration_since(Instant::now())?;
        match rx.recv_timeout(remaining) {
            Ok(msg) => {
                if let Some(out) = pred(&msg) {
                    return Some(out);
                }
            }
            Err(_) => return None,
        }
    }
}

/// Print a PASS/FAIL line and return whether it passed.
fn report(name: &str, passed: bool, detail: &str) -> bool {
    let tag = if passed { "PASS" } else { "FAIL" };
    println!("[{tag}] {name} -- {detail}");
    passed
}

// --- Scenario 1: natural EOF -> Advance ------------------------------------

fn scenario_eof_advances() -> bool {
    let player = Player::new(|_| {});
    player.loadfile(SINE_2S, "replace");

    let outcome = player.wait_for(|m| match m {
        PlayerMsg::EndFile(o) => Some(format!("{o:?}")),
        _ => None,
    });

    match outcome.as_deref() {
        Some("Advance") => report("scenario1_eof_advances", true, "EOF -> Advance"),
        other => report(
            "scenario1_eof_advances",
            false,
            &format!("expected Advance, got {other:?}"),
        ),
    }
}

// --- Scenario 2: loadfile replace mid-play -> Stop, discriminable from Eof --

fn scenario_replace_is_stop() -> bool {
    let player = Player::new(|_| {});

    // Start a long track so it is unambiguously still playing when replaced.
    player.loadfile(SINE_OTHER, "replace");
    // Wait until it is actually playing before interrupting.
    if player
        .wait_for(|m| matches!(m, PlayerMsg::Started).then_some(()))
        .is_none()
    {
        return report("scenario2_replace_is_stop", false, "track A never started");
    }
    // Give the demuxer a moment so the replace clearly aborts a live file.
    thread::sleep(Duration::from_millis(300));

    // Replace mid-play. The interrupted file A must end with reason STOP.
    player.loadfile(SINE_2S, "replace");

    // The very next end-file we see should be A's STOP (not EOF).
    let first_end = player.wait_for(|m| match m {
        PlayerMsg::EndFile(o) => Some(format!("{o:?}")),
        _ => None,
    });

    match first_end.as_deref() {
        Some(s) if s.contains("Ignore(\"STOP\")") => report(
            "scenario2_replace_is_stop",
            true,
            "interrupted file -> Ignore(STOP), distinct from EOF",
        ),
        other => report(
            "scenario2_replace_is_stop",
            false,
            &format!("expected Ignore(STOP), got {other:?}"),
        ),
    }
}

// --- Scenario 3: broken source -> Error (notify only) ----------------------

fn scenario_broken_is_error() -> bool {
    let player = Player::new(|_| {});
    player.loadfile(BROKEN, "replace");

    let outcome = player.wait_for(|m| match m {
        PlayerMsg::EndFile(o @ EndOutcome::Error(_)) => Some(format!("{o:?}")),
        // If somehow an Advance/Ignore came first, capture it to fail loudly.
        PlayerMsg::EndFile(o) => Some(format!("UNEXPECTED:{o:?}")),
        _ => None,
    });

    match outcome.as_deref() {
        Some(s) if s.starts_with("Error(") => report(
            "scenario3_broken_is_error",
            true,
            &format!("broken source -> {s} (notify only, no advance)"),
        ),
        other => report(
            "scenario3_broken_is_error",
            false,
            &format!("expected Error(..), got {other:?}"),
        ),
    }
}

// --- Scenario 4: M2 warm-up (properties / pause / seek / volume) -----------

fn scenario_warmup() -> bool {
    // Observe the two properties the player bar needs, before the event loop.
    let player = Player::new(|mpv| {
        mpv.observe_property("time-pos", Format::Double, 1)
            .expect("observe time-pos");
        mpv.observe_property("duration", Format::Double, 2)
            .expect("observe duration");
    });

    player.loadfile(SINE_OTHER, "replace");

    // 4a. Confirm we receive a duration observation > 0 from the event loop.
    let got_duration = player.wait_for(|m| match m {
        PlayerMsg::Property(name, v) if name == "duration" && *v > 0.0 => Some(*v),
        _ => None,
    });
    let duration_ok = got_duration.is_some();

    // 4b. Confirm we receive a time-pos observation (playback progressing).
    let got_timepos = player.wait_for(|m| match m {
        PlayerMsg::Property(name, v) if name == "time-pos" && *v >= 0.0 => Some(*v),
        _ => None,
    });
    let timepos_ok = got_timepos.is_some();

    // 4c. Pause toggle via property write, then read it back.
    player.mpv.set_property("pause", true).expect("set pause");
    let paused: bool = player.mpv.get_property("pause").expect("get pause");
    let pause_ok = paused;
    player.mpv.set_property("pause", false).expect("unpause");

    // 4d. Volume set + read-back.
    player
        .mpv
        .set_property("volume", 42_i64)
        .expect("set volume");
    let vol: i64 = player.mpv.get_property("volume").expect("get volume");
    let volume_ok = vol == 42;

    // 4e. Relative seek (command form mirrors the Python seek()).
    let seek_ok = player.mpv.command("seek", &["5", "relative"]).is_ok();

    // 4f. Direct property read of time-pos/duration (not via observation).
    let tp: Result<f64, _> = player.mpv.get_property("time-pos");
    let dur: Result<f64, _> = player.mpv.get_property("duration");
    let direct_read_ok = tp.is_ok() && dur.is_ok();

    let all = duration_ok && timepos_ok && pause_ok && volume_ok && seek_ok && direct_read_ok;
    let detail = format!(
        "observe(duration={duration_ok}, time-pos={timepos_ok}) pause={pause_ok} \
         volume42={volume_ok} seek={seek_ok} direct_read={direct_read_ok} \
         [dur={got_duration:?} pos={got_timepos:?} vol={vol}]"
    );
    report("scenario4_m2_warmup", all, &detail)
}

fn main() {
    println!("== mpv_spike: libmpv2 6.0.0 crate-selection spike ==");
    println!(
        "linked mpv client API version: {}.{}",
        libmpv2::MPV_CLIENT_API_MAJOR,
        libmpv2::MPV_CLIENT_API_MINOR
    );
    println!();

    // Each scenario runs to completion and prints its own PASS/FAIL line, so
    // one failure never hides the others. Eager array (not lazy) on purpose.
    let results = [
        scenario_eof_advances(),
        scenario_replace_is_stop(),
        scenario_broken_is_error(),
        scenario_warmup(),
    ];

    let passed = results.iter().filter(|r| **r).count();
    let total = results.len();
    println!();
    println!("== summary: {passed}/{total} scenarios PASS ==");

    if passed != total {
        std::process::exit(1);
    }
}
