use std::cell::Cell;
use std::thread::LocalKey;

use super::key::EditCommand;
use super::pointer::CursorIcon;

thread_local! {
    /// The cursor a widget asked for during the request pass currently running on
    /// this thread — written by [`EventCtx::set_cursor`], bracketed by the
    /// [`RequestPass`] guard [`crate::app::RenderRoot::event`] holds for the
    /// length of its dispatch.
    ///
    /// A side channel for a *routing* reason rather than the missing-handle
    /// reason [`PENDING_RESULT_FLUSH`] and [`FOCUS_ORPHANED`] above have. Unlike
    /// capture, focus, and hover, a cursor request has nothing to record **per
    /// pod**: the root wants one value — whichever widget on the routed path
    /// spoke last — and no container between that widget and the root reads it or
    /// acts on it. Bubbling it pod by pod would mean widening every container's
    /// fold to carry a value no container uses.
    ///
    /// **Pass-scoped, not persistent.** The guard clears the slot before
    /// dispatching and drains it after, so a request never outlives its pass, and
    /// a [`EventCtx::set_cursor`] made from a dispatch no root drives (the
    /// `Cancel` a reconciler synthesizes during a rebuild, say) is dropped by the
    /// next pass's clear rather than leaking into it. Last write wins, which is
    /// what makes the innermost widget the routed path reaches the one that
    /// decides. A *nested* pass is scoped the same way and hands the slot back
    /// (see [`RequestPass`]).
    ///
    /// Thread-local rather than a process-global for the same UI-thread-affinity
    /// reason as its two neighbours: the tree that requests a cursor and the
    /// `RenderRoot` whose shell applies it live on one thread, and a global would
    /// let a hover on one thread reshape another window's pointer.
    pub(super) static CURSOR_REQUEST: Cell<Option<CursorIcon>> = const { Cell::new(None) };

    /// The text a widget asked the shell to put on the host clipboard during the
    /// request pass currently running on this thread — written by
    /// [`EventCtx::write_clipboard`], bracketed by the same [`RequestPass`] guard,
    /// and resolved by [`crate::app::RenderRoot::event`] into the value a shell
    /// drains through
    /// [`RenderRoot::take_clipboard_write`](crate::app::RenderRoot::take_clipboard_write).
    ///
    /// **A slot rather than a bubbled field, for [`CURSOR_REQUEST`]'s routing
    /// reason verbatim** (above): the root wants one value — whichever widget on
    /// the routed path spoke last — and no container between the copying widget
    /// and the root reads it or acts on it, so recording it per pod would widen
    /// every container's fold ([`EventCtx::absorb_child`], and with it
    /// [`crate::widget::ChildPod::event_child`] and every hand-written router in
    /// `frust-widgets`) to carry a payload no container uses. `ImeState` is
    /// bubbled precisely because containers *do* re-publish it; a clipboard write
    /// is a one-way message to the shell.
    ///
    /// **Pass-scoped and last-writer-wins**, exactly like the cursor: a write
    /// made outside any pass (a reconciler's synthesized `Cancel`) is dropped
    /// rather than leaked into the next pass, and a `Cut` that writes from an
    /// inner widget after its container wrote something else sends the inner
    /// widget's text.
    ///
    /// Thread-local for its neighbours' UI-thread-affinity reason: the tree that
    /// copies and the `RenderRoot` whose shell owns the host clipboard live on
    /// one thread.
    pub(super) static CLIPBOARD_WRITE: Cell<Option<String>> = const { Cell::new(None) };

    /// Whether a widget asked the shell to hand it the host clipboard's contents
    /// during the request pass currently running on this thread — raised by
    /// [`EventCtx::request_paste`], bracketed by the same [`RequestPass`] guard,
    /// and resolved by [`crate::app::RenderRoot::event`] into the flag a shell
    /// drains through
    /// [`RenderRoot::take_paste_request`](crate::app::RenderRoot::take_paste_request).
    ///
    /// **Data-free and idempotent**, like [`PENDING_RESULT_FLUSH`]: only the
    /// *fact* that a paste was asked for rides here, because the answer is the
    /// shell's to compose (it reads the host clipboard and dispatches
    /// [`InputEvent::EditCommand`]`(`[`EditCommand::Paste`]`)`). Two widgets
    /// asking in one pass owe exactly one read — there is one host clipboard and
    /// one focused widget to deliver it to, so "who asked" adds nothing.
    ///
    /// Pass-scoped and thread-local for [`CLIPBOARD_WRITE`]'s reasons.
    pub(super) static PASTE_REQUEST: Cell<bool> = const { Cell::new(false) };

    /// Edit commands one widget dispatched to another during the request pass
    /// currently running on this thread — pushed by
    /// [`EventCtx::dispatch_edit_command`] and drained, in order, by
    /// [`EventCtx::take_edit_commands`].
    ///
    /// **A widget-to-widget channel, not a shell-facing one**, which is what
    /// makes it different from its three neighbours above: the cursor, the
    /// clipboard write and the paste request all resolve *at the root* into
    /// something a shell reads, whereas this queue is drained by another widget
    /// in the same pass and the root never looks at it. It rides here anyway
    /// because the two widgets cannot reach each other any other way — a
    /// selection toolbar floated through [`crate::overlay`] is a pod the *text
    /// input* owns but does not contain, so the toolbar's "Copy" tap has no
    /// container path down which to hand the verb back.
    ///
    /// **A FIFO, not a last-writer-wins slot**: a toolbar may answer one tap with
    /// several verbs (a "cut" that is a copy then a delete), and order is
    /// meaning.
    ///
    /// Pass-scoped like its neighbours, and for a sharper reason: an undrained
    /// command must never re-fire in a later pass — a stale `Cut` applied to
    /// whatever is selected two gestures later would silently destroy text. A
    /// pass that ends with the queue non-empty therefore clears it (and says so
    /// in a debug build — see [`RequestPass::take`]) rather than carrying it.
    ///
    /// Thread-local for its neighbours' UI-thread-affinity reason.
    pub(super) static EDIT_COMMAND_QUEUE: Cell<Vec<EditCommand>> = const { Cell::new(Vec::new()) };

    /// Whether a request pass is open on this thread — `false` at rest, `true` for
    /// the length of one, however many are nested. Owned by [`RequestPass`], which
    /// is the only thing that reads or writes it: [`RequestPass::enter`] captures
    /// the previous value into the guard and `Drop` puts exactly that value back,
    /// so an unwind through a nested pass restores the enclosing pass's state
    /// rather than leaving a counter to unwind correctly on its own.
    ///
    /// The one question it answers is whether [`RequestPass::enter`] found an
    /// *enclosing* pass's in-progress requests in the slots (restore them on exit)
    /// or stray requests made outside any pass (drop them), which the slots' own
    /// contents cannot distinguish. One flag covers all three slots because one
    /// guard brackets all three: they open and close together, per dispatch.
    static REQUEST_PASS_OPEN: Cell<bool> = const { Cell::new(false) };
}

/// One pass-scoped request slot's save/restore half — the mechanism [`RequestPass`]
/// owns three of.
///
/// Generic over the slot's payload rather than written out per channel: the
/// cursor, the clipboard write, and the paste flag differ only in what they
/// carry, and three hand-copied guards would be three places for the
/// stash-and-restore invariant to drift. `T::default()` is each slot's "nobody
/// asked" state (`None`, `None`, `false`), which is exactly what makes absence
/// the answer rather than a missing answer.
pub(crate) struct PassSlot<T: Default + 'static> {
    /// The thread-local this half brackets. A `&'static` handle so one generic
    /// body serves every channel — [`LocalKey::with`] needs the `'static`
    /// reference anyway.
    slot: &'static LocalKey<Cell<T>>,
    /// What the slot is restored to when this pass ends: the enclosing pass's
    /// in-progress request when nested, `T::default()` at the outermost level.
    restore: T,
}

impl<T: Default + 'static> PassSlot<T> {
    /// Open this slot for a pass, starting it from "nobody has asked for
    /// anything" and stashing whatever an enclosing pass had collected.
    ///
    /// `nested` is the shared [`REQUEST_PASS_OPEN`] answer: only an enclosing
    /// pass is owed its value back, since a value found in the slot with no pass
    /// open is a stray (see [`CURSOR_REQUEST`]).
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

/// A pass-scoped slot **carrying its own open flag** — [`PassSlot`] made
/// self-contained, for a channel bracketed by a *different* pass than the event
/// dispatch.
///
/// [`RequestPass`] brackets three channels that open and close together, so one
/// [`REQUEST_PASS_OPEN`] flag serves all three. A channel scoped to the **paint**
/// pass instead (the overlay registry and the selection-toolbar publish slot —
/// see [`crate::overlay`] and [`crate::selection_toolbar`]) cannot share that
/// flag: a paint pass runs with no event pass open, and an event pass with no
/// paint pass open, so borrowing the other's flag would answer "is an enclosing
/// pass of MY kind open?" with another kind's state and either restore a stray
/// or drop an enclosing pass's work. One flag per bracket is what keeps the
/// question well-posed.
///
/// Everything else is [`PassSlot`]'s, verbatim: enter stashes, [`take`](Self::take)
/// drains what this pass alone recorded, and `Drop` puts the enclosing pass's
/// stash back (or leaves the slot clear at the outermost level, so a value
/// written with no pass open is dropped rather than leaked into the next one).
///
/// Unlike [`RequestPass::take`] this drains behind `&self` rather than consuming
/// the guard: a paint pass resolves its channels *and then* keeps painting
/// (`RenderRoot::paint` drains the registry, then paints what it drained), so the
/// bracket has to outlive its own drain.
pub(crate) struct PassBracket<T: Default + 'static> {
    /// The value half, which owns the stash/restore invariant.
    slot: PassSlot<T>,
    /// This bracket's own "a pass is open" flag.
    open: &'static LocalKey<Cell<bool>>,
    /// Whether a pass of this kind was already open when this one entered — put
    /// back verbatim by `Drop`.
    was_open: bool,
}

impl<T: Default + 'static> PassBracket<T> {
    /// Open a pass over `slot`, tracked by `open`, starting from
    /// `T::default()` ("nobody has recorded anything").
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

/// Everything one request pass resolved: the three shell-facing values a
/// dispatch can produce, drained together by [`RequestPass::take`].
pub(crate) struct PassRequests {
    /// The cursor the pass's last [`EventCtx::set_cursor`] asked for; `None` when
    /// no widget asked, which resolves to [`CursorIcon::Default`].
    pub(crate) cursor: Option<CursorIcon>,
    /// The text the pass's last [`EventCtx::write_clipboard`] asked the shell to
    /// put on the host clipboard; `None` when no widget copied.
    pub(crate) clipboard_write: Option<String>,
    /// Whether any widget in the pass called [`EventCtx::request_paste`].
    pub(crate) paste_request: bool,
}

/// The open/close bracket around one request pass, and the guard that makes the
/// pass-scoped slots above survive re-entrancy.
///
/// # Why a guard rather than a bare clear/take pair
///
/// Each slot is *pass-scoped*: cleared before a dispatch, drained after it, so a
/// request never outlives the pass that made it. Spelled as a bare
/// `clear_cursor_request` + `take_cursor_request` pair that contract holds
/// only while passes never nest — a nested dispatch's clear would erase a request
/// the enclosing pass had already collected, and its drain would take one the
/// enclosing pass was still owed. Nothing in this workspace nests a pass today
/// ([`RenderRoot::event`](crate::app::RenderRoot::event) documents the rule, and
/// the devtools injector hops its synthetic events onto the UI thread's queue
/// rather than calling into a live dispatch), but the failure is silent and the
/// cost of ruling it out is one stack slot.
///
/// # One guard, three channels
///
/// The cursor, the clipboard write and the paste request are all "one value the
/// root resolves at the end of the dispatch", so they share a bracket and a
/// single [`REQUEST_PASS_OPEN`] flag rather than three copies of this reasoning;
/// the per-slot half is [`PassSlot`], instantiated once per channel. What differs
/// is only what the root *does* with each value — see
/// [`RenderRoot::event`](crate::app::RenderRoot::event), where the cursor commits
/// on pointer-move passes alone while the two clipboard values commit on every
/// pass.
///
/// # What nesting resolves to
///
/// Save-and-restore, so **every** pass — nested or not — resolves exactly the
/// requests made inside it, and an inner pass returns the slots to the enclosing
/// pass untouched:
///
/// * [`RequestPass::enter`] stashes whatever the enclosing pass had collected and
///   starts the inner pass from empty (the same "absence *is* the answer" state a
///   top-level pass starts from).
/// * [`RequestPass::take`] drains what this pass alone recorded, and **consumes
///   the guard**: a pass resolves exactly once, and the drain is what closes it.
/// * `Drop` puts the enclosing pass's stash back — or, at the outermost level,
///   leaves the slots clear, exactly as the bare pair did, so a request made
///   outside any pass (a reconciler's synthesized `Cancel`) is still dropped
///   rather than leaked into the next one.
pub(crate) struct RequestPass {
    /// The cursor half of the bracket.
    cursor: PassSlot<Option<CursorIcon>>,
    /// The clipboard-write half.
    clipboard_write: PassSlot<Option<String>>,
    /// The paste-request half.
    paste_request: PassSlot<bool>,
    /// The widget-to-widget edit-command queue. Bracketed here rather than
    /// resolved into [`PassRequests`]: the root must never see it (its consumer
    /// is another widget in the same pass), so this half exists only to bound the
    /// queue's lifetime to the pass — see [`EDIT_COMMAND_QUEUE`].
    edit_commands: PassSlot<Vec<EditCommand>>,
    /// Whether a pass was already open when this one entered — put back verbatim
    /// by `Drop`, so an inner pass leaves the enclosing one open and the
    /// outermost leaves the thread at rest.
    was_open: bool,
}

impl RequestPass {
    /// Open a request pass, starting every slot from "nobody has asked for
    /// anything".
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

    /// Take what *this* pass recorded — the values the root resolves into its
    /// shell-facing cursor, clipboard write and paste request.
    ///
    /// Consumes the guard, so the pass ends here: a second drain of the same pass
    /// is unrepresentable rather than a silent set of empties (the slots are
    /// drained destructively, so a repeat call would report "nobody asked" for a
    /// pass that had already resolved).
    pub(crate) fn take(self) -> PassRequests {
        // The edit-command queue is drained here but NOT reported: anything left
        // in it is a command whose intended consumer never called
        // `EventCtx::take_edit_commands` — a wiring bug in the dispatching
        // widget, not something the root can act on. Dropping it is the safe
        // resolution (a command that survived into a later pass would apply to
        // whatever is selected *then*), and a debug build says so rather than
        // swallowing it silently.
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

/// Clear any pending cursor request, so the pass about to run starts from
/// "nobody has asked for anything".
///
/// Absence is not a missing answer — it *is* the answer
/// ([`CursorIcon::Default`]), which is why the clear is what makes the request
/// model stateless: a widget that stops asking stops being obeyed, with nothing
/// to release.
///
/// **Test-only.** Production code brackets a pass with [`RequestPass`], which
/// owns both ends; this is the bare clear a test that drives a widget *with no
/// root above it* needs to start from a known slot (`component.rs`'s
/// component-boundary cursor test is the one caller).
#[cfg(test)]
pub(crate) fn clear_cursor_request() {
    CURSOR_REQUEST.with(|slot| slot.set(None));
}

/// Take (and clear) the cursor requested during this pass, `None` when no widget
/// asked — the drain half of the test-only pair (see
/// [`clear_cursor_request`]); a root reaches the same value through
/// [`RequestPass::take`].
#[cfg(test)]
pub(crate) fn take_cursor_request() -> Option<CursorIcon> {
    CURSOR_REQUEST.with(|slot| slot.take())
}
