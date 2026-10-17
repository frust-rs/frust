//! Pass-scoped thread-local request slots (cursor, clipboard, paste, edit commands) and the
//! save/restore guards that bracket them.

use std::cell::Cell;
use std::thread::LocalKey;

use super::key::EditCommand;
use super::pointer::CursorIcon;

thread_local! {
    /// The cursor this pass asked for: written by [`EventCtx::set_cursor`], last writer wins,
    /// drained by the [`RequestPass`] `RenderRoot::event` holds. A slot, not a per-pod field: no
    /// container between the widget and the root reads it.
    pub(super) static CURSOR_REQUEST: Cell<Option<CursorIcon>> = const { Cell::new(None) };

    /// The text this pass asked the shell to copy: written by [`EventCtx::write_clipboard`], last
    /// writer wins, drained through [`RequestPass`] into `RenderRoot::take_clipboard_write`.
    pub(super) static CLIPBOARD_WRITE: Cell<Option<String>> = const { Cell::new(None) };

    /// Whether this pass asked for a paste: raised by [`EventCtx::request_paste`] (idempotent),
    /// drained through [`RequestPass`] into `RenderRoot::take_paste_request`.
    pub(super) static PASTE_REQUEST: Cell<bool> = const { Cell::new(false) };

    /// Widget-to-widget edit commands, FIFO: pushed by [`EventCtx::dispatch_edit_command`],
    /// drained in order by [`EventCtx::take_edit_commands`] in the same pass. The root never reads
    /// it; one left undrained is cleared with its pass so a stale `Cut` can never re-fire.
    pub(super) static EDIT_COMMAND_QUEUE: Cell<Vec<EditCommand>> = const { Cell::new(Vec::new()) };

    /// Whether a request pass is open on this thread; read and written only by [`RequestPass`].
    static REQUEST_PASS_OPEN: Cell<bool> = const { Cell::new(false) };
}

/// One pass-scoped slot's save/restore half; `T::default()` is its "nobody asked" state.
pub(crate) struct PassSlot<T: Default + 'static> {
    slot: &'static LocalKey<Cell<T>>,
    /// The enclosing pass's value when nested, `T::default()` at the outermost level.
    restore: T,
}

impl<T: Default + 'static> PassSlot<T> {
    /// Open the slot empty, stashing its value for the enclosing pass only when `nested` (a value
    /// found with no pass open is a stray and is dropped).
    pub(crate) fn enter(slot: &'static LocalKey<Cell<T>>, nested: bool) -> Self {
        let stashed = slot.with(|cell| cell.take());
        Self {
            slot,
            restore: if nested { stashed } else { T::default() },
        }
    }

    /// Take what *this* pass recorded in this slot, leaving it empty.
    pub(crate) fn take(&self) -> T {
        self.slot.with(|cell| cell.take())
    }
}

impl<T: Default + 'static> Drop for PassSlot<T> {
    fn drop(&mut self) {
        let restore = std::mem::take(&mut self.restore);
        self.slot.with(|cell| cell.set(restore));
    }
}

/// A [`PassSlot`] carrying its own open flag, for a channel bracketed by a pass other than the
/// event dispatch (the paint pass's overlay registry). It drains behind `&self` because a paint
/// pass keeps painting after its drain; a nested pass hands the slot back on drop.
pub(crate) struct PassBracket<T: Default + 'static> {
    slot: PassSlot<T>,
    open: &'static LocalKey<Cell<bool>>,
    /// Whether a pass of this kind was already open on entry; restored by `Drop`.
    was_open: bool,
}

impl<T: Default + 'static> PassBracket<T> {
    /// Open a pass over `slot`, tracked by `open`, starting from `T::default()`.
    pub(crate) fn enter(
        slot: &'static LocalKey<Cell<T>>,
        open: &'static LocalKey<Cell<bool>>,
    ) -> Self {
        let was_open = open.with(|flag| flag.replace(true));
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
        self.open.with(|flag| flag.set(self.was_open));
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
        let was_open = REQUEST_PASS_OPEN.with(|open| open.replace(true));
        Self {
            cursor: PassSlot::enter(&CURSOR_REQUEST, was_open),
            clipboard_write: PassSlot::enter(&CLIPBOARD_WRITE, was_open),
            paste_request: PassSlot::enter(&PASTE_REQUEST, was_open),
            edit_commands: PassSlot::enter(&EDIT_COMMAND_QUEUE, was_open),
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
        REQUEST_PASS_OPEN.with(|open| open.set(self.was_open));
    }
}

/// Test-only bare clear of the cursor slot, for a widget driven with no root above it.
#[cfg(test)]
pub(crate) fn clear_cursor_request() {
    CURSOR_REQUEST.with(|slot| slot.set(None));
}

/// Test-only drain of the cursor slot; `None` when no widget asked.
#[cfg(test)]
pub(crate) fn take_cursor_request() -> Option<CursorIcon> {
    CURSOR_REQUEST.with(|slot| slot.take())
}
