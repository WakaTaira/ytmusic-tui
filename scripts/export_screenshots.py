#!/usr/bin/env python3
"""Generate SVG screenshots of ytmusic-tui views using Textual's export_screenshot.

Each view is switched to, populated with sample data, and exported as SVG.
Run from the project root:

    python scripts/export_screenshots.py

Output goes to screenshots/*.svg.
"""

from __future__ import annotations

import asyncio
from pathlib import Path
from typing import TYPE_CHECKING
from unittest.mock import MagicMock, PropertyMock, patch

if TYPE_CHECKING:
    from textual.app import App

PROJECT_ROOT = Path(__file__).resolve().parent.parent
OUTPUT_DIR = PROJECT_ROOT / "screenshots"

SAMPLE_TRACKS = [
    {
        "video_id": f"vid{i}",
        "title": title,
        "artist": artist,
        "album": album,
        "duration_seconds": dur,
    }
    for i, (title, artist, album, dur) in enumerate(
        [
            ("Midnight City", "M83", "Hurry Up, We're Dreaming", 244),
            ("Retrograde", "James Blake", "Overgrown", 222),
            ("Sunset Lover", "Petit Biscuit", "Petit Biscuit", 239),
            ("Nightcall", "Kavinsky", "OutRun", 256),
            ("Electric Feel", "MGMT", "Oracular Spectacular", 229),
            ("Tame Impala", "Let It Happen", "Currents", 467),
            ("Tadow", "Masego & FKJ", "Tadow", 299),
            ("Pink + White", "Frank Ocean", "Blonde", 193),
        ],
        start=1,
    )
]


def _make_mock_api() -> MagicMock:
    """Create a mock MusicAPI that returns sample data."""
    from ytmusic_tui.api import AlbumInfo, HomeSection, PlaylistInfo, SearchResults
    from ytmusic_tui.queue import Track

    tracks = [Track(**t) for t in SAMPLE_TRACKS]
    api = MagicMock()
    api.get_home.return_value = [
        HomeSection(title="Quick picks", items=tracks[:4]),
        HomeSection(title="Mixed for you", items=tracks[4:]),
    ]
    api.search_all.return_value = SearchResults(
        tracks=tracks[:4],
        albums=[
            AlbumInfo(
                browse_id="alb1",
                title="Hurry Up, We're Dreaming",
                artist="M83",
                year="2011",
            ),
            AlbumInfo(
                browse_id="alb2",
                title="Overgrown",
                artist="James Blake",
                year="2013",
            ),
        ],
        artists=[],
        playlists=[
            PlaylistInfo(
                playlist_id="pl1",
                title="Synthwave Essentials",
                track_count=42,
            ),
        ],
    )
    api.get_library_playlists.return_value = [
        PlaylistInfo(playlist_id="pl1", title="My Favorites", track_count=120),
        PlaylistInfo(playlist_id="pl2", title="Chill Vibes", track_count=45),
        PlaylistInfo(playlist_id="pl3", title="Workout Mix", track_count=33),
    ]
    api.get_library_albums.return_value = [
        AlbumInfo(browse_id="a1", title="Random Access Memories", artist="Daft Punk", year="2013"),
        AlbumInfo(browse_id="a2", title="In Rainbows", artist="Radiohead", year="2007"),
    ]
    api.get_library_artists.return_value = []
    api.get_liked_songs.return_value = tracks[:5]
    api.get_playlist_tracks.return_value = tracks
    return api


async def _export_view(app: App, view_id: str, filename: str) -> None:
    """Switch to a view and save its SVG screenshot."""
    from textual.widgets import ContentSwitcher

    switcher = app.query_one(ContentSwitcher)
    switcher.current = view_id
    await asyncio.sleep(0.5)
    svg = app.export_screenshot()
    output_path = OUTPUT_DIR / filename
    output_path.write_text(svg)
    print(f"  Saved: {output_path}")


async def main() -> None:
    OUTPUT_DIR.mkdir(exist_ok=True)

    with (
        patch("ytmusic_tui.app.MusicAPI") as mock_api_cls,
        patch("ytmusic_tui.app.Player") as mock_player_cls,
    ):
        mock_api_cls.return_value = _make_mock_api()
        mock_player = MagicMock()
        mock_player.get_state.return_value = MagicMock(
            is_playing=True,
            volume=80,
            position=120.0,
            duration=244.0,
            title="Midnight City",
            artist="M83",
            video_id="vid1",
            progress=120.0 / 244.0,
        )
        type(mock_player).is_idle = PropertyMock(return_value=False)
        mock_player_cls.return_value = mock_player

        from ytmusic_tui.app import YtMusicTui

        app = YtMusicTui(auth_path="/dev/null")

        async with app.run_test(size=(120, 40)) as _:
            await asyncio.sleep(1.0)

            print("Exporting screenshots...")
            await _export_view(app, "home", "home.svg")
            await _export_view(app, "library", "library.svg")
            await _export_view(app, "queue", "queue.svg")

            print("Done! Screenshots saved to screenshots/")


if __name__ == "__main__":
    asyncio.run(main())
