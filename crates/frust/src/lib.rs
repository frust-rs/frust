//! Facade crate: the public `frust` framework API.
//!
//! App authors depend on this single crate. It exposes [`run`], the canonical
//! app entry point, and curates the view/widget/reactive
//! vocabulary from the underlying framework crates so the declarative call
//! shape reads exactly as the spec promises:
//!
//! ```no_run
//! use frust::{Component, View, AnyView, any, text};
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
//!     fn build(&self, state: &mut i32) -> AnyView<i32> {
//!         any(text(format!("count: {state}")).size(32.0))
//!     }
//! }
//!
//! frust::run(Counter).unwrap();
//! ```
//!
//! ## Low-level escape hatch: `App::new`
//!
//! [`App`] is the older, lower-level entry point [`run`] is built on: a
//! single ambient `State` and a plain `app_logic(&mut State) -> impl View<State>`
//! function, with no [`Component`] state-boundary or retained local state.
//! It stays fully supported (backward compat is a feature) for apps that
//! don't need per-subtree state:
//!
//! ```no_run
//! struct AppState {
//!     greeting: String,
//! }
//!
//! // `+ use<>`: opt out of edition-2024's implicit lifetime capture; views
//! // are `'static` and borrow nothing from `state`.
//! fn app_logic(state: &mut AppState) -> impl frust::View<AppState> + use<> {
//!     frust::text(state.greeting.clone()).size(32.0)
//! }
//!
//! frust::App::new(AppState { greeting: "Hello from Frust".into() }, app_logic)
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
//! struct AppState;
//!
//! fn app_logic(_state: &mut AppState) -> impl frust::View<AppState> + use<> {
//!     Column(vec![
//!         any(text("title").size(24.0)),
//!         any(Row(vec![any(text("left")), any(text("right"))])),
//!         any(Padding(EdgeInsets::all(8.0), text("padded"))),
//!         any(Align(Alignment::CENTER, text("centered"))),
//!         any(SizedBox(Some(0.0), Some(12.0))),
//!     ])
//! }
//! # let _ = app_logic;
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
// This is the FINDINGS #41 gap: `frust-widgets`' own `show_glyph_dialog`
// (`push_with_options(.., PushOptions::transparent())` with a custom
// [`BackPolicy`]) already depends on exactly this seam, so an app-authored
// dialog/sheet could reach the *method* through [`NavigatorController`] but
// never construct a call to it. See this file's
// `push_with_options_dismiss_animated` test module below for the worked
// `BackPolicy::DismissAnimated` + dismiss-signal example — an app-authored
// modal staging its own exit on Android back instead of vanishing.
pub use frust_widgets::{
    Align, AlignView, Alignment, Axis, BackPolicy, Button, ButtonStyle, ButtonView, Checkbox,
    CheckboxView, ChildKey, Column, CrossAxisAlignment, EdgeInsets, FlexChild, FlexView,
    GestureDetector, GestureDetectorView, HeroView, Icon, IconData, IconSource, IconView,
    IconWidget, Image, ImageError, ImageFit, ImageSource, ImageView, MainAxisAlignment,
    NavigatorController, NavigatorId, NavigatorView, Padding, PaddingView, PageBuilder,
    PageTransition, PageVisibility, PopResult, PushOptions, Radio, RadioView, RadioWidget,
    ResultCallback, Row, SafeAreaView, ScrollInfo, ScrollView, SizedBox, SizedBoxView, Slider,
    SliderView, Stack, StackView, TextInput, TextInputView, TextView, Timing, TransitionSpec,
    TransitionState, VisibilityCallback, button, checkbox, flexible, hero, icon, inflexible, keyed,
    radio, safe_area, scroll_view, slider, text, text_input,
};

/// Platform-view embedding (platform-views feature, tasks 02/07): reserve
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
pub use frust_widgets::{
    DEFAULT_REDIRECT_LIMIT, ErrorBuilder, Location, NavRequest, NavWaker, PathPattern, Redirect,
    Resolution, ResolvedPage, Route, RouteBuilder, RouteNavigator, RouteParams, Router,
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
/// framework's rule changes (FINDINGS #41).
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
/// let mut theme = Theme::m3_baseline();
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

/// The Material 3 Expressive widget catalog: AppBar,
/// Card, Chips, Dialog, FAB, ListView/ListItem, NavigationBar, BottomSheet,
/// Switch, and progress indicators — flat-re-exported from
/// `frust-widgets` so app code (e.g. `examples/catalog`) never names that
/// crate directly, mirroring the baseline-widget re-export block above.
///
/// Gated by this crate's default-on `material` feature, which forwards to
/// `frust-widgets/material` — with it off the catalog is not compiled at all,
/// so this block must be gated in lockstep or it would name absent items.
#[cfg(feature = "material")]
pub use frust_widgets::{
    AppBar, AppBarView, AppBarWidget, AssistChip, AssistChipView, AssistChipWidget,
    BottomSheetView, BottomSheetWidget, ButtonGroup, ButtonGroupView, ButtonGroupWidget,
    CardVariant, CardView, CardWidget, CircularProgress, CircularProgressView,
    CircularProgressWidget, DialogView, DialogWidget, DockedToolbar, FabMenu, FabMenuItem,
    FabMenuView, FabMenuWidget, FabSize, FabView, FabWidget, FilterChip, FilterChipView,
    FilterChipWidget, FloatingToolbar, LinearProgress, LinearProgressView, LinearProgressWidget,
    ListItem, ListItemLines, ListItemWidget, ListView, ListViewWidget, LoadingIndicator,
    LoadingIndicatorView, LoadingIndicatorWidget, NavItem, NavigationBar, NavigationBarView,
    NavigationBarWidget, ONE_LINE_HEIGHT, ProgressValue, RoundedPolygon, SplitButton,
    SplitButtonView, SplitButtonWidget, Switch, SwitchView, SwitchWidget, THREE_LINE_HEIGHT,
    TWO_LINE_HEIGHT, ToolbarVariant, ToolbarView, ToolbarWidget, app_bar, assist_chip,
    bottom_sheet, button_group, card, circular_progress, dialog, docked_toolbar, elevated_card,
    extended_fab, fab, fab_menu, fab_menu_item, filled_card, filter_chip, floating_toolbar,
    linear_progress, list_item, list_view, loading_indicator, morph_path, nav_item, navigation_bar,
    outlined_card, show_bottom_sheet, show_dialog, split_button, switch,
};

/// The Cupertino (iOS) widget catalog: the
/// Flutter-parity counterparts to a subset of the Material catalog above —
/// flat-re-exported from `frust-widgets` for the same reason, and gated by the
/// default-on `cupertino` feature for the same reason as the Material block.
#[cfg(feature = "cupertino")]
pub use frust_widgets::{
    CupertinoActionSheetView, CupertinoActionSheetWidget, CupertinoActionStyle,
    CupertinoActivityIndicator, CupertinoActivityIndicatorView, CupertinoActivityIndicatorWidget,
    CupertinoAlertDialogView, CupertinoAlertDialogWidget, CupertinoButton, CupertinoButtonSize,
    CupertinoButtonStyle, CupertinoButtonView, CupertinoButtonWidget, CupertinoDialogAction,
    CupertinoNavBar, CupertinoNavBarView, CupertinoNavBarWidget, CupertinoSwitch,
    CupertinoSwitchView, CupertinoSwitchWidget, CupertinoTabBar, CupertinoTabBarView,
    CupertinoTabBarWidget, TabItem, action, cupertino_activity_indicator, cupertino_button,
    cupertino_nav_bar, cupertino_switch, cupertino_tab_bar, show_action_sheet,
    show_cupertino_alert, tab_item,
};

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

/// The Glyph widget catalog: the
/// terminal-native components with no Material/Cupertino equivalent
/// (badges/tags/alerts, loaders + toast, nav chrome, content cards, the
/// terminal block + tooltip, and the command-palette overlay). Re-exported
/// **wholesale** (`pub use frust_widgets::glyph;`), following
/// `frust_widgets::icons`/`frust_widgets::motion`'s wholesale-module
/// precedent, so every widget a later fill task adds under
/// `frust_widgets::glyph` rides along under `frust::glyph::*` with no further
/// facade edit.
///
/// Gated by this crate's default-on `glyph` feature (which turns on
/// `frust-widgets/glyph` **and** `frust-theme/glyph` — widgets and tokens
/// together); the design-token half re-exports as [`GlyphInk`] below, and
/// [`glyph_theme::install`] is the seam that makes a Glyph theme the app's
/// starting point.
#[cfg(feature = "glyph")]
pub use frust_widgets::glyph;

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
    pub use frust_core::{
        AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventOutcome,
        EventResult, HeroDirective, HeroFrames, InputEvent, Key, KeyEvent, LayoutCtx, Modifiers,
        NamedKey, PaintCtx, PaintOutcome, PaintScene, PointerButton, PointerEvent, PointerPhase,
        ScrollDelta, SemanticsCtx, SemanticsUpdate, TickClass, View, Widget, WidgetId, any,
    };
    /// The event-pass/IME-surface `EditingState` — see this module's own docs
    /// for the split against [`text::EditingState`](self::text::EditingState),
    /// `frust_text::editor`'s distinct, byte-indexed type. `ImeContentType` is
    /// the input-purpose hint an editable widget publishes on its `ImeState` so
    /// a shell can lock a secret field's keyboard down.
    pub use frust_core::{EditingState, ImeContentType, ImeEvent, ImeState};

    /// The inset vocabulary [`LayoutCtx::window_insets`]/[`PaintCtx::window_insets`]
    /// return — lifted because a widget that lays itself out around the status
    /// bar, notch, or on-screen keyboard cannot otherwise name the value those
    /// accessors hand it.
    pub use frust_core::{WindowEdgeInsets, WindowInsets};

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
    // (see `crates/frust-render/src/convert.rs`'s own import lines).
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
            FamilyName, FontFamily, FontStyle, FontWeight, GenericSlot, LineHeight, TextContext,
            TextLayout, TextStyle,
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
    pub mod scene {
        pub use frust_scene::{
            Command, FontHandle, Glyph, GlyphRun, PathStyle, Scene, SceneBuilder, ShaderProgram,
            arc_path,
        };
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
pub fn navigator<State: 'static>(
    controller: &NavigatorController<State>,
    initial: impl Fn() -> AnyView<State> + 'static,
) -> NavigatorView<State> {
    back_glue::auto_wire(controller);
    frust_widgets::navigator(controller, initial)
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
/// `controller`. With a host *and* an inner navigator there are two registrants,
/// and a press goes to the first one claiming it in build order — outermost
/// first (**R44-back**). A host with no overlays open claims nothing, so back
/// falls through to the inner navigator exactly as before the host existed. See
/// [`back_glue`]'s module docs for the arbitration contract.
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
pub fn overlay_host<State: 'static>(
    controller: &NavigatorController<State>,
    app: impl Fn() -> AnyView<State> + 'static,
) -> NavigatorView<State> {
    back_glue::auto_wire(controller);
    frust_widgets::overlay_host(controller, app)
}

/// Router ⇄ deep-link auto-wiring: [`router_with_deep_links`]/
/// [`RouterDeepLinks`] resolve a [`Router`]'s start location from the process's
/// cold-start deep link (falling back to an app-supplied default) and keep
/// navigating it on every subsequent warm link — see [`RouterDeepLinks`]'s doc
/// for the precedence and dedupe contracts. This is the ONLY place in the
/// facade that sees both `frust-widgets`' `Router` and `frust-reactive`'s
/// deep-link source together; neither underlying crate depends on the other.
pub use router_glue::{RouterDeepLinks, router_with_deep_links};

/// The design-token vocabulary: the [`Theme`] bundle plus its
/// component token tables, flat-re-exported from `frust-theme` so app code
/// never names that crate directly. A root component reads the active theme via
/// [`use_context`]`::<`[`Theme`]`>()`; a widget reads it during paint/layout via
/// `PaintCtx::theme_as`/`LayoutCtx::theme_as` (or `Theme::from_paint_ctx`).
///
/// Includes the glass material tokens:
///
/// ```
/// use frust::GlassScale;
///
/// let glass = GlassScale::ios27();
/// assert_eq!(glass.chrome.blur_radius_intent, 75.0);
/// assert!(!glass.control.is_opaque());
/// ```
///
/// Also the composable-theming surface:
/// [`ThemeBuilder`] (`defineTheme`/`copyWith` analog), the no-lock-in typed
/// extension slot ([`ThemeExtensions`]) plus its first consumer
/// [`StatusPalette`]/[`StatusColors`] (success/warning/info), and the Glyph
/// motion vocabulary ([`MotionDurations`]/[`EasingSet`]) — all
/// flat-re-exported so an app authors a custom theme against `frust::*` alone.
pub use frust_theme::{
    Brightness, ColorScheme, DesignLanguage, EasingSet, Elevation, ElevationLevel, GlassFill,
    GlassMaterial, GlassScale, MotionDurations, MotionScheme, MotionSpring, ShadowSpec, ShapeScale,
    StatusColors, StatusPalette, SurfaceRole, Theme, ThemeBuilder, ThemeExtensions, TypeScale,
};

/// Glyph's brightness-invariant terminal/tooltip ink — a
/// [`ThemeExtensions`] attachment on the Glyph baseline, never
/// brightness-swapped like the rest of a [`ColorScheme`] (see
/// `docs/CODE_STANDARDS.md`'s Theming conventions).
///
/// Gated by the default-on `glyph` feature alongside the widget catalog
/// ([`glyph`](crate::glyph)): `frust_theme::GlyphInk` lives inside
/// `frust-theme`'s own `glyph`-gated module, so this re-export tracks it.
/// `Theme::glyph_baseline()` — the baseline that carries it — appears and
/// disappears with the same feature; [`glyph_theme::install`] is how an app
/// makes it the starting theme.
#[cfg(feature = "glyph")]
pub use frust_theme::GlyphInk;

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
/// use frust::{Theme, set_app_theme};
///
/// // Force the Cupertino baseline regardless of the platform's own
/// // Material-vs-Cupertino default — e.g. the widget catalog's design-
/// // language toggle.
/// set_app_theme(Theme::cupertino_baseline());
/// ```
pub use frust_shell_common::{clear_app_theme, set_app_theme};

/// Design-system-facing base-theme seed:
/// [`set_default_theme`] supplies the *starting* theme a shell seeds itself
/// with, in place of its own built-in fallback — the seam a design-system
/// plugin's `install()` calls. Unlike [`set_app_theme`], this does NOT pin
/// brightness: the shell keeps re-deriving light/dark from the platform's own
/// appearance against this same base, so a Glyph-themed app installed this
/// way still honours system dark mode. See
/// `frust_shell_common::theme_default`'s module docs for the full precedence
/// order ([`set_app_theme`] override → [`set_default_theme`] base → the
/// shell's built-in fallback) and the brightness-following contrast with
/// [`set_app_theme`] spelled out in full.
///
/// ```no_run
/// use frust::{Theme, set_default_theme};
///
/// // A design-system plugin's install() call, seeding its own base theme as
/// // the app's starting point without pinning brightness. The built-in Glyph
/// // installer (`frust::glyph_theme::install`, `glyph` feature) is this call
/// // with `Theme::glyph_baseline()`, plus its bundled-font registration.
/// set_default_theme(Theme::m3_baseline());
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

/// The Glyph design system's one-line installer.
///
/// Every shell's built-in fallback theme is the design-language-free
/// `Theme::neutral()` and no shell registers a font of its own, so a Glyph app
/// supplies both halves itself. [`install`](glyph_theme::install) is that call:
/// it seeds `Theme::glyph_baseline()` through [`set_default_theme`] and pushes
/// the bundled Space Mono / IBM Plex Mono faces through
/// [`register_app_fonts`].
///
/// Gated by the default-on `glyph` feature (the tokens it names live behind
/// `frust-theme`'s own `glyph` gate). Kept in the facade rather than
/// `frust-shell-common` deliberately: it needs `frust-theme`'s Glyph tokens,
/// and shell-common must stay design-language-free
/// (`docs/ARCHITECTURE.md`'s Theme delivery).
#[cfg(feature = "glyph")]
pub mod glyph_theme {
    /// Make the Glyph design system this app's starting point.
    ///
    /// Two process-global pushes, both already public seams:
    ///
    /// 1. [`set_default_theme`](crate::set_default_theme)`(Theme::glyph_baseline())`
    ///    — the *base* a shell seeds itself with instead of its built-in
    ///    `Theme::neutral()` fallback. Deliberately not
    ///    [`set_app_theme`](crate::set_app_theme): a seeded default does not
    ///    pin brightness, so a Glyph app still follows system dark mode.
    /// 2. [`register_app_fonts`](crate::register_app_fonts) for every bundled
    ///    Glyph face, so the Glyph type scale's families actually resolve. With
    ///    `frust`'s `glyph-fonts` feature off the bundled bytes are not
    ///    compiled in and this half is a no-op (`font_data()` answers an empty
    ///    slice) — the type scale then falls back to whatever monospace family
    ///    the platform has.
    ///
    /// # Timing: must run before the first frame
    ///
    /// A shell reads the default-theme slot and drains the font registry
    /// **once, at construction**, before its first rebuild. A call after that
    /// takes effect only on a later `clear_app_theme`-driven reseed, which may
    /// never happen — so a late call silently does nothing visible.
    ///
    /// The supported way to get the timing right on all three platforms is
    /// [`app!`](crate::app)'s setup block, which runs immediately before the
    /// root component's `Component::init` and therefore before any shell
    /// construction:
    ///
    /// ```no_run
    /// use frust::{AnyView, Component, any, text};
    ///
    /// #[derive(Default)]
    /// struct MyApp;
    ///
    /// impl Component for MyApp {
    ///     type State = ();
    ///     fn init(&self) -> Self::State {}
    ///     fn build(&self, _state: &mut Self::State) -> AnyView<Self::State> {
    ///         any(text("glyph"))
    ///     }
    /// }
    ///
    /// frust::app!(MyApp, setup = { frust::glyph_theme::install(); });
    /// # fn main() {}
    /// ```
    ///
    /// Calling it from `Component::init` itself also happens to be early
    /// enough today, but that is not a contract this crate keeps; the setup
    /// block is.
    ///
    /// # Thread contract and repeat calls
    ///
    /// Both underlying seams are plain `Mutex`-guarded process-globals
    /// callable from any thread. Calling `install` twice is harmless but
    /// wasteful: the second `set_default_theme` replaces an identical value,
    /// and the font bytes are pushed (and later re-registered, shadowing the
    /// same family names) a second time. Call it once.
    pub fn install() {
        crate::set_default_theme(crate::Theme::glyph_baseline());
        for bytes in frust_theme::glyph::font_data() {
            crate::register_app_fonts(bytes.to_vec());
        }
    }
}

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
/// (no matching `CompositeAlphaMode`, or a GPU-tier blit-fallback surface —
/// `docs/LIMITATIONS.md`'s `cam-blit-opaque`). frust's paint side degrades to
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
/// fn build_router() -> Router<AppState> {
///     Router::new(vec![Route::new("/", |_params| -> AnyView<AppState> {
///         any(text("home"))
///     })])
/// }
///
/// fn app_logic(_state: &mut AppState) -> impl frust::View<AppState> + use<> {
///     let _router = build_router();
///     // A late-subscribed read: `initial` sees a cold-start link (if any);
///     // `latest` is the live signal a rebuild tracks for warm links.
///     let links = deep_links();
///     let _ = links.initial;
///     let _ = links.latest;
///     text("nav demo")
/// }
/// # let _ = app_logic;
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
// `frust::android_app!(AppState, app_logic)`. `pub use` of a
// `#[macro_export]` macro re-exports it on edition 2021+; the macro only expands
// to real code where its call site is `#[cfg(target_os = "android")]`, so this is
// inert on desktop.
pub use frust_shell_android::android_app;

// Re-export the iOS C-ABI-bridge macro so generated apps write
// `frust::ios_app!(AppState, app_logic)`. Unlike `android_app!`,
// the invocation is unconditional — the macro's generated `frust_*` exports
// are each `#[cfg(target_os = "ios")]`, so it is inert off-iOS.
pub use frust_shell_ios::ios_app;

/// A Frust application: the app state plus the `app_logic` function that maps
/// it to a view tree.
///
/// Construct with [`App::new`] and start the event loop with [`App::run`].
///
/// The view type is intentionally *not* a parameter of this struct: capturing a
/// free `fn app_logic(&mut State) -> impl View<State>`'s opaque return type into
/// a stored type parameter defeats method resolution (the opaque type's trait
/// bounds can't be re-proven on the already-typed value). Instead [`App::run`]
/// infers the view type freshly at the call site, so
/// `App::new(state, app_logic).run()` compiles for both `impl View` and
/// concrete-typed `app_logic`.
// On Android the fields are consumed only by the desktop-gated `run`, so they
// read as dead there; the app is driven through `android_app!`/JNI instead.
#[cfg_attr(target_os = "android", allow(dead_code))]
pub struct App<State, Logic> {
    state: State,
    logic: Logic,
}

impl<State, Logic> App<State, Logic> {
    /// Create an app from an initial `state` and its `app_logic`.
    ///
    /// `logic` is a `FnMut(&mut State) -> impl View<State>` re-run each frame to
    /// produce the current view tree.
    pub fn new(state: State, logic: Logic) -> Self {
        Self { state, logic }
    }
}

#[cfg(not(target_os = "android"))]
impl<State: 'static, Logic> App<State, Logic> {
    /// Run the app in the desktop preview window until it is closed.
    ///
    /// Blocks the calling thread on the platform event loop. Returns once the
    /// window closes, or an error if the window/GPU surface could not be
    /// created. The concrete view type `V` is inferred from `logic`.
    ///
    /// Desktop-only: on Android the app is driven by the JNI bridge that
    /// [`android_app!`] generates, not by this preview loop.
    pub fn run<V>(self) -> anyhow::Result<()>
    where
        V: View<State>,
        Logic: FnMut(&mut State) -> V + 'static,
    {
        frust_shell_desktop::run_desktop(self.state, self.logic)
    }
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
/// [`android_app!`]/JNI instead, not by this preview loop.
#[cfg(not(target_os = "android"))]
pub fn run<C: Component>(root: C) -> anyhow::Result<()> {
    run_with_setup(root, || {})
}

/// [`run`] with a **setup** closure run once, immediately before
/// `root.init()` — the desktop half of [`app!`]'s `setup = { .. }` block.
///
/// `setup` runs on this (the UI) thread, after
/// [`frust_reactive::ReactiveRuntime::init`] and under the runtime's root
/// [`Owner`], and therefore *before* `frust_shell_desktop::run_desktop`
/// constructs the shell — which is the one moment the shell reads
/// [`set_default_theme`]'s slot and drains [`register_app_fonts`]' registry.
/// That is exactly the placement `app!`'s Android/iOS arms get for free (both
/// run the block inside the state factory `create_handle`/`ffi_glue::init`
/// calls before building their `AppHandle`), so the ordering contract is
/// identical on all three platforms.
///
/// A design-system plugin's installer (`frust::glyph_theme::install`, or a
/// third-party equivalent) is the intended payload; app code normally reaches
/// this through [`app!`] rather than calling it directly.
///
/// Desktop-only, matching [`run`]/[`App::run`].
#[cfg(not(target_os = "android"))]
pub fn run_with_setup<C: Component>(root: C, setup: impl FnOnce()) -> anyhow::Result<()> {
    let rt = frust_reactive::ReactiveRuntime::init(std::sync::Arc::new(|| {}));
    let state = rt.with_owner(|| {
        setup();
        root.init()
    });
    App::new(state, move |state: &mut C::State| root.build(state)).run()
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
///   off-iOS.
/// - **Every other target** (the desktop preview): a hidden
///   `#[doc(hidden)] pub fn __frust_main()` that runs `Root` through
///   [`run`], printing the error and exiting non-zero on failure. `app!`
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
/// #     pub fn install() { frust::set_default_theme(frust::Theme::m3_baseline()); }
/// # }
/// # fn main() {}
/// ```
///
/// This exists because a design system's installer must reach
/// [`set_default_theme`]/[`register_app_fonts`] *before* a shell reads them,
/// and Android has no `main` at all (its entry is the JNI `nativeInit`
/// `android_app!` generates). Any plugin can use it; it is not
/// Glyph-specific — the built-in Glyph installer
/// (`frust::glyph_theme::install`, `glyph` feature) is just the first caller.
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
#[macro_export]
macro_rules! app {
    // Internal single implementation, shared by both public forms below.
    // `$($setup:block)?` is empty for the one-argument form, so the two mobile
    // state factories expand byte-identically to the pre-setup macro and the
    // desktop arm's `run_with_setup(root, || {})` is `run`'s own body. Listed
    // FIRST because macro_rules cannot recover from a `$root:ty` fragment that
    // fails to parse: were the public arms first, `@emit` would be fed to
    // `:ty` and error out instead of falling through to this arm.
    (@emit $root:ty, $($setup:block)?) => {
        #[cfg(target_os = "android")]
        $crate::android_app!(
            <$root as $crate::Component>::State,
            || {
                $($setup)?
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
                $crate::Component::init(&<$root as ::core::default::Default>::default())
            },
            {
                let __frust_root = <$root as ::core::default::Default>::default();
                move |state: &mut <$root as $crate::Component>::State| {
                    $crate::Component::build(&__frust_root, state)
                }
            }
        );

        #[cfg(not(target_os = "android"))]
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
    ($root:ty $(,)?) => {
        $crate::app!(@emit $root,);
    };
    ($root:ty, setup = $setup:block $(,)?) => {
        $crate::app!(@emit $root, $setup);
    };
}

/// Compile-only smoke of [`app!`]'s `setup = { .. }` form: a
/// `Component + Default` fixture bound to all three platforms in one call —
/// `cargo test --workspace` compiles this on host (criterion 1: `__frust_main`
/// present, no Android JNI symbols), and `cargo check --target
/// aarch64-linux-android -p frust --tests` / `--target
/// aarch64-apple-ios-sim -p frust --tests` compile it for the two mobile
/// targets (criterion 2: the respective platform's exports appear,
/// `__frust_main` absent on Android). Lives behind `cfg(test)` — never
/// linked into a cdylib/staticlib/binary, so the fixed JNI/C-ABI export names
/// `app!` stamps out (via `android_app!`/`ios_app!`) never collide with a
/// real generated app's.
///
/// **Exactly one `app!` invocation may be compiled per target** — a second one
/// would stamp the same fixed JNI/C-ABI export names — so this fixture takes
/// the *setup* form (the newer, ordering-critical arm, whose per-platform
/// expansions differ) and [`macro_expansion_no_setup`] below carries the
/// one-argument form on host only. The one-argument form's mobile expansion is
/// unchanged from the pre-setup macro (`@emit $root,` emits no setup tokens)
/// and every generated project plus `examples/huddle`/`examples/glyph-catalog`
/// exercises it.
///
/// The setup payload here is deliberately feature-independent
/// ([`set_default_theme`] with the always-available M3 baseline rather than
/// `glyph_theme::install`), so this fixture also compiles under
/// `--no-default-features`.
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
            crate::set_default_theme(crate::Theme::m3_baseline());
        }
    );
}

/// Compile-only smoke of [`app!`]'s **one-argument** form, host-only for the
/// one-invocation-per-target reason spelled out on [`macro_expansion`] above:
/// on a mobile target this module is cfg'd out so the fixture there stays the
/// single setup-form invocation.
#[cfg(all(test, not(any(target_os = "android", target_os = "ios"))))]
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
/// `frust` facade, and the two baselines carry the matching scale.
#[cfg(test)]
mod glass_reexport {
    use crate::{GlassFill, GlassMaterial, GlassScale, Theme};

    // A build-time proof the types name-resolve through the facade.
    #[allow(dead_code)]
    fn _uses_all(_f: GlassFill, _m: GlassMaterial, _s: GlassScale) {}

    #[test]
    fn baselines_expose_glass_through_the_facade() {
        assert!(Theme::m3_baseline().glass.chrome.is_opaque());
        assert!(!Theme::cupertino_baseline().glass.control.is_opaque());
        assert_eq!(
            Theme::cupertino_baseline().glass,
            GlassScale::ios27(),
            "Cupertino baseline carries the iOS-27 glass scale"
        );
    }
}

/// FINDINGS #41 acceptance test: an app-authored animation composed entirely
/// from facade names ([`resolve_spec`]/[`make_driver`]/[`TransitionDriver`]),
/// with **no** hand-rolled reduce-motion branch anywhere in this module — the
/// bar the task sets is the *absence* of an app-side `if reduce_motion { .. }`
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
        let mut theme = Theme::m3_baseline();
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
        let theme = Theme::m3_baseline();
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

/// FINDINGS #41's sharper half: an app-authored modal using
/// [`PushOptions`]/[`BackPolicy`]/[`NavigatorController::push_with_options`] —
/// all facade-reachable only as of this task — with [`BackPolicy::DismissAnimated`]
/// and a dismiss-signal cell, the same mechanism `frust-widgets`' own
/// `show_glyph_dialog` depends on. Before this task `PushOptions`/`BackPolicy`
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
