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
use ytmusic_tui::player::Player;
use ytmusic_tui::views::Theme;
use ytmusic_tui::views::home::{HomeAction, HomeView};
use ytmusic_tui::views::player_bar::{PLAYER_BAR_HEIGHT, PlayerBar, PlayerBarState};

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
        drain_events(events, &mut model);

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

/// Drain all currently-available events into the model (non-blocking).
fn drain_events(events: &std::sync::mpsc::Receiver<AppEvent>, model: &mut AppModel) {
    while let Ok(event) = events.try_recv() {
        model.on_event(event);
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

/// The full UI state for the M5a home + player-bar surface.
///
/// Owns the view widgets' state and the quit flag. Methods are split so the
/// pure parts (event folding, key application driving only the home cursor) are
/// unit-testable; only [`AppModel::apply`] for playback actions needs a
/// `RuntimeHandle`, which is trivially constructible-free in tests by exercising
/// the cursor actions instead.
struct AppModel {
    home: HomeView,
    player: PlayerBarState,
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
            player: PlayerBarState::default(),
            theme,
            session_warning: None,
            status: None,
            should_quit: false,
        }
    }

    /// Fold one runtime event into the model.
    fn on_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::HomeLoaded(sections) => self.home.set_sections(sections),
            AppEvent::ApiError(msg) => {
                // If the home view never loaded, show the error in its body;
                // either way surface it on the status line too.
                if self.home.state().loaded().is_none() {
                    self.home.set_error(msg.clone());
                }
                self.status = Some(msg);
            }
            AppEvent::PlayerProgress(secs) => self.player.on_progress(secs),
            AppEvent::PlayerDuration(secs) => self.player.on_duration(secs),
            AppEvent::PlayerStarted => self.player.on_started(),
            AppEvent::TrackEnded => self.player.on_track_ended(),
            AppEvent::TrackError(detail) => self.status = Some(format!("Playback error: {detail}")),
            AppEvent::SessionInvalid => {
                self.session_warning = Some(SESSION_WARNING.to_owned());
            }
        }
    }

    /// Apply a decoded action, issuing runtime commands for playback actions and
    /// mutating the home cursor for navigation actions.
    fn apply(&mut self, action: Action, runtime: &RuntimeHandle) {
        match action {
            Action::Quit => self.should_quit = true,
            Action::TogglePause => {
                runtime.send(AppCommand::TogglePause);
            }
            Action::VolumeUp => {
                self.player.volume = (self.player.volume + VOLUME_STEP).clamp(0, 100);
                runtime.send(AppCommand::AdjustVolume(VOLUME_STEP));
            }
            Action::VolumeDown => {
                self.player.volume = (self.player.volume - VOLUME_STEP).clamp(0, 100);
                runtime.send(AppCommand::AdjustVolume(-VOLUME_STEP));
            }
            Action::NextSection => self.home.focus_next_section(),
            Action::PreviousSection => self.home.focus_previous_section(),
            Action::SelectNext => self.home.select_next_item(),
            Action::SelectPrevious => self.home.select_previous_item(),
            Action::Activate => self.activate(runtime),
        }
    }

    /// Handle Enter: play a track, or note that playlist navigation is pending.
    fn activate(&mut self, runtime: &RuntimeHandle) {
        match self.home.activate_selected() {
            Some(HomeAction::Play(video_id)) => {
                self.player.on_started();
                runtime.send(AppCommand::Play(video_id));
            }
            Some(HomeAction::OpenPlaylist(_)) => {
                // Playlist view is an M5b target; acknowledge instead of a
                // silent no-op so the keypress is not perceived as broken.
                self.status = Some("Playlist view is not available yet".to_owned());
            }
            None => {}
        }
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
        self.home.render(frame, chunks[1], &self.theme);
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
    use ytmusic_api::{HomeSection, HomeSectionItem, Track};

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
    fn unbound_key_maps_to_none() {
        assert_eq!(map_key(key(KeyCode::Char('z'))), None);
        assert_eq!(map_key(key(KeyCode::Esc)), None);
    }

    // -- event folding -----------------------------------------------------

    fn track_section() -> HomeSection {
        HomeSection {
            title: "Quick picks".to_owned(),
            items: vec![HomeSectionItem::Track(Track::new(
                "vid1", "Song", "Artist", "", 100.0, "",
            ))],
        }
    }

    #[test]
    fn home_loaded_event_populates_view() {
        let mut model = AppModel::new(Theme::default());
        model.on_event(AppEvent::HomeLoaded(vec![track_section()]));
        assert!(model.home.state().loaded().is_some());
        assert_eq!(
            model.home.activate_selected(),
            Some(HomeAction::Play("vid1".to_owned()))
        );
    }

    #[test]
    fn api_error_before_load_sets_error_and_status() {
        let mut model = AppModel::new(Theme::default());
        model.on_event(AppEvent::ApiError("network down".to_owned()));
        assert_eq!(model.home.state().status_line(), Some("network down"));
        assert_eq!(model.status.as_deref(), Some("network down"));
    }

    #[test]
    fn player_events_fold_into_bar_state() {
        let mut model = AppModel::new(Theme::default());
        model.on_event(AppEvent::PlayerStarted);
        model.on_event(AppEvent::PlayerDuration(200.0));
        model.on_event(AppEvent::PlayerProgress(50.0));
        assert!(model.player.has_track);
        assert!(model.player.is_playing);
        assert_eq!(model.player.duration, 200.0);
        assert_eq!(model.player.position, 50.0);
    }

    #[test]
    fn track_ended_event_clears_bar() {
        let mut model = AppModel::new(Theme::default());
        model.on_event(AppEvent::PlayerStarted);
        model.on_event(AppEvent::TrackEnded);
        assert!(!model.player.has_track);
        assert!(!model.player.is_playing);
    }

    #[test]
    fn track_error_event_sets_status() {
        let mut model = AppModel::new(Theme::default());
        model.on_event(AppEvent::TrackError("loading failed".to_owned()));
        assert!(
            model
                .status
                .as_deref()
                .is_some_and(|s| s.contains("loading failed"))
        );
    }

    #[test]
    fn session_invalid_event_sets_warning() {
        let mut model = AppModel::new(Theme::default());
        assert!(model.session_warning.is_none());
        model.on_event(AppEvent::SessionInvalid);
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
        model.on_event(AppEvent::HomeLoaded(vec![track_section()]));
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
        model.on_event(AppEvent::SessionInvalid);
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
        model.on_event(AppEvent::SessionInvalid);
        assert_eq!(model.header_line_count(), 1);
        model.on_event(AppEvent::TrackError("x".to_owned()));
        assert_eq!(model.header_line_count(), 2);
    }
}
