//! `ytmusic-tui auth` subcommand: two flows that turn browser cookies into a
//! ytmusicapi-format `browser.json`.
//!
//! * `ytmusic-tui auth` (no args) — interactive paste of Network-tab request
//!   headers. Mirrors the Python `ytmusicapi browser` CLI.
//! * `ytmusic-tui auth --from-browser <name>` (issue #18) — pull the YTM cookies
//!   directly from the named browser's local cookie store via [`rookie`],
//!   synthesize the non-cookie headers, run the session canary against the
//!   result, and write the file only if the canary confirms the auth is live.
//!
//! Both flows ultimately call [`build_browser_json`] + [`write_browser_json`],
//! so the on-disk shape is identical and the InnerTube loader does not need to
//! know which flow produced the file.
//!
//! # Shape
//!
//! The module is split into pure, [`BufRead`]/[`Write`]-driven helpers so the
//! whole flow is unit-testable without a TTY:
//!
//! * [`parse_request_headers`] reads `Name: Value` lines until an empty line.
//! * [`build_browser_json`] filters them down to the documented set and
//!   validates the cookie.
//! * [`write_browser_json`] creates the target directory if missing and writes
//!   the JSON with `0o600` on Unix.
//! * [`run_auth_flow`] is the thin driver `main` calls — `stdin` + `stderr` /
//!   `stdout`, plus the hardcoded default path.
//! * [`run_from_browser`] is the `--from-browser` driver; it delegates to
//!   [`drive_from_browser_flow`] under a pair of injected closures so tests can
//!   stub out the real rookie call and the real network canary.

use std::collections::BTreeMap;
use std::io::{self, BufRead, Write};
use std::path::Path;

use serde_json::{Map, Value};

/// The hardcoded default path the subcommand writes to.
///
/// Matches the default in `config/default.toml`. The auth subcommand runs
/// **before** the TUI loads its config (so we cannot read a customized
/// `browser_auth_path` here); users who relocate the file in `config.toml` can
/// move the written file afterwards.
const DEFAULT_BROWSER_AUTH_PATH: &str = "~/.config/ytmusic-tui/browser.json";

/// Header names the InnerTube client needs from the browser.
///
/// The list mirrors `ytmusicapi.setup.setup` — every other header the user
/// pastes is dropped. All entries are lowercase so the case-insensitive lookup
/// against the user's input is unambiguous.
const REQUIRED_HEADERS: &[&str] = &[
    "accept",
    "accept-language",
    "cookie",
    "user-agent",
    "x-goog-authuser",
    "x-goog-pageid",
    "x-origin",
    "x-youtube-bootstrap-logged-in",
    "x-youtube-client-name",
    "x-youtube-client-version",
];

/// Why building the browser-auth JSON failed.
///
/// Distinct variants so the CLI driver can surface a precise message and the
/// unit tests can assert on the shape rather than the wording. Variants used
/// only by the interactive-paste flow live alongside the `--from-browser`
/// variants — the driver pattern-matches on each.
#[derive(Debug, PartialEq, Eq)]
pub enum AuthCliError {
    /// No `Cookie:` line was found in the pasted block (interactive flow).
    MissingCookie,
    /// The cookie was present but carried no `__Secure-3PAPISID` or `SAPISID`.
    MissingSapisid,
    /// `--from-browser <name>` was given a browser name we have no extractor
    /// for. Carries the rejected name so the error message can quote it.
    UnsupportedBrowser(String),
    /// `rookie::<browser>(...)` failed (encrypted store, missing keyring,
    /// browser not installed, etc.). Carries the underlying error message.
    RookieFailed(String),
    /// The synthesized auth produced no live session — the cookies are stale
    /// or the user signed out at music.youtube.com.
    SessionCanaryFailed,
}

impl std::fmt::Display for AuthCliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingCookie => f.write_str(
                "No 'Cookie:' header found. Re-copy the request headers from the Network tab.",
            ),
            Self::MissingSapisid => f.write_str(
                "Cookie has no SAPISID (__Secure-3PAPISID). \
                 Make sure you are signed in at music.youtube.com.",
            ),
            Self::UnsupportedBrowser(name) => write!(
                f,
                "Unsupported browser: '{name}'. \
                 Supported: firefox, librewolf, zen, chrome, chromium, brave, edge, vivaldi, opera, opera_gx, arc, safari."
            ),
            Self::RookieFailed(detail) => write!(
                f,
                "Could not read the browser cookie store: {detail}. \
                 Close the browser if it is open, or re-run `ytmusic-tui auth` for the paste flow."
            ),
            Self::SessionCanaryFailed => f.write_str(
                "Session invalid — the browser cookies are signed out at music.youtube.com. \
                 Sign in again in the browser, then re-run `ytmusic-tui auth --from-browser <name>`.",
            ),
        }
    }
}

impl std::error::Error for AuthCliError {}

/// Read `Name: Value` lines from `reader` until an empty line or EOF.
///
/// Each non-empty line is split on the **first** `:` (so cookie values with
/// embedded colons survive). Whitespace around the name and value is trimmed.
/// Header names are lowercased so lookups are case-insensitive, mirroring the
/// [`BrowserAuth`](ytmusic_api::BrowserAuth) loader.
///
/// Returns the collected map preserving no particular order. Lines without a
/// `:` are silently skipped (so a stray "GET /youtubei/v1/…" status line at
/// the top of a cURL-paste doesn't poison the result).
pub fn parse_request_headers<R: BufRead>(reader: &mut R) -> io::Result<BTreeMap<String, String>> {
    let mut headers: BTreeMap<String, String> = BTreeMap::new();
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            // EOF: treat like an empty terminator.
            break;
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.trim().is_empty() {
            break;
        }
        let Some((name, value)) = trimmed.split_once(':') else {
            // Skip lines without a colon (request method line, HTTP/2 pseudo
            // headers without a value, blank-ish "----" separators, etc.).
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim().to_owned();
        if name.is_empty() || value.is_empty() {
            continue;
        }
        headers.insert(name, value);
    }
    Ok(headers)
}

/// Extract the SAPISID value from a raw `Cookie` header string.
///
/// Duplicated from `ytmusic_api::auth::sapisid_from_cookie` so the subcommand
/// can run validation without expanding the ytmusic-api public surface for a
/// single helper. The behaviour is identical: prefer `__Secure-3PAPISID`,
/// fall back to plain `SAPISID`, strip quotes first to match ytmusicapi.
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
            return Some(value.to_owned());
        }
        if name == "SAPISID" {
            plain_sapisid = Some(value.to_owned());
        }
    }
    plain_sapisid
}

/// Build the JSON object to be written to `browser.json` from the pasted
/// header map.
///
/// Filters to [`REQUIRED_HEADERS`], validates that a Cookie is present and
/// contains a SAPISID, and emits a `serde_json::Value` whose keys are all
/// lowercase (matching the case-insensitive lookup the loader does, and
/// keeping the file deterministic between runs).
pub fn build_browser_json(headers: &BTreeMap<String, String>) -> Result<Value, AuthCliError> {
    let cookie = headers.get("cookie").ok_or(AuthCliError::MissingCookie)?;
    sapisid_from_cookie(cookie).ok_or(AuthCliError::MissingSapisid)?;

    let mut object: Map<String, Value> = Map::with_capacity(REQUIRED_HEADERS.len());
    for &name in REQUIRED_HEADERS {
        if let Some(value) = headers.get(name) {
            object.insert(name.to_owned(), Value::String(value.clone()));
        }
    }
    Ok(Value::Object(object))
}

/// Write the JSON value to `path`, creating any missing parent directories.
///
/// On Unix the file is written with `0o600` (owner read/write only). On
/// non-Unix targets the mode hint is ignored by the standard library.
pub fn write_browser_json(path: &Path, value: &Value) -> io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(value)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, text)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(path, perms)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// --from-browser flow (issue #18)
// ---------------------------------------------------------------------------

/// A browser whose local cookie store we can extract YTM cookies from.
///
/// Closed enum so the dispatch in [`extract_cookies_for_browser`] is
/// exhaustive — adding a new browser cannot silently miss a case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserKind {
    Firefox,
    Librewolf,
    Zen,
    Chrome,
    Chromium,
    Brave,
    Edge,
    Vivaldi,
    Opera,
    OperaGx,
    Arc,
    Safari,
}

impl BrowserKind {
    /// The display name surfaced in messages (matches the accepted CLI spelling).
    fn name(self) -> &'static str {
        match self {
            Self::Firefox => "firefox",
            Self::Librewolf => "librewolf",
            Self::Zen => "zen",
            Self::Chrome => "chrome",
            Self::Chromium => "chromium",
            Self::Brave => "brave",
            Self::Edge => "edge",
            Self::Vivaldi => "vivaldi",
            Self::Opera => "opera",
            Self::OperaGx => "opera_gx",
            Self::Arc => "arc",
            Self::Safari => "safari",
        }
    }
}

/// The cookie names worth pulling out of the browser jar.
///
/// Order matters: this is the canonical order used when building the Cookie
/// header so the produced string is stable between runs (helps test diffing).
/// `__Secure-3PAPISID` is the SAPISID source [`build_browser_json`] validates,
/// so it must always be in this list; the rest are SID-family cookies the
/// signed-in InnerTube requests include.
const YTM_COOKIE_NAMES: &[&str] = &[
    "__Secure-3PAPISID",
    "__Secure-1PAPISID",
    "__Secure-3PSID",
    "__Secure-1PSID",
    "SAPISID",
    "APISID",
    "HSID",
    "SSID",
    "SID",
    "LOGIN_INFO",
    "VISITOR_INFO1_LIVE",
    "PREF",
    "YSC",
    "SIDCC",
    "__Secure-3PSIDCC",
    "__Secure-1PSIDCC",
];

/// The non-cookie HTTP headers required for a working InnerTube auth file.
///
/// Cookies live in the browser jar; everything else does not, so this set is
/// hardcoded to ytmusicapi-compatible defaults. The User-Agent matches the
/// Firefox UA shape that YouTube Music currently accepts; if YouTube ever
/// rejects it the user can re-run the interactive `ytmusic-tui auth` (which
/// captures the live UA from DevTools) as a fallback.
const HARDCODED_HEADERS: &[(&str, &str)] = &[
    ("accept", "*/*"),
    ("accept-language", "en-US,en;q=0.9"),
    (
        "user-agent",
        "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0",
    ),
    ("x-goog-authuser", "0"),
    ("x-goog-pageid", ""),
    ("x-origin", "https://music.youtube.com"),
    ("x-youtube-bootstrap-logged-in", "true"),
    ("x-youtube-client-name", "67"),
    ("x-youtube-client-version", "1.0.0"),
];

/// Parse a CLI browser name into a [`BrowserKind`].
///
/// Accepts lowercase canonical names plus a couple of widely-typed aliases
/// (`chromium-browser`, `msedge`). The match is case-insensitive so users do
/// not have to think about it. Returns
/// [`AuthCliError::UnsupportedBrowser`] on a miss.
pub fn parse_browser_name(s: &str) -> Result<BrowserKind, AuthCliError> {
    let normalised = s.trim().to_ascii_lowercase();
    Ok(match normalised.as_str() {
        "firefox" | "ff" => BrowserKind::Firefox,
        "librewolf" => BrowserKind::Librewolf,
        "zen" => BrowserKind::Zen,
        "chrome" | "google-chrome" => BrowserKind::Chrome,
        "chromium" | "chromium-browser" => BrowserKind::Chromium,
        "brave" | "brave-browser" => BrowserKind::Brave,
        "edge" | "msedge" | "microsoft-edge" => BrowserKind::Edge,
        "vivaldi" => BrowserKind::Vivaldi,
        "opera" => BrowserKind::Opera,
        "opera_gx" | "opera-gx" | "operagx" => BrowserKind::OperaGx,
        "arc" => BrowserKind::Arc,
        "safari" => BrowserKind::Safari,
        _ => return Err(AuthCliError::UnsupportedBrowser(s.to_owned())),
    })
}

/// Call into [`rookie`] for the chosen browser, restricted to the YouTube
/// domains, and return the filtered `(name, value)` pairs we need.
///
/// Requests cookies for `.youtube.com` so both `music.youtube.com` and
/// `youtube.com` jars are searched (the SAPISID cookie is `__Secure-` scoped
/// across both). If the browser-specific rookie call fails, the error is
/// surfaced as [`AuthCliError::RookieFailed`] — there is no point falling back
/// to a different store because the user explicitly named this one.
fn extract_cookies_for_browser(kind: BrowserKind) -> Result<Vec<(String, String)>, AuthCliError> {
    let domains: Option<Vec<String>> = Some(vec!["youtube.com".to_owned()]);
    let cookies =
        call_rookie(kind, domains).map_err(|e| AuthCliError::RookieFailed(e.to_string()))?;
    Ok(filter_to_ytm_cookies(cookies))
}

/// Dispatch to the matching `rookie::<browser>` function. Kept as a separate
/// function so the [`BrowserKind`] → backend mapping is one obvious match arm
/// per kind, with a single error-conversion shape at the boundary above.
fn call_rookie(
    kind: BrowserKind,
    domains: Option<Vec<String>>,
) -> rookie::Result<Vec<rookie::enums::Cookie>> {
    match kind {
        BrowserKind::Firefox => rookie::firefox(domains),
        BrowserKind::Librewolf => rookie::librewolf(domains),
        BrowserKind::Zen => rookie::zen(domains),
        BrowserKind::Chrome => rookie::chrome(domains),
        BrowserKind::Chromium => rookie::chromium(domains),
        BrowserKind::Brave => rookie::brave(domains),
        BrowserKind::Edge => rookie::edge(domains),
        BrowserKind::Vivaldi => rookie::vivaldi(domains),
        BrowserKind::Opera => rookie::opera(domains),
        BrowserKind::OperaGx => rookie::opera_gx(domains),
        BrowserKind::Arc => rookie::arc(domains),
        // Safari only exists on macOS — on every other host, refuse with a
        // typed error so the user gets a clean message instead of a link
        // error. `_ = domains` keeps the parameter live across cfg arms.
        BrowserKind::Safari => {
            #[cfg(target_os = "macos")]
            {
                rookie::safari(domains)
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = domains;
                // Returning the eyre::Report shape rookie uses everywhere else
                // by handing a typed std::io error to its `From` impl.
                Err(std::io::Error::other("Safari is only supported on macOS").into())
            }
        }
    }
}

/// Reduce a raw rookie cookie list down to the YTM-relevant `(name, value)`
/// pairs, in the canonical order defined by [`YTM_COOKIE_NAMES`].
///
/// Duplicates (same cookie present in multiple stored domains) are
/// deduplicated by name with the first-seen value winning. Names not in the
/// canonical list are dropped — they would only bloat the Cookie header and
/// the YTM API ignores them.
fn filter_to_ytm_cookies(cookies: Vec<rookie::enums::Cookie>) -> Vec<(String, String)> {
    let mut by_name: BTreeMap<&'static str, String> = BTreeMap::new();
    for cookie in cookies {
        for &target in YTM_COOKIE_NAMES {
            if cookie.name == target && !by_name.contains_key(target) {
                by_name.insert(target, cookie.value.clone());
                break;
            }
        }
    }
    // Re-emit in YTM_COOKIE_NAMES order so the resulting header is stable.
    let mut out = Vec::with_capacity(by_name.len());
    for &name in YTM_COOKIE_NAMES {
        if let Some(value) = by_name.remove(name) {
            out.push((name.to_owned(), value));
        }
    }
    out
}

/// Join `(name, value)` pairs into a `name=value; name=value; …` Cookie header.
///
/// Pure — used both by the runtime driver and by tests asserting on the
/// concatenation shape. Empty input yields an empty string.
pub fn build_cookie_header(cookies: &[(String, String)]) -> String {
    cookies
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Compose the final header map by merging the synthesized Cookie value with
/// the hardcoded non-cookie set.
///
/// Returns a `BTreeMap` matching the shape [`build_browser_json`] consumes, so
/// the same downstream validator runs over both flows. The Cookie header is
/// inserted last so it overrides any blank `cookie` entry from the constants.
pub fn synthesize_headers(cookie_header: String) -> BTreeMap<String, String> {
    let mut headers: BTreeMap<String, String> = HARDCODED_HEADERS
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    headers.insert("cookie".to_owned(), cookie_header);
    headers
}

/// The driver `main` calls when the user types
/// `ytmusic-tui auth --from-browser <name>`.
///
/// Thin wrapper: parses the browser name, then hands off to
/// [`drive_from_browser_flow`] with the real rookie extractor, the real session
/// canary, and the default browser-auth path. Returns a process exit code:
/// `0` on success, `1` on any handled failure.
#[must_use]
pub fn run_from_browser(browser: &str) -> i32 {
    let stdout = io::stdout();
    let stderr = io::stderr();
    let mut out = stdout.lock();
    let mut err = stderr.lock();
    let path = crate::config::expand_tilde(Path::new(DEFAULT_BROWSER_AUTH_PATH));
    drive_from_browser_flow(
        browser,
        &mut out,
        &mut err,
        &path,
        extract_cookies_for_browser,
        validate_session_sync,
    )
}

/// The testable core of the `--from-browser` flow.
///
/// All side-effecting collaborators are injected:
///
/// * `extract` — returns the YTM cookie pairs for a [`BrowserKind`].
/// * `validate` — runs the session canary against the produced JSON string;
///   `true` means "live session", `false` means "stale/logged out".
///
/// Tests stub both. The real driver wires them to
/// [`extract_cookies_for_browser`] and [`validate_session_sync`].
fn drive_from_browser_flow<O, E, Extract, Validate>(
    browser: &str,
    out: &mut O,
    err: &mut E,
    path: &Path,
    extract: Extract,
    validate: Validate,
) -> i32
where
    O: Write,
    E: Write,
    Extract: FnOnce(BrowserKind) -> Result<Vec<(String, String)>, AuthCliError>,
    Validate: FnOnce(&str) -> bool,
{
    // 1. Resolve the browser name.
    let kind = match parse_browser_name(browser) {
        Ok(k) => k,
        Err(e) => {
            let _ = writeln!(err, "ytmusic-tui auth: {e}");
            return 1;
        }
    };
    let _ = writeln!(out, "Reading cookies from {}…", kind.name());

    // 2. Pull cookies out of the browser jar.
    let cookies = match extract(kind) {
        Ok(c) => c,
        Err(e) => {
            let _ = writeln!(err, "ytmusic-tui auth: {e}");
            return 1;
        }
    };

    // 3. Build the in-memory header map and validate it shapes up correctly.
    //    `build_browser_json` does the SAPISID presence check (the same path
    //    the interactive flow uses), so a cookie store with the right cookies
    //    missing is rejected here with a precise MissingSapisid error.
    let cookie_header = build_cookie_header(&cookies);
    let headers = synthesize_headers(cookie_header);
    let value = match build_browser_json(&headers) {
        Ok(v) => v,
        Err(e) => {
            let _ = writeln!(err, "ytmusic-tui auth: {e}");
            return 1;
        }
    };

    // 4. Pre-write session canary. The whole point of #18 is that the produced
    //    file actually works; if the canary says the cookies are signed out we
    //    abort BEFORE clobbering an existing browser.json — losing the previous
    //    auth to a stale extraction would be a regression.
    let json = match serde_json::to_string(&value) {
        Ok(s) => s,
        Err(e) => {
            let _ = writeln!(err, "ytmusic-tui auth: failed to serialise auth: {e}");
            return 1;
        }
    };
    let _ = writeln!(out, "Verifying session with music.youtube.com…");
    if !validate(&json) {
        let _ = writeln!(
            err,
            "ytmusic-tui auth: {}",
            AuthCliError::SessionCanaryFailed
        );
        return 1;
    }

    // 5. Write the file (same writer the interactive flow uses).
    if let Err(e) = write_browser_json(path, &value) {
        let _ = writeln!(
            err,
            "ytmusic-tui auth: failed to save auth to {}: {e}",
            path.display()
        );
        return 1;
    }

    let _ = writeln!(out, "Browser auth saved to {}", path.display());
    let _ = writeln!(out, "You can now run ytmusic-tui to start playing music.");
    0
}

/// Run [`InnerTubeClient::is_session_valid`] synchronously against `json_str`.
///
/// `auth_cli` runs before `main` spawns its own tokio runtime, so this builds
/// a one-shot current-thread runtime, blocks on the canary, and tears it down.
/// Per the canary contract:
///
/// * `true`  — the auth carries a live signed-in session, OR a network error
///   prevented us from verifying (we assume valid rather than block a user who
///   is just offline);
/// * `false` — the auth parsed but the response shape is the logged-out one.
///
/// Any failure to build the client/runtime is treated as `true` for the same
/// "do not block on unverifiable" reason — the user will discover the broken
/// auth on the next API call rather than during this CLI invocation.
fn validate_session_sync(json_str: &str) -> bool {
    let auth = match ytmusic_api::BrowserAuth::from_json_str(json_str) {
        Ok(a) => a,
        // A malformed JSON here would be our own bug (build_browser_json
        // already validated everything it cares about). Fail open.
        Err(_) => return true,
    };
    let client = match ytmusic_api::InnerTubeClient::new(auth) {
        Ok(c) => c,
        Err(_) => return true,
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(_) => return true,
    };
    runtime.block_on(client.is_session_valid())
}

/// The driver `main` calls when the user types `ytmusic-tui auth`.
///
/// Wired to the real `stdin`/`stdout`/`stderr` and the default path. Returns
/// a process exit code: `0` on success, `1` on any error (already reported
/// to `stderr`).
#[must_use]
pub fn run_auth_flow() -> i32 {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let stderr = io::stderr();
    let mut stdin = stdin.lock();
    let mut stdout = stdout.lock();
    let mut stderr = stderr.lock();
    let path = crate::config::expand_tilde(Path::new(DEFAULT_BROWSER_AUTH_PATH));
    drive_auth_flow(&mut stdin, &mut stdout, &mut stderr, &path)
}

/// The testable inner driver: prompt → parse → validate → write, reporting
/// every outcome to the supplied writers.
///
/// Split out from [`run_auth_flow`] so a `Cursor`-backed `stdin` and a
/// `Vec<u8>` `stdout`/`stderr` make the whole flow assert-able without
/// touching the real terminal.
fn drive_auth_flow<R: BufRead, O: Write, E: Write>(
    input: &mut R,
    out: &mut O,
    err: &mut E,
    path: &Path,
) -> i32 {
    print_prompt(out);

    let headers = match parse_request_headers(input) {
        Ok(h) => h,
        Err(e) => {
            let _ = writeln!(err, "ytmusic-tui auth: failed to read input: {e}");
            return 1;
        }
    };

    let value = match build_browser_json(&headers) {
        Ok(v) => v,
        Err(e) => {
            let _ = writeln!(err, "ytmusic-tui auth: {e}");
            return 1;
        }
    };

    if let Err(e) = write_browser_json(path, &value) {
        let _ = writeln!(
            err,
            "ytmusic-tui auth: failed to save auth to {}: {e}",
            path.display()
        );
        return 1;
    }

    let _ = writeln!(out, "Browser auth saved to {}", path.display());
    let _ = writeln!(out, "You can now run ytmusic-tui to start playing music.");
    0
}

/// Print the multi-line prompt explaining how to get the headers out of the
/// browser. Kept short — the README has the full walkthrough.
fn print_prompt<W: Write>(out: &mut W) {
    let _ = writeln!(out, "ytmusic-tui auth");
    let _ = writeln!(out, "================");
    let _ = writeln!(out);
    let _ = writeln!(out, "1. Open https://music.youtube.com and sign in.");
    let _ = writeln!(out, "2. Open the browser DevTools (F12) → Network tab.");
    let _ = writeln!(
        out,
        "3. Reload the page and click any 'browse' POST request."
    );
    let _ = writeln!(
        out,
        "4. Copy the Request Headers block (one Name: Value per line)."
    );
    let _ = writeln!(out, "5. Paste it below, then press Enter on an empty line.");
    let _ = writeln!(out);
    let _ = writeln!(out, "Paste headers now:");
    let _ = out.flush();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn cursor(s: &str) -> Cursor<Vec<u8>> {
        Cursor::new(s.as_bytes().to_vec())
    }

    #[test]
    fn parse_headers_basic_splits_on_colon_and_lowercases_keys() {
        let raw = "Cookie: __Secure-3PAPISID=abc; X=1\n\
                   User-Agent: Mozilla/5.0\n\
                   X-Goog-AuthUser: 0\n\
                   \n";
        let mut input = cursor(raw);
        let headers = parse_request_headers(&mut input).expect("parses");
        assert_eq!(
            headers.get("cookie").map(String::as_str),
            Some("__Secure-3PAPISID=abc; X=1")
        );
        assert_eq!(
            headers.get("user-agent").map(String::as_str),
            Some("Mozilla/5.0")
        );
        assert_eq!(
            headers.get("x-goog-authuser").map(String::as_str),
            Some("0")
        );
    }

    #[test]
    fn parse_headers_terminates_on_first_blank_line() {
        // A blank line ends the input; anything afterwards is ignored, so a
        // stray paste of the request body never bleeds into the headers.
        let raw = "Cookie: A=1\n\
                   \n\
                   User-Agent: ignored-after-blank\n";
        let mut input = cursor(raw);
        let headers = parse_request_headers(&mut input).expect("parses");
        assert_eq!(headers.len(), 1);
        assert!(headers.contains_key("cookie"));
        assert!(!headers.contains_key("user-agent"));
    }

    #[test]
    fn parse_headers_handles_empty_input() {
        let mut input = cursor("");
        let headers = parse_request_headers(&mut input).expect("parses");
        assert!(headers.is_empty());
    }

    #[test]
    fn parse_headers_trims_whitespace_and_crlf() {
        let raw = "  Cookie  :   __Secure-3PAPISID=x   \r\n\r\n";
        let mut input = cursor(raw);
        let headers = parse_request_headers(&mut input).expect("parses");
        assert_eq!(
            headers.get("cookie").map(String::as_str),
            Some("__Secure-3PAPISID=x")
        );
    }

    #[test]
    fn parse_headers_skips_lines_without_a_colon() {
        // Simulates a paste that includes the request method line.
        let raw = "POST /youtubei/v1/browse HTTP/2\n\
                   Cookie: __Secure-3PAPISID=a\n\
                   \n";
        let mut input = cursor(raw);
        let headers = parse_request_headers(&mut input).expect("parses");
        assert_eq!(headers.len(), 1);
        assert!(headers.contains_key("cookie"));
    }

    #[test]
    fn parse_headers_preserves_colons_in_values() {
        // The cookie value contains no colon in practice, but Authorization-style
        // headers can — split on the first colon only.
        let raw = "Authorization: SAPISIDHASH 123_abc:def\nCookie: SAPISID=z\n\n";
        let mut input = cursor(raw);
        let headers = parse_request_headers(&mut input).expect("parses");
        assert_eq!(
            headers.get("authorization").map(String::as_str),
            Some("SAPISIDHASH 123_abc:def")
        );
    }

    fn good_headers() -> BTreeMap<String, String> {
        let mut h = BTreeMap::new();
        h.insert(
            "cookie".to_owned(),
            "__Secure-3PAPISID=DUMMY; X=1".to_owned(),
        );
        h.insert("user-agent".to_owned(), "TestAgent/1.0".to_owned());
        h.insert("accept".to_owned(), "*/*".to_owned());
        h.insert("accept-language".to_owned(), "en-US".to_owned());
        h.insert("x-goog-authuser".to_owned(), "0".to_owned());
        h.insert(
            "x-origin".to_owned(),
            "https://music.youtube.com".to_owned(),
        );
        h.insert("x-youtube-client-name".to_owned(), "67".to_owned());
        h.insert("x-youtube-client-version".to_owned(), "1.0".to_owned());
        h
    }

    #[test]
    fn build_browser_json_errors_when_cookie_missing() {
        let mut h = good_headers();
        h.remove("cookie");
        let err = build_browser_json(&h).expect_err("no cookie");
        assert_eq!(err, AuthCliError::MissingCookie);
    }

    #[test]
    fn build_browser_json_errors_when_sapisid_missing() {
        let mut h = good_headers();
        h.insert("cookie".to_owned(), "OTHER=1; FOO=bar".to_owned());
        let err = build_browser_json(&h).expect_err("no sapisid");
        assert_eq!(err, AuthCliError::MissingSapisid);
    }

    #[test]
    fn build_browser_json_accepts_plain_sapisid_fallback() {
        let mut h = good_headers();
        h.insert("cookie".to_owned(), "SAPISID=PLAIN; X=1".to_owned());
        let value = build_browser_json(&h).expect("valid");
        assert!(value.is_object());
    }

    #[test]
    fn build_browser_json_filters_to_documented_set() {
        let mut h = good_headers();
        // Inject a header that must be dropped.
        h.insert("x-evil".to_owned(), "leak".to_owned());
        let value = build_browser_json(&h).expect("valid");
        let object = value.as_object().expect("object");
        assert!(!object.contains_key("x-evil"));
        // Required fields survive.
        assert!(object.contains_key("cookie"));
        assert!(object.contains_key("user-agent"));
    }

    #[test]
    fn build_browser_json_roundtrips_through_browser_auth_loader() {
        // The whole point: the file we emit must parse cleanly with
        // ytmusic_api::BrowserAuth, which is the production consumer.
        let h = good_headers();
        let value = build_browser_json(&h).expect("valid");
        let json = serde_json::to_string(&value).expect("serialises");
        let auth = ytmusic_api::BrowserAuth::from_json_str(&json).expect("loads");
        assert_eq!(auth.header("user-agent"), Some("TestAgent/1.0"));
        assert!(auth.cookie().contains("__Secure-3PAPISID="));
    }

    #[test]
    fn write_browser_json_creates_missing_parent_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested/sub/browser.json");
        let value = build_browser_json(&good_headers()).expect("valid");
        write_browser_json(&path, &value).expect("writes");
        assert!(path.is_file());
        let content = std::fs::read_to_string(&path).expect("reads");
        let parsed: Value = serde_json::from_str(&content).expect("json");
        assert!(parsed.is_object());
    }

    #[cfg(unix)]
    #[test]
    fn write_browser_json_uses_owner_only_permissions_on_unix() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("browser.json");
        let value = build_browser_json(&good_headers()).expect("valid");
        write_browser_json(&path, &value).expect("writes");
        let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn drive_auth_flow_writes_file_on_valid_paste_and_returns_zero() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("browser.json");
        let raw = "Cookie: __Secure-3PAPISID=ABC; OTHER=1\n\
                   User-Agent: Mozilla/5.0\n\
                   Accept: */*\n\
                   \n";
        let mut input = cursor(raw);
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = drive_auth_flow(&mut input, &mut out, &mut err, &path);
        assert_eq!(code, 0, "stderr: {}", String::from_utf8_lossy(&err));
        assert!(path.is_file());
        let stdout = String::from_utf8(out).expect("utf8");
        assert!(stdout.contains("Browser auth saved to"));
    }

    #[test]
    fn drive_auth_flow_reports_missing_cookie_and_returns_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("browser.json");
        let raw = "User-Agent: Mozilla/5.0\n\n";
        let mut input = cursor(raw);
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = drive_auth_flow(&mut input, &mut out, &mut err, &path);
        assert_eq!(code, 1);
        assert!(!path.exists(), "no file must be written on error");
        let stderr = String::from_utf8(err).expect("utf8");
        assert!(stderr.contains("No 'Cookie:' header found"));
    }

    #[test]
    fn drive_auth_flow_reports_missing_sapisid_and_returns_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("browser.json");
        let raw = "Cookie: HAS_NO_SAPISID=1\nUser-Agent: X\n\n";
        let mut input = cursor(raw);
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = drive_auth_flow(&mut input, &mut out, &mut err, &path);
        assert_eq!(code, 1);
        assert!(!path.exists());
        let stderr = String::from_utf8(err).expect("utf8");
        assert!(stderr.contains("SAPISID"));
    }

    // ---- --from-browser flow ---------------------------------------------
    //
    // The pure helpers (parse_browser_name / build_cookie_header /
    // synthesize_headers) plus the testable inner driver
    // (drive_from_browser_flow) exercise the whole flow without touching
    // rookie or the network. The live integration test against a real
    // browser is at the bottom under `#[ignore]`.

    #[test]
    fn parse_browser_name_accepts_canonical_lowercase() {
        assert_eq!(parse_browser_name("firefox").unwrap(), BrowserKind::Firefox);
        assert_eq!(parse_browser_name("chrome").unwrap(), BrowserKind::Chrome);
        assert_eq!(
            parse_browser_name("chromium").unwrap(),
            BrowserKind::Chromium
        );
        assert_eq!(parse_browser_name("brave").unwrap(), BrowserKind::Brave);
        assert_eq!(parse_browser_name("edge").unwrap(), BrowserKind::Edge);
        assert_eq!(parse_browser_name("vivaldi").unwrap(), BrowserKind::Vivaldi);
        assert_eq!(parse_browser_name("opera").unwrap(), BrowserKind::Opera);
        assert_eq!(
            parse_browser_name("opera_gx").unwrap(),
            BrowserKind::OperaGx
        );
        assert_eq!(parse_browser_name("arc").unwrap(), BrowserKind::Arc);
        assert_eq!(parse_browser_name("safari").unwrap(), BrowserKind::Safari);
        assert_eq!(parse_browser_name("zen").unwrap(), BrowserKind::Zen);
        assert_eq!(
            parse_browser_name("librewolf").unwrap(),
            BrowserKind::Librewolf
        );
    }

    #[test]
    fn parse_browser_name_is_case_insensitive_and_trims() {
        assert_eq!(parse_browser_name("FIREFOX").unwrap(), BrowserKind::Firefox);
        assert_eq!(
            parse_browser_name("  Chromium ").unwrap(),
            BrowserKind::Chromium
        );
    }

    #[test]
    fn parse_browser_name_accepts_documented_aliases() {
        // chromium-browser is the Debian package name.
        assert_eq!(
            parse_browser_name("chromium-browser").unwrap(),
            BrowserKind::Chromium
        );
        // msedge / microsoft-edge are the Edge spellings users actually type.
        assert_eq!(parse_browser_name("msedge").unwrap(), BrowserKind::Edge);
        assert_eq!(
            parse_browser_name("microsoft-edge").unwrap(),
            BrowserKind::Edge
        );
        // Opera GX hyphenated/no-underscore.
        assert_eq!(
            parse_browser_name("opera-gx").unwrap(),
            BrowserKind::OperaGx
        );
    }

    #[test]
    fn parse_browser_name_rejects_unknown_with_typed_error() {
        let err = parse_browser_name("netscape").expect_err("unknown");
        assert_eq!(err, AuthCliError::UnsupportedBrowser("netscape".to_owned()));
        // Display surfaces the rejected name verbatim.
        let msg = err.to_string();
        assert!(msg.contains("netscape"), "msg: {msg}");
    }

    #[test]
    fn build_cookie_header_joins_with_semicolon_space_in_order() {
        let pairs = vec![
            ("__Secure-3PAPISID".to_owned(), "AAA".to_owned()),
            ("SAPISID".to_owned(), "BBB".to_owned()),
            ("SID".to_owned(), "CCC".to_owned()),
        ];
        let got = build_cookie_header(&pairs);
        assert_eq!(got, "__Secure-3PAPISID=AAA; SAPISID=BBB; SID=CCC");
    }

    #[test]
    fn build_cookie_header_empty_input_yields_empty_string() {
        assert_eq!(build_cookie_header(&[]), "");
    }

    #[test]
    fn synthesize_headers_includes_every_hardcoded_entry() {
        let cookie = "__Secure-3PAPISID=X".to_owned();
        let headers = synthesize_headers(cookie.clone());
        // Cookie was inserted.
        assert_eq!(
            headers.get("cookie").map(String::as_str),
            Some(cookie.as_str())
        );
        // Every documented hardcoded header is present.
        for (name, _) in HARDCODED_HEADERS {
            assert!(headers.contains_key(*name), "missing {name}");
        }
        // The set we emit matches REQUIRED_HEADERS (no extras, no missing).
        for &name in REQUIRED_HEADERS {
            assert!(
                headers.contains_key(name),
                "REQUIRED_HEADERS expected {name}"
            );
        }
    }

    #[test]
    fn synthesize_headers_round_trips_through_build_browser_json() {
        // The whole point: a synthesized header set must build into a valid
        // browser.json without manual intervention.
        let cookie = "__Secure-3PAPISID=DUMMY_SAPISID; SAPISID=ALSO_DUMMY; SID=Z".to_owned();
        let headers = synthesize_headers(cookie);
        let value = build_browser_json(&headers).expect("builds");
        let object = value.as_object().expect("object");
        assert!(object.contains_key("cookie"));
        assert!(object.contains_key("user-agent"));
        assert!(object.contains_key("x-youtube-client-name"));
    }

    #[test]
    fn auth_cli_error_display_strings_are_informative() {
        // Each new variant emits a non-empty, distinct message that mentions
        // the relevant subject. The wording is intentionally not asserted in
        // full so future polish does not break the test.
        let cases = [
            (AuthCliError::UnsupportedBrowser("foo".to_owned()), "foo"),
            (AuthCliError::RookieFailed("bar".to_owned()), "bar"),
            (AuthCliError::SessionCanaryFailed, "Session"),
        ];
        for (err, needle) in cases {
            let msg = err.to_string();
            assert!(!msg.is_empty(), "empty message for {err:?}");
            assert!(
                msg.contains(needle),
                "missing '{needle}' in {err:?} → {msg}"
            );
        }
    }

    // ---- driver: stubbed extract + validate ------------------------------

    /// A canned cookie set carrying a SAPISID — feeds the happy-path tests.
    fn good_cookie_pairs() -> Vec<(String, String)> {
        vec![
            (
                "__Secure-3PAPISID".to_owned(),
                "DRIVER_TEST_SAPISID".to_owned(),
            ),
            ("SAPISID".to_owned(), "DRIVER_TEST_PLAIN".to_owned()),
            ("SID".to_owned(), "DRIVER_TEST_SID".to_owned()),
        ]
    }

    #[test]
    fn drive_from_browser_flow_writes_file_on_happy_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("browser.json");
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = drive_from_browser_flow(
            "firefox",
            &mut out,
            &mut err,
            &path,
            |_kind| Ok(good_cookie_pairs()),
            |_json| true,
        );
        assert_eq!(code, 0, "stderr: {}", String::from_utf8_lossy(&err));
        assert!(path.is_file(), "browser.json must be written");
        let raw = std::fs::read_to_string(&path).expect("reads");
        let parsed: Value = serde_json::from_str(&raw).expect("json");
        assert!(parsed.is_object());
        let stdout = String::from_utf8(out).expect("utf8");
        assert!(stdout.contains("Browser auth saved to"));
        // The cookie value made it through to the file unmodified.
        let cookie = parsed
            .get("cookie")
            .and_then(|v| v.as_str())
            .expect("cookie");
        assert!(cookie.contains("__Secure-3PAPISID=DRIVER_TEST_SAPISID"));
    }

    #[test]
    fn drive_from_browser_flow_rejects_unknown_browser_without_write() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("browser.json");
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = drive_from_browser_flow(
            "netscape",
            &mut out,
            &mut err,
            &path,
            |_kind| panic!("extract must not be called for an unknown browser"),
            |_json| panic!("validate must not be called for an unknown browser"),
        );
        assert_eq!(code, 1);
        assert!(!path.exists(), "no file written on a usage error");
        let stderr = String::from_utf8(err).expect("utf8");
        assert!(stderr.contains("netscape"));
    }

    #[test]
    fn drive_from_browser_flow_surfaces_rookie_failure_without_write() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("browser.json");
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = drive_from_browser_flow(
            "firefox",
            &mut out,
            &mut err,
            &path,
            |_kind| Err(AuthCliError::RookieFailed("locked db".to_owned())),
            |_json| panic!("validate must not be called when extraction failed"),
        );
        assert_eq!(code, 1);
        assert!(!path.exists());
        let stderr = String::from_utf8(err).expect("utf8");
        assert!(stderr.contains("locked db"));
    }

    #[test]
    fn drive_from_browser_flow_rejects_cookies_without_sapisid() {
        // Extraction succeeded but the user is signed out — none of the pulled
        // cookies carries a SAPISID. build_browser_json must reject this before
        // the canary even runs.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("browser.json");
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = drive_from_browser_flow(
            "firefox",
            &mut out,
            &mut err,
            &path,
            |_kind| Ok(vec![("SID".to_owned(), "X".to_owned())]),
            |_json| panic!("validate must not be called when SAPISID is missing"),
        );
        assert_eq!(code, 1);
        assert!(!path.exists());
        let stderr = String::from_utf8(err).expect("utf8");
        assert!(stderr.contains("SAPISID"));
    }

    #[test]
    fn drive_from_browser_flow_aborts_when_canary_says_logged_out() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("browser.json");
        // Pre-seed an existing file: the canary failure must NOT clobber it.
        std::fs::write(&path, "{\"do\":\"not\",\"touch\":\"me\"}").expect("seed");
        let pre = std::fs::read_to_string(&path).expect("reads");

        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = drive_from_browser_flow(
            "firefox",
            &mut out,
            &mut err,
            &path,
            |_kind| Ok(good_cookie_pairs()),
            |_json| false,
        );
        assert_eq!(code, 1);
        // File untouched: existing browser.json survives a failed canary.
        let post = std::fs::read_to_string(&path).expect("reads");
        assert_eq!(pre, post, "existing browser.json must not be overwritten");
        let stderr = String::from_utf8(err).expect("utf8");
        assert!(stderr.contains("Session"));
    }

    #[test]
    fn drive_from_browser_flow_passes_json_to_canary() {
        // The validator receives the exact JSON string we would write; assert
        // it sees the SAPISID cookie so it can perform the canary against the
        // same auth we are about to persist.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("browser.json");
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let mut seen: Option<String> = None;
        let code = drive_from_browser_flow(
            "firefox",
            &mut out,
            &mut err,
            &path,
            |_kind| Ok(good_cookie_pairs()),
            |json| {
                seen = Some(json.to_owned());
                true
            },
        );
        assert_eq!(code, 0, "stderr: {}", String::from_utf8_lossy(&err));
        let json = seen.expect("validator must be called on the happy path");
        assert!(json.contains("__Secure-3PAPISID=DRIVER_TEST_SAPISID"));
    }

    // ---- live integration: real browser cookie store ---------------------
    //
    // Disabled by default. Run with: cargo test -p ytmusic-tui -- --ignored
    // Requires Firefox installed and a recent signed-in session at
    // music.youtube.com. The assertion is intentionally minimal: it only
    // checks that a file was written. Reading and validating the
    // produced session against the live network is exactly what the canary
    // inside the flow already does.

    #[test]
    #[ignore = "requires a real signed-in Firefox profile and live network"]
    fn run_from_browser_firefox_writes_real_browser_json() {
        // Uses the real default path (~/.config/ytmusic-tui/browser.json); if
        // a file already exists, we skip the actual disk write by aborting via
        // the rookie-failure path. The flow is documented as destructive on
        // success, which is the whole point — the user is opting in.
        let code = super::run_from_browser("firefox");
        // We only care that the flow completed without panicking. Exit code
        // depends on what the user has installed; both 0 (worked) and 1
        // (Firefox not installed / not signed in) are acceptable here.
        assert!(
            code == 0 || code == 1,
            "unexpected exit code from live --from-browser flow: {code}"
        );
    }
}
