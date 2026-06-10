"""Lyrics display view.

Shows lyrics for the currently playing track. Fetches lyrics via
the YouTube Music API (get_watch_playlist → get_lyrics).
"""

from __future__ import annotations

from typing import TYPE_CHECKING

from textual import work
from textual.containers import VerticalScroll
from textual.widgets import Label, Static

from ytmusic_tui.auth import classify_api_error

if TYPE_CHECKING:
    from textual.app import ComposeResult


class LyricsView(Static):
    """Full-screen lyrics display for the current track."""

    DEFAULT_CSS = """
    LyricsView {
        width: 1fr;
        height: 1fr;
    }
    LyricsView #lyrics-title {
        text-style: bold;
        color: $accent;
        padding: 1 1 0 1;
    }
    LyricsView #lyrics-status {
        height: 1;
        padding: 0 1;
        text-style: italic;
        color: $text-muted;
    }
    LyricsView #lyrics-scroll {
        width: 1fr;
        height: 1fr;
        padding: 0 2;
    }
    LyricsView #lyrics-text {
        width: 1fr;
        padding: 1 0;
    }
    """

    def __init__(self, **kwargs: object) -> None:
        super().__init__(**kwargs)
        self._current_video_id: str = ""

    def compose(self) -> ComposeResult:
        yield Label("Lyrics", id="lyrics-title")
        yield Label("", id="lyrics-status")
        with VerticalScroll(id="lyrics-scroll"):
            yield Label("", id="lyrics-text")

    def load_lyrics(self, video_id: str, title: str = "", artist: str = "") -> None:
        """Fetch and display lyrics for the given track."""
        if not video_id:
            self._show_status("No track playing")
            return

        self._current_video_id = video_id
        header = f"{title} - {artist}" if artist else title
        self.query_one("#lyrics-title", Label).update(header or "Lyrics")
        self._show_status("Loading lyrics...")
        self.query_one("#lyrics-text", Label).update("")
        self._fetch_lyrics(video_id)

    @work(thread=True)
    def _fetch_lyrics(self, video_id: str) -> None:
        api = getattr(self.app, "music_api", None)
        if api is None:
            self.app.call_from_thread(self._show_status, "API not initialized")
            return

        try:
            lyrics = api.get_lyrics(video_id)
            if lyrics:
                self.app.call_from_thread(self._display_lyrics, lyrics)
            else:
                self.app.call_from_thread(self._show_status, "No lyrics available")
        except Exception as exc:
            self.app.call_from_thread(self._show_status, classify_api_error(exc))

    def _display_lyrics(self, text: str) -> None:
        self._show_status("")
        self.query_one("#lyrics-text", Label).update(text)

    def _show_status(self, text: str) -> None:
        self.query_one("#lyrics-status", Label).update(text)
