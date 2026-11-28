//! Every per-thread slot, registry and id counter frust-core keeps, as the fields of one
//! [`CoreLocals`] behind one `thread_local!` static: on Android each static takes one of 128
//! process-wide pthread keys, and a field costs per access what its own static did.

use std::cell::{Cell, RefCell};

use crate::event::{
    ContactFrameState, ContactPassState, CursorIcon, EditCommand, Modifiers, SecondaryWalkFrame,
};
use crate::focus_handle::{Binding, Press};
use crate::focus_scope::FocusScopeKey;
use crate::overlay::{OverlayEntry, OverlayKey};
use crate::selection_toolbar::SelectionToolbarRequest;
use crate::shortcuts::{ShortcutEntry, ShortcutId};
use crate::widget::ChildPod;

/// One field per slot; the owning modules keep the accessors. No outer cell wraps it, so nested
/// access to two fields behaves as nested access to two statics. Once the thread's teardown starts
/// destroying this struct no field is reachable, whatever the order: a `Drop` that can run then
/// (a pod or handle a registry field holds) goes through [`try_with`].
pub(crate) struct CoreLocals {
    /// Entries registered so far in this thread's open paint pass, in order.
    pub(crate) overlay_registry: Cell<Vec<OverlayEntry>>,

    pub(crate) bindings: Cell<Vec<Binding>>,

    /// Declarations made so far in this thread's open paint pass, in paint order.
    pub(crate) shortcut_table: Cell<Vec<ShortcutEntry>>,

    /// Scopes registered so far in this thread's open paint pass, in order, each with whether
    /// its host held the focus path.
    pub(crate) scope_registry: Cell<Vec<(FocusScopeKey, bool)>>,

    /// Widget-to-widget edit commands, FIFO: pushed by `EventCtx::dispatch_edit_command`, drained
    /// in order by `EventCtx::take_edit_commands` in the same pass, cleared with its pass.
    pub(crate) edit_command_queue: Cell<Vec<EditCommand>>,

    /// The text this pass asked the shell to copy: written by `EventCtx::write_clipboard`, last
    /// writer wins, drained through `RequestPass` into `RenderRoot::take_clipboard_write`.
    pub(crate) clipboard_write: Cell<Option<String>>,

    /// The innermost secondary-contact walk level; each level saves and restores the enclosing one.
    pub(crate) secondary_walk: RefCell<Option<SecondaryWalkFrame>>,

    /// The stops named by the open notification broadcast.
    pub(crate) notice: RefCell<Vec<u64>>,

    /// This paint pass's toolbar request, last writer wins; `None` with no publish (a blurred field).
    pub(crate) published: Cell<Option<SelectionToolbarRequest>>,

    pub(crate) toolbar_pass_open: Cell<bool>,

    /// The next [`OverlayKey::next`] value; starts at `1` so `0` can mean "none".
    pub(crate) next_overlay_key: Cell<u64>,

    pub(crate) overlay_pass_open: Cell<bool>,

    pub(crate) bind_pass_open: Cell<bool>,

    pub(crate) press: Cell<Press>,

    /// The next [`ShortcutId::next`] value; starts at `1` so `0` is never handed out.
    pub(crate) next_shortcut_id: Cell<u64>,

    pub(crate) shortcut_pass_open: Cell<bool>,

    /// The floated pod the root is painting, stamped onto every declaration it makes.
    pub(crate) current_overlay: Cell<Option<OverlayKey>>,

    /// Raised when a widget answers a shortcut delivery `Handled` (containers drop the result).
    pub(crate) shortcut_handled: Cell<bool>,

    /// The pod this event pass asked declined keys be forwarded to.
    pub(crate) key_forward: Cell<Option<OverlayKey>>,

    pub(crate) key_forward_open: Cell<bool>,

    /// The next [`FocusScopeKey::new`] value; starts at `1` so `0` can mean "none".
    pub(crate) next_focus_scope_key: Cell<u64>,

    pub(crate) scope_pass_open: Cell<bool>,

    /// The cursor this pass asked for: written by `EventCtx::set_cursor`, last writer wins, drained
    /// by the `RequestPass` `RenderRoot::event` holds.
    pub(crate) cursor_request: Cell<Option<CursorIcon>>,

    /// Whether this pass asked for a paste: raised by `EventCtx::request_paste` (idempotent),
    /// drained through `RequestPass` into `RenderRoot::take_paste_request`.
    pub(crate) paste_request: Cell<bool>,

    /// Whether a request pass is open on this thread; read and written only by `RequestPass`.
    pub(crate) request_pass_open: Cell<bool>,

    /// The smallest hit-testing pod area a pointer event landed inside during the open
    /// `EventCtx::track_pointer_hits` scope (`INFINITY` until one does); `None` with no scope open.
    pub(crate) pointer_hit_area: Cell<Option<f64>>,

    /// The contact pass running on this thread, bracketed by `ContactPass`.
    pub(crate) contact_pass: Cell<ContactPassState>,

    /// The held modifiers of the root dispatch in flight; `None` with no pass open.
    pub(crate) modifiers_pass: Cell<Option<Modifiers>>,

    /// The innermost widget dispatch's opt-in bookkeeping, bracketed by `ContactFrame`.
    pub(crate) contact_frame: Cell<ContactFrameState>,

    /// The secondary walk carrier's key, minted from [`OverlayKey::next`] on first use, so no
    /// overlay owner holds it.
    pub(crate) secondary_walk_key: Cell<Option<OverlayKey>>,

    /// A deferred state-bearing callback is queued; raised by `mark_pending_result_flush`.
    pub(crate) pending_result_flush: Cell<bool>,

    /// A focused pod lost its identity in this thread's view diff; see `mark_focus_orphaned`.
    pub(crate) focus_orphaned: Cell<bool>,

    /// Which root owes a hover end because a pod holding its live link was dropped.
    pub(crate) hover_orphaned: Cell<Option<u64>>,

    /// The live hover link as `(root identity, epoch)`; `(0, 0)` when nothing holds one.
    pub(crate) live_hover_link: Cell<(u64, u64)>,

    /// The next tooling id [`ChildPod::inspect_id`] hands out (UI-thread only).
    pub(crate) next_inspect_id: Cell<u64>,

    /// The inspect id of the pod that recorded this dispatch's hover claim.
    pub(crate) hover_claimant: Cell<Option<u64>>,

    /// Whether the first pod to record this dispatch's focus claim already held a live link.
    pub(crate) focus_req_from_live: Cell<Option<bool>>,

    /// The focus session as `(root, live epoch, claim epoch)`; the claim epoch is a dispatch's
    /// candidate session. `(0, 0, 0)` until a root publishes, which a real root never does.
    pub(crate) live_focus_session: Cell<(u64, u64, u64)>,

    /// The open root→pod focus grant as `(carrier, target inspect id, landed)`; lands once.
    pub(crate) focus_grant: Cell<Option<(ShortcutId, u64, bool)>>,
}

impl CoreLocals {
    /// Every field at the initial value its static had.
    const fn new() -> Self {
        Self {
            overlay_registry: Cell::new(Vec::new()),
            bindings: Cell::new(Vec::new()),
            shortcut_table: Cell::new(Vec::new()),
            scope_registry: Cell::new(Vec::new()),
            edit_command_queue: Cell::new(Vec::new()),
            clipboard_write: Cell::new(None),
            secondary_walk: RefCell::new(None),
            notice: RefCell::new(Vec::new()),
            published: Cell::new(None),
            toolbar_pass_open: Cell::new(false),
            next_overlay_key: Cell::new(1),
            overlay_pass_open: Cell::new(false),
            bind_pass_open: Cell::new(false),
            press: Cell::new(Press::Idle),
            next_shortcut_id: Cell::new(1),
            shortcut_pass_open: Cell::new(false),
            current_overlay: Cell::new(None),
            shortcut_handled: Cell::new(false),
            key_forward: Cell::new(None),
            key_forward_open: Cell::new(false),
            next_focus_scope_key: Cell::new(1),
            scope_pass_open: Cell::new(false),
            cursor_request: Cell::new(None),
            paste_request: Cell::new(false),
            request_pass_open: Cell::new(false),
            pointer_hit_area: Cell::new(None),
            contact_pass: Cell::new(ContactPassState::IDLE),
            modifiers_pass: Cell::new(None),
            contact_frame: Cell::new(ContactFrameState::IDLE),
            secondary_walk_key: Cell::new(None),
            pending_result_flush: Cell::new(false),
            focus_orphaned: Cell::new(false),
            hover_orphaned: Cell::new(None),
            live_hover_link: Cell::new((0, 0)),
            next_inspect_id: Cell::new(ChildPod::INSPECT_ID_BASE),
            hover_claimant: Cell::new(None),
            focus_req_from_live: Cell::new(None),
            live_focus_session: Cell::new((0, 0, 0)),
            focus_grant: Cell::new(None),
        }
    }
}

thread_local! {
    static CORE: CoreLocals = const { CoreLocals::new() };
}

/// Run `f` on this thread's state; panics once the thread's teardown has destroyed it, so a `Drop`
/// that can run then (a pod or callback a registry field holds) uses [`try_with`].
pub(crate) fn with<R>(f: impl FnOnce(&CoreLocals) -> R) -> R {
    CORE.with(f)
}

/// Run `f` on this thread's state, or return `None` once the thread's teardown has destroyed it.
pub(crate) fn try_with<R>(f: impl FnOnce(&CoreLocals) -> R) -> Option<R> {
    CORE.try_with(f).ok()
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;
    use std::thread;

    use kurbo::{Rect, Size};

    use super::*;
    use crate::FocusHandle;
    use crate::insets::WindowInsets;
    use crate::layout::BoxConstraints;
    use crate::overlay::{OutsideTap, OverlayBand, OverlayInput, OverlayKeys};
    use crate::shortcuts::{Chord, ShortcutDecl, ShortcutScope};
    use crate::widget::{LayoutCtx, PaintCtx, PaintScene, Widget};

    /// A leaf whose drop reaches back into the per-thread state, as a user callback may.
    struct Teardown(FocusHandle);

    impl Widget for Teardown {
        fn layout(&mut self, _ctx: &mut LayoutCtx, _bc: &BoxConstraints) -> Size {
            Size::ZERO
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    }

    impl Drop for Teardown {
        fn drop(&mut self) {
            self.0.release();
            crate::event::mark_focus_orphaned();
        }
    }

    #[test]
    fn a_thread_exits_cleanly_with_every_registry_holding_a_value() {
        thread::spawn(|| {
            let handle = FocusHandle::new();
            crate::overlay::register(OverlayEntry {
                key: OverlayKey::next(),
                band: OverlayBand::Floating,
                input: OverlayInput::Interactive,
                outside_tap: OutsideTap::Ignore,
                keys: OverlayKeys::Pass,
                window_rect: Rect::new(0.0, 0.0, 10.0, 10.0),
                pod: Rc::new(RefCell::new(ChildPod::new(Box::new(Teardown(
                    handle.clone(),
                ))))),
                insets: WindowInsets::default(),
            });
            crate::shortcuts::register(ShortcutDecl {
                id: ShortcutId::next(),
                chord: Chord::parse("Mod+K").expect("a valid chord"),
                label: "Search".into(),
                scope: ShortcutScope::Local,
            });
            crate::focus_scope::register(FocusScopeKey::new(), true);
            crate::focus_handle::bind(&handle);
            with(|t| {
                t.clipboard_write.set(Some("copied".into()));
                t.edit_command_queue.set(vec![EditCommand::Copy]);
            });
        })
        .join()
        .expect("the thread's state tears down without a panic");
    }

    struct Inert;

    impl Widget for Inert {
        fn layout(&mut self, _ctx: &mut LayoutCtx, _bc: &BoxConstraints) -> Size {
            Size::ZERO
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    }

    /// The first id each per-thread counter hands out on the calling thread.
    fn first_ids() -> (u64, u64, u64, u64) {
        (
            OverlayKey::next().value(),
            ShortcutId::next().value(),
            FocusScopeKey::new().value(),
            ChildPod::new(Box::new(Inert)).inspect_id().0,
        )
    }

    #[test]
    fn id_counters_start_afresh_on_every_thread() {
        let initial = (1, 1, 1, ChildPod::INSPECT_ID_BASE);
        let _ = first_ids();
        for _ in 0..2 {
            let ids = thread::spawn(first_ids).join().expect("no panic");
            assert_eq!(ids, initial, "each thread's counters are its own");
        }
    }
}
