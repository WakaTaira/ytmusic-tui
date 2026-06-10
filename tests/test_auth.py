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


# ---------------------------------------------------------------------------
# run_auth_setup (ytmusic-tui auth)
# ---------------------------------------------------------------------------


class TestRunAuthSetup:
    def _headers(self) -> str:
        return "cookie: SAPISID=abc; OTHER=x\nuser-agent: test\n"

    def test_success_writes_and_validates(self, tmp_path) -> None:
        import io
        import json
        from unittest.mock import patch

        from ytmusic_tui.auth import run_auth_setup

        target = tmp_path / "browser.json"

        def fake_setup(filepath: str, headers_raw: str) -> str:
            assert "SAPISID" in headers_raw
            payload = json.dumps({"cookie": "SAPISID=abc"})
            target.write_text(payload)
            return payload

        with patch("ytmusicapi.setup", side_effect=fake_setup):
            code = run_auth_setup(target, input_stream=io.StringIO(self._headers()))

        assert code == 0
        assert target.exists()

    def test_empty_input_aborts(self, tmp_path) -> None:
        import io
        from unittest.mock import MagicMock, patch

        from ytmusic_tui.auth import run_auth_setup

        mock_setup = MagicMock()
        with patch("ytmusicapi.setup", mock_setup):
            code = run_auth_setup(tmp_path / "browser.json", input_stream=io.StringIO("   \n"))

        assert code == 1
        mock_setup.assert_not_called()

    def test_setup_failure_returns_error(self, tmp_path) -> None:
        import io
        from unittest.mock import patch

        from ytmusic_tui.auth import run_auth_setup

        with patch("ytmusicapi.setup", side_effect=Exception("bad headers")):
            code = run_auth_setup(
                tmp_path / "browser.json", input_stream=io.StringIO(self._headers())
            )

        assert code == 1

    def test_invalid_result_file_returns_error(self, tmp_path) -> None:
        import io
        from unittest.mock import patch

        from ytmusic_tui.auth import run_auth_setup

        target = tmp_path / "browser.json"

        def fake_setup(filepath: str, headers_raw: str) -> str:
            target.write_text("not json")
            return "not json"

        with patch("ytmusicapi.setup", side_effect=fake_setup):
            code = run_auth_setup(target, input_stream=io.StringIO(self._headers()))

        assert code == 1

    def test_creates_parent_directory(self, tmp_path) -> None:
        import io
        import json
        from unittest.mock import patch

        from ytmusic_tui.auth import run_auth_setup

        target = tmp_path / "nested" / "dir" / "browser.json"

        def fake_setup(filepath: str, headers_raw: str) -> str:
            payload = json.dumps({"cookie": "x"})
            target.write_text(payload)
            return payload

        with patch("ytmusicapi.setup", side_effect=fake_setup):
            code = run_auth_setup(target, input_stream=io.StringIO(self._headers()))

        assert code == 0
        assert target.parent.is_dir()
