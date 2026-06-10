# mpv binding spike — findings (M2 audio playback)

## Decision

**Use `libmpv2` v6.0.0** (with `libmpv2-sys` v4.0.1 as a direct dependency for
human-readable error strings) as the Rust libmpv binding for the M2 player.

It is the maintained successor to `libmpv-rs`, builds cleanly against the system
libmpv 2.5.0, and — the non-negotiable criterion — **surfaces the mpv `end-file`
reason as a discriminable enum**, so the production "advance only on EOF" rule
ports directly. All four spike scenarios PASS, stably, across repeated runs.

- Crate: `libmpv2 = { version = "6.0.0", default-features = false }`
- Sys crate (direct dep): `libmpv2-sys = "4.0.1"` (for `mpv_error_str`)
- Repo: https://github.com/kohsine/libmpv-rs (a fork of ParadoxSpiral/libmpv-rs)
- License: LGPL-2.1 (the binding). Our crate links libmpv dynamically via the
  system lib, same posture as the Python version. Dynamic linking against an
  LGPL wrapper is compatible with an MIT application; flag for final license
  review but it is not a blocker.

### Spike program output (verbatim)

```
== mpv_spike: libmpv2 6.0.0 crate-selection spike ==
linked mpv client API version: 2.2

[PASS] scenario1_eof_advances -- EOF -> Advance
[PASS] scenario2_replace_is_stop -- interrupted file -> Ignore(STOP), distinct from EOF
[PASS] scenario3_broken_is_error -- broken source -> Error("loading failed (code -13)") (notify only, no advance)
[PASS] scenario4_m2_warmup -- observe(duration=true, time-pos=true) pause=true volume42=true seek=true direct_read=true [dur=Some(1.09...) pos=Some(0.064...) vol=42]

== summary: 4/4 scenarios PASS ==
```

Environment: rustc/cargo 1.96.0, system libmpv 2.5.0 (`/usr/lib/libmpv.so.2`,
mpv 0.41.0), `pkg-config mpv` = 2.5.0. Build pulls only 2 crates
(`libmpv2`, `libmpv2-sys`). `cargo fmt --check` clean; `cargo clippy -- -D
warnings` clean.

## Candidate comparison

| Crate | Latest | Updated | Builds vs libmpv 2.5? | end-file reason discriminable? | Verdict |
|---|---|---|---|---|---|
| **`libmpv2`** (kohsine fork) | **6.0.0** | **2026-05-12** | **Yes** (verified, this spike) | **Yes** — `Event::EndFile(EndFileReason)` + reason consts | **CHOSEN** |
| `libmpv` (ParadoxSpiral, original) | 2.0.1 | 2020-09-29 | Unlikely vs 2.x headers; abandoned 5+ yrs | Has the enum, but stalled API | Fallback only; not needed |
| `libmpv-sirno` (fork) | 2.0.2-fork.1 | 2022-12-28 | Stale | n/a | Superseded by `libmpv2` |
| `mpv-client` (cplugin-oriented) | 1.1.0 | 2025-06-28 | For mpv *plugins*, not embedding | n/a | Wrong use case |
| mpv JSON IPC (own impl over `--input-ipc-server`) | — | — | n/a (spawns mpv subprocess) | Yes via JSON `end-file` event | Last resort; not needed |

`libmpv2` wins decisively: actively maintained (6.0.0 released 2026-05-12, the
month of this spike), most downloads of any live libmpv binding (~43k), edition
2024, and its **own test suite already asserts the exact battle-lesson behavior**
(`loadfile ... replace` mid-play -> `EndFile(Stop)`, distinct from natural `Eof`;
see `src/tests.rs:120-144` in the crate). We did not need the `libmpv-rs` or
JSON-IPC fallbacks.

## mpv C reason <-> `libmpv2` enum mapping

`libmpv2::EndFileReason` is a type alias for the raw `libmpv2_sys::mpv_end_file_reason`
(an integer enum); named constants live in module `libmpv2::mpv_end_file_reason`.

| mpv C reason (`client.h`) | int | `libmpv2` constant | Our M2 action |
|---|---|---|---|
| `MPV_END_FILE_REASON_EOF` | 0 | `mpv_end_file_reason::Eof` | **Advance** the queue |
| `MPV_END_FILE_REASON_STOP` | 2 | `mpv_end_file_reason::Stop` | **Ignore** (loadfile-replace / stop / playlist-next abort) |
| `MPV_END_FILE_REASON_QUIT` | 3 | `mpv_end_file_reason::Quit` | **Ignore** |
| `MPV_END_FILE_REASON_ERROR` | 4 | `mpv_end_file_reason::Error` | **Notify only** (see below) |
| `MPV_END_FILE_REASON_REDIRECT` | 5 | `mpv_end_file_reason::Redirect` | **Ignore** |

Compare by value (`reason == mpv_end_file_reason::Eof`) because the type is a
bindgen integer alias, not a Rust `enum` with named variants — you cannot `match`
it with bare variant patterns nor write an exhaustive `match`. Treat unknown
values as **Ignore** (safe default: never auto-advance on something unrecognised).

### CRITICAL gotcha — ERROR is delivered via `Err`, not as an `EndFile` variant

The single most important API-shape finding. In `libmpv2`'s `wait_event`, when an
end-file's `error` field is non-zero (reason == ERROR), the binding returns
**`Some(Err(libmpv2::Error))`** and `Event::EndFile(Error)` is **never produced**:

```rust
// libmpv2 6.0.0, src/mpv/events.rs (EndFile arm)
mpv_event_id::EndFile => {
    let end_file = *(event.data as *mut libmpv2_sys::mpv_event_end_file);
    if let Err(e) = mpv_err((), end_file.error) {
        Some(Err(e))                                   // reason == ERROR lands HERE
    } else {
        Some(Ok(Event::EndFile(end_file.reason as _))) // EOF / STOP / QUIT / REDIRECT
    }
}
```

So the M2 event loop must classify in **two** places:
- `Ok(Event::EndFile(reason))` -> `reason` is EOF/STOP/QUIT/REDIRECT.
- `Err(e)` immediately following our `loadfile` -> that is the ERROR case; `e`
  carries the error detail. Map to "notify only, do not advance".

This is cleaner than Python (where every reason arrived as one event and we read
`event.data.error` ourselves) but non-obvious: a naive port matching only
`Ok(Event::EndFile(_))` would silently drop playback failures. The spike's
`pump_once` shows the correct shape. (An `Err` can in principle also come from a
failed async property/command reply, but the player's read path issues none of
those; M2 can additionally gate on "a track is active" for full disambiguation.)

## How error detail surfaces

`libmpv2::Error::Raw(code)` wraps the raw mpv error int (`libmpv2::MpvError` =
`libmpv2_sys::mpv_error`). For the broken-file scenario the spike observed
`code == -13` = `MPV_ERROR_LOADING_FAILED` — the same failure class Python handled.

For a **human-readable** string (Python used `ErrorCode.human_readable`):
`libmpv2` re-exports the `mpv_error` *type* but **not** a stringify function. The
`-sys` crate does, as a safe helper:

```rust
// libmpv2-sys 4.0.1
pub fn mpv_error_str(e: mpv_error) -> &'static str { /* wraps C mpv_error_string */ }
```

Hence M2 should take a **direct `libmpv2-sys` dependency** pinned to the same
version `libmpv2` uses (4.0.1) so there is a single sys instance (verified via
`cargo tree`), and translate:

```rust
fn error_detail(e: &libmpv2::Error) -> String {
    match e {
        libmpv2::Error::Raw(code) => format!("{} (code {code})", libmpv2_sys::mpv_error_str(*code)),
        other => format!("{other:?}"),
    }
}
// broken file -> "loading failed (code -13)"
```

`libmpv2::Error` is `Clone + Debug + PartialEq + Eq + Hash` and implements
`std::error::Error` + `Display` (Display forwards to `Debug`, hence the helper).

## Other gotchas / constraints

1. **Threading model — use ONE handle shared via `Arc`, NOT `create_client`.**
   The spike's main trap. `Mpv::create_client()` makes a handle with *its own
   event queue and observed-property set* (per mpv docs and libmpv2's doc
   comment). If you `loadfile` on handle A but `wait_event` on client B, **B
   never receives the end-file or property-change events** for that file — the
   loop hangs forever. The working pattern (and what libmpv2's own tests use): a
   single `Mpv` shared as `Arc<Mpv>`; the event thread is the sole `wait_event`
   caller, other threads call `command`/`set_property`/`get_property` on clones.

2. **`Mpv` is `Send + Sync`** (`unsafe impl Send`/`Sync` in `mpv.rs`). The mpv C
   client API is thread-safe, so concurrent `wait_event` (event thread) +
   `command`/`set_property` (UI thread) on the same handle is sound. Confirmed
   under load with no data races.

3. **`set_wakeup_callback` exists but is not needed.** Standard mpv caveat
   applies ("notification only, no API calls inside, wake another thread"). For
   M2 the blocking `wait_event` loop on a dedicated thread is simpler and
   sufficient; reserve the callback only for a future single-thread reactor.

4. **Header / API-version check is automatic and strict on MAJOR.**
   `with_initializer`/`new` compare `MPV_CLIENT_API_MAJOR` (2) against the loaded
   lib's major; a mismatch returns `Error::VersionMismatch`. libmpv2 6.0.0
   advertises client API 2.2; system libmpv is 2.5 — same major (2), so it loads.
   Minor differences are forward-compatible. No header pinning beyond "system
   libmpv major == 2".

5. **`LC_NUMERIC` must be `C`.** mpv requires this (Python set it too). Default C
   locale on Linux already satisfies it, but M2 should set it defensively at
   startup; `Mpv::new` returns `Err` otherwise.

6. **lavfi test media works headless.** `av://lavfi:sine=frequency=440:duration=2`
   plays to a real `EndFile(Eof)` with `ao=null,vo=null,video=no`, no network or
   files — ideal for CI. (ALSA WAVs under `/usr/share/sounds/alsa/` are a
   fallback.) Broken case = any nonexistent path -> ERROR/`Err`.

7. **`default-features = false`.** libmpv2's default feature is `render` (the
   OpenGL/`mpv_render_context` API). The player needs none of it; disabling keeps
   the build lean and future-proofs intent.

8. **ytdl in M2 (option path verified, ytdl itself not exercised).** The spike
   used lavfi sources (ytdl off), but `set_property` at init and
   `set_property`/`command` at runtime all work. For M2, configure like Python:
   - `set_property("ytdl", "yes")`, `set_property("video", "no")` at init;
   - quality mapping via the `ytdl-format` property using the SAME selectors as
     Python's `AUDIO_QUALITY_FORMATS` (`low="bestaudio[abr<=70]/bestaudio/best"`,
     `normal="bestaudio[abr<=131]/bestaudio/best"`, `high="bestaudio/best"`);
   - changing `ytdl-format` mid-session affects only the *next* `loadfile` (same
     as Python — the ytdl hook reads it at load time);
   - play via `command("loadfile", &[url, "replace"])` where `url` is
     `https://music.youtube.com/watch?v=<id>`.
   mpv's bundled `yt-dlp` must be recent (Python flagged a YouTube EJS breakage
   fixed by `yt-dlp>=2026.6.9`) — a runtime concern, not a binding concern.

## Recommended M2 `player.rs` architecture (event thread -> channel)

Mirror the spike's `Player` harness:

- **One `Arc<Mpv>`** via `Mpv::with_initializer` (M2 wants real audio: leave `ao`
  default, set `video=no`, `ytdl=yes`, `terminal=no`, initial `volume`, initial
  `ytdl-format`).
- **A dedicated event thread** looping on `mpv.wait_event(timeout)` with a short
  timeout (spike: 0.25s) so a `stop` `AtomicBool` is honored promptly. It is the
  *only* caller of `wait_event`.
- **A channel** (`std::sync::mpsc`, or crossbeam/tokio to taste) carrying a
  domain `PlayerEvent` enum. The thread translates:
  - `Ok(Event::EndFile(r))` -> classify: `Eof`=>`TrackEnded` (advance);
    `Stop`/`Quit`/`Redirect`=>ignore.
  - `Err(e)` (after a load) -> `TrackError(error_detail(&e))` (notify only).
  - `Ok(Event::PropertyChange{ "time-pos"|"duration", Double(v), .. })` ->
    `Progress`/`Duration` for the player bar.
  - `Ok(Event::StartFile)` / `FileLoaded` -> optional now-playing transitions.
- **Observe properties up front** (`time-pos`, `duration` as `Double`; optionally
  `pause`, `volume`, `media-title`) before the loop starts.
- **Commands from the UI thread** on `Arc` clones: `command("loadfile", ...)`,
  `command("seek", &["5","relative"])` / `&["-5","relative"]`,
  `set_property("pause", bool)`, `set_property("volume", i64)`,
  `set_property("mute", bool)`, `set_property("ytdl-format", sel)`.
- **RAII shutdown**: set the stop flag and `join` the event thread on `Drop`
  (spike's `impl Drop for Player`). Dropping the last `Arc<Mpv>` runs `mpv_destroy`.
- **Port the regression test**: drive `loadfile replace` mid-play and assert the
  interrupted file maps to ignore-not-advance; plus EOF->advance and
  broken-file->error. These are spike scenarios 1-3, the M2 equivalent of
  `tests/test_player.py::TestEndFileHandling`.

## Files in this spike

- `Cargo.toml` — standalone workspace (empty `[workspace]` table), deps
  `libmpv2` (no default features) + `libmpv2-sys`.
- `src/main.rs` — the four-scenario harness; `cargo run` prints PASS/FAIL per
  scenario and exits non-zero if any fail.
- `FINDINGS.md` — this document.
