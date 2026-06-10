//! Error classification: map an [`ApiError`] onto the same user-facing
//! one-liners that `src/ytmusic_tui/auth.py::classify_api_error` produces.
//!
//! The Python function branches on:
//! 1. Auth-error patterns in the exception message
//!    (`"Login Required"`, `"UNAUTHENTICATED"`, `"401"`, `"403"`, …)
//! 2. `MutationFailedError` — surfaces the message verbatim
//! 3. `"oauth json provided"` / `"oauth_credentials"` — broken auth file
//! 4. Timeout keywords
//! 5. Network / connection keywords
//! 6. Not-found / 404
//! 7. Rate-limit keywords
//! 8. Generic fallback: the error message, truncated to 80 characters
//!
//! This module mirrors all eight branches for the Rust [`ApiError`] taxonomy.

use crate::error::{ApiError, AuthLoadError};

/// The auth-error patterns from `auth.py::_AUTH_ERROR_PATTERNS`.
const AUTH_PATTERNS: &[&str] = &[
    "Login Required",
    "Request had invalid authentication credentials",
    "The request is missing a valid API key",
    "UNAUTHENTICATED",
    "403",
    "401",
];

/// Return a user-facing one-line error message for `err`.
///
/// The output is suitable for display in a status-bar toast.  It mirrors
/// `classify_api_error` from `src/ytmusic_tui/auth.py` verbatim — both the
/// branch logic and the exact message strings — so the TUI layer produces
/// identical toasts for the same situations in both implementations.
pub fn classify_api_error(err: &ApiError) -> String {
    match err {
        // Auth-load errors are always auth failures.
        ApiError::Auth(auth_err) => classify_auth_load(auth_err),

        // MutationFailed carries a precise, user-facing message already.
        // Surface it verbatim (mirrors api.py's `isinstance(exc, MutationFailedError)`
        // branch which returns `str(exc)` without decoration).
        ApiError::MutationFailed(msg) => msg.clone(),

        // HTTP status-code errors: check for auth status codes before the
        // generic message-text scan.
        ApiError::Http { status, message } => {
            if matches!(status, 401 | 403) {
                return "Auth expired — run: ytmusic-tui auth".to_owned();
            }
            if *status == 404 {
                return "Not found".to_owned();
            }
            // Fall through to the message-text scan using the server's message.
            classify_message(message)
        }

        // Transport-level errors (DNS, connect, TLS, timeout, reqwest
        // errors): use the Display representation as the text to scan.
        ApiError::Transport(req_err) => {
            let text = req_err.to_string();
            classify_message(&text)
        }

        // Parse errors (logged-out 200 responses lacking expected structure).
        // The Python analogue hits the generic branch; the message often
        // contains something like "KeyError" from ytmusicapi — no special case.
        ApiError::Parse(msg) => classify_message(msg),
    }
}

// ---------------------------------------------------------------------------
// Branch helpers
// ---------------------------------------------------------------------------

/// Classify an [`AuthLoadError`].
///
/// All `AuthLoadError` variants are auth-file problems, so the canonical
/// message is used for each. The `MissingSapisid` case mirrors api.py's
/// `"oauth_credentials"` branch (broken auth file rather than expired session).
fn classify_auth_load(err: &AuthLoadError) -> String {
    match err {
        // A missing SAPISID usually means the file came from a telemetry
        // request that lacks the required auth cookies — matches the
        // "oauth_credentials" pattern in auth.py.
        AuthLoadError::MissingSapisid | AuthLoadError::MissingCookie => {
            "Auth file looks broken — run: ytmusic-tui auth".to_owned()
        }
        _ => "Auth expired — run: ytmusic-tui auth".to_owned(),
    }
}

/// Scan an arbitrary error message for the same keyword patterns that
/// `classify_api_error` applies after the typed-exception branches.
fn classify_message(text: &str) -> String {
    // Auth patterns (case-sensitive, mirrors Python `any(pattern in msg for ...)`).
    for pattern in AUTH_PATTERNS {
        if text.contains(pattern) {
            return "Auth expired — run: ytmusic-tui auth".to_owned();
        }
    }

    // OAuth / broken auth-file heuristic.
    let lower = text.to_lowercase();
    if lower.contains("oauth json provided") || lower.contains("oauth_credentials") {
        return "Auth file looks broken — run: ytmusic-tui auth".to_owned();
    }

    // Timeout.
    if lower.contains("timeout") || lower.contains("timed out") {
        return "Request timed out — check your connection".to_owned();
    }

    // Network / connectivity.
    if lower.contains("network") || lower.contains("connection") || lower.contains("unreachable") {
        return "Network error — check your connection".to_owned();
    }

    // Not found / 404 in the message text.
    if lower.contains("not found") || lower.contains("404") {
        return "Not found".to_owned();
    }

    // Rate limit.
    if lower.contains("rate") && lower.contains("limit") {
        return "Rate limited — try again later".to_owned();
    }

    // Generic fallback: truncate to 80 characters and prefix with "Error: ".
    // Mirrors: `text[:77] + "..."` when `len(text) > 80`.
    if text.len() > 80 {
        format!("Error: {}...", &text[..77])
    } else {
        format!("Error: {text}")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::AuthLoadError;
    use std::path::PathBuf;

    fn http_err(status: u16, msg: &str) -> ApiError {
        ApiError::Http {
            status,
            message: msg.to_owned(),
        }
    }

    // --- Auth variants ---

    #[test]
    fn auth_file_missing_is_auth_expired() {
        let err = ApiError::Auth(AuthLoadError::FileMissing(PathBuf::from("/no/such/file")));
        assert_eq!(
            classify_api_error(&err),
            "Auth expired — run: ytmusic-tui auth"
        );
    }

    #[test]
    fn auth_missing_sapisid_is_broken_file() {
        let err = ApiError::Auth(AuthLoadError::MissingSapisid);
        assert_eq!(
            classify_api_error(&err),
            "Auth file looks broken — run: ytmusic-tui auth"
        );
    }

    #[test]
    fn auth_missing_cookie_is_broken_file() {
        let err = ApiError::Auth(AuthLoadError::MissingCookie);
        assert_eq!(
            classify_api_error(&err),
            "Auth file looks broken — run: ytmusic-tui auth"
        );
    }

    // --- HTTP status ---

    #[test]
    fn http_401_is_auth_expired() {
        assert_eq!(
            classify_api_error(&http_err(401, "Unauthorized")),
            "Auth expired — run: ytmusic-tui auth"
        );
    }

    #[test]
    fn http_403_is_auth_expired() {
        assert_eq!(
            classify_api_error(&http_err(403, "Forbidden")),
            "Auth expired — run: ytmusic-tui auth"
        );
    }

    #[test]
    fn http_404_is_not_found() {
        assert_eq!(classify_api_error(&http_err(404, "Not Found")), "Not found");
    }

    #[test]
    fn http_500_falls_through_to_message_scan() {
        let err = http_err(500, "Rate limit exceeded");
        assert_eq!(classify_api_error(&err), "Rate limited — try again later");
    }

    // --- Auth patterns in message text ---

    #[test]
    fn login_required_in_message_is_auth_expired() {
        let err = ApiError::Parse("Login Required".to_owned());
        assert_eq!(
            classify_api_error(&err),
            "Auth expired — run: ytmusic-tui auth"
        );
    }

    #[test]
    fn unauthenticated_in_message_is_auth_expired() {
        let err = ApiError::Parse("UNAUTHENTICATED".to_owned());
        assert_eq!(
            classify_api_error(&err),
            "Auth expired — run: ytmusic-tui auth"
        );
    }

    #[test]
    fn four_oh_three_in_message_is_auth_expired() {
        let err = ApiError::Parse("403 error from server".to_owned());
        assert_eq!(
            classify_api_error(&err),
            "Auth expired — run: ytmusic-tui auth"
        );
    }

    // --- OAuth / broken file ---

    #[test]
    fn oauth_json_provided_is_broken_file() {
        let err = ApiError::Parse("oauth json provided instead of browser".to_owned());
        assert_eq!(
            classify_api_error(&err),
            "Auth file looks broken — run: ytmusic-tui auth"
        );
    }

    #[test]
    fn oauth_credentials_is_broken_file() {
        let err = ApiError::Parse("oauth_credentials key found".to_owned());
        assert_eq!(
            classify_api_error(&err),
            "Auth file looks broken — run: ytmusic-tui auth"
        );
    }

    // --- Timeout ---

    #[test]
    fn timeout_in_message() {
        let err = ApiError::Parse("request timeout exceeded".to_owned());
        assert_eq!(
            classify_api_error(&err),
            "Request timed out — check your connection"
        );
    }

    #[test]
    fn timed_out_in_message() {
        let err = ApiError::Parse("operation timed out".to_owned());
        assert_eq!(
            classify_api_error(&err),
            "Request timed out — check your connection"
        );
    }

    // --- Network ---

    #[test]
    fn network_error_in_message() {
        let err = ApiError::Parse("network failure".to_owned());
        assert_eq!(
            classify_api_error(&err),
            "Network error — check your connection"
        );
    }

    #[test]
    fn connection_refused_in_message() {
        let err = ApiError::Parse("Connection refused".to_owned());
        assert_eq!(
            classify_api_error(&err),
            "Network error — check your connection"
        );
    }

    #[test]
    fn unreachable_in_message() {
        let err = ApiError::Parse("host unreachable".to_owned());
        assert_eq!(
            classify_api_error(&err),
            "Network error — check your connection"
        );
    }

    // --- Not found ---

    #[test]
    fn not_found_in_message() {
        let err = ApiError::Parse("not found: resource missing".to_owned());
        assert_eq!(classify_api_error(&err), "Not found");
    }

    // --- Rate limit ---

    #[test]
    fn rate_limit_in_message() {
        let err = ApiError::Parse("Rate limit exceeded".to_owned());
        assert_eq!(classify_api_error(&err), "Rate limited — try again later");
    }

    // --- MutationFailed ---

    #[test]
    fn mutation_failed_surfaced_verbatim() {
        let err = ApiError::MutationFailed("Track was not found in the playlist".to_owned());
        assert_eq!(
            classify_api_error(&err),
            "Track was not found in the playlist"
        );
    }

    #[test]
    fn mutation_failed_playlist_not_created() {
        let err = ApiError::MutationFailed("Playlist was not created".to_owned());
        assert_eq!(classify_api_error(&err), "Playlist was not created");
    }

    // --- Generic fallback ---

    #[test]
    fn generic_short_error_prefixed() {
        let err = ApiError::Parse("Something broke".to_owned());
        assert_eq!(classify_api_error(&err), "Error: Something broke");
    }

    #[test]
    fn generic_long_error_truncated_to_80() {
        let long_msg = "x".repeat(200);
        let err = ApiError::Parse(long_msg);
        let result = classify_api_error(&err);
        // "Error: " (7) + 77 chars + "..." (3) = 87 chars displayed
        assert!(result.starts_with("Error: "));
        assert!(result.ends_with("..."));
        // The underlying text portion is ≤ 77 chars.
        assert!(result.len() <= 87);
    }
}
