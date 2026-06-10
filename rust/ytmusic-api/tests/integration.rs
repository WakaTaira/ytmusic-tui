//! Integration tests against the live YouTube Music API.
//!
//! These are `#[ignore]` by default — they require a valid
//! `~/.config/ytmusic-tui/browser.json` and network access. Run them with:
//!
//! ```sh
//! cargo test -p ytmusic-api -- --ignored
//! ```
//!
//! They never print cookie material.

use std::path::PathBuf;

use ytmusic_api::InnerTubeClient;

/// Resolve the default browser-auth path (`~/.config/ytmusic-tui/browser.json`).
fn default_auth_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config/ytmusic-tui/browser.json"))
}

#[tokio::test]
#[ignore = "hits the live YouTube Music API; requires real browser.json"]
async fn is_session_valid_against_real_api() {
    let path = default_auth_path().expect("HOME is set");
    assert!(
        path.is_file(),
        "expected browser.json at {} — run `ytmusic-tui auth`",
        path.display()
    );

    let client = InnerTubeClient::from_auth_path(&path).expect("client builds from auth file");

    // The canary contract: returns a bool, never panics for auth rot.
    let valid = client.is_session_valid().await;
    // Print only the boolean — never any header/cookie material.
    eprintln!("is_session_valid -> {valid}");
    assert!(
        valid,
        "session reported invalid — cookies may be stale; re-run `ytmusic-tui auth`"
    );
}

#[tokio::test]
#[ignore = "hits the live YouTube Music API; requires real browser.json"]
async fn get_account_info_against_real_api() {
    let path = default_auth_path().expect("HOME is set");
    assert!(
        path.is_file(),
        "expected browser.json at {}",
        path.display()
    );

    let client = InnerTubeClient::from_auth_path(&path).expect("client builds");
    match client.get_account_info().await {
        Ok(info) => {
            // The account name confirms a signed-in session. We deliberately do
            // not assert its value (it is the user's real name); just that the
            // canary parsed a non-empty signed-in structure.
            assert!(!info.account_name.is_empty(), "account name was empty");
            eprintln!("account_info parsed: signed in = true");
        }
        Err(e) => panic!("get_account_info failed: {e}"),
    }
}
