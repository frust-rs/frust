//! Layer 1+2 of ForgeKit: the declarative view API, the retained widget tree,
//! box-constraint layout, and the rebuild/layout/paint pass skeleton (spec §5,
//! §6).
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
//! * [`tree`] — the [`WidgetTree`](tree::WidgetTree) arena wrapper and
//!   [`WidgetPod`](tree::WidgetPod).
//! * [`app`] — the [`RenderRoot`](app::RenderRoot) that drives rebuild → layout
//!   → paint. This is what the desktop shell (task 08) owns.
//! * [`component`] — [`Component`](component::Component), Flutter's
//!   `StatefulWidget` analog: a subtree with retained local state, a
//!   per-component reactive `Owner`, and a state boundary the outer view tree
//!   never sees.

pub mod anim;
pub mod app;
pub mod component;
pub mod event;
pub mod input;
pub mod layout;
pub mod semantics;
pub mod tree;
pub mod view;
pub mod widget;

/// Re-export of the [`accesskit`] accessibility vocabulary (roles, node
/// builders, toggle/value state) widgets use to populate a
/// [`semantics::SemanticsCtx`]. Re-exported here — the sole crate that depends
/// on accesskit — so `forgekit-widgets` (and app code) name the vocabulary
/// through `forgekit_core::accesskit::*` without a direct dependency; platform
/// `accesskit_*` adapter crates are phase 6d and live in the shells.
pub use accesskit;
pub use anim::{
    AnimationController, AnimationStatus, Curve, FrameTime, Lerp, Spring, SpringDesc, Tween,
};
pub use app::RenderRoot;
pub use component::{Component, ComponentView, ComponentWidget, component};
pub use event::{
    EditingState, EventCtx, EventOutcome, EventResult, ImeEvent, ImeState, InputEvent, Key,
    KeyEvent, Modifiers, NamedKey, PointerButton, PointerEvent, PointerPhase, ScrollDelta,
};
pub use input::{
    FLING_DECAY, FLING_STOP, MOUSE_SLOP, TOUCH_SLOP, VELOCITY_WINDOW_MS, VelocityTracker,
    WHEEL_LINE_PX, fling_decay, fling_displacement,
};
pub use layout::BoxConstraints;
pub use semantics::{SemanticsCtx, SemanticsUpdate};
pub use tree::{WidgetPod, WidgetTree};
pub use view::{AnyView, BuildCtx, ChangeFlags, View, WidgetId, any};
pub use widget::{ChildPod, LayoutCtx, PaintCtx, PaintOutcome, PaintScene, Widget};
