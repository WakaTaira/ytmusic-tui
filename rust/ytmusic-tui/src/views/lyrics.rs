//! Lyrics display view.
//!
//! Port of `src/ytmusic_tui/views/lyrics.py`. Fetches lyrics for the current
//! track via `get_lyrics(video_id)` and renders them as scrollable text.
//! "No lyrics available" is a valid loaded state (`None` from the API), not an
//! error — the API returns `None` for tracks that have no lyrics associated.
//!
//! # Fetch flow vs Python
//!
//! Python's `LyricsView.load_lyrics(video_id)` ran a Textual worker. Here the
//! view is a pure value; the main loop issues
//! [`crate::app::AppCommand::FetchLyrics`] with the current `video_id` from
//! the player bar and folds [`crate::app::AppEvent::LyricsLoaded`] back. The
//! `Option<String>` payload encodes the "no lyrics" state directly.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use super::{PageState, Theme};

/// The lyrics view: a fetch state whose payload is `Option<String>` to
/// represent "loaded but no lyrics available" as a first-class value.
///
/// `PageState::Loaded(Some(text))` — lyrics to display.
/// `PageState::Loaded(None)` — API confirmed no lyrics for this track.
/// `PageState::Loading` — fetch in flight.
/// `PageState::Error` — API error.
#[derive(Debug, Clone)]
pub struct LyricsView {
    /// Track title + artist shown in the header (e.g. `"Song - Artist"`).
    header: String,
    /// Fetch state; the payload is `None` for "no lyrics available".
    state: PageState<Option<String>>,
    /// Vertical scroll offset (in lines).
    scroll: u16,
}

impl Default for LyricsView {
    fn default() -> Self {
        Self::new()
    }
}

impl LyricsView {
    /// A fresh lyrics view in the loading state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            header: String::new(),
            state: PageState::Loading,
            scroll: 0,
        }
    }

    /// The current fetch state (for tests and the main loop).
    #[must_use]
    pub fn state(&self) -> &PageState<Option<String>> {
        &self.state
    }

    // -- Data loading --------------------------------------------------------

    /// Set the track header (`"Title - Artist"`) and reset to loading when a
    /// new lyrics fetch starts.
    pub fn start_loading(&mut self, header: impl Into<String>) {
        self.header = header.into();
        self.state = PageState::Loading;
        self.scroll = 0;
    }

    /// Load the lyrics result. `None` means "no lyrics available" (the API
    /// returned `None` — a valid response, not an error).
    pub fn set_lyrics(&mut self, lyrics: Option<String>) {
        self.state = PageState::Loaded(lyrics);
        self.scroll = 0;
    }

    /// Transition into the error state with a classified message.
    pub fn set_error(&mut self, message: impl Into<String>) {
        self.state = PageState::Error(message.into());
    }

    // -- Scroll navigation ---------------------------------------------------

    /// Scroll the lyrics down one line.
    pub fn scroll_down(&mut self) {
        self.scroll = self.scroll.saturating_add(1);
    }

    /// Scroll the lyrics up one line.
    pub fn scroll_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(1);
    }

    // -- Rendering -----------------------------------------------------------

    /// Render the lyrics view into `area`.
    pub fn render(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        // Title header (1 row) + status / lyrics body.
        let chunks = Layout::vertical([
            Constraint::Length(1), // header
            Constraint::Min(1),    // body
        ])
        .split(area);

        // Header: track name + artist in accent color (matches Python's
        // `#lyrics-title` which updates to `"{title} - {artist}" or "Lyrics"`).
        let header_text = if self.header.is_empty() {
            "Lyrics".to_owned()
        } else {
            self.header.clone()
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                header_text,
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ))),
            chunks[0],
        );

        self.render_body(frame, chunks[1], theme);
    }

    fn render_body(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        match &self.state {
            PageState::Loading => {
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        "Loading lyrics...",
                        Style::default()
                            .fg(theme.text_muted)
                            .add_modifier(Modifier::ITALIC),
                    ))),
                    area,
                );
            }
            PageState::Error(msg) => {
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        msg.clone(),
                        Style::default().fg(theme.primary),
                    ))),
                    area,
                );
            }
            PageState::Loaded(None) => {
                // "No lyrics available" — API confirmed absence. Python:
                // `if not text: self._set_status("No lyrics available")`.
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        "No lyrics available",
                        Style::default()
                            .fg(theme.text_muted)
                            .add_modifier(Modifier::ITALIC),
                    ))),
                    area,
                );
            }
            PageState::Loaded(Some(text)) => {
                // Render the lyrics as a scrollable paragraph with word wrap.
                let block = Block::default().borders(Borders::NONE);
                let paragraph = Paragraph::new(text.as_str())
                    .block(block)
                    .style(Style::default().fg(theme.text))
                    .wrap(Wrap { trim: false })
                    .scroll((self.scroll, 0));
                frame.render_widget(paragraph, area);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render_to_string(view: &LyricsView, w: u16, h: u16) -> String {
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = Theme::default();
        terminal
            .draw(|frame| view.render(frame, frame.area(), &theme))
            .unwrap();
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

    // -- initial state -------------------------------------------------------

    #[test]
    fn new_view_is_loading() {
        let view = LyricsView::new();
        assert!(matches!(view.state(), PageState::Loading));
        let text = render_to_string(&view, 60, 8);
        assert!(text.contains("Loading lyrics..."), "text:\n{text}");
    }

    // -- start_loading -------------------------------------------------------

    #[test]
    fn start_loading_sets_header_and_resets_state() {
        let mut view = LyricsView::new();
        view.set_lyrics(Some("some lyrics".to_owned()));
        view.scroll_down();
        view.start_loading("Song - Artist");
        assert_eq!(view.header, "Song - Artist");
        assert!(matches!(view.state(), PageState::Loading));
        assert_eq!(view.scroll, 0);
    }

    // -- set_lyrics ----------------------------------------------------------

    #[test]
    fn set_lyrics_some_loads_lyrics_text() {
        let mut view = LyricsView::new();
        view.set_lyrics(Some("Never gonna give you up".to_owned()));
        assert!(matches!(view.state(), PageState::Loaded(Some(_))));
    }

    #[test]
    fn set_lyrics_none_is_no_lyrics_state() {
        let mut view = LyricsView::new();
        view.set_lyrics(None);
        // None is the "no lyrics available" state, not an error.
        assert!(matches!(view.state(), PageState::Loaded(None)));
    }

    // -- scroll navigation ---------------------------------------------------

    #[test]
    fn scroll_down_increments_and_scroll_up_decrements() {
        let mut view = LyricsView::new();
        view.scroll_down();
        view.scroll_down();
        assert_eq!(view.scroll, 2);
        view.scroll_up();
        assert_eq!(view.scroll, 1);
    }

    #[test]
    fn scroll_up_clamps_at_zero() {
        let mut view = LyricsView::new();
        view.scroll_up();
        assert_eq!(view.scroll, 0);
    }

    // -- rendering -----------------------------------------------------------

    #[test]
    fn loading_render_shows_loading_label() {
        let view = LyricsView::new();
        let text = render_to_string(&view, 60, 8);
        assert!(text.contains("Loading lyrics..."), "text:\n{text}");
    }

    #[test]
    fn no_lyrics_render_shows_unavailable_message() {
        let mut view = LyricsView::new();
        view.set_lyrics(None);
        let text = render_to_string(&view, 60, 8);
        assert!(
            text.contains("No lyrics available"),
            "missing no-lyrics message:\n{text}"
        );
    }

    #[test]
    fn lyrics_render_shows_text() {
        let mut view = LyricsView::new();
        view.start_loading("Karma Police - Radiohead");
        view.set_lyrics(Some("Arrest this man\nHe talks in maths".to_owned()));
        let text = render_to_string(&view, 60, 10);
        assert!(text.contains("Karma Police"), "missing header:\n{text}");
        assert!(
            text.contains("Arrest this man"),
            "missing lyrics text:\n{text}"
        );
    }

    #[test]
    fn error_render_shows_message() {
        let mut view = LyricsView::new();
        view.set_error("API timeout");
        let text = render_to_string(&view, 60, 8);
        assert!(text.contains("API timeout"), "text:\n{text}");
    }

    #[test]
    fn empty_header_falls_back_to_lyrics_label() {
        let view = LyricsView::new();
        let text = render_to_string(&view, 60, 8);
        assert!(text.contains("Lyrics"), "missing fallback header:\n{text}");
    }
}
