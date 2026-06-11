//! Page history stack for browser-style back navigation.
//!
//! Matches spotify_player's `UIState.history` pattern: every navigation action
//! pushes the current page onto the stack so Escape can pop back to the
//! previous page.
//!
//! 1:1 port of the Python `ytmusic_tui.navigation` module.
//!
//! Note on naming: this module's [`PageState`] is the navigable-page snapshot
//! (a page type plus its context), distinct from the per-view
//! [`crate::views::PageState`] enum (`Loading` / `Loaded` / `Error`). They live
//! in separate modules and never collide; refer to them by their module path
//! where both are in scope.

use std::collections::HashMap;

/// Maximum number of history entries to prevent unbounded growth.
pub const MAX_HISTORY_DEPTH: usize = 50;

/// Immutable snapshot of a single page and its context.
///
/// `page_type` is the view identifier matching the UI's view ids (e.g.
/// `"home"`, `"search"`, `"album"`, `"artist"`). `context` is an optional map
/// of page-specific data such as `browse_id` for albums or `channel_id` for
/// artists.
///
/// The Python dataclass is `frozen=True`; here immutability is the Rust default
/// — the fields are read-only to callers who hold a `&PageState`, and equality
/// (derived [`PartialEq`]) compares `page_type` and `context` structurally,
/// matching the Python `__eq__`.
///
/// `context` narrows Python's `dict[str, Any]` to string values: every real
/// call site stores ids (`browse_id`, `channel_id`, `video_id`). A future
/// non-string context value would need a typed enum here instead.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PageState {
    pub page_type: String,
    pub context: HashMap<String, String>,
}

impl PageState {
    /// Create a page with no context (the common case).
    pub fn new(page_type: impl Into<String>) -> Self {
        Self {
            page_type: page_type.into(),
            context: HashMap::new(),
        }
    }

    /// Create a page carrying a single `key = value` context entry.
    ///
    /// Convenience for the album/artist/lyrics pages, which each push exactly
    /// one context key (`browse_id` / `channel_id` / `video_id`).
    pub fn with_context(
        page_type: impl Into<String>,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        let mut context = HashMap::new();
        context.insert(key.into(), value.into());
        Self {
            page_type: page_type.into(),
            context,
        }
    }
}

/// Manages a page history stack for back-navigation.
///
/// The manager tracks the *current* page and a LIFO stack of previously
/// visited pages. [`push`](Self::push) saves the current page and switches to a
/// new one; [`pop`](Self::pop) restores the most recent previous page.
pub struct NavigationManager {
    current: PageState,
    history: Vec<PageState>,
    max_depth: usize,
}

impl NavigationManager {
    /// Create a navigation manager starting on `initial_page`.
    ///
    /// Uses the default history depth ([`MAX_HISTORY_DEPTH`]). For a custom
    /// depth, use [`with_max_depth`](Self::with_max_depth).
    pub fn new(initial_page: PageState) -> Self {
        Self::with_max_depth(initial_page, MAX_HISTORY_DEPTH)
    }

    /// Create a navigation manager with an explicit `max_depth`.
    ///
    /// The oldest entries are discarded when the stack exceeds `max_depth`.
    pub fn with_max_depth(initial_page: PageState, max_depth: usize) -> Self {
        Self {
            current: initial_page,
            history: Vec::new(),
            max_depth,
        }
    }

    // -- Public API --------------------------------------------------------

    /// Return the current page state.
    pub fn current(&self) -> &PageState {
        &self.current
    }

    /// Return a copy of the history stack (oldest first).
    ///
    /// Mirrors the Python `history` property, which returns `list(self._history)`
    /// — a shallow copy, so the caller cannot mutate internal state.
    pub fn history(&self) -> Vec<PageState> {
        self.history.clone()
    }

    /// Return `true` if there is at least one page in the history.
    pub fn can_go_back(&self) -> bool {
        !self.history.is_empty()
    }

    /// Push the current page onto the history stack and navigate to `page`.
    ///
    /// If `page` is identical to the current page, this is a no-op to avoid
    /// polluting the stack with duplicates. When the stack exceeds `max_depth`,
    /// the oldest entry is removed.
    pub fn push(&mut self, page: PageState) {
        if page == self.current {
            return;
        }

        // Move the current page into history; replace current with the new page.
        let previous = std::mem::replace(&mut self.current, page);
        self.history.push(previous);

        // Trim the oldest entries when the stack grows beyond the limit.
        if self.history.len() > self.max_depth {
            let overflow = self.history.len() - self.max_depth;
            self.history.drain(0..overflow);
        }
    }

    /// Pop the most recent page from the history and make it current.
    ///
    /// Returns the restored [`PageState`], or `None` when the history is empty.
    pub fn pop(&mut self) -> Option<PageState> {
        let previous = self.history.pop()?;
        self.current = previous.clone();
        Some(previous)
    }

    /// Clear the entire history stack without changing the current page.
    pub fn clear(&mut self) {
        self.history.clear();
    }

    /// Replace the current page without modifying the history stack.
    ///
    /// Useful for in-place updates (e.g. refreshing the same album).
    pub fn replace(&mut self, page: PageState) {
        self.current = page;
    }
}

impl Default for NavigationManager {
    /// Start on the home page with the default history depth.
    fn default() -> Self {
        Self::new(PageState::new("home"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- PageState (port of test_navigation.py::TestPageState) -------------

    #[test]
    fn default_context_is_empty() {
        let page = PageState::new("home");
        assert!(page.context.is_empty());
    }

    #[test]
    fn context_preserved() {
        let page = PageState::with_context("album", "browse_id", "abc123");
        assert_eq!(
            page.context.get("browse_id").map(String::as_str),
            Some("abc123")
        );
        assert_eq!(page.context.len(), 1);
    }

    #[test]
    fn equality() {
        let a = PageState::with_context("album", "browse_id", "x");
        let b = PageState::with_context("album", "browse_id", "x");
        assert_eq!(a, b);
    }

    #[test]
    fn inequality_different_type() {
        let a = PageState::new("home");
        let b = PageState::new("search");
        assert_ne!(a, b);
    }

    #[test]
    fn inequality_different_context() {
        let a = PageState::with_context("album", "browse_id", "x");
        let b = PageState::with_context("album", "browse_id", "y");
        assert_ne!(a, b);
    }

    // -- NavigationManager basics
    //    (port of TestNavigationManagerBasic) ----------------------------

    #[test]
    fn initial_current_is_home() {
        let nav = NavigationManager::default();
        assert_eq!(*nav.current(), PageState::new("home"));
    }

    #[test]
    fn custom_initial_page() {
        let page = PageState::new("search");
        let nav = NavigationManager::new(page.clone());
        assert_eq!(*nav.current(), page);
    }

    #[test]
    fn empty_history_on_init() {
        let nav = NavigationManager::default();
        assert!(nav.history().is_empty());
        assert!(!nav.can_go_back());
    }

    #[test]
    fn push_updates_current() {
        let mut nav = NavigationManager::default();
        let search = PageState::new("search");
        nav.push(search.clone());
        assert_eq!(*nav.current(), search);
    }

    #[test]
    fn push_saves_previous_to_history() {
        let mut nav = NavigationManager::default();
        let home = nav.current().clone();
        nav.push(PageState::new("search"));
        assert_eq!(nav.history(), vec![home]);
        assert!(nav.can_go_back());
    }

    #[test]
    fn push_duplicate_is_noop() {
        let mut nav = NavigationManager::default();
        let home = nav.current().clone();
        nav.push(home.clone());
        assert!(nav.history().is_empty());
        assert_eq!(*nav.current(), home);
    }

    #[test]
    fn pop_returns_previous() {
        let mut nav = NavigationManager::default();
        let home = nav.current().clone();
        nav.push(PageState::new("search"));

        let result = nav.pop();
        assert_eq!(result, Some(home.clone()));
        assert_eq!(*nav.current(), home);
    }

    #[test]
    fn pop_empty_returns_none() {
        let mut nav = NavigationManager::default();
        assert_eq!(nav.pop(), None);
        // Current should remain unchanged.
        assert_eq!(*nav.current(), PageState::new("home"));
    }

    #[test]
    fn clear_empties_history() {
        let mut nav = NavigationManager::default();
        nav.push(PageState::new("search"));
        nav.push(PageState::new("library"));
        nav.clear();
        assert!(nav.history().is_empty());
        assert!(!nav.can_go_back());
        // Current page is not affected.
        assert_eq!(*nav.current(), PageState::new("library"));
    }

    #[test]
    fn replace_changes_current_without_history() {
        let mut nav = NavigationManager::default();
        nav.push(PageState::new("search"));
        let album = PageState::with_context("album", "browse_id", "new");
        nav.replace(album.clone());
        assert_eq!(*nav.current(), album);
        // History still only has the initial home page.
        assert_eq!(nav.history().len(), 1);
    }

    // -- Multi-step sequences (port of TestNavigationManagerSequence) ------

    #[test]
    fn push_pop_sequence() {
        let mut nav = NavigationManager::default();
        let home = nav.current().clone();
        let search = PageState::new("search");
        let album = PageState::with_context("album", "browse_id", "abc");
        let artist = PageState::with_context("artist", "channel_id", "xyz");

        nav.push(search.clone());
        nav.push(album.clone());
        nav.push(artist.clone());

        assert_eq!(*nav.current(), artist);

        assert_eq!(nav.pop(), Some(album.clone()));
        assert_eq!(*nav.current(), album);

        assert_eq!(nav.pop(), Some(search.clone()));
        assert_eq!(*nav.current(), search);

        assert_eq!(nav.pop(), Some(home.clone()));
        assert_eq!(*nav.current(), home);

        assert_eq!(nav.pop(), None);
        assert_eq!(*nav.current(), home);
    }

    #[test]
    fn interleaved_push_pop() {
        let mut nav = NavigationManager::default();
        let search = PageState::new("search");
        let library = PageState::new("library");
        let queue = PageState::new("queue");

        nav.push(search.clone()); // stack: [home]
        nav.push(library); // stack: [home, search]
        nav.pop(); // back to search, stack: [home]
        nav.push(queue.clone()); // stack: [home, search]

        assert_eq!(*nav.current(), queue);
        assert_eq!(nav.pop(), Some(search));
        assert_eq!(nav.pop(), Some(PageState::new("home")));
    }

    // -- Max depth (port of TestNavigationManagerMaxDepth) -----------------

    #[test]
    fn max_depth_trims_oldest() {
        let mut nav = NavigationManager::with_max_depth(PageState::new("home"), 3);
        let pages: Vec<PageState> = (0..5)
            .map(|i| PageState::new(format!("page-{i}")))
            .collect();

        for page in &pages {
            nav.push(page.clone());
        }

        // With max_depth=3, only the 3 most recent should survive.
        let history = nav.history();
        assert_eq!(history.len(), 3);
        // Oldest entries are trimmed; newest are kept.
        assert_eq!(history[0], pages[1]);
        assert_eq!(history[1], pages[2]);
        assert_eq!(history[2], pages[3]);
        assert_eq!(*nav.current(), pages[4]);
    }

    #[test]
    fn default_max_depth() {
        let mut nav = NavigationManager::default();
        assert_eq!(MAX_HISTORY_DEPTH, 50); // sanity check the constant

        for i in 0..60 {
            nav.push(PageState::new(format!("page-{i}")));
        }

        assert_eq!(nav.history().len(), 50);
    }

    #[test]
    fn max_depth_one() {
        let mut nav = NavigationManager::with_max_depth(PageState::new("home"), 1);
        nav.push(PageState::new("a"));
        nav.push(PageState::new("b"));
        nav.push(PageState::new("c"));
        let history = nav.history();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0], PageState::new("b"));
        assert_eq!(*nav.current(), PageState::new("c"));
    }

    // -- history() returns a copy (port of TestNavigationManagerHistoryCopy)

    #[test]
    fn history_returns_copy() {
        let mut nav = NavigationManager::default();
        nav.push(PageState::new("search"));
        let mut history = nav.history();
        history.clear();
        // Internal state should be unaffected.
        assert!(nav.can_go_back());
    }
}
