//! [`ScrollController`]: a cloneable, reactive-free handle onto a
//! [`ScrollView`](crate::ScrollView) — the programmatic-scroll seam.
//!
//! App code keeps a handle in its state, attaches it with
//! [`ScrollView::controller`](crate::ScrollView::controller), and drives or
//! reads the surface from anywhere on the UI thread, inside a dispatch or not:
//!
//! - **Writes are recorded, not applied.** [`ScrollController::jump_to`] and
//!   [`ScrollController::animate_to`] queue a command the attached widget
//!   drains at the start of its next layout or paint (whichever runs first),
//!   so the clamp is always taken against the freshly laid-out extent — a
//!   jump issued in the same handler that appended content lands on the new
//!   bottom, not the old one. A command queued before any surface attaches
//!   (or before the attached one has laid out) waits for the first layout.
//!   Recording raises [`frust_core::mark_pending_result_flush`] so a
//!   frame-gated shell runs the frame that applies it even when nothing else
//!   changed (the `PanZoomController`/`NavigatorController` precedent).
//! - **Reads are the last published snapshot.** [`ScrollController::offset`],
//!   [`ScrollController::max_offset`] and [`ScrollController::viewport_extent`]
//!   return what the attached surface last published — after every layout,
//!   paint and event pass that changed it — and [`ScrollController::on_change`]
//!   listeners hear each new [`ScrollInfo`] as it is published.
//! - **One surface per handle.** The most recent attach wins: a second
//!   `ScrollView` attaching the same handle takes it over and the first stops
//!   draining or publishing; dropping the attached widget detaches it.
//!
//! See `docs/WIDGETS_ARCHITECTURE.md`'s *Scroll Physics* for the surface the
//! handle drives.

use std::cell::RefCell;
use std::fmt;
use std::rc::{Rc, Weak};

use frust_core::Curve;

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
    /// Commands recorded and not yet drained.
    commands: Vec<ScrollCommand>,
    /// Subscribed listeners, keyed by the id their guard removes them by.
    listeners: Vec<(u64, Listener)>,
    /// The id the next [`ScrollController::on_change`] hands out.
    next_listener: u64,
    /// Whether the attached surface is currently running a controller-driven
    /// [`ScrollController::animate_to`] tween — [`ScrollController::is_animating`]'s
    /// backing state, published under the same "last known state" contract
    /// as `info`/`viewport_extent` rather than reset on detach.
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

/// A cloneable handle onto a [`ScrollView`](crate::ScrollView): drive it with
/// [`jump_to`](Self::jump_to), read where it stands with
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
        self.record(ScrollCommand::JumpTo(offset));
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
        self.record(ScrollCommand::AnimateTo(offset, options));
    }

    /// Whether the attached surface is currently running an
    /// [`animate_to`](Self::animate_to) tween — `false` once it completes, is
    /// replaced by a later jump/animate, or is interrupted by user input, and
    /// `false` before any surface has ever started one.
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
    fn record(&self, command: ScrollCommand) {
        self.shared.borrow_mut().commands.push(command);
        frust_core::mark_pending_result_flush();
    }

    /// Whether `self` and `other` are clones of one handle.
    pub(crate) fn same(&self, other: &ScrollController) -> bool {
        Rc::ptr_eq(&self.shared, &other.shared)
    }

    /// Attach a surface, taking the handle over from whichever one held it.
    pub(crate) fn bind(&self) -> ScrollBinding {
        let mut shared = self.shared.borrow_mut();
        let token = shared.next_token;
        shared.next_token += 1;
        shared.attached = Some(token);
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

    /// Take every queued command, in recording order — empty unless this
    /// binding is current.
    pub(crate) fn take_commands(&self) -> Vec<ScrollCommand> {
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

impl Drop for ScrollBinding {
    fn drop(&mut self) {
        let mut shared = self.controller.shared.borrow_mut();
        if shared.attached == Some(self.token) {
            shared.attached = None;
        }
    }
}
