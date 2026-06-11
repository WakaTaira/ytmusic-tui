//! `ytmusic-tui` binary entry point: config + auth + player wiring, the
//! crossterm terminal setup, and the synchronous ratatui render/input loop.
//!
//! # Shape (the M5a fixed architecture)
//!
//! `main` is deliberately thin. It:
//!
//! 1. Loads config, auth, and the player **before** touching the terminal, so
//!    any failure prints a clean line to stderr and exits non-zero while the
//!    terminal is still in its normal cooked mode.
//! 2. Spawns the runtime thread ([`ytmusic_tui::app::spawn_runtime`]) that owns
//!    the InnerTube client and the player.
//! 3. Installs a panic hook that restores the terminal, enters raw mode + the
//!    alternate screen, and runs [`run_loop`].
//! 4. On exit, tears the runtime down ([`RuntimeHandle::shutdown`]) and restores
//!    the terminal.
//!
//! The loop itself is split into a pure [`AppModel`] (view state + event
//! folding + key→action mapping) and the thin terminal I/O in [`run_loop`], so
//! the dispatch logic is unit-tested without a TTY (there is none in CI).
//!
//! # Keymap (M5c)
//!
//! Keys are resolved through [`ytmusic_tui::keymap::Keymap`], built at startup
//! from the merged `action → key` map ([`config::load_keymap`]). The dispatcher
//! parses the Python/Textual key-string syntax, supports comma-separated
//! alternatives and two-key sequences (e.g. `search_page = "g s"`), and is
//! error-tolerant on unparseable bindings. Navigation keys (arrows, `j`/`k`,
//! Tab, Enter) have no keymap entry and are handled directly here, since they
//! are always-on and never user-rebound.

use std::io::{self, Stdout, Write};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{ExecutableCommand, cursor};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use ytmusic_api::{BrowserAuth, InnerTubeClient};
use ytmusic_tui::app::{AppCommand, AppEvent, RuntimeHandle, spawn_runtime};
use ytmusic_tui::config::{self, AppConfig};
use ytmusic_tui::keymap::{Action, Keymap, Resolution};
use ytmusic_tui::navigation::{NavigationManager, PageState as NavPage};
use ytmusic_tui::player::Player;
use ytmusic_tui::views::Theme;
use ytmusic_tui::views::album::{AlbumAction, AlbumView};
use ytmusic_tui::views::artist::{ArtistAction, ArtistView};
use ytmusic_tui::views::filter_bar::{FILTER_BAR_HEIGHT, FilterBar};
use ytmusic_tui::views::history::{HistoryAction, HistoryView};
use ytmusic_tui::views::home::{HomeAction, HomeView};
use ytmusic_tui::views::library::{LibraryAction, LibraryView};
use ytmusic_tui::views::lyrics::LyricsView;
use ytmusic_tui::views::player_bar::{PLAYER_BAR_HEIGHT, PlayerBar, PlayerBarState};
use ytmusic_tui::views::playlist::{PlaylistAction, PlaylistView};
use ytmusic_tui::views::popup::{
    ActionKind, ActionPopup, PickerChoice, PopupContext, PopupItem, PopupOutcome, PopupState,
    ThemePopup,
};
use ytmusic_tui::views::queue_view::{QueueAction, QueueView};
use ytmusic_tui::views::search::{SearchAction, SearchView};
use ytmusic_tui::views::toast::{Severity, ToastManager};

/// How long the input poll waits before the loop falls through to drain events
/// and redraw. ~60 ms keeps the player bar's 1-second-ish progress feel smooth
/// without busy-spinning. (Python relied on a 1 Hz timer; here every tick may
/// fold several progress events, so a sub-100 ms poll is plenty.)
const POLL_INTERVAL: Duration = Duration::from_millis(60);

fn main() {
    // Phase 1: everything that can fail with a clean message happens before the
    // terminal is touched. `run` returns an exit code; non-zero on a fatal
    // setup error (already reported to stderr).
    std::process::exit(run());
}

/// Load config + auth + player, spawn the runtime, run the UI, and tear down.
///
/// Returns a process exit code: `0` on a clean quit, `1` on a fatal setup or
/// terminal error. All error paths print a single human-readable line to
/// stderr; none panic.
fn run() -> i32 {
    let config = match config::load_config(None, None) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ytmusic-tui: failed to load config: {e}");
            return 1;
        }
    };

    // Auth is optional-at-runtime: a missing/!invalid browser.json must not
    // abort startup (the UI degrades to a "sign in" prompt via the session
    // canary), but a malformed file is a hard error worth reporting.
    let client = match load_client(&config) {
        Ok(client) => Some(client),
        Err(ClientLoadOutcome::Missing) => None,
        Err(ClientLoadOutcome::Fatal(msg)) => {
            eprintln!("ytmusic-tui: {msg}");
            return 1;
        }
    };

    // The player is required — without mpv there is nothing to drive.
    let mut player = match Player::new(&config.player.audio_quality) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("ytmusic-tui: failed to initialise the audio player: {e}");
            eprintln!("  (is libmpv installed and on the library path?)");
            return 1;
        }
    };
    // Apply the configured initial volume before the runtime takes ownership.
    // A failure here is non-fatal: mpv may briefly reject `volume` before any
    // audio output exists; the user can re-set it at runtime.
    let _ = player.set_volume(i64::from(config.player.volume));

    // The runtime forwarder owns the single player-event receiver.
    let player_events = player
        .take_events()
        .expect("freshly constructed Player must yield its event receiver once");

    // UI-bound event channel (runtime/forwarder → UI loop).
    let (event_tx, event_rx) = std::sync::mpsc::channel::<AppEvent>();
    let mut runtime = spawn_runtime(client, player, player_events, event_tx);

    // Kick off the initial data load and the session canary.
    runtime.send(AppCommand::FetchHome);
    runtime.send(AppCommand::CheckSession);

    // Phase 2: terminal I/O. From here on, a clean teardown is mandatory.
    let exit_code = match run_with_terminal(&config, &runtime, &event_rx) {
        Ok(()) => 0,
        Err(e) => {
            // The terminal is restored inside run_with_terminal's teardown even
            // on error; report and fall through to the runtime shutdown.
            eprintln!("ytmusic-tui: terminal error: {e}");
            1
        }
    };

    // Deterministic runtime teardown: send Quit and join the threads. The
    // player is dropped inside the runtime thread, stopping mpv.
    runtime.shutdown();
    exit_code
}

/// Outcome of trying to build the InnerTube client.
enum ClientLoadOutcome {
    /// No auth file present — start signed-out (the canary will prompt).
    Missing,
    /// A present-but-broken auth file or client build error worth reporting.
    Fatal(String),
}

/// Build the InnerTube client from the configured browser-auth path.
///
/// Tilde in the configured path is expanded here (the directive's
/// `+expand_tilde on browser_auth_path`). A missing file maps to
/// [`ClientLoadOutcome::Missing`] (signed-out start); a malformed file or a
/// client construction failure maps to [`ClientLoadOutcome::Fatal`].
fn load_client(config: &AppConfig) -> Result<InnerTubeClient, ClientLoadOutcome> {
    let path = expand_tilde(&config.auth.browser_auth_path);
    if !path.is_file() {
        return Err(ClientLoadOutcome::Missing);
    }
    let auth = BrowserAuth::load(&path).map_err(|e| {
        ClientLoadOutcome::Fatal(format!(
            "failed to load browser auth from {}: {e}\n  run: ytmusic-tui auth",
            path.display()
        ))
    })?;
    InnerTubeClient::new(auth)
        .map_err(|e| ClientLoadOutcome::Fatal(format!("failed to build API client: {e}")))
}

/// Expand a leading `~` / `~/…` in a path to `$HOME`.
///
/// A local copy of `config::expand_tilde` (which is `pub(crate)` and so not
/// visible from this separate binary crate). Only a bare `~` or a `~/` prefix
/// is expanded; everything else — including a `~` mid-string — is returned
/// unchanged. Falls back to the original text when `$HOME` is unset. config.rs
/// is outside this milestone's edit boundary, so the helper is duplicated here
/// rather than promoted to `pub`.
fn expand_tilde(path: &str) -> std::path::PathBuf {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    if path == "~" {
        if let Some(home) = home {
            return home;
        }
    } else if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = home
    {
        return home.join(rest);
    }
    std::path::PathBuf::from(path)
}

/// Enter raw mode + the alternate screen, install the panic hook, run the loop,
/// then restore the terminal unconditionally.
///
/// The panic hook restores the terminal first so a panic backtrace is readable
/// rather than scribbled over the alternate screen in raw mode.
fn run_with_terminal(
    config: &AppConfig,
    runtime: &RuntimeHandle,
    events: &std::sync::mpsc::Receiver<AppEvent>,
) -> io::Result<()> {
    install_panic_hook();
    enable_raw_mode()?;
    // From this point the terminal is in raw mode: every exit path — including
    // an Err from the alternate-screen/terminal setup below — must restore it,
    // or the user's shell is left unusable and the error text unreadable.
    let result = run_in_alternate_screen(config, runtime, events);
    restore_terminal();
    result
}

/// The fallible part of terminal setup plus the event loop, split out so
/// [`run_with_terminal`] can restore the terminal on ANY exit once raw mode is
/// active.
fn run_in_alternate_screen(
    config: &AppConfig,
    runtime: &RuntimeHandle,
    events: &std::sync::mpsc::Receiver<AppEvent>,
) -> io::Result<()> {
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    stdout.execute(cursor::Hide)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    run_loop(&mut terminal, config, runtime, events)
}

/// Best-effort terminal restoration: leave the alternate screen, show the
/// cursor, and disable raw mode. Used by both the normal teardown and the panic
/// hook, so every step is independent and ignores its own error (a half-broken
/// terminal is still better than none).
fn restore_terminal() {
    let mut stdout = io::stdout();
    let _ = stdout.execute(LeaveAlternateScreen);
    let _ = stdout.execute(cursor::Show);
    let _ = disable_raw_mode();
    let _ = stdout.flush();
}

/// Install a panic hook that restores the terminal before the default hook runs.
///
/// Without this a panic inside the loop would leave the terminal in raw mode on
/// the alternate screen, making the panic message unreadable.
fn install_panic_hook() {
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        original(info);
    }));
}

/// The synchronous render/input loop.
///
/// Each iteration: draw, poll for a key (with [`POLL_INTERVAL`] timeout), drain
/// any pending [`AppEvent`]s, and dispatch the key into an [`Action`] applied to
/// the [`AppModel`]. Exits when the model's `should_quit` flag is set.
fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    config: &AppConfig,
    runtime: &RuntimeHandle,
    events: &std::sync::mpsc::Receiver<AppEvent>,
) -> io::Result<()> {
    // Build the keymap dispatcher from the merged keymap.toml. A load error
    // (malformed user file) falls back to the compiled-in defaults rather than
    // aborting the UI — the user can still drive the app and fix the file.
    // Unparseable *bindings* (a typo in an otherwise-valid file) are collected as
    // warnings and surfaced once on the status line below.
    let (keymap, keymap_warnings) = match config::load_keymap(None, None) {
        Ok(mut map) => {
            // `search_page` ships unbound in the keymap files; grant it the
            // `g s` sequence default now that the dispatcher supports sequences
            // (directive §6). A user binding in keymap.toml still overrides it.
            map.entry("search_page".to_owned())
                .or_insert_with(|| "g s".to_owned());
            Keymap::from_map_with_warnings(&map)
        }
        Err(_) => (Keymap::defaults(), Vec::new()),
    };
    let mut model = AppModel::with_keymap(Theme::from_name(&config.ui.theme), keymap);
    model.set_theme_name(&config.ui.theme);
    // Surface dropped keymap bindings once, on the status line.
    model.set_keymap_warnings(&keymap_warnings);

    while !model.should_quit {
        // Drain events that arrived since the last tick before drawing, so the
        // frame reflects the latest player position / loaded data.
        drain_events(events, &mut model, runtime);

        // Keep the active view's filter in sync with the bar before drawing
        // (a re-fetch may have replaced the view's data this tick).
        model.apply_filter_to_view();

        // Expire timed-out toasts before drawing so they vanish on schedule even
        // when no key is pressed (the ~60 ms poll drives this).
        model.prune_toasts();

        terminal.draw(|frame| model.render(frame))?;

        // Block up to POLL_INTERVAL for input; on timeout, loop to redraw
        // (the player bar advances from drained progress events).
        if event::poll(POLL_INTERVAL)?
            && let Event::Key(key) = event::read()?
        {
            // Only react to presses (Windows also emits Release/Repeat).
            if key.kind == KeyEventKind::Press {
                model.dispatch_key(key, runtime);
            }
        }
    }
    Ok(())
}

/// Drain all currently-available events into the model (non-blocking),
/// dispatching any follow-up command the fold returns (today: the auto-advance
/// `NextTrack` after a natural `TrackEnded`).
fn drain_events(
    events: &std::sync::mpsc::Receiver<AppEvent>,
    model: &mut AppModel,
    runtime: &RuntimeHandle,
) {
    while let Ok(event) = events.try_recv() {
        if let Some(command) = model.on_event(event) {
            runtime.send(command);
        }
    }
}

// ---------------------------------------------------------------------------
// Action mapping (pure — unit-tested without a terminal)
// ---------------------------------------------------------------------------

/// An always-on navigation action with no keymap binding.
///
/// These keys (arrows, `j`/`k`, Tab/Shift-Tab, Enter) are never user-rebound —
/// they drive the per-view cursor and section focus directly, so they bypass
/// the [`Keymap`] dispatcher. Keeping them as a small typed enum (rather than
/// acting on the raw key) preserves the testable, pure dispatch the M5b code
/// had.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NavAction {
    /// Move to the next section (`Tab`): home sections, search/library panes.
    NextSection,
    /// Move to the previous section (`Shift+Tab` / `BackTab`).
    PreviousSection,
    /// Move the selection down (`Down` / `j`).
    SelectNext,
    /// Move the selection up (`Up` / `k`).
    SelectPrevious,
    /// Activate the current selection (`Enter`).
    Activate,
}

/// Per-press volume step, matching the Python `volume_up` / `volume_down`
/// actions which nudged by 5.
const VOLUME_STEP: i64 = 5;

/// Decode a navigation key (arrows / `j` / `k` / Tab / Enter) into a
/// [`NavAction`], or `None` if it is not a navigation key.
///
/// These keys have no keymap entry; they are handled before the [`Keymap`]
/// dispatcher in [`AppModel::dispatch_key`]. `j`/`k` only act as navigation
/// because no default keymap binding claims them (the dispatcher is consulted
/// first for everything else).
fn map_nav_key(key: KeyEvent) -> Option<NavAction> {
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => Some(NavAction::SelectNext),
        KeyCode::Char('k') | KeyCode::Up => Some(NavAction::SelectPrevious),
        // crossterm reports Shift+Tab as BackTab; also accept Tab+SHIFT.
        KeyCode::BackTab => Some(NavAction::PreviousSection),
        KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => {
            Some(NavAction::PreviousSection)
        }
        KeyCode::Tab => Some(NavAction::NextSection),
        KeyCode::Enter => Some(NavAction::Activate),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// AppModel — view state + event folding + action application (pure-ish)
// ---------------------------------------------------------------------------

/// Which content view is active. The home, playlist, search, library, album,
/// artist, lyrics, history, and queue views each keep their own state; this
/// enum records which one renders and receives navigation keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    /// The home recommendations view (the startup view).
    Home,
    /// The two-level playlist browser.
    Playlist,
    /// The 4-pane search view (Tracks/Albums/Artists/Playlists).
    Search,
    /// The 3-pane library view (Playlists/Albums/Artists).
    Library,
    /// The album detail view (header + track list).
    Album,
    /// The artist page (top songs / albums / related artists).
    Artist,
    /// Scrollable lyrics for the current track.
    Lyrics,
    /// Recently-played track list.
    History,
    /// The current playback queue with position highlight.
    Queue,
}

/// Which surface a `FetchPlaylistTracks` request was issued from.
///
/// Both the standalone playlist view and the library view's Playlists pane drill
/// into a playlist's tracks via the same [`AppCommand::FetchPlaylistTracks`] /
/// [`AppEvent::PlaylistTracksLoaded`] round-trip. The reply carries only the
/// echoed `title`, not the requester, so the model records *which* surface asked
/// when it issues the fetch and routes the reply by this token — replacing the
/// M5b view-state heuristic that mis-routed when both were in flight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingTracksFetch {
    /// No track fetch is in flight.
    None,
    /// The standalone playlist view requested the tracks.
    Playlist,
    /// The library view's Playlists pane requested the tracks.
    Library,
}

/// Map a navigation page type (the `page_type` of a [`NavPage`]) to its content
/// [`View`]. Kept pure so the page→view mapping is unit-testable without a
/// `RuntimeHandle`. Unknown pages fall back to [`View::Home`].
fn view_for_page(page_type: &str) -> View {
    match page_type {
        "playlist" => View::Playlist,
        "search" => View::Search,
        "library" => View::Library,
        "album" => View::Album,
        "artist" => View::Artist,
        "lyrics" => View::Lyrics,
        "history" => View::History,
        "queue" => View::Queue,
        _ => View::Home,
    }
}

/// The navigation `page_type` string for a [`View`] — the inverse of
/// [`view_for_page`] for the top-level views pushed by [`AppModel::switch_view`].
/// Kept pure so the round-trip is unit-testable.
fn page_type_for_view(view: View) -> &'static str {
    match view {
        View::Home => "home",
        View::Playlist => "playlist",
        View::Search => "search",
        View::Library => "library",
        View::Album => "album",
        View::Artist => "artist",
        View::Lyrics => "lyrics",
        View::History => "history",
        View::Queue => "queue",
    }
}

/// The full UI state for the TUI.
///
/// Owns all view widgets' state, the navigation stack, and the quit flag.
/// Methods are split so the pure parts (event folding, navigation/key
/// application) are unit-testable; only [`AppModel::apply`] for playback actions
/// needs a `RuntimeHandle`, which tests exercise via a `None` runtime path where
/// it only mutates view state.
struct AppModel {
    home: HomeView,
    playlist: PlaylistView,
    search: SearchView,
    library: LibraryView,
    album: AlbumView,
    artist: ArtistView,
    lyrics: LyricsView,
    history: HistoryView,
    queue_view: QueueView,
    player: PlayerBarState,
    /// The `video_id` of the currently playing track, updated from
    /// [`AppEvent::NowPlaying`]. Stored here rather than in [`PlayerBarState`]
    /// because the bar view has no need for it; it is only used to fetch lyrics.
    current_video_id: Option<String>,
    /// The page history stack; Esc pops back.
    nav: NavigationManager,
    /// The keymap dispatcher: resolves key events to [`Action`]s via the merged
    /// `keymap.toml`, with two-key sequence support (e.g. `g s` → search page).
    keymap: Keymap,
    /// The `/`-toggled in-page filter bar. When active, printable keys build the
    /// query and the current (filterable) view shows only matching rows.
    filter: FilterBar,
    /// The active popup overlay, if any (action / theme / playlist picker). When
    /// open, keys route to it before the view or keymap (`PopupState::None`
    /// otherwise).
    popups: PopupState,
    /// The currently configured theme name (for the theme popup's "current"
    /// marker and live re-application).
    theme_name: String,
    /// A cache of the user's library playlists `(id, title)`, kept fresh from
    /// every `LibraryPlaylistsLoaded` event. Feeds the playlist-picker popup
    /// independently of the playlist view's level, so opening "Add to playlist"
    /// from a track row never disturbs the active view.
    library_playlists: Vec<(String, String)>,
    /// Which surface a `FetchPlaylistTracks` reply should be routed to. Set when
    /// the fetch is issued, consumed (reset to `None`) when the
    /// [`AppEvent::PlaylistTracksLoaded`] lands.
    ///
    /// Known limitation: a single scalar token means TWO in-flight fetches
    /// (playlist drill, then library drill before the first reply) cross-route
    /// both replies. Last-issued-wins covers the realistic single-flight case;
    /// a per-request id would be the full fix if this ever bites.
    pending_tracks: PendingTracksFetch,
    /// Floating bottom-right toasts: every transient notification (action
    /// confirmations, API/playback errors, the audio-quality change, the
    /// session/MPRIS warnings) is pushed here and auto-expires, replacing the
    /// old persistent single status line (Python's `App.notify()` toasts).
    toasts: ToastManager,
    /// The active content view.
    view: View,
    /// Whether the search input box currently has keyboard focus. While `true`,
    /// printable keys append to the query, Backspace deletes, Enter submits, and
    /// Esc leaves input focus (mirrors Textual's `Input` swallowing keys). Only
    /// meaningful while [`View::Search`] is active.
    input_mode: bool,
    theme: Theme,
    should_quit: bool,
}

/// The warning shown when the session canary fails.
const SESSION_WARNING: &str = "Session invalid — desktop sign-in expired. Run: ytmusic-tui auth";

impl AppModel {
    /// Build a model with the default keymap (the test/convenience path).
    ///
    /// The non-test binary always builds via [`AppModel::with_keymap`] (fed the
    /// merged user `keymap.toml`), so this default-keymap constructor is only
    /// used by the unit tests.
    #[cfg(test)]
    fn new(theme: Theme) -> Self {
        Self::with_keymap(theme, Keymap::defaults())
    }

    /// Record the configured theme name (the runtime path sets this so the theme
    /// popup can mark the current theme and re-apply by name).
    fn set_theme_name(&mut self, name: impl Into<String>) {
        self.theme_name = name.into();
    }

    /// Surface any dropped/unparseable keymap bindings as a startup warning
    /// toast (directive §6). A non-empty list becomes a single message like
    /// `2 keymap bindings ignored: quit = "notakey", next = "??"`. An empty list
    /// is a no-op. The toast auto-expires (warning timeout), matching the "show
    /// once" intent — Python notified the same way.
    fn set_keymap_warnings(&mut self, warnings: &[String]) {
        if warnings.is_empty() {
            return;
        }
        let message = format!(
            "{} keymap binding{} ignored: {}",
            warnings.len(),
            if warnings.len() == 1 { "" } else { "s" },
            warnings.join(", ")
        );
        self.push_toast(message, Severity::Warning);
    }

    /// Build a model with an explicit keymap (the runtime path, fed the merged
    /// user `keymap.toml`).
    fn with_keymap(theme: Theme, keymap: Keymap) -> Self {
        Self {
            home: HomeView::new(),
            playlist: PlaylistView::new(),
            search: SearchView::new(),
            library: LibraryView::new(),
            album: AlbumView::new(),
            artist: ArtistView::new(),
            lyrics: LyricsView::new(),
            history: HistoryView::new(),
            queue_view: QueueView::new(),
            player: PlayerBarState::default(),
            current_video_id: None,
            nav: NavigationManager::new(NavPage::new("home")),
            keymap,
            filter: FilterBar::new(),
            popups: PopupState::None,
            // Defaults to synthwave; the runtime path overwrites this with the
            // configured theme name via `set_theme_name` after construction.
            theme_name: "synthwave".to_owned(),
            library_playlists: Vec::new(),
            pending_tracks: PendingTracksFetch::None,
            toasts: ToastManager::new(),
            view: View::Home,
            input_mode: false,
            theme,
            should_quit: false,
        }
    }

    /// Push a transient toast with its severity's default timeout.
    ///
    /// The single choke point for every notification so the audit stays
    /// reviewable in one place. Uses the real clock (`Instant::now`) as the toast
    /// "now"; the prune in the main loop reads the same clock.
    fn push_toast(&mut self, message: impl Into<String>, severity: Severity) {
        self.toasts
            .push(message, severity, std::time::Instant::now());
    }

    /// Push a toast with an explicit timeout (the two call sites that overrode
    /// Textual's default: the 4 s filter warning and the 10 s session warning).
    fn push_toast_with_timeout(
        &mut self,
        message: impl Into<String>,
        severity: Severity,
        timeout: Duration,
    ) {
        self.toasts
            .push_with_timeout(message, severity, timeout, std::time::Instant::now());
    }

    /// Fold one runtime event into the model, returning any follow-up command
    /// the loop should send back to the runtime.
    ///
    /// The only follow-up today is the auto-advance: a [`AppEvent::TrackEnded`]
    /// (a *natural* EOF) returns [`AppCommand::NextTrack`], which drives the
    /// runtime-side queue forward. A [`AppEvent::TrackError`] returns `None` —
    /// the queue must NEVER advance on a failed stream (the end-file battle
    /// lesson; asserted in the tests). Returning the command (rather than
    /// sending it here) keeps the fold pure and unit-testable without a runtime.
    #[must_use]
    fn on_event(&mut self, event: AppEvent) -> Option<AppCommand> {
        match event {
            AppEvent::HomeLoaded(sections) => {
                self.home.set_sections(sections);
            }
            AppEvent::LibraryPlaylistsLoaded(playlists) => {
                // The library playlist list feeds three consumers: the playlist
                // view's level-1 list, the library view's playlists pane, and
                // the picker cache. Cache the (id, title) pairs first so the
                // playlist-picker popup has data regardless of view state.
                self.library_playlists = playlists
                    .iter()
                    .map(|p| (p.playlist_id.clone(), p.title.clone()))
                    .collect();
                // Only feed the playlist view's level-1 list when it is NOT
                // drilled into a track list: a fetch primed to refresh the
                // picker cache (e.g. opening an action popup over a track) must
                // not yank the view back to level 1.
                if !self.playlist.is_viewing_tracks() {
                    self.playlist.set_playlists(playlists.clone());
                }
                self.library.set_playlists(playlists);
            }
            AppEvent::PlaylistTracksLoaded { title, tracks } => {
                // The same event serves two drill-in surfaces: the standalone
                // playlist view and the library view's Playlists pane. Route by
                // the typed pending-fetch token recorded when the fetch was
                // issued, so concurrent in-flight fetches never cross-route
                // (replaces the M5b view-state heuristic). A `None` token (no
                // recorded requester) falls back to the playlist view, the
                // common drill-in case.
                match self.pending_tracks {
                    PendingTracksFetch::Library => self.library.set_tracks(title, tracks),
                    PendingTracksFetch::Playlist | PendingTracksFetch::None => {
                        self.playlist.set_tracks(title, tracks);
                    }
                }
                self.pending_tracks = PendingTracksFetch::None;
            }
            AppEvent::SearchLoaded(results) => {
                self.search.set_results(results);
            }
            AppEvent::LibraryAlbumsLoaded(albums) => {
                self.library.set_albums(albums);
            }
            AppEvent::LibraryArtistsLoaded(artists) => {
                self.library.set_artists(artists);
            }
            AppEvent::LikedSongsLoaded(tracks) => {
                self.library.set_liked_songs(tracks);
            }
            AppEvent::ApiError(msg) => self.on_api_error(msg),
            AppEvent::NowPlaying(now) => {
                // Keep the video_id available for the lyrics fetch trigger.
                self.current_video_id = if now.video_id.is_empty() {
                    None
                } else {
                    Some(now.video_id.clone())
                };
                self.player.on_now_playing(
                    now.title,
                    now.artist,
                    now.album,
                    now.duration_seconds,
                    now.shuffle,
                    now.repeat.into(),
                );
                // When the queue view is visible, re-snapshot so the ▶ marker
                // stays accurate as tracks advance (FetchQueue is cheap — the
                // runtime replies instantly with the current queue state).
                if self.view == View::Queue {
                    return Some(AppCommand::FetchQueue);
                }
            }
            AppEvent::PlayerProgress(secs) => self.player.on_progress(secs),
            AppEvent::PlayerDuration(secs) => self.player.on_duration(secs),
            AppEvent::PlayerVolume(vol) => self.player.on_volume(vol),
            AppEvent::PlayerMute(muted) => self.player.on_mute(muted),
            AppEvent::AudioQualityChanged(quality) => {
                // Toast the new level; it applies from the next track. This is
                // the `b` message that previously stuck forever on the status
                // line — now it auto-expires like every other notification.
                self.push_toast(
                    format!("Audio quality: {quality} (from next track)"),
                    Severity::Info,
                );
            }
            AppEvent::ActionResult(message) => {
                // A like/add-to-queue/add-to-playlist confirmation toast.
                self.push_toast(message, Severity::Info);
            }
            AppEvent::PlayerStarted => self.player.on_started(),
            AppEvent::TrackEnded => {
                // Natural EOF: clear the bar to idle now, and ask the runtime to
                // advance the queue. The runtime replies with NowPlaying +
                // PlayerStarted for the next track (or an idle NowPlaying at the
                // end of the queue) on the following ticks.
                self.current_video_id = None;
                self.player.on_track_ended();
                return Some(AppCommand::NextTrack);
            }
            AppEvent::TrackError(detail) => {
                // NEVER advance on a failed stream — a broken resolver would
                // machine-gun the queue (the end-file battle lesson). Surface the
                // failure as an error toast (Python's 8 s playback-error toast).
                self.push_toast(format!("Playback error: {detail}"), Severity::Error);
            }
            AppEvent::ArtistResolved(channel_id) => {
                // The `a` / "Go to artist" lookup resolved a channel id: navigate
                // to the artist page (reusing open_artist's view/nav prep) and
                // chain the fetch as the fold's follow-up command.
                return Some(self.prepare_open_artist(&channel_id));
            }
            AppEvent::AlbumResolved(browse_id) => {
                return Some(self.prepare_open_album(&browse_id));
            }
            AppEvent::AlbumLoaded(album) => {
                self.album.set_album(album);
            }
            AppEvent::ArtistLoaded(artist) => {
                self.artist.set_artist(artist);
            }
            AppEvent::LyricsLoaded(lyrics) => {
                self.lyrics.set_lyrics(lyrics);
            }
            AppEvent::HistoryLoaded(tracks) => {
                self.history.set_tracks(tracks);
            }
            AppEvent::QueueSnapshot(snapshot) => {
                self.queue_view.set_snapshot(snapshot);
            }
            AppEvent::SessionInvalid => {
                // Python surfaced the signed-out canary as a 10 s warning toast
                // (`_probe_session`, timeout=10). Mirror that here rather than a
                // persistent header line.
                self.push_toast_with_timeout(
                    SESSION_WARNING,
                    Severity::Warning,
                    Duration::from_secs(10),
                );
            }
        }
        None
    }

    /// Surface an API error: always show it on the status line, and replace a
    /// stuck "Loading…" body so a failed fetch never leaves a view spinning
    /// forever.
    ///
    /// `AppEvent::ApiError` carries no source tag (M5b keeps the event flat), so
    /// the error is applied defensively: the home view is updated whenever it is
    /// still unloaded — this covers a `FetchHome` error that lands *after* the
    /// user has already navigated to the playlist view, which would otherwise
    /// leave home stuck on "Loading…" with no retry. The playlist view is
    /// updated only while it is the active view, since a playlist fetch is only
    /// ever in flight from there. (M5b-2 may add a source tag if a second
    /// background fetch surface makes this ambiguous.)
    fn on_api_error(&mut self, msg: String) {
        if self.home.state().loaded().is_none() {
            self.home.set_error(msg.clone());
        }
        // The remaining views are updated only while active, since their
        // respective fetches are only ever in flight from their own view.
        match self.view {
            View::Playlist => self.playlist.set_error(msg.clone()),
            View::Search => self.search.set_error(msg.clone()),
            View::Library => self.library.set_error(msg.clone()),
            View::Album => self.album.set_error(msg.clone()),
            View::Artist => self.artist.set_error(msg.clone()),
            View::Lyrics => self.lyrics.set_error(msg.clone()),
            View::History => self.history.set_error(msg.clone()),
            View::Home | View::Queue => {}
        }
        // Also surface the failure as an error toast so it is visible even when
        // the active view shows no inline error body (e.g. Queue), matching
        // Python's `classify_api_error` + `notify(..., severity="error")`.
        self.push_toast(msg, Severity::Error);
    }

    /// Dispatch a raw key press: route to the search input editor while the
    /// input is focused, otherwise decode it into an [`Action`] and apply it.
    ///
    /// This is the single key entry point so the input-mode-vs-action-mode
    /// decision lives in one tested place (Textual achieved the same by letting
    /// the focused `Input` widget swallow keys before the app bindings ran).
    fn dispatch_key(&mut self, key: KeyEvent, runtime: &RuntimeHandle) {
        if self.is_input_active() {
            self.handle_input_key(key, runtime);
            return;
        }

        // Ctrl+C always quits, regardless of the keymap (raw mode disables the
        // SIGINT that would otherwise arrive). Checked even while a popup or the
        // filter bar is open so the app is never trapped.
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.should_quit = true;
            return;
        }

        // A popup, when open, captures all keys until it is dismissed or makes a
        // selection (Esc / Enter / j / k). It takes priority over the filter bar
        // and the keymap.
        if self.popups.is_open() {
            let outcome = self.popups.on_key(key.code);
            self.handle_popup_outcome(outcome, runtime);
            return;
        }

        // The in-page filter bar, when open, captures typing (printable chars +
        // Backspace build the query; Esc closes). Arrow keys and Enter still
        // navigate / activate the filtered rows so the user can type-then-play.
        if self.filter.is_active() && self.handle_filter_key(key, runtime) {
            return;
        }

        // Navigation keys (arrows / j / k / Tab / Enter) are always-on and have
        // no keymap binding; handle them before consulting the dispatcher. While
        // a sequence prefix is armed, a nav key cancels it (the prefix is not a
        // navigation modifier).
        if let Some(nav) = map_nav_key(key) {
            self.keymap.clear_pending();
            self.apply_nav(nav, runtime);
            return;
        }

        // Everything else goes through the keymap dispatcher (which handles
        // sequence prefixes and user re-bindings).
        match self.keymap.resolve(key) {
            Resolution::Action(action) => self.apply(action, runtime),
            Resolution::Pending | Resolution::None => {}
        }
    }

    /// Handle a key while the in-page filter bar is open.
    ///
    /// Returns `true` when the key was consumed by the bar (typing / Backspace /
    /// Esc), `false` to let it fall through to the normal navigation handling
    /// (arrows and Enter, so the user can move within and activate the filtered
    /// rows without closing the bar).
    ///
    /// Esc closes the bar and clears the view's filter. Any typing immediately
    /// re-applies the query to the view so the visible rows update live.
    fn handle_filter_key(&mut self, key: KeyEvent, _runtime: &RuntimeHandle) -> bool {
        match key.code {
            KeyCode::Esc => {
                self.filter.hide();
                self.clear_view_filter();
                true
            }
            KeyCode::Backspace => {
                self.filter.backspace();
                self.apply_filter_to_view();
                true
            }
            // Arrows + Enter fall through to nav/activate over the filtered rows.
            KeyCode::Up | KeyCode::Down | KeyCode::Enter => false,
            // Printable characters build the query (CONTROL combos are not text).
            KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.filter.push_char(ch);
                self.apply_filter_to_view();
                true
            }
            // Everything else is ignored while filtering (kept in the bar).
            _ => true,
        }
    }

    /// Whether the search input is currently capturing keystrokes.
    fn is_input_active(&self) -> bool {
        self.input_mode && self.view == View::Search
    }

    /// Handle a key while the search input is focused: printable chars append,
    /// Backspace deletes, Enter submits the search, Esc leaves input focus, and
    /// any other key is ignored (kept in the input box, matching Textual's
    /// `Input` which ignores unhandled keys).
    ///
    /// Submitting fires the runtime [`AppCommand::Search`] (with the parsed
    /// `#category:` filter) and shows the loading state; the input box keeps its
    /// text and focus so a refined re-search is one edit away.
    fn handle_input_key(&mut self, key: KeyEvent, runtime: &RuntimeHandle) {
        match key.code {
            KeyCode::Enter => {
                if let Some((query, filter)) = self.search.submit_query() {
                    self.search.set_loading();
                    runtime.send(AppCommand::Search { query, filter });
                }
            }
            KeyCode::Backspace => self.search.backspace_input(),
            KeyCode::Esc => self.input_mode = false,
            // Ctrl+C quits even while typing: raw mode disables ISIG so no
            // SIGINT arrives — without this arm the only exit is Esc-then-q.
            // Plain q/Q stay typeable (a search for "queen" must work).
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            // Accept printable characters only; control chars (Tab, arrows, F-keys
            // come through as non-Char codes) are ignored while typing.
            KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.search.push_input_char(ch);
            }
            _ => {}
        }
    }

    /// Apply a navigation action (arrows / `j` / `k` / Tab / Enter).
    ///
    /// Tab / Shift-Tab move between "sections": home sections, search panes,
    /// library panes, and artist sections. Single-list views are inert
    /// (Python's PlaylistView / QueueView / HistoryView ignored Tab).
    fn apply_nav(&mut self, nav: NavAction, runtime: &RuntimeHandle) {
        match nav {
            NavAction::NextSection => match self.view {
                View::Home => self.home.focus_next_section(),
                View::Search => self.search.focus_next_pane(),
                View::Library => self.library.focus_next_pane(),
                View::Artist => self.artist.focus_next_section(),
                View::Playlist | View::Album | View::Lyrics | View::History | View::Queue => {}
            },
            NavAction::PreviousSection => match self.view {
                View::Home => self.home.focus_previous_section(),
                View::Search => self.search.focus_previous_pane(),
                View::Library => self.library.focus_previous_pane(),
                View::Artist => self.artist.focus_previous_section(),
                View::Playlist | View::Album | View::Lyrics | View::History | View::Queue => {}
            },
            NavAction::SelectNext => self.select_next(),
            NavAction::SelectPrevious => self.select_previous(),
            NavAction::Activate => self.activate(runtime),
        }
    }

    /// Apply a keymap-decoded [`Action`], issuing runtime commands for
    /// playback/navigation actions and mutating view state for the rest.
    fn apply(&mut self, action: Action, runtime: &RuntimeHandle) {
        match action {
            Action::Quit => self.should_quit = true,
            Action::TogglePause => {
                runtime.send(AppCommand::TogglePause);
            }
            Action::VolumeUp => {
                // Optimistic update for responsiveness; the player's volume
                // observation (PlayerVolume) corrects any clamp drift.
                self.player.volume = (self.player.volume + VOLUME_STEP).clamp(0, 100);
                runtime.send(AppCommand::AdjustVolume(VOLUME_STEP));
            }
            Action::VolumeDown => {
                self.player.volume = (self.player.volume - VOLUME_STEP).clamp(0, 100);
                runtime.send(AppCommand::AdjustVolume(-VOLUME_STEP));
            }
            Action::NextTrack => {
                runtime.send(AppCommand::NextTrack);
            }
            Action::PreviousTrack => {
                runtime.send(AppCommand::PreviousTrack);
            }
            Action::ToggleShuffle => {
                runtime.send(AppCommand::ToggleShuffle);
            }
            Action::CycleRepeat => {
                runtime.send(AppCommand::CycleRepeat);
            }
            Action::CycleAudioQuality => {
                runtime.send(AppCommand::CycleAudioQuality);
            }
            Action::SeekForward => {
                runtime.send(AppCommand::SeekForward);
            }
            Action::SeekBackward => {
                runtime.send(AppCommand::SeekBackward);
            }
            Action::SeekToStart => {
                runtime.send(AppCommand::SeekToStart);
            }
            Action::ToggleMute => {
                runtime.send(AppCommand::ToggleMute);
            }
            Action::ToggleLike => self.toggle_like_current(runtime),
            Action::StartRadio => self.start_radio_current(runtime),
            Action::OpenCurrentArtist => self.open_current_artist(runtime),
            Action::OpenCurrentAlbum => self.open_current_album(runtime),
            Action::GoBack => self.go_back(runtime),
            Action::ToggleFilter => self.toggle_filter(),
            Action::SwitchHome => self.switch_view(View::Home, runtime),
            Action::SearchPage => self.switch_view(View::Search, runtime),
            Action::SwitchLibrary => self.switch_view(View::Library, runtime),
            Action::SwitchQueue => self.switch_view(View::Queue, runtime),
            Action::SwitchHistory => self.switch_view(View::History, runtime),
            Action::SwitchLyrics => self.switch_view(View::Lyrics, runtime),
            Action::OpenActionPopup => self.open_action_popup(runtime),
            Action::OpenThemePopup => self.open_theme_popup(),
        }
    }

    /// Open the context-action popup for the current view's selected item.
    ///
    /// If the view exposes no selectable item (e.g. lyrics, or an empty list) the
    /// popup is not opened and a short note is shown, mirroring Python's
    /// `get_focused_item() is None` early return. The playlist picker needs the
    /// library playlists, so a `FetchLibraryPlaylists` is primed here so the
    /// later "Add to playlist" choice has data.
    fn open_action_popup(&mut self, runtime: &RuntimeHandle) {
        let Some(item) = self.selected_popup_item() else {
            self.push_toast("Nothing to act on here", Severity::Warning);
            return;
        };
        // Prime the playlist list so a subsequent "Add to playlist" can populate
        // the picker without a visible wait.
        runtime.send(AppCommand::FetchLibraryPlaylists);
        self.close_filter();
        let context = self.popup_context();
        self.popups = PopupState::Action(ActionPopup::with_context(item, context));
    }

    /// The action-popup context for the active view (Python's `context` arg to
    /// `ActionPopup.show`): the queue view yields the queue track list (with
    /// "Remove from queue"), the playlist view drilled into its tracks yields the
    /// playlist-track list (with "Remove from playlist"), and everything else is
    /// the plain track list.
    fn popup_context(&self) -> PopupContext {
        match self.view {
            View::Queue => PopupContext::Queue,
            View::Playlist if self.playlist.is_viewing_tracks() => PopupContext::PlaylistTracks,
            _ => PopupContext::Plain,
        }
    }

    /// Open the theme-picker popup, seeded with the built-in theme names and the
    /// currently active theme.
    fn open_theme_popup(&mut self) {
        self.close_filter();
        let names: Vec<String> = config::themes().keys().map(|k| (*k).to_owned()).collect();
        // Sort for a stable, predictable order (HashMap iteration is arbitrary).
        let mut names = names;
        names.sort();
        self.popups = PopupState::Theme(ThemePopup::new(names, &self.theme_name));
    }

    /// The selectable item under the cursor in the active view, as a
    /// [`PopupItem`] for the action popup. `None` when the view has no selectable
    /// item (lyrics, empty lists, or the multi-pane views that the action popup
    /// does not target here).
    fn selected_popup_item(&self) -> Option<PopupItem> {
        match self.view {
            View::Home => self.home.selected_popup_item(),
            View::Playlist => self.playlist.selected_popup_item(),
            View::Search => self.search.selected_popup_item(),
            View::Library => self.library.selected_popup_item(),
            View::Album => self.album.selected_popup_item(),
            View::Artist => self.artist.selected_popup_item(),
            View::History => self.history.selected_popup_item(),
            View::Queue => self.queue_view.selected_popup_item(),
            View::Lyrics => None,
        }
    }

    /// React to a popup selection / dismissal.
    fn handle_popup_outcome(&mut self, outcome: PopupOutcome, runtime: &RuntimeHandle) {
        match outcome {
            PopupOutcome::Consumed | PopupOutcome::Dismissed => {}
            PopupOutcome::ThemeSelected(name) => {
                // Apply the theme live (Python re-applied via set_css_variables).
                self.theme = Theme::from_name(&name);
                self.theme_name = name.clone();
                self.push_toast(format!("Theme: {name}"), Severity::Info);
            }
            PopupOutcome::ActionSelected { action, item } => {
                self.handle_action(action.kind, item, runtime);
            }
            PopupOutcome::PlaylistChosen { choice, track } => match choice {
                PickerChoice::NewPlaylist { name } => {
                    // The picker's name-entry prompt produced a title; create the
                    // playlist and add the track to it (Python's create-then-add).
                    runtime.send(AppCommand::CreatePlaylistAndAdd {
                        title: name,
                        video_id: track.video_id,
                    });
                }
                PickerChoice::Existing(playlist_id) => {
                    runtime.send(AppCommand::AddToPlaylist {
                        playlist_id,
                        video_id: track.video_id,
                    });
                }
            },
        }
    }

    /// Dispatch a selected context action against its item.
    ///
    /// Play / queue / radio / like and the open-artist/album navigations are
    /// wired to real commands; "Add to playlist" opens the playlist picker
    /// (seeded from the library playlists primed when the action popup opened).
    fn handle_action(&mut self, kind: ActionKind, item: PopupItem, runtime: &RuntimeHandle) {
        match (kind, item) {
            (ActionKind::Play, PopupItem::Track(track)) => {
                self.player.on_started();
                runtime.send(AppCommand::Play(track));
            }
            (ActionKind::AddToQueue, PopupItem::Track(track)) => {
                runtime.send(AppCommand::AddToQueue(track));
            }
            (ActionKind::StartRadio, PopupItem::Track(track)) => {
                runtime.send(AppCommand::StartRadio(track.video_id));
            }
            (ActionKind::ToggleLike, PopupItem::Track(track)) => {
                runtime.send(AppCommand::ToggleLike(track.video_id));
            }
            (ActionKind::AddToPlaylist, PopupItem::Track(track)) => {
                self.open_playlist_picker(track);
            }
            (ActionKind::RemoveFromQueue, PopupItem::Track(track)) => {
                runtime.send(AppCommand::RemoveFromQueue(track.video_id));
            }
            (ActionKind::RemoveFromPlaylist, PopupItem::Track(track)) => {
                self.remove_from_playlist(track, runtime);
            }
            (ActionKind::GoToArtist, item) => self.action_go_to_artist(&item, runtime),
            (ActionKind::GoToAlbum, item) => self.action_go_to_album(&item, runtime),
            (ActionKind::PlayAll, PopupItem::Playlist(info)) => self.open_playlist(info, runtime),
            (ActionKind::Open, PopupItem::Playlist(info)) => self.open_playlist(info, runtime),
            (ActionKind::PlayAll | ActionKind::Open, PopupItem::Album(album)) => {
                self.open_album(album.browse_id, runtime);
            }
            // Combinations that don't apply to the item type are no-ops.
            _ => {}
        }
    }

    /// Remove `track` from the playlist currently shown in the playlist view's
    /// track list (the "Remove from playlist" action).
    ///
    /// The playlist id is read from the nav stack's current page context — when
    /// the playlist view is drilled into its tracks the current page is
    /// `("playlist", playlist_id = …)`. Python read it from
    /// `PlaylistView.current_playlist_id`; the Rust playlist view's `Tracks`
    /// level stores only the title, so the id comes from the nav entry that
    /// drilled in (set by `open_playlist` / `activate_playlist`). A missing id
    /// (somehow not on a playlist page) reports the limitation rather than
    /// guessing.
    fn remove_from_playlist(&mut self, track: ytmusic_api::Track, runtime: &RuntimeHandle) {
        let playlist_id = self
            .nav
            .current()
            .context
            .get("playlist_id")
            .filter(|id| !id.is_empty())
            .cloned();
        match playlist_id {
            Some(playlist_id) => {
                runtime.send(AppCommand::RemoveFromPlaylist {
                    playlist_id,
                    video_id: track.video_id,
                });
            }
            None => {
                self.push_toast(
                    "Remove from playlist: no playlist context",
                    Severity::Warning,
                );
            }
        }
    }

    /// Open the playlist picker for `track`, seeded from the cached library
    /// playlists (primed when the action popup opened). When the cache is empty
    /// the picker still offers "New playlist…".
    fn open_playlist_picker(&mut self, track: ytmusic_api::Track) {
        self.popups =
            PopupState::PlaylistPicker(ytmusic_tui::views::popup::PlaylistPickerPopup::new(
                self.library_playlists.clone(),
                track,
            ));
    }

    /// Navigate to the artist of a popup item.
    ///
    /// An [`PopupItem::Album`] row carries the album's artist *name* (the Rust
    /// `Track`/`AlbumInfo` models do not carry an artist channel id), so this
    /// resolves the target by search — the same `_lookup_and_open_artist` flow
    /// the `a` key uses. A track row likewise resolves by its artist name. A
    /// row with no artist name reports the limitation.
    fn action_go_to_artist(&mut self, item: &PopupItem, runtime: &RuntimeHandle) {
        let name = match item {
            PopupItem::Track(t) => t.artist.clone(),
            PopupItem::Album(a) => a.artist.clone(),
            PopupItem::Playlist(_) => String::new(),
        };
        if name.is_empty() {
            self.push_toast("Go to artist: unavailable from this row", Severity::Warning);
            return;
        }
        self.push_toast(format!("Finding artist: {name}…"), Severity::Info);
        runtime.send(AppCommand::SearchAndOpenArtist(name));
    }

    /// Navigate to the album of a popup item. An [`PopupItem::Album`] row carries
    /// a usable browse id directly; a track row carries only the album *name*, so
    /// it resolves by search (`_lookup_and_open_album`), the same flow the `A`
    /// key uses. A row with no album reports the limitation.
    fn action_go_to_album(&mut self, item: &PopupItem, runtime: &RuntimeHandle) {
        match item {
            PopupItem::Album(a) if !a.browse_id.is_empty() => {
                self.open_album(a.browse_id.clone(), runtime);
            }
            PopupItem::Track(t) if !t.album.is_empty() => {
                self.push_toast(format!("Finding album: {}…", t.album), Severity::Info);
                runtime.send(AppCommand::SearchAndOpenAlbum {
                    name: t.album.clone(),
                    artist: t.artist.clone(),
                });
            }
            _ => self.push_toast("Go to album: unavailable from this row", Severity::Warning),
        }
    }

    /// The `f` key: toggle the like state of the *current* track.
    ///
    /// Mirrors Python's `action_toggle_like`, which reads
    /// `queue_manager.current_track` and returns silently when nothing is
    /// playing. The current `video_id` is tracked from [`AppEvent::NowPlaying`].
    fn toggle_like_current(&mut self, runtime: &RuntimeHandle) {
        let Some(video_id) = self.current_video_id.clone() else {
            return; // nothing playing — silent no-op (Python parity)
        };
        runtime.send(AppCommand::ToggleLike(video_id));
    }

    /// The `R` key: start a radio seeded by the *current* track.
    ///
    /// Mirrors Python's `action_start_radio` (uses `current_track`, silent when
    /// nothing is playing).
    fn start_radio_current(&mut self, runtime: &RuntimeHandle) {
        let Some(video_id) = self.current_video_id.clone() else {
            return;
        };
        runtime.send(AppCommand::StartRadio(video_id));
    }

    /// The `a` key: jump to the *current* track's artist, resolved by search.
    ///
    /// Mirrors Python's `action_open_current_artist` → `_lookup_and_open_artist`:
    /// requires a current track with a non-empty artist name, then searches the
    /// name and opens the first hit's page. A missing track or empty artist name
    /// is a silent no-op (Python `return`s).
    fn open_current_artist(&mut self, runtime: &RuntimeHandle) {
        if self.current_video_id.is_none() || self.player.artist.is_empty() {
            return;
        }
        let name = self.player.artist.clone();
        self.push_toast(format!("Finding artist: {name}…"), Severity::Info);
        runtime.send(AppCommand::SearchAndOpenArtist(name));
    }

    /// The `A` key: jump to the *current* track's album, resolved by search.
    ///
    /// Mirrors Python's `action_open_current_album` → `_lookup_and_open_album`.
    fn open_current_album(&mut self, runtime: &RuntimeHandle) {
        if self.current_video_id.is_none() || self.player.album.is_empty() {
            return;
        }
        let name = self.player.album.clone();
        let artist = self.player.artist.clone();
        self.push_toast(format!("Finding album: {name}…"), Severity::Info);
        runtime.send(AppCommand::SearchAndOpenAlbum { name, artist });
    }

    /// Toggle the `/`-triggered in-page filter bar for the current view.
    ///
    /// Only the filterable single-list views (playlist, history, queue) support
    /// filtering. On any other view the toggle is a no-op with a "Nothing to
    /// filter here" note (mirrors Python's FilterBar.show notify when the view
    /// has no filterable table). Toggling off clears the view's filter.
    fn toggle_filter(&mut self) {
        if !self.view_is_filterable() {
            // Python's `filter_bar.py` notified with a 4 s warning timeout.
            self.push_toast_with_timeout(
                "Nothing to filter here",
                Severity::Warning,
                Duration::from_secs(4),
            );
            return;
        }
        let now_active = self.filter.toggle();
        if !now_active {
            // Closed: drop the view's filter so the full list returns.
            self.clear_view_filter();
        }
    }

    /// Whether the active view supports the in-page filter bar.
    ///
    /// The filterable views are the flat single-list ones whose rows the bar can
    /// substring-match: the playlist browser (both levels), history, and queue.
    /// The home / search / library / album / artist / lyrics views have a
    /// different (multi-pane or non-row) shape and are not filtered here,
    /// matching Python where only those views define `toggle_filter`.
    fn view_is_filterable(&self) -> bool {
        matches!(self.view, View::Playlist | View::History | View::Queue)
    }

    /// Push the current filter query into the active filterable view. Called
    /// each tick before rendering so the view's visible rows track the query as
    /// the user types.
    fn apply_filter_to_view(&mut self) {
        let query = self.filter.active_query();
        match self.view {
            View::Playlist => self.playlist.set_filter(query),
            View::History => self.history.set_filter(query),
            View::Queue => self.queue_view.set_filter(query),
            _ => {}
        }
    }

    /// Clear the active view's filter (on bar close or view switch).
    fn clear_view_filter(&mut self) {
        self.playlist.set_filter(None);
        self.history.set_filter(None);
        self.queue_view.set_filter(None);
    }

    /// Close the filter bar and clear the view filter — called on any view
    /// switch so a filter never lingers behind a navigation change.
    fn close_filter(&mut self) {
        if self.filter.is_active() {
            self.filter.hide();
        }
        self.clear_view_filter();
    }

    /// The `(visible, total)` row counts of the active filterable view, for the
    /// filter bar's `visible/total` label.
    fn filter_counts(&self) -> (usize, usize) {
        match self.view {
            View::Playlist => self.playlist.filter_counts(),
            View::History => self.history.filter_counts(),
            View::Queue => self.queue_view.filter_counts(),
            _ => (0, 0),
        }
    }

    /// Switch to one of the top-level views (`g`/`/`/`l`), pushing the nav stack
    /// so Esc can pop back. Mirrors Python's `action_switch_view`, which calls
    /// `nav.push(PageState(view_id))` then applies the page (refreshing the
    /// library / focusing the search input as needed).
    ///
    /// Switching to the view that is already active is a no-op (the nav stack's
    /// duplicate-push guard already prevents a wasted history entry; this also
    /// avoids re-fetching). Switching to search focuses its input
    /// (`input_mode = true`), matching Python's `action_search_page` /
    /// auto-focus; switching to library kicks off the three library fetches
    /// (Python's `refresh_library` on apply).
    fn switch_view(&mut self, target: View, runtime: &RuntimeHandle) {
        if self.view == target {
            // Already here. Search still (re)focuses the input so `/` is a
            // reliable "start typing" shortcut even when search is showing.
            if target == View::Search {
                self.input_mode = true;
            }
            return;
        }
        self.close_filter();
        self.view = target;
        self.nav.push(NavPage::new(page_type_for_view(target)));
        self.input_mode = false;
        match target {
            View::Search => {
                // Focus the input so the user can type immediately (Python
                // deferred-focus of the search input).
                self.input_mode = true;
            }
            View::Library => {
                // (Re)load the library panes on every switch-to.
                self.refresh_library(runtime);
            }
            View::History => {
                // Re-fetch history on every view-switch (mirrors Python's
                // `on_show` re-fetch).
                self.refresh_history(runtime);
            }
            View::Queue => {
                // Immediately snapshot the current queue.
                runtime.send(AppCommand::FetchQueue);
            }
            View::Lyrics => {
                // Fetch lyrics for the currently playing track (if any).
                // If nothing is playing, reflect that in the header.
                if let Some(video_id) = self.current_video_id.clone() {
                    self.lyrics.start_loading(&self.player.title);
                    runtime.send(AppCommand::FetchLyrics(video_id));
                } else {
                    self.lyrics.start_loading("No track playing");
                }
            }
            View::Home | View::Playlist | View::Album | View::Artist => {}
        }
    }

    /// Kick off the three library fetches (playlists / albums / artists) plus
    /// liked songs. Mirrors Python's `LibraryView.refresh_library` →
    /// `_fetch_all_data`, which fires the three workers; liked songs is the
    /// extra pseudo-playlist row this port adds to the playlists pane.
    fn refresh_library(&mut self, runtime: &RuntimeHandle) {
        self.library.set_loading();
        runtime.send(AppCommand::FetchLibraryPlaylists);
        runtime.send(AppCommand::FetchLibraryAlbums);
        runtime.send(AppCommand::FetchLibraryArtists);
        runtime.send(AppCommand::FetchLikedSongs);
    }

    /// Re-fetch history and reset its view to loading. Called on every
    /// switch-to-history (mirrors Python's `HistoryView.on_show` re-fetch).
    fn refresh_history(&mut self, runtime: &RuntimeHandle) {
        self.history.set_loading();
        runtime.send(AppCommand::FetchHistory);
    }

    /// Move the cursor down in the active view.
    fn select_next(&mut self) {
        match self.view {
            View::Home => self.home.select_next_item(),
            View::Playlist => self.playlist.select_next(),
            View::Search => self.search.select_next(),
            View::Library => self.library.select_next(),
            View::Album => self.album.select_next(),
            View::Artist => self.artist.select_next(),
            View::History => self.history.select_next(),
            View::Queue => self.queue_view.select_next(),
            View::Lyrics => self.lyrics.scroll_down(),
        }
    }

    /// Move the cursor up in the active view.
    fn select_previous(&mut self) {
        match self.view {
            View::Home => self.home.select_previous_item(),
            View::Playlist => self.playlist.select_previous(),
            View::Search => self.search.select_previous(),
            View::Library => self.library.select_previous(),
            View::Album => self.album.select_previous(),
            View::Artist => self.artist.select_previous(),
            View::History => self.history.select_previous(),
            View::Queue => self.queue_view.select_previous(),
            View::Lyrics => self.lyrics.scroll_up(),
        }
    }

    /// Handle Enter, dispatched to the active view.
    fn activate(&mut self, runtime: &RuntimeHandle) {
        match self.view {
            View::Home => self.activate_home(runtime),
            View::Playlist => self.activate_playlist(runtime),
            View::Search => self.activate_search(runtime),
            View::Library => self.activate_library(runtime),
            View::Album => self.activate_album(runtime),
            View::Artist => self.activate_artist(runtime),
            View::History => self.activate_history(runtime),
            View::Queue => self.activate_queue(runtime),
            // Lyrics view has no selectable items.
            View::Lyrics => {}
        }
    }

    /// Enter on the home view: play a track, or open a playlist (switch to the
    /// playlist view and drill in).
    fn activate_home(&mut self, runtime: &RuntimeHandle) {
        match self.home.activate_selected() {
            Some(HomeAction::Play(track)) => {
                self.player.on_started();
                runtime.send(AppCommand::Play(track));
            }
            Some(HomeAction::OpenPlaylist(info)) => self.open_playlist(info, runtime),
            None => {}
        }
    }

    /// Enter on the playlist view: drill into a playlist, or play from a track.
    fn activate_playlist(&mut self, runtime: &RuntimeHandle) {
        match self.playlist.activate_selected() {
            Some(PlaylistAction::OpenPlaylist(info)) => {
                // Drill into the selected playlist's tracks (level 2).
                self.playlist.show_track_list_loading(&info.title);
                self.pending_tracks = PendingTracksFetch::Playlist;
                runtime.send(AppCommand::FetchPlaylistTracks {
                    playlist_id: info.playlist_id,
                    title: info.title,
                });
            }
            Some(PlaylistAction::PlayTracks {
                tracks,
                start_index,
            }) => {
                self.player.on_started();
                runtime.send(AppCommand::PlayPlaylist {
                    tracks,
                    start_index,
                });
            }
            None => {}
        }
    }

    /// Enter on the search view: route by the focused pane. Tracks play;
    /// playlists open in the playlist view (reusing its drill-in flow); albums
    /// and artists navigate to their respective detail views.
    fn activate_search(&mut self, runtime: &RuntimeHandle) {
        match self.search.activate_selected() {
            Some(SearchAction::PlayTrack(track)) => {
                self.player.on_started();
                runtime.send(AppCommand::Play(track));
            }
            Some(SearchAction::OpenPlaylist(info)) => self.open_playlist(info, runtime),
            Some(SearchAction::OpenAlbum(album)) => {
                self.open_album(album.browse_id.clone(), runtime)
            }
            Some(SearchAction::OpenArtist(artist)) => {
                self.open_artist(artist.channel_id.clone(), runtime)
            }
            None => {}
        }
    }

    /// Enter on the library view: route by the focused pane. A playlist drills
    /// into its tracks *within* the library's Playlists pane (Python
    /// `_show_track_list`, which stays in `LibraryView` rather than switching to
    /// the standalone playlist view); a track row (drill-down or liked songs)
    /// plays from the cursor, queueing the rest; albums and artists navigate to
    /// their detail views.
    fn activate_library(&mut self, runtime: &RuntimeHandle) {
        match self.library.activate_selected() {
            Some(LibraryAction::OpenPlaylist(info)) => {
                self.library.show_track_list_loading(&info.title);
                self.pending_tracks = PendingTracksFetch::Library;
                runtime.send(AppCommand::FetchPlaylistTracks {
                    playlist_id: info.playlist_id,
                    title: info.title,
                });
            }
            Some(LibraryAction::PlayTracks {
                tracks,
                start_index,
            }) => {
                self.player.on_started();
                runtime.send(AppCommand::PlayPlaylist {
                    tracks,
                    start_index,
                });
            }
            Some(LibraryAction::OpenAlbum(album)) => {
                self.open_album(album.browse_id.clone(), runtime)
            }
            Some(LibraryAction::OpenArtist(artist)) => {
                self.open_artist(artist.channel_id.clone(), runtime)
            }
            None => {}
        }
    }

    /// Navigate to the album detail view and kick off the fetch.
    ///
    /// Pushes "album" onto the nav stack so Esc returns to the caller (search,
    /// library, or artist). Resets the album view to loading and fires
    /// [`AppCommand::FetchAlbum`]. Mirrors Python's `action_open_album`.
    fn open_album(&mut self, browse_id: String, runtime: &RuntimeHandle) {
        let fetch = self.prepare_open_album(&browse_id);
        runtime.send(fetch);
    }

    /// The runtime-free half of [`AppModel::open_album`]: switch to the album
    /// view, reset it to loading, and push the nav entry. Returns the
    /// [`AppCommand::FetchAlbum`] the caller must dispatch — so the event-fold
    /// path (which has no `RuntimeHandle`) can reuse the exact navigation and
    /// return the fetch as its follow-up command.
    fn prepare_open_album(&mut self, browse_id: &str) -> AppCommand {
        self.close_filter();
        self.input_mode = false;
        self.view = View::Album;
        self.album = AlbumView::new(); // fresh loading state for the new album
        self.nav
            .push(NavPage::with_context("album", "browse_id", browse_id));
        AppCommand::FetchAlbum(browse_id.to_owned())
    }

    /// Navigate to the artist detail view and kick off the fetch.
    ///
    /// Mirrors [`AppModel::open_album`] but for artists. Recursive chains like
    /// search→artist→related-artist→album work because each push is independent.
    fn open_artist(&mut self, channel_id: String, runtime: &RuntimeHandle) {
        let fetch = self.prepare_open_artist(&channel_id);
        runtime.send(fetch);
    }

    /// The runtime-free half of [`AppModel::open_artist`] (see
    /// [`AppModel::prepare_open_album`]). Returns the [`AppCommand::FetchArtist`]
    /// the caller dispatches.
    fn prepare_open_artist(&mut self, channel_id: &str) -> AppCommand {
        self.close_filter();
        self.input_mode = false;
        self.view = View::Artist;
        self.artist = ArtistView::new(); // fresh loading state for the new artist
        self.nav
            .push(NavPage::with_context("artist", "channel_id", channel_id));
        AppCommand::FetchArtist(channel_id.to_owned())
    }

    /// Enter on the album view: play from the cursor, queueing the rest of the
    /// album (same semantics as the playlist track list — Python's
    /// `set_playlist(self._tracks, start_index=row_index)`).
    fn activate_album(&mut self, runtime: &RuntimeHandle) {
        match self.album.activate_selected() {
            Some(AlbumAction::PlayTracks {
                tracks,
                start_index,
            }) => {
                self.player.on_started();
                runtime.send(AppCommand::PlayPlaylist {
                    tracks,
                    start_index,
                });
            }
            None => {}
        }
    }

    /// Enter on the artist view: route by the focused section.
    fn activate_artist(&mut self, runtime: &RuntimeHandle) {
        match self.artist.activate_selected() {
            Some(ArtistAction::PlayTrack(track)) => {
                self.player.on_started();
                runtime.send(AppCommand::Play(track));
            }
            Some(ArtistAction::OpenAlbum(album)) => {
                self.open_album(album.browse_id.clone(), runtime)
            }
            Some(ArtistAction::OpenArtist(related)) => {
                self.open_artist(related.channel_id.clone(), runtime);
            }
            None => {}
        }
    }

    /// Enter on the history view: play the history from the cursor, queueing
    /// the rest (same semantics as album/playlist — Python's
    /// `set_playlist(self._tracks, start_index=row_index)`).
    fn activate_history(&mut self, runtime: &RuntimeHandle) {
        match self.history.activate_selected() {
            Some(HistoryAction::PlayTracks {
                tracks,
                start_index,
            }) => {
                self.player.on_started();
                runtime.send(AppCommand::PlayPlaylist {
                    tracks,
                    start_index,
                });
            }
            None => {}
        }
    }

    /// Enter on the queue view: jump to the selected position.
    fn activate_queue(&mut self, runtime: &RuntimeHandle) {
        match self.queue_view.activate_selected() {
            Some(QueueAction::JumpTo {
                tracks,
                start_index,
            }) => {
                self.player.on_started();
                runtime.send(AppCommand::PlayPlaylist {
                    tracks,
                    start_index,
                });
            }
            None => {}
        }
    }

    /// Switch to the playlist view, drilling into `info` and pushing the nav
    /// stack (home → playlist) so Esc can pop back. The level-1 library list is
    /// fetched eagerly too, so a subsequent Esc-to-level-1 has data to show.
    fn open_playlist(&mut self, info: ytmusic_api::PlaylistInfo, runtime: &RuntimeHandle) {
        // Leaving the search view via a result: drop input focus so the flag
        // isn't left stale (inert behind is_input_active, but keep it honest).
        self.close_filter();
        self.input_mode = false;
        self.view = View::Playlist;
        self.nav.push(NavPage::with_context(
            "playlist",
            "playlist_id",
            &info.playlist_id,
        ));
        // Prime the level-1 list (for a later Esc) and drill into level 2.
        runtime.send(AppCommand::FetchLibraryPlaylists);
        self.playlist.show_track_list_loading(&info.title);
        self.pending_tracks = PendingTracksFetch::Playlist;
        runtime.send(AppCommand::FetchPlaylistTracks {
            playlist_id: info.playlist_id,
            title: info.title,
        });
    }

    /// Handle Esc / go-back, in priority order:
    ///
    /// 1. While the search input is focused, Esc just leaves input focus (the
    ///    grid keeps its results) — mirrors Textual's `Input` releasing focus on
    ///    Escape without navigating away.
    /// 2. Inside the playlist view's track list, Esc pops to the playlist list
    ///    (consumed by the view).
    /// 3. Inside the library view's track drill-down, Esc restores the playlists
    ///    pane (consumed by the view; Python `_restore_playlists_pane`).
    /// 4. Otherwise pop the navigation stack to the previous page.
    fn go_back(&mut self, runtime: &RuntimeHandle) {
        // 1. Leave search input focus without navigating.
        if self.view == View::Search && self.input_mode {
            self.input_mode = false;
            return;
        }
        // 2. Playlist level 2 → level 1 is the view's own concern.
        if self.view == View::Playlist && self.playlist.go_back() {
            // The view reset its level-1 list to Loading; re-fetch it.
            runtime.send(AppCommand::FetchLibraryPlaylists);
            return;
        }
        // 3. Library track drill-down → playlists pane (view-consumed).
        if self.view == View::Library && self.library.go_back() {
            return;
        }
        // 4. Otherwise pop the page stack (album/artist/lyrics/history/queue → caller).
        if let Some(page) = self.nav.pop() {
            self.switch_to_page(&page, runtime);
        }
    }

    /// Switch the active view to match a popped navigation page.
    ///
    /// Most views keep their loaded state on pop-back (no re-fetch). The
    /// exceptions are:
    ///
    /// - **Library**: always re-fetches (Python's `_apply_page` calls
    ///   `refresh_library` whenever the library page becomes current — its
    ///   panes are cheap to repopulate and may be stale).
    /// - **History / Queue**: always re-fetch since they track live state.
    /// - **Artist / Album**: *must* re-fetch from context so that popping from
    ///   artist B back to artist A re-loads A's data, not B's stale data.
    ///   Mirrors Python's `_apply_page`, which reads `channel_id` / `browse_id`
    ///   from the page context and re-fires the fetch every time a page is
    ///   applied.
    fn switch_to_page(&mut self, page: &NavPage, runtime: &RuntimeHandle) {
        self.close_filter();
        self.view = view_for_page(&page.page_type);
        // Leaving search input focus when navigating away from search.
        self.input_mode = false;
        match self.view {
            View::Library => self.refresh_library(runtime),
            View::History => self.refresh_history(runtime),
            View::Queue => {
                runtime.send(AppCommand::FetchQueue);
            }
            View::Artist => {
                // Re-fetch from the nav context so pop-back shows the correct
                // artist (not the last one loaded into self.artist).
                if let Some(channel_id) = page.context.get("channel_id") {
                    self.artist = ArtistView::new();
                    runtime.send(AppCommand::FetchArtist(channel_id.clone()));
                }
            }
            View::Album => {
                // Re-fetch from the nav context so pop-back shows the correct
                // album.
                if let Some(browse_id) = page.context.get("browse_id") {
                    self.album = AlbumView::new();
                    runtime.send(AppCommand::FetchAlbum(browse_id.clone()));
                }
            }
            _ => {}
        }
    }

    /// Draw the whole UI: the theme-filled background, the optional pending-key
    /// header line, the content, the optional in-page filter bar, the docked
    /// player bar, any popup overlay, and finally the floating toasts.
    fn render(&self, frame: &mut ratatui::Frame<'_>) {
        let area = frame.area();

        // Paint the entire frame with the theme surface color first so the
        // terminal's default background never shows through (Python's
        // `Screen { background: $surface }`). Every view then draws its own
        // panels on top of this fill.
        frame.render_widget(
            Block::default().style(Style::default().bg(self.theme.surface)),
            area,
        );

        let header_lines = self.header_line_count();
        // The filter bar takes its own fixed-height row just above the player
        // bar when active; otherwise the content extends to the player bar.
        let filter_rows = if self.filter.is_active() {
            FILTER_BAR_HEIGHT
        } else {
            0
        };
        let chunks = Layout::vertical([
            Constraint::Length(header_lines),
            Constraint::Min(1),
            Constraint::Length(filter_rows),
            Constraint::Length(PLAYER_BAR_HEIGHT),
        ])
        .split(area);

        self.render_header(frame, chunks[0]);
        match self.view {
            View::Home => self.home.render(frame, chunks[1], &self.theme),
            View::Playlist => self.playlist.render(frame, chunks[1], &self.theme),
            View::Search => self
                .search
                .render(frame, chunks[1], &self.theme, self.input_mode),
            View::Library => self.library.render(frame, chunks[1], &self.theme),
            View::Album => self.album.render(frame, chunks[1], &self.theme),
            View::Artist => self.artist.render(frame, chunks[1], &self.theme),
            View::Lyrics => self.lyrics.render(frame, chunks[1], &self.theme),
            View::History => self.history.render(frame, chunks[1], &self.theme),
            View::Queue => self.queue_view.render(frame, chunks[1], &self.theme),
        }
        if self.filter.is_active() {
            let (visible, total) = self.filter_counts();
            self.filter
                .render(frame, chunks[2], &self.theme, visible, total);
        }
        PlayerBar.render(frame, chunks[3], &self.player, &self.theme);

        // The popup overlay is drawn last so it floats over everything, centered
        // on the content area (above the player bar).
        if self.popups.is_open() {
            let overlay_area = Rect {
                height: chunks[0].height + chunks[1].height,
                ..area
            };
            self.popups.render(frame, overlay_area, &self.theme);
        }

        // Toasts float on top of even the popups, anchored to the bottom-right
        // above the player bar (Python's `notify()` toast rack). The content +
        // header area excludes the docked player and filter bars.
        let toast_area = Rect {
            height: chunks[0].height + chunks[1].height,
            ..area
        };
        self.toasts.render(frame, toast_area, &self.theme);
    }

    /// Number of header rows currently needed.
    ///
    /// The header now carries only the pending key-sequence prefix hint (the one
    /// genuinely persistent indicator Python kept on screen). Every transient
    /// message — errors, confirmations, warnings — is a floating toast instead.
    fn header_line_count(&self) -> u16 {
        u16::from(self.keymap.pending().is_some())
    }

    /// Render the pending key-sequence prefix line into the header (if armed).
    fn render_header(&self, frame: &mut ratatui::Frame<'_>, area: Rect) {
        if area.height == 0 {
            return;
        }
        // The armed key-sequence prefix (e.g. `g…`), mirroring spotify_player's
        // pending-key hint so the user knows a sequence is in progress. This is
        // the only persistent header line; transient messages are toasts.
        if let Some(prefix) = self.keymap.pending_label() {
            let line = Line::from(Span::styled(
                format!("{prefix}…"),
                Style::default()
                    .fg(self.theme.accent)
                    .add_modifier(Modifier::BOLD),
            ));
            frame.render_widget(Paragraph::new(line), area);
        }
    }

    /// Expire any toasts whose timeout has elapsed. Called once per UI tick.
    fn prune_toasts(&mut self) {
        self.toasts.prune();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ytmusic_api::{HomeSection, HomeSectionItem, PlaylistInfo, Track};

    // -- nav-key mapping ---------------------------------------------------
    //
    // The keymap-bound actions (Q/space/n/p/s/r/g/l/q/H/L/.../digits) are
    // covered exhaustively in the `keymap` module's own tests. Here we cover the
    // always-on navigation keys (which bypass the dispatcher) and the
    // integration path (dispatch_key → view switch / command) that the keymap
    // unit tests cannot see.

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    // -- toast assertions (replacing the old single status line) -----------

    /// The message of the most-recently-pushed live toast, if any. Toasts are
    /// appended, so the newest is last.
    fn latest_toast(model: &AppModel) -> Option<&str> {
        model.toasts.toasts().last().map(|t| t.message.as_str())
    }

    /// Whether any live toast's message contains `needle`.
    fn has_toast_containing(model: &AppModel, needle: &str) -> bool {
        model
            .toasts
            .toasts()
            .iter()
            .any(|t| t.message.contains(needle))
    }

    #[test]
    fn nav_keys_map_to_nav_actions() {
        assert_eq!(map_nav_key(key(KeyCode::Down)), Some(NavAction::SelectNext));
        assert_eq!(
            map_nav_key(key(KeyCode::Char('j'))),
            Some(NavAction::SelectNext)
        );
        assert_eq!(
            map_nav_key(key(KeyCode::Up)),
            Some(NavAction::SelectPrevious)
        );
        assert_eq!(
            map_nav_key(key(KeyCode::Char('k'))),
            Some(NavAction::SelectPrevious)
        );
        assert_eq!(map_nav_key(key(KeyCode::Tab)), Some(NavAction::NextSection));
        assert_eq!(
            map_nav_key(key(KeyCode::BackTab)),
            Some(NavAction::PreviousSection)
        );
        assert_eq!(map_nav_key(key(KeyCode::Enter)), Some(NavAction::Activate));
    }

    #[test]
    fn shift_tab_via_modifier_maps_to_previous_section() {
        let shift_tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT);
        assert_eq!(map_nav_key(shift_tab), Some(NavAction::PreviousSection));
    }

    #[test]
    fn non_nav_keys_are_not_nav_actions() {
        // The keymap-bound keys are NOT navigation keys (they go to the
        // dispatcher instead).
        assert_eq!(map_nav_key(key(KeyCode::Char('Q'))), None);
        assert_eq!(map_nav_key(key(KeyCode::Char('s'))), None);
        assert_eq!(map_nav_key(key(KeyCode::Esc)), None);
    }

    // -- dispatch_key integration (keymap → action → effect) ---------------

    #[test]
    fn capital_q_quits_via_dispatch() {
        let (runtime, _rx) = RuntimeHandle::stub();
        let mut model = AppModel::new(Theme::default());
        model.dispatch_key(key(KeyCode::Char('Q')), &runtime);
        assert!(model.should_quit);
    }

    #[test]
    fn ctrl_c_quits_via_dispatch() {
        let (runtime, _rx) = RuntimeHandle::stub();
        let mut model = AppModel::new(Theme::default());
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        model.dispatch_key(ctrl_c, &runtime);
        assert!(model.should_quit);
    }

    #[test]
    fn lowercase_q_switches_to_queue_via_dispatch() {
        // Lowercase q switches to the queue view, not quit (M5b-2b resolution).
        let (runtime, _rx) = RuntimeHandle::stub();
        let mut model = AppModel::new(Theme::default());
        model.dispatch_key(key(KeyCode::Char('q')), &runtime);
        assert_eq!(model.view, View::Queue);
        assert!(!model.should_quit);
    }

    #[test]
    fn digit_5_switches_to_history_via_dispatch() {
        let (runtime, _rx) = RuntimeHandle::stub();
        let mut model = AppModel::new(Theme::default());
        model.dispatch_key(key(KeyCode::Char('5')), &runtime);
        assert_eq!(model.view, View::History);
    }

    #[test]
    fn space_sends_toggle_pause_via_dispatch() {
        let (runtime, mut rx) = RuntimeHandle::stub();
        let mut model = AppModel::new(Theme::default());
        model.dispatch_key(key(KeyCode::Char(' ')), &runtime);
        assert_eq!(rx.try_recv().ok(), Some(AppCommand::TogglePause));
    }

    #[test]
    fn transport_keys_send_commands_via_dispatch() {
        let (runtime, mut rx) = RuntimeHandle::stub();
        let mut model = AppModel::new(Theme::default());
        model.dispatch_key(key(KeyCode::Char('n')), &runtime);
        assert_eq!(rx.try_recv().ok(), Some(AppCommand::NextTrack));
        model.dispatch_key(key(KeyCode::Char('p')), &runtime);
        assert_eq!(rx.try_recv().ok(), Some(AppCommand::PreviousTrack));
        model.dispatch_key(key(KeyCode::Char('s')), &runtime);
        assert_eq!(rx.try_recv().ok(), Some(AppCommand::ToggleShuffle));
        model.dispatch_key(key(KeyCode::Char('r')), &runtime);
        assert_eq!(rx.try_recv().ok(), Some(AppCommand::CycleRepeat));
    }

    #[test]
    fn b_cycles_audio_quality_via_dispatch() {
        let (runtime, mut rx) = RuntimeHandle::stub();
        let mut model = AppModel::new(Theme::default());
        model.dispatch_key(key(KeyCode::Char('b')), &runtime);
        assert_eq!(rx.try_recv().ok(), Some(AppCommand::CycleAudioQuality));
    }

    // -- seek / mute (Stage 1) ---------------------------------------------

    #[test]
    fn seek_keys_send_seek_commands_via_dispatch() {
        let (runtime, mut rx) = RuntimeHandle::stub();
        let mut model = AppModel::new(Theme::default());
        model.dispatch_key(key(KeyCode::Char('>')), &runtime);
        assert_eq!(rx.try_recv().ok(), Some(AppCommand::SeekForward));
        model.dispatch_key(key(KeyCode::Char('<')), &runtime);
        assert_eq!(rx.try_recv().ok(), Some(AppCommand::SeekBackward));
        model.dispatch_key(key(KeyCode::Char('^')), &runtime);
        assert_eq!(rx.try_recv().ok(), Some(AppCommand::SeekToStart));
    }

    #[test]
    fn underscore_sends_toggle_mute_via_dispatch() {
        let (runtime, mut rx) = RuntimeHandle::stub();
        let mut model = AppModel::new(Theme::default());
        model.dispatch_key(key(KeyCode::Char('_')), &runtime);
        assert_eq!(rx.try_recv().ok(), Some(AppCommand::ToggleMute));
    }

    #[test]
    fn player_mute_event_folds_into_bar() {
        let mut model = AppModel::new(Theme::default());
        fold(&mut model, AppEvent::PlayerMute(true));
        assert!(model.player.is_muted);
        fold(&mut model, AppEvent::PlayerMute(false));
        assert!(!model.player.is_muted);
    }

    // -- keymap warnings as a startup toast (Stage 4) ----------------------

    #[test]
    fn keymap_warnings_surface_as_a_toast() {
        let mut model = AppModel::new(Theme::default());
        model.set_keymap_warnings(&["quit = \"notakey\"".to_owned()]);
        let status = latest_toast(&model).unwrap_or("");
        assert!(status.contains("1 keymap binding ignored"), "got: {status}");
        assert!(status.contains("notakey"), "got: {status}");
        // It is a warning-severity toast.
        assert_eq!(model.toasts.toasts()[0].severity, Severity::Warning);
    }

    #[test]
    fn keymap_warnings_plural_and_empty() {
        let mut model = AppModel::new(Theme::default());
        model.set_keymap_warnings(&["a = \"?\"".to_owned(), "b = \"!\"".to_owned()]);
        assert!(has_toast_containing(&model, "2 keymap bindings ignored"));
        // An empty list pushes no toast.
        let mut clean = AppModel::new(Theme::default());
        clean.set_keymap_warnings(&[]);
        assert!(clean.toasts.is_empty());
    }

    // -- like / radio / a / A (Stage 2) ------------------------------------

    /// A model with a current track loaded (the state after a NowPlaying).
    fn model_with_current_track() -> AppModel {
        use ytmusic_tui::app::NowPlaying;
        use ytmusic_tui::queue::RepeatMode;
        let mut model = AppModel::new(Theme::default());
        fold(
            &mut model,
            AppEvent::NowPlaying(NowPlaying {
                title: "Get Lucky".to_owned(),
                artist: "Daft Punk".to_owned(),
                album: "Random Access Memories".to_owned(),
                video_id: "v1".to_owned(),
                duration_seconds: 248.0,
                shuffle: false,
                repeat: RepeatMode::Off,
            }),
        );
        model
    }

    #[test]
    fn f_toggles_like_on_current_track() {
        let (runtime, mut rx) = RuntimeHandle::stub();
        let mut model = model_with_current_track();
        model.dispatch_key(key(KeyCode::Char('f')), &runtime);
        assert_eq!(
            rx.try_recv().ok(),
            Some(AppCommand::ToggleLike("v1".into()))
        );
    }

    #[test]
    fn f_with_no_current_track_is_silent() {
        let (runtime, mut rx) = RuntimeHandle::stub();
        let mut model = AppModel::new(Theme::default()); // nothing playing
        model.dispatch_key(key(KeyCode::Char('f')), &runtime);
        assert!(rx.try_recv().is_err(), "no command when nothing is playing");
    }

    #[test]
    fn capital_r_starts_radio_on_current_track() {
        let (runtime, mut rx) = RuntimeHandle::stub();
        let mut model = model_with_current_track();
        model.dispatch_key(key(KeyCode::Char('R')), &runtime);
        assert_eq!(
            rx.try_recv().ok(),
            Some(AppCommand::StartRadio("v1".into()))
        );
    }

    #[test]
    fn a_resolves_current_artist_by_name() {
        let (runtime, mut rx) = RuntimeHandle::stub();
        let mut model = model_with_current_track();
        model.dispatch_key(key(KeyCode::Char('a')), &runtime);
        assert_eq!(
            rx.try_recv().ok(),
            Some(AppCommand::SearchAndOpenArtist("Daft Punk".into()))
        );
    }

    #[test]
    fn capital_a_resolves_current_album_by_name() {
        let (runtime, mut rx) = RuntimeHandle::stub();
        let mut model = model_with_current_track();
        model.dispatch_key(key(KeyCode::Char('A')), &runtime);
        assert_eq!(
            rx.try_recv().ok(),
            Some(AppCommand::SearchAndOpenAlbum {
                name: "Random Access Memories".into(),
                artist: "Daft Punk".into(),
            })
        );
    }

    #[test]
    fn a_with_no_current_track_is_silent() {
        let (runtime, mut rx) = RuntimeHandle::stub();
        let mut model = AppModel::new(Theme::default());
        model.dispatch_key(key(KeyCode::Char('a')), &runtime);
        assert!(rx.try_recv().is_err(), "no lookup when nothing is playing");
    }

    #[test]
    fn artist_resolved_event_navigates_and_chains_fetch() {
        // The runtime's search resolved a channel id: the fold must switch to
        // the artist view, push nav, and return the FetchArtist follow-up.
        let mut model = AppModel::new(Theme::default());
        let follow_up = model.on_event(AppEvent::ArtistResolved("UC_chan".to_owned()));
        assert_eq!(model.view, View::Artist);
        assert_eq!(
            follow_up,
            Some(AppCommand::FetchArtist("UC_chan".to_owned()))
        );
        assert_eq!(model.nav.current().page_type, "artist");
    }

    #[test]
    fn album_resolved_event_navigates_and_chains_fetch() {
        let mut model = AppModel::new(Theme::default());
        let follow_up = model.on_event(AppEvent::AlbumResolved("MPRE_x".to_owned()));
        assert_eq!(model.view, View::Album);
        assert_eq!(follow_up, Some(AppCommand::FetchAlbum("MPRE_x".to_owned())));
        assert_eq!(model.nav.current().page_type, "album");
    }

    #[test]
    fn go_to_album_on_track_row_resolves_by_search() {
        // The action popup's "Go to album" on a track row (which carries only the
        // album name) resolves by search rather than reporting unavailable.
        let (runtime, mut rx) = RuntimeHandle::stub();
        let mut model = model_on_history();
        let item = PopupItem::Track(Track::new("v1", "Song", "Band", "Greatest Hits", 100.0, ""));
        model.action_go_to_album(&item, &runtime);
        assert_eq!(
            rx.try_recv().ok(),
            Some(AppCommand::SearchAndOpenAlbum {
                name: "Greatest Hits".into(),
                artist: "Band".into(),
            })
        );
    }

    #[test]
    fn g_then_s_switches_to_search_via_dispatch() {
        // The `g s` sequence reaches the search view (search_page). A bare `g`
        // first arms the prefix (no view change yet).
        let (runtime, _rx) = RuntimeHandle::stub();
        let mut model = AppModel::new(Theme::default());
        model.dispatch_key(key(KeyCode::Char('g')), &runtime);
        assert_eq!(model.view, View::Home, "bare g only arms the prefix");
        assert!(model.keymap.pending().is_some());
        model.dispatch_key(key(KeyCode::Char('s')), &runtime);
        assert_eq!(model.view, View::Search);
        assert!(model.keymap.pending().is_none());
    }

    #[test]
    fn digit_2_switches_to_search_via_dispatch() {
        let (runtime, _rx) = RuntimeHandle::stub();
        let mut model = AppModel::new(Theme::default());
        model.dispatch_key(key(KeyCode::Char('2')), &runtime);
        assert_eq!(model.view, View::Search);
    }

    #[test]
    fn slash_toggles_filter_not_search_view() {
        // The M5c reclaim: `/` is the filter toggle, no longer the search view.
        let (runtime, _rx) = RuntimeHandle::stub();
        let mut model = AppModel::new(Theme::default());
        model.dispatch_key(key(KeyCode::Char('/')), &runtime);
        assert_eq!(model.view, View::Home, "/ must not switch to search");
    }

    #[test]
    fn nav_key_cancels_pending_prefix() {
        // While `g` is armed, a navigation key cancels the sequence and acts as
        // navigation (it does not complete or fall back to a keymap action).
        let (runtime, _rx) = RuntimeHandle::stub();
        let mut model = AppModel::new(Theme::default());
        model.dispatch_key(key(KeyCode::Char('g')), &runtime);
        assert!(model.keymap.pending().is_some());
        model.dispatch_key(key(KeyCode::Down), &runtime);
        assert!(model.keymap.pending().is_none(), "nav key clears prefix");
        assert_eq!(model.view, View::Home);
    }

    // -- filter bar integration (Stage 2) ----------------------------------

    /// A model on the history view with three tracks loaded.
    fn model_on_history() -> AppModel {
        let mut model = AppModel::new(Theme::default());
        model.view = View::History;
        model.history.set_tracks(vec![
            Track::new("v1", "Pyramid Song", "Radiohead", "Amnesiac", 100.0, ""),
            Track::new("v2", "Get Lucky", "Daft Punk", "Discovery", 100.0, ""),
            Track::new("v3", "Idioteque", "Radiohead", "Kid A", 100.0, ""),
        ]);
        model
    }

    #[test]
    fn slash_opens_filter_on_filterable_view() {
        let (runtime, _rx) = RuntimeHandle::stub();
        let mut model = model_on_history();
        model.dispatch_key(key(KeyCode::Char('/')), &runtime);
        assert!(model.filter.is_active(), "/ opens the filter on history");
    }

    #[test]
    fn slash_on_non_filterable_view_shows_note() {
        let (runtime, _rx) = RuntimeHandle::stub();
        let mut model = AppModel::new(Theme::default()); // Home view
        model.dispatch_key(key(KeyCode::Char('/')), &runtime);
        assert!(!model.filter.is_active(), "home is not filterable");
        assert_eq!(latest_toast(&model), Some("Nothing to filter here"));
    }

    #[test]
    fn typing_in_filter_narrows_the_view() {
        let (runtime, _rx) = RuntimeHandle::stub();
        let mut model = model_on_history();
        model.dispatch_key(key(KeyCode::Char('/')), &runtime); // open
        for ch in "daft".chars() {
            model.dispatch_key(key(KeyCode::Char(ch)), &runtime);
        }
        model.apply_filter_to_view();
        assert_eq!(model.filter.query(), "daft");
        let (visible, total) = model.history.filter_counts();
        assert_eq!((visible, total), (1, 3), "only the Daft Punk track matches");
    }

    #[test]
    fn esc_closes_filter_and_restores_view() {
        let (runtime, _rx) = RuntimeHandle::stub();
        let mut model = model_on_history();
        model.dispatch_key(key(KeyCode::Char('/')), &runtime);
        model.dispatch_key(key(KeyCode::Char('x')), &runtime); // no match
        model.apply_filter_to_view();
        assert_eq!(model.history.filter_counts().0, 0);
        model.dispatch_key(key(KeyCode::Esc), &runtime); // close
        model.apply_filter_to_view();
        assert!(!model.filter.is_active());
        assert_eq!(model.history.filter_counts().0, 3, "full list restored");
    }

    #[test]
    fn switching_view_closes_filter() {
        let (runtime, _rx) = RuntimeHandle::stub();
        let mut model = model_on_history();
        model.dispatch_key(key(KeyCode::Char('/')), &runtime);
        assert!(model.filter.is_active());
        // Switch to home via `g` then unbound key fallback, then assert closed.
        model.switch_view(View::Home, &runtime);
        assert!(!model.filter.is_active(), "view switch closes the filter");
    }

    // -- popup integration (Stage 3) ---------------------------------------

    #[test]
    fn open_action_popup_dot_key_on_track_row() {
        let (runtime, _rx) = RuntimeHandle::stub();
        let mut model = model_on_history();
        // `.` (full_stop) opens the action popup on the selected track.
        model.dispatch_key(key(KeyCode::Char('.')), &runtime);
        assert!(model.popups.is_open(), "action popup should open");
    }

    #[test]
    fn open_action_popup_on_lyrics_view_is_noop() {
        let (runtime, _rx) = RuntimeHandle::stub();
        let mut model = AppModel::new(Theme::default());
        model.view = View::Lyrics;
        model.dispatch_key(key(KeyCode::Char('.')), &runtime);
        assert!(!model.popups.is_open(), "lyrics has no selectable item");
        assert_eq!(latest_toast(&model), Some("Nothing to act on here"));
    }

    #[test]
    fn action_popup_play_sends_play_command() {
        let (runtime, mut rx) = RuntimeHandle::stub();
        let mut model = model_on_history();
        model.dispatch_key(key(KeyCode::Char('.')), &runtime);
        // Drain the primed FetchLibraryPlaylists.
        while let Ok(cmd) = rx.try_recv() {
            if matches!(cmd, AppCommand::FetchLibraryPlaylists) {
                continue;
            }
            break;
        }
        // First action is "Play"; Enter selects it.
        model.dispatch_key(key(KeyCode::Enter), &runtime);
        assert!(!model.popups.is_open(), "popup closes after selection");
        // The Play command was sent for the selected (first) history track.
        let mut saw_play = false;
        while let Ok(cmd) = rx.try_recv() {
            if matches!(cmd, AppCommand::Play(ref t) if t.video_id == "v1") {
                saw_play = true;
            }
        }
        assert!(saw_play, "Play action should send AppCommand::Play");
    }

    #[test]
    fn action_popup_add_to_queue_sends_command() {
        let (runtime, mut rx) = RuntimeHandle::stub();
        let mut model = model_on_history();
        model.dispatch_key(key(KeyCode::Char('.')), &runtime);
        // Move to "Add to queue" (index 1) and select.
        model.dispatch_key(key(KeyCode::Char('j')), &runtime);
        model.dispatch_key(key(KeyCode::Enter), &runtime);
        let mut saw_add = false;
        while let Ok(cmd) = rx.try_recv() {
            if matches!(cmd, AppCommand::AddToQueue(ref t) if t.video_id == "v1") {
                saw_add = true;
            }
        }
        assert!(saw_add, "Add to queue should send AppCommand::AddToQueue");
    }

    #[test]
    fn theme_popup_applies_theme_live() {
        let (runtime, _rx) = RuntimeHandle::stub();
        let mut model = AppModel::new(Theme::default());
        model.theme_name = "synthwave".to_owned();
        model.dispatch_key(key(KeyCode::Char('T')), &runtime); // open theme popup
        assert!(model.popups.is_open());
        // The cursor starts on the current theme (synthwave, last in the sorted
        // list); `k` moves up to a different theme. Select it.
        model.dispatch_key(key(KeyCode::Char('k')), &runtime);
        model.dispatch_key(key(KeyCode::Enter), &runtime);
        assert!(!model.popups.is_open());
        assert_ne!(model.theme_name, "synthwave", "theme changed");
        assert_eq!(model.theme, Theme::from_name(&model.theme_name));
    }

    #[test]
    fn action_popup_add_to_playlist_opens_picker() {
        let (runtime, _rx) = RuntimeHandle::stub();
        let mut model = model_on_history();
        // Seed the picker cache as if LibraryPlaylistsLoaded had arrived.
        model.library_playlists = vec![("PL1".to_owned(), "My Mix".to_owned())];
        model.dispatch_key(key(KeyCode::Char('.')), &runtime);
        // "Add to playlist" is index 5 in the track action list.
        for _ in 0..5 {
            model.dispatch_key(key(KeyCode::Char('j')), &runtime);
        }
        model.dispatch_key(key(KeyCode::Enter), &runtime);
        // The action popup closed and the playlist picker opened.
        assert!(
            matches!(model.popups, PopupState::PlaylistPicker(_)),
            "Add to playlist should open the picker"
        );
    }

    #[test]
    fn esc_closes_popup_without_navigating() {
        let (runtime, _rx) = RuntimeHandle::stub();
        let mut model = model_on_history();
        model.dispatch_key(key(KeyCode::Char('.')), &runtime);
        assert!(model.popups.is_open());
        model.dispatch_key(key(KeyCode::Esc), &runtime);
        assert!(!model.popups.is_open(), "Esc dismisses the popup");
        assert_eq!(model.view, View::History, "Esc did not navigate");
    }

    // -- new-playlist naming + remove actions (Stage 3) --------------------

    #[test]
    fn new_playlist_naming_flow_sends_create_with_typed_name() {
        let (runtime, mut rx) = RuntimeHandle::stub();
        let mut model = model_on_history();
        model.library_playlists = vec![("PL1".to_owned(), "My Mix".to_owned())];
        model.dispatch_key(key(KeyCode::Char('.')), &runtime);
        // Navigate to "Add to playlist" (index 5) and open the picker.
        for _ in 0..5 {
            model.dispatch_key(key(KeyCode::Char('j')), &runtime);
        }
        model.dispatch_key(key(KeyCode::Enter), &runtime);
        assert!(matches!(model.popups, PopupState::PlaylistPicker(_)));
        // Cursor is on row 0 ("New playlist…"); Enter begins naming, then type.
        model.dispatch_key(key(KeyCode::Enter), &runtime);
        for ch in "Focus".chars() {
            model.dispatch_key(key(KeyCode::Char(ch)), &runtime);
        }
        model.dispatch_key(key(KeyCode::Enter), &runtime);
        assert!(
            !model.popups.is_open(),
            "picker closes after naming confirm"
        );
        // The create-and-add command carries the typed name (video_id v1 is the
        // first/selected history track).
        let mut saw_create = false;
        while let Ok(cmd) = rx.try_recv() {
            if let AppCommand::CreatePlaylistAndAdd { title, video_id } = cmd {
                assert_eq!(title, "Focus");
                assert_eq!(video_id, "v1");
                saw_create = true;
            }
        }
        assert!(
            saw_create,
            "expected CreatePlaylistAndAdd with the typed name"
        );
    }

    #[test]
    fn queue_view_popup_offers_remove_from_queue() {
        let (runtime, mut rx) = RuntimeHandle::stub();
        let mut model = AppModel::new(Theme::default());
        model.view = View::Queue;
        model
            .queue_view
            .set_snapshot(ytmusic_tui::views::queue_view::QueueSnapshot {
                tracks: vec![
                    Track::new("v1", "First", "A", "Al", 100.0, ""),
                    Track::new("v2", "Second", "B", "Al", 120.0, ""),
                ],
                current_index: Some(0),
            });
        model.dispatch_key(key(KeyCode::Char('.')), &runtime);
        assert!(model.popups.is_open(), "queue popup opens");
        // The queue-context action list is Play, Remove from queue, … so move to
        // index 1 and select.
        model.dispatch_key(key(KeyCode::Char('j')), &runtime);
        model.dispatch_key(key(KeyCode::Enter), &runtime);
        let mut saw_remove = false;
        while let Ok(cmd) = rx.try_recv() {
            if matches!(cmd, AppCommand::RemoveFromQueue(ref id) if id == "v1") {
                saw_remove = true;
            }
        }
        assert!(saw_remove, "Remove from queue should send the command");
    }

    /// A model on the playlist view, drilled into a playlist's track list, with
    /// the playlist id on the nav stack (the state after opening a playlist).
    fn model_on_playlist_tracks_with_id() -> AppModel {
        let mut model = AppModel::new(Theme::default());
        model.view = View::Playlist;
        model
            .nav
            .push(NavPage::with_context("playlist", "playlist_id", "PL42"));
        model.playlist.show_track_list_loading("My Mix");
        model.playlist.set_tracks(
            "My Mix",
            vec![Track::new("v1", "First", "A", "Al", 100.0, "")],
        );
        model
    }

    #[test]
    fn playlist_track_popup_offers_remove_from_playlist() {
        let (runtime, mut rx) = RuntimeHandle::stub();
        let mut model = model_on_playlist_tracks_with_id();
        model.dispatch_key(key(KeyCode::Char('.')), &runtime);
        assert!(model.popups.is_open());
        // The playlist-track action list is Play, Add to queue, Remove from
        // playlist (index 2), … — move to index 2 and select.
        model.dispatch_key(key(KeyCode::Char('j')), &runtime);
        model.dispatch_key(key(KeyCode::Char('j')), &runtime);
        model.dispatch_key(key(KeyCode::Enter), &runtime);
        let mut saw_remove = false;
        while let Ok(cmd) = rx.try_recv() {
            if let AppCommand::RemoveFromPlaylist {
                playlist_id,
                video_id,
            } = cmd
            {
                assert_eq!(playlist_id, "PL42");
                assert_eq!(video_id, "v1");
                saw_remove = true;
            }
        }
        assert!(
            saw_remove,
            "Remove from playlist should send the command with the nav playlist id"
        );
    }

    #[test]
    fn popup_context_is_plain_on_history_view() {
        // A non-queue, non-playlist-track view gets the plain track action list
        // (no remove actions).
        let model = model_on_history();
        assert_eq!(model.popup_context(), PopupContext::Plain);
    }

    // -- pending-tracks token routing (Stage 4) ----------------------------

    #[test]
    fn library_token_routes_tracks_to_library_view() {
        // With the token set to Library, a PlaylistTracksLoaded reply fills the
        // library track list even while the playlist view is also waiting — the
        // M5b heuristic would have mis-routed this.
        let mut model = AppModel::new(Theme::default());
        model.view = View::Library;
        model.library.show_track_list_loading("Lib Mix");
        model.pending_tracks = PendingTracksFetch::Library;
        // The playlist view is *also* sitting at a track-list loading state.
        model.playlist.show_track_list_loading("PL Mix");

        fold(
            &mut model,
            AppEvent::PlaylistTracksLoaded {
                title: "Lib Mix".to_owned(),
                tracks: vec![Track::new("v1", "Lib Track", "A", "Al", 100.0, "")],
            },
        );
        // The library view received the tracks; the playlist view did NOT.
        assert!(model.library.is_viewing_tracks());
        let text = render_model(&model, 90, 14);
        assert!(text.contains("Lib Track"), "library missing track:\n{text}");
        // Token consumed.
        assert_eq!(model.pending_tracks, PendingTracksFetch::None);
    }

    #[test]
    fn playlist_token_routes_tracks_to_playlist_view() {
        let mut model = AppModel::new(Theme::default());
        model.view = View::Playlist;
        model.playlist.show_track_list_loading("PL Mix");
        model.pending_tracks = PendingTracksFetch::Playlist;

        fold(
            &mut model,
            AppEvent::PlaylistTracksLoaded {
                title: "PL Mix".to_owned(),
                tracks: vec![Track::new("v1", "PL Track", "A", "Al", 100.0, "")],
            },
        );
        assert!(model.playlist.is_viewing_tracks());
        assert_eq!(model.pending_tracks, PendingTracksFetch::None);
    }

    #[test]
    fn none_token_defaults_to_playlist_view() {
        // A reply with no recorded requester (token None) falls back to the
        // playlist view — the common drill-in case.
        let mut model = AppModel::new(Theme::default());
        model.view = View::Playlist;
        model.playlist.show_track_list_loading("PL Mix");
        assert_eq!(model.pending_tracks, PendingTracksFetch::None);
        fold(
            &mut model,
            AppEvent::PlaylistTracksLoaded {
                title: "PL Mix".to_owned(),
                tracks: vec![Track::new("v1", "Track", "A", "Al", 100.0, "")],
            },
        );
        assert!(model.playlist.is_viewing_tracks());
    }

    #[test]
    fn enter_while_filtering_activates_filtered_row() {
        // With a filter active, Enter falls through to activation over the
        // filtered rows (plays the matching track), without closing the bar.
        let (runtime, mut rx) = RuntimeHandle::stub();
        let mut model = model_on_history();
        model.dispatch_key(key(KeyCode::Char('/')), &runtime);
        for ch in "daft".chars() {
            model.dispatch_key(key(KeyCode::Char(ch)), &runtime);
        }
        model.apply_filter_to_view();
        model.dispatch_key(key(KeyCode::Enter), &runtime);
        // The runtime received a PlayPlaylist for the single filtered track.
        let cmd = rx.try_recv().ok();
        assert!(
            matches!(cmd, Some(AppCommand::PlayPlaylist { ref tracks, .. }) if tracks.len() == 1 && tracks[0].video_id == "v2"),
            "Enter should play the filtered Daft Punk track, got {cmd:?}"
        );
    }

    // -- event folding -----------------------------------------------------

    /// Fold an event, discarding the (here-irrelevant) follow-up command.
    fn fold(model: &mut AppModel, event: AppEvent) {
        let _ = model.on_event(event);
    }

    fn track_section() -> HomeSection {
        HomeSection {
            title: "Quick picks".to_owned(),
            items: vec![HomeSectionItem::Track(Track::new(
                "vid1", "Song", "Artist", "", 100.0, "",
            ))],
        }
    }

    /// The `video_id` of an [`HomeAction::Play`], for terse id assertions.
    fn home_played_id(action: Option<HomeAction>) -> Option<String> {
        match action {
            Some(HomeAction::Play(track)) => Some(track.video_id),
            _ => None,
        }
    }

    #[test]
    fn home_loaded_event_populates_view() {
        let mut model = AppModel::new(Theme::default());
        fold(&mut model, AppEvent::HomeLoaded(vec![track_section()]));
        assert!(model.home.state().loaded().is_some());
        assert_eq!(
            home_played_id(model.home.activate_selected()),
            Some("vid1".to_owned())
        );
    }

    #[test]
    fn api_error_before_load_sets_error_and_toast() {
        let mut model = AppModel::new(Theme::default());
        fold(&mut model, AppEvent::ApiError("network down".to_owned()));
        // The inline home-view error body is set...
        assert_eq!(model.home.state().status_line(), Some("network down"));
        // ...and an error toast is raised.
        assert_eq!(latest_toast(&model), Some("network down"));
        assert_eq!(model.toasts.toasts()[0].severity, Severity::Error);
    }

    #[test]
    fn player_events_fold_into_bar_state() {
        let mut model = AppModel::new(Theme::default());
        fold(&mut model, AppEvent::PlayerStarted);
        fold(&mut model, AppEvent::PlayerDuration(200.0));
        fold(&mut model, AppEvent::PlayerProgress(50.0));
        assert!(model.player.has_track);
        assert!(model.player.is_playing);
        assert_eq!(model.player.duration, 200.0);
        assert_eq!(model.player.position, 50.0);
    }

    // -- auto-advance flow (the end-file battle lesson, at the UI layer) ----

    #[test]
    fn track_ended_clears_bar_and_requests_next_track() {
        // A natural EOF must (a) return the bar to idle and (b) ask the runtime
        // to advance the queue.
        let mut model = AppModel::new(Theme::default());
        fold(&mut model, AppEvent::PlayerStarted);
        let follow_up = model.on_event(AppEvent::TrackEnded);
        assert!(!model.player.has_track);
        assert!(!model.player.is_playing);
        assert_eq!(follow_up, Some(AppCommand::NextTrack));
    }

    #[test]
    fn track_error_never_requests_next_track() {
        // A failed stream must NOT advance the queue (a broken resolver would
        // machine-gun it). The fold returns no follow-up command.
        let mut model = AppModel::new(Theme::default());
        let follow_up = model.on_event(AppEvent::TrackError("loading failed".to_owned()));
        assert_eq!(follow_up, None, "TrackError must not advance the queue");
        assert!(has_toast_containing(&model, "loading failed"));
    }

    #[test]
    fn now_playing_event_fills_bar_metadata() {
        use ytmusic_tui::app::NowPlaying;
        use ytmusic_tui::queue::RepeatMode;
        let mut model = AppModel::new(Theme::default());
        fold(
            &mut model,
            AppEvent::NowPlaying(NowPlaying {
                title: "Around the World".to_owned(),
                artist: "Daft Punk".to_owned(),
                album: "Homework".to_owned(),
                video_id: "v1".to_owned(),
                duration_seconds: 425.0,
                shuffle: true,
                repeat: RepeatMode::All,
            }),
        );
        assert_eq!(model.player.title, "Around the World");
        assert_eq!(model.player.artist, "Daft Punk");
        assert_eq!(model.player.album, "Homework");
        assert_eq!(model.player.api_duration, 425.0);
        assert!(model.player.shuffle);
    }

    #[test]
    fn player_volume_event_corrects_bar_volume() {
        let mut model = AppModel::new(Theme::default());
        fold(&mut model, AppEvent::PlayerVolume(55));
        assert_eq!(model.player.volume, 55);
    }

    #[test]
    fn error_raises_a_toast_that_a_later_load_does_not_clear() {
        // Toasts auto-expire on their own timer; unlike the old single status
        // line, a subsequent successful load must NOT yank an in-flight error
        // toast off screen mid-read.
        let mut model = AppModel::new(Theme::default());
        fold(&mut model, AppEvent::TrackError("boom".to_owned()));
        assert!(has_toast_containing(&model, "boom"));
        // A successful home reload leaves the live toast in place (it expires by
        // its own timeout, not on the next event).
        fold(&mut model, AppEvent::HomeLoaded(vec![track_section()]));
        assert!(
            has_toast_containing(&model, "boom"),
            "a load should not clear a live error toast"
        );
    }

    #[test]
    fn home_fetch_error_marks_home_even_when_on_playlist_view() {
        // A FetchHome error landing after navigating to the playlist view must
        // still mark home as errored (not leave it stuck on Loading forever).
        let mut model = AppModel::new(Theme::default());
        model.view = View::Playlist;
        fold(&mut model, AppEvent::ApiError("network down".to_owned()));
        assert_eq!(model.home.state().status_line(), Some("network down"));
    }

    #[test]
    fn session_invalid_event_raises_warning_toast() {
        let mut model = AppModel::new(Theme::default());
        assert!(model.toasts.is_empty());
        fold(&mut model, AppEvent::SessionInvalid);
        assert!(has_toast_containing(&model, "Session invalid"));
        assert_eq!(model.toasts.toasts()[0].severity, Severity::Warning);
    }

    // -- rendering (TestBackend) -------------------------------------------

    /// Flatten a TestBackend buffer into one string for substring assertions.
    fn render_model(model: &AppModel, w: u16, h: u16) -> String {
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| model.render(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        let width = buffer.area().width as usize;
        let mut out = String::new();
        for (i, cell) in buffer.content().iter().enumerate() {
            out.push_str(cell.symbol());
            if (i + 1) % width == 0 {
                out.push('\n');
            }
        }
        out
    }

    #[test]
    fn render_shows_home_content_and_player_bar() {
        let mut model = AppModel::new(Theme::default());
        fold(&mut model, AppEvent::HomeLoaded(vec![track_section()]));
        let text = render_model(&model, 70, 20);
        // Home section title + item.
        assert!(text.contains("Quick picks"), "missing section:\n{text}");
        assert!(text.contains("Song"), "missing track:\n{text}");
        // Player bar (idle volume default).
        assert!(text.contains("Vol: 80"), "missing player bar:\n{text}");
    }

    #[test]
    fn render_fills_the_frame_with_the_theme_surface_background() {
        // Fix 2: the whole frame is painted with the theme surface color first,
        // so the terminal's default background never shows through. Sample the
        // top-left corner (above any view content) — it must carry surface bg.
        let model = AppModel::new(Theme::default());
        let backend = TestBackend::new(60, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| model.render(frame)).unwrap();
        let theme = Theme::default();
        let buffer = terminal.backend().buffer();
        assert_eq!(
            buffer[(0, 0)].style().bg,
            Some(theme.surface),
            "frame top-left is not painted with the theme surface background"
        );
    }

    #[test]
    fn render_draws_a_bordered_section_title_with_accent() {
        // A loaded home view draws each section in a bordered panel whose title
        // is the section name in the accent color (the Python panel-title CSS).
        let mut model = AppModel::new(Theme::default());
        fold(&mut model, AppEvent::HomeLoaded(vec![track_section()]));
        let backend = TestBackend::new(70, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| model.render(frame)).unwrap();
        let theme = Theme::default();
        let buffer = terminal.backend().buffer();
        // A border glyph somewhere carries the active section's primary border.
        let has_border = buffer.content().iter().any(|c| {
            (c.symbol() == "─" || c.symbol() == "│") && c.style().fg == Some(theme.primary)
        });
        assert!(has_border, "no accented/primary section border rendered");
        // The section title 'Q' (of "Quick picks") is drawn in the accent color.
        let has_accent_title = buffer
            .content()
            .iter()
            .any(|c| c.symbol() == "Q" && c.style().fg == Some(theme.accent));
        assert!(
            has_accent_title,
            "section title not drawn in the accent color"
        );
    }

    #[test]
    fn render_styles_the_selected_row_with_cursor_colors() {
        // The active section's selected row uses the cursor colors (primary bg).
        let mut model = AppModel::new(Theme::default());
        fold(&mut model, AppEvent::HomeLoaded(vec![track_section()]));
        let backend = TestBackend::new(70, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| model.render(frame)).unwrap();
        let theme = Theme::default();
        let buffer = terminal.backend().buffer();
        let has_cursor = buffer
            .content()
            .iter()
            .any(|c| c.style().bg == Some(theme.primary) && c.style().fg == Some(theme.background));
        assert!(
            has_cursor,
            "no row rendered with the selected-row cursor colors"
        );
    }

    #[test]
    fn render_shows_session_warning_as_toast() {
        let mut model = AppModel::new(Theme::default());
        fold(&mut model, AppEvent::SessionInvalid);
        // The session warning now renders as a floating toast rather than a
        // persistent header line; the text is still on screen.
        let text = render_model(&model, 80, 12);
        assert!(
            text.contains("Session invalid"),
            "missing session warning:\n{text}"
        );
        assert!(
            text.contains("ytmusic-tui auth"),
            "missing auth hint:\n{text}"
        );
    }

    #[test]
    fn render_shows_loading_state_before_data() {
        let model = AppModel::new(Theme::default());
        let text = render_model(&model, 60, 12);
        assert!(
            text.contains("Loading..."),
            "missing loading state:\n{text}"
        );
    }

    #[test]
    fn header_line_count_tracks_only_the_pending_key_sequence() {
        // Transient messages are toasts now and never grow the header; only an
        // armed key-sequence prefix takes a header row.
        let mut model = AppModel::new(Theme::default());
        assert_eq!(model.header_line_count(), 0);
        // Toasts do not affect the header height.
        fold(&mut model, AppEvent::SessionInvalid);
        fold(&mut model, AppEvent::TrackError("x".to_owned()));
        assert_eq!(model.header_line_count(), 0);
        // Arming a two-key sequence prefix (g …) takes one header row.
        let (runtime, _rx) = RuntimeHandle::stub();
        model.dispatch_key(key(KeyCode::Char('g')), &runtime);
        assert_eq!(model.header_line_count(), 1);
    }

    // -- view switching + navigation (M5b) ---------------------------------

    /// Build a model on the playlist view at the track-list level, with a
    /// playlist on the nav stack — the state after opening a home playlist.
    fn model_on_playlist_tracks() -> AppModel {
        let mut model = AppModel::new(Theme::default());
        model.view = View::Playlist;
        model
            .nav
            .push(NavPage::with_context("playlist", "playlist_id", "PL1"));
        model.playlist.show_track_list_loading("My Mix");
        model
    }

    #[test]
    fn playlist_tracks_loaded_event_fills_the_view() {
        let mut model = model_on_playlist_tracks();
        fold(
            &mut model,
            AppEvent::PlaylistTracksLoaded {
                title: "My Mix".to_owned(),
                tracks: vec![Track::new("v1", "First", "A", "Al", 100.0, "")],
            },
        );
        assert!(model.playlist.is_viewing_tracks());
        let text = render_model(&model, 60, 12);
        assert!(text.contains("First"), "missing track:\n{text}");
    }

    #[test]
    fn library_playlists_loaded_event_fills_the_view() {
        let mut model = AppModel::new(Theme::default());
        model.view = View::Playlist;
        fold(
            &mut model,
            AppEvent::LibraryPlaylistsLoaded(vec![PlaylistInfo::new("PL1", "My Mix", "", 25, "")]),
        );
        let text = render_model(&model, 60, 12);
        assert!(text.contains("My Mix"), "missing playlist:\n{text}");
        assert!(text.contains("playlist(s)"), "missing count:\n{text}");
    }

    #[test]
    fn esc_in_track_list_returns_to_playlist_list() {
        // Esc at level 2 of the playlist view pops back to level 1 (handled by
        // the view), staying on the Playlist view — it does NOT pop nav to home.
        let mut model = model_on_playlist_tracks();
        fold(
            &mut model,
            AppEvent::PlaylistTracksLoaded {
                title: "My Mix".to_owned(),
                tracks: vec![Track::new("v1", "First", "A", "Al", 100.0, "")],
            },
        );
        assert!(model.playlist.is_viewing_tracks());
        // The view consumes the Esc: still on Playlist, but at level 1.
        let handled = model.playlist.go_back();
        assert!(handled);
        assert_eq!(model.view, View::Playlist);
        assert!(!model.playlist.is_viewing_tracks());
    }

    #[test]
    fn esc_at_playlist_list_pops_nav_back_to_home() {
        // With the playlist at level 1, Esc pops the nav stack (playlist → home).
        let mut model = model_on_playlist_tracks();
        // Drop to level 1 first (view consumes the first Esc).
        assert!(model.playlist.go_back());
        // Now the next Esc pops nav: the previous page is home.
        let popped = model.nav.pop();
        assert_eq!(popped.map(|p| p.page_type), Some("home".to_owned()));
        // The page→view map (what switch_to_page applies) routes "home" → Home.
        assert_eq!(view_for_page("home"), View::Home);
        assert_eq!(view_for_page("playlist"), View::Playlist);
    }

    #[test]
    fn playlist_view_renders_when_active() {
        let model = model_on_playlist_tracks();
        // Loading state renders the per-level loading line.
        let text = render_model(&model, 60, 12);
        assert!(
            text.contains("Loading tracks for My Mix"),
            "missing playlist loading line:\n{text}"
        );
    }

    // -- Fix 1: stale artist/album on nav pop-back (M5b-2b review) ----------

    /// Simulate opening artist A, then navigating into artist B (related
    /// artist), then pressing Esc.  The model must reset the artist view to
    /// Loading and issue a FetchArtist for artist A's channel_id — not show
    /// artist B's already-loaded data.
    #[test]
    fn esc_from_related_artist_refetches_original_artist() {
        use ytmusic_api::ArtistInfo;
        use ytmusic_tui::app::AppCommand;

        let (runtime, mut cmd_rx) = RuntimeHandle::stub();

        let mut model = AppModel::new(Theme::default());

        // Step 1: navigate to artist A and simulate its data arriving.
        model.open_artist("channel_A".to_owned(), &runtime);
        // Drain the FetchArtist(channel_A) command that open_artist sent.
        let _ = cmd_rx.try_recv();

        fold(
            &mut model,
            AppEvent::ArtistLoaded(ArtistInfo::new(
                "channel_A",
                "Artist A",
                "",
                vec![],
                vec![],
                vec![],
                "",
            )),
        );
        assert_eq!(model.view, View::Artist);

        // Step 2: navigate to related artist B (open_artist pushes a new entry).
        model.open_artist("channel_B".to_owned(), &runtime);
        // Drain the FetchArtist(channel_B) command.
        let _ = cmd_rx.try_recv();

        fold(
            &mut model,
            AppEvent::ArtistLoaded(ArtistInfo::new(
                "channel_B",
                "Artist B",
                "",
                vec![],
                vec![],
                vec![],
                "",
            )),
        );

        // Step 3: press Esc — should pop back to artist A, reset view to
        // Loading, and issue FetchArtist("channel_A").
        model.go_back(&runtime);

        assert_eq!(model.view, View::Artist, "still on Artist view after Esc");
        assert!(
            matches!(model.artist.state(), ytmusic_tui::views::PageState::Loading),
            "artist view should be reset to Loading after pop-back"
        );
        let cmd = cmd_rx
            .try_recv()
            .expect("FetchArtist command must be issued on pop-back");
        assert_eq!(
            cmd,
            AppCommand::FetchArtist("channel_A".to_owned()),
            "must re-fetch artist A, not artist B"
        );
    }

    // -- Fix 2: NowPlaying triggers FetchQueue when queue view is open -------

    /// While the queue view is active, a NowPlaying event must return
    /// Some(FetchQueue) so the ▶ marker re-snapshots after auto-advance.
    #[test]
    fn now_playing_while_queue_view_open_returns_fetch_queue() {
        use ytmusic_tui::app::{AppCommand, NowPlaying};
        use ytmusic_tui::queue::RepeatMode;

        let mut model = AppModel::new(Theme::default());
        model.view = View::Queue;

        let follow_up = model.on_event(AppEvent::NowPlaying(NowPlaying {
            title: "Song".to_owned(),
            artist: "Band".to_owned(),
            album: "LP".to_owned(),
            video_id: "v1".to_owned(),
            duration_seconds: 200.0,
            shuffle: false,
            repeat: RepeatMode::Off,
        }));
        assert_eq!(
            follow_up,
            Some(AppCommand::FetchQueue),
            "NowPlaying while Queue view is open must return FetchQueue"
        );
    }

    /// While a non-queue view is active, NowPlaying must NOT return FetchQueue.
    #[test]
    fn now_playing_while_home_view_open_returns_none() {
        use ytmusic_tui::app::NowPlaying;
        use ytmusic_tui::queue::RepeatMode;

        let mut model = AppModel::new(Theme::default());
        // Default view is Home; no explicit assignment needed, but be explicit.
        model.view = View::Home;

        let follow_up = model.on_event(AppEvent::NowPlaying(NowPlaying {
            title: "Song".to_owned(),
            artist: "Band".to_owned(),
            album: "LP".to_owned(),
            video_id: "v2".to_owned(),
            duration_seconds: 150.0,
            shuffle: false,
            repeat: RepeatMode::Off,
        }));
        assert_eq!(
            follow_up, None,
            "NowPlaying while Home view is open must not return FetchQueue"
        );
    }
}
