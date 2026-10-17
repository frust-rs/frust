//! Keyboard shortcuts: the [`Chord`] spec and the per-paint declaration table behind
//! [`PaintCtx::register_shortcut`](crate::widget::PaintCtx::register_shortcut).
//!
//! A widget declares a [`ShortcutDecl`] from its own `paint`, exactly like an overlay entry: the
//! root clears the table every paint, so a declarer that stops declaring (or unmounts) stops
//! matching after the next paint, with nothing to unregister. A declaration made while the root
//! paints a floated pod is stamped with that pod's [`OverlayKey`].
//!
//! A `Key` the focus path left unhandled is matched against the last paint's table in tier
//! order — declarations stamped with the topmost interactive overlay, then main-tree
//! [`ShortcutScope::Local`], then [`ShortcutScope::AppGlobal`] — newest declaration first within
//! a tier. Each candidate gets an [`InputEvent::Shortcut`](crate::event::InputEvent::Shortcut)
//! broadcast (wrapped in an overlay event for a stamped one); the declarer answers `Handled`, and
//! an unanswered delivery falls through to the next candidate.

use std::cell::Cell;
use std::fmt;

use crate::event::{Key, KeyEvent, Modifiers, NamedKey, PassBracket};
use crate::overlay::OverlayKey;

thread_local! {
    /// The next [`ShortcutId::next`] value; starts at `1` so `0` is never handed out.
    static NEXT_SHORTCUT_ID: Cell<u64> = const { Cell::new(1) };

    /// Declarations made so far in this thread's open paint pass, in paint order.
    static SHORTCUT_TABLE: Cell<Vec<ShortcutEntry>> = const { Cell::new(Vec::new()) };

    static SHORTCUT_PASS_OPEN: Cell<bool> = const { Cell::new(false) };

    /// The floated pod the root is painting, stamped onto every declaration it makes.
    static CURRENT_OVERLAY: Cell<Option<OverlayKey>> = const { Cell::new(None) };

    /// Raised when a widget answers a shortcut delivery `Handled` (containers drop the result).
    static SHORTCUT_HANDLED: Cell<bool> = const { Cell::new(false) };
}

/// Whether `Mod` means Command (Apple targets) or Ctrl (everything else, `wasm32` included).
const MOD_IS_META: bool = cfg!(any(target_os = "macos", target_os = "ios"));

/// A shortcut's identity, owned by its declarer and quoted back by its delivery.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ShortcutId(u64);

impl ShortcutId {
    /// Allocate an id unique on this thread. Call once when the declarer is built.
    pub fn next() -> Self {
        ShortcutId(NEXT_SHORTCUT_ID.with(|next| {
            let id = next.get();
            next.set(id.wrapping_add(1));
            id
        }))
    }

    /// The raw integer, for map keys; allocation order is not a contract.
    pub fn value(self) -> u64 {
        self.0
    }
}

/// Which tier a main-tree declaration joins; an overlay-stamped one is always tried first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShortcutScope {
    /// Tried before every app-global declaration.
    Local,
    /// The last tier: whatever no overlay or local declaration answered.
    AppGlobal,
}

/// A key plus the exact modifier set it must be pressed with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chord {
    /// A character key is stored lowercased and matched case-insensitively.
    pub key: Key,
    pub mods: Modifiers,
}

/// Why [`Chord::parse`] refused a spec.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChordParseError {
    /// The spec was empty or whitespace.
    Empty,
    /// A token that is neither a modifier nor a key name.
    UnknownToken(String),
    /// A key the input vocabulary cannot deliver (function keys: `NamedKey` has none).
    UnsupportedKey(String),
    /// Only modifiers, or an empty token between two `+`.
    MissingKey,
    /// More than one non-modifier token.
    MultipleKeys,
}

impl fmt::Display for ChordParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChordParseError::Empty => f.write_str("empty chord"),
            ChordParseError::UnknownToken(token) => write!(f, "unknown chord token `{token}`"),
            ChordParseError::UnsupportedKey(token) => {
                write!(f, "key `{token}` is not in the input vocabulary")
            }
            ChordParseError::MissingKey => f.write_str("chord names no key"),
            ChordParseError::MultipleKeys => f.write_str("chord names more than one key"),
        }
    }
}

impl std::error::Error for ChordParseError {}

impl Chord {
    /// A chord from its parts; a character key is lowercased so it matches either case.
    pub fn new(key: Key, mods: Modifiers) -> Self {
        let key = match key {
            Key::Character(text) => Key::Character(text.to_lowercase()),
            named => named,
        };
        Chord { key, mods }
    }

    /// Parse `"Mod+K"`, `"Shift+Tab"`, `"Ctrl+Shift+P"`, `"1"`: `+`-separated, case-insensitive
    /// modifiers then one key. `Mod` is Command on macOS/iOS and Ctrl elsewhere, chosen at compile
    /// time, so a web build running on a Mac maps it to Ctrl.
    pub fn parse(spec: &str) -> Result<Chord, ChordParseError> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err(ChordParseError::Empty);
        }
        // A trailing `++` names the plus key itself.
        let (head, plus) = match spec.strip_suffix("++") {
            Some(head) => (head, true),
            None if spec == "+" => ("", true),
            None => (spec, false),
        };
        let mut mods = Modifiers::default();
        let mut key = plus.then(|| Key::Character("+".to_string()));
        let tokens = (!head.is_empty()).then(|| head.split('+'));
        for token in tokens.into_iter().flatten() {
            let token = token.trim();
            if token.is_empty() {
                return Err(ChordParseError::MissingKey);
            }
            if apply_modifier(&mut mods, token) {
                continue;
            }
            let parsed = parse_key(token)?;
            if key.replace(parsed).is_some() {
                return Err(ChordParseError::MultipleKeys);
            }
        }
        let key = key.ok_or(ChordParseError::MissingKey)?;
        Ok(Chord::new(key, mods))
    }

    /// Whether `event` presses this chord: the same key (characters case-insensitively) and
    /// exactly the same modifiers. An auto-repeat matches like a fresh press.
    pub fn matches(&self, event: &KeyEvent) -> bool {
        if event.modifiers != self.mods {
            return false;
        }
        match (&self.key, &event.key) {
            (Key::Named(want), Key::Named(got)) => want == got,
            (Key::Character(want), Key::Character(got)) => *want == got.to_lowercase(),
            _ => false,
        }
    }
}

/// Set the modifier `token` names and return `true`, or `false` when it names none.
fn apply_modifier(mods: &mut Modifiers, token: &str) -> bool {
    match token.to_ascii_lowercase().as_str() {
        "mod" if MOD_IS_META => mods.meta = true,
        "mod" => mods.ctrl = true,
        "ctrl" | "control" => mods.ctrl = true,
        "shift" => mods.shift = true,
        "alt" | "option" | "opt" => mods.alt = true,
        "meta" | "cmd" | "command" | "super" | "win" => mods.meta = true,
        _ => return false,
    }
    true
}

/// The key one non-modifier token names.
fn parse_key(token: &str) -> Result<Key, ChordParseError> {
    if token.chars().count() == 1 {
        return Ok(Key::Character(token.to_lowercase()));
    }
    let lower = token.to_ascii_lowercase();
    let named = match lower.as_str() {
        "enter" | "return" => NamedKey::Enter,
        "backspace" => NamedKey::Backspace,
        "delete" | "del" => NamedKey::Delete,
        "left" | "arrowleft" => NamedKey::ArrowLeft,
        "right" | "arrowright" => NamedKey::ArrowRight,
        "up" | "arrowup" => NamedKey::ArrowUp,
        "down" | "arrowdown" => NamedKey::ArrowDown,
        "home" => NamedKey::Home,
        "end" => NamedKey::End,
        "escape" | "esc" => NamedKey::Escape,
        "tab" => NamedKey::Tab,
        "insert" | "ins" => NamedKey::Insert,
        "copy" => NamedKey::Copy,
        "cut" => NamedKey::Cut,
        "paste" => NamedKey::Paste,
        "space" => return Ok(Key::Character(" ".to_string())),
        _ if is_function_key(&lower) => {
            return Err(ChordParseError::UnsupportedKey(token.to_string()));
        }
        _ => return Err(ChordParseError::UnknownToken(token.to_string())),
    };
    Ok(Key::Named(named))
}

/// `f1`..`f24`.
fn is_function_key(lower: &str) -> bool {
    lower
        .strip_prefix('f')
        .and_then(|n| n.parse::<u8>().ok())
        .is_some_and(|n| (1..=24).contains(&n))
}

/// One shortcut a widget declares for the paint pass in progress.
#[derive(Clone, Debug, PartialEq)]
pub struct ShortcutDecl {
    pub id: ShortcutId,
    pub chord: Chord,
    /// What a hint surface shows for it.
    pub label: String,
    pub scope: ShortcutScope,
}

/// A declaration as the root keeps it: stamped with the floated pod that declared it, if any.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ShortcutEntry {
    pub(crate) decl: ShortcutDecl,
    pub(crate) overlay: Option<OverlayKey>,
}

/// The [`PassBracket`] around one declaration pass: enter clears, `take` drains.
pub(crate) struct ShortcutPaintPass(PassBracket<Vec<ShortcutEntry>>);

impl ShortcutPaintPass {
    pub(crate) fn enter() -> Self {
        ShortcutPaintPass(PassBracket::enter(&SHORTCUT_TABLE, &SHORTCUT_PASS_OPEN))
    }

    pub(crate) fn take(&self) -> Vec<ShortcutEntry> {
        self.0.take()
    }
}

/// Record `decl` in this thread's open pass, stamped with the pod being painted.
pub(crate) fn register(decl: ShortcutDecl) {
    let overlay = CURRENT_OVERLAY.with(Cell::get);
    SHORTCUT_TABLE.with(|table| {
        let mut entries = table.take();
        entries.push(ShortcutEntry { decl, overlay });
        table.set(entries);
    });
}

/// Stamps declarations with `key` while held; restores the enclosing stamp on drop.
pub(crate) struct OverlayStamp(Option<OverlayKey>);

impl OverlayStamp {
    pub(crate) fn enter(key: OverlayKey) -> Self {
        OverlayStamp(CURRENT_OVERLAY.with(|slot| slot.replace(Some(key))))
    }
}

impl Drop for OverlayStamp {
    fn drop(&mut self) {
        CURRENT_OVERLAY.with(|slot| slot.set(self.0));
    }
}

/// Note that a widget answered a shortcut delivery `Handled` (behind `ChildPod`'s dispatch).
pub(crate) fn note_handled() {
    SHORTCUT_HANDLED.with(|flag| flag.set(true));
}

/// Brackets one delivery's handled flag: enter lowers it, drop restores the enclosing value.
pub(crate) struct DeliveryProbe(bool);

impl DeliveryProbe {
    pub(crate) fn enter() -> Self {
        DeliveryProbe(SHORTCUT_HANDLED.with(|flag| flag.replace(false)))
    }

    /// Whether a widget answered this delivery.
    pub(crate) fn handled(&self) -> bool {
        SHORTCUT_HANDLED.with(Cell::get)
    }
}

impl Drop for DeliveryProbe {
    fn drop(&mut self) {
        SHORTCUT_HANDLED.with(|flag| flag.set(self.0));
    }
}

/// The last paint's declarations, in paint order: the snapshot `RenderRoot` matches keys against.
#[derive(Clone, Debug, Default)]
pub(crate) struct ShortcutTable {
    entries: Vec<ShortcutEntry>,
}

impl ShortcutTable {
    pub(crate) fn new(entries: Vec<ShortcutEntry>) -> Self {
        Self { entries }
    }

    #[cfg(test)]
    pub(crate) fn entries(&self) -> &[ShortcutEntry] {
        &self.entries
    }

    /// The declarations `event` presses, as `(id, overlay stamp)` in delivery order: the
    /// `top_overlay` pod's, then main-tree `Local`, then main-tree `AppGlobal`, newest first in
    /// each. Declarations from any other floated pod are unreachable and left out.
    pub(crate) fn candidates(
        &self,
        event: &KeyEvent,
        top_overlay: Option<OverlayKey>,
    ) -> Vec<(ShortcutId, Option<OverlayKey>)> {
        let tiers: [&dyn Fn(&ShortcutEntry) -> bool; 3] = [
            &|e| e.overlay.is_some() && e.overlay == top_overlay,
            &|e| e.overlay.is_none() && e.decl.scope == ShortcutScope::Local,
            &|e| e.overlay.is_none() && e.decl.scope == ShortcutScope::AppGlobal,
        ];
        let mut out = Vec::new();
        for in_tier in tiers {
            let tier: Vec<_> = self
                .entries
                .iter()
                .rev()
                .filter(|e| in_tier(e) && e.decl.chord.matches(event))
                .map(|e| (e.decl.id, e.overlay))
                .collect();
            #[cfg(debug_assertions)]
            if tier.len() > 1 {
                log::warn!(
                    "frust-core: {} shortcuts in one tier match {:?}; the last declared wins",
                    tier.len(),
                    event.key
                );
            }
            out.extend(tier);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widget::PaintCtx;
    use kurbo::{Point, Size};

    fn mods(shift: bool, ctrl: bool, alt: bool, meta: bool) -> Modifiers {
        Modifiers {
            shift,
            ctrl,
            alt,
            meta,
        }
    }

    fn press(key: Key, modifiers: Modifiers) -> KeyEvent {
        KeyEvent {
            key,
            modifiers,
            repeat: false,
        }
    }

    fn character(text: &str) -> Key {
        Key::Character(text.to_string())
    }

    fn decl(chord: &str, scope: ShortcutScope) -> ShortcutDecl {
        ShortcutDecl {
            id: ShortcutId::next(),
            chord: Chord::parse(chord).expect("valid chord"),
            label: chord.to_string(),
            scope,
        }
    }

    #[test]
    fn mod_maps_to_meta_on_apple_targets_and_ctrl_elsewhere() {
        let chord = Chord::parse("Mod+K").unwrap();
        let apple = cfg!(any(target_os = "macos", target_os = "ios"));
        assert_eq!(chord.mods, mods(false, !apple, false, apple));
        assert_eq!(chord.key, character("k"));
    }

    #[test]
    fn parses_named_keys_modifier_stacks_and_bare_characters() {
        assert_eq!(
            Chord::parse("Shift+Tab").unwrap(),
            Chord::new(Key::Named(NamedKey::Tab), mods(true, false, false, false))
        );
        assert_eq!(
            Chord::parse("Ctrl+Shift+P").unwrap(),
            Chord::new(character("p"), mods(true, true, false, false))
        );
        assert_eq!(
            Chord::parse("1").unwrap(),
            Chord::new(character("1"), Modifiers::default())
        );
        assert_eq!(
            Chord::parse(" cmd + alt + Esc ").unwrap(),
            Chord::new(Key::Named(NamedKey::Escape), mods(false, false, true, true))
        );
        assert_eq!(
            Chord::parse("Ctrl++").unwrap(),
            Chord::new(character("+"), mods(false, true, false, false))
        );
        assert_eq!(Chord::parse("Space").unwrap().key, character(" "));
    }

    #[test]
    fn unknown_tokens_and_malformed_specs_are_typed_errors() {
        assert_eq!(
            Chord::parse("Hyper+K"),
            Err(ChordParseError::UnknownToken("Hyper".to_string()))
        );
        assert_eq!(
            Chord::parse("Ctrl+Banana"),
            Err(ChordParseError::UnknownToken("Banana".to_string()))
        );
        assert_eq!(
            Chord::parse("F2"),
            Err(ChordParseError::UnsupportedKey("F2".to_string()))
        );
        assert_eq!(Chord::parse(""), Err(ChordParseError::Empty));
        assert_eq!(Chord::parse("Shift"), Err(ChordParseError::MissingKey));
        assert_eq!(Chord::parse("Ctrl+"), Err(ChordParseError::MissingKey));
        assert_eq!(Chord::parse("K+J"), Err(ChordParseError::MultipleKeys));
        let message = Chord::parse("Hyper+K").unwrap_err().to_string();
        assert!(message.contains("Hyper"), "{message}");
    }

    #[test]
    fn matching_is_exact_on_modifiers_case_blind_on_characters_and_ignores_repeat() {
        let chord = Chord::parse("Ctrl+K").unwrap();
        let ctrl = mods(false, true, false, false);
        assert!(chord.matches(&press(character("k"), ctrl)));
        assert!(chord.matches(&press(character("K"), ctrl)));
        let mut held = press(character("k"), ctrl);
        held.repeat = true;
        assert!(chord.matches(&held), "an auto-repeat matches too");
        assert!(!chord.matches(&press(character("k"), Modifiers::default())));
        assert!(!chord.matches(&press(character("k"), mods(true, true, false, false))));
        assert!(!chord.matches(&press(character("j"), ctrl)));
        assert!(!chord.matches(&press(Key::Named(NamedKey::Tab), ctrl)));
    }

    #[test]
    fn candidates_follow_tier_order_newest_first() {
        let (top, other) = (OverlayKey::next(), OverlayKey::next());
        let global = decl("Ctrl+K", ShortcutScope::AppGlobal);
        let local_old = decl("Ctrl+K", ShortcutScope::Local);
        let local_new = decl("Ctrl+K", ShortcutScope::Local);
        let in_top = decl("Ctrl+K", ShortcutScope::AppGlobal);
        let in_other = decl("Ctrl+K", ShortcutScope::Local);
        let unrelated = decl("Ctrl+J", ShortcutScope::Local);
        let stamped = |d: &ShortcutDecl, overlay| ShortcutEntry {
            decl: d.clone(),
            overlay,
        };
        let table = ShortcutTable::new(vec![
            stamped(&global, None),
            stamped(&local_old, None),
            stamped(&in_top, Some(top)),
            stamped(&local_new, None),
            stamped(&in_other, Some(other)),
            stamped(&unrelated, None),
        ]);
        let event = press(character("k"), mods(false, true, false, false));
        assert_eq!(
            table.candidates(&event, Some(top)),
            vec![
                (in_top.id, Some(top)),
                (local_new.id, None),
                (local_old.id, None),
                (global.id, None),
            ],
            "overlay tier, then Local (last declared first), then AppGlobal"
        );
        assert_eq!(
            table.candidates(&event, None).len(),
            3,
            "with no interactive pod on top, no stamped declaration is reachable"
        );
    }

    #[test]
    fn the_table_is_pass_scoped_and_stamps_the_pod_being_painted() {
        let pass = ShortcutPaintPass::enter();
        let (plain, floated) = (
            decl("1", ShortcutScope::Local),
            decl("2", ShortcutScope::Local),
        );
        let key = OverlayKey::next();
        register(plain.clone());
        {
            let _stamp = OverlayStamp::enter(key);
            register(floated.clone());
        }
        let drained = pass.take();
        assert_eq!(
            drained.iter().map(|e| e.overlay).collect::<Vec<_>>(),
            vec![None, Some(key)]
        );
        assert!(pass.take().is_empty(), "the drain empties the table");
        drop(pass);
        register(plain);
        let next = ShortcutPaintPass::enter();
        assert!(next.take().is_empty(), "a stray declaration never leaks in");
    }

    #[test]
    fn declaring_counts_as_a_paint_publication() {
        let _pass = ShortcutPaintPass::enter();
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        assert_eq!(ctx.paint_publications(), 0);
        ctx.register_shortcut(decl("Mod+S", ShortcutScope::AppGlobal));
        assert_eq!(
            ctx.paint_publications(),
            1,
            "a retained boundary must refuse to replay a declaring body"
        );
    }

    #[test]
    fn a_delivery_probe_brackets_the_handled_flag() {
        let outer = DeliveryProbe::enter();
        note_handled();
        {
            let inner = DeliveryProbe::enter();
            assert!(!inner.handled(), "a nested delivery starts unanswered");
        }
        assert!(outer.handled(), "and hands the enclosing answer back");
        drop(outer);
        assert!(!DeliveryProbe::enter().handled());
    }
}
