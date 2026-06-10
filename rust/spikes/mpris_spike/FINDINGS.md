# MPRIS Crate Selection Spike — Findings (M6)

Spike for choosing the Rust MPRIS server crate for the ytmusic-tui port.
Built and probed live on Arch Linux against the user session bus.

## TL;DR — Recommendation

**Use `mpris-server` v0.10.0** (built on `zbus` v5.16.0), with the `tokio`
feature enabled. First-choice candidate from the directive; cleared every
requirement:

- The async zbus write path makes `WouldBlock`/`EAGAIN` **structurally
  impossible to be fatal** — the exact failure class that hung the Python
  (dbus-fast) build cannot occur (source citations below).
- The crate emits `PropertiesChanged` with **only the properties you pass**, and
  declares `Position` as `emits_changed_signal = "false"`, so the waybar
  "never spam Position" rule is enforced at the library level.
- Connection death is **observable two ways** (`MessageStream` error/EOF and
  `Connection::monitor_activity()`), the zbus analogue of Python's
  `bus.wait_for_disconnect()`.
- `mpris:trackid` is **forced to a real D-Bus object path** (`o`), so we emit a
  spec-compliant value instead of the Python `Variant("s")` workaround — but it
  means YouTube IDs must be encoded (a `-` is illegal in an object path).

### Resolved versions (from `Cargo.lock`)

| Crate | Version |
|-------|---------|
| `mpris-server` | 0.10.0 |
| `zbus` | 5.16.0 |
| `zvariant` | 5.12.0 |
| `zbus_names` | 4.3.2 |

`mpris-server` 0.10.0 declares `zbus = "5.14"`; it resolved to 5.16.0 here.

---

## Candidate comparison

| Candidate | Backend | EAGAIN safety | Trackid type | Verdict |
|-----------|---------|---------------|--------------|---------|
| **`mpris-server` 0.10** | zbus (async; tokio or async-io) | structural — `WouldBlock` re-registers the waker, never fatal | `TrackId(ObjectPath)` → forced `o` | **CHOSEN** |
| `souvlaki` 0.8.3 | **default = C `libdbus`** via the `dbus` crate; optional `use_zbus` uses `pollster` (blocking) | default path is the synchronous C library — not the async-poll model we want; would re-introduce manual backpressure handling | abstracts metadata, less control | Rejected — wrong abstraction level + non-async default; also cross-platform baggage we don't need |
| raw `zbus` `#[interface]` | zbus (async) | identical to mpris-server (same write path) | you marshal it yourself | Viable fallback, but mpris-server already gives us the typed `RootInterface`/`PlayerInterface` traits, the bus-name prefix, and the `properties_changed` categorizer for free. No reason to hand-roll. |

`souvlaki` detail: `cargo info souvlaki` shows `default = [use_dbus]` →
`dbus` + `dbus-crossroads` (libdbus C bindings). The zbus backend is opt-in and
drives it with `pollster` (a block-on executor), the opposite of the poll-based
reactor that gives us the EAGAIN guarantee. For a project whose whole MPRIS
history is an EAGAIN backpressure bug, picking the libdbus path would throw away
the very property we are selecting for.

---

## Core deliverable: EAGAIN / WouldBlock source-level audit

**Verdict: in zbus 5.x, a full kernel send buffer (`EAGAIN`/`WouldBlock`) is
NOT an error condition at all. It is a poll signal that yields `Poll::Pending`
and re-registers the task waker. The runtime reactor re-wakes the write when the
socket is writable again. There is no code path where transient backpressure
tears down the connection.** This is the structural opposite of the Python bug,
where dbus-fast caught `BlockingIOError` in a blanket `except` and silently
deregistered the writer (HANDOFF.md §4).

### The write path, traced

`mpris-server` never touches sockets directly. Emission flows:

```
Server::properties_changed([Property::...])         // mpris-server/src/server.rs
  +- Connection::emit_signal(...)                    // zbus/src/connection/mod.rs
       +- writes a Message through the WriteHalf     // zbus/src/connection/socket/mod.rs
            +- WriteHalf::sendmsg(buffer, fds)        // zbus/src/connection/socket/unix.rs  <- THE write
```

The `sendmsg` impl is where EAGAIN would surface. Both reactor backends handle
it the same way.

#### async-io backend (default zbus feature)

`zbus/src/connection/socket/unix.rs`, `impl super::WriteHalf for
Arc<Async<UnixStream>>`, `async fn sendmsg` (~ lines 64-95). Verbatim:

```rust
async fn sendmsg(&mut self, buffer: &[u8], fds: &[BorrowedFd<'_>]) -> std::io::Result<usize> {
    poll_fn(|cx| {
        loop {
            match fd_sendmsg(self.as_fd(), buffer, fds) {
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    match self.poll_writable(cx) {
                        Poll::Pending => return Poll::Pending,   // <- yield; waker re-registered
                        Poll::Ready(res) => res?,                // <- writable again: loop & retry
                    }
                }
                v => return Poll::Ready(v),
            }
        }
    })
    .await
}
```

`WouldBlock` (the Rust kind for `EAGAIN`/`EWOULDBLOCK`) is matched **before** the
catch-all `v => return Poll::Ready(v)`. It can never reach the error return. It
calls `Async::poll_writable`, which registers the waker with the `async-io`
reactor; the future yields `Poll::Pending`; the reactor re-polls once the fd is
writable. `Interrupted` (`EINTR`) is likewise swallowed and retried. The only
values that propagate out are real errors (`ECONNRESET`, etc.).

#### tokio backend (the feature we enable)

`zbus/src/connection/socket/unix.rs`, `impl super::WriteHalf for
tokio::net::unix::OwnedWriteHalf`, `async fn sendmsg` (~ lines 166-195):

```rust
poll_fn(|cx| {
    loop {
        match stream.try_io(tokio::io::Interest::WRITABLE, || {
            fd_sendmsg(stream.as_fd(), buffer, fds)
        }) {
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                match stream.poll_write_ready(cx) {
                    Poll::Pending => return Poll::Pending,   // <- same structure
                    Poll::Ready(res) => res?,
                }
            }
            v => return Poll::Ready(v),
        }
    }
})
.await
```

Same shape: `try_io(WRITABLE, ...)` returns `WouldBlock` when the buffer is full,
which routes to `poll_write_ready` (registers the waker with the tokio runtime)
and yields `Poll::Pending`. Identical guarantee.

### Why the Python bug cannot recur here

| | Python (dbus-fast 5.x) | Rust (zbus 5.16) |
|--|------------------------|------------------|
| Full send buffer raises | `BlockingIOError` (EAGAIN) | `io::ErrorKind::WouldBlock` |
| How it's handled | blanket `except Exception` treats it as fatal; `_finalize()` drops the loop writer without closing the socket | matched explicitly; calls `poll_writable`, yields `Poll::Pending`, waker re-registered |
| Result | bus name stays registered, nobody serves it -> "hang" | task parks, resumes when writable; service keeps serving |
| Fix required | vendored EAGAIN-tolerant writer patch | none — it's how the async transport is built |

The standalone-repro caveat from the Python story (the bug only showed up under
1 Hz update traffic) is exactly what the stress scenario below reproduces — and
zbus passes it cold, with no patch.

---

## Scenario results (live, on the session bus)

Bus name: `org.mpris.MediaPlayer2.ytmusic_spike`, object `/org/mpris/MediaPlayer2`.

### 1 + 2. Minimal player + external probe

`busctl --user introspect ... org.mpris.MediaPlayer2.Player` (abridged):

```
.Metadata       property a{sv}  4 "mpris:length" x 213000000 "mpris:trackid" o "/dev/ytmusic_tui/track/dQw4_2d9WgXcQ" "xesam:artist" as 1 "Rick Astley" "xesam:title" s "Never Gonna Give You Up"   emits-change
.PlaybackStatus property s      "Paused"    emits-change
.Position       property x      0           (no emits-change flag)
.Volume         property d      0.8         emits-change writable
.CanControl     property b      true
.SetPosition    method   ox     -
```

`playerctl -p ytmusic_spike metadata`:

```
ytmusic_spike mpris:length   213000000
ytmusic_spike mpris:trackid  '/dev/ytmusic_tui/track/dQw4_2d9WgXcQ'
ytmusic_spike xesam:artist   Rick Astley
ytmusic_spike xesam:title    Never Gonna Give You Up
```

`playerctl --list-all` -> `ytmusic_spike`. Both clients serve correctly.

### 3. Stress (the EAGAIN reproduction)

5000 `PropertiesChanged` as fast as possible, then a 3 s burst loop, **while
30 concurrent `busctl get-property` probes ran against it**:

```
[stress] 5000 emits in 185.57ms (26943 emits/s) — service still up
[stress] burst loop done: 117989 emits over 3s (~39000 emits/s) — service still up
concurrent busctl probes during stress: OK=30 FAIL=0
```

No hang, no timeout, no silent death. This is the precise traffic profile that
killed the Python build; zbus serves through it untouched.

### 4. EAGAIN audit

See the section above — source citations in `zbus/src/connection/socket/unix.rs`.

### 5. Connection-death observability (`death` run mode)

```
[death-demo] opened session connection: Some(OwnedUniqueName(":1.1263"))
[death-demo] closing connection now...
[death-demo] monitor_activity() fired => observable
[death-demo] MessageStream yielded Err => disconnect: I/O error: failed to read from socket
[death-demo] done — connection death was observable on BOTH paths
```

Both detection paths fired. Recipe documented below.

### Inbound control relay

`playerctl play-pause / next / previous` were each received by the app over the
forwarding channel:

```
[control] received from D-Bus client: PlayPause
[control] received from D-Bus client: Next
[control] received from D-Bus client: Previous
```

---

## `mpris:trackid` findings (important — behavior differs from Python)

**`mpris_server::TrackId` is a newtype over `zbus::zvariant::ObjectPath`** and
forces a valid D-Bus **object path** (`o`). Source: `mpris-server/src/track_id.rs`:

```rust
pub struct TrackId(ObjectPath<'static>);
// TryFrom<&str>/<String> go through ObjectPath::try_from -> can FAIL.
```

`Metadata::set_trackid(Option<impl Into<TrackId>>)` therefore only accepts a
valid object path. **There is no escape hatch** to emit a plain `s` string the
way Python did with `Variant("s", ...)`.

Why it matters for YouTube: D-Bus object-path elements may only contain
`[A-Za-z0-9_]`. YouTube video IDs use the base64url alphabet, which includes
`-`. A real ID like `dQw4-9WgXcQ` or `O-_kV-pP4kE` is therefore an **invalid**
object-path element and `ObjectPath::try_from` rejects it. (Unit test
`raw_youtube_id_with_dash_is_rejected_by_objectpath` proves this.)

**Decision for M6: encode, don't escape-hatch.** This spike maps the YouTube ID
into a guaranteed-valid path (`src/trackid.rs`): `-` -> `_2d`, `_` -> `_5f`
(reversible, stays within `[A-Za-z0-9_]`), under our own namespace
`/dev/ytmusic_tui/track/<encoded>`. The live probe confirms the emitted
`mpris:trackid` is type `o` = `/dev/ytmusic_tui/track/dQw4_2d9WgXcQ`. This is
**strictly better than the Python workaround**: it is spec-compliant, playerctl
reads it fine, and it round-trips back to the original video ID. Never use a path
under `/org/mpris` (spec-reserved); `.../TrackList/NoTrack` (`TrackId::NO_TRACK`)
is the canonical "no track" value.

---

## Threading / executor model (M6: MPRIS alongside a sync ratatui loop)

### What zbus runs internally

zbus owns an internal executor abstraction (`crate::Executor` / `crate::Task`,
`async-executor`-based) plus a socket-reader task
(`zbus/src/connection/socket_reader.rs`). With the **`tokio` feature** (what we
enable), zbus integrates with the ambient tokio runtime instead of spinning its
own async-io reactor thread, so all D-Bus I/O is driven by tokio.

### Recommended M6 architecture

ratatui draws on a **synchronous** loop; zbus/mpris-server are **async** and want
tokio. Keep them on separate threads and talk over channels — this mirrors the
Python design that fixed the bug (all D-Bus mutation on one executor context;
UI <-> D-Bus hand-off via channels, the analogue of `call_soon_threadsafe` /
`call_from_thread`):

```
main thread: ratatui sync render/input loop (crossterm)
   |  std::sync::mpsc / tokio mpsc
   v
dedicated tokio runtime thread (std::thread + Runtime::new().block_on)
   +- mpris_server::Server (owns the zbus Connection)
   +- task: drain UI->MPRIS state updates -> Server::properties_changed([...changed])
   +- task: forward inbound control (Play/Pause/Next) MPRIS->UI
```

- Build one `tokio::runtime::Runtime` (multi-thread or current-thread+`LocalSet`)
  on its own `std::thread`; the player/mpv side can share it.
- **All property mutation happens inside that runtime** (the single-executor
  rule). The UI thread never calls zbus directly; it sends a typed message and
  the MPRIS task applies it. This spike models it with
  `Arc<Mutex<PlayerState>>` + an mpsc `Control` channel.
- mpris-server's `Server` is `Send + Sync + 'static` (asserted in its own tests),
  so sharing it across tasks via `Arc` is fine.

> Note: the `mpris-server` examples use `#[tokio::main(flavor = "local")]` +
> `spawn_local(player.run())` because the ready-made `Player` helper is `!Send`.
> We use the lower-level `Server` + manual `RootInterface`/`PlayerInterface`
> impls instead (this spike does), which **is** `Send + Sync`, so it composes
> cleanly with a multi-thread runtime and a separate UI thread. That is the
> recommended path for M6.

---

## Only-changed-properties emission pattern (waybar lesson, in code)

`Server::properties_changed(properties: impl IntoIterator<Item = Property>)`
emits `PropertiesChanged` containing **only** the `Property` variants you pass,
auto-sorted into the spec's `changed` vs `invalidated` buckets (source:
`mpris-server/src/server.rs`). Pass exactly what changed:

```rust
// On track change — emit ONLY status + metadata, nothing else:
server.properties_changed([
    Property::PlaybackStatus(PlaybackStatus::Playing),
    Property::Metadata(new_metadata),
]).await?;

// On pause — emit ONLY the status:
server.properties_changed([Property::PlaybackStatus(PlaybackStatus::Paused)]).await?;
```

**Position is never emitted here.** The crate declares the `Position` property
`#[zbus(property(emits_changed_signal = "false"))]` (source: `server.rs`
`RawPlayerInterface::position`), so it is structurally excluded from
`PropertiesChanged`. The live `busctl introspect` confirms `Position` has no
`emits-change` flag while `PlaybackStatus`/`Metadata`/`Volume` do. This satisfies
the waybar rule at the library level — there is no way to accidentally spam
Position. (waybar still renders Position client-side via its `"interval": 1`
config; that is a client setting, unchanged from the Python findings.)

---

## Connection-death detection recipe (zbus equivalent of `wait_for_disconnect`)

Two independent, non-silent signals — use either or both:

1. **`MessageStream` over the connection.** When the socket dies, the zbus
   socket-reader (`socket_reader.rs::receive_msg`) broadcasts the read error to
   all stream senders, then `senders.clear()` and returns. So a
   `zbus::MessageStream` yields `Some(Err(_))` then `None`. Demonstrated live:
   `MessageStream yielded Err => disconnect: I/O error: failed to read from socket`.

2. **`Connection::monitor_activity() -> EventListener`** (source:
   `zbus/src/connection/mod.rs:1229`). Returns an `event_listener::EventListener`
   you can `.await`. `Connection::close()` calls
   `self.inner.activity_event.notify(usize::MAX)` (mod.rs:1285), so the listener
   resolves on close/death. Demonstrated live: `monitor_activity() fired`.

Minimal pattern (see `src/main.rs::demo_connection_death`):

```rust
let activity = conn.monitor_activity();          // arm before anything else
let watch = conn.clone();
tokio::spawn(async move {
    let mut stream = zbus::MessageStream::from(watch);
    while let Some(item) = stream.next().await {
        if item.is_err() { /* surface to UI: reconnect / warn */ break; }
    }
    // stream ended (None) => connection is dead — also surface it
});
// elsewhere: `activity.await` resolves when the connection dies/closes.
```

For M6: on either signal, surface a visible status (toast) and attempt to rebuild
the `Server`, rather than letting the bus name silently stop being served — the
positive lesson from the Python incident. To intentionally tear down, prefer
`Connection::graceful_shutdown()` (mod.rs:1338) which drains outstanding calls;
`close()` is the hard variant.

---

## Files in this spike

```
rust/spikes/mpris_spike/
+- Cargo.toml          # standalone [workspace]; mpris-server + zbus (tokio feature)
+- Cargo.lock
+- FINDINGS.md         # this file
+- src/
   +- main.rs          # serve | once | death modes; stress loop; only-changed emission; death demo
   +- player_impl.rs   # manual RootInterface + PlayerInterface (Send+Sync); control relay channel
   +- trackid.rs       # YouTube-ID -> valid ObjectPath encoding + tests (the trackid constraint)
```

Run modes:
- `cargo run` (or `... serve`) — register, run the stress loop, stay alive for
  external `busctl`/`playerctl` probes.
- `cargo run -- once` — register, emit one update, exit after 2 s.
- `cargo run -- death` — connection-death observability demo.
- `cargo test` — trackid encoding/round-trip proofs (4 tests).

Quality: `cargo fmt` applied; `cargo clippy --all-targets -- -D warnings` clean;
`cargo test` green (4/4).
