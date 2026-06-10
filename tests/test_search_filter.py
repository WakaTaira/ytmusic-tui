"""Tests for category-specific search (prefix parsing and filtered search)."""

from __future__ import annotations

from types import SimpleNamespace
from unittest.mock import MagicMock, patch

from ytmusic_tui.views.search import SearchView, _parse_search_prefix


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


class TestSearchDispatch:
    """Verify the prefix parser is wired into the input handler."""

    def _submit(self, raw: str) -> list[tuple[str, str | None]]:
        view = SearchView()
        calls: list[tuple[str, str | None]] = []
        view._run_search = (  # type: ignore[method-assign]
            lambda query, category=None: calls.append((query, category))
        )
        view.on_input_submitted(SimpleNamespace(value=raw))  # type: ignore[arg-type]
        return calls

    def test_plain_query_searches_all_categories(self) -> None:
        assert self._submit("lofi beats") == [("lofi beats", None)]

    def test_prefixed_query_restricts_category(self) -> None:
        assert self._submit("#albums:ok computer") == [("ok computer", "albums")]

    def test_songs_prefix_dispatches_songs(self) -> None:
        assert self._submit("#songs:rick astley") == [("rick astley", "songs")]

    def test_empty_input_does_not_search(self) -> None:
        assert self._submit("   ") == []

    def test_prefix_without_query_falls_back_to_full_search(self) -> None:
        # "#songs:" alone is not a valid prefixed query; it is searched as-is.
        assert self._submit("#songs:") == [("#songs:", None)]


class TestSearchAllFilterPassthrough:
    """search_all must forward the category filter to ytmusicapi."""

    @patch("ytmusic_tui.api.YTMusic")
    def test_filter_forwarded_to_client(self, mock_ytmusic_cls: MagicMock) -> None:
        from ytmusic_tui.api import MusicAPI

        mock_client = MagicMock()
        mock_client.search.return_value = []
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        api.search_all("query", limit=20, filter="albums")

        mock_client.search.assert_called_once_with("query", filter="albums", limit=20)

    @patch("ytmusic_tui.api.YTMusic")
    def test_no_filter_by_default(self, mock_ytmusic_cls: MagicMock) -> None:
        from ytmusic_tui.api import MusicAPI

        mock_client = MagicMock()
        mock_client.search.return_value = []
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        api.search_all("query")

        mock_client.search.assert_called_once_with("query", filter=None, limit=10)

    @patch("ytmusic_tui.api.YTMusic")
    def test_filtered_results_populate_matching_category(
        self, mock_ytmusic_cls: MagicMock
    ) -> None:
        from ytmusic_tui.api import MusicAPI

        mock_client = MagicMock()
        mock_client.search.return_value = [
            {
                "resultType": "album",
                "title": "OK Computer",
                "artists": [{"name": "Radiohead", "id": "a1"}],
                "browseId": "MPREb_x",
                "year": "1997",
            }
        ]
        mock_ytmusic_cls.return_value = mock_client

        api = MusicAPI("/fake/path")
        results = api.search_all("ok computer", filter="albums")

        assert len(results.albums) == 1
        assert results.albums[0].title == "OK Computer"
        assert results.tracks == []
