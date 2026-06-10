"""Tests for category-specific search (prefix parsing and filtered search)."""

from __future__ import annotations

import pytest

from ytmusic_tui.views.search import _parse_search_prefix


class TestParseSearchPrefix:
    """Verify the #category:query prefix parser."""

    def test_no_prefix(self) -> None:
        cat, query = _parse_search_prefix("hello world")
        assert cat is None
        assert query == "hello world"

    def test_songs_prefix(self) -> None:
        cat, query = _parse_search_prefix("#songs:lofi beats")
        assert cat == "songs"
        assert query == "lofi beats"

    def test_albums_prefix(self) -> None:
        cat, query = _parse_search_prefix("#albums:dark side of the moon")
        assert cat == "albums"
        assert query == "dark side of the moon"

    def test_artists_prefix(self) -> None:
        cat, query = _parse_search_prefix("#artists:radiohead")
        assert cat == "artists"
        assert query == "radiohead"

    def test_playlists_prefix(self) -> None:
        cat, query = _parse_search_prefix("#playlists:chill vibes")
        assert cat == "playlists"
        assert query == "chill vibes"

    def test_case_insensitive(self) -> None:
        cat, query = _parse_search_prefix("#SONGS:test")
        assert cat == "songs"
        assert query == "test"

    def test_mixed_case(self) -> None:
        cat, query = _parse_search_prefix("#Albums:discovery")
        assert cat == "albums"
        assert query == "discovery"

    def test_prefix_without_query_returns_none(self) -> None:
        cat, query = _parse_search_prefix("#songs:")
        assert cat is None
        assert query == "#songs:"

    def test_prefix_with_whitespace_only_returns_none(self) -> None:
        cat, query = _parse_search_prefix("#songs:   ")
        assert cat is None
        assert query == "#songs:   "

    def test_unknown_prefix_ignored(self) -> None:
        cat, query = _parse_search_prefix("#videos:music video")
        assert cat is None
        assert query == "#videos:music video"

    def test_hash_in_middle_not_treated_as_prefix(self) -> None:
        cat, query = _parse_search_prefix("my #songs:query")
        assert cat is None
        assert query == "my #songs:query"

    def test_strips_whitespace_after_prefix(self) -> None:
        cat, query = _parse_search_prefix("#artists:   frank ocean  ")
        assert cat == "artists"
        assert query == "frank ocean"

    def test_preserves_original_case_in_query(self) -> None:
        cat, query = _parse_search_prefix("#Songs:The Beatles")
        assert cat == "songs"
        assert query == "The Beatles"

    def test_empty_string(self) -> None:
        cat, query = _parse_search_prefix("")
        assert cat is None
        assert query == ""
