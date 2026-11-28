//! The keys `RenderRoot` was told are down, and the release each one owes: an
//! [`InputEvent::KeyUp`] down the focus path, then an [`InputEvent::ShortcutReleased`] for the
//! shortcut that answered the press, addressed by its recorded id rather than a table re-match.
//! Only a non-repeat press opens an entry, because a shell names the release by the key of the
//! physical key's first press, which a repeat's own key need not match.

use super::RenderRoot;
use crate::event::{
    EventOutcome, InputEvent, Key, KeyEvent, Modifiers, ModifiersPass, OverlayEvent,
    OverlayEventKind,
};
use crate::overlay::OverlayKey;
use crate::shortcuts::ShortcutId;
use crate::view::View;

/// The held set's bound; a press past it evicts the oldest entry with a release.
const MAX_HELD_KEYS: usize = 64;

/// One key held down, with the shortcut (and the floated pod stamping it) that answered it.
pub(super) struct HeldKey {
    key: Key,
    shortcut: Option<(ShortcutId, Option<OverlayKey>)>,
}

/// The held keys, oldest first.
#[derive(Default)]
pub(super) struct HeldKeys(Vec<HeldKey>);

/// Key identity: named keys by equality, characters case-insensitively (as `Chord::matches`).
fn same_key(a: &Key, b: &Key) -> bool {
    match (a, b) {
        (Key::Named(a), Key::Named(b)) => a == b,
        (Key::Character(a), Key::Character(b)) => a.to_lowercase() == b.to_lowercase(),
        _ => false,
    }
}

impl HeldKeys {
    fn position(&self, key: &Key) -> Option<usize> {
        self.0.iter().position(|held| same_key(&held.key, key))
    }

    fn remove(&mut self, key: &Key) -> Option<HeldKey> {
        self.position(key).map(|index| self.0.remove(index))
    }

    /// Record `key` unless held; returns the oldest entry evicted to make room.
    fn insert(&mut self, key: &Key) -> Option<HeldKey> {
        if self.position(key).is_some() {
            return None;
        }
        let evicted = (self.0.len() >= MAX_HELD_KEYS).then(|| self.0.remove(0));
        self.0.push(HeldKey {
            key: key.clone(),
            shortcut: None,
        });
        evicted
    }
}

/// Fold `other`'s redraw into `outcome`, keeping `outcome`'s `handled`.
fn fold_redraw(outcome: &mut EventOutcome, other: EventOutcome) {
    outcome.needs_redraw |= other.needs_redraw;
}

impl<State: 'static, V: View<State>> RenderRoot<State, V> {
    /// A press: a fresh press of a held key releases it first, then the key is recorded (evicting
    /// the oldest past the cap) and dispatched. A repeat is dispatched without touching the held
    /// set: only a non-repeat press opens an entry, because the release is named by the key of
    /// the first press, which a repeat's key need not match. `handled` is the press's own.
    pub(super) fn press_key(
        &mut self,
        state: &mut State,
        event: &InputEvent,
        key: &KeyEvent,
    ) -> EventOutcome {
        let mut released = EventOutcome::default();
        if !key.repeat {
            if let Some(entry) = self.held_keys.remove(&key.key) {
                fold_redraw(
                    &mut released,
                    self.release_held(state, entry, Modifiers::NONE),
                );
            }
            if let Some(evicted) = self.held_keys.insert(&key.key) {
                fold_redraw(
                    &mut released,
                    self.release_held(state, evicted, Modifiers::NONE),
                );
            }
        }
        let mut outcome = self.dispatch_pass(state, event, None).0;
        fold_redraw(&mut outcome, released);
        outcome
    }

    /// A release: `None` when `key` is not held (nothing is dispatched).
    pub(super) fn release_key(
        &mut self,
        state: &mut State,
        key: &KeyEvent,
    ) -> Option<EventOutcome> {
        let entry = self.held_keys.remove(&key.key)?;
        Some(self.release_held(state, entry, key.modifiers))
    }

    /// `entry`'s `KeyUp`, then its recorded shortcut's release whether or not the `KeyUp` was
    /// handled; `handled` is the `KeyUp`'s.
    fn release_held(
        &mut self,
        state: &mut State,
        entry: HeldKey,
        modifiers: Modifiers,
    ) -> EventOutcome {
        let up = InputEvent::KeyUp(KeyEvent {
            key: entry.key,
            modifiers,
            repeat: false,
        });
        let mut outcome = self.dispatch_pass(state, &up, None).0;
        if let Some(shortcut) = entry.shortcut {
            fold_redraw(&mut outcome, self.release_shortcut(state, shortcut));
        }
        outcome
    }

    /// Deliver `id`'s release: bare, or through the owner of the floated pod that stamped it.
    fn release_shortcut(
        &mut self,
        state: &mut State,
        (id, overlay): (ShortcutId, Option<OverlayKey>),
    ) -> EventOutcome {
        let delivery = match overlay {
            None => InputEvent::ShortcutReleased(id),
            Some(key) => InputEvent::Overlay(OverlayEvent {
                key,
                kind: OverlayEventKind::ShortcutReleased(id),
            }),
        };
        self.dispatch_pass(state, &delivery, None).0
    }

    /// Record that `shortcut` answered held `key`, first releasing a different id it replaces.
    pub(super) fn record_held_shortcut(
        &mut self,
        state: &mut State,
        key: &Key,
        shortcut: (ShortcutId, Option<OverlayKey>),
        carried: &mut EventOutcome,
    ) {
        let Some(index) = self.held_keys.position(key) else {
            return;
        };
        let previous = self.held_keys.0[index].shortcut;
        if let Some(previous) = previous.filter(|(id, _)| *id != shortcut.0) {
            fold_redraw(carried, self.release_shortcut(state, previous));
        }
        // Looked up again: `release_shortcut` dispatches, which can change the held set.
        if let Some(index) = self.held_keys.position(key) {
            self.held_keys.0[index].shortcut = Some(shortcut);
        }
    }

    /// Release every held key and its shortcut, oldest first, as on window focus loss: each gets
    /// a `KeyUp` with no modifiers, then its shortcut's release. A no-op when nothing is held.
    pub fn cancel_held_keys(&mut self, state: &mut State) -> EventOutcome {
        if self.held_keys.0.is_empty() {
            return EventOutcome::default();
        }
        let _modifiers = ModifiersPass::enter(Modifiers::NONE);
        let mut outcome = EventOutcome::default();
        for entry in std::mem::take(&mut self.held_keys.0) {
            let released = self.release_held(state, entry, Modifiers::NONE);
            outcome.handled |= released.handled;
            outcome.needs_redraw |= released.needs_redraw;
        }
        self.finish_event(state, outcome)
    }

    /// Whether `key` has a held entry a shortcut answering it can be recorded against.
    pub(super) fn key_is_held(&self, key: &Key) -> bool {
        self.held_keys.position(key).is_some()
    }

    /// The keys held down, oldest first.
    pub fn held_keys(&self) -> Vec<Key> {
        self.held_keys
            .0
            .iter()
            .map(|held| held.key.clone())
            .collect()
    }
}
