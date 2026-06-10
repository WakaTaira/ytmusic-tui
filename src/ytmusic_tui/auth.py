"""Authentication helpers, error classification, and the auth CLI flow."""

from __future__ import annotations

import json
from pathlib import Path
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from typing import TextIO

_AUTH_ERROR_PATTERNS = (
    "Login Required",
    "Request had invalid authentication credentials",
    "The request is missing a valid API key",
    "UNAUTHENTICATED",
    "403",
    "401",
)


def is_auth_error(exc: Exception) -> bool:
    """Return True if *exc* looks like an authentication failure."""
    msg = str(exc)
    return any(pattern in msg for pattern in _AUTH_ERROR_PATTERNS)


def classify_api_error(exc: Exception) -> str:
    """Return a user-friendly one-line error message."""
    if is_auth_error(exc):
        return "Auth expired — run: ytmusic-tui auth"

    # A MutationFailedError already carries a precise, user-facing message
    # ("Track was not found in the playlist", etc.): surface it verbatim.
    # Local import keeps auth.py free of a module-level api dependency.
    from ytmusic_tui.api import MutationFailedError

    if isinstance(exc, MutationFailedError):
        return str(exc)

    msg = str(exc).lower()
    if "oauth json provided" in msg or "oauth_credentials" in msg:
        # ytmusicapi misclassifies browser files lacking the
        # `authorization` header as OAuth.
        return "Auth file looks broken — run: ytmusic-tui auth"
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


def run_auth_setup(
    auth_path: str | Path | None = None,
    *,
    input_stream: TextIO | None = None,
) -> int:
    """Interactive browser-auth setup (the ``ytmusic-tui auth`` subcommand).

    Guides the user through copying request headers from
    music.youtube.com and writes the ytmusicapi browser-auth JSON to the
    configured path. Returns a process exit code.
    """
    import sys

    from ytmusicapi import setup as ytmusicapi_setup

    if auth_path is None:
        # Local import: config is only needed for the CLI flow.
        from ytmusic_tui.config import load_config

        auth_path = load_config().auth.browser_auth_path
    path = Path(auth_path).expanduser()

    stream = input_stream if input_stream is not None else sys.stdin

    print("ytmusic-tui browser authentication setup")
    print("----------------------------------------")
    print("1. Open https://music.youtube.com in your browser and sign in.")
    print("2. Open DevTools (F12) -> Network tab and filter for 'browse'.")
    print("   Click a request to music.youtube.com/youtubei/v1/browse.")
    print("   (Telemetry requests like /api/stats/... lack the required")
    print("   'authorization' header — do not use those.)")
    print("3. Copy the raw *Request Headers* block.")
    print("4. Paste it below, then finish with Ctrl-D on an empty line.")
    print(f"   Credentials will be written to: {path}")
    print()

    try:
        headers_raw = stream.read()
    except KeyboardInterrupt:
        print("\nAborted.")
        return 1

    if not headers_raw.strip():
        print("No headers received — aborted.")
        return 1

    # Keep the previous credentials so a bad paste cannot leave the
    # user worse off than before.
    backup = path.read_bytes() if path.is_file() else None

    def _restore() -> None:
        if backup is not None:
            path.write_bytes(backup)
            print("Previous credentials were restored.")
        elif path.is_file():
            path.unlink()

    path.parent.mkdir(parents=True, exist_ok=True)
    try:
        ytmusicapi_setup(filepath=str(path), headers_raw=headers_raw)
    except Exception as exc:
        print(f"Setup failed: {exc}")
        print("Make sure you copied the complete request headers (including Cookie).")
        _restore()
        return 1

    problem = validate_auth_file(path)
    if problem:
        print(f"Setup wrote a file, but it does not look valid: {problem}")
        _restore()
        return 1

    print(f"Success — credentials saved to {path}")
    print("Restart ytmusic-tui to use the new session.")
    return 0


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

    # ytmusicapi requires both headers for browser auth; without
    # `authorization` it misclassifies the file as OAuth and refuses to
    # start. Telemetry requests (e.g. /api/stats/qoe) lack it, so guide
    # the user toward a real API request.
    keys = {key.lower() for key in data}
    for required in ("cookie", "authorization"):
        if required not in keys:
            return (
                f"Auth file is missing the '{required}' header — copy the headers "
                "from a 'browse' request (filter the Network tab for 'browse'), "
                "then re-run: ytmusic-tui auth"
            )
    return None
