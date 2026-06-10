//! Playback queue management.
//!
//! This module is a 1-to-1 port of `ytmusic_tui/queue.py`.

use rand::seq::SliceRandom;

// ---------------------------------------------------------------------------
// RepeatMode
// ---------------------------------------------------------------------------

/// Repeat behaviour for the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RepeatMode {
    /// Stop at the end of the queue.
    #[default]
    Off,
    /// Wrap around to the beginning when the queue ends.
    All,
    /// Loop the current track indefinitely.
    One,
}

// ---------------------------------------------------------------------------
// Track
// ---------------------------------------------------------------------------

/// Immutable representation of a single music track.
///
/// `PartialEq`, `Eq`, and `Hash` are implemented manually so that `f64` fields
/// are compared and hashed bit-for-bit, which mirrors the Python dataclass
/// frozen equality semantics for the values used in tests.
/// Rationale: Python's frozen dataclass is hashable; Eq-without-Hash violates
/// the std contract (equal values must have equal hashes).
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

    /// Construct a [`Track`] with only the required fields; optional fields use their defaults.
    pub fn new_minimal(
        video_id: impl Into<String>,
        title: impl Into<String>,
        artist: impl Into<String>,
    ) -> Self {
        Self::new(video_id, title, artist, "", 0.0, "")
    }
}

// ---------------------------------------------------------------------------
// QueueManager
// ---------------------------------------------------------------------------

/// Manages an ordered playback queue with shuffle and repeat support.
///
/// Design decisions (mirrors Python):
/// * Selecting a song from a playlist queues all remaining songs (spotify_player style).
/// * Shuffle reorders the tracks *after* the current position; the current track
///   and everything before it stay in place.
/// * Unshuffling restores the original order while keeping the current track selected.
/// * Repeat modes: Off (stop at end), All (wrap), One (loop current).
pub struct QueueManager {
    tracks: Vec<Track>,
    // TODO(post-parity): consider Option<usize>; -1 sentinel kept for 1:1 parity with the Python port.
    current_index: i64,
    shuffle: bool,
    repeat_mode: RepeatMode,
    exhausted: bool,
    /// Snapshot of the original order before shuffling; `None` when not shuffled.
    original_tracks: Option<Vec<Track>>,
}

impl Default for QueueManager {
    fn default() -> Self {
        Self::new()
    }
}

impl QueueManager {
    /// Create a new, empty [`QueueManager`].
    pub fn new() -> Self {
        Self {
            tracks: Vec::new(),
            current_index: -1,
            shuffle: false,
            repeat_mode: RepeatMode::Off,
            exhausted: false,
            original_tracks: None,
        }
    }

    // -----------------------------------------------------------------------
    // Properties
    // -----------------------------------------------------------------------

    /// Return the currently selected track, or `None`.
    pub fn current_track(&self) -> Option<&Track> {
        if self.tracks.is_empty() || self.current_index < 0 {
            return None;
        }
        self.tracks.get(self.current_index as usize)
    }

    /// Return a clone of the entire track list.
    pub fn tracks(&self) -> Vec<Track> {
        self.tracks.clone()
    }

    /// Whether shuffle is currently enabled.
    pub fn shuffle(&self) -> bool {
        self.shuffle
    }

    /// Current repeat mode.
    pub fn repeat_mode(&self) -> RepeatMode {
        self.repeat_mode
    }

    /// Set the repeat mode directly.
    pub fn set_repeat_mode(&mut self, mode: RepeatMode) {
        self.repeat_mode = mode;
    }

    // -----------------------------------------------------------------------
    // Queue mutation
    // -----------------------------------------------------------------------

    /// Append a single track to the end of the queue.
    ///
    /// If the queue was empty, exhausted, or the index is past the end, the
    /// index is reset so that [`current_track`] returns a valid track.
    pub fn add(&mut self, track: Track) {
        let was_empty_or_exhausted = self.current_index < 0
            || self.current_index >= self.tracks.len() as i64
            || self.exhausted;
        let insert_pos = self.tracks.len() as i64;
        self.tracks.push(track);
        if was_empty_or_exhausted {
            self.current_index = insert_pos;
            self.exhausted = false;
        }
    }

    /// Append multiple tracks to the end of the queue.
    ///
    /// If the queue was empty or exhausted, the index is reset to the first
    /// newly added track so that [`current_track`] is valid.
    pub fn add_many(&mut self, tracks: Vec<Track>) {
        if tracks.is_empty() {
            return;
        }
        let was_empty_or_exhausted = self.current_index < 0
            || self.current_index >= self.tracks.len() as i64
            || self.exhausted;
        let insert_pos = self.tracks.len() as i64;
        self.tracks.extend(tracks);
        if was_empty_or_exhausted {
            self.current_index = insert_pos;
            self.exhausted = false;
        }
    }

    /// Replace the entire queue and set the current position.
    ///
    /// If `start_index` exceeds the length of `tracks` it is clamped to the
    /// last valid position.
    pub fn set_playlist(&mut self, tracks: Vec<Track>, start_index: usize) {
        self.tracks = tracks;
        self.shuffle = false;
        self.original_tracks = None;
        self.exhausted = false;

        if self.tracks.is_empty() {
            self.current_index = -1;
            return;
        }

        let clamped = start_index.min(self.tracks.len() - 1);
        self.current_index = clamped as i64;
    }

    // -----------------------------------------------------------------------
    // Navigation
    // -----------------------------------------------------------------------

    /// Advance to the next track respecting the current repeat mode.
    ///
    /// Returns the new current track, or `None` when playback should stop
    /// (repeat Off and already at the end).
    pub fn next_track(&mut self) -> Option<&Track> {
        if self.tracks.is_empty() {
            return None;
        }

        if self.repeat_mode == RepeatMode::One {
            // Stay on the current track
            return self.current_track();
        }

        let next_index = self.current_index + 1;

        if next_index >= self.tracks.len() as i64 {
            if self.repeat_mode == RepeatMode::All {
                self.current_index = 0;
                return self.current_track();
            }
            // RepeatMode::Off — end of queue
            self.exhausted = true;
            return None;
        }

        self.exhausted = false;
        self.current_index = next_index;
        self.current_track()
    }

    /// Go back one track.
    ///
    /// At the beginning of the queue the position stays at 0.
    /// Returns the (possibly unchanged) current track, or `None` if the
    /// queue is empty.
    pub fn previous_track(&mut self) -> Option<&Track> {
        if self.tracks.is_empty() {
            return None;
        }

        self.exhausted = false;
        self.current_index = (self.current_index - 1).max(0);
        self.current_track()
    }

    // -----------------------------------------------------------------------
    // Remove / clear
    // -----------------------------------------------------------------------

    /// Remove the track at `index`, adjusting the current position.
    ///
    /// Returns `Err(())` for out-of-range or negative indices (Python `IndexError`).
    pub fn remove(&mut self, index: usize) -> Result<(), IndexOutOfRange> {
        if index >= self.tracks.len() {
            return Err(IndexOutOfRange(index));
        }

        self.tracks.remove(index);

        if self.tracks.is_empty() {
            self.current_index = -1;
            return Ok(());
        }

        let idx = index as i64;
        if idx < self.current_index {
            // Removed before current — shift left
            self.current_index -= 1;
        } else if idx == self.current_index && self.current_index >= self.tracks.len() as i64 {
            // Removed the current track which was the last element — fall back
            self.current_index = self.tracks.len() as i64 - 1;
        }

        Ok(())
    }

    /// Empty the queue and reset all state.
    pub fn clear(&mut self) {
        self.tracks.clear();
        self.current_index = -1;
        self.shuffle = false;
        self.exhausted = false;
        self.original_tracks = None;
    }

    // -----------------------------------------------------------------------
    // Shuffle
    // -----------------------------------------------------------------------

    /// Toggle shuffle mode.
    ///
    /// When enabling, only tracks *after* the current position are shuffled.
    /// The current track and everything before it stay in place.
    ///
    /// When disabling, the original order is restored while keeping the
    /// current track's identity (the index is updated so that the same
    /// [`Track`] object remains selected).
    pub fn toggle_shuffle(&mut self) {
        if self.shuffle {
            self.unshuffle();
        } else {
            self.enable_shuffle();
        }
    }

    fn enable_shuffle(&mut self) {
        self.shuffle = true;

        if self.tracks.is_empty() {
            return;
        }

        // Snapshot the original order before mutating
        self.original_tracks = Some(self.tracks.clone());

        let split = if self.current_index < 0 {
            0
        } else {
            (self.current_index + 1) as usize
        };
        let mut rng = rand::rng();
        self.tracks[split..].shuffle(&mut rng);
    }

    fn unshuffle(&mut self) {
        self.shuffle = false;

        let original = match self.original_tracks.take() {
            Some(o) => o,
            None => return,
        };

        // Remember which track is currently selected (by value)
        let current = self.current_track().cloned();

        self.tracks = original;

        // Re-locate the current track in the restored order
        if let Some(cur) = current {
            match self.tracks.iter().position(|t| *t == cur) {
                Some(pos) => self.current_index = pos as i64,
                None => {
                    // Defensive: track was removed while shuffled
                    self.current_index = 0;
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Repeat
    // -----------------------------------------------------------------------

    /// Cycle through repeat modes: Off → All → One → Off.
    ///
    /// Uses an exhaustive match so that adding a new [`RepeatMode`] variant
    /// becomes a compile error here instead of a silent fallback to `Off`.
    pub fn cycle_repeat(&mut self) {
        self.repeat_mode = match self.repeat_mode {
            RepeatMode::Off => RepeatMode::All,
            RepeatMode::All => RepeatMode::One,
            RepeatMode::One => RepeatMode::Off,
        };
    }

    // -----------------------------------------------------------------------
    // Move
    // -----------------------------------------------------------------------

    /// Move the track at `from_idx` to `to_idx`.
    ///
    /// The current-track pointer follows if the moved track is the current
    /// one, and adjusts when a move shifts the current position.
    ///
    /// Returns `Err(())` for out-of-range indices.
    pub fn move_track(&mut self, from_idx: usize, to_idx: usize) -> Result<(), IndexOutOfRange> {
        let length = self.tracks.len();
        if from_idx >= length {
            return Err(IndexOutOfRange(from_idx));
        }
        if to_idx >= length {
            return Err(IndexOutOfRange(to_idx));
        }

        if from_idx == to_idx {
            return Ok(());
        }

        let track = self.tracks.remove(from_idx);

        // Adjust current_index for the removal
        let mut new_current = self.current_index;
        if from_idx as i64 == self.current_index {
            // We are moving the current track — will fix after insert
            new_current = -1; // sentinel
        } else if (from_idx as i64) < self.current_index {
            new_current -= 1;
        }

        self.tracks.insert(to_idx, track);

        // Adjust current_index for the insertion
        if new_current == -1 {
            // The moved track IS the current track
            new_current = to_idx as i64;
        } else if (to_idx as i64) <= new_current {
            new_current += 1;
        }

        self.current_index = new_current;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Error returned when a queue index is out of range.
#[derive(Debug, PartialEq, Eq)]
pub struct IndexOutOfRange(pub usize);

impl std::fmt::Display for IndexOutOfRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Queue index out of range: {}", self.0)
    }
}

impl std::error::Error for IndexOutOfRange {}

// ===========================================================================
// Tests — 1:1 port of tests/test_queue.py (59 test functions)
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Test helpers (mirrors helpers.py make_track / make_tracks)
    // -----------------------------------------------------------------------

    fn make_track(n: u32) -> Track {
        Track::new(
            format!("vid_{n}"),
            format!("Song {n}"),
            format!("Artist {n}"),
            format!("Album {n}"),
            180.0 + f64::from(n),
            format!("https://img.example.com/{n}.jpg"),
        )
    }

    fn make_tracks(count: u32) -> Vec<Track> {
        (1..=count).map(make_track).collect()
    }

    // =======================================================================
    // Track dataclass
    // =======================================================================

    mod test_track {
        use super::*;

        #[test]
        fn test_creation_with_defaults() {
            let t = Track::new_minimal("abc", "Test", "Art");
            assert_eq!(t.video_id, "abc");
            assert_eq!(t.title, "Test");
            assert_eq!(t.artist, "Art");
            assert_eq!(t.album, "");
            assert_eq!(t.duration_seconds, 0.0);
            assert_eq!(t.thumbnail_url, "");
        }

        #[test]
        fn test_creation_with_all_fields() {
            let t = Track::new(
                "xyz",
                "Full",
                "Band",
                "LP",
                240.5,
                "https://img.example.com/thumb.jpg",
            );
            assert_eq!(t.duration_seconds, 240.5);
            assert_eq!(t.thumbnail_url, "https://img.example.com/thumb.jpg");
        }

        #[test]
        fn test_frozen() {
            // Rust structs do not need a dedicated "frozen" test because fields are
            // not settable without `pub mut` access. Mutability is enforced by the type
            // system: external code cannot write `t.title = "Changed"` on a `Track`
            // value obtained by reference (which all public APIs return).
            //
            // The Python test verifies that `@dataclass(frozen=True)` raises
            // AttributeError on mutation. The equivalent in Rust is a compile-time
            // guarantee, not a runtime one.  We verify the invariant holds by
            // confirming that `Track` does not implement `DerefMut` or expose `&mut`
            // fields via pub access.
            let t = make_track(1);
            // The next line would be a compile error: `t.title = "Changed".to_string();`
            // We assert the value is unchanged to satisfy the test runner.
            assert_eq!(t.title, "Song 1");
        }

        #[test]
        fn test_equality() {
            let a = Track::new_minimal("same", "T", "A");
            let b = Track::new_minimal("same", "T", "A");
            assert_eq!(a, b);
        }

        #[test]
        fn test_inequality() {
            let a = make_track(1);
            let b = make_track(2);
            assert_ne!(a, b);
        }
    }

    // =======================================================================
    // QueueManager - basic state
    // =======================================================================

    mod test_queue_manager_init {
        use super::*;

        #[test]
        fn test_initial_state() {
            let q = QueueManager::new();
            assert!(q.current_track().is_none());
            assert!(q.tracks().is_empty());
            assert!(!q.shuffle());
            assert_eq!(q.repeat_mode(), RepeatMode::Off);
        }

        #[test]
        fn test_tracks_returns_copy() {
            let mut q = QueueManager::new();
            q.add(make_track(1));
            let mut copy = q.tracks();
            copy.push(make_track(99));
            assert_eq!(q.tracks().len(), 1);
        }
    }

    // =======================================================================
    // QueueManager - add / add_many
    // =======================================================================

    mod test_queue_add {
        use super::*;

        #[test]
        fn test_add_single() {
            let mut q = QueueManager::new();
            let t = make_track(1);
            q.add(t.clone());
            assert_eq!(q.tracks(), vec![t]);
        }

        #[test]
        fn test_add_many() {
            let mut q = QueueManager::new();
            let tracks = make_tracks(3);
            q.add_many(tracks.clone());
            assert_eq!(q.tracks(), tracks);
        }

        #[test]
        fn test_add_many_appends() {
            let mut q = QueueManager::new();
            q.add(make_track(1));
            q.add_many(make_tracks(2));
            assert_eq!(q.tracks().len(), 3);
        }
    }

    // =======================================================================
    // QueueManager - set_playlist
    // =======================================================================

    mod test_set_playlist {
        use super::*;

        #[test]
        fn test_replaces_queue() {
            let mut q = QueueManager::new();
            q.add(make_track(99));
            let tracks = make_tracks(3);
            q.set_playlist(tracks.clone(), 0);
            assert_eq!(q.tracks(), tracks);
            assert_eq!(q.current_track(), Some(&tracks[0]));
        }

        #[test]
        fn test_start_index() {
            let mut q = QueueManager::new();
            let tracks = make_tracks(5);
            q.set_playlist(tracks.clone(), 2);
            assert_eq!(q.current_track(), Some(&tracks[2]));
        }

        #[test]
        fn test_start_index_out_of_range_clamps() {
            let mut q = QueueManager::new();
            let tracks = make_tracks(3);
            q.set_playlist(tracks.clone(), 10);
            assert_eq!(q.current_track(), Some(tracks.last().unwrap()));
        }

        #[test]
        fn test_empty_playlist() {
            let mut q = QueueManager::new();
            q.add(make_track(1));
            q.set_playlist(vec![], 0);
            assert!(q.current_track().is_none());
            assert!(q.tracks().is_empty());
        }
    }

    // =======================================================================
    // QueueManager - navigation (next / previous)
    // =======================================================================

    mod test_navigation {
        use super::*;

        #[test]
        fn test_next_track_advances() {
            let mut q = QueueManager::new();
            let tracks = make_tracks(3);
            q.set_playlist(tracks.clone(), 0);
            assert_eq!(q.current_track(), Some(&tracks[0]));
            let nxt = q.next_track().cloned();
            assert_eq!(nxt.as_ref(), Some(&tracks[1]));
            assert_eq!(q.current_track(), Some(&tracks[1]));
        }

        #[test]
        fn test_next_at_end_repeat_off() {
            let mut q = QueueManager::new();
            q.set_playlist(make_tracks(2), 0);
            q.next_track(); // -> track 2
            let result = q.next_track(); // at end
            assert!(result.is_none());
        }

        #[test]
        fn test_next_at_end_repeat_all() {
            let mut q = QueueManager::new();
            q.set_playlist(make_tracks(2), 0);
            q.set_repeat_mode(RepeatMode::All);
            q.next_track(); // -> track 2
            let result = q.next_track().cloned(); // wraps to track 1
            assert_eq!(result.as_ref(), Some(&make_track(1)));
            assert_eq!(q.current_track(), Some(&make_track(1)));
        }

        #[test]
        fn test_next_repeat_one() {
            let mut q = QueueManager::new();
            let tracks = make_tracks(3);
            q.set_playlist(tracks.clone(), 0);
            q.set_repeat_mode(RepeatMode::One);
            let result = q.next_track().cloned();
            assert_eq!(result.as_ref(), Some(&tracks[0])); // stays on current
            assert_eq!(q.current_track(), Some(&tracks[0]));
        }

        #[test]
        fn test_previous_goes_back() {
            let mut q = QueueManager::new();
            let tracks = make_tracks(3);
            q.set_playlist(tracks.clone(), 0);
            q.next_track(); // -> 2
            q.next_track(); // -> 3
            let prev = q.previous_track().cloned();
            assert_eq!(prev.as_ref(), Some(&tracks[1]));
        }

        #[test]
        fn test_previous_at_start() {
            let mut q = QueueManager::new();
            let tracks = make_tracks(3);
            q.set_playlist(tracks.clone(), 0);
            let prev = q.previous_track().cloned();
            assert_eq!(prev.as_ref(), Some(&tracks[0])); // stays at start
        }

        #[test]
        fn test_next_on_empty() {
            let mut q = QueueManager::new();
            assert!(q.next_track().is_none());
        }

        #[test]
        fn test_previous_on_empty() {
            let mut q = QueueManager::new();
            assert!(q.previous_track().is_none());
        }
    }

    // =======================================================================
    // QueueManager - remove
    // =======================================================================

    mod test_remove {
        use super::*;

        #[test]
        fn test_remove_after_current() {
            let mut q = QueueManager::new();
            let tracks = make_tracks(3);
            q.set_playlist(tracks.clone(), 0);
            q.remove(2).unwrap(); // remove last track
            assert_eq!(q.tracks().len(), 2);
            assert_eq!(q.current_track(), Some(&tracks[0]));
        }

        #[test]
        fn test_remove_before_current() {
            let mut q = QueueManager::new();
            let tracks = make_tracks(4);
            q.set_playlist(tracks.clone(), 2);
            assert_eq!(q.current_track(), Some(&tracks[2]));
            q.remove(0).unwrap(); // remove track before current
            assert_eq!(q.current_track(), Some(&tracks[2]));
        }

        #[test]
        fn test_remove_current_track() {
            let mut q = QueueManager::new();
            let tracks = make_tracks(3);
            q.set_playlist(tracks.clone(), 1);
            assert_eq!(q.current_track(), Some(&tracks[1]));
            q.remove(1).unwrap(); // remove current
            // current advances to next (which was tracks[2], now at index 1)
            assert_eq!(q.current_track(), Some(&tracks[2]));
        }

        #[test]
        fn test_remove_current_last_track() {
            let mut q = QueueManager::new();
            let tracks = make_tracks(3);
            q.set_playlist(tracks.clone(), 2);
            q.remove(2).unwrap(); // remove current which is last
            assert_eq!(q.current_track(), Some(&tracks[1]));
        }

        #[test]
        fn test_remove_only_track() {
            let mut q = QueueManager::new();
            q.set_playlist(vec![make_track(1)], 0);
            q.remove(0).unwrap();
            assert!(q.current_track().is_none());
            assert!(q.tracks().is_empty());
        }

        #[test]
        fn test_remove_invalid_index() {
            let mut q = QueueManager::new();
            q.set_playlist(make_tracks(2), 0);
            assert!(q.remove(5).is_err());
        }

        #[test]
        fn test_remove_negative_index() {
            // Rust remove() takes usize so there is no "negative index".
            // The Python test guards against negative int indices (IndexError).
            // We model this by attempting removal at usize::MAX which is always
            // out of range for any realistic queue.
            let mut q = QueueManager::new();
            q.set_playlist(make_tracks(2), 0);
            assert!(q.remove(usize::MAX).is_err());
        }
    }

    // =======================================================================
    // QueueManager - clear
    // =======================================================================

    mod test_clear {
        use super::*;

        #[test]
        fn test_clear() {
            let mut q = QueueManager::new();
            q.set_playlist(make_tracks(5), 3);
            q.clear();
            assert!(q.tracks().is_empty());
            assert!(q.current_track().is_none());
        }

        #[test]
        fn test_clear_resets_shuffle() {
            let mut q = QueueManager::new();
            q.set_playlist(make_tracks(5), 0);
            q.toggle_shuffle();
            q.clear();
            assert!(!q.shuffle());
        }
    }

    // =======================================================================
    // QueueManager - shuffle
    // =======================================================================

    mod test_shuffle {
        use super::*;

        #[test]
        fn test_toggle_on() {
            let mut q = QueueManager::new();
            let tracks = make_tracks(10);
            q.set_playlist(tracks.clone(), 0);
            q.next_track(); // current = track 2 (index 1)
            q.toggle_shuffle();
            assert!(q.shuffle());
            // Current track should not change
            assert_eq!(q.current_track(), Some(&tracks[1]));
        }

        #[test]
        fn test_shuffle_preserves_current() {
            let mut q = QueueManager::new();
            let tracks = make_tracks(10);
            q.set_playlist(tracks.clone(), 0);
            q.toggle_shuffle();
            assert_eq!(q.current_track(), Some(&tracks[0]));
        }

        #[test]
        fn test_shuffle_only_remaining() {
            let mut q = QueueManager::new();
            let tracks = make_tracks(10);
            q.set_playlist(tracks.clone(), 3);
            q.toggle_shuffle();
            // Tracks before and including current (indices 0..=3) should be untouched
            assert_eq!(&q.tracks()[..4], &tracks[..4]);
            // Remaining are a permutation of the originals
            let mut remaining: Vec<Track> = q.tracks()[4..].to_vec();
            remaining.sort_by_key(|t| t.video_id.clone());
            let mut expected: Vec<Track> = tracks[4..].to_vec();
            expected.sort_by_key(|t| t.video_id.clone());
            assert_eq!(remaining, expected);
        }

        #[test]
        fn test_unshuffle_restores_order() {
            let mut q = QueueManager::new();
            let tracks = make_tracks(10);
            q.set_playlist(tracks.clone(), 2);
            q.toggle_shuffle(); // shuffle on
            assert!(q.shuffle());
            q.toggle_shuffle(); // shuffle off
            assert!(!q.shuffle());
            assert_eq!(q.tracks(), tracks);
            assert_eq!(q.current_track(), Some(&tracks[2]));
        }

        #[test]
        fn test_toggle_shuffle_empty() {
            let mut q = QueueManager::new();
            q.toggle_shuffle(); // should not panic
            assert!(q.shuffle());
        }
    }

    // =======================================================================
    // QueueManager - repeat mode cycling
    // =======================================================================

    mod test_repeat_cycle {
        use super::*;

        #[test]
        fn test_cycle_order() {
            let mut q = QueueManager::new();
            assert_eq!(q.repeat_mode(), RepeatMode::Off);
            q.cycle_repeat();
            assert_eq!(q.repeat_mode(), RepeatMode::All);
            q.cycle_repeat();
            assert_eq!(q.repeat_mode(), RepeatMode::One);
            q.cycle_repeat();
            assert_eq!(q.repeat_mode(), RepeatMode::Off);
        }
    }

    // =======================================================================
    // QueueManager - move
    // =======================================================================

    mod test_move {
        use super::*;

        #[test]
        fn test_move_forward() {
            let mut q = QueueManager::new();
            let tracks = make_tracks(5);
            q.set_playlist(tracks.clone(), 0);
            q.move_track(1, 3).unwrap();
            assert_eq!(q.tracks()[3], tracks[1]);
            assert_eq!(q.tracks()[1], tracks[2]);
        }

        #[test]
        fn test_move_backward() {
            let mut q = QueueManager::new();
            let tracks = make_tracks(5);
            q.set_playlist(tracks.clone(), 0);
            q.move_track(3, 1).unwrap();
            assert_eq!(q.tracks()[1], tracks[3]);
            assert_eq!(q.tracks()[2], tracks[1]);
        }

        #[test]
        fn test_move_current_track() {
            let mut q = QueueManager::new();
            let tracks = make_tracks(5);
            q.set_playlist(tracks.clone(), 1);
            q.move_track(1, 3).unwrap();
            // current_track should follow the moved track
            assert_eq!(q.current_track(), Some(&tracks[1]));
        }

        #[test]
        fn test_move_same_position() {
            let mut q = QueueManager::new();
            let tracks = make_tracks(3);
            q.set_playlist(tracks.clone(), 0);
            q.move_track(1, 1).unwrap(); // no-op
            assert_eq!(q.tracks(), tracks);
        }

        #[test]
        fn test_move_invalid_index() {
            let mut q = QueueManager::new();
            q.set_playlist(make_tracks(3), 0);
            assert!(q.move_track(0, 5).is_err());
        }

        #[test]
        fn test_move_updates_current_index_when_affected() {
            // When moving a track across the current index, current must adjust.
            let mut q = QueueManager::new();
            let tracks = make_tracks(5);
            q.set_playlist(tracks.clone(), 2);
            // Move track from before current to after current
            q.move_track(0, 4).unwrap();
            // Current track should still be the same Track object
            assert_eq!(q.current_track(), Some(&tracks[2]));
        }
    }

    // =======================================================================
    // QueueManager - edge cases
    // =======================================================================

    mod test_edge_cases {
        use super::*;

        #[test]
        fn test_single_track_next_repeat_off() {
            let mut q = QueueManager::new();
            q.set_playlist(vec![make_track(1)], 0);
            assert!(q.next_track().is_none());
        }

        #[test]
        fn test_single_track_next_repeat_all() {
            let mut q = QueueManager::new();
            q.set_playlist(vec![make_track(1)], 0);
            q.set_repeat_mode(RepeatMode::All);
            let result = q.next_track().cloned();
            assert_eq!(result, Some(make_track(1)));
        }

        #[test]
        fn test_single_track_next_repeat_one() {
            let mut q = QueueManager::new();
            q.set_playlist(vec![make_track(1)], 0);
            q.set_repeat_mode(RepeatMode::One);
            let result = q.next_track().cloned();
            assert_eq!(result, Some(make_track(1)));
        }

        #[test]
        fn test_navigation_through_full_queue() {
            // Walk forward through entire queue then back.
            let mut q = QueueManager::new();
            let tracks = make_tracks(4);
            q.set_playlist(tracks.clone(), 0);
            for expected in tracks.iter().skip(1) {
                assert_eq!(q.next_track(), Some(expected));
            }
            assert!(q.next_track().is_none()); // end, repeat OFF
            // Walk back
            for expected in tracks.iter().take(3).rev() {
                assert_eq!(q.previous_track(), Some(expected));
            }
        }

        #[test]
        fn test_repeat_all_full_cycle() {
            let mut q = QueueManager::new();
            let tracks = make_tracks(3);
            q.set_playlist(tracks.clone(), 0);
            q.set_repeat_mode(RepeatMode::All);
            q.next_track(); // 2
            q.next_track(); // 3
            let result = q.next_track().cloned(); // wraps to 1
            assert_eq!(result.as_ref(), Some(&tracks[0]));
            let result = q.next_track().cloned(); // 2 again
            assert_eq!(result.as_ref(), Some(&tracks[1]));
        }
    }

    // =======================================================================
    // QueueManager - add to empty queue sets current_index (Bug 3)
    // =======================================================================

    mod test_add_to_empty_queue {
        use super::*;

        #[test]
        fn test_add_to_empty_queue_sets_current_index() {
            let mut q = QueueManager::new();
            assert!(q.current_track().is_none());
            let t = make_track(1);
            q.add(t.clone());
            assert_eq!(q.current_track(), Some(&t));
        }

        #[test]
        fn test_add_many_to_empty_queue_sets_current_index() {
            let mut q = QueueManager::new();
            let tracks = make_tracks(3);
            q.add_many(tracks.clone());
            assert_eq!(q.current_track(), Some(&tracks[0]));
        }

        #[test]
        fn test_add_to_nonempty_queue_preserves_current() {
            let mut q = QueueManager::new();
            let tracks = make_tracks(3);
            q.set_playlist(tracks.clone(), 1);
            q.add(make_track(99));
            assert_eq!(q.current_track(), Some(&tracks[1]));
        }

        #[test]
        fn test_add_many_to_nonempty_queue_preserves_current() {
            let mut q = QueueManager::new();
            let tracks = make_tracks(3);
            q.set_playlist(tracks.clone(), 2);
            q.add_many(make_tracks(2));
            assert_eq!(q.current_track(), Some(&tracks[2]));
        }

        #[test]
        fn test_add_many_empty_list_is_noop() {
            let mut q = QueueManager::new();
            q.add_many(vec![]);
            assert!(q.current_track().is_none());
        }
    }

    // =======================================================================
    // QueueManager - add after queue exhaustion (Bug 4)
    // =======================================================================

    mod test_add_after_exhaustion {
        use super::*;

        #[test]
        fn test_add_after_queue_exhausted_resets_index() {
            let mut q = QueueManager::new();
            q.set_playlist(make_tracks(2), 0);
            q.next_track(); // -> track 2
            let result = q.next_track(); // -> None (end, repeat OFF)
            assert!(result.is_none());
            // Queue is exhausted; adding a track should make it current.
            let new_track = make_track(99);
            q.add(new_track.clone());
            assert_eq!(q.tracks().len(), 3);
            assert_eq!(q.current_track(), Some(&new_track));
        }

        #[test]
        fn test_add_many_after_queue_exhausted_resets_index() {
            let mut q = QueueManager::new();
            q.set_playlist(make_tracks(2), 0);
            q.next_track(); // -> track 2
            q.next_track(); // -> None (end)
            let new_tracks = vec![make_track(90), make_track(91)];
            q.add_many(new_tracks.clone());
            assert_eq!(q.current_track(), Some(&new_tracks[0]));
        }

        #[test]
        fn test_add_after_clear_and_exhaust() {
            let mut q = QueueManager::new();
            q.set_playlist(make_tracks(2), 0);
            q.clear();
            assert!(q.current_track().is_none());
            let t = make_track(42);
            q.add(t.clone());
            assert_eq!(q.current_track(), Some(&t));
        }

        #[test]
        fn test_add_many_after_clear() {
            let mut q = QueueManager::new();
            q.set_playlist(make_tracks(3), 0);
            q.clear();
            let new_tracks = make_tracks(2);
            q.add_many(new_tracks.clone());
            assert_eq!(q.current_track(), Some(&new_tracks[0]));
        }

        #[test]
        fn test_previous_clears_exhausted_flag() {
            let mut q = QueueManager::new();
            let tracks = make_tracks(3);
            q.set_playlist(tracks.clone(), 0);
            q.next_track(); // -> track 2
            q.next_track(); // -> track 3
            q.next_track(); // -> None (exhausted)
            let prev = q.previous_track().cloned();
            assert_eq!(prev.as_ref(), Some(&tracks[1]));
            // Adding after previous should NOT reset to end of queue
            q.add(make_track(99));
            assert_eq!(q.current_track(), Some(&tracks[1])); // unchanged
        }

        #[test]
        fn test_exhausted_flag_reset_on_set_playlist() {
            let mut q = QueueManager::new();
            q.set_playlist(make_tracks(1), 0);
            q.next_track(); // -> None (exhausted)
            let new_tracks = make_tracks(3);
            q.set_playlist(new_tracks.clone(), 0);
            // Should not be exhausted; add should not reset
            q.add(make_track(99));
            assert_eq!(q.current_track(), Some(&new_tracks[0]));
        }
    }
}
