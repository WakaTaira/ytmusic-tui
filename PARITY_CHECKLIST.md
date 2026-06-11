# Rust Port — Parity Checklist (M7 working document)

> Source of truth for the Rust port's feature parity vs the Python MVP.
> Walks `HANDOFF.md` §2 Feature Inventory and §5.5 spotify_player comparison.
> Status as of M5d (seek/mute, like/radio/a-A, playlist-name input, remove
> actions, keymap warnings). Honest and terse — update as M6/M7 land.
>
> Legend: **works** = fully wired + tested · **partial** = present with a noted
> gap · **missing** = not ported yet.

## HANDOFF §2 Feature Inventory

| Feature | Rust status | Notes |
|---|---|---|
| Home (recommendations) | works | `views/home.rs`; `FetchHome` → `HomeLoaded` |
| Search (4-pane grid) | works | `views/search.rs`; Enter-confirm, 2×2 grid |
| Search filter (`#songs:` …) | works | `Search { filter }` → `search_all(filter)` |
| Library (3-pane) | works | `views/library.rs`; playlists/albums/artists |
| Playlist browse + drill | works | `views/playlist.rs`; two-level, nav-stack pop-back |
| Album / Artist detail | works | `views/album.rs`, `views/artist.rs`; re-fetch on pop-back |
| Queue management | works | `views/queue_view.rs`; jump-to, **Remove from queue** (M5d) |
| Player bar | works | `views/player_bar.rs`; now-playing/progress/volume/**mute** (M5d) |
| Seek (`>` `<` `^`, ±5 s) | works | M5d: `SeekForward/Backward/ToStart`; video_id guard + suppressed-error parity |
| Mute (`_`) | works | M5d: `ToggleMute` + observed `mute` property → `Vol: MUTE` |
| Like / Unlike (`f`) | works | M5d: acts on current track; popup path also wired |
| Start radio (`R`) | works | M5d: current track; popup path also wired |
| Go to current artist / album (`a` / `A`) | works | M5d: search-resolve by name → navigate (see flow below) |
| Action / theme / picker popups | works | `views/popup.rs`; context-aware action lists (M5d) |
| New-playlist naming prompt | works | M5d: name-entry sub-mode inside the picker popup |
| Add to playlist (existing) | works | `AddToPlaylist`; picker seeded from cached library playlists |
| Remove from playlist | works | M5d: `RemoveFromPlaylist`; playlist_id from nav context |
| Lyrics (`L`) | works | `views/lyrics.rs`; `get_lyrics`, "no lyrics" is a valid state |
| Filter bar (`/`) | works | `views/filter_bar.rs`; playlist/history/queue; original-index requeue |
| Nav history (Esc) | works | `navigation.rs`; per-view Esc + page-stack pop |
| Custom keymaps (TOML) | works | `config::load_keymap` + `keymap.rs` dispatcher; sequences (`g s`) |
| Keymap parse warnings | works | **M5d (new vs Python)**: dropped bindings surfaced on the status line once |
| Responsive layout | works | `layout.rs` aspect-ratio 2.3; used by search + library render |
| Session canary (logged-out HTTP 200) | works | `CheckSession` → `is_session_valid` → `SessionInvalid` warning line |
| Audio quality cycle (`b`) | works | `CycleAudioQuality`; applies from next track |
| MPRIS2 | works | M6: mpris-server 0.10 on the runtime tokio; only-changed emit, Position structurally un-emittable, live playerctl/busctl verified |
| CI | works | `.github/workflows/ci.yml`: fmt + clippy (`-D warnings`) + test on every push/PR, with libmpv-dev installed |

## HANDOFF §5.5 spotify_player comparison (remaining gaps)

These were Backlog/Skip in Python; status carried into the Rust port.

| Feature | Rust status | Notes |
|---|---|---|
| Browse page (moods/genres/charts) | missing | needs `get_mood_categories`/`get_charts` API helpers (not in ytmusic-api yet) — Backlog |
| Sort tracks in tables (`s t/a/d`) | missing | Backlog; ratatui enables key-sequences so this is cleaner post-parity (per CLAUDE.md) |
| Top tracks page | missing | Skip (no direct API) |
| Follow/unfollow artist | missing | needs `subscribe_artists` API helper — Backlog (small) |
| Save album/playlist to library | missing | needs `rate_playlist` API helper — Backlog (small) |
| Playlist item reorder (C-j/C-k) | missing | needs `edit_playlist` API helper — Backlog (medium) |
| Copy link (OSC52) | missing | Backlog (small) |
| Album art in terminal | missing | Rust-native (ratatui-image) — post-parity |
| Audio visualization | missing | optional Rust extra |
| Daemon + CLI remote control | n/a | covered by MPRIS once M6 lands |
| Synced lyrics | n/a | YTM returns plain text only — not possible upstream |

Rust extras carried over from Python (spotify_player lacks these): YTM home
recommendations page, `#category:` search prefixes, 4-pane search grid.

## The `a` / `A` current-artist / current-album resolution flow

The Rust `Track` / `AlbumInfo` models do **not** carry an artist `channel_id` or
(for a track) an album `browse_id`, so `a` / `A` cannot resolve a navigation
target by id. The port mirrors Python's `_lookup_and_open_artist` /
`_lookup_and_open_album`: resolve by **search on the name**.

1. **Key press** (`a` / `A`) acts on the **current track** (from the player bar
   state + `current_video_id`). A missing track or an empty artist/album name is
   a **silent no-op** (Python `return`s).
2. **`a`** → `AppCommand::SearchAndOpenArtist(artist_name)`.
   **`A`** → `AppCommand::SearchAndOpenAlbum { name, artist }`.
3. **Runtime** searches the name with the `artists` / `albums` filter
   (`limit = 5`, matching Python), takes the **first hit** carrying a non-empty
   `channel_id` / `browse_id`, and emits `AppEvent::ArtistResolved(id)` /
   `AlbumResolved(id)`.
4. **UI fold** of `ArtistResolved` / `AlbumResolved` reuses the normal
   open-artist / open-album path (`prepare_open_artist` / `prepare_open_album`):
   switch view + push the nav entry, then **chain the fetch** by returning
   `FetchArtist(id)` / `FetchAlbum(id)` as the fold's follow-up command. The
   existing `ArtistLoaded` / `AlbumLoaded` flow then fills the page.

The action popup's **"Go to artist" / "Go to album"** rows use the same
resolution: a track / album row resolves its artist by name, and a track row
resolves its album by name (an album row still uses its `browse_id` directly).

### Failure modes
- **Nothing playing / empty name** → silent no-op (no command sent).
- **Empty search result** → `ApiError("Artist not found: <name>")` /
  `"Album not found: <name>"` on the status line (Python notified a warning).
- **API error** → classified `ApiError` on the status line.
- **First-hit heuristic** → if search returns a different same-named
  artist/album first, the wrong one opens. This is a faithful port of Python's
  "first hit with an id" behavior, not a regression. A disambiguating picker is
  a post-parity refinement.

## Summary counts (as of M7 — Python removed, `rust/` promoted to repo root)

- **§2 Feature Inventory (24 rows):** works 24 · partial 0 · missing 0. MPRIS2
  (M6) and CI both landed; every row in the original Python feature set is now
  fully ported.
- **§5.5 remaining gaps (11 rows):** missing 9 (all pre-existing Backlog/Skip;
  several blocked on absent ytmusic-api helpers) · n/a 2.
- **Net for M7:** there is **no remaining parity gap** inside the original Python
  feature set. Everything still listed as missing was already Backlog/Skip in
  Python, not a regression. Several Backlog items are **API-blocked**:
  `get_mood_categories`, `get_charts`, `subscribe_artists`, `rate_playlist`,
  `edit_playlist` are not yet in `ytmusic-api`.
- **Test suite:** 785 passing + 18 ignored (live, network-gated) across the
  `ytmusic-api` and `ytmusic-tui` crates.
