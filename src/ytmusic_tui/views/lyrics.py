"""Lyrics display view.

Shows lyrics for the currently playing track. Fetches lyrics via
the YouTube Music API (get_watch_playlist → get_lyrics).
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any, ClassVar

from textual.containers import VerticalScroll
from textual.widgets import Label

from ytmusic_tui.views.base import FetchView
from ytmusic_tui.views.guards import teardown_safe

if TYPE_CHECKING:
    from textual.app import ComposeResult


class LyricsView(FetchView):
    """Full-screen lyrics display for the current track."""

    STATUS_LABEL_ID: ClassVar[str] = "#lyrics-status"

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

    def __init__(self, **kwargs: Any) -> None:
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
            self._set_status("No track playing")
            return

        self._current_video_id = video_id
        header = f"{title} - {artist}" if artist else title
        self.query_one("#lyrics-title", Label).update(header or "Lyrics")
        self.query_one("#lyrics-text", Label).update("")
        self._run_fetch(
            lambda: self.music_app.music_api.get_lyrics(video_id),
            self._display_lyrics,
            loading="Loading lyrics...",
        )

    @teardown_safe
    def _display_lyrics(self, text: str | None) -> None:
        """Render fetched lyrics, or report that none were found."""
        if not text:
            self._set_status("No lyrics available")
            return
        self._set_status("")
        self.query_one("#lyrics-text", Label).update(text)
