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
//! # Keymap (M5c note)
//!
//! Keys are hard-coded here to the [`DEFAULT_KEYMAP`] values
//! (`ytmusic_tui::config::DEFAULT_KEYMAP`). Full `keymap.toml` dispatch — load
//! the merged keymap and resolve actions by name — is deferred to M5c; this
//! milestone wires the fixed default bindings only.

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
use ratatui::widgets::Paragraph;

use ytmusic_api::{BrowserAuth, InnerTubeClient};
use ytmusic_tui::app::{AppCommand, AppEvent, RuntimeHandle, spawn_runtime};
use ytmusic_tui::config::{self, AppConfig};
use ytmusic_tui::navigation::{NavigationManager, PageState as NavPage};
use ytmusic_tui::player::Player;
use ytmusic_tui::views::Theme;
use ytmusic_tui::views::home::{HomeAction, HomeView};
use ytmusic_tui::views::player_bar::{PLAYER_BAR_HEIGHT, PlayerBar, PlayerBarState};
use ytmusic_tui::views::playlist::{PlaylistAction, PlaylistView};

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
    let mut model = AppModel::new(Theme::from_name(&config.ui.theme));

    while !model.should_quit {
        // Drain events that arrived since the last tick before drawing, so the
        // frame reflects the latest player position / loaded data.
        drain_events(events, &mut model, runtime);

        terminal.draw(|frame| model.render(frame))?;

        // Block up to POLL_INTERVAL for input; on timeout, loop to redraw
        // (the player bar advances from drained progress events).
        if event::poll(POLL_INTERVAL)?
            && let Event::Key(key) = event::read()?
        {
            // Only react to presses (Windows also emits Release/Repeat).
            if key.kind == KeyEventKind::Press
                && let Some(action) = map_key(key)
            {
                model.apply(action, runtime);
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

/// A high-level UI action, decoded from a key event.
///
/// Mapping keys to this enum (rather than acting on the raw key) keeps the
/// dispatch table pure and testable. Bindings match [`config::DEFAULT_KEYMAP`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    /// Quit the app (`q` / `Q`). DEFAULT_KEYMAP maps `quit = "Q"` and
    /// `switch_queue = "q"`. Until M5c wires the full keymap, lowercase `q`
    /// also quits so the app is quittable before the queue view exists; the
    /// keymap dispatcher supersedes this.
    Quit,
    /// Toggle play/pause (`space`).
    TogglePause,
    /// Volume up (`+` / `=` — DEFAULT_KEYMAP `volume_up = "plus,equal"`).
    VolumeUp,
    /// Volume down (`-` — DEFAULT_KEYMAP `volume_down = "minus"`).
    VolumeDown,
    /// Move to the next home section (`Tab`).
    NextSection,
    /// Move to the previous home section (`Shift+Tab`).
    PreviousSection,
    /// Move the selection down within a section (`Down` / `j`).
    SelectNext,
    /// Move the selection up within a section (`Up` / `k`).
    SelectPrevious,
    /// Activate the current selection (`Enter`).
    Activate,
    /// Skip to the next queued track (DEFAULT_KEYMAP `next_track = "n"`).
    NextTrack,
    /// Go back to the previous track (DEFAULT_KEYMAP `previous_track = "p"`).
    PreviousTrack,
    /// Toggle shuffle on the queue (DEFAULT_KEYMAP `toggle_shuffle = "s"`).
    ToggleShuffle,
    /// Cycle the repeat mode (DEFAULT_KEYMAP `cycle_repeat = "r"`).
    CycleRepeat,
    /// Go back / pop the navigation stack (DEFAULT_KEYMAP `go_back = "escape"`).
    GoBack,
}

/// Per-press volume step, matching the Python `volume_up` / `volume_down`
/// actions which nudged by 5.
const VOLUME_STEP: i64 = 5;

/// Decode a key event into an [`Action`], or `None` if unbound.
///
/// Bindings are the [`config::DEFAULT_KEYMAP`] values; full keymap.toml dispatch
/// is M5c.
fn map_key(key: KeyEvent) -> Option<Action> {
    match key.code {
        // `Q` is the canonical quit (DEFAULT_KEYMAP); `q` is also accepted.
        KeyCode::Char('q' | 'Q') => Some(Action::Quit),
        KeyCode::Char(' ') => Some(Action::TogglePause),
        KeyCode::Char('+' | '=') => Some(Action::VolumeUp),
        KeyCode::Char('-') => Some(Action::VolumeDown),
        // Transport keys (DEFAULT_KEYMAP: n/p/s/r). Hardcoded until M5c wires
        // the merged keymap.toml.
        KeyCode::Char('n') => Some(Action::NextTrack),
        KeyCode::Char('p') => Some(Action::PreviousTrack),
        KeyCode::Char('s') => Some(Action::ToggleShuffle),
        KeyCode::Char('r') => Some(Action::CycleRepeat),
        KeyCode::Esc => Some(Action::GoBack),
        KeyCode::Char('j') => Some(Action::SelectNext),
        KeyCode::Char('k') => Some(Action::SelectPrevious),
        KeyCode::Down => Some(Action::SelectNext),
        KeyCode::Up => Some(Action::SelectPrevious),
        // crossterm reports Shift+Tab as BackTab; also accept Tab+SHIFT.
        KeyCode::BackTab => Some(Action::PreviousSection),
        KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => {
            Some(Action::PreviousSection)
        }
        KeyCode::Tab => Some(Action::NextSection),
        KeyCode::Enter => Some(Action::Activate),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// AppModel — view state + event folding + action application (pure-ish)
// ---------------------------------------------------------------------------

/// Which content view is active (the M5b minimal view switch; M5c/M5b-2 add the
/// rest). The home and playlist views each keep their own state; this enum just
/// records which one renders and receives navigation keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    /// The home recommendations view (the startup view).
    Home,
    /// The two-level playlist browser.
    Playlist,
}

/// Map a navigation page type (the `page_type` of a [`NavPage`]) to its content
/// [`View`]. Kept pure so the page→view mapping is unit-testable without a
/// `RuntimeHandle`. Unknown / not-yet-ported pages fall back to [`View::Home`].
fn view_for_page(page_type: &str) -> View {
    match page_type {
        "playlist" => View::Playlist,
        _ => View::Home,
    }
}

/// The full UI state for the M5b home + playlist + player-bar surface.
///
/// Owns the view widgets' state, the navigation stack, and the quit flag.
/// Methods are split so the pure parts (event folding, navigation/key
/// application) are unit-testable; only [`AppModel::apply`] for playback actions
/// needs a `RuntimeHandle`, which tests exercise via a `None` runtime path where
/// it only mutates view state.
struct AppModel {
    home: HomeView,
    playlist: PlaylistView,
    player: PlayerBarState,
    /// The page history stack (home ↔ playlist); Esc pops back.
    nav: NavigationManager,
    /// The active content view.
    view: View,
    theme: Theme,
    /// Set once the session canary reports an invalid (logged-out) session;
    /// renders a one-line warning above the content.
    session_warning: Option<String>,
    /// A transient status line (e.g. an API error or a "not yet implemented"
    /// note for unported actions). Shown under the warning when present.
    status: Option<String>,
    should_quit: bool,
}

/// The warning shown when the session canary fails.
const SESSION_WARNING: &str = "Session invalid — desktop sign-in expired. Run: ytmusic-tui auth";

impl AppModel {
    fn new(theme: Theme) -> Self {
        Self {
            home: HomeView::new(),
            playlist: PlaylistView::new(),
            player: PlayerBarState::default(),
            nav: NavigationManager::new(NavPage::new("home")),
            view: View::Home,
            theme,
            session_warning: None,
            status: None,
            should_quit: false,
        }
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
                self.status = None; // a successful load clears any stale error
            }
            AppEvent::LibraryPlaylistsLoaded(playlists) => {
                self.playlist.set_playlists(playlists);
                self.status = None;
            }
            AppEvent::PlaylistTracksLoaded { title, tracks } => {
                self.playlist.set_tracks(title, tracks);
                self.status = None;
            }
            AppEvent::ApiError(msg) => self.on_api_error(msg),
            AppEvent::NowPlaying(now) => {
                self.player.on_now_playing(
                    now.title,
                    now.artist,
                    now.album,
                    now.duration_seconds,
                    now.shuffle,
                    now.repeat.into(),
                );
            }
            AppEvent::PlayerProgress(secs) => self.player.on_progress(secs),
            AppEvent::PlayerDuration(secs) => self.player.on_duration(secs),
            AppEvent::PlayerVolume(vol) => self.player.on_volume(vol),
            AppEvent::PlayerStarted => self.player.on_started(),
            AppEvent::TrackEnded => {
                // Natural EOF: clear the bar to idle now, and ask the runtime to
                // advance the queue. The runtime replies with NowPlaying +
                // PlayerStarted for the next track (or an idle NowPlaying at the
                // end of the queue) on the following ticks.
                self.player.on_track_ended();
                return Some(AppCommand::NextTrack);
            }
            AppEvent::TrackError(detail) => {
                // NEVER advance on a failed stream — a broken resolver would
                // machine-gun the queue (the end-file battle lesson).
                self.status = Some(format!("Playback error: {detail}"));
            }
            AppEvent::SessionInvalid => {
                self.session_warning = Some(SESSION_WARNING.to_owned());
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
        if self.view == View::Playlist {
            self.playlist.set_error(msg.clone());
        }
        self.status = Some(msg);
    }

    /// Apply a decoded action, issuing runtime commands for playback/navigation
    /// actions and mutating the active view's cursor for selection actions.
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
            // Section moves only apply to the home view (the playlist view is a
            // single flat list, so Tab/Shift-Tab are inert there).
            Action::NextSection => {
                if self.view == View::Home {
                    self.home.focus_next_section();
                }
            }
            Action::PreviousSection => {
                if self.view == View::Home {
                    self.home.focus_previous_section();
                }
            }
            Action::SelectNext => self.select_next(),
            Action::SelectPrevious => self.select_previous(),
            Action::Activate => self.activate(runtime),
            Action::GoBack => self.go_back(runtime),
        }
    }

    /// Move the cursor down in the active view.
    fn select_next(&mut self) {
        match self.view {
            View::Home => self.home.select_next_item(),
            View::Playlist => self.playlist.select_next(),
        }
    }

    /// Move the cursor up in the active view.
    fn select_previous(&mut self) {
        match self.view {
            View::Home => self.home.select_previous_item(),
            View::Playlist => self.playlist.select_previous(),
        }
    }

    /// Handle Enter, dispatched to the active view.
    fn activate(&mut self, runtime: &RuntimeHandle) {
        match self.view {
            View::Home => self.activate_home(runtime),
            View::Playlist => self.activate_playlist(runtime),
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

    /// Switch to the playlist view, drilling into `info` and pushing the nav
    /// stack (home → playlist) so Esc can pop back. The level-1 library list is
    /// fetched eagerly too, so a subsequent Esc-to-level-1 has data to show.
    fn open_playlist(&mut self, info: ytmusic_api::PlaylistInfo, runtime: &RuntimeHandle) {
        self.view = View::Playlist;
        self.nav.push(NavPage::with_context(
            "playlist",
            "playlist_id",
            &info.playlist_id,
        ));
        // Prime the level-1 list (for a later Esc) and drill into level 2.
        runtime.send(AppCommand::FetchLibraryPlaylists);
        self.playlist.show_track_list_loading(&info.title);
        runtime.send(AppCommand::FetchPlaylistTracks {
            playlist_id: info.playlist_id,
            title: info.title,
        });
    }

    /// Handle Esc / go-back. Inside the playlist view's track list it pops back
    /// to the playlist list (consumed by the view); otherwise it pops the
    /// navigation stack to the previous page (playlist → home).
    fn go_back(&mut self, runtime: &RuntimeHandle) {
        // Level 2 → level 1 is the view's own concern (Python `on_key` Escape).
        if self.view == View::Playlist && self.playlist.go_back() {
            // The view reset its level-1 list to Loading; re-fetch it.
            runtime.send(AppCommand::FetchLibraryPlaylists);
            return;
        }
        // Otherwise pop the page stack (playlist → home).
        if let Some(page) = self.nav.pop() {
            self.switch_to_page(&page.page_type, runtime);
        }
    }

    /// Switch the active view to match a popped navigation page.
    ///
    /// `runtime` is reserved for pages whose data must be re-fetched on return;
    /// the M5b pages (home/playlist) keep their loaded state, so it is currently
    /// unused beyond documenting the seam for M5b-2's pages.
    fn switch_to_page(&mut self, page_type: &str, runtime: &RuntimeHandle) {
        self.view = view_for_page(page_type);
        let _ = runtime;
    }

    /// Draw the whole UI: optional warning + status lines, the home content,
    /// and the player bar docked at the bottom.
    fn render(&self, frame: &mut ratatui::Frame<'_>) {
        let area = frame.area();
        let header_lines = self.header_line_count();
        let chunks = Layout::vertical([
            Constraint::Length(header_lines),
            Constraint::Min(1),
            Constraint::Length(PLAYER_BAR_HEIGHT),
        ])
        .split(area);

        self.render_header(frame, chunks[0]);
        match self.view {
            View::Home => self.home.render(frame, chunks[1], &self.theme),
            View::Playlist => self.playlist.render(frame, chunks[1], &self.theme),
        }
        PlayerBar.render(frame, chunks[2], &self.player, &self.theme);
    }

    /// Number of header rows currently needed (warning and/or status).
    fn header_line_count(&self) -> u16 {
        u16::from(self.session_warning.is_some()) + u16::from(self.status.is_some())
    }

    /// Render the warning and status lines into the header area.
    fn render_header(&self, frame: &mut ratatui::Frame<'_>, area: Rect) {
        if area.height == 0 {
            return;
        }
        let mut lines: Vec<Line> = Vec::new();
        if let Some(warning) = &self.session_warning {
            lines.push(Line::from(Span::styled(
                warning.clone(),
                Style::default()
                    .fg(self.theme.primary)
                    .add_modifier(Modifier::BOLD),
            )));
        }
        if let Some(status) = &self.status {
            lines.push(Line::from(Span::styled(
                status.clone(),
                Style::default().fg(self.theme.secondary),
            )));
        }
        frame.render_widget(Paragraph::new(lines), area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ytmusic_api::{HomeSection, HomeSectionItem, PlaylistInfo, Track};

    // -- key mapping -------------------------------------------------------

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn quit_keys_map_to_quit() {
        assert_eq!(map_key(key(KeyCode::Char('q'))), Some(Action::Quit));
        assert_eq!(map_key(key(KeyCode::Char('Q'))), Some(Action::Quit));
    }

    #[test]
    fn space_maps_to_toggle_pause() {
        assert_eq!(map_key(key(KeyCode::Char(' '))), Some(Action::TogglePause));
    }

    #[test]
    fn volume_keys_map() {
        assert_eq!(map_key(key(KeyCode::Char('+'))), Some(Action::VolumeUp));
        assert_eq!(map_key(key(KeyCode::Char('='))), Some(Action::VolumeUp));
        assert_eq!(map_key(key(KeyCode::Char('-'))), Some(Action::VolumeDown));
    }

    #[test]
    fn navigation_keys_map() {
        assert_eq!(map_key(key(KeyCode::Down)), Some(Action::SelectNext));
        assert_eq!(map_key(key(KeyCode::Char('j'))), Some(Action::SelectNext));
        assert_eq!(map_key(key(KeyCode::Up)), Some(Action::SelectPrevious));
        assert_eq!(
            map_key(key(KeyCode::Char('k'))),
            Some(Action::SelectPrevious)
        );
        assert_eq!(map_key(key(KeyCode::Tab)), Some(Action::NextSection));
        assert_eq!(
            map_key(key(KeyCode::BackTab)),
            Some(Action::PreviousSection)
        );
        assert_eq!(map_key(key(KeyCode::Enter)), Some(Action::Activate));
    }

    #[test]
    fn shift_tab_via_modifier_maps_to_previous_section() {
        let shift_tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT);
        assert_eq!(map_key(shift_tab), Some(Action::PreviousSection));
    }

    #[test]
    fn transport_keys_map() {
        assert_eq!(map_key(key(KeyCode::Char('n'))), Some(Action::NextTrack));
        assert_eq!(
            map_key(key(KeyCode::Char('p'))),
            Some(Action::PreviousTrack)
        );
        assert_eq!(
            map_key(key(KeyCode::Char('s'))),
            Some(Action::ToggleShuffle)
        );
        assert_eq!(map_key(key(KeyCode::Char('r'))), Some(Action::CycleRepeat));
        assert_eq!(map_key(key(KeyCode::Esc)), Some(Action::GoBack));
    }

    #[test]
    fn unbound_key_maps_to_none() {
        assert_eq!(map_key(key(KeyCode::Char('z'))), None);
        assert_eq!(map_key(key(KeyCode::Char('x'))), None);
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
    fn api_error_before_load_sets_error_and_status() {
        let mut model = AppModel::new(Theme::default());
        fold(&mut model, AppEvent::ApiError("network down".to_owned()));
        assert_eq!(model.home.state().status_line(), Some("network down"));
        assert_eq!(model.status.as_deref(), Some("network down"));
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
        assert!(
            model
                .status
                .as_deref()
                .is_some_and(|s| s.contains("loading failed"))
        );
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
    fn successful_load_clears_stale_status_error() {
        let mut model = AppModel::new(Theme::default());
        // A prior error left a status line behind.
        fold(&mut model, AppEvent::TrackError("boom".to_owned()));
        assert!(model.status.is_some());
        // A successful home reload clears it.
        fold(&mut model, AppEvent::HomeLoaded(vec![track_section()]));
        assert!(model.status.is_none(), "stale status should clear on load");
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
    fn session_invalid_event_sets_warning() {
        let mut model = AppModel::new(Theme::default());
        assert!(model.session_warning.is_none());
        fold(&mut model, AppEvent::SessionInvalid);
        assert!(model.session_warning.is_some());
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
    fn render_shows_session_warning_line() {
        let mut model = AppModel::new(Theme::default());
        fold(&mut model, AppEvent::SessionInvalid);
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
    fn header_line_count_tracks_warning_and_status() {
        let mut model = AppModel::new(Theme::default());
        assert_eq!(model.header_line_count(), 0);
        fold(&mut model, AppEvent::SessionInvalid);
        assert_eq!(model.header_line_count(), 1);
        fold(&mut model, AppEvent::TrackError("x".to_owned()));
        assert_eq!(model.header_line_count(), 2);
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
}
