//! Facade crate: the public `frust` framework API.
//!
//! App authors depend on this single crate. It exposes [`run`], the canonical
//! app entry point, and curates the view/widget/reactive
//! vocabulary from the underlying framework crates so the declarative call
//! shape reads exactly as the spec promises:
//!
//! ```no_run
//! use frust::{AnyView, Column, Component, View, any, text};
//!
//! // A small stateless widget: a plain fn, generic over the state of
//! // whichever component places it.
//! fn greeting<S: 'static>(message: &str) -> impl View<S> + use<S> {
//!     text(message.to_owned()).size(24.0)
//! }
//!
//! struct Counter;
//!
//! impl Component for Counter {
//!     type State = i32;
//!
//!     fn init(&self) -> i32 {
//!         0
//!     }
//!
//!     // The whole UI lives in `build`; split it into widget fns like
//!     // `greeting` rather than nesting ever deeper inside it.
//!     fn build(&self, state: &mut i32) -> AnyView<i32> {
//!         any(Column(vec![
//!             any(greeting("Hello from Frust")),
//!             any(text(format!("count: {state}")).size(32.0)),
//!         ]))
//!     }
//! }
//!
//! frust::run(Counter).unwrap();
//! ```
//!
//! The composition rule: stateless pieces are plain widget functions called
//! from `build`; a piece with its own state is a child [`Component`] mounted
//! with [`component`], which gives it a private state boundary. [`run`] (or
//! the [`app!`] macro, which also covers the mobile and web shells) starts it.
//!
//! ## Low-level escape hatch: `App::new`
//!
//! [`App`] is the low-level primitive [`run`] and [`app!`] expand into
//! (`App::new(state, move |state| root.build(state))`): a single ambient
//! `State` and a build closure, with no [`Component`] state-boundary or
//! retained local state. Ordinary apps use a [`Component`] with [`run`] or
//! [`app!`]; reach for `App::new` directly only when you need that raw
//! `(state, build)` pair:
//!
//! ```no_run
//! struct AppState {
//!     greeting: String,
//! }
//!
//! let build = |state: &mut AppState| -> frust::AnyView<AppState> {
//!     frust::any(frust::text(state.greeting.clone()).size(32.0))
//! };
//!
//! frust::App::new(AppState { greeting: "Hello from Frust".into() }, build)
//!     .run()
//!     .unwrap();
//! ```
//!
//! ## Layout containers
//!
//! The primitive containers compose heterogeneous children through
//! [`any`] (type erasure) into the declarative call shape:
//!
//! ```
//! use frust::{Align, Alignment, Column, EdgeInsets, Padding, Row, SizedBox, any, text};
//!
//! struct Layouts;
//!
//! impl frust::Component for Layouts {
//!     type State = ();
//!
//!     fn init(&self) {}
//!
//!     fn build(&self, _state: &mut ()) -> frust::AnyView<()> {
//!         any(Column(vec![
//!             any(text("title").size(24.0)),
//!             any(Row(vec![any(text("left")), any(text("right"))])),
//!             any(Padding(EdgeInsets::all(8.0), text("padded"))),
//!             any(Align(Alignment::CENTER, text("centered"))),
//!             any(SizedBox(Some(0.0), Some(12.0))),
//!         ]))
//!     }
//! }
//! # let _ = Layouts;
//! ```

pub use frust_core::component::{Component, ComponentView, component};
pub use frust_core::view::{AnyView, View, any};
// [`NavigatorController::push_with_options`] (an existing method on the
// already-flat-re-exported [`NavigatorController`] below) needed its argument
// type actually constructible from `frust::` — added here as part of the
// baseline re-export list, alongside the other bare navigator vocabulary
// (`NavigatorController`/`NavigatorId`/`PageBuilder`/`PopResult`) it was
// missing:
//
// - [`PushOptions`] carries a pushed page's back-press [`BackPolicy`] (and,
//   for a [`BackPolicy::DismissAnimated`] overlay, the shared dismiss-signal
//   cell) alongside opacity/transition/result.
// - [`PushOptions::on_visibility`]/[`NavigatorController::transition`] hand
//   back [`PageVisibility`]/[`TransitionState`] respectively — both
//   otherwise unnameable without [`PageVisibility`]/[`VisibilityCallback`]/
//   [`TransitionState`] themselves in scope.
//
// This closes a re-export gap: a design-system plugin's own modal helper
// (`frust_glyph::show_glyph_dialog` is the shipped example —
// `push_with_options(.., PushOptions::transparent())` with a custom
// [`BackPolicy`]) depends on exactly this seam, so an app- or plugin-authored
// dialog/sheet could reach the *method* through [`NavigatorController`] but
// never construct a call to it. See this file's
// `push_with_options_dismiss_animated` test module below for the worked
// `BackPolicy::DismissAnimated` + dismiss-signal example — an app-authored
// modal staging its own exit on Android back instead of vanishing.
pub use frust_widgets::{
    Align, AlignView, Alignment, AlwaysScrollable, Axis, BackPolicy, BorderStyle, Bouncing, Button,
    ButtonStyle, ButtonView, Checkbox, CheckboxView, ChildKey, Clamping, Column, ContainerView,
    ContainerWidget, CrossAxisAlignment, DecelerationRate, DividerView, DividerWidget, EdgeInsets,
    FlexChild, FlexView, GestureDetector, GestureDetectorView, HeroView, Icon, IconButton,
    IconButtonView, IconData, IconSource, IconView, IconWidget, Image, ImageError, ImageFit,
    ImageSource, ImageView, ListView, ListViewWidget, MAX_FLING_VELOCITY, MIN_FLING_VELOCITY,
    MainAxisAlignment, NavigatorController, NavigatorId, NavigatorView, NeverScrollable,
    OverlayAlign, OverlayPlacement, OverlayPortalView, OverlaySide, OverscrollEffect, Padding,
    PaddingView, PageBuilder, PageTransition, PageVisibility, PopResult, PushOptions, Radio,
    RadioView, RadioWidget, ResultCallback, Row, RubberBand, SafeAreaView, ScaffoldView,
    ScrollInfo, ScrollMetrics, ScrollPhysics, ScrollView, Simulation, SizedBox, SizedBoxView,
    Slider, SliderView, SpringDescription, Stack, StackView, TextInput, TextInputView, TextView,
    Timing, Tolerance, TransitionSpec, TransitionState, VisibilityCallback, button, checkbox,
    colored_box, container, divider, flexible, hero, icon, icon_button, inflexible, keyed,
    list_view, overlay_portal, radio, safe_area, scaffold, scroll_view, slider, text, text_input,
};

/// The overlay portal's own vocabulary, flat-re-exported from `frust-core`:
/// which z-band a floated surface sits in, whether the pointer reaches it, and
/// what a press landing outside it delivers.
///
/// [`overlay_portal`] floats a surface above the whole app, anchored to its
/// child's bounds and painted after the entire main tree — so it escapes both
/// the child's paint order and every ancestor's clip, and follows the child
/// across scroll and relayout with nothing subscribed. The surface is mounted
/// while [`OverlayPortalView::overlay`] is `Some`; a widget that hosts a
/// surface of its own instead reaches for `authoring::OverlaySlot`.
///
/// Lifted flat for [`EditCommand`]'s reason: an app or design system naming a
/// band, an input class or a light-dismiss policy is configuring a portal, not
/// authoring a widget.
///
/// ```
/// use frust::{
///     OverlayBand, OverlayInput, OverlayPlacement, OverlaySide, View, any, overlay_portal, text,
/// };
///
/// struct App {
///     hovering: bool,
/// }
///
/// // A tooltip: above its trigger, in the topmost band, and transparent to the
/// // pointer so hovering the trigger is never interrupted by its own tip.
/// fn trigger(state: &mut App) -> impl View<App> + use<> {
///     overlay_portal(text("Save"))
///         .overlay(state.hovering.then(|| any(text("Save the document"))))
///         .placement(OverlayPlacement::on(OverlaySide::Top))
///         .band(OverlayBand::Tooltip)
///         .input(OverlayInput::Transparent)
/// }
/// # let _ = trigger;
/// ```
pub use frust_core::{OutsideTap, OverlayBand, OverlayInput};

/// The clipboard/selection command vocabulary, flat-re-exported from
/// `frust-core::event` (and also available through
/// [`authoring::EditCommand`]).
///
/// [`InputEvent::EditCommand`](authoring::InputEvent) delivers one of
/// [`EditCommand::Copy`], [`EditCommand::Cut`], [`EditCommand::Paste`] or
/// [`EditCommand::SelectAll`] down the focus chain, already decoded by the shell
/// from the platform's own gesture — `Cmd+C` on macOS, `Ctrl+C` or `Ctrl+Insert`
/// elsewhere, a hardware clipboard key, an Android `ACTION_PROCESS_TEXT`, an iOS
/// edit-menu tap — so app and design-system code never decodes a chord itself.
///
/// Lifted flat rather than left inside [`authoring`] for [`CursorIcon`]'s reason:
/// a design system's own context menu or toolbar builder names these verbs in
/// its public API without being a widget author. `Paste` carries its text
/// because the shell has already read the host clipboard; its [`Debug`] redacts
/// that text, since a paste payload can be a password or a token.
pub use frust_core::EditCommand;

/// The selection-toolbar seam: the request a text field publishes when it
/// has a selection ([`SelectionToolbarRequest`]/[`SelectionToolbarActions`]),
/// and the knobs that decide who draws it.
///
/// [`set_selection_toolbar_policy`] chooses
/// [`SelectionToolbarPolicy::Framework`] (the default — a field floats its
/// own pod through the overlay portal) or [`SelectionToolbarPolicy::Native`]
/// (the platform's own edit menu, e.g. iOS's `UIEditMenuInteraction`) — see
/// that type's own docs for the two-route split. This is **not** an
/// unconditional free choice, though: a platform shell whose system owns the
/// menu can LOCK it instead, with `frust_core::lock_selection_toolbar_policy`
/// — once locked, [`set_selection_toolbar_policy`] is refused rather than
/// obeyed, so an app cannot silently undo a platform requirement it doesn't
/// know exists. iOS's shell locks [`SelectionToolbarPolicy::Native`] at
/// start-up for exactly this reason: a framework-drawn toolbar's own Paste
/// button would break the exemption that keeps iOS's per-app paste-permission
/// prompt from firing on every paste.
///
/// [`set_selection_toolbar_builder`] installs the view a `Framework`-policy
/// pod floats, **replacing** whatever the framework's own baseline (installed
/// at bootstrap, before the first frame) or an earlier design system already
/// installed. A design system's own installer should reach for
/// `frust_core::install_selection_toolbar_builder_if_unset` instead — the
/// cooperative, set-if-unset half of the same pair — so two catalogs linked
/// into one binary never fight over the slot, and neither ever undoes an
/// app's own explicit override.
///
/// ```
/// use frust::{
///     SelectionToolbarPolicy, lock_selection_toolbar_policy, set_selection_toolbar_policy,
/// };
///
/// // A shell whose platform owns its own system edit menu locks the route,
/// // so that requirement wins no matter what app code does afterward.
/// let claimed = lock_selection_toolbar_policy(SelectionToolbarPolicy::Native);
/// assert!(claimed, "the first claim always takes the lock");
///
/// // An app's later override is refused rather than obeyed once locked.
/// set_selection_toolbar_policy(SelectionToolbarPolicy::Framework);
/// ```
pub use frust_core::{
    SelectionToolbarActions, SelectionToolbarPolicy, SelectionToolbarRequest,
    lock_selection_toolbar_policy, set_selection_toolbar_builder, set_selection_toolbar_policy,
};

/// Platform-view embedding (platform-views feature): reserve
/// layout space for a native view (a map, a video player, ...) composited
/// alongside the frust surface. [`platform_view`] takes the
/// `"dev.frust.<Factory>"`-style native factory name registered on each
/// platform and returns a builder ([`PlatformViewView`]) over the
/// params/size contract — flat-re-exported from `frust-widgets` so app code
/// never names that crate directly.
///
/// # Paint contract (Mode B)
///
/// Under Mode B compositing the frust surface itself is translucent, so
/// **any region this slot's parent doesn't paint over is a window straight
/// through to the native view (or the OS background) behind it** — this is
/// the mechanism Mode B relies on, not a bug. Size/position a slot
/// deliberately, and don't rely on an unpainted sibling region staying
/// opaque.
///
/// Mode B is selected by the generated host's `FRUST_TRANSLUCENT_SURFACE`
/// (Android)/`translucentSurface` (iOS) build-time constant — **not** from
/// Rust: that constant drives, in one host-glue branch, the native window's
/// pixel format (`PixelFormat.TRANSLUCENT`/`CAMetalLayer.isOpaque = false`),
/// the native-sibling z-order/subview arrangement, and the
/// `nativeSetSurfaceMode`/`frust_set_surface_mode` call together
/// (flipping only some of these is a host-template defect). There is
/// no app-Rust opt-in call; the generated project's template is
/// the sole route into Mode B.
///
/// ```no_run
/// use frust::{AnyView, Column, Component, any, platform_view, text};
///
/// #[derive(Default)]
/// struct MapDemo;
///
/// impl Component for MapDemo {
///     type State = ();
///
///     fn init(&self) -> Self::State {}
///
///     fn build(&self, _state: &mut Self::State) -> AnyView<Self::State> {
///         any(Column(vec![
///             any(text("map below")),
///             any(platform_view("dev.frust.MapFactory")
///                 .params_json(r#"{"style":"dark"}"#)
///                 .size(320.0, 240.0)),
///         ]))
///     }
/// }
///
/// frust::app!(MapDemo);
/// # fn main() {}
/// ```
///
/// # Z-shields ([`shield`])
///
/// A slot marked `.interactive()` forwards a touch-DOWN inside its rect to the
/// native view, including one that landed on frust chrome painted over it (the
/// OS-side hit test knows nothing about the frust scene). Wrap that chrome in
/// [`shield`] and it keeps winning input: the wrapper reports the rect it
/// painted every frame, and the shell hands the overlapping ones to the host
/// with the slot's placement.
pub use frust_widgets::{PlatformViewView, ShieldView, platform_view, shield};

/// Declarative custom painting over the [`PaintScene`] trait object — a chart,
/// a node-and-edge graph, a game board — without hand-rolling a `View`/
/// `Widget` pair. [`canvas`] takes a paint closure that runs in **local
/// space** (the widget's own top-left is always `(0, 0)`, and painting past
/// its own size is clipped, never a bug to chase) — flat-re-exported from
/// `frust-widgets` so app code never names that crate directly. See
/// [`CanvasView`]'s own doc for the full builder contract (`.size`/`.expand`
/// sizing, `.on_hit`-gated `.on_tap`/`.on_pointer`, and `.repaint_key` for
/// paint-only dirtying driven by data outside the ordinary `View` diff).
///
/// ```no_run
/// use frust::authoring::{PaintCtx, PaintScene};
/// use frust::{AnyView, Component, any, canvas};
/// use kurbo::{Point, Size};
/// use peniko::Color;
///
/// #[derive(Default)]
/// struct Clock;
///
/// impl Component for Clock {
///     type State = u32;
///
///     fn init(&self) -> Self::State {
///         0
///     }
///
///     fn build(&self, state: &mut Self::State) -> AnyView<Self::State> {
///         let ticks = *state;
///         any(canvas(move |scene: &mut dyn PaintScene, size: Size, _ctx: &PaintCtx| {
///             scene.fill_rect(Point::ZERO, size, Color::from_rgb8(0x10, 0x10, 0x10));
///         })
///         .expand()
///         .repaint_key(ticks))
///     }
/// }
///
/// frust::app!(Clock);
/// # fn main() {}
/// ```
pub use frust_widgets::{CanvasView, CanvasWidget, canvas};

/// A pan/zoom viewport over one child — a node-and-edge graph, a map, a large
/// image — without hand-rolling the gesture math. [`pan_zoom`] lays its child
/// out at its natural size and places it under a scale-then-translate
/// transform ([`PanZoomTransform`]) the user drives: primary drag pans (unless
/// the child claims the press), a touch pinch or a desktop ctrl/⌘+wheel and
/// trackpad pinch zooms about the gesture's focal point, clamped to
/// `.min_scale`/`.max_scale`. A plain wheel still reaches the child, and the
/// child sees its own unscaled local coordinates at any zoom. Flat-re-exported
/// from `frust-widgets` so app code never names that crate directly; see
/// [`PanZoomView`]'s own doc for the full contract (`.inertia` glide,
/// `.on_transform` notification, and the [`PanZoomController`] handle's
/// `jump_to`/`fit_to_bounds`/`fit_rect`).
///
/// ```no_run
/// use frust::authoring::{PaintCtx, PaintScene};
/// use frust::{AnyView, Component, PanZoomController, PanZoomTransform, any, canvas, pan_zoom};
/// use kurbo::{Point, Size};
/// use peniko::Color;
///
/// #[derive(Default)]
/// struct Board;
///
/// struct BoardState {
///     zoom: PanZoomController,
///     transform: PanZoomTransform,
/// }
///
/// impl Component for Board {
///     type State = BoardState;
///
///     fn init(&self) -> Self::State {
///         BoardState {
///             zoom: PanZoomController::new(),
///             transform: PanZoomTransform::IDENTITY,
///         }
///     }
///
///     fn build(&self, state: &mut Self::State) -> AnyView<Self::State> {
///         let board = canvas(|scene: &mut dyn PaintScene, size: Size, _ctx: &PaintCtx| {
///             scene.fill_rect(Point::ZERO, size, Color::from_rgb8(0x20, 0x20, 0x20));
///         })
///         .size(Size::new(2000.0, 1500.0));
///         any(pan_zoom(board)
///             .max_scale(4.0)
///             .inertia(true)
///             .controller(state.zoom.clone())
///             .on_transform(|state: &mut BoardState, t| state.transform = t))
///     }
/// }
///
/// frust::app!(Board);
/// # fn main() {}
/// ```
pub use frust_widgets::{
    PanZoomController, PanZoomTransform, PanZoomView, PanZoomWidget, pan_zoom,
};

/// The vendored Material Symbols starter icon set,
/// flat-re-exported so app code names `frust::icons::HOME` rather than the
/// underlying `frust-widgets` crate. Each entry is an
/// [`IconSource`](crate::IconSource) usable directly with [`icon`](crate::icon);
/// an app can also supply its own vector icons via
/// [`IconData::from_path`](crate::IconData::from_path).
pub use frust_widgets::icons;

/// The declarative router vocabulary (a go_router-subset layer),
/// flat-re-exported from `frust-widgets` so app code never names that
/// crate directly: [`Router`] resolves a location against a [`Route`] table
/// (built via [`RouteBuilder`]) into [`Resolution`]/[`ResolvedPage`]s driving
/// a [`NavigatorController`], with `:param`/query parsing
/// ([`Location`]/[`PathPattern`]/[`RouteParams`]), per-route/top-level
/// [`Redirect`]s (loop-guarded at [`DEFAULT_REDIRECT_LIMIT`]), and an
/// [`ErrorBuilder`] fallback for an unmatched location.
///
/// A page builder receives the location's query merged **under** its path
/// captures, so `/terminal?session=abc` reads its own parameter.
///
/// [`RouteNavigator`] is the seam a screen navigates through: a `Send + Sync`
/// queue of [`NavRequest`] data (paths and names, never closures) that rides
/// `provide_context` — the [`Router`] itself cannot, since it holds `Rc` page
/// builders. [`RouterDeepLinks::track`] drains it every rebuild, so a request
/// queued in an event handler (on any thread — the queue never panics
/// off-thread) applies on the next frame.
///
/// [`shell_route`] is the nested-navigator binding: its children resolve onto a
/// second [`NavigatorController`] the app owns and its own page — the chrome
/// wrapping that inner navigator — stays retained while they do, so navigating
/// between siblings inside the shell never rebuilds the chrome. See its docs
/// for the keep rule and the per-verb table.
pub use frust_widgets::{
    DEFAULT_REDIRECT_LIMIT, ErrorBuilder, Location, NavChange, NavRequest, NavWaker, PathPattern,
    Redirect, Resolution, ResolvedPage, Route, RouteBuilder, RouteNavigator, RouteParams,
    RouteStack, Router, shell_route,
};

/// The page-transition **resolve/drive** path: the framework's own reduce-
/// motion collapse policy, re-exported so an app-authored animation reuses it
/// rather than re-deriving it.
///
/// [`resolve_spec`] applies `Theme.motion.reduce_motion`'s hard accessibility
/// rule to a [`TransitionSpec`] — collapsing any animated preset to a
/// `≤120ms` linear cross-fade — and resolves a bare [`Timing::ThemeDefault`]
/// against the active [`MotionScheme`]. [`make_driver`] then turns the
/// resolved [`Timing`] into a running [`TransitionDriver`] (advanced each
/// frame via `TransitionDriver::advance`) plus the [`SpringDesc`] to settle it
/// with later (an interactive edge-swipe's release fling). This is exactly
/// the pair `frust-widgets`' own navigator and
/// [`motion::switcher::PatternSwitcher`](crate::motion::switcher::PatternSwitcher)
/// drive their transitions through — before this re-export, an app widget
/// implementing its own page/dialog animation had to hand-roll the
/// reduce-motion collapse (read `Theme.motion.reduce_motion`, apply the
/// `120ms` + linear + zero-motion rule) instead of calling the framework's one
/// implementation, so the two would silently drift the next time the
/// framework's rule changes.
///
/// Not otherwise re-exported at `frust-widgets`' own crate root (reached here
/// via `frust_widgets::nav::transition`, a `pub mod` two levels down) — this
/// is the facade's job precisely so app code never has to know that.
///
/// ```
/// use frust::{Curve, PageTransition, Theme, Timing, TransitionDriver, TransitionSpec};
///
/// // A theme with reduce_motion on: resolve_spec collapses the spec to the
/// // framework's own linear cross-fade, with no app-side collapse logic.
/// let mut theme = Theme::neutral();
/// theme.motion.reduce_motion = true;
///
/// let spec = TransitionSpec::duration(PageTransition::SlideUp);
/// let resolved = frust::resolve_spec(spec, Some(&theme.motion));
/// assert_eq!(resolved.preset, PageTransition::ReducedCrossfade);
/// assert_eq!(
///     resolved.timing,
///     Timing::Duration(std::time::Duration::from_millis(120), Curve::Linear)
/// );
///
/// let (driver, _settle_spring) = frust::make_driver(resolved.timing);
/// assert!(matches!(driver, TransitionDriver::Auto(_)));
/// ```
pub use frust_widgets::nav::transition::{TransitionDriver, make_driver, resolve_spec};

/// The `motion` module: declarative
/// implicit-animation wrappers (`AnimatedOpacity`/`AnimatedScale` today;
/// `switcher`/`patterns` land later) over `frust-core`'s `anim`
/// vocabulary. Re-exported **wholesale**
/// (`pub use frust_widgets::motion;`), mirroring `frust_widgets::icons`'s
/// existing wholesale-module precedent — the only other one in this facade —
/// so later types under `frust_widgets::motion` ride along under
/// `frust::motion::*` with no further facade edits (see that module's own
/// docs for the full rationale).
///
/// ```
/// use frust::motion::{AnimatedOpacity, AnimatedOpacityView, AnimatedScale};
/// use frust::text;
///
/// let _opacity: AnimatedOpacityView<()> = AnimatedOpacity(1.0, text("hi"));
/// let _scale: frust::motion::AnimatedScaleView<()> = AnimatedScale(1.0, text("hi"));
/// ```
pub use frust_widgets::motion;

/// Everything needed to author a custom `View`/`Widget` pair.
///
/// **`frust` alone is sufficient**: an app that implements its own
/// layout/paint/event widget should need no framework dependency but this
/// crate. If something is reachable through neither this module nor the flat
/// facade, that is a bug — file it rather than reaching for `frust-core`
/// directly.
///
/// `use frust::authoring::*;` covers the widget-authoring vocabulary proper.
/// Two neighbouring surfaces are deliberately *not* duplicated here because
/// they are already reachable flat, and a real widget usually wants them too:
///
/// - [`frust::input`](crate::input) — gesture constants such as `TOUCH_SLOP`.
/// - The animation vocabulary — [`AnimationController`](crate::AnimationController),
///   [`Curve`](crate::Curve), [`FrameTime`](crate::FrameTime) and friends.
///
/// For anything the by-name lists below omit, reach for the whole-crate valves:
/// [`frust::kurbo`](crate::kurbo), [`frust::peniko`](crate::peniko),
/// [`frust::accesskit`](crate::accesskit).
///
/// Not feature-gated (unlike the design-system re-exports above): an app that
/// disables every catalog (`--no-default-features`) still needs this seam to
/// build its own design system, so it always resolves.
///
/// # The `EditingState` split
///
/// `frust_core::event::EditingState` (flat, right here in `authoring`) and
/// `frust_text::editor::EditingState` (nested in [`text`](authoring::text))
/// are **two genuinely distinct types** — the first is the retained
/// IME-surface payload an event-routing widget publishes/receives
/// (`ImeState`/`ImeEvent`), the second is `TextEditor`'s own byte-indexed
/// editing snapshot. A glob importing both into one scope will not compile
/// (`use frust::authoring::*; use frust::authoring::text::*;` collides on the
/// name); reach for the flat one for event/IME plumbing and
/// `authoring::text::EditingState` only alongside a `TextEditor` you're
/// driving yourself.
///
/// # Example: a one-child container widget
///
/// A container that offsets its single child, wired through the full
/// lifecycle — build, rebuild, teardown, layout, paint, event routing,
/// semantics forwarding — with **only** `frust` named. Ported from
/// `frust_widgets::authoring`'s own worked example (the toolkit this module
/// re-exports), which an app cannot reach directly without depending on
/// `frust-widgets` itself.
///
/// ```
/// use frust::authoring::*;
///
/// /// The declarative half: a child plus the offset to apply to it.
/// struct OffsetView<State: 'static> {
///     offset: Point,
///     child: AnyView<State>,
/// }
///
/// /// The view-fn app code calls; `any` erases the concrete child view.
/// fn offset<State: 'static, V: View<State>>(offset: Point, child: V) -> OffsetView<State> {
///     OffsetView { offset, child: any(child) }
/// }
///
/// /// The retained half: the live child pod plus the applied offset.
/// struct OffsetWidget {
///     offset: Point,
///     child: ChildPod,
/// }
///
/// impl<State: 'static> View<State> for OffsetView<State> {
///     type Element = OffsetWidget;
///
///     fn build(&self, ctx: &mut BuildCtx<'_>) -> OffsetWidget {
///         OffsetWidget { offset: self.offset, child: build_child(&self.child, ctx) }
///     }
///
///     fn rebuild(
///         &self,
///         prev: &Self,
///         element: &mut OffsetWidget,
///         ctx: &mut BuildCtx<'_>,
///     ) -> ChangeFlags {
///         let mut flags = ChangeFlags::NONE;
///         if prev.offset != self.offset {
///             element.offset = self.offset;
///             flags |= ChangeFlags::LAYOUT;
///         }
///         flags | rebuild_child(&prev.child, &self.child, &mut element.child, ctx)
///     }
///
///     fn teardown(&self, element: &mut OffsetWidget, ctx: &mut BuildCtx<'_>) {
///         teardown_child(&self.child, &mut element.child, ctx);
///     }
/// }
///
/// impl Widget for OffsetWidget {
///     fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
///         let size = self.child.layout_child(ctx, bc);
///         self.child.set_origin(self.offset);
///         bc.constrain(size)
///     }
///
///     fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
///         self.child.paint_child(ctx, scene);
///     }
///
///     fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
///         // Never re-hit-test a captured child by hand — this helper owns the
///         // capture/focus fast paths and the blur-on-outside-tap rule.
///         route_event_single(&mut self.child, ctx, event)
///     }
///
///     fn semantics(&self, ctx: &mut SemanticsCtx) {
///         // A transparent wrapper still MUST forward, or the child's whole
///         // subtree drops out of the accessibility tree.
///         self.child.semantics_child(ctx);
///     }
/// }
/// # fn main() {}
/// ```
pub mod authoring {
    // tier 1 — the trait vocabulary (frust-core)
    /// The cursor vocabulary a widget requests through
    /// [`EventCtx::set_cursor`](frust_core::event::EventCtx::set_cursor) — lifted
    /// so a design system's own controls can name a shape (`Pointer` on a button,
    /// `Text` over an editable, `Grabbing` on a live drag) without a direct
    /// `frust-core` dependency. Also re-exported flat as
    /// [`frust::CursorIcon`](crate::CursorIcon).
    pub use frust_core::CursorIcon;
    pub use frust_core::{
        AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, DiscardScene, EventCtx,
        EventOutcome, EventResult, HeroDirective, HeroFrames, InputEvent, Key, KeyEvent, LayoutCtx,
        Modifiers, NamedKey, PaintCtx, PaintOutcome, PaintScene, PointerButton, PointerEvent,
        PointerPhase, ScrollDelta, SemanticsCtx, SemanticsUpdate, TickClass, View, Widget,
        WidgetId, any,
    };

    /// The clipboard/selection vocabulary an editable widget matches on —
    /// [`InputEvent::EditCommand`] carries one of these four verbs, already
    /// decoded from whatever chord, hardware key or edit-menu tap produced it.
    /// A widget answers copy/cut through
    /// [`EventCtx::write_clipboard`](frust_core::EventCtx::write_clipboard) and
    /// asks for a paste through
    /// [`EventCtx::request_paste`](frust_core::EventCtx::request_paste); the shell
    /// owns the host clipboard at both ends. Also re-exported flat as
    /// [`frust::EditCommand`](crate::EditCommand), for the same reason
    /// [`CursorIcon`] is: a design system's public API can name a verb without
    /// authoring a widget.
    pub use frust_core::EditCommand;

    /// The paint vocabulary [`PaintScene`]'s per-corner and dashed methods name
    /// — `fill_rounded_rect_radii`/`push_clip_rounded_radii` take a
    /// [`CornerRadii`], `stroke_path_dashed` a [`DashPattern`]. Lifted for the
    /// same reason [`WindowInsets`] is: a widget calling those methods cannot
    /// otherwise name their arguments. Re-exported through `frust-core`'s paint
    /// surface, so this is the same type [`scene::CornerRadii`] names.
    pub use frust_core::{CornerRadii, DashPattern};

    /// The event-pass/IME-surface `EditingState` — see this module's own docs
    /// for the split against [`text::EditingState`](self::text::EditingState),
    /// `frust_text::editor`'s distinct, byte-indexed type. `ImeContentType` is
    /// the input-purpose hint an editable widget publishes on its `ImeState` so
    /// a shell can lock a secret field's keyboard down.
    pub use frust_core::{EditingState, ImeContentType, ImeEvent, ImeState};

    /// The inset vocabulary [`LayoutCtx::window_insets`]/[`PaintCtx::window_insets`]
    /// return — lifted because a widget that lays itself out around the status
    /// bar, notch, or on-screen keyboard cannot otherwise name the value those
    /// accessors hand it. A bar laying out around the iPadOS window control
    /// names the corner value, [`CornerInsets`].
    pub use frust_core::{CornerInset, CornerInsets, WindowEdgeInsets, WindowInsets};

    /// The window-shape value recovered via
    /// `use_context::<`[`WindowMetrics`]`>()` inside `Component::build` — lifted
    /// alongside [`WindowInsets`] for the same reason: a widget or component
    /// laying itself out around window size/scale/orientation cannot otherwise
    /// name the value. See [`WindowMetrics`]'s own doc for the plain-value
    /// delivery contract, the alongside-not-superseding relationship with
    /// [`WindowInsets`], and the derived-orientation/rebuild-cost notes.
    pub use frust_core::{Orientation, WindowMetrics};

    /// The accessibility-node vocabulary a widget that *contributes* a
    /// semantics node needs — as opposed to one that only forwards a child's.
    ///
    /// [`SemanticsCtx::push_node`] is
    /// `(role: Role, build: impl FnOnce(&mut Node)) -> NodeId`;
    /// [`SemanticsCtx::push_container`] takes the same two plus a
    /// `visit: impl FnOnce(&mut SemanticsCtx)` for its children. `Action`,
    /// `Live` and `Toggled` are lifted alongside them because they are what
    /// the `build` closure actually reaches for — `node.add_action(Action::Click)`
    /// is in eight of `frust-widgets`' own widgets, and a list of exports that
    /// stopped at `Node` would not be closed over `Node`'s own signature.
    ///
    /// For the rest of the crate, use the whole-crate
    /// [`frust::accesskit`](crate::accesskit) valve.
    pub use frust_core::accesskit::{Action, Live, Node, NodeId, Role, Toggled};

    // tier 2 — child/event/callback plumbing, verbatim
    pub use frust_widgets::authoring::*;

    // tier 3 — geometry/paint. NOTE the split: `Stroke` is kurbo, `Fill` is peniko
    // (kurbo owns the geometry vocabulary, peniko the paint vocabulary).
    pub use kurbo::{Affine, BezPath, Line, Point, Rect, RoundedRect, Shape, Size, Stroke, Vec2};
    pub use peniko::{Brush, Color, Fill, ImageData};

    /// Text shaping/measurement for widgets laying out their own glyph runs.
    ///
    /// Nests `EditingState` deliberately: `frust_text::editor::EditingState` is
    /// `TextEditor`'s own byte-indexed editing snapshot, a **distinct type**
    /// from the flat `authoring::EditingState` (the retained IME-surface
    /// payload) — see this module's parent docs for the full split. Keeping it
    /// nested here rather than flattened avoids the glob collision a flat
    /// re-export of both would cause.
    pub mod text {
        pub use frust_text::{EditOp, EditingState, EditingStateBytes, TextEditor};
        pub use frust_text::{
            FamilyName, FontFamily, FontStyle, FontWeight, GenericSlot, LineHeight, TextAlign,
            TextContext, TextLayout, TextOverflow, TextStyle,
        };
        /// Types named in [`TextContext`]'s own public signatures —
        /// `register_fonts() -> Result<Vec<RegisteredFamily>, FontError>` and
        /// `shape_cache_stats() -> ShapeCacheStats`. Without these an app could
        /// call the methods but never name what they return.
        pub use frust_text::{FontError, RegisteredFamily, ShapeCacheStats};
        /// The index bridge between the two `EditingState`s this seam exposes:
        /// [`super::EditingState`] (core/IME) counts UTF-16 code units, while
        /// [`EditingStateBytes`] counts bytes. A widget driving its own
        /// [`TextEditor`] against the IME surface needs to convert between
        /// them — `frust_widgets::textinput` calls `utf16_to_byte` to place an
        /// IME-supplied cursor into its byte-indexed editor; `byte_to_utf16`
        /// is the return direction, used inside `frust-text` itself to build
        /// an `EditingState` back out of editor state.
        pub use frust_text::{byte_to_utf16, utf16_to_byte};
    }

    /// The renderer-agnostic display list, for widgets painting below `PaintScene`.
    ///
    /// `CornerRadii`/`DashPattern` are the same types the flat
    /// [`authoring::CornerRadii`](super::CornerRadii)/[`authoring::DashPattern`](super::DashPattern)
    /// re-exports name — listed here too because they are named by
    /// `Command::RoundedRect`/`PathStyle`, which a widget recording commands
    /// directly has to match on.
    pub mod scene {
        pub use frust_scene::{
            Command, CornerRadii, DashPattern, FontHandle, Glyph, GlyphRun, PathStyle, Scene,
            SceneBuilder, ShaderProgram, arc_path,
        };
    }
}

/// The GPU substrate seam, behind the non-default `gpu` cargo feature: the
/// `frust-gpu` device/texture/pipeline vocabulary an app — or a future
/// 3D-rendering crate sitting beside the facade — needs to register an
/// externally owned GPU texture and draw it through
/// [`authoring::scene::SceneBuilder::scene_texture`], without a direct
/// `frust-render`/`frust-gpu` dependency of its own.
///
/// [`Context`] is `frust-gpu`'s `RenderContext` (the device/instance
/// foundation every shell already owns) under the name this seam exposes it
/// as. [`Texture`]/[`TextureDesc`]/[`RenderTarget`] describe a GPU texture
/// and where it can be rendered into; [`SceneTextureId`] is the
/// process-unique id [`Texture::as_scene_texture`] mints, and is the value a
/// [`authoring::scene::Command::SceneTexture`] resolves against — an id
/// nothing binds simply draws nothing, never an error (see
/// `scene_texture`'s own docs). [`CommandBuffer`], [`ShaderLibrary`], and
/// [`RenderPipelineDesc`] are the lower-level encode/pipeline vocabulary a
/// caller driving its own render pass beside the engine's needs.
///
/// This seam is deliberately narrow: nothing above RENDER depends on
/// `frust-gpu` today (`docs/ARCHITECTURE.md`'s GPU-substrate layer
/// boundary), and this feature-gated re-export is the one sanctioned
/// exception — an app opts in explicitly, and the facade's own default build
/// carries none of it.
///
/// A real [`Texture`] is built from a [`TextureDesc`] naming a
/// `wgpu::TextureFormat`/`wgpu::TextureUsages` pair directly — this crate
/// does not re-export `wgpu` itself (only the GPU-substrate *types* built on
/// it), so a caller filling in those two fields depends on `wgpu` in its own
/// right, exactly as any other code driving a render pass beside the engine
/// does. Once minted, [`Texture::as_scene_texture`]'s id composes with
/// [`authoring::scene::SceneBuilder::scene_texture`] with no GPU device in
/// the loop at all:
///
/// ```
/// use frust::authoring::Rect;
/// use frust::authoring::scene::{Command, Scene, SceneBuilder};
/// use frust::gpu::Texture;
///
/// // Checked at compile time over any texture/view pair a caller supplies —
/// // `Texture::as_scene_texture()`'s minted id composes with
/// // `SceneBuilder::scene_texture` with no live `Texture` needed to prove it.
/// fn draw_registered_texture<T, V>(texture: &Texture<T, V>, scene: &mut Scene, dest: Rect) {
///     let mut builder = SceneBuilder::new(scene);
///     builder.scene_texture(texture.as_scene_texture().get(), dest);
/// }
///
/// // The same recording behavior, exercised at runtime with the same opaque
/// // `u64` id a minted `SceneTextureId::get()` ultimately is.
/// let id = 42;
/// let dest = Rect::new(0.0, 0.0, 64.0, 64.0);
/// let mut scene = Scene::new();
/// SceneBuilder::new(&mut scene).scene_texture(id, dest);
///
/// match &scene.commands()[0] {
///     Command::SceneTexture { id: recorded, .. } => assert_eq!(*recorded, id),
///     other => panic!("expected SceneTexture, got {other:?}"),
/// }
/// ```
///
/// # Reaching the shell's own live device: [`with_context`]
///
/// Everything above builds a *standalone* [`Context`] — useful for a
/// headless harness, but a second device, not the one the running shell
/// already created and is presenting frames through. [`with_context`] reaches
/// that one instead: the shell's render executor installs its
/// [`DeviceHandle`] (the device/queue/adapter pair, cheap to clone since
/// wgpu's own types are `Arc`-backed) into a process-wide slot the first time
/// a surface — and so a device — comes up
/// (`frust_shell_common::gpu::install_gpu_handle`, see that module's docs),
/// and [`with_context`] reads it back through the identical seam
/// (`frust_shell_common::gpu::gpu_handle`). It answers `None` before that
/// first surface exists (desktop's zero-config preview window, a mobile
/// shell before its first frame) — check the `Option`, never assume `Some`.
/// Never blocks: once installed, the read is a lock-free `OnceLock::get`, so
/// calling this from the UI thread is always safe.
///
/// Only the desktop shell installs one today; the two mobile shells forward
/// the `gpu` feature but do not install yet (an accepted gap — see
/// `frust-shell-android`/`frust-shell-ios`'s own crate docs), so
/// `with_context` always answers `None` there for now.
///
/// ```
/// // No device exists in this doctest process, so `with_context` answers
/// // `None` — exactly the state an app sees before its shell's first frame.
/// let adapter_name = frust::gpu::with_context(|handle| handle.caps.adapter_name.clone());
/// assert!(adapter_name.is_none());
/// ```
///
/// # Rendering your own texture every frame: [`ExternalPass`]
///
/// Everything above binds a texture whose *pixels* something else already
/// produced. A caller whose pixels are produced by GPU work of its own — a 3D
/// scene, a simulation, a video frame converted on the GPU — needs that work
/// recorded inside the frame that samples it, and needs it recorded every
/// frame. That is [`ExternalPass`]: implement it, register it under an id,
/// and the engine tier's renderer calls it once per frame with the frame's
/// own device, queue and `wgpu::CommandEncoder` ([`ExternalFrame`]) before
/// anything of the scene is recorded.
///
/// The four steps a widget owning such a texture takes:
///
/// 1. **Mint an id**, once, and keep it: [`SceneTextureId::mint`] when the
///    widget has no [`Texture`] to mint from (the usual case here — the pass
///    creates and re-creates its own target), or
///    [`Texture::as_scene_texture`] when it does. One id survives every
///    re-creation of the underlying texture, which is what lets the display
///    list keep naming the same thing across a resize.
/// 2. **Register a pass** under it with [`register_external_pass`], handing
///    over an `Arc<dyn ExternalPass>`. It answers `false` if that id already
///    has a pass, and changes nothing — a registration is a claim, never a
///    silent takeover.
/// 3. **Paint the id.** In `Widget::paint`, record
///    [`authoring::PaintScene::draw_scene_texture`] with
///    `id.get()` and the destination rectangle. The pass's binding and the
///    display list naming it meet inside one frame, so the ordering is
///    already right; an id whose pass has not bound anything yet simply draws
///    nothing that frame, never an error.
/// 4. **Unregister on teardown** with [`unregister_external_pass`] — in
///    `View::teardown` (a component's `ComponentWidget::teardown`) or
///    `on_cleanup` for reactive state, never a hand-rolled `Drop`
///    (`docs/CODE_STANDARDS.md`'s State & Reactivity rule: `Drop` order
///    across a component's state/element/owner triple is not a contract,
///    `on_cleanup`/`teardown` are). The next *drained* frame clears the
///    engine's binding for that id before it runs any pass, so no view is
///    kept alive for a texture whose owner is gone — though unregistering is
///    not itself a barrier: a drain already mid-flight when it runs may still
///    call this pass's `record` once more, and the unbind
///    lands only once a frame is actually drained, not while the surface is
///    idle.
///
/// A widget whose texture is *animating* asks for the next frame exactly as
/// any other animating widget does — `PaintCtx::request_frame` (or
/// `request_frame_paced` for a decorative loop). A pass is called once per
/// frame the app actually renders; it does not drive the frame loop, and
/// registering one does not by itself keep frames coming.
///
/// ```text
/// struct Starfield { target: Mutex<Option<wgpu::Texture>> }
///
/// impl frust::gpu::ExternalPass for Starfield {
///     fn record(&self, frame: &mut frust::gpu::ExternalFrame<'_>) {
///         let texture = self.ensure_target(frame.device());   // my own target
///         let view = texture.create_view(&Default::default());
///         {
///             let mut pass = frame.encoder().begin_render_pass(&/* … */);
///             // … draw into `view` …
///         }                                                    // pass ends here
///         frame.bind_texture((WIDTH, HEIGHT), view);  // engine draws it, under this pass's own id
///     }
/// }
/// ```
///
/// Four rules that go with it:
///
/// - **A pass renders into its own target, never into the frame's.** The
///   engine clears the frame's colour attachment when it records the scene,
///   so pixels a pass wrote there are gone; what survives is what the scene
///   composites from a bound texture. A pass also never submits — the
///   renderer submits the whole frame, once, which is what puts the pass's
///   work ahead of the scene's in a single command buffer.
/// - **A pass never shares the frame's own depth attachment.** It records
///   only into attachments it owns, on a target it owns, sized however that
///   target needs to be — [`ExternalFrame`] hands out no depth view of its
///   own. `frust-gpu::encoder`'s two depth caller rules — the module
///   [`CommandBuffer`] comes from — are for a *host* sharing one depth buffer
///   across renderers of its own; they have nothing to do with a pass
///   registered here.
/// - **The result is always composited blended**, never treated as opaque:
///   the engine does not read the caller's texels, so it cannot know they are
///   (`docs/LIMITATIONS.md`'s `engine-scene-texture-always-blended`). Return
///   premultiplied colour, the convention every paint in an engine frame
///   travels in.
/// - **A pass's recorded work is submitted only if the frame is** — a
///   refused engine frame drops the encoder unsubmitted, and every command a
///   pass recorded into it goes with it — **but [`ExternalFrame::bind_texture`]
///   is not part of that encoder**; it writes the engine's registry directly,
///   inside `record`, so its side effect survives a refusal the recorded draw
///   commands do not. A pass whose target must never be sampled half-written
///   binds only once that target genuinely holds something, rather than
///   relying on the frame that was meant to fill it having been accepted.
///
/// The registry itself is shell-agnostic — a process-wide map with no device,
/// window or platform in it, registerable from anywhere at any time. The
/// gate is which frame path drains it: any engine-tier `SurfaceRenderer`
/// frame, `submit` and `submit_deferred` alike, which is what the Android and
/// iOS shells call exactly as desktop does — a registered pass is not
/// desktop-only by construction, unlike [`with_context`], which genuinely
/// does answer `None` on mobile today (no shell there has installed a
/// [`DeviceHandle`] yet). Only the desktop headless path has actually been
/// *exercised* so far (`docs/LIMITATIONS.md`'s
/// `facade-external-pass-desktop-only`) — mobile is wired and compiles the
/// seam, but no registered pass has been driven through either shell's own
/// frame loop yet.
///
/// A panic inside `record` is caught in a debug build: the pass is reported,
/// unregistered and unbound, and the frame is recorded without it. The
/// workspace's `release` profile is `panic = "abort"`, so that is a
/// development net rather than a shipped guarantee — a pass must not panic.
#[cfg(feature = "gpu")]
pub mod gpu {
    pub use frust_gpu::{
        CommandBuffer, DeviceHandle, RenderContext as Context, RenderPipelineDesc, RenderTarget,
        SceneTextureId, ShaderLibrary, Texture, TextureDesc,
    };
    /// The pre-scene pass seam (see the module docs' *Rendering your own
    /// texture every frame*), from the renderer that owns the frame the
    /// passes are recorded into rather than from `frust-gpu`: the registry is
    /// process-wide, but only an engine-tier frame drains it.
    pub use frust_render::{
        ExternalFrame, ExternalPass, register_external_pass, unregister_external_pass,
    };
    /// The wgpu graphics library, re-exported for plugins that record an
    /// [`ExternalPass`] so they do not need a direct `wgpu` dependency.
    #[cfg(feature = "gpu")]
    pub use wgpu;

    /// Run `f` against the shell-owned live [`DeviceHandle`], or `None` if no
    /// shell has installed one yet (see the module docs' *Reaching the
    /// shell's own live device* section).
    ///
    /// Distinct from [`Context`] (`frust-gpu`'s `RenderContext`), which an app
    /// can build a *standalone* device from (`Context::new()` +
    /// `ensure_device_headless`/a real surface) — that path always creates a
    /// second device. `with_context` never creates one; it only reads back
    /// the one the running shell already owns.
    pub fn with_context<R>(f: impl FnOnce(&DeviceHandle) -> R) -> Option<R> {
        frust_shell_common::gpu::gpu_handle::<DeviceHandle>().map(f)
    }
}

/// The accessibility vocabulary crate, whole — the long-tail valve behind
/// [`authoring`]'s by-name `Node`/`NodeId`/`Role`, for the rest of what a
/// semantics-contributing widget may need (`Action`, `Live`, `Toggled`, …).
/// Re-exported through `frust-core`, which owns the `accesskit` version pin
/// (see `docs/DEVELOPMENT.md` § Version-Pin Policy) — naming it here keeps an
/// app on that single pinned version rather than declaring its own.
pub use frust_core::accesskit;
/// The geometry crate, whole, for types [`authoring`] does not lift by name
/// (e.g. `kurbo::Circle`) — the long-tail escape valve alongside the by-name
/// list above.
pub use kurbo;
/// The brush/color crate, whole, for types [`authoring`] does not lift by name
/// (e.g. `peniko::Blob`, `peniko::color::DynamicColor`) — the long-tail escape
/// valve alongside [`Color`]/the by-name list above.
pub use peniko;

mod back_glue;
mod route_state;
mod router_glue;

/// Android back-press ⇄ navigator auto-wiring:
/// [`attach_back_handler`]/[`BackHandler`] pop a [`NavigatorController`] on a
/// platform back press and keep `frust-reactive`'s `handles_back` flag in
/// sync with the stack depth, so a shell knows whether a root-level back should
/// fall through to the platform (activity finish). Mirrors
/// [`RouterDeepLinks`]'s shape and, like it, is the ONLY place in the facade
/// that sees both `frust-widgets`' `NavigatorController` and
/// `frust-reactive`'s back-press source together — see [`BackHandler`]'s doc
/// for the consume/dedupe and timing contracts. Call [`BackHandler::track`]
/// from every `Component::build`.
pub use back_glue::{BackHandler, attach_back_handler};

/// Build a [`NavigatorView`] driven by `controller` — the facade's back-aware
/// wrapper over [`frust_widgets::navigator`].
///
/// Interposes on the flat widget re-export: same signature and return shape, but
/// every rebuild it *additionally* auto-wires Android/gesture back handling for
/// `controller` — so an app using `frust::navigator` gets the full back contract
/// (dismissable overlay dismiss → navigation pop → app exit at the root) with
/// **zero** back-specific app code, and with `frust::handles_back` reporting
/// whether a root-level press should fall through to the platform.
///
/// The wiring routes a consumed press through
/// [`NavigatorController::request_back`] (honoring each page's back policy —
/// pop / animated-dismiss / veto — rather than a bare pop) and computes
/// `handles_back` from the navigator's predictive-back interest. It shares a
/// single process-wide consumption source with any explicit [`BackHandler`] on
/// the same controller, so constructing a `BackHandler` *and* calling
/// `frust::navigator` (as `examples/huddle` does) still consumes each press
/// exactly once — no double-pop. See [`back_glue`]'s module docs for the
/// consume/dedupe and timing contracts.
///
/// ```no_run
/// use frust::{AnyView, Component, NavigatorController, any, navigator, text};
///
/// #[derive(Default)]
/// struct App;
///
/// struct AppState {
///     nav: NavigatorController<AppState>,
/// }
///
/// impl Component for App {
///     type State = AppState;
///
///     fn init(&self) -> AppState {
///         AppState { nav: NavigatorController::new() }
///     }
///
///     fn build(&self, state: &mut AppState) -> AnyView<AppState> {
///         // Back handling is automatic — no BackHandler needed.
///         any(navigator(&state.nav, || any(text("home"))))
///     }
/// }
///
/// frust::app!(App);
/// # fn main() {}
/// ```
///
/// Also sets the platform-aware edge-swipe default
/// ([`NavigatorView::platform_pop_swipe`]) to `cfg!(target_os = "ios")`: the
/// interactive pop-swipe is on by default on iOS and off on Android/desktop
/// (where the system/window-manager back gesture already exists), with no
/// app-side wiring. An app can still override it per navigator
/// ([`NavigatorView::pop_swipe`]) or per page
/// ([`frust_widgets::PushOptions::pop_swipe`]).
pub fn navigator<State: 'static>(
    controller: &NavigatorController<State>,
    initial: impl Fn() -> AnyView<State> + 'static,
) -> NavigatorView<State> {
    back_glue::auto_wire(controller);
    frust_widgets::navigator(controller, initial).platform_pop_swipe(cfg!(target_os = "ios"))
}

/// Build a **root overlay host** driven by `controller`, wrapping the app's
/// whole root view — the facade's back-aware wrapper over
/// [`frust_widgets::overlay_host`].
///
/// The host is a [`navigator`] whose root page is the entire app (chrome, tab
/// shell, inner navigator and all) and whose pushed pages are app-level modals,
/// with two defaults changed: no edge-swipe pop, and no host transition (each
/// overlay stages its own). Because it sits *above* every piece of chrome, an
/// overlay pushed here dims and blocks chrome that an overlay on an inner
/// navigator cannot reach — and the chrome goes inert to pointers *and* to
/// assistive technology for free, since the navigator routes input and forwards
/// accessibility nodes for the top page only.
///
/// Like [`navigator`], every rebuild additionally auto-wires back handling for
/// `controller` — here as a **host**, the rank that claims a press ahead of any
/// plain navigator (**R44-back**), whenever and however often either wires. A
/// host with no overlays open claims nothing, so back falls through to the inner
/// navigator exactly as before the host existed. See [`back_glue`]'s module docs
/// for the arbitration contract.
///
/// ```no_run
/// use frust::{
///     AnyView, Component, NavigatorController, Stack, any, navigator, overlay_host, text,
/// };
///
/// #[derive(Default)]
/// struct App;
///
/// struct AppState {
///     nav: NavigatorController<AppState>,
///     overlays: NavigatorController<AppState>,
/// }
///
/// impl Component for App {
///     type State = AppState;
///
///     fn init(&self) -> AppState {
///         AppState {
///             nav: NavigatorController::new(),
///             overlays: NavigatorController::new(),
///         }
///     }
///
///     fn build(&self, state: &mut AppState) -> AnyView<AppState> {
///         let nav = state.nav.clone();
///         // The former root view moves INSIDE the host's page builder — which
///         // is also what puts the inner navigator's wiring after the host's.
///         any(overlay_host(&state.overlays, move || {
///             any(Stack(vec![
///                 any(navigator(&nav, || any(text("home")))),
///                 any(text("persistent chrome")),
///             ]))
///         }))
///     }
/// }
///
/// frust::app!(App);
/// # fn main() {}
/// ```
///
/// Also sets the platform-aware edge-swipe default like [`navigator`] does,
/// though it never actually changes the host's own behaviour: `overlay_host`
/// pins an *explicit* [`NavigatorView::pop_swipe(false)`](NavigatorView::pop_swipe)
/// (an edge swipe must never dismiss an overlay), which outranks the platform
/// slot by construction.
pub fn overlay_host<State: 'static>(
    controller: &NavigatorController<State>,
    app: impl Fn() -> AnyView<State> + 'static,
) -> NavigatorView<State> {
    back_glue::auto_wire_overlay_host(controller);
    frust_widgets::overlay_host(controller, app).platform_pop_swipe(cfg!(target_os = "ios"))
}

/// Router ⇄ deep-link auto-wiring: [`router_with_deep_links`]/
/// [`RouterDeepLinks`] resolve a [`Router`]'s start location from the process's
/// cold-start deep link (falling back to an app-supplied default) and keep
/// navigating it on every subsequent warm link — see [`RouterDeepLinks`]'s doc
/// for the precedence and dedupe contracts. This is the ONLY place in the
/// facade that sees both `frust-widgets`' `Router` and `frust-reactive`'s
/// deep-link source together; neither underlying crate depends on the other.
pub use router_glue::{RouterDeepLinks, router_with_deep_links};

/// The reactive route-state observable: [`RouteObserver`] is the signal
/// face over `frust_widgets`' signal-free `RouteStack`/[`NavChange`] — the
/// counterpart to [`RouteNavigator`] (*intent*, queued requests) that reads
/// *fact* (the last-published stack) instead. Construct once (typically in
/// `Component::init`) and attach with
/// [`observe`](RouteObserver::observe)`(navigator(...))`;
/// [`RouterDeepLinks::routes`] hands out the one it wired for a router-driven
/// navigator. See `route_state`'s module docs for why this bridge lives in
/// the facade rather than `frust-widgets`.
pub use route_state::RouteObserver;

/// The design-token vocabulary: the [`Theme`] bundle plus its
/// component token tables, flat-re-exported from `frust-theme` so app code
/// never names that crate directly. A root component reads the active theme via
/// [`use_context`]`::<`[`Theme`]`>()`; a widget reads it during paint/layout via
/// `PaintCtx::theme_as`/`LayoutCtx::theme_as` (or `Theme::from_paint_ctx`).
///
/// Includes the glass material tokens — this crate ships the opaque recipe,
/// and a design system authors its own translucent one over the same types:
///
/// ```
/// use frust::GlassScale;
///
/// let glass = GlassScale::opaque_material();
/// assert_eq!(glass.chrome.blur_radius_intent, 0.0);
/// assert!(glass.control.is_opaque());
/// ```
///
/// Also the composable-theming surface:
/// [`ThemeBuilder`] (`defineTheme`/`copyWith` analog), the no-lock-in typed
/// extension slot ([`ThemeExtensions`]) plus its first consumer
/// [`StatusPalette`]/[`StatusColors`] (success/warning/info), and the motion
/// vocabulary ([`MotionDurations`]/[`EasingSet`]) — all flat-re-exported so an
/// app (or a design-system plugin) authors a theme against `frust::*` alone.
pub use frust_theme::{
    Brightness, ColorScheme, CosmeticLoopRate, DesignLanguage, EasingSet, Elevation,
    ElevationLevel, FontFace, GlassFill, GlassMaterial, GlassScale, MotionDurations, MotionScheme,
    MotionSpring, NativeTypefaces, ShadowSpec, ShapeScale, StatusColors, StatusPalette,
    SurfaceRole, Theme, ThemeBuilder, ThemeExtensions, TypeScale,
};

/// The color type every [`ColorScheme`] role is expressed in
/// ([`peniko::Color`]), re-exported so app code can author its own color
/// values (e.g. a custom accent palette that composes onto a baseline
/// [`ColorScheme`]) without naming `peniko` directly — the same
/// facade-only-dependency rule the theme re-exports above follow. Construct
/// one with [`Color::from_rgb8`]/[`Color::new`]; read its channels via
/// `Color::components` (`[f32; 4]`, straight-alpha RGBA).
pub use peniko::Color;

/// App-facing theme override:
/// [`set_app_theme`] forces the app's active [`Theme`] end-to-end — both
/// delivery paths a shell owned exclusively before this (widget paint/layout
/// via `RenderRoot::set_theme`, and `use_context::<Theme>()` via
/// `provide_context`) — reflecting the change the next time the running shell
/// polls (once per frame; desktop before rebuild, mobile at the top of the
/// frame callback). [`clear_app_theme`] returns to the platform's own
/// light/dark-derived default. See `frust_shell_common::theme_override`'s
/// module docs for the full layering rationale, the thread contract (a plain
/// `Mutex`-guarded process-global — no UI-thread panic, unlike
/// [`push_deep_link`]), and the override-wins-over-appearance rule (an app
/// override, once set, is never overridden back by a live platform dark-mode
/// flip until [`clear_app_theme`] runs).
///
/// ```no_run
/// use frust::{Brightness, Theme, set_app_theme};
///
/// // Force one appearance end-to-end regardless of what the platform reports
/// // — e.g. an in-app light/dark toggle. Any `Theme` works here; a design
/// // system passes its own baseline instead of the neutral floor.
/// set_app_theme(Theme::neutral().with_brightness(Brightness::Dark));
/// ```
pub use frust_shell_common::{clear_app_theme, set_app_theme};

/// Design-system-facing base-theme seed:
/// [`set_default_theme`] supplies the *starting* theme a shell seeds itself
/// with, in place of its own built-in fallback — the seam a design-system
/// plugin's `install()` calls. Unlike [`set_app_theme`], this does NOT pin
/// brightness: the shell keeps re-deriving light/dark from the platform's own
/// appearance against this same base, so a design-system-themed app installed
/// this way still honours system dark mode. See
/// `frust_shell_common::theme_default`'s module docs for the full precedence
/// order ([`set_app_theme`] override → [`set_default_theme`] base → the
/// shell's built-in fallback) and the brightness-following contrast with
/// [`set_app_theme`] spelled out in full.
///
/// ```no_run
/// use frust::{Color, Theme, set_default_theme};
///
/// // A design-system plugin's install() call, seeding its own base theme as
/// // the app's starting point without pinning brightness. A real installer
/// // hands over its whole token set; this one edits a single role off the
/// // neutral floor to keep the example dependency-free.
/// let base = Theme::builder(Theme::neutral())
///     .map_colors_light(|mut c| {
///         c.primary = Color::from_rgb8(0x6B, 0x4E, 0xFF);
///         c
///     })
///     .build();
/// set_default_theme(base);
/// ```
pub use frust_shell_common::set_default_theme;

/// App-facing system-UI (system-bar) override:
/// [`set_system_ui_mode`] requests a status-/navigation-bar visibility mode —
/// the Flutter `SystemChrome.setEnabledSystemUIMode` analog — reaching
/// whichever shell is running the next time it polls (once per frame,
/// mirroring [`set_app_theme`]'s delivery timing). See
/// `frust_shell_common::system_ui`'s module docs for the full layering
/// rationale, the thread contract (a plain `Mutex`-guarded process-global,
/// callable from any thread), the FFI wire format each mobile shell exports, and
/// where Android/iOS diverge from the five-mode vocabulary.
///
/// ```no_run
/// use frust::{SystemUiMode, set_system_ui_mode};
///
/// // Hide all system bars; an edge swipe re-shows them.
/// set_system_ui_mode(SystemUiMode::Immersive);
/// ```
pub use frust_shell_common::{SystemUiMode, SystemUiOverlay, set_system_ui_mode};

/// App-facing pending-font registry:
/// [`register_app_fonts`] pushes raw font bytes (TTF/OTF, or a TTC/OTC
/// collection) to be registered into the running shell's `TextContext` the
/// next time it drains this registry (construction time, and once per
/// frame -- each shell's own wiring). See
/// `frust_shell_common::font_registry`'s module docs for the full layering
/// rationale and thread contract (mirrors [`set_app_theme`]'s: a plain
/// `Mutex`-guarded process-global, callable from any thread).
///
/// ```no_run
/// // Push bundled font bytes (e.g. loaded via `include_bytes!` at the app
/// // crate's own build) before or after the app starts running; the shell
/// // picks them up on its next drain.
/// let font_bytes: Vec<u8> = vec![];
/// frust::register_app_fonts(font_bytes);
/// ```
pub use frust_shell_common::font_registry::register_app_fonts;

// No app-facing translucent-surface opt-in lives here: Mode B is a
// build-time HOST configuration selected by the
// generated template's `FRUST_TRANSLUCENT_SURFACE`/`translucentSurface`
// constant, never a runtime Rust call — see [`platform_view`]'s Mode B
// section above. `frust_shell_common::declare_host_translucent_surface`
// exists only for the generated host glue (Android's `nativeSetSurfaceMode`,
// iOS's `frust_set_surface_mode`) to call from the same branch that already
// configured the native window translucent, and is deliberately not
// re-exported past that crate.

/// App-facing **resolved** surface mode — the
/// read-only outward half of the Mode B seam whose setter is deliberately
/// absent (see the comment above): what the platform actually gave this
/// process, not what the host asked for.
///
/// [`resolved_surface_mode`] answers [`ResolvedSurfaceMode::Unknown`] until a
/// shell publishes (no surface yet, or the desktop preview, which has no Mode
/// B host seam), then `Opaque`/`Translucent` — or
/// [`ResolvedSurfaceMode::RefusedTranslucent`], the case this exists for: the
/// host declared Mode B and the platform resolved the surface opaque anyway
/// (no matching `CompositeAlphaMode`; see `docs/NATIVE_WIDGETS_ARCHITECTURE.md`'s
/// Mode-B paragraph). frust's paint side degrades to
/// the Mode A contract on its own, but the host's native-sibling z-order was
/// fixed at build time, so a sibling arranged *behind* the surface is
/// invisible **and untappable**. Branch on
/// [`ResolvedSurfaceMode::translucency_refused`] to render a deliberate
/// fallback instead of a dead rect.
///
/// **This is a poll, not a subscription** — the same contract as
/// [`set_app_theme`]'s slot: reading it subscribes to nothing and a change
/// never wakes a frame by itself. Read it during a rebuild (or paint/an event
/// handler) on the UI thread, exactly where you'd read any other
/// process-global shell state, and if the answer must change your UI's shape,
/// write it into your own state so the normal dirty path runs.
///
/// ```no_run
/// use frust::{ResolvedSurfaceMode, resolved_surface_mode};
///
/// // A native-widget slot deciding whether its platform sibling can actually
/// // be seen this frame.
/// let native_sibling_visible = match resolved_surface_mode() {
///     ResolvedSurfaceMode::RefusedTranslucent => false,
///     ResolvedSurfaceMode::Unknown
///     | ResolvedSurfaceMode::Opaque
///     | ResolvedSurfaceMode::Translucent => true,
/// };
/// ```
pub use frust_shell_common::{ResolvedSurfaceMode, resolved_surface_mode};

/// The animation vocabulary: the shell-fed frame clock ([`FrameTime`])
/// plus the pure easing/interpolation/spring math a widget or app advances it
/// through, flat-re-exported from `frust-core::anim`. Time enters from the
/// shell during paint (`PaintCtx::frame_time`); nothing here reads a clock.
pub use frust_core::anim::{
    AnimationController, AnimationStatus, Curve, FrameTime, Lerp, Spring, SpringDesc, Tween,
};

/// The window's shape, flat-re-exported from `frust-core::app` so app code
/// (not just widget authors, see [`authoring::WindowMetrics`]) can recover it
/// via `use_context::<`[`WindowMetrics`]`>()` inside `Component::build` — a
/// component laying itself out around window size/scale/orientation reads
/// this the same way it reads a [`Theme`] via `use_context`. Delivered as a
/// plain value (not an `RwSignal`); see [`WindowMetrics`]'s own doc for the
/// derived-[`Orientation`] and rebuild-cost notes.
pub use frust_core::{Orientation, WindowMetrics};

/// The pointer-cursor vocabulary, flat-re-exported from `frust-core::event` (and
/// also available through [`authoring::CursorIcon`]).
///
/// A widget asks for a shape from its own pointer handling —
/// `ctx.set_cursor(CursorIcon::Pointer)` — and the desktop shell applies whatever
/// the pass resolved; the request is per-pass and stateless, so a widget that
/// stops asking falls back to [`CursorIcon::Default`] with nothing to clear. Lifted
/// flat rather than left inside [`authoring`] because a design-system plugin's
/// public builder API can *name* a cursor (a `Button::cursor(..)` override, a
/// disabled control asking for [`CursorIcon::NotAllowed`]) without being a widget
/// author itself. `#[non_exhaustive]`: match with a wildcard arm.
///
/// Honoured on desktop only — the mobile shells never read the resolved value, so
/// setting a cursor unconditionally is safe on every platform.
pub use frust_core::CursorIcon;

/// Pure input/gesture helpers (slop constants, [`input::VelocityTracker`], the
/// fling-decay math) re-exported for app authors and advanced widgets.
pub mod input {
    pub use frust_core::input::{
        FLING_DECAY, FLING_STOP, MOUSE_SLOP, TOUCH_SLOP, VELOCITY_WINDOW_MS, VelocityTracker,
        WHEEL_LINE_PX, fling_decay, fling_displacement,
    };
}

/// The reactive-programming vocabulary [`Component`] state is built
/// on: signals, memos, and context, flat-re-exported from `frust-reactive`/
/// `reactive_graph` so app authors never name either crate directly.
pub use frust_reactive::{RwSignal, on_cleanup, provide_context, use_context};

/// The deep-link read surface (`frust-reactive`'s `app_links`-
/// style process-wide source — see its module docs for the semantics): a
/// mobile shell delivers a platform link via `frust-reactive`'s
/// [`push_deep_link`], and app code reads it here —
/// [`deep_links()`] returns a [`DeepLinks`] snapshot ([`DeepLinks::initial`])
/// plus the live, trackable [`DeepLinks::latest`] signal a
/// [`Component::build`] reads to react to cold-start and subsequent links
/// uniformly. Router auto-wiring (resolving `deep_links()` against a
/// [`Router`]) is a separate opt-in, not automatic here.
///
/// ```no_run
/// use frust::{AnyView, Route, Router, any, deep_links, text};
///
/// struct AppState;
///
/// struct NavDemo;
///
/// fn build_router() -> Router<AppState> {
///     Router::new(vec![Route::new("/", |_params| -> AnyView<AppState> {
///         any(text("home"))
///     })])
/// }
///
/// impl frust::Component for NavDemo {
///     type State = AppState;
///
///     fn init(&self) -> AppState {
///         AppState
///     }
///
///     fn build(&self, _state: &mut AppState) -> AnyView<AppState> {
///         let _router = build_router();
///         // A late-subscribed read: `initial` sees a cold-start link (if any);
///         // `latest` is the live signal a rebuild tracks for warm links.
///         let links = deep_links();
///         let _ = links.initial;
///         let _ = links.latest;
///         any(text("nav demo"))
///     }
/// }
/// # let _ = NavDemo;
/// ```
/// [`push_deep_link`] is normally called by a mobile shell's platform-link
/// handler; it is also re-exported here as the desktop dev seam
/// (no shell writes on desktop yet) — `examples/navdemo`'s
/// "simulate deep link" button calls it directly to demonstrate warm-link
/// navigation without a real platform link.
pub use frust_reactive::{DeepLink, DeepLinks, deep_links, push_deep_link};

/// The Android back-press source (`frust-reactive`'s
/// process-wide back source — see its `back` module docs). A mobile shell
/// delivers a hardware/gesture back press via [`push_back_press`], and the
/// facade's [`BackHandler`] reads it via [`back_presses`] to pop a navigator;
/// [`set_handles_back`]/[`handles_back`] are the "framework consumes the next
/// back" flag a shell polls to decide whether a root-level back falls through
/// to the platform. App code normally uses [`attach_back_handler`] rather than
/// these directly; [`push_back_press`] is also the desktop dev seam (no shell
/// writes on desktop yet).
pub use frust_reactive::{
    BackPresses, back_presses, handles_back, push_back_press, set_handles_back,
};

/// The **menu-activation** read surface (`frust-reactive`'s process-wide menu
/// source — see its module docs): a per-OS desktop shell drains its native menu
/// queue once per frame and reports each activation, and app code observes it
/// here. [`menu_events()`] returns the live [`MenuEvents`] handle whose `latest`
/// signal carries a [`MenuEvent`] — the activated item's `id` exactly as the
/// app wrote it in its [`MenuSpec`], plus a monotonic `sequence` so choosing
/// the same item twice reads as two activations rather than one stale value.
///
/// ```no_run
/// use frust::{AnyView, Component, Get, any, menu_events, text};
///
/// #[derive(Default)]
/// struct MenuDemo;
///
/// impl Component for MenuDemo {
///     type State = ();
///
///     fn init(&self) -> Self::State {}
///
///     fn build(&self, _state: &mut Self::State) -> AnyView<Self::State> {
///         // A tracked read: this rebuild re-runs when an item is activated.
///         let label = match menu_events().latest.get() {
///             Some(event) => format!("chose {}", event.id),
///             None => "nothing chosen yet".to_string(),
///         };
///         any(text(label))
///     }
/// }
/// ```
///
/// Available on **every** target, unlike the [`MenuSpec`] vocabulary that
/// describes the menu itself: a component reading menu events compiles on
/// Android/iOS too (where nothing ever pushes one), so shared component code
/// needs no `cfg` of its own.
///
/// Unlike [`push_deep_link`], the *push* side is deliberately **not**
/// re-exported: a menu activation has exactly one producer — the per-OS shell
/// that owns the platform menu — and nothing in an app simulates one the way
/// `examples/navdemo` simulates a warm deep link.
pub use frust_reactive::{MenuEvent, MenuEvents, menu_events};
pub use reactive_graph::computed::Memo;
pub use reactive_graph::signal::{ReadSignal, WriteSignal, signal};
// The access traits the signal types' methods are defined through — without
// these in scope, `sig.get()`/`sig.set(..)`/`sig.update(..)` do not compile,
// so a facade-only consumer could name the types but never use them.
pub use reactive_graph::traits::{Get, GetUntracked, Set, Track, Update, With, WithUntracked};

/// Spawns a `Send` future on the background reactive runtime (Tokio-backed —
/// see `frust_reactive::ReactiveRuntime`). A thin wrapper over
/// `any_spawner::Executor::spawn`; app authors never name `any_spawner`.
pub fn spawn(fut: impl std::future::Future<Output = ()> + Send + 'static) {
    any_spawner::Executor::spawn(fut);
}

/// Spawns a `!Send` future on the UI-thread local task queue, drained each
/// frame by the shell (`ReactiveRuntime::pump_local`). A thin wrapper over
/// `any_spawner::Executor::spawn_local`; must be called on the UI thread —
/// see `frust_reactive::ReactiveRuntime::pump_local`'s doc for the panic
/// this triggers off-thread.
pub fn spawn_local(fut: impl std::future::Future<Output = ()> + 'static) {
    any_spawner::Executor::spawn_local(fut);
}

/// The heavy-work idiom: [`AsyncValue`] state,
/// [`use_task`] (the blessed load/compute helper), [`UseTask`] handle, and
/// [`spawn_blocking`] (the CPU-bound entry point) — Frust's counterpart to
/// Flutter's `compute()`/`FutureBuilder`, with explicit cancellation on
/// component teardown. `spawn_blocking` joins the existing
/// [`spawn`]/[`spawn_local`] routing pair (async IO / UI-thread `!Send` /
/// one-off CPU work). App crates need no new dependency: this is the whole
/// heavy-work surface. See `frust_reactive::task` for the threading contract.
pub use frust_reactive::{AsyncValue, TaskError, UseTask, spawn_blocking, use_task};

mod image_async;

/// Off-thread image decode: [`decode_image_async`] wraps
/// the existing synchronous `ImageSource::decode` in `spawn_blocking`, so it
/// composes with [`use_task`] for the full load/error/ready idiom without
/// ever blocking the UI thread on a decode. See `image_async`'s module docs
/// for why this lives in the facade rather than `frust-widgets` (which stays
/// reactive-free by charter) and for the Arc-move contract the decoded
/// [`ImageSource`] crosses threads under.
pub use image_async::{ImageDecodeError, decode_image_async};

// Re-export the Android JNI-bridge macro so generated apps write
// `frust::android_app!(AppState, build)`. `pub use` of a
// `#[macro_export]` macro re-exports it on edition 2021+; the macro only expands
// to real code where its call site is `#[cfg(target_os = "android")]`, so this is
// inert on desktop.
pub use frust_shell_android::android_app;

// Re-export the iOS C-ABI-bridge macro so generated apps write
// `frust::ios_app!(AppState, build)`. Unlike `android_app!`,
// the invocation is unconditional — the macro's generated `frust_*` exports
// are each `#[cfg(target_os = "ios")]`, so it is inert off-iOS.
pub use frust_shell_ios::ios_app;

// Hidden, wasm32-only re-export of the `wasm-bindgen` crate.
// [`web_app!`]'s generated `#[wasm_bindgen(start)]` shim references the
// attribute through `$crate::__wasm_bindgen::prelude::wasm_bindgen` rather
// than a bare `wasm_bindgen::prelude::wasm_bindgen`, so a generated app never
// needs `wasm-bindgen` in its own `Cargo.toml` — the same reason
// `frust-shell-android`'s `android_app!` routes its JNI types through that
// crate's own `__jni` re-export instead of naming `jni` directly (see that
// crate's `lib.rs`).
#[cfg(target_arch = "wasm32")]
#[doc(hidden)]
pub use wasm_bindgen as __wasm_bindgen;

// Hidden, wasm32-only re-export of the browser shell crate, for the identical
// reason as `__wasm_bindgen` above. [`web_app!`]'s generated shim hands the
// initialized state and build closure to `__frust_shell_web::run_app`
// (`fn run_app<State: 'static, Build, V>(state: State, build: Build)`,
// mirroring `frust_shell_desktop::run_desktop_with`'s `(state, build, ..)`
// convention), the browser shell's entry point: it wraps `spawn_app`, which
// owns the canvas-bound event loop and the `requestAnimationFrame` frame
// pipeline, and reports an event-loop construction failure through the `log`
// facade because the `wasm_bindgen(start)` shim has nowhere to return one to.
// The reference lives inside a `macro_rules!` body, so it is type-checked
// only where a caller invokes [`web_app!`]/[`app!`] for a wasm32 target —
// `cargo check --target wasm32-unknown-unknown -p frust-ui --tests` covers it.
#[cfg(target_arch = "wasm32")]
#[doc(hidden)]
pub use frust_shell_web as __frust_shell_web;

/// Browser panic hook + console log sink, installed once at the top of
/// [`web_app!`]'s generated start shim — the wasm32 counterpart of the
/// bring-up boilerplate a desktop binary gets for free from a terminal.
/// Without the panic hook a Rust panic reaches the browser console as a bare
/// `unreachable` trap with no message; without the log sink, `log::warn!`
/// and friends (including `frust-render`/`frust-gpu`/`wgpu`'s own records)
/// go nowhere, since stderr is a silent no-op in a browser. Mirrors the
/// proven shape in the browser render probe (`examples/web-spike/src/main.rs`'s
/// `mod web::start`), at `Warn` rather than that probe's `Debug`/`Info` — a
/// shipped app's default should not carry wgpu's naga typifier chatter.
///
/// `#[doc(hidden)]` and free-standing (not inside the macro body): this
/// function only calls real, already-present dependencies
/// (`console_error_panic_hook`, `console_log`, `log`), so it is safe to keep
/// as ordinary always-compiled code rather than deferring it into
/// `web_app!`'s uninstantiated macro text the way [`__frust_shell_web`]'s
/// `run_app` reference must be.
#[cfg(target_arch = "wasm32")]
#[doc(hidden)]
pub fn __web_bootstrap() {
    console_error_panic_hook::set_once();
    let _ = console_log::init_with_level(log::Level::Warn);
}

/// Brings up the process-wide [`frust_reactive::ReactiveRuntime`] (the w0-04
/// wasm arm — `ReactiveRuntime::init`'s `#[cfg(target_family = "wasm")]` arm
/// calls `Executor::init_wasm_bindgen()` explicitly, since no automatic wasm
/// executor default exists; see `frust_reactive::runtime`'s module docs) and
/// runs `state_init` under its root [`frust_reactive::Owner`], exactly the
/// [`run_with_setup_and_config`] shape every other platform's entry uses —
/// so a signal or context `state_init` creates already has a runtime and an
/// owner to be created under.
///
/// Real, already-present dependencies only (`frust-reactive`), so — like
/// [`__web_bootstrap`] — this stays ordinary always-compiled code rather than
/// living inside [`web_app!`]'s macro text.
#[cfg(target_arch = "wasm32")]
#[doc(hidden)]
pub fn __web_init_state<State: 'static>(state_init: impl FnOnce() -> State) -> State {
    let rt = frust_reactive::ReactiveRuntime::init(std::sync::Arc::new(|| {}));
    rt.with_owner(state_init)
}

/// Install the baseline framework-drawn selection-toolbar view
/// ([`frust_widgets::selection_toolbar`]) as the process's default
/// [`SelectionToolbarBuilder`](frust_core::SelectionToolbarBuilder), **set-if-
/// unset** (`frust_core::install_selection_toolbar_builder_if_unset`) — so a
/// design system that already installed its own keeps it, whether that
/// install happened earlier in `main` (before `frust::app!`/`frust::run`
/// ran at all) or inside an `app!` `setup = { .. }` block.
///
/// Called once, on the UI thread, at the same point on every platform this
/// crate starts an app from — after any `setup = { .. }` block and
/// immediately before the root [`Component::init`] (Android/iOS/wasm32, from
/// inside [`app!`]'s `@emit_mobile` closures) or before the desktop shell is
/// constructed ([`run_desktop_configured`], which every desktop entry point —
/// [`App::run`], [`run`], [`run_with_setup`], [`run_desktop_config`],
/// [`run_with_setup_and_config`] and `app!`'s desktop arm — funnels through).
/// Running it *after* setup, not before, is deliberate: a design system's own
/// cooperative install (also `install_selection_toolbar_builder_if_unset`,
/// called from inside a `setup = { .. }` block) must get first claim on the
/// empty slot, or this baseline would win the race and leave the design
/// system's own catalog toolbar never installed.
///
/// `#[doc(hidden)]`, not part of the public API: reached only through
/// `$crate::` from [`app!`]'s macro expansion and this crate's own desktop
/// entry points, exactly like [`__web_bootstrap`]/[`__web_init_state`].
#[doc(hidden)]
pub fn __install_default_selection_toolbar() {
    frust_core::install_selection_toolbar_builder_if_unset(std::sync::Arc::new(|req, _win| {
        frust_widgets::selection_toolbar(req)
    }));
}

/// The browser counterpart of [`android_app!`]/[`ios_app!`]: binds a
/// [`Component`]'s state and build closure to the browser shell's wasm-bindgen
/// entry point. Takes the identical two-argument (state type + build
/// expression, state built via `Default`) / three-argument (state type +
/// explicit state-init expression + build expression) shape
/// `android_app!` does, and `app!` drives it through the three-argument form
/// exactly the way it drives `android_app!`/`ios_app!` (see `@emit_mobile`
/// below) — most apps reach this through `app!`/`web_app!` rather than
/// hand-writing the explicit `state_init` form.
///
/// Expands to two `#[cfg(target_arch = "wasm32")]` functions — so, like
/// [`ios_app!`], the invocation itself is unconditional and self-gating:
/// calling `web_app!` off wasm32 expands to nothing. `__frust_web_start` is
/// `#[wasm_bindgen(start)]` — wasm-bindgen's own module-init entry point, run
/// once when the browser instantiates the compiled `.wasm` — and its body is
/// a single call into `__frust_web_run`, kept deliberately separate: feeding
/// `wasm_bindgen`'s attribute macro a body built straight out of
/// `$state_init`/`$build` (closures that can carry a macro-substituted
/// `$($setup)?` block nested inside another closure — see `app!`'s
/// `@emit_mobile` arm) trips its own re-parse of the function into a spurious
/// syntax error on that nested-block shape; a plain, macro-fragment-free call
/// is all it ever sees. `__frust_web_run` carries the real work, in order:
///
/// 1. [`__web_bootstrap`]: installs the panic hook and console log sink.
/// 2. [`__web_init_state`]: brings up the [`frust_reactive::ReactiveRuntime`]
///    and runs `$state_init` under its root owner — the point at which a
///    `setup = { .. }` block bundled into `$state_init` by `app!` (see
///    `@emit_mobile`) runs, identically ordered to every other platform.
/// 3. Hands the initialized state and `$build` to
///    `frust_shell_web::run_app` (via the hidden [`__frust_shell_web`]
///    re-export) — the browser shell's own entry point, which owns the
///    canvas-bound event loop and the `requestAnimationFrame` frame pipeline
///    (see [`__frust_shell_web`]'s doc comment for the contract).
#[macro_export]
macro_rules! web_app {
    ($state_ty:ty, $build:expr $(,)?) => {
        $crate::web_app!(
            $state_ty,
            <$state_ty as ::core::default::Default>::default,
            $build
        );
    };
    ($state_ty:ty, $state_init:expr, $build:expr $(,)?) => {
        // Factored out of the `#[wasm_bindgen(start)]` function below rather
        // than inlined into it: `wasm_bindgen`'s attribute macro re-parses
        // the function it is attached to, and a body built straight out of
        // `$state_init`/`$build` — themselves closures that may carry a
        // macro-substituted `$($setup)?` block nested inside another closure
        // (see `app!`'s `@emit_mobile` arm) — trips it into a spurious parse
        // error on that nested-block shape. A plain, macro-fragment-free call
        // is all `#[wasm_bindgen(start)]` ever sees; this function carries
        // the real work instead.
        #[cfg(target_arch = "wasm32")]
        fn __frust_web_run() {
            $crate::__web_bootstrap();
            let __frust_state: $state_ty = $crate::__web_init_state($state_init);
            $crate::__frust_shell_web::run_app(__frust_state, $build);
        }

        #[cfg(target_arch = "wasm32")]
        #[$crate::__wasm_bindgen::prelude::wasm_bindgen(start)]
        pub fn __frust_web_start() {
            __frust_web_run();
        }
    };
}

/// The desktop app's identity and native-integration vocabulary, re-exported
/// from the desktop core so app code never names a shell crate:
/// [`DesktopConfig`] (the whole declaration — app name, reverse-DNS id, window
/// icon, menu bar, last-window-close policy — handed to [`App::desktop`],
/// [`run_desktop_config`] or [`app!`]'s `desktop = { .. }` argument),
/// [`MenuSpec`]/[`MenuItemSpec`]/[`MenuRole`] (the platform-independent native
/// menu tree a per-OS shell translates into an NSApp menu bar or an `HMENU`),
/// [`DesktopIconData`] (decoded, tightly-packed RGBA8 — decoding a
/// PNG/ICO/ICNS is the caller's job), and [`DEFAULT_APP_NAME`] (the window
/// title a config that names nothing still gets).
///
/// `frust-shell-desktop`'s `IconData` is re-exported **renamed**: the facade
/// already carries `frust-widgets`' [`IconData`], an in-UI vector icon, and the
/// two are unrelated (one is a `BezPath` a widget paints, the other is the
/// window/taskbar bitmap the OS shows). The `Desktop` prefix matches
/// [`DesktopConfig`], whose `with_window_icon` is its only consumer.
///
/// ```no_run
/// use frust::{DesktopConfig, MenuItemSpec, MenuRole, MenuSpec};
///
/// let file = MenuSpec::new()
///     .with_item(MenuItemSpec::item("file.open", "Open…").with_accelerator("CmdOrCtrl+O"))
///     .with_item(MenuItemSpec::separator())
///     .with_item(MenuItemSpec::role(MenuRole::Quit));
///
/// let config = DesktopConfig::new()
///     .with_app_name("Huddle")
///     .with_app_id("dev.frust.huddle")
///     .with_menu_spec(MenuSpec::new().with_item(MenuItemSpec::submenu("File", file)));
/// # let _ = config;
/// ```
///
/// **Desktop-only**, unlike the [`menu_events`] read side: these types are
/// defined in `frust-shell-desktop`, which is not in a mobile build's or a
/// wasm32 build's dependency graph at all (see this crate's `Cargo.toml`).
/// Code shared with a mobile or wasm32 target keeps a `DesktopConfig` behind
/// its own
/// `#[cfg(not(any(target_os = "android", target_os = "ios", target_arch = "wasm32")))]`
/// — or, more simply, writes it inline in [`app!`]'s `desktop = { .. }`
/// argument, which the macro already emits only on the targets that have it.
#[cfg(not(any(target_os = "android", target_os = "ios", target_arch = "wasm32")))]
pub use frust_shell_desktop::{
    DEFAULT_APP_NAME, DesktopConfig, IconData as DesktopIconData, MenuItemSpec, MenuRole, MenuSpec,
};

/// A Frust application: the app state plus the `build` closure that maps it
/// to a view tree.
///
/// This is the low-level primitive. [`app!`] and [`run`] expand into
/// `App::new(state, move |state| root.build(state))` (see
/// [`run_with_setup_and_config`]); ordinary apps write a [`Component`] and
/// start it with those instead of constructing an `App` by hand.
///
/// Construct with [`App::new`] and start the event loop with [`App::run`],
/// optionally naming a desktop identity with [`App::desktop`] in between.
///
/// The view type is intentionally *not* a parameter of this struct: capturing a
/// build fn's `impl View<State>` opaque return type into a stored type
/// parameter defeats method resolution (the opaque type's trait bounds can't
/// be re-proven on the already-typed value). Instead [`App::run`] infers the
/// view type freshly at the call site, so `App::new(state, build).run()`
/// compiles for both `impl View` and concrete-typed `build`.
// On a mobile target the fields are consumed only by the desktop-gated `run`,
// so they read as dead there; the app is driven through `android_app!`/JNI or
// `ios_app!`/C-ABI instead.
#[cfg_attr(
    any(target_os = "android", target_os = "ios", target_arch = "wasm32"),
    allow(dead_code)
)]
pub struct App<State, Build> {
    state: State,
    build: Build,
    /// The desktop identity [`App::run`] hands the shell —
    /// [`DesktopConfig::default()`] (today's zero-config preview window) unless
    /// [`App::desktop`] replaced it.
    ///
    /// Absent on mobile rather than carried and ignored: the type itself lives
    /// in the desktop core, which is not in a mobile build's graph at all.
    #[cfg(not(any(target_os = "android", target_os = "ios", target_arch = "wasm32")))]
    config: DesktopConfig,
}

impl<State, Build> App<State, Build> {
    /// Create an app from an initial `state` and its `build` closure.
    ///
    /// `build` is a `FnMut(&mut State) -> impl View<State>` re-run each frame to
    /// produce the current view tree.
    pub fn new(state: State, build: Build) -> Self {
        Self {
            state,
            build,
            #[cfg(not(any(target_os = "android", target_os = "ios", target_arch = "wasm32")))]
            config: DesktopConfig::default(),
        }
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios", target_arch = "wasm32")))]
impl<State: 'static, Build> App<State, Build> {
    /// Give the app a desktop identity — name, reverse-DNS id, window icon,
    /// native menu bar, last-window-close policy (see [`DesktopConfig`]).
    ///
    /// Optional: an app that never calls this runs with
    /// [`DesktopConfig::default()`], which is the dev-preview window exactly as
    /// it has always been. Each per-OS shell reads the fields it can act on and
    /// ignores the rest (Linux has no native menu bar, macOS wants an
    /// application icon rather than a window one, and so on).
    ///
    /// ```no_run
    /// # struct AppState;
    /// # fn build(_: &mut AppState) -> impl frust::View<AppState> + use<> { frust::text("hi") }
    /// // `App::new` is the low-level primitive; apps normally use a `Component`.
    /// frust::App::new(AppState, build)
    ///     .desktop(frust::DesktopConfig::new().with_app_name("Huddle"))
    ///     .run()
    ///     .unwrap();
    /// ```
    pub fn desktop(mut self, config: DesktopConfig) -> Self {
        self.config = config;
        self
    }

    /// Run the app in the desktop window until it is closed.
    ///
    /// Blocks the calling thread on the platform event loop. Returns once the
    /// window closes, or an error if the window/GPU surface could not be
    /// created. The concrete view type `V` is inferred from `build`.
    ///
    /// Desktop-only: on Android the app is driven by the JNI bridge that
    /// [`android_app!`] generates and on iOS by the C-ABI entry points
    /// [`ios_app!`] generates, not by this loop.
    pub fn run<V>(self) -> anyhow::Result<()>
    where
        V: View<State>,
        Build: FnMut(&mut State) -> V + 'static,
    {
        let Self {
            state,
            build,
            config,
        } = self;
        run_desktop_configured(state, build, config)
    }
}

/// Start the shared desktop core with this target's native shell attached.
///
/// The extension is built *before* the config moves into the core: every per-OS
/// constructor takes `&DesktopConfig` and clones out the fields it acts on
/// (`app_id`, `window_icon`, `menu_spec`, the close policy), while the core
/// itself takes ownership to title the window.
///
/// This is the single desktop choke point every desktop entry funnels
/// through — [`App::run`] directly, and [`run`]/[`run_with_setup`]/
/// [`run_desktop_config`]/[`run_with_setup_and_config`]/`app!`'s desktop arm
/// indirectly, via `App::new(..).desktop(config).run()` — which is why
/// [`__install_default_selection_toolbar`] sits here rather than duplicated
/// across each of those: whatever path an app took to get here, any `setup`
/// it ran (and any explicit override that ran even earlier, in `main`) has
/// already had its chance to claim the selection-toolbar builder slot before
/// this call, and this is the one point strictly before the shell — and
/// therefore the first frame — starts.
#[cfg(not(any(target_os = "android", target_os = "ios", target_arch = "wasm32")))]
fn run_desktop_configured<State, Build, V>(
    state: State,
    build: Build,
    config: DesktopConfig,
) -> anyhow::Result<()>
where
    State: 'static,
    V: View<State>,
    Build: FnMut(&mut State) -> V + 'static,
{
    __install_default_selection_toolbar();
    let extensions = desktop_extensions(&config);
    frust_shell_desktop::run_desktop_with(state, build, config, extensions)
}

/// The per-OS shell selection: one arm per shell crate this crate's manifest
/// gates in, under the identical `cfg` the dependency itself carries.
///
/// This function is the whole of the facade's platform knowledge on desktop —
/// nothing else here names a per-OS shell crate.
#[cfg(target_os = "macos")]
fn desktop_extensions(
    config: &DesktopConfig,
) -> impl frust_shell_desktop::DesktopExtensions + use<> {
    frust_shell_macos::MacosExtensions::new(config)
}

/// See the macOS arm above.
#[cfg(target_os = "windows")]
fn desktop_extensions(
    config: &DesktopConfig,
) -> impl frust_shell_desktop::DesktopExtensions + use<> {
    frust_shell_windows::WindowsExtensions::new(config)
}

/// See the macOS arm above.
#[cfg(target_os = "linux")]
fn desktop_extensions(
    config: &DesktopConfig,
) -> impl frust_shell_desktop::DesktopExtensions + use<> {
    frust_shell_linux::LinuxExtensions::new(config)
}

/// The fallback arm: a desktop target with no per-OS shell crate of its own (a
/// BSD, say) runs the shared winit core with the whole-set no-op extension —
/// window, input, theme and accessibility all work, and only the native
/// identity/menu integration is absent. `config` is still threaded through, so
/// such a host keeps its window title.
#[cfg(not(any(
    target_os = "android",
    target_os = "ios",
    target_os = "macos",
    target_os = "windows",
    target_os = "linux",
    target_arch = "wasm32"
)))]
fn desktop_extensions(
    config: &DesktopConfig,
) -> impl frust_shell_desktop::DesktopExtensions + use<> {
    let _ = config;
    frust_shell_desktop::NoExtensions
}

/// Run a root [`Component`] in the desktop preview shell until the window
/// closes — the canonical `runApp` equivalent for the Component model.
///
/// `root.init()` seeds the component's retained `State` once; the resulting
/// `AnyView<C::State>` is then driven through the same desktop preview loop
/// [`App::run`] uses, rebuilding from `root.build(state)` every frame.
///
/// Initializes the process-wide [`frust_reactive::ReactiveRuntime`] with a
/// no-op waker *before* `root.init()` runs, since a signal or context created
/// there must already have a runtime to be created under — the desktop
/// preview loop's own later `ReactiveRuntime::init` call swaps in the real
/// proxy waker on top of this seed, the documented swap-on-reinit behavior.
///
/// `root.init()` runs under the runtime's root [`Owner`] (via
/// [`with_owner`](frust_reactive::ReactiveRuntime::with_owner)): `init`
/// unblocks signal/executor creation, but `provide_context`/`on_cleanup` are
/// no-ops unless an `Owner` is ambient, and a root component has no enclosing
/// component to supply one — so the root component registers against the root
/// owner (process lifetime, never disposed). The desktop loop separately wraps
/// each per-frame rebuild in the same owner.
///
/// Desktop-only, matching [`App::run`]: on Android the app is driven by
/// [`android_app!`]/JNI and on iOS by [`ios_app!`]'s C-ABI entry points
/// instead, not by this loop.
///
/// Zero-config: the window is [`DesktopConfig::default()`]'s. Name the app, its
/// icon or its menu bar with [`run_desktop_config`] (or [`app!`]'s
/// `desktop = { .. }` argument) instead.
#[cfg(not(any(target_os = "android", target_os = "ios", target_arch = "wasm32")))]
pub fn run<C: Component>(root: C) -> anyhow::Result<()> {
    run_with_setup(root, || {})
}

/// [`run`] with a **setup** closure run once, immediately before
/// `root.init()` — the desktop half of [`app!`]'s `setup = { .. }` block.
///
/// `setup` runs on this (the UI) thread, after
/// [`frust_reactive::ReactiveRuntime::init`] and under the runtime's root
/// [`Owner`], and therefore *before* the desktop shell is constructed — which
/// is the one moment the shell reads [`set_default_theme`]'s slot and drains
/// [`register_app_fonts`]' registry. That is exactly the placement `app!`'s
/// Android/iOS arms get for free (both run the block inside the state factory
/// `create_handle`/`ffi_glue::init` calls before building their `AppHandle`),
/// so the ordering contract is identical on all three platforms.
///
/// A design-system plugin's installer (`frust_glyph::install`, or any other
/// design system's equivalent) is the intended payload; app code normally
/// reaches this through [`app!`] rather than calling it directly.
///
/// Desktop-only, matching [`run`]/[`App::run`].
#[cfg(not(any(target_os = "android", target_os = "ios", target_arch = "wasm32")))]
pub fn run_with_setup<C: Component>(root: C, setup: impl FnOnce()) -> anyhow::Result<()> {
    run_with_setup_and_config(root, setup, DesktopConfig::default())
}

/// [`run`] with the app's desktop identity ([`DesktopConfig`]) — the
/// no-setup half of the config-carrying pair, and the function
/// [`app!`]'s `desktop = { .. }` argument routes through when no
/// `setup = { .. }` block accompanies it.
///
/// ```no_run
/// # use frust::{AnyView, Component, any, text};
/// # #[derive(Default)]
/// # struct MyApp;
/// # impl Component for MyApp {
/// #     type State = ();
/// #     fn init(&self) -> Self::State {}
/// #     fn build(&self, _state: &mut Self::State) -> AnyView<Self::State> { any(text("hi")) }
/// # }
/// frust::run_desktop_config(
///     MyApp,
///     frust::DesktopConfig::new()
///         .with_app_name("Huddle")
///         .with_app_id("dev.frust.huddle"),
/// )
/// .unwrap();
/// ```
///
/// Desktop-only, matching [`run`]/[`App::run`].
#[cfg(not(any(target_os = "android", target_os = "ios", target_arch = "wasm32")))]
pub fn run_desktop_config<C: Component>(root: C, config: DesktopConfig) -> anyhow::Result<()> {
    run_with_setup_and_config(root, || {}, config)
}

/// [`run_with_setup`] and [`run_desktop_config`] at once: the single desktop
/// entry point the other three delegate to, differing only in which of `setup`
/// and `config` they default.
///
/// `setup` keeps the ordering contract [`run_with_setup`] documents; `config`
/// reaches the shared desktop core and this target's native shell together (see
/// [`App::desktop`]).
///
/// Desktop-only, matching [`run`]/[`App::run`].
#[cfg(not(any(target_os = "android", target_os = "ios", target_arch = "wasm32")))]
pub fn run_with_setup_and_config<C: Component>(
    root: C,
    setup: impl FnOnce(),
    config: DesktopConfig,
) -> anyhow::Result<()> {
    let rt = frust_reactive::ReactiveRuntime::init(std::sync::Arc::new(|| {}));
    let state = rt.with_owner(|| {
        setup();
        root.init()
    });
    App::new(state, move |state: &mut C::State| root.build(state))
        .desktop(config)
        .run()
}

/// The canonical app entry point: one line binds a root
/// [`Component`] to all three platforms.
///
/// ```no_run
/// use frust::{AnyView, Component, any, text};
///
/// #[derive(Default)]
/// struct App;
///
/// impl Component for App {
///     type State = i32;
///
///     fn init(&self) -> i32 {
///         0
///     }
///
///     fn build(&self, state: &mut i32) -> AnyView<i32> {
///         any(text(format!("count: {state}")).size(32.0))
///     }
/// }
///
/// frust::app!(App);
/// # fn main() {}
/// ```
///
/// Expands to:
/// - **Android**: `#[cfg(target_os = "android")] frust::android_app!(...)`
///   bound to `Root::State`/`Root::build`, mirroring [`android_app!`]'s own
///   not-self-gating contract (its generated symbols only compile where the
///   Android FFI glue they call into exists).
/// - **iOS**: `frust::ios_app!(...)`, invoked unconditionally — like a
///   direct `ios_app!` call, it self-gates: every symbol it emits is itself
///   `#[cfg(target_os = "ios")]`, so the invocation expands to nothing
///   off-iOS. Plus a `#[cfg(target_os = "ios")] __frust_main` **stub**: the
///   generated `main.rs` calls `__frust_main()` under
///   `#[cfg(not(target_os = "android"))]`, and Xcode builds that bin target,
///   so the symbol must exist on iOS even though no desktop shell does. The
///   stub explains itself on stderr and exits non-zero rather than silently
///   doing nothing — a real iOS app is launched by its Xcode host through the
///   C-ABI entry points above, never by running this binary.
/// - **Every other target** (desktop): a hidden
///   `#[doc(hidden)] pub fn __frust_main()` that runs `Root` through
///   [`run`] (or, with a `desktop = { .. }` argument,
///   [`run_with_setup_and_config`]), printing the error and exiting non-zero
///   on failure. `app!`
///   never emits a `fn main` itself — a generated project splits `lib.rs`
///   (where `app!` is called) from `main.rs` (a one-line `fn main() {
///   <crate>::__frust_main() }`, scaffolded by `frust create` and never
///   hand-edited), since only the binary crate may define `main`.
///
/// `Root` must implement `Component + Default`: the macro constructs a `Root`
/// twice with no arguments (once to build each platform's state factory,
/// once to close over `build`) — a root component is stateless configuration
/// (any real data lives in its `Component::State`), so two independent,
/// short-lived instances are inexpensive and behaviorally identical.
///
/// # `setup = { .. }`: run code before the shell exists
///
/// An optional second argument is a **setup block** the macro emits at the top
/// of each platform's entry, before the shell is constructed:
///
/// ```no_run
/// # use frust::{AnyView, Component, any, text};
/// # #[derive(Default)]
/// # struct MyApp;
/// # impl Component for MyApp {
/// #     type State = ();
/// #     fn init(&self) -> Self::State {}
/// #     fn build(&self, _state: &mut Self::State) -> AnyView<Self::State> { any(text("hi")) }
/// # }
/// frust::app!(MyApp, setup = { my_design_system::install(); });
/// # mod my_design_system {
/// #     pub fn install() { frust::set_default_theme(frust::Theme::neutral()); }
/// # }
/// # fn main() {}
/// ```
///
/// This exists because a design system's installer must reach
/// [`set_default_theme`]/[`register_app_fonts`] *before* a shell reads them,
/// and Android has no `main` at all (its entry is the JNI `nativeInit`
/// `android_app!` generates). Any plugin can use it; it is not tied to one
/// design system — `frust_glyph::install` is just the first caller.
///
/// **When it runs**, identically on all three platforms: on the UI thread,
/// after `ReactiveRuntime::init`, under the root reactive `Owner`, immediately
/// before the root component's `Component::init` — and therefore before any
/// shell construction, which is the one point a shell reads
/// `set_default_theme`'s slot and drains the pending-font registry. The
/// *expansions* differ per platform (Android/iOS place the block inside the
/// state factory `android_app!`/`ios_app!` receive, which
/// `jni_glue::create_handle`/`ffi_glue::init` call before building their
/// `AppHandle`; the desktop arm routes through [`run_with_setup`]); the
/// ordering contract above is what is guaranteed.
///
/// The one-argument form is unchanged: no setup tokens are emitted, the two
/// mobile state factories expand to exactly what they always did, and the
/// desktop arm's `run_with_setup(root, || {})` is literally what [`run`] itself
/// is.
///
/// # `desktop = { .. }`: name the app on desktop
///
/// A second optional argument, alongside or instead of `setup`, is an
/// expression evaluating to a [`DesktopConfig`] — the app's desktop identity
/// (name, reverse-DNS id, window icon, native menu bar, close policy):
///
/// ```no_run
/// # use frust::{AnyView, Component, any, text};
/// # #[derive(Default)]
/// # struct MyApp;
/// # impl Component for MyApp {
/// #     type State = ();
/// #     fn init(&self) -> Self::State {}
/// #     fn build(&self, _state: &mut Self::State) -> AnyView<Self::State> { any(text("hi")) }
/// # }
/// frust::app!(MyApp, desktop = {
///     frust::DesktopConfig::new()
///         .with_app_name("Huddle")
///         .with_app_id("dev.frust.huddle")
///         .with_menu_spec(
///             frust::MenuSpec::new()
///                 .with_item(frust::MenuItemSpec::role(frust::MenuRole::Quit)),
///         )
/// });
/// # fn main() {}
/// ```
///
/// The two may be combined in either order
/// (`setup = { .. }, desktop = { .. }` or the reverse). The expression is
/// emitted **only** into the desktop `__frust_main`, so it may name
/// desktop-only types like [`DesktopConfig`] without any `cfg` of its own and
/// the mobile arms never see it — an app whose menu/identity matters on desktop
/// still compiles unchanged for Android and iOS. It is evaluated once, at
/// startup: the macro passes it straight into the run call, so it runs
/// *before* the `setup` block (which runs inside, under the reactive owner) and
/// before the shell is constructed — a config expression must not depend on
/// what `setup` installs.
///
/// Omitting it is exactly today's behavior:
/// [`DesktopConfig::default()`]'s window, through the same
/// [`run_with_setup`] call the pre-config macro emitted.
#[macro_export]
macro_rules! app {
    // Internal arms, shared by the public forms below. `$($setup:block)?` is
    // empty for the one-argument form, so the two mobile state factories expand
    // byte-identically to the pre-setup macro and the desktop arm's
    // `run_with_setup(root, || {})` is `run`'s own body. Listed FIRST because
    // macro_rules cannot recover from a `$root:ty` fragment that fails to parse:
    // were the public arms first, `@emit` would be fed to `:ty` and error out
    // instead of falling through to these arms.
    //
    // The mobile half is factored into `@emit_mobile` because it is identical
    // for every public form — only the desktop `__frust_main` differs between
    // `@emit` (zero-config) and `@emit_desktop` (config-carrying), and two
    // hand-maintained copies of the JNI/C-ABI bindings would be free to drift.
    (@emit_mobile $root:ty, $($setup:block)?) => {
        #[cfg(target_os = "android")]
        $crate::android_app!(
            <$root as $crate::Component>::State,
            || {
                $($setup)?
                $crate::__install_default_selection_toolbar();
                $crate::Component::init(&<$root as ::core::default::Default>::default())
            },
            {
                let __frust_root = <$root as ::core::default::Default>::default();
                move |state: &mut <$root as $crate::Component>::State| {
                    $crate::Component::build(&__frust_root, state)
                }
            }
        );

        $crate::ios_app!(
            <$root as $crate::Component>::State,
            || {
                $($setup)?
                $crate::__install_default_selection_toolbar();
                $crate::Component::init(&<$root as ::core::default::Default>::default())
            },
            {
                let __frust_root = <$root as ::core::default::Default>::default();
                move |state: &mut <$root as $crate::Component>::State| {
                    $crate::Component::build(&__frust_root, state)
                }
            }
        );

        // The fourth platform: [`web_app!`] self-gates the same way
        // `ios_app!` does (its one generated symbol is itself
        // `#[cfg(target_arch = "wasm32")]`), so this call is additionally
        // gated here too — belt-and-braces, matching `android_app!`'s
        // call-site-gated style right above, and making the wasm32 arm easy
        // to find beside the other two platforms' calls without reading
        // `web_app!`'s own body.
        #[cfg(target_arch = "wasm32")]
        $crate::web_app!(
            <$root as $crate::Component>::State,
            || {
                $($setup)?
                $crate::__install_default_selection_toolbar();
                $crate::Component::init(&<$root as ::core::default::Default>::default())
            },
            {
                let __frust_root = <$root as ::core::default::Default>::default();
                move |state: &mut <$root as $crate::Component>::State| {
                    $crate::Component::build(&__frust_root, state)
                }
            }
        );

        // The iOS `__frust_main` stub. The generated `main.rs` calls
        // `__frust_main()` under `#[cfg(not(target_os = "android"))]` and the
        // Xcode phase builds that bin target, so the symbol must exist on iOS
        // even though iOS has no desktop shell to start. It logs and exits
        // non-zero rather than returning quietly: reaching it means something
        // ran the binary directly instead of letting the Xcode host drive
        // `ios_app!`'s C-ABI entry points, and a silent success would look like
        // an app that started and vanished.
        #[cfg(target_os = "ios")]
        #[doc(hidden)]
        pub fn __frust_main() {
            eprintln!(
                "frust: __frust_main is the desktop entry point and does nothing on iOS \
                 — an iOS app is started by its Xcode host, which drives the C-ABI entry \
                 points `frust::app!` generates."
            );
            ::std::process::exit(1);
        }
    };
    // Zero-config desktop entry: byte-identical to the pre-config macro.
    (@emit $root:ty, $($setup:block)?) => {
        $crate::app!(@emit_mobile $root, $($setup)?);

        #[cfg(not(any(target_os = "android", target_os = "ios", target_arch = "wasm32")))]
        #[doc(hidden)]
        pub fn __frust_main() {
            let __frust_setup = || { $($setup)? };
            if let Err(e) = $crate::run_with_setup(
                <$root as ::core::default::Default>::default(),
                __frust_setup,
            ) {
                eprintln!("frust: {e:#}");
                ::std::process::exit(1);
            }
        }
    };
    // Config-carrying desktop entry. `$config` is emitted only inside this
    // `cfg`-gated function, so it may name desktop-only types (`DesktopConfig`
    // and the menu vocabulary) while the mobile arms above stay untouched.
    (@emit_desktop $root:ty, $config:expr, $($setup:block)?) => {
        $crate::app!(@emit_mobile $root, $($setup)?);

        #[cfg(not(any(target_os = "android", target_os = "ios", target_arch = "wasm32")))]
        #[doc(hidden)]
        pub fn __frust_main() {
            let __frust_setup = || { $($setup)? };
            if let Err(e) = $crate::run_with_setup_and_config(
                <$root as ::core::default::Default>::default(),
                __frust_setup,
                $config,
            ) {
                eprintln!("frust: {e:#}");
                ::std::process::exit(1);
            }
        }
    };
    ($root:ty $(,)?) => {
        $crate::app!(@emit $root,);
    };
    ($root:ty, setup = $setup:block $(,)?) => {
        $crate::app!(@emit $root, $setup);
    };
    ($root:ty, desktop = $config:expr $(,)?) => {
        $crate::app!(@emit_desktop $root, $config,);
    };
    ($root:ty, setup = $setup:block, desktop = $config:expr $(,)?) => {
        $crate::app!(@emit_desktop $root, $config, $setup);
    };
    // The same pair the other way round: an app author writing the identity
    // first should not meet a macro error over argument order.
    ($root:ty, desktop = $config:expr, setup = $setup:block $(,)?) => {
        $crate::app!(@emit_desktop $root, $config, $setup);
    };
}

/// Compile-only smoke of [`app!`]'s `setup = { .. }` form: a
/// `Component + Default` fixture bound to all four platforms in one call —
/// `cargo test --workspace` compiles this on host (criterion 1: `__frust_main`
/// present, no Android JNI symbols), and `cargo check --target
/// aarch64-linux-android -p frust-ui --tests` / `--target
/// aarch64-apple-ios-sim -p frust-ui --tests` compile it for the two mobile
/// targets (criterion 2: the respective platform's exports appear,
/// `__frust_main` absent on Android). Lives behind `cfg(test)` — never
/// linked into a cdylib/staticlib/binary, so the fixed JNI/C-ABI export names
/// `app!` stamps out (via `android_app!`/`ios_app!`) never collide with a
/// real generated app's; [`web_app!`]'s single `#[wasm_bindgen(start)]`
/// function is the same kind of fixed, per-crate-unique export name, for the
/// same reason.
///
/// The fourth platform's own compile-check, `cargo check --target
/// wasm32-unknown-unknown -p frust-ui --tests`, type-checks `web_app!`'s
/// generated shim (which calls `frust_shell_web::run_app` — see
/// [`__frust_shell_web`]'s doc comment) and is part of the documented wasm
/// gate in `docs/DEVELOPMENT.md`.
///
/// **Exactly one `app!` invocation may be compiled per target** — a second one
/// would stamp the same fixed JNI/C-ABI export names — so this fixture takes
/// the *setup* form (the newer, ordering-critical arm, whose per-platform
/// expansions differ) and [`macro_expansion_no_setup`]/
/// [`macro_expansion_desktop_config`] below carry the remaining forms on host
/// only. The one-argument form's mobile expansion is
/// unchanged from the pre-setup macro (`@emit $root,` emits no setup tokens)
/// and every generated project plus `examples/huddle`/`examples/glyph-catalog`
/// exercises it.
///
/// The rule binds *targets*, not modules: the host-only fixtures below invoke
/// `app!` again, which is fine because on host the macro emits a plain
/// `pub fn __frust_main` (one per module, no fixed export name) and the
/// `android_app!`/`ios_app!` halves expand to nothing.
///
/// The setup payload here names no design system
/// ([`set_default_theme`] with [`Theme::neutral`] rather than a plugin's
/// `install()`), so this fixture depends on the facade alone.
#[cfg(test)]
mod macro_expansion {
    // `#[allow(dead_code)]`: a zero-field unit struct's derived `Default::default()`
    // call (inside `app!`'s generated `__frust_main`/state-factory closures) is
    // not recognized as a "construction site" by the dead-code lint the way a
    // struct-literal expression is, even though it is genuinely used.
    #[derive(Default)]
    #[allow(dead_code)]
    struct TestApp;

    impl crate::Component for TestApp {
        type State = u32;

        fn init(&self) -> u32 {
            0
        }

        fn build(&self, state: &mut u32) -> crate::AnyView<u32> {
            *state += 1;
            crate::any(crate::text(format!("{state}")))
        }
    }

    crate::app!(
        TestApp,
        setup = {
            crate::set_default_theme(crate::Theme::neutral());
        }
    );
}

/// Compile-only smoke of [`app!`]'s **one-argument** form, host-only for the
/// one-invocation-per-target reason spelled out on [`macro_expansion`] above:
/// on a mobile target this module is cfg'd out so the fixture there stays the
/// single setup-form invocation.
#[cfg(all(
    test,
    not(any(target_os = "android", target_os = "ios", target_arch = "wasm32"))
))]
mod macro_expansion_no_setup {
    #[derive(Default)]
    #[allow(dead_code)]
    struct TestAppNoSetup;

    impl crate::Component for TestAppNoSetup {
        type State = u32;

        fn init(&self) -> u32 {
            0
        }

        fn build(&self, state: &mut u32) -> crate::AnyView<u32> {
            *state += 1;
            crate::any(crate::text(format!("{state}")))
        }
    }

    crate::app!(TestAppNoSetup);
}

/// Compile-only smoke of [`app!`]'s **`desktop = { .. }`** forms — all three of
/// them (config alone, and combined with `setup` in either order), one per
/// submodule because each expansion defines its own `__frust_main`.
///
/// Host-only, for two reasons: the one-invocation-per-target rule spelled out
/// on [`macro_expansion`] above, and [`DesktopConfig`] itself, which is not in
/// a mobile build's dependency graph at all — which is precisely the property
/// these fixtures pin, since a `desktop = { .. }` expression must reach only
/// the desktop `__frust_main` and never the mobile state factories.
///
/// The config payload exercises the full re-exported vocabulary
/// ([`DesktopConfig`] + [`MenuSpec`]/[`MenuItemSpec`]/[`MenuRole`]) through the
/// facade only, so a re-export dropped from `lib.rs` fails to compile here
/// rather than in an app.
#[cfg(all(
    test,
    not(any(target_os = "android", target_os = "ios", target_arch = "wasm32"))
))]
mod macro_expansion_desktop_config {
    #[derive(Default)]
    #[allow(dead_code)]
    struct TestAppDesktop;

    impl crate::Component for TestAppDesktop {
        type State = u32;

        fn init(&self) -> u32 {
            0
        }

        fn build(&self, state: &mut u32) -> crate::AnyView<u32> {
            *state += 1;
            crate::any(crate::text(format!("{state}")))
        }
    }

    /// The identity an app would write inline in its own `desktop = { .. }`
    /// block, factored out so the three fixtures below differ only in the macro
    /// arm they take.
    // `#[allow(dead_code)]`: its only call sites are inside the `__frust_main`
    // bodies `app!` generates, which nothing in a test binary ever calls — the
    // point of these fixtures is that they *compile*.
    #[allow(dead_code)]
    fn fixture_config() -> crate::DesktopConfig {
        crate::DesktopConfig::new()
            .with_app_name("Fixture")
            .with_app_id("dev.frust.fixture")
            .with_menu_spec(
                crate::MenuSpec::new().with_item(crate::MenuItemSpec::submenu(
                    "File",
                    crate::MenuSpec::new()
                        .with_item(
                            crate::MenuItemSpec::item("file.open", "Open…")
                                .with_accelerator("CmdOrCtrl+O"),
                        )
                        .with_item(crate::MenuItemSpec::separator())
                        .with_item(crate::MenuItemSpec::role(crate::MenuRole::Quit)),
                )),
            )
    }

    mod config_only {
        crate::app!(super::TestAppDesktop, desktop = { super::fixture_config() });
    }

    mod setup_then_config {
        crate::app!(
            super::TestAppDesktop,
            setup = {
                crate::set_default_theme(crate::Theme::neutral());
            },
            desktop = { super::fixture_config() }
        );
    }

    mod config_then_setup {
        crate::app!(
            super::TestAppDesktop,
            desktop = { super::fixture_config() },
            setup = {
                crate::set_default_theme(crate::Theme::neutral());
            }
        );
    }
}

/// The config-threading half of the desktop run path: [`App::desktop`] is what
/// carries an app's [`DesktopConfig`] to the shell, and a zero-config
/// [`App::new`] still carries [`DesktopConfig::default()`] — the window the
/// preview has always opened.
///
/// Opening a window isn't testable headless, so this asserts against the value
/// `run` would hand `run_desktop_with` rather than the window itself.
#[cfg(all(
    test,
    not(any(target_os = "android", target_os = "ios", target_arch = "wasm32"))
))]
mod desktop_config_threading {
    use crate::{App, DesktopConfig, View, text};

    fn logic(_state: &mut ()) -> impl View<()> + use<> {
        text("fixture")
    }

    #[test]
    fn a_fresh_app_carries_the_zero_config_desktop_identity() {
        let app = App::new((), logic);
        assert_eq!(app.config, DesktopConfig::default());
        assert_eq!(app.config.window_title(), crate::DEFAULT_APP_NAME);
    }

    #[test]
    fn desktop_threads_the_app_name_into_the_config_the_run_path_receives() {
        let app = App::new((), logic).desktop(DesktopConfig::new().with_app_name("Huddle"));
        assert_eq!(app.config.app_name.as_deref(), Some("Huddle"));
        // The title the shared core titles its window with (and the name the
        // macOS shell builds its application menu around).
        assert_eq!(app.config.window_title(), "Huddle");
    }

    #[test]
    fn desktop_replaces_the_whole_config_rather_than_merging() {
        let app = App::new((), logic)
            .desktop(DesktopConfig::new().with_app_name("First"))
            .desktop(DesktopConfig::new().with_app_id("dev.frust.second"));
        assert_eq!(app.config.app_name, None);
        assert_eq!(app.config.app_id.as_deref(), Some("dev.frust.second"));
    }
}

/// Facade-level regression test: [`run`] runs a root
/// [`Component`]'s `init` under the process-wide root [`Owner`], so a
/// `provide_context` there actually registers (rather than silently no-opping
/// with no ambient owner). Opening a preview window isn't testable headless, so
/// this exercises `run`'s init-wrapping *equivalent* directly — the same
/// `ReactiveRuntime::init` + `with_owner(|| root.init())` shape — and asserts
/// the context resolves for code running under that same owner (the extent the
/// desktop loop's per-frame rebuild also runs in).
#[cfg(test)]
mod root_owner_wrap {
    use frust_reactive::{ReactiveRuntime, provide_context, use_context};

    use crate::Component;

    // A context type unique to this test so it can't collide with any other
    // test providing context on the shared process-wide root owner.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    struct RunCtxMarker(u32);

    struct RootWithContext;
    impl Component for RootWithContext {
        type State = u32;
        fn init(&self) -> u32 {
            provide_context(RunCtxMarker(7));
            0
        }
        fn build(&self, _state: &mut u32) -> crate::AnyView<u32> {
            crate::any(crate::text(String::new()))
        }
    }

    #[test]
    fn run_init_wrapping_makes_root_context_resolvable() {
        // Mirror `run()`'s first two lines: init the runtime with a no-op waker,
        // then run the root's `init` under the root owner.
        let rt = ReactiveRuntime::init(std::sync::Arc::new(|| {}));
        let root = RootWithContext;

        let resolved = rt.with_owner(|| {
            // `init` provides the context...
            let _state = root.init();
            // ...and (still under the same root owner, as a per-frame rebuild
            // would be) it resolves.
            use_context::<RunCtxMarker>()
        });

        assert_eq!(
            resolved,
            Some(RunCtxMarker(7)),
            "run() must wrap root.init() in the root Owner so provide_context sticks"
        );
    }
}

/// Facade-level check that the glass material tokens
/// ([`GlassScale`]/[`GlassMaterial`]/[`GlassFill`]) re-export through the
/// `frust` facade — including enough of the type to let a design-system
/// plugin author its own translucent recipe against `frust::*` alone.
#[cfg(test)]
mod glass_reexport {
    use crate::{GlassFill, GlassMaterial, GlassScale, ShadowSpec, Theme};

    // A build-time proof the types name-resolve through the facade.
    #[allow(dead_code)]
    fn _uses_all(_f: GlassFill, _m: GlassMaterial, _s: GlassScale) {}

    #[test]
    fn the_baseline_exposes_its_opaque_glass_through_the_facade() {
        assert!(Theme::neutral().glass.chrome.is_opaque());
        assert_eq!(Theme::neutral().glass, GlassScale::opaque_material());
    }

    #[test]
    fn a_design_system_can_author_a_translucent_scale_through_the_facade() {
        // The out-of-tree half of the same seam: every type a translucent
        // recipe needs (`GlassScale`/`GlassMaterial`/`GlassFill`/`ShadowSpec`)
        // must be constructible from `frust::*` with no `frust-theme`
        // dependency — this is how a Cupertino-style plugin ships its own
        // glass without the framework carrying the recipe.
        let lens = GlassMaterial {
            blur_radius_intent: 75.0,
            fills_light: vec![GlassFill::new(1.0, 1.0, 1.0, 0.34)],
            fills_dark: vec![GlassFill::new(0.0, 0.0, 0.0, 0.41)],
            hairline_alpha: 0.5,
            shadow: ShadowSpec {
                y_offset: 18.0,
                blur_std_dev: 24.0,
                color_alpha: 0.30,
            },
        };
        assert!(!lens.is_opaque());
        let theme = Theme::builder(Theme::neutral())
            .glass(GlassScale {
                chrome: lens.clone(),
                bar: lens.clone(),
                control: lens,
            })
            .build();
        assert!(!theme.glass.control.is_opaque());
        assert_ne!(theme.glass, GlassScale::opaque_material());
    }
}

/// Facade-level check that the pointer-cursor vocabulary ([`CursorIcon`])
/// re-exports through the `frust` facade — flat *and* through
/// [`authoring`](crate::authoring), since a design-system plugin reaches for it
/// from both sides (its public builder API names a shape; its widget internals
/// request one).
#[cfg(test)]
mod cursor_icon_reexport {
    use crate::CursorIcon;

    // A build-time proof the type name-resolves through the facade, flat and
    // through the authoring seam, and that the two are the same type.
    #[allow(dead_code)]
    fn _uses_both(flat: CursorIcon, nested: crate::authoring::CursorIcon) {
        let _same: CursorIcon = nested;
        let _also: crate::authoring::CursorIcon = flat;
    }

    #[test]
    fn the_cursor_vocabulary_resolves_through_the_facade() {
        // Every shape a desktop-class design system needs is nameable from
        // `frust::` alone — no `frust-core` dependency — and `Default` is what an
        // unrequested pass resolves to, so a plugin can compare against it.
        assert_eq!(CursorIcon::default(), CursorIcon::Default);
        let shapes = [
            CursorIcon::Default,
            CursorIcon::Pointer,
            CursorIcon::Text,
            CursorIcon::Grab,
            CursorIcon::Grabbing,
            CursorIcon::ColResize,
            CursorIcon::RowResize,
            CursorIcon::NotAllowed,
        ];
        assert_ne!(shapes[1], CursorIcon::Default, "the shapes are distinct");
        // `#[non_exhaustive]`: an out-of-tree match must carry a wildcard arm, so
        // this is the shape a plugin's own mapping has to take.
        for shape in shapes {
            let described = match shape {
                CursorIcon::Pointer => "clickable",
                CursorIcon::Text => "editable",
                _ => "other",
            };
            assert!(!described.is_empty());
        }
    }
}

/// Facade-level check that the native-typeface vocabulary
/// ([`FontFace`]/[`NativeTypefaces`]) re-export through the
/// `frust` facade — the seam a design system attaches via [`ThemeExtensions`]
/// for a native-widgets plugin to consume.
#[cfg(test)]
mod native_typefaces_reexport {
    use crate::{FontFace, NativeTypefaces};

    // A build-time proof the types name-resolve through the facade.
    #[allow(dead_code)]
    fn _uses_all(_f: FontFace, _n: NativeTypefaces) {}

    #[test]
    fn native_typefaces_resolve_through_facade() {
        // FontFace constructor is const and can be used in a static context.
        let face = FontFace::new("test-family", b"test bytes");
        assert_eq!(face.family, "test-family");
        assert_eq!(face.bytes, b"test bytes");

        // NativeTypefaces can be constructed and used as a ThemeExtensions payload.
        let typefaces = NativeTypefaces {
            button: Some(face),
            body: Some(face),
        };
        assert!(typefaces.button.is_some());
        assert!(typefaces.body.is_some());
    }
}

/// Facade-level check that the pluggable scroll-physics vocabulary
/// ([`ScrollPhysics`]/[`Bouncing`]/[`OverscrollEffect`]/[`RubberBand`],
/// alongside the rest of `frust_widgets::physics`'s flat re-export) resolves
/// through the `frust` facade — the seam [`ScrollView::physics`]/
/// [`ScrollView::overscroll_effect`] need a consumer to actually construct an
/// argument for without reaching into `frust_widgets` directly.
#[cfg(test)]
mod scroll_physics_reexport {
    use crate::{
        Bouncing, OverscrollEffect, RubberBand, ScrollPhysics, ScrollView, scroll_view, text,
    };

    // A build-time proof the types name-resolve through the facade.
    #[allow(dead_code)]
    fn _uses_all(_p: &dyn ScrollPhysics, _e: OverscrollEffect) {}

    #[test]
    fn physics_and_effect_types_resolve_through_facade() {
        let view: ScrollView<()> = scroll_view(text("hi"))
            .physics(Bouncing::new())
            .overscroll_effect(OverscrollEffect::Stretch);
        let _ = view;
        // RubberBand — the pre-seam feel, now an opt-in rather than any
        // platform's default — is directly constructible through the facade
        // too, which is the whole point of it staying reachable.
        let _: RubberBand = RubberBand::new();
    }
}

/// Acceptance test: an app-authored animation composed entirely
/// from facade names ([`resolve_spec`]/[`make_driver`]/[`TransitionDriver`]),
/// with **no** hand-rolled reduce-motion branch anywhere in this module — the
/// bar this test sets is the *absence* of an app-side `if reduce_motion { .. }`
/// check, not merely that these names resolve.
#[cfg(test)]
mod motion_resolve_reuse {
    use std::time::Duration;

    use crate::{
        Curve, PageTransition, Theme, Timing, TransitionDriver, make_driver, resolve_spec,
    };

    /// Under `Theme.motion.reduce_motion`, the framework's own collapse policy
    /// (`≤120ms` linear cross-fade) applies via a single [`resolve_spec`] call
    /// — the app never re-derives it.
    #[test]
    fn resolve_spec_collapses_under_reduce_motion_without_app_derivation() {
        let mut theme = Theme::neutral();
        theme.motion.reduce_motion = true;

        let spec = crate::TransitionSpec::duration(PageTransition::SlideUp);
        let resolved = resolve_spec(spec, Some(&theme.motion));

        assert_eq!(
            resolved.preset,
            PageTransition::ReducedCrossfade,
            "reduce_motion collapses any animated preset to ReducedCrossfade"
        );
        assert_eq!(
            resolved.timing,
            Timing::Duration(Duration::from_millis(120), Curve::Linear),
            "reduce_motion collapses to the framework's own <=120ms linear timing"
        );

        // Drive the resolved timing through the same `make_driver` the
        // navigator/PatternSwitcher use — an app widget advances `driver` from
        // its own paint pass exactly like they do.
        let (driver, _settle_spring) = make_driver(resolved.timing);
        assert!(
            matches!(driver, TransitionDriver::Auto(_)),
            "a Duration timing always drives TransitionDriver::Auto"
        );
    }

    /// With reduce_motion off, a bare [`Timing::ThemeDefault`] still resolves
    /// to a concrete timing through the same call — the non-collapsed half of
    /// the same resolve path, still with no app-side branching.
    #[test]
    fn resolve_spec_resolves_theme_default_when_reduce_motion_is_off() {
        let theme = Theme::neutral();
        assert!(!theme.motion.reduce_motion);

        let spec = crate::TransitionSpec::new(PageTransition::SlideUp, Timing::ThemeDefault);
        let resolved = resolve_spec(spec, Some(&theme.motion));

        assert_eq!(
            resolved.preset,
            PageTransition::SlideUp,
            "preset is unchanged"
        );
        assert_ne!(
            resolved.timing,
            Timing::ThemeDefault,
            "ThemeDefault is resolved to a concrete timing, not passed through raw"
        );

        let (_driver, _settle_spring) = make_driver(resolved.timing);
    }
}

/// A second acceptance case, the sharper half: an app-authored modal using
/// [`PushOptions`]/[`BackPolicy`]/[`NavigatorController::push_with_options`] —
/// now facade-reachable — with [`BackPolicy::DismissAnimated`]
/// and a dismiss-signal cell, the same mechanism `frust-widgets`' own
/// `show_glyph_dialog` depends on. Previously `PushOptions`/`BackPolicy`
/// were unnameable through `frust::`, so this call could not be constructed at
/// all: Android back on an app-authored sheet had no way to stage the exit and
/// the sheet would vanish instead of sliding down (the `pop`-not-`request_back`
/// symptom this policy prevents).
///
/// Uses `NavigatorController::request_back` directly rather than the full
/// `frust::navigator`-auto-wired + `push_back_press()` path deliberately: the
/// auto-wiring reads `frust-reactive`'s process-wide back-press signal, which
/// needs an active `ReactiveRuntime` and races `back_glue`'s own
/// `TEST_LOCK`-guarded suite if run concurrently in the same test binary.
/// `request_back` is the exact call `back_glue::route_back` makes on a real
/// consumed press (see `back_glue`'s module docs), so this covers the same
/// mechanism without that shared global.
#[cfg(test)]
mod push_with_options_dismiss_animated {
    use std::cell::Cell;
    use std::rc::Rc;

    use frust_core::RenderRoot;
    use frust_widgets::navigator as raw_navigator;

    use crate::{AnyView, BackPolicy, NavigatorController, PushOptions, any, text};

    fn page() -> AnyView<()> {
        any(text("modal"))
    }

    #[test]
    fn dismiss_animated_stages_the_exit_instead_of_vanishing() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let mut root: RenderRoot<(), _> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| raw_navigator(&ctrl, page)
        };
        let mut state = ();
        root.rebuild(&mut app, &mut state);

        // An app-authored modal: transparent, DismissAnimated, with its own
        // shared dismiss-signal cell.
        let signal = Rc::new(Cell::new(0u64));
        controller.push_with_options(
            page,
            PushOptions::transparent()
                .back(BackPolicy::DismissAnimated)
                .dismiss_signal(signal.clone()),
        );
        root.rebuild(&mut app, &mut state);
        assert_eq!(controller.depth(), 2, "modal pushed");
        assert_eq!(signal.get(), 0, "no back yet");

        // Android back: routed through `request_back`.
        controller.request_back();
        root.rebuild(&mut app, &mut state);

        assert_eq!(
            controller.depth(),
            2,
            "DismissAnimated does not pop on back — the stack stages the exit \
             rather than the page vanishing"
        );
        assert_eq!(
            signal.get(),
            1,
            "back fired the dismiss signal exactly once, for the modal's own \
             widget to stage its exit animation on"
        );
    }
}

/// [`__install_default_selection_toolbar`]'s two contracts: it installs the
/// framework baseline when the slot is empty, and a prior explicit
/// [`set_selection_toolbar_builder`] always survives it.
///
/// `frust-core` keeps its builder slot private and offers no way to reset it
/// (unlike its own in-crate tests, which reach the static directly) — this is
/// the only test in this crate's suite that touches the process-global slot,
/// so its very first touch below really is that slot's virgin state in this
/// process; every assertion after that is made deterministic by an explicit
/// `set_selection_toolbar_builder`/[`Arc::ptr_eq`] check instead of relying on
/// ambient state again.
#[cfg(test)]
mod selection_toolbar_bootstrap {
    use std::any::Any;
    use std::sync::Arc;

    use frust_core::{BuildCtx, SelectionToolbarBuilder};
    use kurbo::{Rect, Size};

    use crate::{
        __install_default_selection_toolbar, SelectionToolbarActions, SelectionToolbarRequest,
        View, any, set_selection_toolbar_builder, text,
    };

    fn sample_request() -> SelectionToolbarRequest {
        SelectionToolbarRequest {
            anchor: Rect::new(0.0, 0.0, 10.0, 10.0),
            actions: SelectionToolbarActions {
                copy: true,
                ..Default::default()
            },
            present_menu: true,
        }
    }

    #[test]
    fn installs_the_baseline_when_unset_and_yields_to_a_prior_explicit_install() {
        // The slot's virgin state (see the module doc comment above): nothing
        // has claimed it yet in this process, so the bootstrap call must take
        // it.
        __install_default_selection_toolbar();
        let installed = frust_core::selection_toolbar_builder()
            .expect("the bootstrap install must take the empty slot");

        // Prove it really is the framework baseline — the widget it builds is
        // `frust_widgets::selection_toolbar`'s own type, not merely
        // "something" — by comparing the boxed elements' concrete `TypeId`s,
        // the same type-swap-detection technique
        // `frust_widgets::authoring::rebuild_child_tracked` uses internally.
        let sample = sample_request();
        let probe_view = installed(&sample, Size::new(400.0, 600.0));
        let mut probe_id = 0u64;
        let probe_widget = View::<()>::build(&probe_view, &mut BuildCtx::new(&mut probe_id));

        let baseline_view = frust_widgets::selection_toolbar(&sample);
        let mut baseline_id = 0u64;
        let baseline_widget =
            View::<()>::build(&baseline_view, &mut BuildCtx::new(&mut baseline_id));

        let probe_any: &dyn Any = &*probe_widget;
        let baseline_any: &dyn Any = &*baseline_widget;
        assert_eq!(
            probe_any.type_id(),
            baseline_any.type_id(),
            "the installed default must build selection_toolbar's own widget"
        );

        // Now the other half: an explicit `set_selection_toolbar_builder`
        // always outranks the bootstrap's own set-if-unset call, whatever the
        // slot already held going in (the framework's own baseline, just
        // installed above, included).
        let marker: SelectionToolbarBuilder = Arc::new(|_req, _win| any(text("prior-marker")));
        set_selection_toolbar_builder(Arc::clone(&marker));
        __install_default_selection_toolbar();
        let after = frust_core::selection_toolbar_builder().expect("a builder is installed");
        assert!(
            Arc::ptr_eq(&after, &marker),
            "a prior explicit set_selection_toolbar_builder must survive the bootstrap install"
        );
    }
}
