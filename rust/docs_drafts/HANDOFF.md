# ytmusic-tui — Session Handoff Document (Rust)

> For: any fresh agent continuing this project after M7
> Last major update: 2026-06-11 (M7 — feature parity confirmed, Python removed, rust/ promoted)
> Owner: たいら (WakaTaira)

---

## 1. What This Is

A terminal music player for YouTube Music. Inspired by
[spotify_player](https://github.com/aome510/spotify_player). Written in Rust
(ratatui + libmpv-rs + a hand-rolled InnerTube client).

```
ytmusic-api (InnerTube) → libmpv (playback) → ratatui (TUI)
```

## 2. Current State — M7 Parity Complete

| Metric | Value |
|--------|-------|
| Tests | 728 passing (cargo test, dry run; 18 ignored = live integration tests) |
| Lint | cargo fmt --check + cargo clippy -- -D warnings: clean |
| CI | Rust CI (`.github/workflows/rust.yml`): fmt/clippy/test on every push to `rust/` paths |
| Auth | Browser cookie auth (`~/.config/ytmusic-tui/browser.json`). OAuth broken upstream (ytmusicapi #813) |
| MPRIS | mpris-server 0.10 on the tokio runtime; live-verified with playerctl |

### 2.1 Feature Inventory (all works unless noted)

| Feature | Notes |
|---|---|
| Home (recommendations) | `views/home.rs`; `FetchHome` → `HomeLoaded` |
| Search (4-pane grid) | `views/search.rs`; Enter-confirm, 2×2 grid |
| Search filter (`#songs:` …) | `Search { filter }` → `search_all(filter)` |
| Library (3-pane) | `views/library.rs`; playlists/albums/artists + liked-songs row |
| Playlist browse + drill | `views/playlist.rs`; two-level, nav-stack pop-back |
| Album / Artist detail | `views/album.rs`, `views/artist.rs`; re-fetch on pop-back |
| Queue management | `views/queue_view.rs`; jump-to, Remove from queue |
| Player bar | `views/player_bar.rs`; now-playing/progress/volume/mute |
| Seek (`>` `<` `^`, ±5 s) | `SeekForward/Backward/ToStart`; video_id guard + suppressed-error parity |
| Mute (`_`) | `ToggleMute` + observed `mute` property → `Vol: MUTE` |
| Like / Unlike (`f`) | acts on current track; popup path also wired |
| Start radio (`R`) | current track; popup path also wired |
| Go to current artist / album (`a` / `A`) | search-resolve by name → navigate |
| Action / theme / picker popups | `views/popup.rs`; context-aware action lists |
| New-playlist naming prompt | name-entry sub-mode inside the picker popup |
| Add to playlist (existing) | `AddToPlaylist`; picker seeded from cached library playlists |
| Remove from playlist | `RemoveFromPlaylist`; playlist_id from nav context |
| Lyrics (`L`) | `views/lyrics.rs`; `get_lyrics`, "no lyrics" is a valid state |
| Filter bar (`/`) | `views/filter_bar.rs`; playlist/history/queue; original-index requeue |
| Nav history (Esc) | `navigation.rs`; per-view Esc + page-stack pop |
| Custom keymaps (TOML) | `config::load_keymap` + `keymap.rs` dispatcher; sequences (`g s`) native |
| Keymap parse warnings | dropped bindings surfaced on the status line once at startup |
| Responsive layout | `layout.rs` aspect-ratio 2.3; used by search + library render |
| Session canary | `CheckSession` → `is_session_valid` → `SessionInvalid` warning line |
| Audio quality cycle (`b`) | `CycleAudioQuality`; applies from next track |
| MPRIS2 | mpris-server 0.10 on the runtime tokio; only-changed emit, Position structurally un-emittable, live playerctl verified |

### 2.2 Post-Parity Backlog

Items that were Backlog/Skip in Python and carry forward unchanged:

| Feature | Blocked on | Effort |
|---|---|---|
| Browse page (moods/genres/charts) | `get_mood_categories`/`get_charts` API helpers not in `ytmusic-api` yet | Medium |
| Sort tracks in tables (`s t/a/d`) | Cleanest post-parity; ratatui key-sequences make this tractable | Medium |
| Follow/unfollow artist | needs `subscribe_artists` API helper | Small |
| Save album/playlist to library | needs `rate_playlist` API helper | Small |
| Playlist item reorder (C-j/C-k) | needs `edit_playlist` API helper | Medium |
| Copy link (OSC52) | standalone; no deps | Small |
| Album art in terminal | ratatui-image | Rust-native |
| Audio visualization | optional extra | Rust-native |
| Top tracks page | no direct YTM API | Skip |
| Synced lyrics | YTM returns plain text only | Not possible upstream |

### 2.3 Known M7-Deferred Decisions

- **Liked Songs row in library**: currently shows as `"★ Liked Songs"` in the
  library playlists pane only when `LikedSongsLoaded` has fired. Decision
  deferred: should this row always appear (as a placeholder that triggers a
  fetch) or remain absent until the data arrives?
- **Search limit**: `SEARCH_LIMIT = 20` (per-category cap, matching Python's
  `search_all(limit=20)`). No pagination; a larger or configurable limit is a
  Backlog item once continuations are in scope.

### 2.4 Code Quality TODOs in Tree

These are annotated in the source; do not fix without understanding the parity
rationale:

- `queue.rs:47` — `current_index: i64` uses `-1` as a sentinel (empty queue).
  `TODO(post-parity): consider Option<usize>` once the Python parity tests are
  retired.
- `models.rs:284,295` — `AlbumInfo`/`RelatedArtist` lack `Hash`; add in
  lockstep if render-time dedup is needed.

## 3. Architecture

### Crate Layout

```
rust/
├── Cargo.toml          # Workspace: members = ["ytmusic-api", "ytmusic-tui"]
├── ytmusic-api/        # Library crate: InnerTube transport, auth, domain models, parse
└── ytmusic-tui/        # Binary (ytmusic-tui) + lib crate: TUI, player, queue, config
```

**`ytmusic-api`** is a pure library. It depends on reqwest (rustls), serde,
serde_json, sha1, thiserror. No ratatui, no tokio (except for test helpers),
no UI code.

**`ytmusic-tui`** depends on `ytmusic-api`, ratatui, crossterm, libmpv2,
tokio (rt + sync), mpris-server (0.10), zbus (5.16), thiserror.

### `ytmusic-api` internals

```
src/
├── lib.rs              # Public re-exports: InnerTubeClient, BrowserAuth, Track, ...
├── auth.rs             # BrowserAuth::load, SAPISIDHASH generation
├── client.rs           # InnerTubeClient: HTTP post, is_session_valid canary
├── classify.rs         # classify_api_error: auth / not-found / network / server
├── context.rs          # InnerTube request context builder
├── endpoints/
│   ├── mod.rs          # PostRequest seam (trait) + InnerTubeClient method wrappers
│   ├── home.rs         # get_home_sections
│   ├── search.rs       # search_all + filter routing (musicVideoType disambiguation)
│   ├── library.rs      # get_library_playlists, get_library_albums, get_library_artists
│   ├── playlist.rs     # get_playlist_tracks
│   ├── album.rs        # get_album
│   ├── artist.rs       # get_artist
│   ├── lyrics.rs       # get_lyrics
│   ├── history.rs      # get_history
│   ├── radio.rs        # get_radio
│   ├── mutations.rs    # rate_track, create_playlist, add_to_playlist, remove_from_playlist
│   ├── stage1.rs       # musicShelfRenderer column-index parser (shared by search+playlist)
│   ├── songruns.rs     # songRun extractor (title/artist from flexColumns)
│   └── tests.rs        # Fixture-driven endpoint tests + FakePost stub
├── models.rs           # Track, AlbumInfo, ArtistInfo, PlaylistInfo, SearchResults, ...
├── nav.rs              # Value navigator helpers (descend, get_str, get_arr, ...)
└── parse.rs            # musicDetailRenderer / thumbnailRenderer shared parsers
```

### `ytmusic-tui` internals

```
src/
├── main.rs             # Binary: config load, terminal setup, ratatui render/input loop
│                       # AppModel: view state + event folding + key dispatch (pure-ish)
├── lib.rs              # Module root
├── app/mod.rs          # AppCommand / AppEvent / RuntimeHandle / spawn_runtime
├── player.rs           # libmpv2 wrapper: play, seek, vol, mute, PlayerEvent stream
├── queue.rs            # QueueManager: track list, shuffle, repeat, current index
├── config.rs           # AppConfig, KeymapConfig, themes, load_config / load_keymap
├── keymap.rs           # Keymap: key→Action dispatch, two-key sequence state machine
├── navigation.rs       # NavigationManager (page-stack) + PageState (nav entry)
├── layout.rs           # Orientation detection (aspect ratio 2.3 threshold)
├── formatting.rs       # Duration formatting
├── mpris/
│   ├── mod.rs          # MPRIS task: spawn, MprisHandle, update_state, death watcher
│   ├── player_impl.rs  # RootInterface + PlayerInterface trait impls
│   └── trackid.rs      # object-path-safe track id encoding
└── views/
    ├── mod.rs          # Theme + shared PageState<T> enum (Loading/Loaded/Error)
    ├── home.rs         # HomeView
    ├── search.rs       # SearchView (2×2 grid)
    ├── library.rs      # LibraryView (3-pane)
    ├── playlist.rs     # PlaylistView (2-level)
    ├── album.rs        # AlbumView
    ├── artist.rs       # ArtistView (3-section)
    ├── lyrics.rs       # LyricsView
    ├── history.rs      # HistoryView
    ├── queue_view.rs   # QueueView
    ├── player_bar.rs   # PlayerBarState + render
    ├── popup.rs        # ActionPopup / ThemePopup / PlaylistPickerPopup
    └── filter_bar.rs   # FilterBar (substring filter for flat-list views)
```

### Thread Model (the M5a fixed architecture)

```
┌──────────────────────────────────────────────────────────────┐
│ main thread: synchronous ratatui + crossterm render/input loop │
│   poll crossterm (60 ms timeout) → AppCommand                  │
│   drain AppEvent receiver (try_recv) each tick → mutate state  │
│   render AppModel                                              │
└────────┬──────────────────────────────────▲──────────────────┘
         │ AppCommand                        │ AppEvent
         │ tokio::sync::mpsc (UI→runtime)    │ std::sync::mpsc (→UI)
         ▼                                   │
┌─────────────────────────────────────────────┴──────────────────┐
│ runtime thread (std::thread hosting tokio::Runtime::block_on)   │
│   owns: InnerTubeClient, QueueManager, Player                   │
│   async command loop: cmd_rx.recv().await                       │
│   FetchHome → client.get_home() → HomeLoaded / ApiError         │
│   Play / TogglePause / AdjustVolume → player ops                │
│   MPRIS server tasks join this same runtime (M6)                │
└──────────────────────────────────────▲─────────────────────────┘
         ▲ PlayerEvent (std::sync::mpsc, from mpv's event thread) │
         │                                                         │
┌────────┴─────────────────────────────────────────────────────┐  │
│ player forwarder thread (std::thread)                          │  │
│   loop { player_events.recv() → map to AppEvent → ev_tx }     │──┘
│   M6: second sink to MPRIS via a direct update channel         │
└────────────────────────────────────────────────────────────────┘
```

Key channel choices:
- **AppCommand**: `tokio::sync::mpsc::UnboundedSender/Receiver` — the
  consumer is an async task; the producer is synchronous (no `await` needed).
- **AppEvent**: `std::sync::mpsc::Sender/Receiver` — the consumer is the
  synchronous render loop (`try_recv` per tick); producers are the runtime
  thread and the forwarder.

Auto-advance flow: a natural EOF arrives on the forwarder thread, which
emits `AppEvent::TrackEnded`. The UI fold returns `AppCommand::NextTrack`.
The runtime advances the queue and emits `AppEvent::NowPlaying`. A
`TrackError` event does **NOT** trigger `NextTrack` — a broken stream must
never machine-gun the queue (the end-file battle lesson).

## 4. Battle Lessons (encoded as design constraints)

These are not notes about past bugs — they are invariants baked into the code.
Do not undo them.

### 4.1 mpv end-file reasons must be classified

**`classify_end_file_reason`** in `player.rs` maps `EndFileReason` to
`EndFileAction`:

- `Eof` → `TrackEnded` → UI sends `NextTrack` (auto-advance)
- `Error` → `TrackError` → UI shows error toast; **no auto-advance**
- `Stop / Quit / Redirect` → `Ignore` (file replacement during seek/load)

The `Stop` path is critical: replacing a playing file (`loadfile` while
something is playing) fires an `EndFile(Stop)` event. Reacting to it as EOF
would skip past the user's selection. The same bug existed in Python (fixed in
commit `3f4e97c`).

### 4.2 D-Bus EAGAIN is structurally absent in zbus

**Python lesson:** dbus-next / dbus-fast had a synchronous write loop that
treated `BlockingIOError` (EAGAIN) as fatal, silently deregistering the writer.

**Rust approach:** mpris-server 0.10 is built on zbus 5.x, which uses tokio's
async I/O. `AsyncWriteExt::write_all` never surfaces `EAGAIN` as an error —
the tokio reactor retries transparently. The Python vendored EAGAIN patch has
no equivalent in the Rust build; it is structurally not needed.

### 4.3 Position is un-emittable via PropertiesChanged

The MPRIS spec says clients should not deduce Position from property changes.
mpris-server's `Player::set_position` deliberately does NOT emit
`PropertiesChanged` for Position. The Rust MPRIS task never calls
`set_position` in a periodic loop; position is only set when it would be read
directly (via `Player::get_position`). This is correct spec behavior and
prevents signal flooding.

The waybar counter freeze this causes is a waybar client-side issue; fix by
adding `"interval": 1` to the mpris block in `~/.config/waybar/config.jsonc`.

### 4.4 Session canary: auth rot looks like empty HTTP 200

YouTube serves logged-out pages with HTTP 200. A stale `browser.json` returns
empty playlists and `None` payloads with no error. `InnerTubeClient::is_session_valid`
hits the `account` endpoint on startup; a missing signed-in structure returns
`false`. The runtime emits `AppEvent::SessionInvalid` → the UI shows a
persistent banner: `"Session invalid — desktop sign-in expired. Run: ytmusic-tui auth"`.

### 4.5 Scan-loop let-else-continue pattern in InnerTube parsers

InnerTube responses embed track data in nested shelf/renderer structures. Many
fields are optional; a missing field means "skip this item", not "fail the
parse". The parser convention is:

```rust
for item in shelf_items {
    let Some(video_id) = nav(&item, &[...]) else { continue };
    let Some(title) = nav_str(&item, &[...]) else { continue };
    // only push when all required fields resolved
    tracks.push(Track { video_id, title, ... });
}
```

This mirrors ytmusicapi's `if not channel_id: continue` pattern. Do not convert
these to `?`-propagated errors — a missing field in one row must not abort the
entire list.

### 4.6 Music-video ID substitution in search

YouTube Music returns two result shapes for a song: a pure-audio track (no
video) and a music video. The `musicVideoType` field distinguishes them.
`search.rs` walks the `musicVideoType` run to identify `MUSIC_VIDEO_TYPE_ATV`
(audio track). The parser discards music-video entries (type ≠ ATV) to avoid
duplicates. This mirrors `api.py`'s `song` vs `video` routing in `search_all`.

## 5. Conventions

### 5.1 PageState<T> view pattern

Every data-backed view carries a `PageState<T>` enum (`Loading` / `Loaded(T)` /
`Error(String)`). The view renders a spinner for `Loading`, the content for
`Loaded`, and an error line for `Error`. The event fold on `AppModel::on_event`
transitions state and clears `self.status` on a successful load. On
`AppEvent::ApiError` the active view's `set_error` is called to replace a stuck
`Loading` with the error message.

### 5.2 PostRequest seam for endpoint testability

All endpoint functions accept `&impl PostRequest` rather than
`&InnerTubeClient` directly. `PostRequest` is a sealed async trait with one
method (`post`). `InnerTubeClient` implements it for production use;
`FakePost` (in `tests.rs`) implements it with a fixture-returning fake. This
means every endpoint is unit-testable without HTTP.

### 5.3 Fixture provenance method

InnerTube fixture files in `ytmusic-api/tests/fixtures_innertube/` are raw
JSON captures from the live API. Ground-truth expected values are independently
derived by running the **Python** `api.py` pipeline over the same fixture (not
by hand-reading the JSON). This dual-derivation prevents tests from merely
mirroring the Rust parser's bugs.

To generate a new fixture:
1. Capture the raw InnerTube POST response (Wireshark or reqwest debug log).
2. Save as `tests/fixtures_innertube/<name>.json`.
3. Run the Python `api.py` method against the same JSON to derive expected values.
4. Write a `FakePost`-based test asserting the Rust parser matches Python's output.

### 5.4 Conventions checklist

- `navigate_to_*` functions (e.g. `open_album`, `open_artist`) always
  `push` to the nav stack AND return the follow-up fetch command as the fold's
  `Option<AppCommand>` return value. Do not split the two.
- `Action` enum variants are named after the user action, not the implementation
  detail (`Action::ToggleLike`, not `Action::RateCurrentTrack`).
- Keymap `.toml` key strings follow Python/Textual conventions (the
  `default_keymap.toml` shipped in `config/` is the canonical reference).
- The `QueueManager` lives exclusively in the runtime thread. Never send it
  across thread boundaries.

## 6. spotify_player Feature Comparison (remaining gaps)

Unchanged from Python — these are Backlog/Skip, not regressions:

| Feature | Status | Notes |
|---|---|---|
| Browse page | missing | API helpers absent |
| Sort tracks in tables | missing | ratatui key-sequences enable this post-parity |
| Top tracks page | skip | no direct API |
| Follow/unfollow artist | missing | needs `subscribe_artists` |
| Save album/playlist | missing | needs `rate_playlist` |
| Playlist item reorder | missing | needs `edit_playlist` |
| Copy link (OSC52) | missing | small; standalone |
| Album art in terminal | missing | ratatui-image, post-parity |
| Synced lyrics | n/a | YTM plain text only |

Rust extras vs Python (which spotify_player also lacks): YTM home
recommendations, `#category:` search prefixes, 4-pane search grid, native
two-key sequence support in keymap (`g s`), per-startup keymap warning line.

## 7. Commands

```bash
# Build
cd rust
cargo build --release
# Binary: rust/target/release/ytmusic-tui

# Test (all; skips live integration tests)
cargo test

# Unit tests only for the API crate
cargo test -p ytmusic-api

# Live integration tests (requires browser.json + network)
cargo test -p ytmusic-api -- --ignored

# Lint gate (matches CI)
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings

# Auth setup (requires Python ytmusicapi)
ytmusicapi browser --file ~/.config/ytmusic-tui/browser.json
```

## 8. Project Policies

- Code + comments: English (OSS); conversation: Japanese
- Conventional Commits; **never push to main without たいら's confirmation**
- `cargo fmt --check` + `cargo clippy -D warnings` + `cargo test` must stay
  green (CI gate is `.github/workflows/rust.yml`)
- `unsafe_code = "warn"` in workspace `Cargo.toml` — every `unsafe` block
  needs a `// SAFETY:` comment
- MIT license, repo WakaTaira/ytmusic-tui (private → public)
- Branch `rust-rewrite` is the working branch; main is the stable branch;
  no direct commits to main
