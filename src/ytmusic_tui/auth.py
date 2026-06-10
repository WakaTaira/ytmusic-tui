"""Authentication helpers and error classification."""

from __future__ import annotations

import json
from pathlib import Path

_AUTH_ERROR_PATTERNS = (
    "Login Required",
    "Request had invalid authentication credentials",
    "The request is missing a valid API key",
    "UNAUTHENTICATED",
    "403",
    "401",
)


class AuthError(Exception):
    """Raised when an API call fails due to authentication."""


def is_auth_error(exc: Exception) -> bool:
    """Return True if *exc* looks like an authentication failure."""
    msg = str(exc)
    return any(pattern in msg for pattern in _AUTH_ERROR_PATTERNS)


def classify_api_error(exc: Exception) -> str:
    """Return a user-friendly one-line error message."""
    if is_auth_error(exc):
        return "Auth expired — run: ytmusic-tui auth"

    msg = str(exc).lower()
    if "timeout" in msg or "timed out" in msg:
        return "Request timed out — check your connection"
    if "network" in msg or "connection" in msg or "unreachable" in msg:
        return "Network error — check your connection"
    if "not found" in msg or "404" in msg:
        return "Not found"
    if "rate" in msg and "limit" in msg:
        return "Rate limited — try again later"

    text = str(exc)
    if len(text) > 80:
        text = text[:77] + "..."
    return f"Error: {text}"


def validate_auth_file(path: str | Path) -> str | None:
    """Check that the browser auth JSON exists and looks valid.

    Returns None if valid, or a user-friendly error string.
    """
    p = Path(path).expanduser()
    if not p.exists():
        return f"Auth file not found: {p}\nRun: ytmusic-tui auth"
    if not p.is_file():
        return f"Auth path is not a file: {p}"
    try:
        data = json.loads(p.read_text())
        if not isinstance(data, dict):
            return "Auth file is not a valid JSON object"
        if not data:
            return "Auth file is empty"
    except json.JSONDecodeError:
        return "Auth file contains invalid JSON"
    except OSError as e:
        return f"Cannot read auth file: {e}"
    return None
