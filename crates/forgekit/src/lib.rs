//! Facade crate: the public `forgekit` framework API (spec §5).
//!
//! App authors depend on this single crate. It exposes [`run`], the canonical
//! app entry point (phase 5.5), and curates the view/widget/reactive
//! vocabulary from the underlying framework crates so the declarative call
//! shape reads exactly as the spec promises:
//!
//! ```no_run
//! use forgekit::{Component, View, AnyView, any, text};
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
//! forgekit::run(Counter).unwrap();
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
//! fn app_logic(state: &mut AppState) -> impl forgekit::View<AppState> + use<> {
//!     forgekit::text(state.greeting.clone()).size(32.0)
//! }
//!
//! forgekit::App::new(AppState { greeting: "Hello from ForgeKit".into() }, app_logic)
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
//! use forgekit::{Align, Alignment, Column, EdgeInsets, Padding, Row, SizedBox, any, text};
//!
//! struct AppState;
//!
//! fn app_logic(_state: &mut AppState) -> impl forgekit::View<AppState> + use<> {
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

pub use forgekit_core::component::{Component, ComponentView, component};
pub use forgekit_core::view::{AnyView, View, any};
pub use forgekit_widgets::{
    Align, AlignView, Alignment, Axis, Button, ButtonView, Checkbox, CheckboxView, ChildKey,
    Column, CrossAxisAlignment, EdgeInsets, FlexChild, FlexView, GestureDetector,
    GestureDetectorView, Image, ImageError, ImageFit, ImageSource, ImageView, MainAxisAlignment,
    Padding, PaddingView, Row, ScrollView, SizedBox, SizedBoxView, Slider, SliderView, Stack,
    StackView, TextInput, TextInputView, TextView, button, checkbox, flexible, inflexible, keyed,
    scroll_view, slider, text, text_input,
};

/// Pure input/gesture helpers (slop constants, [`input::VelocityTracker`], the
/// fling-decay math) re-exported for app authors and advanced widgets.
pub mod input {
    pub use forgekit_core::input::{
        FLING_DECAY, FLING_STOP, MOUSE_SLOP, TOUCH_SLOP, VELOCITY_WINDOW_MS, VelocityTracker,
        WHEEL_LINE_PX, fling_decay, fling_displacement,
    };
}

/// The reactive-programming vocabulary (spec §5.5) [`Component`] state is built
/// on: signals, memos, and context, flat-re-exported from `forgekit-reactive`/
/// `reactive_graph` so app authors never name either crate directly.
pub use forgekit_reactive::{RwSignal, on_cleanup, provide_context, use_context};
pub use reactive_graph::computed::Memo;
pub use reactive_graph::signal::{ReadSignal, WriteSignal, signal};

/// Spawns a `Send` future on the background reactive runtime (Tokio-backed —
/// see `forgekit_reactive::ReactiveRuntime`). A thin wrapper over
/// `any_spawner::Executor::spawn`; app authors never name `any_spawner`.
pub fn spawn(fut: impl std::future::Future<Output = ()> + Send + 'static) {
    any_spawner::Executor::spawn(fut);
}

/// Spawns a `!Send` future on the UI-thread local task queue, drained each
/// frame by the shell (`ReactiveRuntime::pump_local`). A thin wrapper over
/// `any_spawner::Executor::spawn_local`; must be called on the UI thread —
/// see `forgekit_reactive::ReactiveRuntime::pump_local`'s doc for the panic
/// this triggers off-thread.
pub fn spawn_local(fut: impl std::future::Future<Output = ()> + 'static) {
    any_spawner::Executor::spawn_local(fut);
}

// Re-export the Android JNI-bridge macro so generated apps write
// `forgekit::android_app!(AppState, app_logic)` (spec §10.1). `pub use` of a
// `#[macro_export]` macro re-exports it on edition 2021+; the macro only expands
// to real code where its call site is `#[cfg(target_os = "android")]`, so this is
// inert on desktop.
pub use forgekit_shell_android::android_app;

// Re-export the iOS C-ABI-bridge macro so generated apps write
// `forgekit::ios_app!(AppState, app_logic)` (spec §10.2). Unlike `android_app!`,
// the invocation is unconditional — the macro's generated `forgekit_*` exports
// are each `#[cfg(target_os = "ios")]`, so it is inert off-iOS.
pub use forgekit_shell_ios::ios_app;

/// A ForgeKit application: the app state plus the `app_logic` function that maps
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
        forgekit_shell_desktop::run_desktop(self.state, self.logic)
    }
}

/// Run a root [`Component`] in the desktop preview shell until the window
/// closes — the canonical `runApp` equivalent (spec §5.5's Component model).
///
/// `root.init()` seeds the component's retained `State` once; the resulting
/// `AnyView<C::State>` is then driven through the same desktop preview loop
/// [`App::run`] uses, rebuilding from `root.build(state)` every frame.
///
/// Initializes the process-wide [`forgekit_reactive::ReactiveRuntime`] with a
/// no-op waker *before* `root.init()` runs, since a signal or context created
/// there must already have a runtime to be created under — the desktop
/// preview loop's own later `ReactiveRuntime::init` call swaps in the real
/// proxy waker on top of this seed, the documented swap-on-reinit behavior.
///
/// Desktop-only, matching [`App::run`]: on Android the app is driven by
/// [`android_app!`]/JNI instead, not by this preview loop.
#[cfg(not(target_os = "android"))]
pub fn run<C: Component>(root: C) -> anyhow::Result<()> {
    forgekit_reactive::ReactiveRuntime::init(std::sync::Arc::new(|| {}));
    App::new(root.init(), move |state: &mut C::State| root.build(state)).run()
}
