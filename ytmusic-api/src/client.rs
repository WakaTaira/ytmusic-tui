//! The InnerTube HTTP client: builds and signs `youtubei/v1` requests, plus the
//! session canary.
//!
//! Request shape mirrors `ytmusicapi.ytmusic.YTMusicBase._send_request`:
//! `POST https://music.youtube.com/youtubei/v1/{endpoint}?alt=json&key=...`
//! with a JSON body of `{...extras, context: {...}}` and the browser-auth
//! headers (authorization recomputed per call).

use std::time::Duration;

use reqwest::header::{
    AUTHORIZATION, CONTENT_TYPE, COOKIE, HeaderMap, HeaderName, HeaderValue, ORIGIN, USER_AGENT,
};
use serde_json::{Map, Value};

use crate::auth::BrowserAuth;
use crate::context::build_context;
use crate::error::ApiError;

/// Base URL for the InnerTube API. Matches `YTM_BASE_API` in ytmusicapi.
const YTM_BASE_API: &str = "https://music.youtube.com/youtubei/v1/";

/// Query string appended to browser-auth requests.
///
/// `?alt=json` is `YTM_PARAMS`; the `&key=...` suffix is `YTM_PARAMS_KEY`, which
/// ytmusicapi only appends for `AuthType.BROWSER`. The key is the public web
/// client's InnerTube API key, identical for every user (not a secret).
const YTM_PARAMS: &str = "?alt=json&key=AIzaSyC9XL3ZjWddXya6X74dJoCTL-WEYFDNX30";

/// The `SOCS` cookie ytmusicapi sends alongside the auth Cookie header (Google
/// cookie-consent value; see the note in `YTMusicBase.__init__`).
const SOCS_COOKIE: &str = "SOCS=CAI";

/// Default `User-Agent` used when the auth file omits one. Matches `USER_AGENT`
/// in `ytmusicapi/constants.py`.
const DEFAULT_USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:88.0) Gecko/20100101 Firefox/88.0";

/// Account information returned by the [`InnerTubeClient::get_account_info`]
/// canary. A successfully parsed value means a signed-in session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountInfo {
    /// The signed-in account's display name.
    pub account_name: String,
    /// The account's channel handle (e.g. `@SampleUser`), when present.
    pub channel_handle: Option<String>,
    /// URL of the account photo, when present.
    pub account_photo_url: Option<String>,
}

/// An authenticated InnerTube client.
///
/// Holds a pooled [`reqwest::Client`], the parsed [`BrowserAuth`], and the
/// pre-built request context. Construct once and reuse.
#[derive(Clone)]
pub struct InnerTubeClient {
    http: reqwest::Client,
    auth: BrowserAuth,
    context: Value,
}

impl std::fmt::Debug for InnerTubeClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InnerTubeClient")
            .field("auth", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl InnerTubeClient {
    /// Build a client from already-loaded browser auth.
    pub fn new(auth: BrowserAuth) -> Result<Self, ApiError> {
        // Interactive TUI client — a hung request must not stall a worker
        // forever.  (The Python implementation used requests with no timeout;
        // that was a liability, not a contract.)
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .connect_timeout(Duration::from_secs(5))
            .build()?;
        Ok(Self {
            http,
            auth,
            context: build_context(),
        })
    }

    /// Load browser auth from `path` and build a client.
    pub fn from_auth_path(path: impl AsRef<std::path::Path>) -> Result<Self, ApiError> {
        let auth = BrowserAuth::load(path)?;
        Self::new(auth)
    }

    /// POST to an InnerTube endpoint, returning the parsed JSON response.
    ///
    /// `body_extras` is merged into `{context: {...}}` to form the request body
    /// (extras first, then context — matching ytmusicapi's `body.update(context)`,
    /// where the context always wins on key collisions). A non-2xx status maps to
    /// [`ApiError::Http`] with the message lifted from the response's
    /// `error.message` field when available.
    pub async fn post(&self, endpoint: &str, body_extras: Value) -> Result<Value, ApiError> {
        let url = format!("{YTM_BASE_API}{endpoint}{YTM_PARAMS}");
        let body = self.build_body(body_extras);

        let response = self
            .http
            .post(&url)
            .headers(self.request_headers()?)
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        let text = response.text().await?;
        let parsed: Value = serde_json::from_str(&text)
            .map_err(|e| ApiError::Parse(format!("response was not JSON: {e}")))?;

        if !status.is_success() {
            let message = parsed
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            return Err(ApiError::Http {
                status: status.as_u16(),
                message,
            });
        }

        Ok(parsed)
    }

    /// Merge `body_extras` with the request context into a single object body.
    fn build_body(&self, body_extras: Value) -> Value {
        let mut map: Map<String, Value> = match body_extras {
            Value::Object(m) => m,
            // A non-object extras payload is treated as "no extras"; the context
            // alone is a valid InnerTube body (the canary sends exactly this).
            _ => Map::new(),
        };
        if let Some(context) = self.context.as_object() {
            for (key, value) in context {
                map.insert(key.clone(), value.clone());
            }
        }
        Value::Object(map)
    }

    /// Build the header map for a request: the auth file's headers as the base,
    /// with the signing-critical headers set/overridden explicitly.
    ///
    /// `authorization` is recomputed every call (it embeds a timestamp). The
    /// `Cookie` header carries the auth file's cookie plus `SOCS=CAI`.
    fn request_headers(&self) -> Result<HeaderMap, ApiError> {
        let mut headers = HeaderMap::new();

        // Replay the auth file's headers as the base. Skip ones we set
        // explicitly below (cookie/authorization/origin) and any that are not
        // valid HTTP header names/values.
        for &name in self.auth_header_names() {
            if matches!(name, "cookie" | "authorization" | "origin") {
                continue;
            }
            if let Some(value) = self.auth.header(name)
                && let (Ok(header_name), Ok(header_value)) = (
                    HeaderName::from_bytes(name.as_bytes()),
                    HeaderValue::from_str(value),
                )
            {
                headers.insert(header_name, header_value);
            }
        }

        // User-Agent: keep the auth file's if present, else the ytmusicapi default.
        if !headers.contains_key(USER_AGENT) {
            headers.insert(USER_AGENT, HeaderValue::from_static(DEFAULT_USER_AGENT));
        }
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        // Origin (signing + CORS): the value the SAPISIDHASH was computed over.
        headers.insert(
            ORIGIN,
            HeaderValue::from_str(self.auth.origin())
                .map_err(|e| ApiError::Parse(format!("invalid origin header: {e}")))?,
        );

        // Authorization: SAPISIDHASH for the current instant.
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&self.auth.authorization())
                .map_err(|e| ApiError::Parse(format!("invalid authorization header: {e}")))?,
        );

        // Cookie: the auth file's cookie, with SOCS appended.
        let cookie = format!("{}; {SOCS_COOKIE}", self.auth.cookie());
        headers.insert(
            COOKIE,
            HeaderValue::from_str(&cookie)
                .map_err(|e| ApiError::Parse(format!("invalid cookie header: {e}")))?,
        );

        Ok(headers)
    }

    /// The set of header names from a browser.json file that are forwarded on
    /// every InnerTube request.
    ///
    /// Philosophy: forward only the InnerTube-relevant subset of the headers
    /// captured from DevTools. Leftover DevTools noise (`host`, `content-length`,
    /// `accept-encoding`, `sec-fetch-*`, ...) is deliberately dropped — it
    /// either conflicts with reqwest's own transport layer or is irrelevant to
    /// the API. This matches the header pruning ytmusicapi performs in
    /// `YTMusicBase.setup()`. `cookie`, `authorization`, and `origin` are
    /// excluded here because they are set/recomputed explicitly in
    /// [`request_headers`](Self::request_headers).
    fn auth_header_names(&self) -> &'static [&'static str] {
        &[
            "user-agent",
            "accept",
            "accept-language",
            "x-goog-authuser",
            "x-goog-visitor-id",
            "x-goog-pageid",
            "x-origin",
            "x-youtube-client-name",
            "x-youtube-client-version",
        ]
    }

    /// The session canary: fetch the signed-in account's info.
    ///
    /// Hits `account/account_menu` (the same endpoint ytmusicapi's
    /// `get_account_info` uses) and walks the same path to the account name. A
    /// logged-out response (served with HTTP 200 when cookies are stale) lacks
    /// the `activeAccountHeaderRenderer` structure, so the required-path lookup
    /// fails and this returns [`ApiError::Parse`].
    pub async fn get_account_info(&self) -> Result<AccountInfo, ApiError> {
        let response = self
            .post("account/account_menu", Value::Object(Map::new()))
            .await?;
        parse_account_info(&response)
    }

    /// Best-effort check that the auth carries a signed-in session.
    ///
    /// Mirrors the Python contract in `src/ytmusic_tui/api.py::is_session_valid`
    /// exactly:
    ///
    /// * the account header parses → `true` (signed in);
    /// * HTTP 200 but the logged-out shape ([`ApiError::Parse`]) → `false` —
    ///   the "valid-looking but logged-out" auth-rot signal;
    /// * network or transient errors → `true` ("cannot verify, assume valid"):
    ///   the consumer is a startup warning toast, and a false "session
    ///   expired" warning for a merely-offline user is worse than staying
    ///   quiet. Callers that need to tell "offline" apart from "logged out"
    ///   should call [`get_account_info`](Self::get_account_info) and match
    ///   on [`ApiError`].
    pub async fn is_session_valid(&self) -> bool {
        match self.get_account_info().await {
            Ok(_) => true,
            Err(ApiError::Parse(_)) => false,
            // Network or transient errors: cannot verify, assume valid.
            Err(_) => true,
        }
    }
}

/// Walk an `account/account_menu` response into [`AccountInfo`].
///
/// Path mirrors `get_account_info` in `ytmusicapi/mixins/library.py`:
/// `actions[0].openPopupAction.popup.multiPageMenuRenderer.header
///  .activeAccountHeaderRenderer`, then `accountName.runs[0].text` (required),
/// `channelHandle.runs[0].text` and `accountPhoto.thumbnails[0].url` (optional).
/// Returns [`ApiError::Parse`] when the required account-name path is absent —
/// the "valid-looking but logged-out" signal.
fn parse_account_info(response: &Value) -> Result<AccountInfo, ApiError> {
    let header = response
        .get("actions")
        .and_then(|a| a.get(0))
        .and_then(|a| a.get("openPopupAction"))
        .and_then(|a| a.get("popup"))
        .and_then(|a| a.get("multiPageMenuRenderer"))
        .and_then(|a| a.get("header"))
        .and_then(|a| a.get("activeAccountHeaderRenderer"))
        .ok_or_else(|| {
            ApiError::Parse(
                "account_menu response lacks activeAccountHeaderRenderer (logged out?)".to_owned(),
            )
        })?;

    let account_name = runs_text(header.get("accountName")).ok_or_else(|| {
        ApiError::Parse("account_menu response lacks accountName (logged out?)".to_owned())
    })?;

    let channel_handle = runs_text(header.get("channelHandle"));
    let account_photo_url = header
        .get("accountPhoto")
        .and_then(|p| p.get("thumbnails"))
        .and_then(|t| t.get(0))
        .and_then(|t| t.get("url"))
        .and_then(Value::as_str)
        .map(str::to_owned);

    Ok(AccountInfo {
        account_name,
        channel_handle,
        account_photo_url,
    })
}

/// Extract `runs[0].text` from a renderer text node, if present.
fn runs_text(node: Option<&Value>) -> Option<String> {
    node.and_then(|n| n.get("runs"))
        .and_then(|r| r.get(0))
        .and_then(|r| r.get("text"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A logged-in `account/account_menu` response (trimmed to the path
    /// `get_account_info` walks).
    fn logged_in_response() -> Value {
        json!({
            "actions": [{
                "openPopupAction": {
                    "popup": {
                        "multiPageMenuRenderer": {
                            "header": {
                                "activeAccountHeaderRenderer": {
                                    "accountName": { "runs": [{ "text": "Sample User" }] },
                                    "channelHandle": { "runs": [{ "text": "@SampleUser" }] },
                                    "accountPhoto": {
                                        "thumbnails": [{ "url": "https://yt3.ggpht.com/sample" }]
                                    }
                                }
                            }
                        }
                    }
                }
            }]
        })
    }

    #[test]
    fn parses_logged_in_account() {
        let info = parse_account_info(&logged_in_response()).expect("logged-in parses");
        assert_eq!(info.account_name, "Sample User");
        assert_eq!(info.channel_handle.as_deref(), Some("@SampleUser"));
        assert_eq!(
            info.account_photo_url.as_deref(),
            Some("https://yt3.ggpht.com/sample")
        );
    }

    #[test]
    fn logged_in_without_optional_fields_still_parses() {
        // Required accountName present, optionals absent.
        let response = json!({
            "actions": [{
                "openPopupAction": { "popup": { "multiPageMenuRenderer": { "header": {
                    "activeAccountHeaderRenderer": {
                        "accountName": { "runs": [{ "text": "Only Name" }] }
                    }
                }}}}
            }]
        });
        let info = parse_account_info(&response).expect("parses");
        assert_eq!(info.account_name, "Only Name");
        assert_eq!(info.channel_handle, None);
        assert_eq!(info.account_photo_url, None);
    }

    #[test]
    fn logged_out_response_fails_to_parse() {
        // A signed-out account_menu reply: HTTP 200, but the menu has no
        // activeAccountHeaderRenderer (e.g. just a sign-in prompt). This is the
        // "valid-looking but logged-out" shape the canary must reject.
        let logged_out = json!({
            "actions": [{
                "openPopupAction": { "popup": { "multiPageMenuRenderer": {
                    "header": {},
                    "sections": []
                }}}
            }]
        });
        let err = parse_account_info(&logged_out).expect_err("logged-out rejected");
        assert!(matches!(err, ApiError::Parse(_)), "got: {err:?}");
    }

    #[test]
    fn empty_response_fails_to_parse() {
        let err = parse_account_info(&json!({})).expect_err("empty rejected");
        assert!(matches!(err, ApiError::Parse(_)), "got: {err:?}");
    }

    /// A dummy client over fabricated auth (no real credentials). The cookie
    /// carries a placeholder SAPISID so signing succeeds offline.
    fn dummy_client() -> InnerTubeClient {
        let auth = BrowserAuth::from_json_str(
            r#"{
                "Cookie": "__Secure-3PAPISID=DUMMY; X=1",
                "Authorization": "SAPISIDHASH 1_2",
                "Origin": "https://music.youtube.com",
                "User-Agent": "test-agent",
                "X-Goog-AuthUser": "0",
                "X-Goog-Visitor-Id": "VISITOR123"
            }"#,
        )
        .expect("dummy auth parses");
        InnerTubeClient::new(auth).expect("client builds")
    }

    #[test]
    fn build_body_merges_extras_with_context() {
        let client = dummy_client();
        let body = client.build_body(json!({ "browseId": "FEmusic_home" }));
        // Extra preserved.
        assert_eq!(body["browseId"], "FEmusic_home");
        // Context injected with the WEB_REMIX client.
        assert_eq!(body["context"]["client"]["clientName"], "WEB_REMIX");
        assert_eq!(body["context"]["client"]["hl"], "en");
    }

    #[test]
    fn build_body_from_non_object_yields_context_only() {
        let client = dummy_client();
        let body = client.build_body(Value::Null);
        assert!(body["context"]["client"]["clientName"] == "WEB_REMIX");
        // Only the context key is present.
        assert_eq!(body.as_object().unwrap().len(), 1);
    }

    #[test]
    fn request_headers_set_signing_and_replay_headers() {
        let client = dummy_client();
        let headers = client.request_headers().expect("headers build");

        // Authorization is a freshly computed SAPISIDHASH, not the file's stub.
        let auth = headers.get(AUTHORIZATION).unwrap().to_str().unwrap();
        assert!(auth.starts_with("SAPISIDHASH "), "auth: {auth}");
        assert_ne!(auth, "SAPISIDHASH 1_2");

        // Origin matches what was signed.
        assert_eq!(
            headers.get(ORIGIN).unwrap().to_str().unwrap(),
            "https://music.youtube.com"
        );

        // Cookie replays the file value AND appends SOCS=CAI.
        let cookie = headers.get(COOKIE).unwrap().to_str().unwrap();
        assert!(
            cookie.contains("__Secure-3PAPISID=DUMMY"),
            "cookie: {cookie}"
        );
        assert!(cookie.contains("SOCS=CAI"), "cookie: {cookie}");

        // Content-Type is JSON.
        assert_eq!(
            headers.get(CONTENT_TYPE).unwrap().to_str().unwrap(),
            "application/json"
        );

        // Replayed InnerTube headers carried through.
        assert_eq!(
            headers.get("x-goog-visitor-id").unwrap().to_str().unwrap(),
            "VISITOR123"
        );
        assert_eq!(
            headers.get(USER_AGENT).unwrap().to_str().unwrap(),
            "test-agent"
        );
    }

    #[test]
    fn header_present_but_account_name_missing_fails() {
        // The renderer exists but accountName is gone — also logged-out-ish.
        let response = json!({
            "actions": [{
                "openPopupAction": { "popup": { "multiPageMenuRenderer": { "header": {
                    "activeAccountHeaderRenderer": {
                        "channelHandle": { "runs": [{ "text": "@x" }] }
                    }
                }}}}
            }]
        });
        let err = parse_account_info(&response).expect_err("rejected");
        assert!(matches!(err, ApiError::Parse(_)), "got: {err:?}");
    }
}
