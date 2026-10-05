//! [`ScrollController`]: a cloneable, reactive-free handle onto a
//! [`ScrollView`](crate::ScrollView) or a [`ListView`](crate::ListView) — the
//! programmatic-scroll seam.
//!
//! App code keeps a handle in its state, attaches it with
//! [`ScrollView::controller`](crate::ScrollView::controller) or
//! [`ListView::controller`](crate::ListView::controller), and drives or reads
//! the surface from anywhere on the UI thread, inside a dispatch or not:
//!
//! - **Writes are recorded, not applied.** [`ScrollController::jump_to`],
//!   [`ScrollController::animate_to`] and [`ScrollController::scroll_to_item`]
//!   queue a command the attached widget drains at the start of its next
//!   layout or paint (whichever runs first — a `ListView` also drains at its
//!   next rebuild, before it plans the window, so the jump's frame already
//!   materializes the rows it lands on), so the clamp is always taken against
//!   the freshly laid-out extent — a jump issued in the same handler that
//!   appended content lands on the new bottom, not the old one. A command
//!   queued before any surface attaches (or before the attached one has laid
//!   out) waits for the first layout. Recording raises
//!   [`frust_core::mark_pending_result_flush`] so a frame-gated shell runs the
//!   frame that applies it even when nothing else changed (the
//!   `PanZoomController`/`NavigatorController` precedent).
//! - **Reads are the last published snapshot.** [`ScrollController::offset`],
//!   [`ScrollController::max_offset`] and [`ScrollController::viewport_extent`]
//!   return what the attached surface last published — after every layout,
//!   paint and event pass that changed it — and [`ScrollController::on_change`]
//!   listeners hear each new [`ScrollInfo`] as it is published.
//! - **One surface per handle.** The most recent attach wins: a second
//!   surface attaching the same handle takes it over and the first stops
//!   draining or publishing; dropping the attached widget detaches it.
//! - **`is_animating` tracks the current binding, never a stale one.**
//!   [`ScrollController::is_animating`] is true only while the *currently
//!   attached* binding's surface has a live controller-driven
//!   [`ScrollController::animate_to`] tween running — never a snapshot of
//!   whatever the handle last happened to report. Both places current-ness
//!   changes reconcile it so a detached or superseded binding can never
//!   leave it stuck true: [`ScrollController::bind`] resets it for the newly
//!   attached binding, and dropping the binding that was still current
//!   clears it (`impl Drop for ScrollBinding`) even when the widget itself
//!   never got to call [`ScrollController::is_animating`]'s setter —
//!   covering a tween whose surface is dropped, taken over, or rebound
//!   mid-flight. A widget whose tween genuinely survives a rebind to a
//!   different handle (the tween belongs to the widget, not the handle)
//!   republishes its live state onto the freshly bound handle right after
//!   binding, so a real in-flight tween is still reported, just under the
//!   new current binding instead of the old one.
//! - **Item commands are list-only.** [`ScrollController::scroll_to_item`]
//!   names a row by its [`ChildKey`], which only a keyed
//!   [`ListView`](crate::ListView) can resolve. The queue therefore holds a
//!   wider crate-internal command type than the offset-only `ScrollCommand`
//!   a `ScrollView` matches on: the `ScrollView` drain filters item commands
//!   out (ignoring each with a debug-build log) and keeps the offset ones in
//!   recording order, so `ScrollView` never sees — and never has to match
//!   on — a variant it cannot act on.
//! - **The queue is bounded, not an unbounded backlog.** Offset commands
//!   are grouped into segments by the item commands between them, and
//!   within a segment a `jump_to` supersedes every earlier offset command
//!   (it cancels any tween and sets the position outright) while an
//!   `animate_to` supersedes an earlier `animate_to` but keeps a preceding
//!   `jump_to`, so the tween still starts from the jumped-to position. A
//!   segment therefore holds at most one jump and one animate in whatever
//!   order they were recorded — a tight burst (a pointer-drag thumb, say) or
//!   an alternating jump/animate loop both leave at most two entries — and
//!   superseding never crosses a [`ScrollController::scroll_to_item`] (an
//!   offset/item interleaving stays in exact recording order). Item commands
//!   keep their own ordered channel but are capped at
//!   [`ITEM_COMMAND_CAP`] entries; recording one past the cap drops the
//!   *oldest* queued item command (debug-build log) rather than growing
//!   further, so the whole queue never exceeds the cap plus two offset
//!   entries per segment. One behavioural consequence: a superseded
//!   `jump_to` that would, had it actually applied, transiently cross a
//!   `ListView` near-start/near-end threshold never does — the edge
//!   callbacks evaluate only the surviving target, not every value a caller
//!   recorded.
//! - **The pre-attach backlog survives a bind.** [`ScrollController::bind`]
//!   does not clear the queue: a command recorded before any surface
//!   attaches (or before the newly attached one has laid out) waits for that
//!   surface's first layout, per the apply contract above. The hazard this
//!   trades for: a handle driven in a loop while nothing is attached cannot
//!   grow without bound either — superseding and the item cap still apply —
//!   but it does accumulate up to that bound (at most one jump and one
//!   animate per segment, up to [`ITEM_COMMAND_CAP`] item commands) rather
//!   than draining as it is issued.
//!
//! See `docs/WIDGETS_ARCHITECTURE.md`'s *Scroll Physics* for the surface the
//! handle drives.

use std::cell::RefCell;
use std::fmt;
use std::rc::{Rc, Weak};

use frust_core::Curve;

use crate::ChildKey;
use crate::scroll::ScrollInfo;

/// A command recorded on a [`ScrollController`], drained by the attached
/// widget in recording order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ScrollCommand {
    /// Move to this offset at once, clamped to `[0, max_offset]`.
    JumpTo(f64),
    /// Ease to this offset over the paired [`AnimateTo`]'s duration/curve,
    /// clamped to `[0, max_offset]`.
    AnimateTo(f64, AnimateTo),
}

/// Every command a [`ScrollController`] queues: an offset command any
/// surface applies, or an item command only a keyed
/// [`ListView`](crate::ListView) can resolve. Kept apart from
/// [`ScrollCommand`] so the `ScrollView` drain — which matches exhaustively on
/// that offset-only enum — never has to grow an arm for a key it has no
/// rows to resolve against (see the [module docs](self)).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ControllerCommand {
    /// A jump/animate to an offset, applied by every surface.
    Offset(ScrollCommand),
    /// Bring a keyed row into view — applied by a keyed `ListView`, ignored
    /// (with a debug-build log) by a `ScrollView`.
    ScrollToItem(ScrollToItem),
}

/// A recorded [`ScrollController::scroll_to_item`] request.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ScrollToItem {
    /// The row's stable identity, as the list's `key_of` produces it.
    pub(crate) key: ChildKey,
    /// Where in the viewport the row should land.
    pub(crate) alignment: ItemAlignment,
    /// Ease there (through the same tween `animate_to` runs) rather than jump.
    pub(crate) animated: bool,
}

/// Where [`ScrollController::scroll_to_item`] lands a row in the viewport.
///
/// Every alignment is clamped to the list's `[0, max_offset]` range, so a row
/// too near either end of the content to reach the requested position stops
/// at the edge instead (the first row can never be centered).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ItemAlignment {
    /// The row's top edge at the viewport's top edge.
    #[default]
    Start,
    /// The row's center at the viewport's center.
    Center,
    /// The row's bottom edge at the viewport's bottom edge.
    End,
    /// The least movement that shows the whole row: nothing at all when it is
    /// already fully visible, [`ItemAlignment::Start`] when it sits above the
    /// viewport (or is taller than it), [`ItemAlignment::End`] when below.
    Nearest,
}

/// The duration/curve [`ScrollController::animate_to`] eases a programmatic
/// scroll through. Both fields are required rather than defaulted — Flutter's
/// `ScrollController.animateTo` keeps `duration`/`curve` required for the
/// same reason: an app names the motion it wants rather than inheriting a
/// guessed one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnimateTo {
    /// How long the tween runs, in milliseconds. A non-positive or
    /// non-finite value (`<= 0.0`, or `NaN`) behaves like
    /// [`ScrollController::jump_to`] — an instant move, no animation —
    /// matching how the app's `reduce_motion` theme flag collapses this call.
    pub duration_ms: f64,
    /// The easing curve the tween runs through.
    pub curve: Curve,
}

/// How many queued [`ControllerCommand::ScrollToItem`] entries
/// [`ScrollController::scroll_to_item`] keeps at once. Recording one past
/// this bound drops the oldest queued item command (debug-build log) — see
/// the [module docs](self).
const ITEM_COMMAND_CAP: usize = 8;

/// A subscribed [`ScrollController::on_change`] listener.
type Listener = Rc<dyn Fn(ScrollInfo)>;

/// The state every clone of a [`ScrollController`] shares with the widget
/// it is attached to.
struct Shared {
    /// The last [`ScrollInfo`] the attached surface published.
    info: ScrollInfo,
    /// The attached surface's last laid-out extent along the scroll axis.
    viewport_extent: f64,
    /// The token of the binding currently attached, or `None`.
    attached: Option<u64>,
    /// The token the next [`ScrollController::bind`] hands out.
    next_token: u64,
    /// Commands recorded and not yet drained, in recording order. Bounded —
    /// see [`ScrollController::record`] and the [module docs](self).
    commands: Vec<ControllerCommand>,
    /// Subscribed listeners, keyed by the id their guard removes them by.
    listeners: Vec<(u64, Listener)>,
    /// The id the next [`ScrollController::on_change`] hands out.
    next_listener: u64,
    /// Whether the *currently attached* binding's surface is running a
    /// controller-driven [`ScrollController::animate_to`] tween —
    /// [`ScrollController::is_animating`]'s backing state. Unlike
    /// `info`/`viewport_extent`, this is a liveness flag, not a last-known
    /// snapshot: it is true only while the current binding has a live tween,
    /// and is reconciled back to `false` whenever current-ness changes
    /// without the widget itself clearing it first — [`ScrollController::bind`]
    /// resets it for the newly attached binding, and dropping the binding
    /// that was current clears it too (see `impl Drop for ScrollBinding`).
    animating: bool,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            info: ScrollInfo {
                offset: 0.0,
                max_offset: 0.0,
                overscroll: 0.0,
            },
            viewport_extent: 0.0,
            attached: None,
            next_token: 0,
            commands: Vec::new(),
            listeners: Vec::new(),
            next_listener: 0,
            animating: false,
        }
    }
}

/// A cloneable handle onto a [`ScrollView`](crate::ScrollView) or a
/// [`ListView`](crate::ListView): drive it with [`jump_to`](Self::jump_to)/
/// [`animate_to`](Self::animate_to) (and, on a keyed list,
/// [`scroll_to_item`](Self::scroll_to_item)), read where it stands with
/// [`offset`](Self::offset)/[`max_offset`](Self::max_offset)/
/// [`viewport_extent`](Self::viewport_extent), and observe it with
/// [`on_change`](Self::on_change). Every clone shares one state, so the handle
/// an app keeps in its state and the one attached to the view are the same.
/// See the [module docs](self) for the apply/publish contract.
///
/// ```
/// use frust_widgets::{ScrollController, ScrollView, scroll_view, text};
/// let controller = ScrollController::new();
/// let view: ScrollView<()> = scroll_view(text("hi")).controller(controller.clone());
/// controller.jump_to(120.0); // applied at the view's next layout or paint
/// # let _ = view;
/// ```
#[derive(Clone, Default)]
pub struct ScrollController {
    shared: Rc<RefCell<Shared>>,
}

impl fmt::Debug for ScrollController {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let shared = self.shared.borrow();
        f.debug_struct("ScrollController")
            .field("info", &shared.info)
            .field("viewport_extent", &shared.viewport_extent)
            .field("attached", &shared.attached.is_some())
            .field("pending_commands", &shared.commands.len())
            .field("listeners", &shared.listeners.len())
            .field("animating", &shared.animating)
            .finish()
    }
}

/// Apply the superseding rule to the offset commands of one segment: the
/// entries from `start` up to the next item command (or the end). Walking
/// them in recording order, a `JumpTo` drops everything the segment held
/// before it and an `AnimateTo` drops an earlier `AnimateTo`, so what
/// survives is at most one jump followed by at most one animate. Called
/// after every offset record and after an item drop merges two segments.
fn supersede_segment(commands: &mut Vec<ControllerCommand>, start: usize) {
    let end = commands[start..]
        .iter()
        .position(|command| matches!(command, ControllerCommand::ScrollToItem(_)))
        .map_or(commands.len(), |index| start + index);
    let mut jump = None;
    let mut animate = None;
    for command in commands.drain(start..end) {
        match command {
            ControllerCommand::Offset(ScrollCommand::JumpTo(_)) => {
                jump = Some(command);
                animate = None;
            }
            ControllerCommand::Offset(ScrollCommand::AnimateTo(..)) => animate = Some(command),
            ControllerCommand::ScrollToItem(_) => {
                unreachable!("the segment ends before the next item command")
            }
        }
    }
    for (offset, command) in [jump, animate].into_iter().flatten().enumerate() {
        commands.insert(start + offset, command);
    }
}

impl ScrollController {
    /// A controller attached to nothing yet, reading offset, extent and
    /// viewport `0.0`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Scroll to `offset` (px scrolled down) at once.
    ///
    /// Applied by the attached surface at the start of its next layout or
    /// paint, clamped to `[0, max_offset]` against that pass's extent; it stops
    /// any fling, release-settle or ballistic simulation in flight. `NaN` is
    /// ignored; `f64::INFINITY` lands on the bottom edge. Issued while nothing
    /// is attached (or before the attached surface has laid out), it waits for
    /// the first layout of the next surface to attach.
    pub fn jump_to(&self, offset: f64) {
        self.record(ControllerCommand::Offset(ScrollCommand::JumpTo(offset)));
    }

    /// Ease to `offset` over `options.duration_ms`/`options.curve`, clamped
    /// to `[0, max_offset]`.
    ///
    /// Applied — like [`ScrollController::jump_to`] — at the attached
    /// surface's next layout or paint, and runs through the same ballistic
    /// driver a release fling does, so paint cadence, `request_frame` and the
    /// installed physics' boundary handling are shared rather than
    /// duplicated. A later `jump_to`/`animate_to` recorded before this one
    /// finishes replaces it outright (drained in recording order, so the
    /// later command's own reset always runs after this one's). Any user
    /// pointer `Down` or wheel input on the surface cancels it too, leaving
    /// the surface wherever it had eased to — user input always wins. The
    /// app's `motion.reduce_motion` theme flag collapses this to an instant
    /// jump. `NaN` is ignored.
    pub fn animate_to(&self, offset: f64, options: AnimateTo) {
        self.record(ControllerCommand::Offset(ScrollCommand::AnimateTo(
            offset, options,
        )));
    }

    /// Scroll a keyed [`ListView`](crate::ListView) so the row whose key is
    /// `key` lands at `alignment` in the viewport — jumping there, or easing
    /// there when `animated` is `true`.
    ///
    /// `key` is the same identity the list's
    /// [`builder_keyed`](crate::ListView::builder_keyed) `key_of` returns for
    /// the row (anything that converts into a [`ChildKey`]: an id, a name, a
    /// `ChildKey` itself). Applied like [`ScrollController::jump_to`] — at the
    /// attached list's next rebuild, layout or paint, replacing any jump or
    /// animation in flight — and resolved then against the list's current
    /// data:
    ///
    /// - **Resolution is O(item count) at worst.** The list keeps no
    ///   key-to-index map beyond its materialized window, so a key outside the
    ///   window is found by calling `key_of` over the items in order. One scan
    ///   per request (and per settle frame for a row that left the window),
    ///   never per frame.
    /// - **Estimated, then corrected.** In variable-extent mode
    ///   ([`ListView::estimated_item_extent`](crate::ListView::estimated_item_extent))
    ///   the row's position is computed from the heights measured so far and
    ///   the estimate for every row not yet laid out. Once the jump lands (or
    ///   the animation finishes), each following layout measures the rows now
    ///   on screen, re-resolves the row's aligned position and corrects to it,
    ///   for at most a few frames (the list's own docs state the bound) — so
    ///   the row ends aligned even when the rows ahead of it were never
    ///   measured. A user `Down`, wheel or `Cancel` ends the request at once.
    /// - **An unknown key is a no-op.** A key no row carries — or any key on a
    ///   positional [`ListView::builder`](crate::ListView::builder) list, which
    ///   has no keys — leaves the list where it is and reports itself only
    ///   through a debug-build log, since a recorded command has no caller left
    ///   to return a `Result` to.
    /// - **Lists only.** A [`ScrollView`](crate::ScrollView) holding this handle
    ///   ignores the request (debug-build log); its offset commands are
    ///   unaffected.
    ///
    /// `animated` eases through the same tween [`ScrollController::animate_to`]
    /// runs, timed from the theme's motion tokens (`motion.durations.slow` and
    /// `motion.easing.spatial`, the neutral scheme's values when no theme is
    /// threaded); the theme's `motion.reduce_motion` flag collapses it to a
    /// jump.
    pub fn scroll_to_item(
        &self,
        key: impl Into<ChildKey>,
        alignment: ItemAlignment,
        animated: bool,
    ) {
        self.record(ControllerCommand::ScrollToItem(ScrollToItem {
            key: key.into(),
            alignment,
            animated,
        }));
    }

    /// Whether the *currently attached* surface is running an
    /// [`animate_to`](Self::animate_to) tween — `false` once it completes, is
    /// replaced by a later jump/animate, or is interrupted by user input;
    /// `false` before any surface has ever started one; and `false` once the
    /// surface that was running one is dropped, taken over, or rebinds to a
    /// different handle (see the [module docs](self) for the reconciliation
    /// that guarantees this).
    pub fn is_animating(&self) -> bool {
        self.shared.borrow().animating
    }

    /// The clamped offset the attached surface last published (`0.0` until a
    /// surface attaches and lays out; the last published value after it
    /// detaches).
    pub fn offset(&self) -> f64 {
        self.shared.borrow().info.offset
    }

    /// The maximum offset (`content − viewport`, never negative) the attached
    /// surface last published.
    pub fn max_offset(&self) -> f64 {
        self.shared.borrow().info.max_offset
    }

    /// The attached surface's last laid-out extent along the scroll axis (its
    /// height, for the vertical [`ScrollView`](crate::ScrollView)).
    pub fn viewport_extent(&self) -> f64 {
        self.shared.borrow().viewport_extent
    }

    /// Whether a surface currently holds this handle.
    pub fn is_attached(&self) -> bool {
        self.shared.borrow().attached.is_some()
    }

    /// Call `listener` with every [`ScrollInfo`] the attached surface
    /// publishes that differs from the previous one — user drag, wheel,
    /// fling/settle frames, relayout and applied jumps alike.
    ///
    /// The listener runs with no `&mut State` (it may fire during layout or
    /// paint), so it reacts by writing a signal or recording into the handle,
    /// never by mutating app state directly. It stays subscribed for as long
    /// as the returned [`ScrollSubscription`] lives.
    pub fn on_change<F: Fn(ScrollInfo) + 'static>(&self, listener: F) -> ScrollSubscription {
        let mut shared = self.shared.borrow_mut();
        let id = shared.next_listener;
        shared.next_listener += 1;
        shared.listeners.push((id, Rc::new(listener)));
        ScrollSubscription {
            shared: Rc::downgrade(&self.shared),
            id,
        }
    }

    /// Queue `command` and raise [`frust_core::mark_pending_result_flush`] so a
    /// frame runs to apply it even when it came from outside any input path.
    ///
    /// Within the current segment — the offset commands recorded since the
    /// last item command — a `jump_to` supersedes every earlier offset
    /// command (it cancels any tween and sets the position outright, so
    /// nothing before it can still matter) and an `animate_to` supersedes an
    /// earlier `animate_to` but keeps a preceding `jump_to`, its start
    /// position. A segment therefore never holds more than one jump and one
    /// animate, whatever order a caller records them in. An item command
    /// keeps its own ordered channel, capped at [`ITEM_COMMAND_CAP`] (oldest
    /// dropped past the cap, debug-build log). See the [module docs](self)
    /// for the policy this implements.
    fn record(&self, command: ControllerCommand) {
        let mut shared = self.shared.borrow_mut();
        match command {
            ControllerCommand::Offset(offset) => {
                let segment_start = shared
                    .commands
                    .iter()
                    .rposition(|command| matches!(command, ControllerCommand::ScrollToItem(_)))
                    .map_or(0, |index| index + 1);
                shared.commands.push(ControllerCommand::Offset(offset));
                supersede_segment(&mut shared.commands, segment_start);
            }
            ControllerCommand::ScrollToItem(item) => {
                shared.commands.push(ControllerCommand::ScrollToItem(item));
                let item_count = shared
                    .commands
                    .iter()
                    .filter(|command| matches!(command, ControllerCommand::ScrollToItem(_)))
                    .count();
                if item_count > ITEM_COMMAND_CAP {
                    let oldest = shared
                        .commands
                        .iter()
                        .position(|command| matches!(command, ControllerCommand::ScrollToItem(_)))
                        .expect("item_count > 0 implies at least one ScrollToItem entry");
                    if let ControllerCommand::ScrollToItem(dropped) = shared.commands.remove(oldest)
                    {
                        drop_oldest_item_command(&dropped);
                    }
                    // Dropping the oldest item merges the two segments around
                    // it; re-apply the superseding rule so the merged segment
                    // is bounded like every other one.
                    supersede_segment(&mut shared.commands, 0);
                }
            }
        }
        drop(shared);
        frust_core::mark_pending_result_flush();
    }

    /// Whether `self` and `other` are clones of one handle.
    pub(crate) fn same(&self, other: &ScrollController) -> bool {
        Rc::ptr_eq(&self.shared, &other.shared)
    }

    /// Attach a surface, taking the handle over from whichever one held it.
    /// Resets `animating` to `false` for the new binding — a stale tween
    /// reported by whichever surface held the handle before never leaks
    /// into the new attach; a widget whose tween genuinely survives the
    /// rebind (see `ScrollWidget`/`ListViewWidget::attach_controller`)
    /// republishes its live state on the freshly bound handle right after.
    pub(crate) fn bind(&self) -> ScrollBinding {
        let mut shared = self.shared.borrow_mut();
        let token = shared.next_token;
        shared.next_token += 1;
        shared.attached = Some(token);
        shared.animating = false;
        ScrollBinding {
            controller: self.clone(),
            token,
        }
    }
}

/// Keeps an [`ScrollController::on_change`] listener subscribed; dropping it
/// unsubscribes. Holds the handle weakly, so it never keeps a controller alive.
#[must_use = "dropping a ScrollSubscription unsubscribes its listener at once"]
pub struct ScrollSubscription {
    shared: Weak<RefCell<Shared>>,
    id: u64,
}

impl fmt::Debug for ScrollSubscription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScrollSubscription")
            .field("id", &self.id)
            .finish()
    }
}

impl Drop for ScrollSubscription {
    fn drop(&mut self) {
        if let Some(shared) = self.shared.upgrade() {
            shared
                .borrow_mut()
                .listeners
                .retain(|(id, _)| *id != self.id);
        }
    }
}

/// A surface's claim on a [`ScrollController`]. Only the most recent binding
/// is *current*; an older one (taken over by a later attach) neither drains
/// nor publishes. Dropping the current binding detaches the handle.
pub(crate) struct ScrollBinding {
    controller: ScrollController,
    token: u64,
}

impl ScrollBinding {
    /// The handle this binding claims.
    pub(crate) fn controller(&self) -> &ScrollController {
        &self.controller
    }

    /// Whether this binding still holds the handle.
    pub(crate) fn is_current(&self) -> bool {
        self.controller.shared.borrow().attached == Some(self.token)
    }

    /// Whether this binding holds the handle and has commands to drain.
    pub(crate) fn has_pending(&self) -> bool {
        let shared = self.controller.shared.borrow();
        shared.attached == Some(self.token) && !shared.commands.is_empty()
    }

    /// Take every queued **offset** command, in recording order — empty unless
    /// this binding is current. The `ScrollView` drain: an item command
    /// ([`ScrollController::scroll_to_item`]) has no rows to resolve against
    /// here, so it is dropped with a debug-build log while the offset commands
    /// around it keep their order.
    pub(crate) fn take_commands(&self) -> Vec<ScrollCommand> {
        self.take_all_commands()
            .into_iter()
            .filter_map(|command| match command {
                ControllerCommand::Offset(command) => Some(command),
                ControllerCommand::ScrollToItem(item) => {
                    ignore_item_command(&item);
                    None
                }
            })
            .collect()
    }

    /// Take every queued command, item commands included, in recording order
    /// — empty unless this binding is current. The `ListView` drain.
    pub(crate) fn take_all_commands(&self) -> Vec<ControllerCommand> {
        let mut shared = self.controller.shared.borrow_mut();
        if shared.attached != Some(self.token) {
            return Vec::new();
        }
        std::mem::take(&mut shared.commands)
    }

    /// Publish the surface's current position, firing every listener when
    /// `info` differs from the last published value. A no-op unless this
    /// binding is current. Listeners run after the shared state is released,
    /// so one may read the handle or record a command re-entrantly.
    pub(crate) fn publish(&self, info: ScrollInfo, viewport_extent: f64) {
        let listeners: Vec<Listener> = {
            let mut shared = self.controller.shared.borrow_mut();
            if shared.attached != Some(self.token) {
                return;
            }
            shared.viewport_extent = viewport_extent;
            if shared.info == info {
                return;
            }
            shared.info = info;
            shared
                .listeners
                .iter()
                .map(|(_, listener)| Rc::clone(listener))
                .collect()
        };
        for listener in listeners {
            listener(info);
        }
    }

    /// Set whether a controller-driven [`ScrollController::animate_to`]
    /// tween is live, if this binding is still current — a no-op once a
    /// later attach took the handle over. Mirrors [`ScrollBinding::publish`]'s
    /// current-binding guard but fires no listener: `is_animating` is a
    /// plain poll, not an `on_change` event.
    pub(crate) fn set_animating(&self, animating: bool) {
        let mut shared = self.controller.shared.borrow_mut();
        if shared.attached != Some(self.token) {
            return;
        }
        shared.animating = animating;
    }
}

/// Report an item command a surface with no keyed rows dropped — a wiring gap
/// worth hearing about in a debug build, not an error a release build can act
/// on (the `selection_toolbar` precedent).
fn ignore_item_command(_item: &ScrollToItem) {
    #[cfg(debug_assertions)]
    eprintln!(
        "frust-widgets: ScrollController::scroll_to_item({:?}, {:?}) ignored: the attached \
         surface is a ScrollView, which has no keyed rows (attach the handle to a keyed \
         ListView instead)",
        _item.key, _item.alignment
    );
}

/// Report an item command [`ScrollController::record`] dropped to stay within
/// [`ITEM_COMMAND_CAP`] — a wiring gap (something recording `scroll_to_item`
/// faster than the surface drains it) worth hearing about in a debug build,
/// mirroring [`ignore_item_command`].
fn drop_oldest_item_command(_item: &ScrollToItem) {
    #[cfg(debug_assertions)]
    eprintln!(
        "frust-widgets: ScrollController::scroll_to_item({:?}, {:?}) dropped: more than \
         {ITEM_COMMAND_CAP} item commands were queued with nothing draining them",
        _item.key, _item.alignment
    );
}

/// Detaches the handle when the dropped binding is still current, clearing
/// both `attached` and `animating` in the same branch — a binding taken over
/// by a later [`ScrollController::bind`] is already stale (its drop touches
/// neither field, since `attached` no longer names its token and `animating`
/// already belongs to whichever binding took over). This is what keeps
/// [`ScrollController::is_animating`] from reporting a tween that ended only
/// because its surface was dropped mid-flight — the widget's own
/// `stop_ballistic` never runs in that case, so nothing else would clear it.
impl Drop for ScrollBinding {
    fn drop(&mut self) {
        let mut shared = self.controller.shared.borrow_mut();
        if shared.attached == Some(self.token) {
            shared.attached = None;
            shared.animating = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use frust_core::{BoxConstraints, BuildCtx, LayoutCtx, View, Widget};
    use kurbo::Size;

    use super::*;
    use crate::test_support::leaf;
    use crate::{ScrollView, scroll_view};

    #[test]
    fn the_scroll_view_drain_drops_item_commands_and_keeps_offset_order() {
        let controller = ScrollController::new();
        let binding = controller.bind();
        controller.jump_to(10.0);
        controller.scroll_to_item(ChildKey::new(7u64), ItemAlignment::Center, true);
        controller.jump_to(20.0);
        assert!(binding.has_pending());
        assert_eq!(
            binding.take_commands(),
            vec![ScrollCommand::JumpTo(10.0), ScrollCommand::JumpTo(20.0)],
            "the item command is filtered out, the offset ones keep their order"
        );
        assert!(
            !binding.has_pending(),
            "the item command was drained, not left queued"
        );
    }

    #[test]
    fn the_list_drain_keeps_every_command_in_recording_order() {
        let controller = ScrollController::new();
        let binding = controller.bind();
        controller.scroll_to_item("row", ItemAlignment::Nearest, false);
        controller.jump_to(5.0);
        assert_eq!(
            binding.take_all_commands(),
            vec![
                ControllerCommand::ScrollToItem(ScrollToItem {
                    key: ChildKey::new("row"),
                    alignment: ItemAlignment::Nearest,
                    animated: false,
                }),
                ControllerCommand::Offset(ScrollCommand::JumpTo(5.0)),
            ]
        );
    }

    #[test]
    fn a_scroll_view_ignores_scroll_to_item() {
        let controller = ScrollController::new();
        let view: ScrollView<()> = scroll_view(leaf(200.0, 1000.0)).controller(controller.clone());
        let mut counter = 0u64;
        let mut widget = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        let viewport = BoxConstraints::loose(Size::new(200.0, 100.0));

        controller.scroll_to_item(ChildKey::new(3u64), ItemAlignment::Start, false);
        widget.layout(&mut LayoutCtx::new(), &viewport);
        assert_eq!(
            controller.offset(),
            0.0,
            "an item command moves a ScrollView nowhere"
        );
        assert_eq!(
            controller.max_offset(),
            900.0,
            "but the surface still publishes"
        );

        controller.scroll_to_item(ChildKey::new(3u64), ItemAlignment::Start, false);
        controller.jump_to(250.0);
        widget.layout(&mut LayoutCtx::new(), &viewport);
        assert_eq!(
            widget.offset(),
            250.0,
            "the offset command after it still applies"
        );
        assert_eq!(controller.offset(), 250.0);
    }

    #[test]
    fn a_burst_of_jump_to_calls_coalesces_to_the_last_value() {
        let controller = ScrollController::new();
        let binding = controller.bind();
        for offset in 0..50 {
            controller.jump_to(offset as f64);
        }
        assert_eq!(
            binding.take_all_commands(),
            vec![ControllerCommand::Offset(ScrollCommand::JumpTo(49.0))],
            "a run of same-kind offset commands leaves only the last one queued"
        );
    }

    #[test]
    fn jump_to_then_animate_to_keeps_both_in_either_order() {
        let options = AnimateTo {
            duration_ms: 200.0,
            curve: Curve::Linear,
        };

        let controller = ScrollController::new();
        let binding = controller.bind();
        controller.jump_to(10.0);
        controller.animate_to(20.0, options);
        assert_eq!(
            binding.take_all_commands(),
            vec![
                ControllerCommand::Offset(ScrollCommand::JumpTo(10.0)),
                ControllerCommand::Offset(ScrollCommand::AnimateTo(20.0, options)),
            ],
            "a jump followed by an animate keeps both, so the tween starts from the jumped position"
        );

        let controller = ScrollController::new();
        let binding = controller.bind();
        controller.animate_to(20.0, options);
        controller.jump_to(10.0);
        assert_eq!(
            binding.take_all_commands(),
            vec![ControllerCommand::Offset(ScrollCommand::JumpTo(10.0))],
            "a jump supersedes an earlier animate — it would have cancelled the tween anyway"
        );
    }

    #[test]
    fn alternating_offset_kinds_stay_bounded_to_one_jump_and_one_animate() {
        let options = AnimateTo {
            duration_ms: 200.0,
            curve: Curve::Linear,
        };
        let controller = ScrollController::new();
        let binding = controller.bind();
        for round in 0..50 {
            controller.jump_to(round as f64);
            controller.animate_to(round as f64 + 0.5, options);
        }
        assert_eq!(
            binding.take_all_commands(),
            vec![
                ControllerCommand::Offset(ScrollCommand::JumpTo(49.0)),
                ControllerCommand::Offset(ScrollCommand::AnimateTo(49.5, options)),
            ],
            "whatever order kinds alternate in, a segment keeps one jump and one animate"
        );

        // Item commands break segments, and each segment is bounded on its own.
        let controller = ScrollController::new();
        let binding = controller.bind();
        for round in 0..50 {
            controller.jump_to(round as f64);
            controller.scroll_to_item(ChildKey::new(round as u64), ItemAlignment::Start, false);
        }
        let queued = binding.take_all_commands();
        assert!(
            queued.len() <= 2 * (ITEM_COMMAND_CAP + 1) + ITEM_COMMAND_CAP,
            "the whole queue stays under the item cap plus two offsets per segment: {}",
            queued.len()
        );
        assert_eq!(
            queued
                .iter()
                .filter(|command| matches!(command, ControllerCommand::ScrollToItem(_)))
                .count(),
            ITEM_COMMAND_CAP
        );
    }

    #[test]
    fn scroll_to_item_between_two_jump_to_calls_keeps_all_three_in_order() {
        let controller = ScrollController::new();
        let binding = controller.bind();
        controller.jump_to(10.0);
        controller.scroll_to_item(ChildKey::new(7u64), ItemAlignment::Center, false);
        controller.jump_to(20.0);
        assert_eq!(
            binding.take_all_commands(),
            vec![
                ControllerCommand::Offset(ScrollCommand::JumpTo(10.0)),
                ControllerCommand::ScrollToItem(ScrollToItem {
                    key: ChildKey::new(7u64),
                    alignment: ItemAlignment::Center,
                    animated: false,
                }),
                ControllerCommand::Offset(ScrollCommand::JumpTo(20.0)),
            ],
            "an item command in between stops the surrounding jumps from coalescing"
        );
    }

    #[test]
    fn the_item_command_cap_drops_the_oldest_and_keeps_the_newest() {
        let controller = ScrollController::new();
        let binding = controller.bind();
        for key in 0..(ITEM_COMMAND_CAP as u64 + 3) {
            controller.scroll_to_item(ChildKey::new(key), ItemAlignment::Start, false);
        }
        let drained = binding.take_all_commands();
        assert_eq!(
            drained.len(),
            ITEM_COMMAND_CAP,
            "queuing past the cap drops the oldest rather than growing further"
        );
        let expected: Vec<ControllerCommand> = (3..(ITEM_COMMAND_CAP as u64 + 3))
            .map(|key| {
                ControllerCommand::ScrollToItem(ScrollToItem {
                    key: ChildKey::new(key),
                    alignment: ItemAlignment::Start,
                    animated: false,
                })
            })
            .collect();
        assert_eq!(
            drained, expected,
            "the surviving entries are the newest ones, oldest-dropped-first, in recording order"
        );
    }

    #[test]
    fn bind_resets_animating_and_drop_clears_it_only_for_the_current_binding() {
        let controller = ScrollController::new();
        let first = controller.bind();
        first.set_animating(true);
        assert!(controller.is_animating(), "the current binding set it");

        // A fresh bind resets the flag, even though the just-bound-over
        // first binding never ran its own tear-down.
        let second = controller.bind();
        assert!(
            !controller.is_animating(),
            "bind() resets animating for the newly attached binding"
        );

        // The second binding can still set its own live state independently
        // of the first's stale `true`.
        second.set_animating(true);
        assert!(controller.is_animating());

        // Dropping the stale, taken-over first binding must not touch the
        // current holder's flag.
        drop(first);
        assert!(
            controller.is_animating(),
            "dropping a non-current binding leaves the current holder's flag alone"
        );
        assert!(
            controller.is_attached(),
            "dropping a non-current binding does not detach the handle"
        );

        // Dropping the current binding clears it.
        drop(second);
        assert!(
            !controller.is_animating(),
            "dropping the current binding clears animating"
        );
        assert!(!controller.is_attached());
    }
}
