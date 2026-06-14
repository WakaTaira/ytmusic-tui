//! OSC52 clipboard helpers for "Copy link to selected item" (issue #14).
//!
//! YouTube Music share URLs for a track, album, playlist, or artist are
//! constructed from id fields the app already holds (no network call). The
//! payload is then written to the terminal as an OSC52 sequence — the standard
//! "set selection clipboard" escape understood by tmux, wezterm, kitty, alacritty,
//! foot, and most modern terminals over ssh. The sequence format is:
//!
//! ```text
//! ESC ] 52 ; c ; <base64-encoded-text> ESC \
//! ```
//!
//! `c` selects the system clipboard. The terminator is the canonical ST
//! (String Terminator) form `ESC \` rather than BEL, because tmux and screen
//! pass through ST reliably while some configurations strip BEL.
//!
//! # Failure mode
//!
//! The terminal silently consumes the sequence (no ack); we cannot verify the
//! clipboard was actually updated. The function reports an `Err` only when the
//! tty write itself fails (e.g. stdout closed). Both branches end with a toast
//! at the call site so the user always sees feedback.

use std::io::Write;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ytmusic_api::{AlbumInfo, PlaylistInfo, Track};

/// YouTube Music base URL (all share links are anchored here).
const YTM_BASE: &str = "https://music.youtube.com";

/// Build the share URL for a track (`watch?v=<video_id>`).
#[must_use]
pub fn track_url(track: &Track) -> String {
    format!("{YTM_BASE}/watch?v={}", track.video_id)
}

/// Build the share URL for an album (`browse/<browse_id>`).
#[must_use]
pub fn album_url(album: &AlbumInfo) -> String {
    format!("{YTM_BASE}/browse/{}", album.browse_id)
}

/// Build the share URL for a playlist (`playlist?list=<playlist_id>`).
#[must_use]
pub fn playlist_url(info: &PlaylistInfo) -> String {
    format!("{YTM_BASE}/playlist?list={}", info.playlist_id)
}

/// Build the share URL for an artist channel (`channel/<channel_id>`).
///
/// Kept for completeness even though the action popup does not currently surface
/// an Artist variant (see issue #14 follow-up). Exposed so a future
/// `PopupItem::Artist` wiring drops in cleanly.
#[must_use]
pub fn artist_url(channel_id: &str) -> String {
    format!("{YTM_BASE}/channel/{channel_id}")
}

/// Encode `text` as the body of an OSC52 sequence (`ESC]52;c;<base64>ESC\`).
///
/// Pure string transform — split out so the encoding contract is unit-testable
/// without touching stdout.
#[must_use]
pub fn osc52_sequence(text: &str) -> String {
    let payload = STANDARD.encode(text.as_bytes());
    format!("\x1b]52;c;{payload}\x1b\\")
}

/// Copy `text` to the system clipboard by emitting an OSC52 escape to stdout.
///
/// The UI loop is single-threaded — between two ratatui frames there are no
/// other writes, so the escape is delivered atomically. A locked handle is
/// taken to make that ordering explicit. Returns `Err` only when the write or
/// flush fails (e.g. the tty was closed); the caller surfaces both branches as
/// toasts.
pub fn copy_to_clipboard(text: &str) -> Result<(), String> {
    let sequence = osc52_sequence(text);
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    handle
        .write_all(sequence.as_bytes())
        .map_err(|err| err.to_string())?;
    handle.flush().map_err(|err| err.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_track() -> Track {
        Track::new("dQw4w9WgXcQ", "Song", "Artist", "Album", 213.0, "")
    }

    fn sample_album() -> AlbumInfo {
        AlbumInfo::new_without_tracks("MPREb_xyz123", "LP", "Band", "2020", "")
    }

    fn sample_playlist() -> PlaylistInfo {
        PlaylistInfo::new("PLabc123", "Mix", "", 10, "")
    }

    #[test]
    fn track_url_uses_watch_with_video_id() {
        assert_eq!(
            track_url(&sample_track()),
            "https://music.youtube.com/watch?v=dQw4w9WgXcQ"
        );
    }

    #[test]
    fn album_url_uses_browse_with_browse_id() {
        assert_eq!(
            album_url(&sample_album()),
            "https://music.youtube.com/browse/MPREb_xyz123"
        );
    }

    #[test]
    fn playlist_url_uses_playlist_query() {
        assert_eq!(
            playlist_url(&sample_playlist()),
            "https://music.youtube.com/playlist?list=PLabc123"
        );
    }

    #[test]
    fn artist_url_uses_channel_path() {
        assert_eq!(
            artist_url("UCsuchannelid"),
            "https://music.youtube.com/channel/UCsuchannelid"
        );
    }

    #[test]
    fn osc52_sequence_wraps_payload_in_esc_terminators() {
        // Empty payload still produces a well-formed sequence.
        assert_eq!(osc52_sequence(""), "\x1b]52;c;\x1b\\");
    }

    #[test]
    fn osc52_sequence_base64_encodes_ascii_payload() {
        // "hello" → base64 "aGVsbG8=" (RFC 4648 with padding).
        let seq = osc52_sequence("hello");
        assert!(seq.starts_with("\x1b]52;c;"), "missing OSC52 introducer");
        assert!(seq.ends_with("\x1b\\"), "missing ST terminator");
        assert!(
            seq.contains("aGVsbG8="),
            "payload not base64-encoded: {seq}"
        );
    }

    #[test]
    fn osc52_sequence_handles_unicode_payload() {
        // CJK and emoji pass through the base64 encoder as their UTF-8 bytes —
        // verify the encode is byte-exact by round-tripping through decode.
        let url = "https://music.youtube.com/watch?v=テスト🎵";
        let seq = osc52_sequence(url);
        let body = seq
            .strip_prefix("\x1b]52;c;")
            .and_then(|s| s.strip_suffix("\x1b\\"))
            .expect("sequence framing");
        let decoded = STANDARD.decode(body).expect("base64 decodes");
        assert_eq!(std::str::from_utf8(&decoded).unwrap(), url);
    }

    #[test]
    fn osc52_sequence_for_real_track_url_decodes_to_input() {
        // End-to-end: build a real share URL and prove the OSC52 body decodes
        // back to it byte-for-byte.
        let url = track_url(&sample_track());
        let seq = osc52_sequence(&url);
        let body = seq
            .strip_prefix("\x1b]52;c;")
            .and_then(|s| s.strip_suffix("\x1b\\"))
            .expect("sequence framing");
        let decoded = STANDARD.decode(body).expect("base64 decodes");
        assert_eq!(std::str::from_utf8(&decoded).unwrap(), url);
    }
}
