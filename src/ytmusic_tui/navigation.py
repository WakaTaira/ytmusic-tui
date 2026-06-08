"""Page history stack for browser-style back navigation.

Matches spotify_player's UIState.history pattern: every navigation
action pushes the current page onto the stack so Escape can pop
back to the previous page.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any

# Maximum number of history entries to prevent unbounded growth.
MAX_HISTORY_DEPTH = 50


@dataclass(frozen=True)
class PageState:
    """Immutable snapshot of a single page and its context.

    Attributes:
        page_type: View identifier matching ContentSwitcher IDs
                   (e.g. "home", "search", "album", "artist").
        context: Optional dictionary with page-specific data such as
                 ``browse_id`` for albums or ``channel_id`` for artists.
    """

    page_type: str
    context: dict[str, Any] = field(default_factory=dict)


class NavigationManager:
    """Manages a page history stack for back-navigation.

    The manager tracks the *current* page and a LIFO stack of
    previously visited pages.  ``push()`` saves the current page
    and switches to a new one; ``pop()`` restores the most recent
    previous page.
    """

    def __init__(
        self,
        initial_page: PageState | None = None,
        *,
        max_depth: int = MAX_HISTORY_DEPTH,
    ) -> None:
        """Initialize the navigation manager.

        Args:
            initial_page: Starting page.  Defaults to the home page.
            max_depth: Maximum number of entries kept in the history
                       stack.  Oldest entries are discarded when the
                       limit is exceeded.
        """
        self._current: PageState = initial_page or PageState(page_type="home")
        self._history: list[PageState] = []
        self._max_depth: int = max_depth

    # -- Public API --------------------------------------------------------

    @property
    def current(self) -> PageState:
        """Return the current page state."""
        return self._current

    @property
    def history(self) -> list[PageState]:
        """Return a shallow copy of the history stack (oldest first)."""
        return list(self._history)

    @property
    def can_go_back(self) -> bool:
        """Return True if there is at least one page in the history."""
        return len(self._history) > 0

    def push(self, page: PageState) -> None:
        """Push the current page onto the history stack and navigate to *page*.

        If the new page is identical to the current page, this is a no-op
        to avoid polluting the stack with duplicates.

        When the stack exceeds *max_depth*, the oldest entry is removed.
        """
        if page == self._current:
            return

        self._history.append(self._current)

        # Trim the oldest entry when the stack grows beyond the limit.
        if len(self._history) > self._max_depth:
            self._history = self._history[-self._max_depth :]

        self._current = page

    def pop(self) -> PageState | None:
        """Pop the most recent page from the history and make it current.

        Returns:
            The restored :class:`PageState`, or ``None`` when the history
            is empty.
        """
        if not self._history:
            return None

        previous = self._history.pop()
        self._current = previous
        return previous

    def clear(self) -> None:
        """Clear the entire history stack without changing the current page."""
        self._history.clear()

    def replace(self, page: PageState) -> None:
        """Replace the current page without modifying the history stack.

        Useful for in-place updates (e.g. refreshing the same album).
        """
        self._current = page
