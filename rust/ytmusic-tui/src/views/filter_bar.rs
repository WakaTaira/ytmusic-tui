//! The in-page live filter bar (the `/`-toggled row filter).
//!
//! Port of `src/ytmusic_tui/views/filter_bar.py`. A one-line input docked at the
//! bottom of the content area that filters the current view's list rows live as
//! the user types: a **case-insensitive substring** match across all of a row's
//! cells (Python's `any(query_lower in cell.lower() for cell in row)`). The
//! filter is non-destructive — closing it (Esc) restores the full list.
//!
//! # Architecture vs Python
//!
//! Textual's `FilterBar` was a widget that mutated a `DataTable`'s rows in place
//! and restored them on hide. Here the views are pure values that re-render
//! every tick, so there is nothing to "restore": the filter is just a query
//! string the filterable views read when computing which rows to show. This
//! module owns:
//!
//! * [`FilterBar`] — the bar's own state (the query buffer + active flag) plus
//!   its one-line render and key handling.
//! * [`matches_filter`] — the shared row-matching rule, reused by every
//!   filterable view so the semantics are identical everywhere.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::Theme;

/// The height (in rows) of the rendered filter bar, including its border.
pub const FILTER_BAR_HEIGHT: u16 = 3;

/// The `/`-toggled live filter bar state.
///
/// Holds the query buffer and whether the bar is currently showing. The bar is
/// driven by the main loop: while active, printable keys append, Backspace
/// deletes, and Esc hides (clearing the query). The filtered views read
/// [`FilterBar::query`] to decide which rows to render.
#[derive(Debug, Clone, Default)]
pub struct FilterBar {
    /// The current filter text.
    query: String,
    /// Whether the bar is showing and capturing keystrokes.
    active: bool,
}

impl FilterBar {
    /// A fresh, hidden filter bar with an empty query.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the bar is currently showing (and capturing keys).
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// The current query text (empty when the bar is hidden or nothing typed).
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// The query as an `Option`, `None` when the bar is hidden or the query is
    /// blank — the form the filterable views consume (`None` = no filtering).
    #[must_use]
    pub fn active_query(&self) -> Option<&str> {
        if self.active && !self.query.trim().is_empty() {
            Some(self.query.as_str())
        } else {
            None
        }
    }

    /// Show the bar with an empty query (the `/` toggle, opening).
    pub fn show(&mut self) {
        self.active = true;
        self.query.clear();
    }

    /// Hide the bar and clear the query (Esc, or a toggle while open). The full
    /// list is restored implicitly because the views stop filtering once the
    /// query is gone.
    pub fn hide(&mut self) {
        self.active = false;
        self.query.clear();
    }

    /// Toggle the bar: show if hidden, hide if showing. Returns the new active
    /// state so the caller can react (e.g. cancel a pending key prefix).
    pub fn toggle(&mut self) -> bool {
        if self.active {
            self.hide();
        } else {
            self.show();
        }
        self.active
    }

    /// Append a printable character to the query (a keystroke while active).
    pub fn push_char(&mut self, ch: char) {
        self.query.push(ch);
    }

    /// Delete the last character of the query (Backspace while active).
    pub fn backspace(&mut self) {
        self.query.pop();
    }

    /// Render the one-line bar into `area` (the border + the typed query, with a
    /// trailing block cursor). `visible_count` / `total_count` are the
    /// post-filter / pre-filter row counts shown on the right (Python's
    /// `visible/total` label).
    pub fn render(
        &self,
        frame: &mut Frame<'_>,
        area: Rect,
        theme: &Theme,
        visible_count: usize,
        total_count: usize,
    ) {
        if !self.active || area.height == 0 {
            return;
        }
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.accent))
            .title(Span::styled(
                "Filter",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ));
        let count = format!("  {visible_count}/{total_count}");
        let line = Line::from(vec![
            Span::styled("/ ", Style::default().fg(theme.accent)),
            Span::styled(
                format!("{}\u{2588}", self.query),
                Style::default().fg(theme.text),
            ),
            Span::styled(count, Style::default().fg(theme.text_muted)),
        ]);
        frame.render_widget(Paragraph::new(line).block(block), area);
    }
}

/// The shared row-matching rule: case-insensitive substring across all cells.
///
/// A row matches when the (lowercased) `query` appears as a substring of any
/// (lowercased) cell. A blank query matches everything (the caller should pass
/// `None` to skip filtering entirely, but a blank string here is permissive to
/// be safe). 1:1 with Python's `any(query_lower in cell.lower() for cell in row)`.
#[must_use]
pub fn matches_filter(query: &str, cells: &[&str]) -> bool {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    cells.iter().any(|cell| cell.to_lowercase().contains(&q))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    // -- matches_filter ----------------------------------------------------

    #[test]
    fn matches_substring_case_insensitively() {
        assert!(matches_filter("daft", &["Daft Punk", "Discovery"]));
        assert!(matches_filter("DISCO", &["Daft Punk", "Discovery"]));
    }

    #[test]
    fn matches_in_any_cell() {
        assert!(matches_filter("punk", &["Get Lucky", "Daft Punk", "3:42"]));
    }

    #[test]
    fn non_match_returns_false() {
        assert!(!matches_filter("zzz", &["Daft Punk", "Discovery"]));
    }

    #[test]
    fn blank_query_matches_everything() {
        assert!(matches_filter("", &["anything"]));
        assert!(matches_filter("   ", &["anything"]));
    }

    // -- FilterBar state machine -------------------------------------------

    #[test]
    fn new_bar_is_hidden_and_empty() {
        let bar = FilterBar::new();
        assert!(!bar.is_active());
        assert_eq!(bar.query(), "");
        assert_eq!(bar.active_query(), None);
    }

    #[test]
    fn toggle_shows_then_hides() {
        let mut bar = FilterBar::new();
        assert!(bar.toggle());
        assert!(bar.is_active());
        assert!(!bar.toggle());
        assert!(!bar.is_active());
    }

    #[test]
    fn typing_builds_query() {
        let mut bar = FilterBar::new();
        bar.show();
        "punk".chars().for_each(|c| bar.push_char(c));
        assert_eq!(bar.query(), "punk");
        assert_eq!(bar.active_query(), Some("punk"));
    }

    #[test]
    fn backspace_removes_last_char() {
        let mut bar = FilterBar::new();
        bar.show();
        "ab".chars().for_each(|c| bar.push_char(c));
        bar.backspace();
        assert_eq!(bar.query(), "a");
        bar.backspace();
        bar.backspace(); // extra on empty is a no-op
        assert_eq!(bar.query(), "");
    }

    #[test]
    fn hide_clears_query() {
        let mut bar = FilterBar::new();
        bar.show();
        "x".chars().for_each(|c| bar.push_char(c));
        bar.hide();
        assert!(!bar.is_active());
        assert_eq!(bar.query(), "");
    }

    #[test]
    fn active_query_is_none_when_blank() {
        let mut bar = FilterBar::new();
        bar.show();
        // Active but blank → None (no filtering).
        assert_eq!(bar.active_query(), None);
        bar.push_char(' ');
        assert_eq!(bar.active_query(), None, "whitespace-only is still None");
    }

    // -- rendering ---------------------------------------------------------

    #[test]
    fn render_hidden_bar_draws_nothing() {
        let bar = FilterBar::new();
        let backend = TestBackend::new(40, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = Theme::default();
        terminal
            .draw(|frame| bar.render(frame, frame.area(), &theme, 0, 0))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let text: String = buffer.content().iter().map(|c| c.symbol()).collect();
        assert!(!text.contains("Filter"), "hidden bar should draw nothing");
    }

    #[test]
    fn render_active_bar_shows_query_and_counts() {
        let mut bar = FilterBar::new();
        bar.show();
        "punk".chars().for_each(|c| bar.push_char(c));
        let backend = TestBackend::new(40, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = Theme::default();
        terminal
            .draw(|frame| bar.render(frame, frame.area(), &theme, 2, 9))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let width = buffer.area().width as usize;
        let mut text = String::new();
        for (i, cell) in buffer.content().iter().enumerate() {
            text.push_str(cell.symbol());
            if (i + 1) % width == 0 {
                text.push('\n');
            }
        }
        assert!(text.contains("Filter"), "missing title:\n{text}");
        assert!(text.contains("punk"), "missing query:\n{text}");
        assert!(text.contains("2/9"), "missing count:\n{text}");
    }
}
