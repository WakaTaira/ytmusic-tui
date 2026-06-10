"""Tests for the page history stack (NavigationManager)."""

from __future__ import annotations

from unittest.mock import patch

import pytest

from ytmusic_tui.navigation import MAX_HISTORY_DEPTH, NavigationManager, PageState
from helpers import make_app as _make_app

# ===================================================================
# PageState
# ===================================================================


class TestPageState:
    def test_default_context_is_empty_dict(self) -> None:
        page = PageState(page_type="home")
        assert page.context == {}

    def test_context_preserved(self) -> None:
        page = PageState(page_type="album", context={"browse_id": "abc123"})
        assert page.context == {"browse_id": "abc123"}

    def test_frozen_immutability(self) -> None:
        page = PageState(page_type="home")
        with pytest.raises(AttributeError):
            page.page_type = "search"  # type: ignore[misc]

    def test_equality(self) -> None:
        a = PageState(page_type="album", context={"browse_id": "x"})
        b = PageState(page_type="album", context={"browse_id": "x"})
        assert a == b

    def test_inequality_different_type(self) -> None:
        a = PageState(page_type="home")
        b = PageState(page_type="search")
        assert a != b

    def test_inequality_different_context(self) -> None:
        a = PageState(page_type="album", context={"browse_id": "x"})
        b = PageState(page_type="album", context={"browse_id": "y"})
        assert a != b


# ===================================================================
# NavigationManager - Basic operations
# ===================================================================


class TestNavigationManagerBasic:
    def test_initial_current_is_home(self) -> None:
        nav = NavigationManager()
        assert nav.current == PageState(page_type="home")

    def test_custom_initial_page(self) -> None:
        page = PageState(page_type="search")
        nav = NavigationManager(initial_page=page)
        assert nav.current == page

    def test_empty_history_on_init(self) -> None:
        nav = NavigationManager()
        assert nav.history == []
        assert not nav.can_go_back

    def test_push_updates_current(self) -> None:
        nav = NavigationManager()
        search = PageState(page_type="search")
        nav.push(search)
        assert nav.current == search

    def test_push_saves_previous_to_history(self) -> None:
        nav = NavigationManager()
        home = nav.current
        search = PageState(page_type="search")
        nav.push(search)
        assert nav.history == [home]
        assert nav.can_go_back

    def test_push_duplicate_is_noop(self) -> None:
        nav = NavigationManager()
        home = nav.current
        nav.push(home)
        assert nav.history == []
        assert nav.current == home

    def test_pop_returns_previous(self) -> None:
        nav = NavigationManager()
        home = nav.current
        search = PageState(page_type="search")
        nav.push(search)

        result = nav.pop()
        assert result == home
        assert nav.current == home

    def test_pop_empty_returns_none(self) -> None:
        nav = NavigationManager()
        assert nav.pop() is None
        # Current should remain unchanged
        assert nav.current == PageState(page_type="home")

    def test_clear_empties_history(self) -> None:
        nav = NavigationManager()
        nav.push(PageState(page_type="search"))
        nav.push(PageState(page_type="library"))
        nav.clear()
        assert nav.history == []
        assert not nav.can_go_back
        # Current page is not affected
        assert nav.current == PageState(page_type="library")

    def test_replace_changes_current_without_history(self) -> None:
        nav = NavigationManager()
        nav.push(PageState(page_type="search"))
        album = PageState(page_type="album", context={"browse_id": "new"})
        nav.replace(album)
        assert nav.current == album
        # History still only has the initial home page
        assert len(nav.history) == 1


# ===================================================================
# NavigationManager - Multi-step sequences
# ===================================================================


class TestNavigationManagerSequence:
    def test_push_pop_sequence(self) -> None:
        nav = NavigationManager()
        home = nav.current
        search = PageState(page_type="search")
        album = PageState(page_type="album", context={"browse_id": "abc"})
        artist = PageState(page_type="artist", context={"channel_id": "xyz"})

        nav.push(search)
        nav.push(album)
        nav.push(artist)

        assert nav.current == artist

        assert nav.pop() == album
        assert nav.current == album

        assert nav.pop() == search
        assert nav.current == search

        assert nav.pop() == home
        assert nav.current == home

        assert nav.pop() is None
        assert nav.current == home

    def test_interleaved_push_pop(self) -> None:
        nav = NavigationManager()
        search = PageState(page_type="search")
        library = PageState(page_type="library")
        queue = PageState(page_type="queue")

        nav.push(search)  # stack: [home]
        nav.push(library)  # stack: [home, search]
        nav.pop()  # back to search, stack: [home]
        nav.push(queue)  # stack: [home, search]

        assert nav.current == queue
        assert nav.pop() == search
        assert nav.pop() == PageState(page_type="home")


# ===================================================================
# NavigationManager - Max depth
# ===================================================================


class TestNavigationManagerMaxDepth:
    def test_max_depth_trims_oldest(self) -> None:
        nav = NavigationManager(max_depth=3)
        pages = [PageState(page_type=f"page-{i}") for i in range(5)]

        for page in pages:
            nav.push(page)

        # With max_depth=3, only the 3 most recent should survive
        assert len(nav.history) == 3
        # Oldest entries are trimmed; newest are kept
        assert nav.history[0] == pages[1]
        assert nav.history[1] == pages[2]
        assert nav.history[2] == pages[3]
        assert nav.current == pages[4]

    def test_default_max_depth(self) -> None:
        nav = NavigationManager()
        assert MAX_HISTORY_DEPTH == 50  # sanity check the constant

        for i in range(60):
            nav.push(PageState(page_type=f"page-{i}"))

        assert len(nav.history) == 50

    def test_max_depth_one(self) -> None:
        nav = NavigationManager(max_depth=1)
        nav.push(PageState(page_type="a"))
        nav.push(PageState(page_type="b"))
        nav.push(PageState(page_type="c"))
        assert len(nav.history) == 1
        assert nav.history[0] == PageState(page_type="b")
        assert nav.current == PageState(page_type="c")


# ===================================================================
# NavigationManager - history returns a copy
# ===================================================================


class TestNavigationManagerHistoryCopy:
    def test_history_property_returns_copy(self) -> None:
        nav = NavigationManager()
        nav.push(PageState(page_type="search"))
        history = nav.history
        history.clear()
        # Internal state should be unaffected
        assert nav.can_go_back


# ===================================================================
# App integration: navigation wiring
# ===================================================================


class TestAppNavigation:
    @pytest.mark.asyncio
    async def test_initial_page_is_home(self) -> None:
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            assert app.nav.current == PageState(page_type="home")
            assert not app.nav.can_go_back

    @pytest.mark.asyncio
    async def test_switch_view_pushes_history(self) -> None:
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            app.action_switch_view("search")
            assert app.nav.current == PageState(page_type="search")
            assert app.nav.can_go_back

    @pytest.mark.asyncio
    async def test_escape_pops_back(self) -> None:
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            app.action_switch_view("search")
            app.action_switch_view("library")

            # Go back once: should return to search
            app.action_go_back()
            assert app.nav.current == PageState(page_type="search")

            # Go back again: should return to home
            app.action_go_back()
            assert app.nav.current == PageState(page_type="home")

    @pytest.mark.asyncio
    async def test_escape_on_empty_history_goes_home(self) -> None:
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            # Already on home; Escape should be a no-op fallback
            app.action_go_back()
            assert app.nav.current == PageState(page_type="home")

    @pytest.mark.asyncio
    async def test_open_album_pushes_with_context(self) -> None:
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            app.action_open_album("ALBUM_123")
            assert app.nav.current == PageState(
                page_type="album", context={"browse_id": "ALBUM_123"}
            )
            assert app.nav.can_go_back

    @pytest.mark.asyncio
    async def test_open_artist_pushes_with_context(self) -> None:
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            app.action_open_artist("CHANNEL_456")
            assert app.nav.current == PageState(
                page_type="artist", context={"channel_id": "CHANNEL_456"}
            )
            assert app.nav.can_go_back

    @pytest.mark.asyncio
    async def test_album_to_artist_and_back(self) -> None:
        """Navigate home -> album -> artist, then Escape back twice."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            app.action_open_album("ALBUM_A")
            app.action_open_artist("ARTIST_B")

            app.action_go_back()
            assert app.nav.current == PageState(
                page_type="album", context={"browse_id": "ALBUM_A"}
            )

            app.action_go_back()
            assert app.nav.current == PageState(page_type="home")

    @pytest.mark.asyncio
    async def test_content_switcher_follows_navigation(self) -> None:
        """Verify the ContentSwitcher actually changes with navigation."""
        from textual.widgets import ContentSwitcher

        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            switcher = app.query_one(ContentSwitcher)
            assert switcher.current == "home"

            app.action_switch_view("queue")
            assert switcher.current == "queue"

            app.action_go_back()
            assert switcher.current == "home"

    @pytest.mark.asyncio
    async def test_focus_search_pushes_history(self) -> None:
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            app.action_focus_search()
            assert app.nav.current == PageState(page_type="search")
            assert app.nav.can_go_back

    @pytest.mark.asyncio
    async def test_duplicate_navigation_does_not_push(self) -> None:
        """Switching to the same view twice should not pollute the stack."""
        app = _make_app()
        async with app.run_test(size=(120, 40)) as _pilot:
            app.action_switch_view("search")
            app.action_switch_view("search")
            # Only one entry (home) should be in history
            assert len(app.nav.history) == 1
