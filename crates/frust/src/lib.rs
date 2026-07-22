//! Facade crate: the public `frust` framework API (spec §5).
//!
//! App authors depend on this single crate. It exposes [`run`], the canonical
//! app entry point (phase 5.5), and curates the view/widget/reactive
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
//! ## Layout containers (spec §6.2)
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
pub use frust_widgets::{
    Align, AlignView, Alignment, Axis, Button, ButtonView, Checkbox, CheckboxView, ChildKey,
    Column, CrossAxisAlignment, EdgeInsets, FlexChild, FlexView, GestureDetector,
    GestureDetectorView, HeroView, Icon, IconData, IconSource, IconView, IconWidget, Image,
    ImageError, ImageFit, ImageSource, ImageView, MainAxisAlignment, NavigatorController,
    NavigatorView, Padding, PaddingView, PageBuilder, PageTransition, PopResult, Radio, RadioView,
    RadioWidget, ResultCallback, Row, SafeAreaView, ScrollInfo, ScrollView, SizedBox, SizedBoxView,
    Slider, SliderView, Stack, StackView, TextInput, TextInputView, TextView, Timing,
    TransitionSpec, button, checkbox, flexible, hero, icon, inflexible, keyed, navigator, radio,
    safe_area, scroll_view, slider, text, text_input,
};

/// The vendored Material Symbols starter icon set (Huddle showcase, Phase A),
/// flat-re-exported so app code names `frust::icons::HOME` rather than the
/// underlying `frust-widgets` crate. Each entry is an
/// [`IconSource`](crate::IconSource) usable directly with [`icon`](crate::icon);
/// an app can also supply its own vector icons via
/// [`IconData::from_path`](crate::IconData::from_path).
pub use frust_widgets::icons;

/// The declarative router vocabulary (spec §19's go_router-subset layer),
/// flat-re-exported from `frust-widgets` so app code never names that
/// crate directly: [`Router`] resolves a location against a [`Route`] table
/// (built via [`RouteBuilder`]) into [`Resolution`]/[`ResolvedPage`]s driving
/// a [`NavigatorController`], with `:param`/query parsing
/// ([`Location`]/[`PathPattern`]/[`RouteParams`]), per-route/top-level
/// [`Redirect`]s (loop-guarded at [`DEFAULT_REDIRECT_LIMIT`]), and an
/// [`ErrorBuilder`] fallback for an unmatched location.
pub use frust_widgets::{
    DEFAULT_REDIRECT_LIMIT, ErrorBuilder, Location, PathPattern, Redirect, Resolution,
    ResolvedPage, Route, RouteBuilder, RouteParams, Router,
};

/// The Material 3 Expressive widget catalog (Phase 6c, PLAN.md D5): AppBar,
/// Card, Chips, Dialog, FAB, ListView/ListItem, NavigationBar, BottomSheet,
/// Switch, and progress indicators — flat-re-exported from
/// `frust-widgets` so app code (e.g. `examples/catalog`) never names that
/// crate directly, mirroring the baseline-widget re-export block above.
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

/// The Cupertino (iOS) widget catalog (Phase 6c, PLAN.md D5): the
/// Flutter-parity counterparts to a subset of the Material catalog above —
/// flat-re-exported from `frust-widgets` for the same reason.
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

/// The `motion` module (glyph-design-system task 12): declarative
/// implicit-animation wrappers (`AnimatedOpacity`/`AnimatedScale` today;
/// `switcher`/`patterns` land in later tasks) over `frust-core`'s `anim`
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

mod back_glue;
mod router_glue;

/// Android back-press ⇄ navigator auto-wiring (device-parity task 05):
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

/// Router ⇄ deep-link auto-wiring (Phase 6b, task 08): [`router_with_deep_links`]/
/// [`RouterDeepLinks`] resolve a [`Router`]'s start location from the process's
/// cold-start deep link (falling back to an app-supplied default) and keep
/// navigating it on every subsequent warm link — see [`RouterDeepLinks`]'s doc
/// for the precedence and dedupe contracts. This is the ONLY place in the
/// facade that sees both `frust-widgets`' `Router` and `frust-reactive`'s
/// deep-link source together; neither underlying crate depends on the other.
pub use router_glue::{RouterDeepLinks, router_with_deep_links};

/// The design-token vocabulary (spec §11): the [`Theme`] bundle plus its
/// component token tables, flat-re-exported from `frust-theme` so app code
/// never names that crate directly. A root component reads the active theme via
/// [`use_context`]`::<`[`Theme`]`>()`; a widget reads it during paint/layout via
/// `PaintCtx::theme_as`/`LayoutCtx::theme_as` (or `Theme::from_paint_ctx`).
///
/// Includes the glass material tokens (task 6f-01):
///
/// ```
/// use frust::GlassScale;
///
/// let glass = GlassScale::ios27();
/// assert_eq!(glass.chrome.blur_radius_intent, 75.0);
/// assert!(!glass.control.is_opaque());
/// ```
///
/// Also the composable-theming surface (glyph-design-system tasks 04/05/09/11/15):
/// [`ThemeBuilder`] (`defineTheme`/`copyWith` analog), the no-lock-in typed
/// extension slot ([`ThemeExtensions`]) plus its first two consumers
/// [`StatusPalette`]/[`StatusColors`] (success/warning/info) and [`GlyphInk`]
/// (Glyph's brightness-invariant terminal/tooltip ink), and the Glyph motion
/// vocabulary ([`MotionDurations`]/[`EasingSet`]) — all flat-re-exported so an
/// app authors a custom theme against `frust::*` alone. The Glyph baseline
/// itself is [`Theme::glyph_baseline`].
pub use frust_theme::{
    Brightness, ColorScheme, DesignLanguage, EasingSet, Elevation, ElevationLevel, GlassFill,
    GlassMaterial, GlassScale, GlyphInk, MotionDurations, MotionScheme, MotionSpring, ShadowSpec,
    ShapeScale, StatusColors, StatusPalette, SurfaceRole, Theme, ThemeBuilder, ThemeExtensions,
    TypeScale,
};

/// The color type every [`ColorScheme`] role is expressed in
/// ([`peniko::Color`]), re-exported so app code can author its own color
/// values (e.g. a custom accent palette that composes onto a baseline
/// [`ColorScheme`]) without naming `peniko` directly — the same
/// facade-only-dependency rule the theme re-exports above follow. Construct
/// one with [`Color::from_rgb8`]/[`Color::new`]; read its channels via
/// `Color::components` (`[f32; 4]`, straight-alpha RGBA).
pub use peniko::Color;

/// App-facing theme override (PLAN.md D2 correction, task 6c-04):
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

/// App-facing pending-font registry (glyph-design-system task 08):
/// [`register_app_fonts`] pushes raw font bytes (TTF/OTF, or a TTC/OTC
/// collection) to be registered into the running shell's `TextContext` the
/// next time it drains this registry (construction time, and once per
/// frame -- task 14's shell wiring). See
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

/// The animation vocabulary (spec §8): the shell-fed frame clock ([`FrameTime`])
/// plus the pure easing/interpolation/spring math a widget or app advances it
/// through, flat-re-exported from `frust-core::anim`. Time enters from the
/// shell during paint (`PaintCtx::frame_time`); nothing here reads a clock.
pub use frust_core::anim::{
    AnimationController, AnimationStatus, Curve, FrameTime, Lerp, Spring, SpringDesc, Tween,
};

/// Pure input/gesture helpers (slop constants, [`input::VelocityTracker`], the
/// fling-decay math) re-exported for app authors and advanced widgets.
pub mod input {
    pub use frust_core::input::{
        FLING_DECAY, FLING_STOP, MOUSE_SLOP, TOUCH_SLOP, VELOCITY_WINDOW_MS, VelocityTracker,
        WHEEL_LINE_PX, fling_decay, fling_displacement,
    };
}

/// The reactive-programming vocabulary (spec §5.5) [`Component`] state is built
/// on: signals, memos, and context, flat-re-exported from `frust-reactive`/
/// `reactive_graph` so app authors never name either crate directly.
pub use frust_reactive::{RwSignal, on_cleanup, provide_context, use_context};

/// The deep-link read surface (spec §19; `frust-reactive`'s `app_links`-
/// style process-wide source — see its module docs for the semantics): a
/// mobile shell delivers a platform link via `frust-reactive`'s
/// [`push_deep_link`], and app code reads it here —
/// [`deep_links()`] returns a [`DeepLinks`] snapshot ([`DeepLinks::initial`])
/// plus the live, trackable [`DeepLinks::latest`] signal a
/// [`Component::build`] reads to react to cold-start and subsequent links
/// uniformly. Router auto-wiring (resolving `deep_links()` against a
/// [`Router`]) is a separate opt-in (task 08), not automatic here.
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
/// handler; it is also re-exported here as the desktop dev seam Phase 6b's
/// task 06 documents (no shell writes on desktop yet) — `examples/navdemo`'s
/// "simulate deep link" button calls it directly to demonstrate warm-link
/// navigation without a real platform link.
pub use frust_reactive::{DeepLink, DeepLinks, deep_links, push_deep_link};

/// The Android back-press source (device-parity task 05; `frust-reactive`'s
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

/// The heavy-work idiom (phase 9.A): [`AsyncValue`] state,
/// [`use_task`] (the blessed load/compute helper), [`UseTask`] handle, and
/// [`spawn_blocking`] (the CPU-bound entry point) — Frust's counterpart to
/// Flutter's `compute()`/`FutureBuilder`, with explicit cancellation on
/// component teardown. `spawn_blocking` joins the existing
/// [`spawn`]/[`spawn_local`] routing pair (async IO / UI-thread `!Send` /
/// one-off CPU work). App crates need no new dependency: this is the whole
/// heavy-work surface. See `frust_reactive::task` for the threading contract.
pub use frust_reactive::{AsyncValue, TaskError, UseTask, spawn_blocking, use_task};

mod image_async;

/// Off-thread image decode (Phase 9.B step 1): [`decode_image_async`] wraps
/// the existing synchronous `ImageSource::decode` in `spawn_blocking`, so it
/// composes with [`use_task`] for the full load/error/ready idiom without
/// ever blocking the UI thread on a decode. See `image_async`'s module docs
/// for why this lives in the facade rather than `frust-widgets` (which stays
/// reactive-free by charter) and for the Arc-move contract the decoded
/// [`ImageSource`] crosses threads under.
pub use image_async::{ImageDecodeError, decode_image_async};

// Re-export the Android JNI-bridge macro so generated apps write
// `frust::android_app!(AppState, app_logic)` (spec §10.1). `pub use` of a
// `#[macro_export]` macro re-exports it on edition 2021+; the macro only expands
// to real code where its call site is `#[cfg(target_os = "android")]`, so this is
// inert on desktop.
pub use frust_shell_android::android_app;

// Re-export the iOS C-ABI-bridge macro so generated apps write
// `frust::ios_app!(AppState, app_logic)` (spec §10.2). Unlike `android_app!`,
// the invocation is unconditional — the macro's generated `frust_*` exports
// are each `#[cfg(target_os = "ios")]`, so it is inert off-iOS.
pub use frust_shell_ios::ios_app;

/// A Frust application: the app state plus the `app_logic` function that maps
/// it to a view tree (spec §5).
///
/// Construct with [`App::new`] and start the event loop with [`App::run`].
///
/// The view type is intentionally *not* a parameter of this struct: capturing a
/// free `fn app_logic(&mut State) -> impl View<State>`'s opaque return type into
/// a stored type parameter defeats method resolution (the opaque type's trait
/// bounds can't be re-proven on the already-typed value). Instead [`App::run`]
/// infers the view type freshly at the call site, so the exact spec §5 shape —
/// `App::new(state, app_logic).run()` — compiles for both `impl View` and
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
    /// Run the app in the desktop preview window until it is closed (spec §12.9).
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
/// closes — the canonical `runApp` equivalent (spec §5.5's Component model).
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
    let rt = frust_reactive::ReactiveRuntime::init(std::sync::Arc::new(|| {}));
    let state = rt.with_owner(|| root.init());
    App::new(state, move |state: &mut C::State| root.build(state)).run()
}

/// The canonical app entry point (spec §5.5): one line binds a root
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
#[macro_export]
macro_rules! app {
    ($root:ty $(,)?) => {
        #[cfg(target_os = "android")]
        $crate::android_app!(
            <$root as $crate::Component>::State,
            || $crate::Component::init(&<$root as ::core::default::Default>::default()),
            {
                let __frust_root = <$root as ::core::default::Default>::default();
                move |state: &mut <$root as $crate::Component>::State| {
                    $crate::Component::build(&__frust_root, state)
                }
            }
        );

        $crate::ios_app!(
            <$root as $crate::Component>::State,
            || $crate::Component::init(&<$root as ::core::default::Default>::default()),
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
            if let Err(e) = $crate::run(<$root as ::core::default::Default>::default()) {
                eprintln!("frust: {e:#}");
                ::std::process::exit(1);
            }
        }
    };
}

/// Compile-only smoke of [`app!`]: a `Component + Default` fixture bound to
/// all three platforms in one call, exercising acceptance criteria 1-3 —
/// `cargo test --workspace` compiles this on host (criterion 1: `__frust_main`
/// present, no Android JNI symbols), and `cargo check --target
/// aarch64-linux-android -p frust --tests` / `--target
/// aarch64-apple-ios-sim -p frust --tests` compile it for the two mobile
/// targets (criterion 2: the respective platform's exports appear,
/// `__frust_main` absent on Android). Lives behind `cfg(test)` — never
/// linked into a cdylib/staticlib/binary, so the fixed JNI/C-ABI export names
/// `app!` stamps out (via `android_app!`/`ios_app!`) never collide with a
/// real generated app's.
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

    crate::app!(TestApp);
}

/// Facade-level regression test for review F1: [`run`] runs a root
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

/// Facade-level check for task 6f-01: the glass material tokens
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
