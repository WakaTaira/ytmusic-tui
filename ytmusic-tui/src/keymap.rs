//! Keymap dispatcher: resolve crossterm key events to [`Action`]s via the
//! loaded `action → key` map (see [`crate::config::load_keymap`]).
//!
//! # Why a dispatcher (the M5c rework)
//!
//! Through M5b the binary hard-coded a `match` from [`KeyEvent`] to action
//! (`main.rs::map_key`). M5c wires the *configurable* keymap: `config::load_keymap`
//! merges the hard-coded [`crate::config::DEFAULT_KEYMAP`], the bundled
//! `default_keymap.toml`, and the user's `keymap.toml` into a
//! `HashMap<action_name, key_string>`. The **action names are the contract**
//! (`quit`, `search`, `switch_queue`, `cycle_audio_quality`, `search_page`, …);
//! this module inverts that map into a lookup from a parsed [`KeySpec`] to an
//! [`Action`], so re-binding a key in `keymap.toml` Just Works.
//!
//! # Key-string syntax (mirrors Python / Textual conventions)
//!
//! A keymap *value* is one of:
//!
//! * A single printable char: `"n"`, `"Q"`, `"+"`.
//! * A named special key: `"space"`, `"escape"`, `"tab"`, `"enter"`, `"plus"`,
//!   `"minus"`, `"equal"`, `"slash"`, `"full_stop"`, `"greater_than_sign"`,
//!   `"less_than_sign"`, `"circumflex_accent"`, `"underscore"`.
//! * A modifier-prefixed key: `"ctrl+s"`, `"ctrl+shift+s"`.
//! * Comma-separated **alternatives**, any of which trigger the action:
//!   `"plus,equal"`.
//! * Space-separated **sequences** of two keys (the textual-couldn't-do-this
//!   feature, directive §6): `"g s"` fires the action only after `g` then `s`.
//!
//! Unknown / unparseable specs are dropped silently (error-tolerant): a typo in
//! a user `keymap.toml` disables that one binding rather than aborting startup.

use std::collections::HashMap;
use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config;

/// A high-level UI action, the resolved target of a key binding.
///
/// The variants correspond 1:1 to the action *names* in
/// [`crate::config::DEFAULT_KEYMAP`] plus a couple of selection/navigation
/// actions that have no keymap entry (they are bound to the always-on
/// arrow/`j`/`k`/Tab/Enter keys).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Quit the app (`quit`, default `Q`; Ctrl+C also quits, handled directly).
    Quit,
    /// Toggle play/pause (`toggle_pause`, default `space`).
    TogglePause,
    /// Volume up (`volume_up`, default `plus,equal`).
    VolumeUp,
    /// Volume down (`volume_down`, default `minus`).
    VolumeDown,
    /// Skip to the next queued track (`next_track`, default `n`).
    NextTrack,
    /// Go back to the previous track (`previous_track`, default `p`).
    PreviousTrack,
    /// Toggle shuffle (`toggle_shuffle`, default `s`).
    ToggleShuffle,
    /// Cycle the repeat mode (`cycle_repeat`, default `r`).
    CycleRepeat,
    /// Cycle the audio quality (`cycle_audio_quality`, default `b`).
    CycleAudioQuality,
    /// Seek forward (`seek_forward`, default `greater_than_sign` / `>`).
    SeekForward,
    /// Seek backward (`seek_backward`, default `less_than_sign` / `<`).
    SeekBackward,
    /// Seek to the start of the track (`seek_start`, default
    /// `circumflex_accent` / `^`).
    SeekToStart,
    /// Toggle audio mute (`toggle_mute`, default `underscore` / `_`).
    ToggleMute,
    /// Toggle the like state of the current track (`toggle_like`, default `f`).
    ToggleLike,
    /// Start a radio seeded by the current track (`start_radio`, default `R`).
    StartRadio,
    /// Jump to the current track's artist (`open_current_artist`, default `a`).
    OpenCurrentArtist,
    /// Jump to the current track's album (`open_current_album`, default `A`).
    OpenCurrentAlbum,
    /// Go back / pop the navigation stack (`go_back`, default `escape`).
    GoBack,
    /// Toggle the in-page filter bar (`search`, default `slash`).
    ToggleFilter,
    /// Switch to the home view (`switch_home`, default `g`).
    SwitchHome,
    /// Switch to the search view + focus its input (`search_page`, default
    /// `g s`). Reclaimed from `/` in M5c (now the filter toggle).
    SearchPage,
    /// Switch to the library view (`switch_library`, default `l`).
    SwitchLibrary,
    /// Switch to the queue view (`switch_queue`, default `q`).
    SwitchQueue,
    /// Switch to the history view (`switch_history`, default `H`).
    SwitchHistory,
    /// Open lyrics for the current track (`open_lyrics`, default `L`).
    SwitchLyrics,
    /// Open the context-action popup for the selected item (`open_action_popup`,
    /// default `full_stop`).
    OpenActionPopup,
    /// Open the theme-picker popup (`open_theme_popup`, default `T`).
    OpenThemePopup,
}

/// A parsed key specification: a [`KeyCode`] plus its required [`KeyModifiers`].
///
/// This is the normalized form a [`KeyEvent`] is reduced to for lookup. Only the
/// modifiers Python expresses (`ctrl`, `shift`) are tracked; `shift` is folded
/// into the char itself for printable keys (Textual treats `"Q"` as Shift+`q`),
/// so an explicit `SHIFT` modifier is only kept for non-char keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeySpec {
    code: KeyCode,
    /// The modifiers that must be present. CONTROL is the only one carried for
    /// char keys; SHIFT for a char is encoded by the uppercase char itself.
    modifiers: KeyModifiers,
}

impl KeySpec {
    /// Reduce a live [`KeyEvent`] to the [`KeySpec`] used for map lookup.
    ///
    /// Normalization rules (so the lookup table and the live event agree):
    ///
    /// * Only CONTROL is significant for char keys. SHIFT is already baked into
    ///   the uppercase char that crossterm reports, so it is stripped here to
    ///   avoid a double-count (otherwise `"Q"` would need `SHIFT` *and* the
    ///   uppercase char).
    /// * For non-char keys, CONTROL and SHIFT are both kept (e.g. a future
    ///   `ctrl+enter`); ALT and the rest are dropped — Python's keymap never
    ///   used them.
    #[must_use]
    pub fn from_event(event: KeyEvent) -> Self {
        let mut modifiers = event.modifiers & (KeyModifiers::CONTROL | KeyModifiers::SHIFT);
        if matches!(event.code, KeyCode::Char(_)) {
            // The char already encodes case; drop SHIFT so it is not required
            // on top of the uppercase char.
            modifiers.remove(KeyModifiers::SHIFT);
        }
        Self {
            code: event.code,
            modifiers,
        }
    }
}

/// Parse a single (non-comma, non-space) key token into a [`KeySpec`].
///
/// Handles `modifier+...+key` chains. Returns `None` for an unparseable token
/// (error-tolerant: the caller drops the whole alternative).
fn parse_key_token(token: &str) -> Option<KeySpec> {
    let token = token.trim();
    if token.is_empty() {
        return None;
    }

    // Split off modifier prefixes (`ctrl+`, `shift+`). The final segment is the
    // key name itself. A `+` that *is* the key (the `plus` key typed literally)
    // is handled by the named-key table, so a lone "+" never reaches the split
    // logic ambiguously: we only treat `+` as a separator when it is not the
    // entire token.
    if token != "+" && token.contains('+') {
        let mut modifiers = KeyModifiers::NONE;
        let parts: Vec<&str> = token.split('+').collect();
        let (key_part, mod_parts) = parts.split_last()?;
        for m in mod_parts {
            match m.to_ascii_lowercase().as_str() {
                "ctrl" => modifiers |= KeyModifiers::CONTROL,
                "shift" => modifiers |= KeyModifiers::SHIFT,
                "alt" => modifiers |= KeyModifiers::ALT,
                _ => return None, // unknown modifier → drop the binding
            }
        }
        let base = parse_bare_key(key_part)?;
        // Fold a `shift` modifier on a char into the uppercase char so it
        // matches the normalized live event (which strips SHIFT for chars).
        return Some(fold_shift_into_char(base, modifiers));
    }

    parse_bare_key(token)
}

/// Parse a bare key name (no modifiers) into a [`KeySpec`].
fn parse_bare_key(name: &str) -> Option<KeySpec> {
    let code = match name {
        "space" => KeyCode::Char(' '),
        "escape" | "esc" => KeyCode::Esc,
        "tab" => KeyCode::Tab,
        "enter" | "return" => KeyCode::Enter,
        "backspace" => KeyCode::Backspace,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        // Named punctuation (Textual key names).
        "plus" => KeyCode::Char('+'),
        "minus" => KeyCode::Char('-'),
        "equal" | "equals" => KeyCode::Char('='),
        "slash" => KeyCode::Char('/'),
        "full_stop" | "period" | "dot" => KeyCode::Char('.'),
        "comma" => KeyCode::Char(','),
        "greater_than_sign" => KeyCode::Char('>'),
        "less_than_sign" => KeyCode::Char('<'),
        "circumflex_accent" => KeyCode::Char('^'),
        "underscore" => KeyCode::Char('_'),
        // A single printable character is the key itself.
        other => {
            let mut chars = other.chars();
            let first = chars.next()?;
            if chars.next().is_some() {
                // More than one char and not a known name → unparseable.
                return None;
            }
            KeyCode::Char(first)
        }
    };
    Some(KeySpec {
        code,
        modifiers: KeyModifiers::NONE,
    })
}

/// Fold an explicit `shift` modifier on a char key into the uppercase char,
/// matching [`KeySpec::from_event`]'s normalization. Non-char keys keep their
/// modifiers verbatim.
fn fold_shift_into_char(spec: KeySpec, modifiers: KeyModifiers) -> KeySpec {
    match spec.code {
        KeyCode::Char(c) if modifiers.contains(KeyModifiers::SHIFT) => {
            let upper = c.to_ascii_uppercase();
            let rest = modifiers & !KeyModifiers::SHIFT;
            KeySpec {
                code: KeyCode::Char(upper),
                modifiers: rest,
            }
        }
        KeyCode::Char(_) => KeySpec {
            code: spec.code,
            // Char keys never carry SHIFT (folded above) — keep CONTROL/ALT.
            modifiers: modifiers & !KeyModifiers::SHIFT,
        },
        _ => KeySpec {
            code: spec.code,
            modifiers,
        },
    }
}

/// A parsed binding value: either a set of single-key alternatives, or a
/// two-key sequence (the `g s` form). Comma-separated alternatives that contain
/// a sequence are not supported (none exist in the contract); the first parse
/// wins per the simple grammar.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Binding {
    /// One or more single-key alternatives (`"n"`, `"plus,equal"`).
    Single(Vec<KeySpec>),
    /// A two-key sequence: press `0`, then `1` (`"g s"`).
    Sequence(KeySpec, KeySpec),
}

/// Parse a keymap *value* string into a [`Binding`].
///
/// * Contains a space → a two-key [`Binding::Sequence`] (only the first two
///   tokens are used; the contract only has 2-key sequences).
/// * Otherwise → comma-separated [`Binding::Single`] alternatives.
///
/// Returns `None` when nothing parses (the whole binding is dropped).
fn parse_binding(value: &str) -> Option<Binding> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if value.contains(' ') {
        let mut tokens = value.split_whitespace();
        let first = parse_key_token(tokens.next()?)?;
        let second = parse_key_token(tokens.next()?)?;
        return Some(Binding::Sequence(first, second));
    }
    let specs: Vec<KeySpec> = value.split(',').filter_map(parse_key_token).collect();
    if specs.is_empty() {
        None
    } else {
        Some(Binding::Single(specs))
    }
}

/// Map a keymap action *name* to the [`Action`] enum, or `None` for an action
/// this UI does not implement (e.g. `seek_forward`, handled elsewhere, or a
/// user-invented name).
fn action_for_name(name: &str) -> Option<Action> {
    Some(match name {
        "quit" => Action::Quit,
        "toggle_pause" => Action::TogglePause,
        "volume_up" => Action::VolumeUp,
        "volume_down" => Action::VolumeDown,
        "next_track" => Action::NextTrack,
        "previous_track" => Action::PreviousTrack,
        "toggle_shuffle" => Action::ToggleShuffle,
        "cycle_repeat" => Action::CycleRepeat,
        "cycle_audio_quality" => Action::CycleAudioQuality,
        "seek_forward" => Action::SeekForward,
        "seek_backward" => Action::SeekBackward,
        "seek_start" => Action::SeekToStart,
        "toggle_mute" => Action::ToggleMute,
        "toggle_like" => Action::ToggleLike,
        "start_radio" => Action::StartRadio,
        "open_current_artist" => Action::OpenCurrentArtist,
        "open_current_album" => Action::OpenCurrentAlbum,
        "go_back" => Action::GoBack,
        "search" => Action::ToggleFilter,
        "switch_home" => Action::SwitchHome,
        "search_page" => Action::SearchPage,
        "switch_library" => Action::SwitchLibrary,
        "switch_queue" => Action::SwitchQueue,
        "switch_history" => Action::SwitchHistory,
        "open_lyrics" => Action::SwitchLyrics,
        "open_action_popup" => Action::OpenActionPopup,
        "open_theme_popup" => Action::OpenThemePopup,
        _ => return None,
    })
}

/// The result of feeding a key into the dispatcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// The key resolved to an action; apply it.
    Action(Action),
    /// The key is the first half of a pending sequence (e.g. `g`). The
    /// dispatcher is now armed for the second key; the UI should show the
    /// prefix in the status line.
    Pending,
    /// The key matched nothing (and cleared any pending prefix).
    None,
}

/// The compiled keymap dispatcher.
///
/// Built once at startup from the merged `action → key` map. Holds:
///
/// * `singles`: a [`KeySpec`] → [`Action`] lookup for direct bindings.
/// * `prefixes`: the set of [`KeySpec`]s that start a two-key sequence, each
///   mapping to a `(second_key → action)` table.
/// * `pending`: the armed first key of a sequence, if any (mutated as keys come
///   in).
#[derive(Debug, Clone)]
pub struct Keymap {
    singles: HashMap<KeySpec, Action>,
    prefixes: HashMap<KeySpec, HashMap<KeySpec, Action>>,
    pending: Option<KeySpec>,
}

// A key that is *both* a sequence prefix and a single binding (e.g. `g` =
// `switch_home` and the prefix of `g s` = `search_page`) is treated
// prefix-first: pressing it arms the sequence rather than firing its single
// action immediately. The single action is preserved as the prefix's fallback
// (see `Keymap::resolve`).
impl Keymap {
    /// Build a dispatcher from a merged `action → key` map (the output of
    /// [`crate::config::load_keymap`]), discarding any parse warnings.
    ///
    /// Convenience wrapper over [`Keymap::from_map_with_warnings`] for callers
    /// (and tests) that do not surface the warnings.
    #[must_use]
    pub fn from_map(map: &HashMap<String, String>) -> Self {
        Self::from_map_with_warnings(map).0
    }

    /// Build a dispatcher from a merged `action → key` map, also returning the
    /// list of bindings that were dropped because their key string did not
    /// parse.
    ///
    /// Later actions cannot clobber an earlier single binding silently: the map
    /// is keyed by `KeySpec`, so the *last* action to claim a given key wins
    /// (matching a `HashMap` insert). Only configured bindings exist — no
    /// hidden aliases (the invented digit shortcuts were removed for Python
    /// parity in M7-fix-2).
    ///
    /// # Warnings
    ///
    /// A warning is collected only when the action name is one this UI handles
    /// *and* its key string is unparseable (a typo like `quit = "notakey"`) — the
    /// kind of mistake the user wants to know about. Each warning is
    /// `"<action> = \"<value>\""`. An *unknown* action name is **not** warned: it
    /// is intentional forward-compatibility (a future action a newer build will
    /// understand), so reporting it would be noise. The returned vector is sorted
    /// for a stable status-line order (the input `HashMap` iterates arbitrarily).
    #[must_use]
    pub fn from_map_with_warnings(map: &HashMap<String, String>) -> (Self, Vec<String>) {
        let mut singles: HashMap<KeySpec, Action> = HashMap::new();
        let mut prefixes: HashMap<KeySpec, HashMap<KeySpec, Action>> = HashMap::new();
        let mut warnings: Vec<String> = Vec::new();

        for (name, value) in map {
            let Some(action) = action_for_name(name) else {
                continue; // action this UI does not handle → forward-compat, no warning
            };
            let Some(binding) = parse_binding(value) else {
                // A known action with an unparseable key string is a real user
                // typo worth surfacing (the binding is dropped — that action is
                // left unbound).
                warnings.push(format!("{name} = {value:?}"));
                continue;
            };
            match binding {
                Binding::Single(specs) => {
                    for spec in specs {
                        singles.insert(spec, action);
                    }
                }
                Binding::Sequence(first, second) => {
                    prefixes.entry(first).or_default().insert(second, action);
                }
            }
        }
        warnings.sort();

        (
            Self {
                singles,
                prefixes,
                pending: None,
            },
            warnings,
        )
    }

    /// Build a dispatcher from the default keymap only (no *user* config read).
    ///
    /// Layers the hard-coded [`crate::config::DEFAULT_KEYMAP`] and the bundled
    /// `default_keymap.toml` (which carries the actions absent from the static
    /// table — `switch_history`, the seek/mute keys, etc.) exactly as
    /// [`crate::config::load_keymap`] does, but skips the user file by pointing
    /// at a path that cannot exist. Adds the `search_page = "g s"` default the
    /// directive grants now that sequences are supported.
    ///
    /// Falls back to the static-only map if `load_keymap` somehow errors (it
    /// only errors on malformed embedded TOML, which the tests guard against).
    #[must_use]
    pub fn defaults() -> Self {
        // A path under an impossible directory so no user keymap.toml is read.
        let no_user = Path::new("/nonexistent-ytmusic-tui/keymap.toml");
        let mut map = config::load_keymap(Some(no_user), None).unwrap_or_else(|_| {
            config::DEFAULT_KEYMAP
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect()
        });
        // search_page is unbound by default; grant it the sequence binding now
        // that the dispatcher supports sequences (directive §6).
        map.entry("search_page".to_owned())
            .or_insert_with(|| "g s".to_owned());
        Self::from_map(&map)
    }

    /// Whether a sequence prefix is currently armed (for the status line).
    #[must_use]
    pub fn pending(&self) -> Option<KeySpec> {
        self.pending
    }

    /// The human-readable label of the armed prefix, for the status line
    /// (e.g. `"g"`). `None` when no prefix is pending.
    #[must_use]
    pub fn pending_label(&self) -> Option<String> {
        self.pending.map(|spec| key_spec_label(&spec))
    }

    /// Cancel any armed sequence prefix (e.g. when a popup opens or focus
    /// changes). Idempotent.
    pub fn clear_pending(&mut self) {
        self.pending = None;
    }

    /// Feed a key event into the dispatcher and resolve it.
    ///
    /// Sequence handling:
    ///
    /// * If a prefix is armed, the incoming key is looked up in that prefix's
    ///   second-key table. A hit resolves to the action (and disarms); a miss
    ///   disarms and falls through to a fresh single-key lookup so the key is
    ///   not swallowed (mirrors spotify_player: a non-matching key cancels the
    ///   sequence but still acts on its own where bound).
    /// * Otherwise the key is looked up as a single binding; if it is instead a
    ///   known prefix, the dispatcher arms it and returns [`Resolution::Pending`].
    pub fn resolve(&mut self, event: KeyEvent) -> Resolution {
        let spec = KeySpec::from_event(event);

        if let Some(prefix) = self.pending.take() {
            // 1. The armed prefix completes a sequence → fire it.
            if let Some(table) = self.prefixes.get(&prefix)
                && let Some(action) = table.get(&spec)
            {
                return Resolution::Action(*action);
            }
            // 2. Sequence cancelled. If the second key is itself bound as a
            //    single, that single still fires (the key is not swallowed).
            if let Some(action) = self.singles.get(&spec) {
                return Resolution::Action(*action);
            }
            // 3. Otherwise apply the *prefix's own* single action as a fallback,
            //    so a key that is both prefix and single (e.g. `g` = home) still
            //    works when the follow-up doesn't extend a sequence. This is why
            //    a lone `g` then an unbound key still goes home.
            if let Some(action) = self.singles.get(&prefix) {
                return Resolution::Action(*action);
            }
            return Resolution::None;
        }

        // No prefix armed. A key that starts a sequence arms it *first*, even if
        // it is also a single binding (prefix-first; the single is the fallback
        // resolved above when the sequence is cancelled).
        if self.prefixes.contains_key(&spec) {
            self.pending = Some(spec);
            return Resolution::Pending;
        }
        if let Some(action) = self.singles.get(&spec) {
            return Resolution::Action(*action);
        }
        Resolution::None
    }
}

/// A short human label for a [`KeySpec`], used in the pending-prefix status hint.
fn key_spec_label(spec: &KeySpec) -> String {
    let base = match spec.code {
        KeyCode::Char(' ') => "space".to_owned(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Esc => "esc".to_owned(),
        KeyCode::Tab => "tab".to_owned(),
        KeyCode::Enter => "enter".to_owned(),
        other => format!("{other:?}"),
    };
    if spec.modifiers.contains(KeyModifiers::CONTROL) {
        format!("ctrl+{base}")
    } else {
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ch(c: char) -> KeyEvent {
        key(KeyCode::Char(c))
    }

    // -- key-string parsing ------------------------------------------------

    #[test]
    fn parses_single_char() {
        let spec = parse_key_token("n").unwrap();
        assert_eq!(spec.code, KeyCode::Char('n'));
        assert_eq!(spec.modifiers, KeyModifiers::NONE);
    }

    #[test]
    fn parses_named_special_keys() {
        assert_eq!(parse_bare_key("space").unwrap().code, KeyCode::Char(' '));
        assert_eq!(parse_bare_key("escape").unwrap().code, KeyCode::Esc);
        assert_eq!(parse_bare_key("slash").unwrap().code, KeyCode::Char('/'));
        assert_eq!(parse_bare_key("plus").unwrap().code, KeyCode::Char('+'));
        assert_eq!(parse_bare_key("minus").unwrap().code, KeyCode::Char('-'));
        assert_eq!(parse_bare_key("equal").unwrap().code, KeyCode::Char('='));
        assert_eq!(
            parse_bare_key("full_stop").unwrap().code,
            KeyCode::Char('.')
        );
        assert_eq!(
            parse_bare_key("greater_than_sign").unwrap().code,
            KeyCode::Char('>')
        );
        assert_eq!(
            parse_bare_key("underscore").unwrap().code,
            KeyCode::Char('_')
        );
        assert_eq!(
            parse_bare_key("circumflex_accent").unwrap().code,
            KeyCode::Char('^')
        );
    }

    #[test]
    fn parses_ctrl_modifier() {
        let spec = parse_key_token("ctrl+s").unwrap();
        assert_eq!(spec.code, KeyCode::Char('s'));
        assert!(spec.modifiers.contains(KeyModifiers::CONTROL));
    }

    #[test]
    fn shift_folds_into_uppercase_char() {
        // "shift+q" normalizes to the uppercase char with no SHIFT modifier,
        // matching the live-event normalization.
        let spec = parse_key_token("shift+q").unwrap();
        assert_eq!(spec.code, KeyCode::Char('Q'));
        assert!(!spec.modifiers.contains(KeyModifiers::SHIFT));
    }

    #[test]
    fn unparseable_token_is_none() {
        assert!(parse_key_token("notakey").is_none());
        assert!(parse_key_token("ctrl+notakey").is_none());
        assert!(parse_key_token("").is_none());
    }

    #[test]
    fn parses_comma_alternatives() {
        match parse_binding("plus,equal").unwrap() {
            Binding::Single(specs) => {
                assert_eq!(specs.len(), 2);
                assert_eq!(specs[0].code, KeyCode::Char('+'));
                assert_eq!(specs[1].code, KeyCode::Char('='));
            }
            other => panic!("expected Single, got {other:?}"),
        }
    }

    #[test]
    fn parses_sequence() {
        match parse_binding("g s").unwrap() {
            Binding::Sequence(a, b) => {
                assert_eq!(a.code, KeyCode::Char('g'));
                assert_eq!(b.code, KeyCode::Char('s'));
            }
            other => panic!("expected Sequence, got {other:?}"),
        }
    }

    // -- event normalization -----------------------------------------------

    #[test]
    fn from_event_strips_shift_on_char() {
        let event = KeyEvent::new(KeyCode::Char('Q'), KeyModifiers::SHIFT);
        let spec = KeySpec::from_event(event);
        assert_eq!(spec.code, KeyCode::Char('Q'));
        assert!(!spec.modifiers.contains(KeyModifiers::SHIFT));
    }

    #[test]
    fn from_event_keeps_ctrl() {
        let event = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let spec = KeySpec::from_event(event);
        assert!(spec.modifiers.contains(KeyModifiers::CONTROL));
    }

    // -- dispatcher: default bindings resolve ------------------------------

    #[test]
    fn defaults_resolve_core_actions() {
        let mut km = Keymap::defaults();
        assert_eq!(km.resolve(ch('Q')), Resolution::Action(Action::Quit));
        assert_eq!(km.resolve(ch(' ')), Resolution::Action(Action::TogglePause));
        assert_eq!(km.resolve(ch('n')), Resolution::Action(Action::NextTrack));
        assert_eq!(
            km.resolve(ch('p')),
            Resolution::Action(Action::PreviousTrack)
        );
        assert_eq!(
            km.resolve(ch('s')),
            Resolution::Action(Action::ToggleShuffle)
        );
        assert_eq!(km.resolve(ch('r')), Resolution::Action(Action::CycleRepeat));
        assert_eq!(
            km.resolve(ch('b')),
            Resolution::Action(Action::CycleAudioQuality)
        );
        assert_eq!(
            km.resolve(ch('l')),
            Resolution::Action(Action::SwitchLibrary)
        );
        assert_eq!(km.resolve(ch('q')), Resolution::Action(Action::SwitchQueue));
        assert_eq!(
            km.resolve(ch('H')),
            Resolution::Action(Action::SwitchHistory)
        );
        assert_eq!(
            km.resolve(ch('L')),
            Resolution::Action(Action::SwitchLyrics)
        );
        assert_eq!(
            km.resolve(ch('.')),
            Resolution::Action(Action::OpenActionPopup)
        );
        assert_eq!(
            km.resolve(ch('T')),
            Resolution::Action(Action::OpenThemePopup)
        );
    }

    #[test]
    fn slash_resolves_to_toggle_filter() {
        // The M5c reclaim: `/` is now the filter toggle, not the search view.
        let mut km = Keymap::defaults();
        assert_eq!(
            km.resolve(ch('/')),
            Resolution::Action(Action::ToggleFilter)
        );
    }

    #[test]
    fn volume_alternatives_both_resolve() {
        let mut km = Keymap::defaults();
        assert_eq!(km.resolve(ch('+')), Resolution::Action(Action::VolumeUp));
        assert_eq!(km.resolve(ch('=')), Resolution::Action(Action::VolumeUp));
        assert_eq!(km.resolve(ch('-')), Resolution::Action(Action::VolumeDown));
    }

    #[test]
    fn escape_resolves_to_go_back() {
        let mut km = Keymap::defaults();
        assert_eq!(
            km.resolve(key(KeyCode::Esc)),
            Resolution::Action(Action::GoBack)
        );
    }

    #[test]
    fn switch_home_g_is_also_a_prefix() {
        // `g` is both `switch_home` and the prefix of `g s` (search_page). The
        // bare `g` arms the prefix (Pending); only a following non-`s` falls
        // back. spotify_player's `g` is the home/prefix key likewise.
        let mut km = Keymap::defaults();
        assert_eq!(km.resolve(ch('g')), Resolution::Pending);
        assert!(km.pending().is_some());
    }

    // -- sequences ---------------------------------------------------------

    #[test]
    fn g_then_s_resolves_to_search_page() {
        let mut km = Keymap::defaults();
        assert_eq!(km.resolve(ch('g')), Resolution::Pending);
        assert_eq!(km.resolve(ch('s')), Resolution::Action(Action::SearchPage));
        assert!(km.pending().is_none(), "prefix disarmed after sequence");
    }

    #[test]
    fn g_then_unrelated_key_cancels_and_falls_through() {
        // After `g`, a non-`s` key cancels the sequence. If that key is itself
        // bound it still acts (here `n` → NextTrack).
        let mut km = Keymap::defaults();
        assert_eq!(km.resolve(ch('g')), Resolution::Pending);
        assert_eq!(km.resolve(ch('n')), Resolution::Action(Action::NextTrack));
        assert!(km.pending().is_none());
    }

    #[test]
    fn g_then_unbound_key_falls_back_to_prefix_single() {
        // `g` is both `switch_home` and the `g s` prefix. After `g`, an unbound
        // key (`z`) cancels the sequence and falls back to `g`'s own single
        // action, so a lone `g` still reaches Home.
        let mut km = Keymap::defaults();
        assert_eq!(km.resolve(ch('g')), Resolution::Pending);
        assert_eq!(km.resolve(ch('z')), Resolution::Action(Action::SwitchHome));
        assert!(km.pending().is_none());
    }

    #[test]
    fn pure_prefix_then_unbound_key_is_none() {
        // A prefix that is NOT also a single binding has no fallback: rebinding
        // search_page to a `x y` sequence makes `x` a pure prefix, so `x` then
        // an unbound key resolves to None.
        let mut map: HashMap<String, String> = config::DEFAULT_KEYMAP
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        map.insert("search_page".to_owned(), "x y".to_owned());
        let mut km = Keymap::from_map(&map);
        assert_eq!(km.resolve(ch('x')), Resolution::Pending);
        assert_eq!(km.resolve(ch('z')), Resolution::None);
        assert!(km.pending().is_none());
    }

    #[test]
    fn clear_pending_disarms() {
        let mut km = Keymap::defaults();
        let _ = km.resolve(ch('g'));
        assert!(km.pending().is_some());
        km.clear_pending();
        assert!(km.pending().is_none());
    }

    #[test]
    fn pending_label_shows_prefix() {
        let mut km = Keymap::defaults();
        let _ = km.resolve(ch('g'));
        assert_eq!(km.pending_label().as_deref(), Some("g"));
    }

    #[test]
    fn unbound_key_is_none() {
        let mut km = Keymap::defaults();
        assert_eq!(km.resolve(ch('z')), Resolution::None);
        assert_eq!(km.resolve(ch('x')), Resolution::None);
    }

    // -- user rebinding via the merged map ---------------------------------

    #[test]
    fn rebinding_via_map_takes_effect() {
        // A user map that rebinds quit to ctrl+q and search_page to a sequence.
        let mut map: HashMap<String, String> = config::DEFAULT_KEYMAP
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        map.insert("quit".to_owned(), "ctrl+q".to_owned());
        map.insert("search_page".to_owned(), "g f".to_owned());
        let mut km = Keymap::from_map(&map);

        // ctrl+q now quits; bare Q no longer does (it is unbound).
        assert_eq!(
            km.resolve(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)),
            Resolution::Action(Action::Quit)
        );

        // g f now reaches search_page.
        assert_eq!(km.resolve(ch('g')), Resolution::Pending);
        assert_eq!(km.resolve(ch('f')), Resolution::Action(Action::SearchPage));
    }

    #[test]
    fn unknown_action_name_is_ignored() {
        let mut map: HashMap<String, String> = HashMap::new();
        map.insert("totally_made_up".to_owned(), "z".to_owned());
        map.insert("quit".to_owned(), "Q".to_owned());
        let mut km = Keymap::from_map(&map);
        // The made-up action's key does nothing; quit still works.
        assert_eq!(km.resolve(ch('z')), Resolution::None);
        assert_eq!(km.resolve(ch('Q')), Resolution::Action(Action::Quit));
    }

    #[test]
    fn unparseable_binding_is_dropped_not_fatal() {
        let mut map: HashMap<String, String> = HashMap::new();
        map.insert("quit".to_owned(), "notarealkey".to_owned());
        map.insert("next_track".to_owned(), "n".to_owned());
        let mut km = Keymap::from_map(&map);
        // quit's binding was dropped; next_track still parses.
        assert_eq!(km.resolve(ch('n')), Resolution::Action(Action::NextTrack));
    }

    // -- parse warnings (Stage 4) ------------------------------------------

    #[test]
    fn unparseable_known_action_binding_is_warned() {
        // A known action with a bad key string is collected as a warning (a typo
        // the user wants to know about) while the rest of the keymap still works.
        let mut map: HashMap<String, String> = HashMap::new();
        map.insert("quit".to_owned(), "notarealkey".to_owned());
        map.insert("next_track".to_owned(), "n".to_owned());
        let (km, warnings) = Keymap::from_map_with_warnings(&map);
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("quit") && warnings[0].contains("notarealkey"),
            "warning should name the action and value: {warnings:?}"
        );
        // next_track is unaffected.
        let mut km = km;
        assert_eq!(km.resolve(ch('n')), Resolution::Action(Action::NextTrack));
    }

    #[test]
    fn unknown_action_name_is_not_warned() {
        // Forward-compat: a novel action name is silently skipped, not warned —
        // it is intentional, not a typo.
        let mut map: HashMap<String, String> = HashMap::new();
        map.insert("future_action".to_owned(), "z".to_owned());
        map.insert("quit".to_owned(), "Q".to_owned());
        let (_km, warnings) = Keymap::from_map_with_warnings(&map);
        assert!(
            warnings.is_empty(),
            "unknown action names must not warn: {warnings:?}"
        );
    }

    #[test]
    fn valid_keymap_yields_no_warnings() {
        let (_km, warnings) = Keymap::from_map_with_warnings(
            &config::DEFAULT_KEYMAP
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        );
        assert!(warnings.is_empty(), "default keymap parses cleanly");
    }

    #[test]
    fn multiple_warnings_are_sorted() {
        let mut map: HashMap<String, String> = HashMap::new();
        map.insert("quit".to_owned(), "??".to_owned());
        map.insert("next_track".to_owned(), "@@".to_owned());
        let (_km, warnings) = Keymap::from_map_with_warnings(&map);
        assert_eq!(warnings.len(), 2);
        // Sorted for a stable status-line order: "next_track" < "quit".
        assert!(warnings[0].starts_with("next_track"));
        assert!(warnings[1].starts_with("quit"));
    }
}
