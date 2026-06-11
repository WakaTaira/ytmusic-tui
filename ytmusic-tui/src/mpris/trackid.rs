//! `mpris:trackid` handling for YouTube video IDs.
//!
//! KEY CONSTRAINT (the whole reason this module exists), proven in the M0 spike
//! (`spikes/mpris_spike/FINDINGS.md` §"mpris:trackid findings"):
//!
//! [`mpris_server::TrackId`] is a newtype around `zbus::zvariant::ObjectPath`
//! (see mpris-server `src/track_id.rs`). It FORCES a valid D-Bus object path.
//! There is no escape hatch to stuff a raw `Variant("s", ...)` string in like
//! the Python (dbus-fast) implementation did.
//!
//! D-Bus object-path grammar (dbus spec): elements match `[A-Za-z0-9_]`,
//! separated by `/`, must begin with `/`. YouTube video IDs use the base64url
//! alphabet which INCLUDES `-` and `_`. `_` is legal in a path element, but
//! `-` is NOT. So a raw YouTube ID such as `O-_kV-pP4kE` is a *valid* video ID
//! but an *invalid* object-path element, and `ObjectPath::try_from` rejects it.
//!
//! Python sidestepped this by emitting `mpris:trackid` as `Variant("s")` (a
//! plain string under `/org/mpris/MediaPlayer2/Track/<id>`); playerctl tolerates
//! it, but it violates the spec (`o`). For the Rust port we instead ENCODE the
//! YouTube ID into a guaranteed-valid object path. `-` -> `_2d` (its hex code,
//! prefixed to disambiguate from a literal `_`), and we escape `_` itself as
//! `_5f` so the mapping stays reversible. This yields a spec-compliant `o` value
//! AND round-trips back to the YT ID. This is strictly better than the Python
//! workaround.

use mpris_server::TrackId;

/// Object-path prefix for our track ids. Lives under our own namespace, never
/// under `/org/mpris` (which the spec reserves).
const TRACK_PREFIX: &str = "/dev/ytmusic_tui/track/";

/// Encode a YouTube video id into a valid D-Bus object path element.
///
/// Reversible: every `_` becomes `_5f` and every `-` becomes `_2d`; all other
/// base64url characters (`[A-Za-z0-9]`) are already path-legal and pass
/// through. The result therefore only contains `[A-Za-z0-9_]`.
pub fn encode_youtube_id(video_id: &str) -> String {
    let mut out = String::with_capacity(video_id.len() + 8);
    for ch in video_id.chars() {
        match ch {
            '_' => out.push_str("_5f"),
            '-' => out.push_str("_2d"),
            c if c.is_ascii_alphanumeric() => out.push(c),
            // Defensive: any unexpected char gets hex-escaped too, keeping the
            // output strictly within the object-path alphabet.
            c => {
                out.push('_');
                out.push_str(&format!("{:02x}", c as u32));
            }
        }
    }
    out
}

/// Build a spec-compliant `mpris:trackid` ([`TrackId`] == `ObjectPath`) from a
/// YouTube video id. Falls back to the canonical "no track" id when empty.
pub fn youtube_trackid(video_id: &str) -> TrackId {
    if video_id.is_empty() {
        return TrackId::NO_TRACK;
    }
    let path = format!("{TRACK_PREFIX}{}", encode_youtube_id(video_id));
    // This `try_from` is exactly the call that would Err on a raw YT id with a
    // `-`. Because we encoded, it is now infallible in practice; we still handle
    // the error rather than unwrap to keep the contract honest.
    TrackId::try_from(path).unwrap_or(TrackId::NO_TRACK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mpris_server::TrackId;

    #[test]
    fn raw_youtube_id_with_dash_is_rejected_by_objectpath() {
        // Proves the constraint: the "natural" approach of shoving the YT id
        // straight into a trackid path fails for ids containing '-'.
        let raw = "/dev/ytmusic_tui/track/O-_kV-pP4kE";
        assert!(
            TrackId::try_from(raw).is_err(),
            "expected ObjectPath to reject '-' in a path element"
        );
    }

    #[test]
    fn encoded_youtube_id_is_accepted() {
        let id = youtube_trackid("O-_kV-pP4kE");
        // Round-trips through ObjectPath successfully (no NO_TRACK fallback).
        assert_ne!(id, TrackId::NO_TRACK);
        assert!(id.as_str().starts_with(TRACK_PREFIX));
    }

    #[test]
    fn encoding_only_uses_path_legal_chars() {
        let enc = encode_youtube_id("aB3-_zZ");
        assert!(
            enc.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "encoded id must stay within [A-Za-z0-9_], got {enc}"
        );
    }

    #[test]
    fn empty_id_maps_to_no_track() {
        assert_eq!(youtube_trackid(""), TrackId::NO_TRACK);
    }
}
