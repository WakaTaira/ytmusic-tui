//! Domain model types shared across the ytmusic crate family.
//!
//! These are 1-to-1 ports of the Python dataclasses defined in
//! `src/ytmusic_tui/api.py` (supporting types) and `src/ytmusic_tui/queue.py`
//! (`Track`). All structs are intentionally immutable (no `&mut self` API).

// ---------------------------------------------------------------------------
// Track
// ---------------------------------------------------------------------------

/// Immutable representation of a single music track.
///
/// `PartialEq`, `Eq`, and `Hash` are implemented manually so that the `f64`
/// field is compared and hashed bit-for-bit, mirroring the Python frozen
/// dataclass equality semantics used in tests.
/// Rationale: Python's frozen dataclass is hashable; `Eq`-without-`Hash`
/// violates the std contract (equal values must have equal hashes).
#[derive(Debug, Clone)]
pub struct Track {
    pub video_id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// Duration in seconds (mirrors Python `float`).
    pub duration_seconds: f64,
    pub thumbnail_url: String,
}

impl PartialEq for Track {
    fn eq(&self, other: &Self) -> bool {
        self.video_id == other.video_id
            && self.title == other.title
            && self.artist == other.artist
            && self.album == other.album
            && self.duration_seconds.to_bits() == other.duration_seconds.to_bits()
            && self.thumbnail_url == other.thumbnail_url
    }
}

impl Eq for Track {}

impl std::hash::Hash for Track {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.video_id.hash(state);
        self.title.hash(state);
        self.artist.hash(state);
        self.album.hash(state);
        self.duration_seconds.to_bits().hash(state);
        self.thumbnail_url.hash(state);
    }
}

impl Track {
    /// Construct a [`Track`] with all fields.
    pub fn new(
        video_id: impl Into<String>,
        title: impl Into<String>,
        artist: impl Into<String>,
        album: impl Into<String>,
        duration_seconds: f64,
        thumbnail_url: impl Into<String>,
    ) -> Self {
        Self {
            video_id: video_id.into(),
            title: title.into(),
            artist: artist.into(),
            album: album.into(),
            duration_seconds,
            thumbnail_url: thumbnail_url.into(),
        }
    }

    /// Construct a [`Track`] with only the required fields; optional fields use
    /// their defaults (empty strings, 0.0 duration).
    pub fn new_minimal(
        video_id: impl Into<String>,
        title: impl Into<String>,
        artist: impl Into<String>,
    ) -> Self {
        Self::new(video_id, title, artist, "", 0.0, "")
    }
}

// ---------------------------------------------------------------------------
// RelatedArtist
// ---------------------------------------------------------------------------

/// Lightweight artist reference (e.g. from a "related artists" section).
///
/// Python equivalent: `@dataclass(frozen=True) class RelatedArtist`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelatedArtist {
    pub channel_id: String,
    pub name: String,
    /// Defaults to `""` (Python default `thumbnail_url: str = ""`).
    pub thumbnail_url: String,
}

impl RelatedArtist {
    pub fn new(
        channel_id: impl Into<String>,
        name: impl Into<String>,
        thumbnail_url: impl Into<String>,
    ) -> Self {
        Self {
            channel_id: channel_id.into(),
            name: name.into(),
            thumbnail_url: thumbnail_url.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// AlbumInfo
// ---------------------------------------------------------------------------

/// Album metadata with optional track listing.
///
/// Python equivalent: `@dataclass(frozen=True) class AlbumInfo`.
/// The `tracks` field is empty by default (matches `field(default_factory=list)`).
/// Note: `Eq` is derivable here because `Track` implements `Eq` (manually, bit-for-bit).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlbumInfo {
    pub browse_id: String,
    pub title: String,
    pub artist: String,
    /// Defaults to `""` (Python default `year: str = ""`).
    pub year: String,
    /// Defaults to `[]` (Python `field(default_factory=list)`).
    pub tracks: Vec<Track>,
    /// Defaults to `""` (Python default `thumbnail_url: str = ""`).
    pub thumbnail_url: String,
}

impl AlbumInfo {
    pub fn new(
        browse_id: impl Into<String>,
        title: impl Into<String>,
        artist: impl Into<String>,
        year: impl Into<String>,
        tracks: Vec<Track>,
        thumbnail_url: impl Into<String>,
    ) -> Self {
        Self {
            browse_id: browse_id.into(),
            title: title.into(),
            artist: artist.into(),
            year: year.into(),
            tracks,
            thumbnail_url: thumbnail_url.into(),
        }
    }

    /// Construct an [`AlbumInfo`] without a track listing (library listing context).
    pub fn new_without_tracks(
        browse_id: impl Into<String>,
        title: impl Into<String>,
        artist: impl Into<String>,
        year: impl Into<String>,
        thumbnail_url: impl Into<String>,
    ) -> Self {
        Self::new(browse_id, title, artist, year, vec![], thumbnail_url)
    }
}

// ---------------------------------------------------------------------------
// ArtistInfo
// ---------------------------------------------------------------------------

/// Artist page data: top songs, albums, and related artists.
///
/// Python equivalent: `@dataclass(frozen=True) class ArtistInfo`.
/// Note: `Eq` is derivable because all contained types implement `Eq`.
/// M3d: the get_artist endpoint builds ArtistInfo inline (no dict_to_ helper in Python either).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistInfo {
    pub channel_id: String,
    pub name: String,
    /// Defaults to `""` (Python `description: str = ""`).
    pub description: String,
    /// Defaults to `[]`.
    pub top_songs: Vec<Track>,
    /// Defaults to `[]`.
    pub albums: Vec<AlbumInfo>,
    /// Defaults to `[]`.
    pub related_artists: Vec<RelatedArtist>,
    /// Defaults to `""`.
    pub thumbnail_url: String,
}

impl ArtistInfo {
    pub fn new(
        channel_id: impl Into<String>,
        name: impl Into<String>,
        description: impl Into<String>,
        top_songs: Vec<Track>,
        albums: Vec<AlbumInfo>,
        related_artists: Vec<RelatedArtist>,
        thumbnail_url: impl Into<String>,
    ) -> Self {
        Self {
            channel_id: channel_id.into(),
            name: name.into(),
            description: description.into(),
            top_songs,
            albums,
            related_artists,
            thumbnail_url: thumbnail_url.into(),
        }
    }

    /// Construct a simplified [`ArtistInfo`] (library listing: only identity fields populated).
    pub fn new_minimal(
        channel_id: impl Into<String>,
        name: impl Into<String>,
        thumbnail_url: impl Into<String>,
    ) -> Self {
        Self::new(channel_id, name, "", vec![], vec![], vec![], thumbnail_url)
    }
}

// ---------------------------------------------------------------------------
// PlaylistInfo
// ---------------------------------------------------------------------------

/// Metadata for a playlist (no track contents).
///
/// Python equivalent: `@dataclass(frozen=True) class PlaylistInfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistInfo {
    pub playlist_id: String,
    pub title: String,
    /// Defaults to `""`.
    pub description: String,
    /// Defaults to `0`. Expected range: 0–5000 (ytmusicapi caps at API limit ~5k).
    pub track_count: u32,
    /// Defaults to `""`.
    pub thumbnail_url: String,
}

impl PlaylistInfo {
    pub fn new(
        playlist_id: impl Into<String>,
        title: impl Into<String>,
        description: impl Into<String>,
        track_count: u32,
        thumbnail_url: impl Into<String>,
    ) -> Self {
        Self {
            playlist_id: playlist_id.into(),
            title: title.into(),
            description: description.into(),
            track_count,
            thumbnail_url: thumbnail_url.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// SearchResults
// ---------------------------------------------------------------------------

/// Categorized search results across all content types.
///
/// Python equivalent: `@dataclass(frozen=True) class SearchResults`.
/// All fields default to empty vectors.
/// Note: `Eq` is derivable because all contained types implement `Eq`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchResults {
    pub tracks: Vec<Track>,
    pub albums: Vec<AlbumInfo>,
    pub artists: Vec<RelatedArtist>,
    pub playlists: Vec<PlaylistInfo>,
}

// ---------------------------------------------------------------------------
// HomeSection / HomeSectionItem
// ---------------------------------------------------------------------------

/// A single item in a home page section — either a track or a playlist reference.
///
/// Python equivalent: `Track | PlaylistInfo` in `HomeSection.items`.
/// Note: `Eq` is derivable because `Track` implements `Eq` and `PlaylistInfo` derives `Eq`.
/// TODO: add Hash in lockstep with Track if render-time dedup needs it
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HomeSectionItem {
    Track(Track),
    Playlist(PlaylistInfo),
}

/// A section on the home page (e.g. "Quick picks").
///
/// Python equivalent: `@dataclass(frozen=True) class HomeSection`.
/// Note: `Eq` is derivable because `HomeSectionItem` implements `Eq`.
/// TODO: add Hash in lockstep with Track if render-time dedup needs it
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeSection {
    pub title: String,
    /// Each entry is either a `Track` or a `PlaylistInfo`.
    pub items: Vec<HomeSectionItem>,
}
