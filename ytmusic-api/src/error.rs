//! Error taxonomy for the InnerTube client.
//!
//! The variants are deliberately aligned with the distinctions the Python
//! wrapper's `classify_api_error` draws (see `src/ytmusic_tui/auth.py`), so the
//! TUI layer can eventually map an [`ApiError`] onto the same user-facing
//! one-liners ("Auth expired", "Request timed out", "Not found", ...).

use std::path::PathBuf;

use thiserror::Error;

/// Errors raised while loading authentication material from a browser.json file.
///
/// These mirror the failure points ytmusicapi guards at construction time and
/// the checks in `validate_auth_file`: a missing/unreadable file, malformed
/// JSON, or a cookie that lacks the SAPISID value required to sign requests.
#[derive(Debug, Error)]
pub enum AuthLoadError {
    /// The auth file does not exist at the (expanded) path.
    #[error("auth file not found: {0}")]
    FileMissing(PathBuf),

    /// The auth file exists but could not be read.
    #[error("cannot read auth file {path}: {source}")]
    FileUnreadable {
        /// The path that could not be read.
        path: PathBuf,
        /// The underlying I/O error.
        source: std::io::Error,
    },

    /// The auth file is not a JSON object of header name -> value.
    #[error("auth file is not valid JSON: {0}")]
    NotJson(String),

    /// The `Cookie` header is absent (case-insensitive lookup failed).
    #[error("auth file is missing the 'Cookie' header")]
    MissingCookie,

    /// The `Cookie` header is present but carries no SAPISID value.
    ///
    /// ytmusicapi reads `__Secure-3PAPISID`; we additionally accept the plain
    /// `SAPISID` cookie as a fallback. Neither was found.
    #[error("cookie is missing the required SAPISID value (__Secure-3PAPISID)")]
    MissingSapisid,
}

/// Errors raised while talking to the InnerTube API.
#[derive(Debug, Error)]
pub enum ApiError {
    /// Loading or interpreting the auth material failed.
    #[error(transparent)]
    Auth(#[from] AuthLoadError),

    /// A transport-level failure (DNS, connect, TLS, timeout, ...).
    ///
    /// Note: reqwest errors can carry the request URL, which includes the
    /// InnerTube `key=` query parameter. That parameter is the public shared
    /// web-client API key, identical for every user — not a user secret.
    #[error("transport error: {0}")]
    Transport(#[from] reqwest::Error),

    /// The server returned a non-success HTTP status.
    #[error("server returned HTTP {status}: {message}")]
    Http {
        /// The HTTP status code.
        status: u16,
        /// The error message extracted from the response body, if any.
        message: String,
    },

    /// The response body could not be parsed into the expected shape.
    ///
    /// This is the "valid-looking but logged-out" signal: the request itself
    /// succeeded (HTTP 200) but the JSON did not carry the expected structure.
    #[error("failed to parse response: {0}")]
    Parse(String),

    /// A mutation request completed (HTTP 200) but the service rejected it
    /// logically — no `playlistId` returned, `status` was not
    /// `STATUS_SUCCEEDED`, or the removal target was absent.
    ///
    /// Mirrors `api.py`'s `MutationFailedError`: the variant carries the
    /// verbatim user-facing string already suitable for display in a toast.
    /// `classify_api_error` surfaces it unchanged.
    #[error("{0}")]
    MutationFailed(String),
}
