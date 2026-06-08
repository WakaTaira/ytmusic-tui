"""Responsive layout orientation detection.

Determines whether the terminal is wide enough for a horizontal
(side-by-side) pane layout or should fall back to a vertical
(stacked) layout.  Uses the same heuristic as spotify_player:
``columns / rows > 2.3`` selects horizontal.
"""

from __future__ import annotations

from enum import Enum


class Orientation(Enum):
    """Layout orientation for multi-pane views."""

    HORIZONTAL = "horizontal"
    VERTICAL = "vertical"


# spotify_player uses 2.3 as the aspect ratio threshold
_ASPECT_THRESHOLD: float = 2.3


def detect_orientation(columns: int, rows: int) -> Orientation:
    """Return the recommended layout orientation for the given terminal size.

    Args:
        columns: Terminal width in characters.
        rows: Terminal height in lines.

    Returns:
        ``HORIZONTAL`` when the terminal is wide enough for
        side-by-side panes, ``VERTICAL`` otherwise.
    """
    if columns / max(rows, 1) > _ASPECT_THRESHOLD:
        return Orientation.HORIZONTAL
    return Orientation.VERTICAL
