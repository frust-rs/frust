//! Pairs a key release with the logical [`Key`] reported at press.
//!
//! Platforms identify a release by a physical key, and the text a release
//! yields can differ from the press (Shift changed mid-hold, or the platform
//! gives no text on release). A shell records the logical key at press here
//! and reads it back at release so both name the same key. The set also
//! yields the keys pressed while Meta was held, for hosts that never see a
//! release under Meta. Winit-free: `P` is whatever physical identifier the
//! shell uses.

use frust_core::{Key, Modifiers};

/// The physical-to-logical pairing of the keys currently held, oldest first.
#[derive(Clone, Debug)]
pub struct PressedKeys<P> {
    entries: Vec<(P, Key, Modifiers)>,
}

impl<P> Default for PressedKeys<P> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
}

impl<P: PartialEq> PressedKeys<P> {
    /// The most keys tracked at once; past it the oldest entry is dropped.
    pub const CAPACITY: usize = 64;

    /// An empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a press. A repeat or re-press of the same `physical` keeps its
    /// original position and key.
    pub fn press(&mut self, physical: P, key: Key, modifiers: Modifiers) {
        if self.entries.iter().any(|(p, _, _)| *p == physical) {
            return;
        }
        if self.entries.len() >= Self::CAPACITY {
            self.entries.remove(0);
        }
        self.entries.push((physical, key, modifiers));
    }

    /// Removes `physical` and returns the key recorded at its press, or
    /// `None` when it was never recorded.
    pub fn release(&mut self, physical: &P) -> Option<Key> {
        let at = self.entries.iter().position(|(p, _, _)| p == physical)?;
        Some(self.entries.remove(at).1)
    }

    /// Removes and returns, oldest first, every key whose press had Meta held.
    pub fn release_pressed_with_meta(&mut self) -> Vec<Key> {
        let mut out = Vec::new();
        self.entries.retain_mut(|(_, key, mods)| {
            if mods.meta {
                out.push(key.clone());
                false
            } else {
                true
            }
        });
        out
    }

    /// Forgets every held key.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Whether no key is held.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// How many keys are held.
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(s: &str) -> Key {
        Key::Character(s.to_string())
    }

    fn meta() -> Modifiers {
        Modifiers {
            meta: true,
            ..Modifiers::default()
        }
    }

    #[test]
    fn a_release_returns_the_key_recorded_at_press() {
        let mut keys = PressedKeys::new();
        keys.press(1u32, ch("a"), Modifiers::default());
        assert_eq!(keys.release(&1), Some(ch("a")));
        assert!(keys.is_empty());
    }

    #[test]
    fn a_repeat_keeps_the_first_recorded_key() {
        let mut keys = PressedKeys::new();
        keys.press(1u32, ch("a"), Modifiers::default());
        keys.press(2, ch("b"), Modifiers::default());
        keys.press(1, ch("A"), Modifiers::default());
        assert_eq!(keys.len(), 2);
        assert_eq!(keys.release(&1), Some(ch("a")));
    }

    #[test]
    fn an_unknown_release_is_none() {
        let mut keys: PressedKeys<u32> = PressedKeys::new();
        assert_eq!(keys.release(&9), None);
    }

    #[test]
    fn meta_chords_drain_oldest_first_and_leave_the_rest() {
        let mut keys = PressedKeys::new();
        keys.press(1u32, ch("c"), meta());
        keys.press(2, ch("x"), Modifiers::default());
        keys.press(3, ch("v"), meta());
        assert_eq!(keys.release_pressed_with_meta(), vec![ch("c"), ch("v")]);
        assert_eq!(keys.len(), 1);
        assert_eq!(keys.release(&2), Some(ch("x")));
    }

    #[test]
    fn clear_empties_the_set() {
        let mut keys = PressedKeys::new();
        keys.press(1u32, ch("a"), Modifiers::default());
        keys.clear();
        assert!(keys.is_empty());
        assert_eq!(keys.len(), 0);
    }

    #[test]
    fn the_oldest_entry_is_dropped_at_capacity() {
        let mut keys = PressedKeys::new();
        for i in 0..=PressedKeys::<usize>::CAPACITY {
            keys.press(i, ch("k"), Modifiers::default());
        }
        assert_eq!(keys.len(), PressedKeys::<usize>::CAPACITY);
        assert_eq!(keys.release(&0), None);
        assert_eq!(keys.release(&1), Some(ch("k")));
    }
}
