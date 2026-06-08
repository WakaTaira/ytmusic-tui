"""Shared formatting utilities."""

from __future__ import annotations


def format_duration(seconds: float) -> str:
    """Format *seconds* as ``M:SS`` or ``H:MM:SS``.

    When *seconds* is zero or negative the track has no known duration
    (e.g. YouTube Music ``get_home()`` omits the field), so we return
    a dash instead of a misleading ``0:00``.
    """
    total = int(seconds)
    if total <= 0:
        return "—"
    hours, remainder = divmod(total, 3600)
    minutes, secs = divmod(remainder, 60)
    if hours > 0:
        return f"{hours}:{minutes:02d}:{secs:02d}"
    return f"{minutes}:{secs:02d}"
