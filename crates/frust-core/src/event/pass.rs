//! Pass-scoped per-thread request slots (cursor, clipboard, paste, edit commands) and the
//! save/restore guards that bracket them; the slots are [`CoreLocals`] fields. The cursor is a
//! slot, not a per-pod field: no container between the widget and the root reads it. The root
//! never reads the edit-command queue; one left undrained is cleared with its pass so a stale
//! `Cut` can never re-fire.

use std::cell::Cell;

use super::key::{EditCommand, Modifiers};
use super::pointer::CursorIcon;
use crate::thread_state::{self, CoreLocals};

/// Selects one per-thread slot (or open flag) from this thread's [`CoreLocals`].
pub(crate) type Field<T> = fn(&CoreLocals) -> &Cell<T>;

/// One pass-scoped slot's save/restore half; `T::default()` is its "nobody asked" state.
pub(crate) struct PassSlot<T: Default + 'static> {
    slot: Field<T>,
    /// The enclosing pass's value when nested, `T::default()` at the outermost level.
    restore: T,
}

impl<T: Default + 'static> PassSlot<T> {
    /// Open the slot empty, stashing its value for the enclosing pass only when `nested` (a value
    /// found with no pass open is a stray and is dropped).
    pub(crate) fn enter(slot: Field<T>, nested: bool) -> Self {
        let stashed = thread_state::with(|t| slot(t).take());
        Self {
            slot,
            restore: if nested { stashed } else { T::default() },
        }
    }

    /// Take what *this* pass recorded in this slot, leaving it empty.
    pub(crate) fn take(&self) -> T {
        thread_state::with(|t| (self.slot)(t).take())
    }
}

impl<T: Default + 'static> Drop for PassSlot<T> {
    fn drop(&mut self) {
        let restore = std::mem::take(&mut self.restore);
        thread_state::with(|t| (self.slot)(t).set(restore));
    }
}

/// A [`PassSlot`] carrying its own open flag, for a channel bracketed by a pass other than the
/// event dispatch (the paint pass's overlay registry). It drains behind `&self` because a paint
/// pass keeps painting after its drain; a nested pass hands the slot back on drop.
pub(crate) struct PassBracket<T: Default + 'static> {
    slot: PassSlot<T>,
    open: Field<bool>,
    /// Whether a pass of this kind was already open on entry; restored by `Drop`.
    was_open: bool,
}

impl<T: Default + 'static> PassBracket<T> {
    /// Open a pass over `slot`, tracked by `open`, starting from `T::default()`.
    pub(crate) fn enter(slot: Field<T>, open: Field<bool>) -> Self {
        let was_open = thread_state::with(|t| open(t).replace(true));
        Self {
            slot: PassSlot::enter(slot, was_open),
            open,
            was_open,
        }
    }

    /// Take what *this* pass recorded, leaving the slot empty.
    pub(crate) fn take(&self) -> T {
        self.slot.take()
    }
}

impl<T: Default + 'static> Drop for PassBracket<T> {
    fn drop(&mut self) {
        // The inner `PassSlot` restores the value as it drops, right after this.
        thread_state::with(|t| (self.open)(t).set(self.was_open));
    }
}

/// The shell-facing values one request pass resolved, drained by [`RequestPass::take`].
pub(crate) struct PassRequests {
    /// `None` when no widget asked, which resolves to [`CursorIcon::Default`].
    pub(crate) cursor: Option<CursorIcon>,
    /// `None` when no widget copied.
    pub(crate) clipboard_write: Option<String>,
    pub(crate) paste_request: bool,
}

/// The guard bracketing one request pass over every slot above. Each pass resolves exactly the
/// requests made inside it: a nested pass starts empty and hands the enclosing pass's slots back
/// on drop, and a leaked edit command is cleared with its pass.
pub(crate) struct RequestPass {
    cursor: PassSlot<Option<CursorIcon>>,
    clipboard_write: PassSlot<Option<String>>,
    paste_request: PassSlot<bool>,
    /// Bracketed only to bound the queue's lifetime; never reported to the root.
    edit_commands: PassSlot<Vec<EditCommand>>,
    was_open: bool,
}

impl RequestPass {
    /// Open a request pass, starting every slot from "nobody has asked for anything".
    pub(crate) fn enter() -> Self {
        let was_open = thread_state::with(|t| t.request_pass_open.replace(true));
        Self {
            cursor: PassSlot::enter(|t| &t.cursor_request, was_open),
            clipboard_write: PassSlot::enter(|t| &t.clipboard_write, was_open),
            paste_request: PassSlot::enter(|t| &t.paste_request, was_open),
            edit_commands: PassSlot::enter(|t| &t.edit_command_queue, was_open),
            was_open,
        }
    }

    /// Resolve and close the pass, consuming the guard so a second drain is unrepresentable.
    pub(crate) fn take(self) -> PassRequests {
        // An undrained edit command is a wiring bug in the dispatching widget; dropping it is the
        // safe resolution, and a debug build says so.
        let leaked = self.edit_commands.take();
        #[cfg(debug_assertions)]
        if !leaked.is_empty() {
            eprintln!(
                "frust-core: {} edit command(s) dispatched but never taken in this \
                 pass ({leaked:?}); clearing — the dispatching widget's consumer \
                 must call EventCtx::take_edit_commands in the same pass",
                leaked.len()
            );
        }
        drop(leaked);
        PassRequests {
            cursor: self.cursor.take(),
            clipboard_write: self.clipboard_write.take(),
            paste_request: self.paste_request.take(),
        }
    }
}

impl Drop for RequestPass {
    fn drop(&mut self) {
        // Each `PassSlot` restores its own slot as it drops, right after this.
        thread_state::with(|t| t.request_pass_open.set(self.was_open));
    }
}

/// The bracket around one root dispatch's held modifiers. Only the outermost guard owns the slot:
/// a nested root dispatch (the overlay pre-pass) inherits the open value and leaves it alone.
pub(crate) struct ModifiersPass {
    owns: bool,
}

impl ModifiersPass {
    /// Open a pass reading `modifiers`, unless one is already open (then the guard is inert).
    pub(crate) fn enter(modifiers: Modifiers) -> Self {
        let owns = thread_state::with(|t| {
            if t.modifiers_pass.get().is_some() {
                false
            } else {
                t.modifiers_pass.set(Some(modifiers));
                true
            }
        });
        Self { owns }
    }
}

impl Drop for ModifiersPass {
    fn drop(&mut self) {
        if self.owns {
            thread_state::with(|t| t.modifiers_pass.set(None));
        }
    }
}

/// The modifiers the running pass reads; [`Modifiers::NONE`] outside any pass.
pub(crate) fn current_modifiers() -> Modifiers {
    thread_state::with(|t| t.modifiers_pass.get()).unwrap_or(Modifiers::NONE)
}

/// Run `f` with `modifiers` as the running pass's held modifiers, restoring the previous value on
/// exit, unwind included.
#[cfg(any(test, feature = "test-support"))]
pub fn with_modifiers_for_test<R>(modifiers: Modifiers, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<Modifiers>);
    impl Drop for Restore {
        fn drop(&mut self) {
            thread_state::with(|t| t.modifiers_pass.set(self.0));
        }
    }
    let _restore = Restore(thread_state::with(|t| {
        t.modifiers_pass.replace(Some(modifiers))
    }));
    f()
}

/// Test-only bare clear of the cursor slot, for a widget driven with no root above it.
#[cfg(test)]
pub(crate) fn clear_cursor_request() {
    thread_state::with(|t| t.cursor_request.set(None));
}

/// Test-only drain of the cursor slot; `None` when no widget asked.
#[cfg(test)]
pub(crate) fn take_cursor_request() -> Option<CursorIcon> {
    thread_state::with(|t| t.cursor_request.take())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay::OverlayKey;

    fn key_forward(t: &CoreLocals) -> &Cell<Option<OverlayKey>> {
        &t.key_forward
    }

    fn set_key_forward(key: OverlayKey) {
        thread_state::with(|t| t.key_forward.set(Some(key)));
    }

    #[test]
    fn a_nested_bracket_starts_empty_and_hands_the_outer_value_back() {
        let (outer_key, inner_key) = (OverlayKey::next(), OverlayKey::next());
        let outer = PassBracket::enter(key_forward, |t| &t.key_forward_open);
        set_key_forward(outer_key);
        {
            let inner = PassBracket::enter(key_forward, |t| &t.key_forward_open);
            assert_eq!(inner.take(), None, "a nested pass starts empty");
            set_key_forward(inner_key);
            assert_eq!(inner.take(), Some(inner_key));
            set_key_forward(inner_key);
        }
        assert!(thread_state::with(|t| t.key_forward_open.get()));
        assert_eq!(outer.take(), Some(outer_key), "the outer value is restored");
        drop(outer);
        assert!(!thread_state::with(|t| t.key_forward_open.get()));
        assert_eq!(thread_state::with(|t| t.key_forward.get()), None);
    }

    #[test]
    fn a_slot_keeps_the_enclosing_value_only_when_nested() {
        let write = |text: &str| thread_state::with(|t| t.clipboard_write.set(Some(text.into())));
        write("stray");
        let outer = PassSlot::enter(|t| &t.clipboard_write, false);
        write("outer");
        {
            let inner = PassSlot::enter(|t| &t.clipboard_write, true);
            assert_eq!(inner.take(), None);
            write("inner");
        }
        assert_eq!(outer.take().as_deref(), Some("outer"));
        drop(outer);
        assert_eq!(
            thread_state::with(|t| t.clipboard_write.take()),
            None,
            "the stray is gone"
        );
    }
}
