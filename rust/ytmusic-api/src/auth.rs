//! Browser-header authentication: load a ytmusicapi-format `browser.json`,
//! extract the values needed to sign requests, and compute the per-request
//! `SAPISIDHASH` authorization header.
//!
//! Reference: `ytmusicapi/helpers.py` (`get_authorization`, `sapisid_from_cookie`)
//! and `ytmusicapi/ytmusic.py` (`YTMusicBase.__init__` / `headers`).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use sha1::{Digest, Sha1};

use crate::error::AuthLoadError;

/// The origin ytmusicapi signs requests against. The `SAPISIDHASH` input is
/// `"{sapisid} {origin}"`, and the request also sends this as the `Origin`
/// header. Matches `YTM_DOMAIN` in `ytmusicapi/constants.py`.
pub const YTM_ORIGIN: &str = "https://music.youtube.com";

/// Authentication material parsed from a ytmusicapi browser-auth file.
///
/// Holds the full set of raw HTTP headers (with case-insensitive lookup) plus
/// the two derived values the signing flow needs: the SAPISID (from the Cookie)
/// and the origin (from the `Origin`/`x-origin` header, defaulting to
/// [`YTM_ORIGIN`]).
#[derive(Clone)]
pub struct BrowserAuth {
    /// Header name (lowercased) -> raw header value, as read from the file.
    headers: HashMap<String, String>,
    /// The SAPISID cookie value used to compute the authorization hash.
    sapisid: String,
    /// The origin string used both in the hash input and the `Origin` header.
    origin: String,
}

impl std::fmt::Debug for BrowserAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowserAuth")
            .field("origin", &self.origin)
            .field(
                "headers",
                &format!("[{} headers, redacted]", self.headers.len()),
            )
            .field("sapisid", &"[REDACTED]")
            .finish()
    }
}

impl BrowserAuth {
    /// Load and validate a browser-auth JSON file.
    ///
    /// `path` may start with `~`, which is expanded to the user's home
    /// directory. The file must be a JSON object mapping header names to string
    /// values, must contain a `Cookie` header (case-insensitive), and that
    /// cookie must carry a SAPISID value.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, AuthLoadError> {
        let expanded = expand_tilde(path.as_ref());
        let raw = std::fs::read_to_string(&expanded).map_err(|source| {
            if source.kind() == std::io::ErrorKind::NotFound {
                AuthLoadError::FileMissing(expanded.clone())
            } else {
                AuthLoadError::FileUnreadable {
                    path: expanded.clone(),
                    source,
                }
            }
        })?;
        Self::from_json_str(&raw)
    }

    /// Parse and validate browser-auth material from a JSON string.
    ///
    /// Split out from [`load`](Self::load) so the parsing and validation logic
    /// can be unit-tested without touching the filesystem.
    pub fn from_json_str(raw: &str) -> Result<Self, AuthLoadError> {
        let parsed: serde_json::Value =
            serde_json::from_str(raw).map_err(|e| AuthLoadError::NotJson(e.to_string()))?;
        let object = parsed
            .as_object()
            .ok_or_else(|| AuthLoadError::NotJson("expected a JSON object".to_owned()))?;

        // Lowercase keys for case-insensitive lookup, keeping only string values
        // (ytmusicapi browser files are flat string maps).
        let mut headers: HashMap<String, String> = HashMap::with_capacity(object.len());
        for (key, value) in object {
            if let Some(text) = value.as_str() {
                headers.insert(key.to_ascii_lowercase(), text.to_owned());
            }
        }

        let cookie = headers.get("cookie").ok_or(AuthLoadError::MissingCookie)?;
        let sapisid = sapisid_from_cookie(cookie).ok_or(AuthLoadError::MissingSapisid)?;

        // ytmusicapi: origin = headers["origin"] or headers["x-origin"].
        // Default to the YTM origin so a file that omits both still signs
        // correctly against music.youtube.com.
        let origin = headers
            .get("origin")
            .or_else(|| headers.get("x-origin"))
            .map(String::as_str)
            .unwrap_or(YTM_ORIGIN)
            .to_owned();

        Ok(Self {
            headers,
            sapisid,
            origin,
        })
    }

    /// Look up a header by name, case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }

    /// The raw `Cookie` header value to replay on every request.
    pub fn cookie(&self) -> &str {
        // Presence is guaranteed by construction (MissingCookie otherwise).
        self.headers
            .get("cookie")
            .map(String::as_str)
            .unwrap_or_default()
    }

    /// The origin used for signing and the `Origin` request header.
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// The SAPISID value extracted from the cookie.
    ///
    /// Only needed within the crate to compute the signing hash — not part of
    /// the public API surface.
    pub(crate) fn sapisid(&self) -> &str {
        &self.sapisid
    }

    /// Compute the `SAPISIDHASH` authorization header for the current instant.
    ///
    /// Recomputed per request because the hash embeds a Unix timestamp.
    pub fn authorization(&self) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        sapisid_authorization(self.sapisid(), &self.origin, now)
    }
}

/// Expand a leading `~` to the user's home directory.
///
/// Only a bare `~` or a `~/...` prefix is expanded (the common case for config
/// paths); any other path is returned unchanged. Falls back to the original
/// path when `$HOME` is unavailable.
fn expand_tilde(path: &Path) -> PathBuf {
    expand_tilde_with(path, home_dir())
}

/// Inner implementation that accepts the home directory as a parameter.
///
/// Separating the home-resolution from the expansion logic makes tilde
/// expansion testable without touching the process environment (which would
/// race under the parallel test harness).
fn expand_tilde_with(path: &Path, home: Option<PathBuf>) -> PathBuf {
    let Some(text) = path.to_str() else {
        return path.to_path_buf();
    };
    if text == "~"
        && let Some(h) = home
    {
        return h;
    } else if let Some(rest) = text.strip_prefix("~/")
        && let Some(h) = home
    {
        return h.join(rest);
    }
    path.to_path_buf()
}

/// Resolve the user's home directory from the environment.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Extract the SAPISID value from a raw `Cookie` header.
///
/// Mirrors `ytmusicapi.helpers.sapisid_from_cookie`, which strips quotes and
/// reads the `__Secure-3PAPISID` cookie. As a fallback (per the M3b directive)
/// the plain `SAPISID` cookie is accepted when the `__Secure-` variant is
/// absent. Returns `None` if neither is present.
fn sapisid_from_cookie(raw_cookie: &str) -> Option<String> {
    let cleaned = raw_cookie.replace('"', "");
    let mut plain_sapisid: Option<String> = None;

    for pair in cleaned.split(';') {
        let pair = pair.trim();
        let Some((name, value)) = pair.split_once('=') else {
            continue;
        };
        let name = name.trim();
        let value = value.trim();
        if name == "__Secure-3PAPISID" {
            // Preferred source, matches ytmusicapi exactly: return immediately.
            return Some(value.to_owned());
        }
        if name == "SAPISID" {
            plain_sapisid = Some(value.to_owned());
        }
    }

    plain_sapisid
}

/// Compute the `SAPISIDHASH` authorization header value.
///
/// Exact algorithm from `ytmusicapi.helpers.get_authorization`:
/// `"SAPISIDHASH {ts}_{sha1_hex}"` where the SHA-1 input is
/// `"{ts} {sapisid} {origin}"`. Taking the timestamp as a parameter keeps this
/// pure and unit-testable against fixed vectors.
pub fn sapisid_authorization(sapisid: &str, origin: &str, unix_ts: u64) -> String {
    let input = format!("{unix_ts} {sapisid} {origin}");
    let mut hasher = Sha1::new();
    hasher.update(input.as_bytes());
    let digest = hasher.finalize();

    let mut hex = String::with_capacity(40);
    for b in digest {
        // Infallible when writing to a String; two lowercase hex digits per byte
        // (matches Python's hexdigest()).
        write!(hex, "{b:02x}").unwrap();
    }

    format!("SAPISIDHASH {unix_ts}_{hex}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::AuthLoadError;

    // ---- SAPISIDHASH vectors -------------------------------------------------
    //
    // Both vectors were generated with ytmusicapi's own `get_authorization`
    // under a pinned clock (see the M3b report for the exact Python command).
    // Hardcoding them locks our algorithm to the reference implementation.

    #[test]
    fn sapisidhash_matches_ytmusicapi_vector_1() {
        // ts=1700000000, sapisid="TEST_SAPISID_VALUE", origin=YTM_ORIGIN
        let got = sapisid_authorization("TEST_SAPISID_VALUE", YTM_ORIGIN, 1_700_000_000);
        assert_eq!(
            got,
            "SAPISIDHASH 1700000000_ea470418ef844e689a8ef5e5eed68b92a6642fb5"
        );
    }

    #[test]
    fn sapisidhash_matches_ytmusicapi_vector_2() {
        // ts=1609459200, sapisid="AbCdEf0123456789", origin=YTM_ORIGIN
        let got = sapisid_authorization("AbCdEf0123456789", YTM_ORIGIN, 1_609_459_200);
        assert_eq!(
            got,
            "SAPISIDHASH 1609459200_f99e5d30bf15fe76d1bfc4b382e08aefd8e7b793"
        );
    }

    #[test]
    fn sapisidhash_is_lowercase_hex_of_length_40() {
        let got = sapisid_authorization("x", YTM_ORIGIN, 1);
        let hex = got.split('_').nth(1).unwrap();
        assert_eq!(hex.len(), 40);
        assert!(
            hex.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    // ---- browser.json parsing ------------------------------------------------

    /// A minimal-but-valid browser auth object. The cookie carries a
    /// `__Secure-3PAPISID` value; the SAPISID itself is a dummy string (no real
    /// credential material appears in tests).
    fn good_auth_json() -> &'static str {
        r#"{
            "Cookie": "__Secure-3PAPISID=DUMMY_SAPISID_AAA; OTHER=1",
            "Authorization": "SAPISIDHASH 123_abc",
            "Origin": "https://music.youtube.com",
            "User-Agent": "test-agent",
            "X-Goog-AuthUser": "0"
        }"#
    }

    #[test]
    fn parses_good_browser_json() {
        let auth = BrowserAuth::from_json_str(good_auth_json()).expect("parses");
        assert_eq!(auth.sapisid(), "DUMMY_SAPISID_AAA");
        assert_eq!(auth.origin(), "https://music.youtube.com");
        // Cookie replayed verbatim.
        assert!(auth.cookie().contains("__Secure-3PAPISID="));
    }

    #[test]
    fn header_lookup_is_case_insensitive() {
        let auth = BrowserAuth::from_json_str(good_auth_json()).expect("parses");
        // File key was "User-Agent"; look it up in several cases.
        assert_eq!(auth.header("user-agent"), Some("test-agent"));
        assert_eq!(auth.header("User-Agent"), Some("test-agent"));
        assert_eq!(auth.header("USER-AGENT"), Some("test-agent"));
        assert_eq!(auth.header("x-goog-authuser"), Some("0"));
    }

    #[test]
    fn cookie_key_is_case_insensitive() {
        // Lowercase "cookie" key must be accepted just like "Cookie".
        let json = r#"{ "cookie": "__Secure-3PAPISID=ABC", "authorization": "x" }"#;
        let auth = BrowserAuth::from_json_str(json).expect("parses");
        assert_eq!(auth.sapisid(), "ABC");
    }

    #[test]
    fn accepts_plain_sapisid_fallback() {
        // No __Secure-3PAPISID, but a plain SAPISID cookie is present.
        let json = r#"{ "Cookie": "SAPISID=PLAIN_VALUE; X=1", "Authorization": "y" }"#;
        let auth = BrowserAuth::from_json_str(json).expect("parses");
        assert_eq!(auth.sapisid(), "PLAIN_VALUE");
    }

    #[test]
    fn prefers_secure_variant_over_plain_sapisid() {
        // When both exist, the __Secure-3PAPISID value wins (matches ytmusicapi).
        let json =
            r#"{ "Cookie": "SAPISID=PLAIN; __Secure-3PAPISID=SECURE", "Authorization": "z" }"#;
        let auth = BrowserAuth::from_json_str(json).expect("parses");
        assert_eq!(auth.sapisid(), "SECURE");
    }

    #[test]
    fn strips_quotes_from_cookie_values() {
        // ytmusicapi does raw_cookie.replace('"', "") before parsing.
        let json = r#"{ "Cookie": "__Secure-3PAPISID=\"QUOTED\"; X=1", "Authorization": "a" }"#;
        let auth = BrowserAuth::from_json_str(json).expect("parses");
        assert_eq!(auth.sapisid(), "QUOTED");
    }

    #[test]
    fn origin_falls_back_to_x_origin_then_default() {
        // No Origin, but X-Origin present.
        let json = r#"{ "Cookie": "__Secure-3PAPISID=A", "Authorization": "x",
                        "X-Origin": "https://music.youtube.com" }"#;
        let auth = BrowserAuth::from_json_str(json).expect("parses");
        assert_eq!(auth.origin(), "https://music.youtube.com");

        // Neither Origin nor X-Origin: default to YTM_ORIGIN.
        let json2 = r#"{ "Cookie": "__Secure-3PAPISID=A", "Authorization": "x" }"#;
        let auth2 = BrowserAuth::from_json_str(json2).expect("parses");
        assert_eq!(auth2.origin(), YTM_ORIGIN);
    }

    #[test]
    fn rejects_missing_cookie() {
        let json = r#"{ "Authorization": "SAPISIDHASH 1_2", "User-Agent": "x" }"#;
        let err = BrowserAuth::from_json_str(json).expect_err("missing cookie");
        assert!(matches!(err, AuthLoadError::MissingCookie), "got: {err:?}");
    }

    #[test]
    fn rejects_cookie_without_sapisid() {
        // Cookie present but carries neither __Secure-3PAPISID nor SAPISID.
        let json = r#"{ "Cookie": "FOO=bar; BAZ=qux", "Authorization": "x" }"#;
        let err = BrowserAuth::from_json_str(json).expect_err("missing sapisid");
        assert!(matches!(err, AuthLoadError::MissingSapisid), "got: {err:?}");
    }

    #[test]
    fn rejects_non_json() {
        let err = BrowserAuth::from_json_str("not json at all").expect_err("not json");
        assert!(matches!(err, AuthLoadError::NotJson(_)), "got: {err:?}");
    }

    #[test]
    fn rejects_json_array() {
        // Valid JSON, but not an object of headers.
        let err = BrowserAuth::from_json_str("[1, 2, 3]").expect_err("not an object");
        assert!(matches!(err, AuthLoadError::NotJson(_)), "got: {err:?}");
    }

    #[test]
    fn load_missing_file_reports_file_missing() {
        let err = BrowserAuth::load("/nonexistent/path/to/browser.json").expect_err("missing file");
        assert!(matches!(err, AuthLoadError::FileMissing(_)), "got: {err:?}");
    }

    // ---- tilde expansion -----------------------------------------------------

    #[test]
    fn expand_tilde_resolves_home() {
        // Use expand_tilde_with so we can inject a fake home without mutating
        // the process environment (which would race against other tests under
        // the parallel test harness).
        let fake_home = Some(PathBuf::from("/tmp/fake-home"));

        assert_eq!(
            expand_tilde_with(Path::new("~"), fake_home.clone()),
            PathBuf::from("/tmp/fake-home")
        );
        assert_eq!(
            expand_tilde_with(Path::new("~/.config/x"), fake_home.clone()),
            PathBuf::from("/tmp/fake-home/.config/x")
        );
        // A path that merely contains ~ mid-string is left untouched.
        assert_eq!(
            expand_tilde_with(Path::new("/etc/~weird"), fake_home.clone()),
            PathBuf::from("/etc/~weird")
        );
        // When no home is available the path is returned unchanged.
        assert_eq!(expand_tilde_with(Path::new("~"), None), PathBuf::from("~"));
    }

    // ---- Debug redaction -----------------------------------------------------

    #[test]
    fn debug_output_does_not_expose_sapisid_or_cookie() {
        // Construct a BrowserAuth from minimal in-test JSON.  The SAPISID and
        // cookie values are distinct enough that any leakage into the Debug
        // string would be clearly visible.
        let json = r#"{
            "Cookie": "__Secure-3PAPISID=SECRET_SAPISID_VALUE; OTHER=cookie_noise",
            "Authorization": "SAPISIDHASH 1_2",
            "Origin": "https://music.youtube.com"
        }"#;
        let auth = BrowserAuth::from_json_str(json).expect("parses");
        let debug = format!("{auth:?}");

        // The actual SAPISID value must NOT appear.
        assert!(
            !debug.contains("SECRET_SAPISID_VALUE"),
            "SAPISID leaked into Debug output: {debug}"
        );
        // The raw cookie string must NOT appear.
        assert!(
            !debug.contains("cookie_noise"),
            "Cookie leaked into Debug output: {debug}"
        );
        // A placeholder indicating redaction must be present.
        assert!(
            debug.contains("[REDACTED]"),
            "Expected [REDACTED] marker in Debug output: {debug}"
        );
    }
}
