"""Tests for authentication helpers and error classification."""

from __future__ import annotations

import json
from typing import TYPE_CHECKING

from ytmusic_tui.auth import (
    classify_api_error,
    is_auth_error,
    validate_auth_file,
)

if TYPE_CHECKING:
    from pathlib import Path

# ---------------------------------------------------------------------------
# is_auth_error
# ---------------------------------------------------------------------------


class TestIsAuthError:
    def test_login_required(self) -> None:
        assert is_auth_error(Exception("Login Required")) is True

    def test_unauthenticated(self) -> None:
        assert is_auth_error(Exception("UNAUTHENTICATED")) is True

    def test_403_in_message(self) -> None:
        assert is_auth_error(Exception("Server returned 403")) is True

    def test_401_in_message(self) -> None:
        assert is_auth_error(Exception("HTTP 401 Unauthorized")) is True

    def test_invalid_credentials(self) -> None:
        assert is_auth_error(Exception("Request had invalid authentication credentials")) is True

    def test_generic_error_not_auth(self) -> None:
        assert is_auth_error(Exception("Something went wrong")) is False

    def test_empty_message(self) -> None:
        assert is_auth_error(Exception("")) is False


# ---------------------------------------------------------------------------
# classify_api_error
# ---------------------------------------------------------------------------


class TestClassifyApiError:
    def test_auth_error(self) -> None:
        msg = classify_api_error(Exception("Login Required"))
        assert "auth" in msg.lower() or "Auth" in msg

    def test_timeout_error(self) -> None:
        msg = classify_api_error(Exception("Request timed out"))
        assert "timed out" in msg.lower()

    def test_network_error(self) -> None:
        msg = classify_api_error(Exception("Connection refused"))
        assert "connection" in msg.lower() or "network" in msg.lower()

    def test_not_found(self) -> None:
        msg = classify_api_error(Exception("404 Not Found"))
        assert "not found" in msg.lower()

    def test_rate_limit(self) -> None:
        msg = classify_api_error(Exception("Rate limit exceeded"))
        assert "rate" in msg.lower()

    def test_generic_error(self) -> None:
        msg = classify_api_error(Exception("Something broke"))
        assert "Something broke" in msg

    def test_long_message_truncated(self) -> None:
        long_msg = "x" * 200
        msg = classify_api_error(Exception(long_msg))
        assert len(msg) <= 90
        assert msg.endswith("...")


# ---------------------------------------------------------------------------
# validate_auth_file
# ---------------------------------------------------------------------------


class TestValidateAuthFile:
    def test_valid_file(self, tmp_path: Path) -> None:
        auth_file = tmp_path / "browser.json"
        auth_file.write_text(json.dumps({"cookie": "abc"}))
        assert validate_auth_file(auth_file) is None

    def test_missing_file(self, tmp_path: Path) -> None:
        result = validate_auth_file(tmp_path / "missing.json")
        assert result is not None
        assert "not found" in result.lower()

    def test_not_a_file(self, tmp_path: Path) -> None:
        result = validate_auth_file(tmp_path)
        assert result is not None
        assert "not a file" in result.lower()

    def test_invalid_json(self, tmp_path: Path) -> None:
        auth_file = tmp_path / "bad.json"
        auth_file.write_text("not json")
        result = validate_auth_file(auth_file)
        assert result is not None
        assert "invalid JSON" in result

    def test_empty_json_object(self, tmp_path: Path) -> None:
        auth_file = tmp_path / "empty.json"
        auth_file.write_text("{}")
        result = validate_auth_file(auth_file)
        assert result is not None
        assert "empty" in result.lower()

    def test_json_array_not_object(self, tmp_path: Path) -> None:
        auth_file = tmp_path / "array.json"
        auth_file.write_text("[]")
        result = validate_auth_file(auth_file)
        assert result is not None
        assert "not a valid" in result.lower()
