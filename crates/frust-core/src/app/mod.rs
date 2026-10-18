//! The render root: the object each platform shell drives each frame.
//!
//! [`RenderRoot`] owns the retained [`WidgetTree`] and the previous [`View`], and runs the
//! [`RenderRoot::rebuild`], [`RenderRoot::layout`], [`RenderRoot::paint`] and
//! [`RenderRoot::event`] passes.
//!
//! Single-root: the build closure returns one `impl View<State>` of a fixed type, stored typed.

mod config;
mod dispatch;
mod layout;
mod metrics;
mod paint;
mod rebuild;
mod semantics;
mod session;
#[cfg(test)]
mod tests;

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::num::NonZeroU64;
use std::rc::Weak;
use std::sync::atomic::{AtomicU64, Ordering};

use kurbo::{Rect, Size};

use crate::event::{CursorIcon, EventOutcome, ImeState, PointerId};
use crate::insets::WindowInsets;
use crate::overlay::{OverlayHit, OverlayKey};
use crate::selection_toolbar::SelectionToolbarRequest;
use crate::shortcuts::{ShortcutHint, ShortcutId, ShortcutTable};
use crate::tree::WidgetTree;
use crate::view::{ChangeFlags, View, WidgetId};
use crate::widget::{ChildPod, PlatformViewFrame};

pub use config::FocusRingPolicy;
use layout::LayoutAudit;
pub use metrics::{Orientation, WindowMetrics};

/// Rebuild flush passes per frame: a result that navigates once, plus that page's own result.
const MAX_PENDING_RESULT_FLUSH_PASSES: usize = 3;

/// Allocates `root_identity` (`0` = no root); module-level so distinct `<State, V>` never collide.
static NEXT_ROOT_IDENTITY: AtomicU64 = AtomicU64::new(1);

/// The next focus epoch after `epoch`, skipping `0` (a never-claimed pod's stamp).
fn advance_focus_epoch(epoch: u64) -> u64 {
    match epoch.wrapping_add(1) {
        0 => 1,
        next => next,
    }
}

/// What [`RenderRoot::event`]'s overlay pre-pass decided about one incoming event.
enum OverlayRoute {
    /// The overlay layer consumed the event; return this outcome unchanged.
    Consumed(EventOutcome),
    /// The main dispatch continues; merge this outcome into its own.
    Continue(EventOutcome),
}

/// Owns the retained tree and drives the rebuild/layout/paint/event passes of a single-root app.
pub struct RenderRoot<State: 'static, V: View<State>> {
    tree: WidgetTree,
    root_id: Option<WidgetId>,
    prev_view: Option<V>,
    /// Widget-id counter lent to each `BuildCtx`.
    next_id: u64,
    window_size: Size,
    /// The contact whose `Down` claimed the live capture; only its own `Up`/`Cancel` clears it.
    capture_claimant: Option<PointerId>,
    /// Whether the live captor opted into the gesture's other contacts; `false` when uncaptured.
    capture_contacts: bool,
    /// Whether that opt-in came from the root widget itself rather than a pod below it.
    contacts_captor_is_root: bool,
    /// Whether some widget holds focus; set by a focus claim, cleared only by a session release.
    focus_active: bool,
    /// The branch owning the live focus session: `None` for the main tree, `Some(key)` for a
    /// floated surface. Resolved by the paint pass from live links; other passes only clear it.
    focus_surface: Option<OverlayKey>,
    /// The live focus session's identity, stamped beside each pod's focus link; never `0`.
    focus_epoch: u64,
    /// The pod declined keys are forwarded to, with the `focus_epoch` it was recorded under.
    key_forward: Option<(OverlayKey, u64)>,
    /// The carrier id of this root's traversal grants (`widget::FocusGrant`); declared by no one.
    focus_grant: ShortcutId,
    /// Whether traversal granted the live focus; any other claim, a release or a `Down` clears it.
    keyboard_focus: bool,
    focus_ring_policy: FocusRingPolicy,
    /// Whether a declined bare Tab/Shift+Tab runs traversal ([`RenderRoot::set_tab_traversal`]).
    tab_traversal: bool,
    hover_active: bool,
    /// Identity of the latest hover pass; a pod's hover stamp counts only while it equals this.
    hover_epoch: u64,
    /// Process-unique id qualifying this root's thread-local focus and hover channels.
    root_identity: u64,
    cursor: CursorIcon,
    /// Undrained clipboard write; a newer write replaces one nobody has taken yet.
    pending_clipboard_write: Option<String>,
    /// Whether a widget asked to read the host clipboard since the last drain.
    pending_paste_request: bool,
    ime_state: Option<ImeState>,
    /// Bumped on every actual change of `focus_active` or `ime_state`, never on a same-value write.
    focus_ime_gen: u64,
    /// The frames the last paint published; replaced wholesale every pass.
    platform_view_frames: Vec<PlatformViewFrame>,
    /// The z-shield rects the last paint reported; replaced wholesale every pass.
    input_shields: Vec<Rect>,
    /// Dirtiness accumulated since the last [`RenderRoot::take_change_flags`].
    pending: ChangeFlags,
    /// The active theme, type-erased so this crate needs no `frust-theme` dependency.
    theme: Option<Box<dyn Any>>,
    insets: WindowInsets,
    /// The shell's presented-frame count: a paint-time observation that dirties nothing.
    presented_frames: Option<u64>,
    /// Whether the GPU surface is translucent ("Mode B"); read by the platform-view hole-punch.
    surface_translucent: bool,
    /// Next never-reused semantics base id; starts at `2` (`0` keeps `NonZeroU64` valid, `1` is
    /// the window node). A `Cell` because [`RenderRoot::semantics`] takes `&self`.
    semantics_alloc: Cell<u64>,
    root_semantics_id: Cell<Option<NonZeroU64>>,
    /// Bumped when a rebuild, theme swap or inset change may have changed the semantics tree.
    semantics_gen: u64,
    /// Set when the deferred-callback flush owes a frame; the next paint reports `needs_frame`.
    deferred_frame: bool,
    /// The root widget's own trailing-paint bit, since it sits in no pod.
    painted_animating: bool,
    /// Routing half of the overlay entries the last paint registered, in paint order; no pods.
    overlay_hits: Vec<OverlayHit>,
    /// The shortcut declarations the last paint made, matched against unhandled keys.
    shortcuts: ShortcutTable,
    /// The focused field's selection-toolbar request from the last paint; a release clears it.
    selection_toolbar: Option<SelectionToolbarRequest>,
    /// Bumped on every actual change of `selection_toolbar`, a clear included.
    selection_toolbar_gen: u64,
    /// Bumped by a theme swap, an inset change, a resize, or a dirty floated surface.
    layout_epoch: u64,
    /// A pod painted under another paint epoch is never reported clean.
    paint_epoch: u64,
    /// Serial of the most recent paint (`0` before the first).
    paint_serial: u64,
    /// The hover claimant's `inspect_id` (`0` for the root widget); `None` while nothing hovers.
    hover_link: Option<u64>,
    /// `(hover_link, focus_epoch)` as the last paint saw them; a change bumps `paint_epoch`.
    painted_links: (Option<u64>, u64),
    retained_layers: bool,
    /// The renderer's resident-key ledger as of the previous frame.
    retained_ledger: crate::retained::RetainedLedger,
    layout_skip: bool,
    layout_audit: LayoutAudit,
    /// Weak handles to the floated pods the last paint drew. Their owners lay them out, so
    /// [`RenderRoot::layout`] bumps the layout epoch when any is dirty.
    overlay_pods: Vec<Weak<RefCell<ChildPod>>>,
    _state: core::marker::PhantomData<fn(&mut State)>,
}

impl<State: 'static, V: View<State>> RenderRoot<State, V> {
    /// Create an empty render root with no widget yet built.
    pub fn new() -> Self {
        Self {
            tree: WidgetTree::new(),
            root_id: None,
            prev_view: None,
            next_id: 0,
            window_size: Size::ZERO,
            capture_claimant: None,
            capture_contacts: false,
            contacts_captor_is_root: false,
            focus_active: false,
            focus_surface: None,
            focus_epoch: 1,
            key_forward: None,
            focus_grant: ShortcutId::next(),
            keyboard_focus: false,
            focus_ring_policy: FocusRingPolicy::KeyboardOnly,
            tab_traversal: true,
            hover_active: false,
            hover_epoch: 1,
            root_identity: NEXT_ROOT_IDENTITY.fetch_add(1, Ordering::Relaxed),
            cursor: CursorIcon::Default,
            pending_clipboard_write: None,
            pending_paste_request: false,
            ime_state: None,
            focus_ime_gen: 0,
            platform_view_frames: Vec::new(),
            input_shields: Vec::new(),
            pending: ChangeFlags::NONE,
            theme: None,
            insets: WindowInsets::default(),
            presented_frames: None,
            surface_translucent: false,
            semantics_alloc: Cell::new(2),
            root_semantics_id: Cell::new(None),
            semantics_gen: 0,
            deferred_frame: false,
            painted_animating: false,
            overlay_hits: Vec::new(),
            shortcuts: ShortcutTable::default(),
            selection_toolbar: None,
            selection_toolbar_gen: 0,
            layout_epoch: 1,
            paint_epoch: 1,
            paint_serial: 0,
            hover_link: None,
            painted_links: (None, 1),
            retained_layers: true,
            retained_ledger: crate::retained::RetainedLedger::empty(),
            layout_skip: true,
            layout_audit: LayoutAudit::default(),
            overlay_pods: Vec::new(),
            _state: core::marker::PhantomData,
        }
    }

    /// The last paint's shortcut declarations `filter` keeps, in paint order: a palette's list.
    pub fn shortcut_hints(&self, filter: impl FnMut(&ShortcutHint) -> bool) -> Vec<ShortcutHint> {
        self.shortcuts.hints(filter)
    }
}

impl<State: 'static, V: View<State>> Default for RenderRoot<State, V> {
    fn default() -> Self {
        Self::new()
    }
}
