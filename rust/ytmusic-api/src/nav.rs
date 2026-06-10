//! Navigation over untyped InnerTube JSON, mirroring `ytmusicapi.navigation`.
//!
//! The stage-1 parsers in [`crate::endpoints`] walk the same renderer chains
//! ytmusicapi does. This module provides:
//!
//! * [`nav`] — the `Option`-returning analogue of ytmusicapi's `nav(root, path,
//!   none_if_absent=True)`. (ytmusicapi's *raising* `nav` has no place here:
//!   stage-1 parsers treat a missing path as "field absent", matching how
//!   `api.py` reads optional dict keys.)
//! * [`Step`] — one path element: an object key or an array index.
//! * Path constants named like ytmusicapi's (`TITLE_TEXT`, `NAVIGATION_VIDEO_ID`,
//!   …), built from `&[Step]` slices.
//!
//! Only the constants the four M3d-1 endpoints (search / playlist / album /
//! artist) actually need are defined — deliberately a small subset of
//! ytmusicapi's full `navigation.py`.

use serde_json::Value;

/// One element of a navigation path: an object key or an array index.
///
/// Mirrors how ytmusicapi mixes `str` keys and `int` indices in a single path
/// list (e.g. `["runs", 0, "text"]`).
#[derive(Debug, Clone, Copy)]
pub enum Step {
    /// Index into a JSON object by key.
    Key(&'static str),
    /// Index into a JSON array by position.
    Index(usize),
}

/// Walk `root` along `path`, returning the value at the end or `None` if any
/// step is absent.
///
/// Equivalent to ytmusicapi's `nav(root, path, none_if_absent=True)`: a missing
/// key or out-of-range index yields `None` rather than raising.
pub fn nav<'a>(root: &'a Value, path: &[Step]) -> Option<&'a Value> {
    let mut cur = root;
    for step in path {
        cur = match step {
            Step::Key(k) => cur.get(k)?,
            Step::Index(i) => cur.get(i)?,
        };
    }
    Some(cur)
}

/// Convenience: walk `path` and return the terminal value as `&str`.
pub fn nav_str<'a>(root: &'a Value, path: &[Step]) -> Option<&'a str> {
    nav(root, path).and_then(Value::as_str)
}

/// Convenience: walk `path` and return the terminal value as a JSON array.
pub fn nav_array<'a>(root: &'a Value, path: &[Step]) -> Option<&'a Vec<Value>> {
    nav(root, path).and_then(Value::as_array)
}

// ---------------------------------------------------------------------------
// Path constants (named after ytmusicapi/navigation.py)
// ---------------------------------------------------------------------------
//
// `K` / `I` shorthands keep these declarations readable and 1:1 with the Python
// list literals they mirror.

const fn k(s: &'static str) -> Step {
    Step::Key(s)
}
const fn i(n: usize) -> Step {
    Step::Index(n)
}

/// `["title", "runs", 0, "text"]`
pub const TITLE_TEXT: &[Step] = &[k("title"), k("runs"), i(0), k("text")];

/// `["navigationEndpoint", "browseEndpoint", "browseId"]`
pub const NAVIGATION_BROWSE_ID: &[Step] =
    &[k("navigationEndpoint"), k("browseEndpoint"), k("browseId")];

/// Thumbnails on a list item:
/// `["thumbnail", "musicThumbnailRenderer", "thumbnail", "thumbnails"]`
pub const THUMBNAILS: &[Step] = &[
    k("thumbnail"),
    k("musicThumbnailRenderer"),
    k("thumbnail"),
    k("thumbnails"),
];

/// Thumbnails on a two-row item:
/// `["thumbnailRenderer", "musicThumbnailRenderer", "thumbnail", "thumbnails"]`
pub const THUMBNAIL_RENDERER: &[Step] = &[
    k("thumbnailRenderer"),
    k("musicThumbnailRenderer"),
    k("thumbnail"),
    k("thumbnails"),
];

/// The play-button overlay on a list item, down to the play navigation endpoint:
/// `["overlay", "musicItemThumbnailOverlayRenderer", "content",
///   "musicPlayButtonRenderer", "playNavigationEndpoint", "watchEndpoint",
///   "videoId"]`
pub const PLAY_BUTTON_VIDEO_ID: &[Step] = &[
    k("overlay"),
    k("musicItemThumbnailOverlayRenderer"),
    k("content"),
    k("musicPlayButtonRenderer"),
    k("playNavigationEndpoint"),
    k("watchEndpoint"),
    k("videoId"),
];

/// Renderer-type marker keys (ytmusicapi's MRLIR / MTRIR / MMRIR string constants).
/// `musicResponsiveListItemRenderer`
pub const MRLIR: &str = "musicResponsiveListItemRenderer";
/// `musicTwoRowItemRenderer`
pub const MTRIR: &str = "musicTwoRowItemRenderer";

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn nav_walks_keys_and_indices() {
        let v = json!({"a": {"b": [10, 20, {"c": "hit"}]}});
        let path = &[
            Step::Key("a"),
            Step::Key("b"),
            Step::Index(2),
            Step::Key("c"),
        ];
        assert_eq!(nav(&v, path).and_then(Value::as_str), Some("hit"));
    }

    #[test]
    fn nav_returns_none_on_missing_key() {
        let v = json!({"a": 1});
        assert!(nav(&v, &[Step::Key("missing")]).is_none());
    }

    #[test]
    fn nav_returns_none_on_out_of_range_index() {
        let v = json!([1, 2]);
        assert!(nav(&v, &[Step::Index(5)]).is_none());
    }

    #[test]
    fn nav_str_extracts_terminal_string() {
        let v = json!({"title": {"runs": [{"text": "Song"}]}});
        assert_eq!(nav_str(&v, TITLE_TEXT), Some("Song"));
    }

    #[test]
    fn nav_array_extracts_terminal_array() {
        let v = json!({"subtitle": {"runs": [{"text": "a"}, {"text": "b"}]}});
        let runs = nav_array(&v, &[Step::Key("subtitle"), Step::Key("runs")]).expect("array");
        assert_eq!(runs.len(), 2);
    }

    #[test]
    fn empty_path_returns_root() {
        let v = json!({"x": 1});
        assert!(std::ptr::eq(nav(&v, &[]).unwrap(), &v));
    }
}
