//! Layer 1+2 of Frust: the declarative view API, the retained widget tree,
//! box-constraint layout, and the rebuild/layout/paint pass skeleton.
//!
//! The design mirrors `xilem_core`'s proven `View` lifecycle and Masonry's
//! `tree_arena`-backed widget tree, but owns its implementation — there is **no
//! xilem/masonry dependency and no `unsafe`** in this crate.
//!
//! # Layers
//!
//! * [`view`] — layer 1: the [`View`](view::View) trait, [`ChangeFlags`], and
//!   [`BuildCtx`](view::BuildCtx). Views are cheap descriptors produced by
//!   `fn app_logic(&mut State) -> impl View<State>`.
//! * [`widget`] — layer 2: the [`Widget`](widget::Widget) trait and its
//!   layout/paint/event contexts, the [`PaintScene`](widget::PaintScene) paint
//!   boundary, and container-owned [`ChildPod`](widget::ChildPod) children.
//! * [`event`] — layer 2 input: [`InputEvent`](event::InputEvent)/
//!   [`PointerEvent`](event::PointerEvent) and the [`EventCtx`](event::EventCtx)
//!   handlers mutate state through.
//! * [`input`] — pure gesture helpers: slop/wheel constants, a
//!   [`VelocityTracker`](input::VelocityTracker), and the fling-decay math the
//!   interactive widgets build on.
//! * [`layout`] — the [`BoxConstraints`](layout::BoxConstraints) box model.
//! * [`tree`] — the [`WidgetTree`](tree::WidgetTree) arena wrapper,
//!   [`WidgetPod`](tree::WidgetPod), and the read-only
//!   [`InspectNode`](tree::InspectNode) walk tooling reads the tree through.
//! * [`app`] — the [`RenderRoot`](app::RenderRoot) that drives rebuild → layout
//!   → paint. This is what each platform shell owns.
//! * [`component`] — [`Component`](component::Component), Flutter's
//!   `StatefulWidget` analog: a subtree with retained local state, a
//!   per-component reactive `Owner`, and a state boundary the outer view tree
//!   never sees.
//! * [`overlay`] — the overlay portal: an owner-hosted, root-painted,
//!   root-routed pod a widget floats above the whole app (menus, popovers,
//!   tooltips, selection toolbars).
//! * [`selection_toolbar`] — what a text field publishes when it has a
//!   selection, plus the process-global policy/builder slots that decide who
//!   draws the toolbar.

pub mod anim;
pub mod app;
pub mod component;
pub mod event;
pub mod input;
pub mod insets;
pub mod layout;
pub mod overlay;
pub mod selection_toolbar;
pub mod semantics;
pub mod tree;
pub mod view;
pub mod widget;

/// Re-export of the [`accesskit`] accessibility vocabulary (roles, node
/// builders, toggle/value state) widgets use to populate a
/// [`semantics::SemanticsCtx`]. Re-exported here — the sole crate that depends
/// on accesskit — so `frust-widgets` (and app code) name the vocabulary
/// through `frust_core::accesskit::*` without a direct dependency; the
/// platform `accesskit_*` adapter crates live in the shells, not here.
pub use accesskit;
pub use anim::{
    AnimationController, AnimationStatus, Curve, FrameTime, Lerp, Spring, SpringDesc, Tween,
};
pub use app::{Orientation, RenderRoot, WindowMetrics};
pub use component::{Component, ComponentView, ComponentWidget, component};
pub use event::{
    CursorIcon, EditCommand, EditingState, EventCtx, EventOutcome, EventResult, ImeContentType,
    ImeEvent, ImeState, InputEvent, Key, KeyEvent, Modifiers, NamedKey, OverlayEvent,
    OverlayEventKind, PointerButton, PointerEvent, PointerPhase, ScrollDelta,
    has_pending_result_flush, mark_focus_orphaned, mark_pending_result_flush, take_focus_orphaned,
    take_pending_result_flush,
};
pub use input::{
    FLING_DECAY, FLING_STOP, MOUSE_SLOP, TOUCH_SLOP, VELOCITY_WINDOW_MS, VelocityTracker,
    WHEEL_LINE_PX, fling_decay, fling_displacement,
};
pub use insets::{EdgeInsets as WindowEdgeInsets, WindowInsets};
pub use layout::BoxConstraints;
pub use overlay::{
    OutsideTap, OverlayBand, OverlayEntry, OverlayHit, OverlayInput, OverlayKey, OverlayPod,
};
pub use selection_toolbar::{
    SelectionToolbarActions, SelectionToolbarBuilder, SelectionToolbarPolicy,
    SelectionToolbarRequest, install_selection_toolbar_builder_if_unset,
    lock_selection_toolbar_policy, selection_toolbar_builder, selection_toolbar_policy,
    set_selection_toolbar_builder, set_selection_toolbar_policy,
};
pub use semantics::{SemanticsCtx, SemanticsUpdate};
pub use tree::{InspectNode, WidgetPod, WidgetTree};
pub use view::{AnyView, BuildCtx, ChangeFlags, View, WidgetId, any};
pub use widget::{
    ChildPod, CornerRadii, DashPattern, DiscardScene, HeroDirective, HeroFrames, LayoutCtx,
    PaintCtx, PaintOutcome, PaintScene, TickClass, Widget,
};
