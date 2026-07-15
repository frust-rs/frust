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
//!   layout/paint contexts, plus the [`PaintScene`](widget::PaintScene) paint
//!   boundary.
//! * [`layout`] — the [`BoxConstraints`](layout::BoxConstraints) box model.
//! * [`tree`] — the [`WidgetTree`](tree::WidgetTree) arena wrapper and
//!   [`WidgetPod`](tree::WidgetPod).
//! * [`app`] — the [`RenderRoot`](app::RenderRoot) that drives rebuild → layout
//!   → paint. This is what the desktop shell (task 08) owns.

pub mod app;
pub mod layout;
pub mod tree;
pub mod view;
pub mod widget;

pub use app::RenderRoot;
pub use layout::BoxConstraints;
pub use tree::{WidgetPod, WidgetTree};
pub use view::{BuildCtx, ChangeFlags, View, WidgetId};
pub use widget::{LayoutCtx, PaintCtx, PaintScene, Widget};
