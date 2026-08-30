//! The winit 0.30 [`ApplicationHandler`] that drives a Frust app in a desktop
//! preview window.
//!
//! Ownership mirrors the shared platform bootstrap: the shell owns
//! the shared [`TextContext`], the [`RenderRoot`], the application
//! `State`/`app_logic`, and a [`FrameExecutor`](crate::render::FrameExecutor).
//! The executor owns the render stack — either on this (the UI) thread (the
//! single-thread fallback) or on a dedicated render thread (the
//! split, chosen by the `FRUST_NO_RENDER_THREAD` kill switch — see
//! [`crate::render`]). Each on-demand frame runs the UI passes in Masonry order
//! (the v0 subset) on this thread — rebuild → layout → paint — then hands the
//! finished scene to the executor for encode → acquire → submit.
//!
//! It also owns an `accesskit_winit` [`Adapter`]: created in
//! `resumed` with the same [`EventLoopProxy`](winit::event_loop::EventLoopProxy)
//! the [`ShellUserEvent`] wake mechanism already uses (extended with an
//! [`ShellUserEvent::Accessibility`] variant), forwarded every
//! [`WindowEvent`] via [`Adapter::process_event`] (its documented contract:
//! *"This must be called whenever a new window event is received"*), and fed a
//! fresh [`accesskit::TreeUpdate`](frust_core::accesskit::TreeUpdate) —
//! built from [`RenderRoot::semantics`]/[`RenderRoot::semantics_if_changed`] —
//! post-layout in `RedrawRequested`. Platform `ActionRequest`s arrive back
//! through the same proxy and route into
//! [`RenderRoot::perform_accessibility_action`]. Adapter creation cannot panic
//! without a real platform a11y bus present: `accesskit_winit`'s unix backend
//! only lazily connects to AT-SPI/D-Bus and stays inert (`update_if_active`
//! becomes a no-op) until an assistive-technology client actually activates it
//! — safe to construct in a headless/no-AT-client CI environment.

use std::any::Any;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use accesskit_winit::{
    Adapter, Event as AccessibilityEvent, WindowEvent as AccessibilityWindowEvent,
};
use anyhow::Result;
use frust_core::FrameTime;
use frust_core::RenderRoot;
use frust_core::SemanticsUpdate;
use frust_core::accesskit::{Tree, TreeId, TreeUpdate};
use frust_core::event::{
    CursorIcon, EventOutcome, ImeContentType, ImeEvent, InputEvent, Key, KeyEvent, Modifiers,
    NamedKey, PointerButton, PointerEvent, PointerPhase, ScrollDelta,
};
use frust_core::insets::WindowInsets;
use frust_core::view::View;
use frust_reactive::{FrameWaker, ReactiveRuntime, TrackedScope, provide_context};
use frust_scene::{Scene, SceneBuilder};
use frust_shell_common::font_registry::FontRegistryWatcher;
use frust_shell_common::perf::UiSpans;
use frust_shell_common::{
    SurfaceSize, ThemeOverrideWatcher, WindowMetricsPublisher, default_theme,
    effective_brightness_for_platform_change, render_thread_enabled,
};
use frust_text::TextContext;
use frust_theme::{Brightness, Theme};
use kurbo::{Affine, Point, Size};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalPosition, LogicalSize};
use winit::event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key as WinitKey, ModifiersState, NamedKey as WinitNamedKey};
use winit::window::{
    CursorIcon as WinitCursorIcon, ImePurpose, Theme as WinitTheme, Window, WindowAttributes,
    WindowId,
};

use crate::config::DesktopConfig;
use crate::extensions::{CloseAction, DesktopExtensions, NoExtensions};
use crate::paced_wake::{ControlFlowIntent, next_paced_wake, paced_wake_action};
use crate::render::FrameExecutor;

/// Initial preview-window size, in logical pixels.
const INITIAL_SIZE: LogicalSize<u32> = LogicalSize::new(800, 600);

/// The env/compile-time var overriding [`INITIAL_SIZE`]: `<width>x<height>` in
/// logical pixels (e.g. `2560x1440`). Measurement knob — see
/// [`parse_window_size`] and [`window_size_config`]. Same
/// compile-time-`option_env!`-or-runtime-`std::env::var` shape as
/// `FRUST_TRACE`/`FRUST_NO_RENDER_THREAD`, runtime winning: it exists so a
/// large-surface device/desktop gate (a macOS 5K display, say) can be measured
/// without an accessibility-scripted window move, which loses the window.
const WINDOW_SIZE_VAR: &str = "FRUST_WINDOW_SIZE";

/// The env/compile-time var maximizing the preview window at creation
/// (`1`/`true`, case-insensitive; every other value, including unset, leaves
/// it un-maximized). Measurement knob, same shape as [`WINDOW_SIZE_VAR`]. When
/// both knobs are set, [`WINDOW_SIZE_VAR`] is still applied but maximizing
/// wins visually — the OS ignores a requested inner size for a window it
/// maximizes at creation.
const WINDOW_MAXIMIZED_VAR: &str = "FRUST_WINDOW_MAXIMIZED";

/// Where an effective [`WINDOW_SIZE_VAR`]/[`WINDOW_MAXIMIZED_VAR`] value came
/// from, for the one-line-once log [`window_size_config`] emits — the same
/// "which build ran" evidence `frust-render`'s `FRUST_AA_MODE`/
/// `FRUST_RENDER_SCALE` knobs log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum WindowKnobSource {
    /// Neither knob's env var carried a non-empty value — the byte-identical
    /// default path (800x600, not maximized).
    Default,
    /// A non-empty value came from the compile-time `option_env!` half (baked
    /// in by `frust build --define`).
    Define,
    /// A non-empty value came from the runtime process environment — wins
    /// over a compile-time define when both are set.
    Env,
}

impl std::fmt::Display for WindowKnobSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            WindowKnobSource::Default => "default",
            WindowKnobSource::Define => "define",
            WindowKnobSource::Env => "env",
        })
    }
}

/// Resolves one `FRUST_*` string-valued knob's raw value plus which half
/// supplied it, checking both the compile-time (`option_env!`) and runtime
/// (`std::env::var`) halves like `frust-render::context::env_str` — **runtime
/// wins**: a non-empty runtime value is returned even when a compile-time
/// value is also set; an empty (`""`) runtime value is treated as unset and
/// falls through to the compile-time half. Not shared with `frust-render`'s
/// copy (that one is crate-private to `frust-render`) — this crate's write
/// scope is `app_handler.rs` alone, so the shape is duplicated rather than
/// promoted to a shared crate.
fn resolved_window_knob(
    compile_time: Option<&'static str>,
    runtime: Option<String>,
) -> (Option<String>, WindowKnobSource) {
    fn non_empty(value: Option<String>) -> Option<String> {
        value.filter(|v| !v.is_empty())
    }
    if let Some(value) = non_empty(runtime) {
        (Some(value), WindowKnobSource::Env)
    } else if let Some(value) = non_empty(compile_time.map(str::to_string)) {
        (Some(value), WindowKnobSource::Define)
    } else {
        (None, WindowKnobSource::Default)
    }
}

/// Parses one `<width>x<height>` dimension: strictly `^[0-9]+$`, and at least
/// 1 (a 0-sized axis is not a window).
fn parse_window_dimension(raw: &str) -> Option<u32> {
    if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    raw.parse::<u32>().ok().filter(|value| *value >= 1)
}

/// Parses a raw [`WINDOW_SIZE_VAR`] value (already resolved by
/// [`resolved_window_knob`]) into a logical window size.
///
/// Strict: `^[0-9]+x[0-9]+$`, both components at least 1 — no surrounding
/// whitespace inside either component, no sign, no decimal point, exactly one
/// `x` separator (surrounding whitespace on the whole value is trimmed first,
/// matching every other `FRUST_*` string knob's parser). An unset or empty
/// value falls back to [`INITIAL_SIZE`] silently — that is simply "the knob
/// wasn't touched". Any other value (missing/extra `x`, non-digit characters,
/// a zero component, a component too large for `u32`) also falls back to
/// `INITIAL_SIZE`, but logs one `log::warn!` naming the offending value, since
/// that case is far more likely a typo than a deliberate default.
fn parse_window_size(raw: Option<&str>) -> LogicalSize<u32> {
    let Some(trimmed) = raw.map(str::trim).filter(|v| !v.is_empty()) else {
        return INITIAL_SIZE;
    };
    if let Some((width_raw, height_raw)) = trimmed.split_once('x')
        && let (Some(width), Some(height)) = (
            parse_window_dimension(width_raw),
            parse_window_dimension(height_raw),
        )
    {
        return LogicalSize::new(width, height);
    }
    log::warn!(
        "frust-shell-desktop: invalid {WINDOW_SIZE_VAR} value {trimmed:?}, expected \
         <width>x<height> in logical pixels (e.g. 1280x720) with both at least 1, falling back \
         to {}x{}",
        INITIAL_SIZE.width,
        INITIAL_SIZE.height
    );
    INITIAL_SIZE
}

/// Parses a raw [`WINDOW_MAXIMIZED_VAR`] value (already resolved by
/// [`resolved_window_knob`]) into whether the preview window should be
/// created maximized.
///
/// `1` and `true` (case-insensitive, surrounding whitespace trimmed) enable
/// it; every other value, including unset, leaves the window un-maximized.
/// Deliberately no warn-on-unrecognised arm: unlike
/// [`parse_window_size`]'s strict grammar this is a plain flag, and the
/// effective value is logged either way by [`window_size_config`].
fn parse_window_maximized(raw: Option<&str>) -> bool {
    raw.map(str::trim)
        .is_some_and(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true"))
}

/// The process-wide effective preview-window size/maximized configuration,
/// resolved once from [`WINDOW_SIZE_VAR`]/[`WINDOW_MAXIMIZED_VAR`]
/// (compile-time-or-runtime, see [`resolved_window_knob`]) and cached — a
/// knob change requires a fresh process, matching every other `FRUST_*` knob.
/// Logs the effective choice exactly once, the same one-line-once convention
/// `frust-render`'s `FRUST_AA_MODE`/`FRUST_RENDER_SCALE` knobs use, so a
/// captured log proves which configuration a run used.
fn window_size_config() -> (LogicalSize<u32>, bool, WindowKnobSource) {
    static CONFIG: OnceLock<(LogicalSize<u32>, bool, WindowKnobSource)> = OnceLock::new();
    *CONFIG.get_or_init(|| {
        let (size_raw, size_source) = resolved_window_knob(
            option_env!("FRUST_WINDOW_SIZE"),
            std::env::var(WINDOW_SIZE_VAR).ok(),
        );
        let (maximized_raw, maximized_source) = resolved_window_knob(
            option_env!("FRUST_WINDOW_MAXIMIZED"),
            std::env::var(WINDOW_MAXIMIZED_VAR).ok(),
        );
        let size = parse_window_size(size_raw.as_deref());
        let maximized = parse_window_maximized(maximized_raw.as_deref());
        // Whichever of the two independent knobs was set from the
        // higher-precedence half decides the reported source (`Env` >
        // `Define` > `Default`) — a single log line describes the whole
        // window configuration, not each knob separately.
        let source = size_source.max(maximized_source);
        log::info!(
            "frust-shell-desktop window size={}x{} logical maximized={maximized} source={source}",
            size.width,
            size.height
        );
        (size, maximized, source)
    })
}

/// User events posted to the desktop event loop from off the UI thread.
///
/// Winit's [`ControlFlow::Wait`] idles the loop until an event arrives, so a
/// signal write on a background thread would otherwise never surface. The
/// [`FrameWaker`] installed in [`run_desktop`] fires on the clean→dirty edge of
/// a tracked signal and sends one of these through the loop's `EventLoopProxy`,
/// waking the loop to pump local tasks and schedule a redraw. Kept an enum (not
/// a unit type) so future user-driven events can be added without changing the
/// loop's user-event type.
///
/// [`ShellUserEvent::Accessibility`] is the second producer of
/// this same proxy: `accesskit_winit`'s [`Adapter::with_event_loop_proxy`]
/// requires its `T: From<accesskit_winit::Event>` bound, satisfied below. Its
/// payload (an [`accesskit::ActionRequest`](frust_core::accesskit::ActionRequest)
/// wrapper) is neither `Copy` nor `PartialEq`, so this enum can no longer
/// derive those either — every existing use already matched/constructed it by
/// value, so nothing downstream needed to change.
///
/// `pub` only so [`DesktopEventLoopBuilder`](crate::DesktopEventLoopBuilder)
/// — the type the builder-stage extension hook takes — can name the loop's
/// user-event type; `#[doc(hidden)]` keeps it out of published docs, the same
/// escape the `paced_wake` module uses for its test-only visibility.
#[doc(hidden)]
#[derive(Debug)]
pub enum ShellUserEvent {
    /// One or more tracked signals became dirty since the last frame — pump the
    /// UI-thread local task queue and request a redraw.
    SignalsDirty,
    /// An `accesskit_winit` adapter event: an initial-tree pull, a platform
    /// accessibility action request, or a deactivation notice — see
    /// [`ShellHandler::handle_accessibility_event`].
    Accessibility(AccessibilityEvent),
    /// The render thread (split path) needs another frame — a stale-swapchain
    /// reconfigure consumed the last handed-off scene, so the UI thread must
    /// repaint and re-hand one off. Routed through the proxy (rather than a
    /// direct `request_redraw` from the render thread) so winit's dirty-driven
    /// `ControlFlow::Wait` model is preserved.
    RenderNeedsRedraw,
    /// The render thread lost the surface and needs it re-created (split path).
    /// Re-creation reads the window handle, which winit only yields on the main
    /// thread, so the render thread routes the request here and the UI thread
    /// re-runs the detached surface creation and hands a fresh one across, then
    /// requests a repaint.
    RenderRecreateSurface,
    /// The render thread hit an unrecoverable init error (surface creation
    /// failed). Surfaced back so `run_desktop` returns it as `Err`, mirroring the
    /// inline path's fatal handling.
    RenderFatal(anyhow::Error),
    /// The devtools service queued a request that needs UI-thread state (a
    /// widget-tree snapshot, an injected event). This shell idles under
    /// [`ControlFlow::Wait`], so the queue is drained on the loop turn this
    /// event produces — the desktop half of `frust_shell_common::devtools`'s
    /// UI-thread hop.
    #[cfg(feature = "devtools")]
    Devtools,
}

impl From<AccessibilityEvent> for ShellUserEvent {
    fn from(event: AccessibilityEvent) -> Self {
        ShellUserEvent::Accessibility(event)
    }
}

/// Run `app_logic` over `state` in a desktop preview window until it is closed.
///
/// Blocks the calling thread on the winit event loop. The event loop is
/// on-demand ([`ControlFlow::Wait`]): frames are produced only in response to a
/// redraw request (state change on rebuild, or a resize), never free-running.
///
/// The zero-config entry point: delegates to [`run_desktop_with`] with a
/// default [`DesktopConfig`] (title `"Frust"`, no icon, no menu, close quits)
/// and [`NoExtensions`], so the dev preview behaves exactly as it did before
/// either seam existed. A per-OS shell crate calls [`run_desktop_with`]
/// instead.
pub fn run_desktop<State, Logic, V>(state: State, app_logic: Logic) -> Result<()>
where
    State: 'static,
    V: View<State>,
    Logic: FnMut(&mut State) -> V + 'static,
{
    run_desktop_with(state, app_logic, DesktopConfig::default(), NoExtensions)
}

/// [`run_desktop`] with the app's desktop identity ([`DesktopConfig`]) and a
/// per-OS extension set ([`DesktopExtensions`]) supplied.
///
/// This is the entry point the facade calls once it knows which platform shell
/// it is running under: `frust-shell-macos`/`-windows`/`-linux` each construct
/// their own `E` (handing it the same `config`, which they read for identity
/// this core only carries — `app_id`, `window_icon`, `menu_spec`) and call
/// here. Generic over `E` rather than boxed — see [`crate::extensions`]'s
/// module docs.
pub fn run_desktop_with<State, Logic, V, E>(
    state: State,
    app_logic: Logic,
    config: DesktopConfig,
    mut extensions: E,
) -> Result<()>
where
    State: 'static,
    V: View<State>,
    Logic: FnMut(&mut State) -> V + 'static,
    E: DesktopExtensions,
{
    // Install the stderr `log::Log` sink once, so `frust-shell-common::perf`'s
    // `log::info!` startup-span/frame-stats lines below are actually visible
    // (desktop had no logger at all before this — see `logger`'s module docs).
    crate::logger::init_once();

    // The shell-owned monotonic epoch every per-frame `FrameTime` is measured
    // from (time enters from the shell). Captured before any GPU/thread
    // setup so the render thread's startup line and the UI thread's frame clock
    // share one origin.
    let epoch = Instant::now();

    // winit 0.30 has no `EventLoop::<T>::new()` — the typed-user-event loop is
    // built through the builder only. The builder is held in a local (rather
    // than built off a temporary) so the extension's builder-stage hook can
    // install platform builder extensions winit accepts nowhere else — a
    // Windows message hook for menu accelerators being the motivating case.
    let mut builder = EventLoop::<ShellUserEvent>::with_user_event();
    extensions.on_event_loop_builder(&mut builder);
    let event_loop = builder.build()?;
    event_loop.set_control_flow(ControlFlow::Wait);

    // A tracked signal write on any thread fires this waker, which sends a user
    // event through the proxy to wake the `Wait` loop. `send_event` fails only
    // once the loop has closed (a shutdown race) — benign, so its result is
    // ignored. `ReactiveRuntime::init` must run on this (the UI) thread: it
    // claims the thread as owner of the local task queue `pump_local` drains.
    let proxy = event_loop.create_proxy();
    // A second clone is kept on the handler (`accesskit_proxy`) so `resumed` can
    // hand the accesskit_winit `Adapter` its own event-loop proxy — the waker
    // closure below consumes its own clone, so neither producer starves.
    let accesskit_proxy = proxy.clone();
    let waker: FrameWaker = Arc::new(move || {
        let _ = proxy.send_event(ShellUserEvent::SignalsDirty);
    });
    let runtime = ReactiveRuntime::init(waker);

    // Devtools, compiled in only under this crate's `devtools` feature. Started
    // here because both of its prerequisites first exist at this point: the
    // stderr logger installed above (the discovery line must reach a
    // developer's terminal) and an `EventLoopProxy` to wake the `Wait` loop
    // with when a backend request needs the UI thread. Never load-bearing — a
    // bind failure or `FRUST_DEVTOOLS=0` just means no devtools this run.
    #[cfg(feature = "devtools")]
    {
        let devtools_proxy = event_loop.create_proxy();
        frust_shell_common::devtools::start(
            frust_shell_common::devtools::app_name_from_process(),
            Some(Box::new(move || {
                // Fails only once the loop has closed (a shutdown race) — the
                // same benign case the frame waker above ignores.
                let _ = devtools_proxy.send_event(ShellUserEvent::Devtools);
            })),
        );
    }

    // Pick the frame executor once at startup from the `FRUST_NO_RENDER_THREAD`
    // kill switch. The split path spawns a dedicated render
    // thread that owns the `RenderContext` + `SurfaceRenderer` wholesale (both
    // `Send`); the inline path keeps them on this (the UI) thread — the fallback
    // preserved for comparison against the split.
    let executor = if render_thread_enabled() {
        let render_proxy = event_loop.create_proxy();
        FrameExecutor::Split(crate::render::spawn_render_thread(render_proxy))
    } else {
        FrameExecutor::Inline(Box::default())
    };

    let mut handler = ShellHandler {
        state,
        app_logic,
        config,
        extensions,
        runtime,
        scope: TrackedScope::new(),
        root: RenderRoot::new(),
        text_ctx: TextContext::new(),
        executor,
        scene: Scene::new(),
        cursor: Point::ZERO,
        modifiers: Modifiers::default(),
        secondary_down_delivered: false,
        compose: ComposeLatch::default(),
        ime_sync: ImeSync::default(),
        cursor_icon: CursorIcon::Default,
        window: None,
        accesskit_proxy,
        adapter: None,
        // The generation `RenderRoot::new()` starts at (0); the first rebuild
        // always dirties it to a different value (a first `rebuild_view` always
        // returns non-empty `ChangeFlags` — see `RenderRoot::rebuild`), so the
        // first post-layout `semantics_if_changed` check below is guaranteed to
        // see a change and push the initial tree.
        semantics_seen: 0,
        fatal: None,
        epoch,
        // The seeded base theme (`base_theme`: a design system's
        // `set_default_theme`, else the built-in `Theme::neutral()` fallback),
        // carrying that base's own brightness only until `resumed` seeds the
        // window's real preference (`brightness_from_winit` there overwrites
        // `brightness` unconditionally, so a seeded base's own starting
        // brightness never leaks into a light-preference platform).
        theme: base_theme(default_theme()),
        theme_seeded: false,
        // No brightness reported to the extension yet, so the first
        // resolution (`resumed`'s seed) counts as a change and fires the hook.
        brightness_notified: None,
        theme_override: ThemeOverrideWatcher::new(),
        theme_override_active: false,
        font_registry: FontRegistryWatcher::new(),
        paced_wake: None,
        anim_pacing: !frust_shell_common::anim_pacing_kill_switch_engaged(),
        window_metrics: WindowMetricsPublisher::new(),
    };

    // Construction-time font drain: apply any fonts registered via
    // `frust::register_app_fonts` before `run` (app construction) into the
    // shell-owned `TextContext` before the first layout — inline, on this UI
    // thread. Pre-first-layout, so no invalidation/relayout is needed; the
    // per-frame poll in `RedrawRequested` picks up any later registration.
    handler.font_registry.drain_into(&mut handler.text_ctx);

    event_loop.run_app(&mut handler)?;
    finish(handler.fatal)
}

/// The base [`Theme`] this shell seeds itself from: the design-system-supplied
/// default (`frust_shell_common::set_default_theme`, read back through
/// [`default_theme`]) when a plugin seeded one, else the shell's own built-in
/// [`Theme::neutral`] fallback.
///
/// A seeded default supplies only the *starting point*: unlike an app-forced
/// override (`set_app_theme`) it does not pin brightness — every call site
/// below still derives `brightness` from the platform's own preference against
/// this base, so a design-system default keeps following system dark mode.
///
/// The fallback is deliberately the design-language-free
/// [`Theme::neutral`] — system fonts, no bundled font bytes: a shell names no
/// design system of its own, so an app that installs none gets the neutral
/// floor rather than someone's brand. A design system supplies both halves
/// itself (its base theme through `set_default_theme`, its font bytes through
/// `frust_shell_common::font_registry::register_app_fonts`).
///
/// Takes the slot's value as an argument rather than reading the process-global
/// itself, so the fallback ladder is unit-testable without touching a
/// process-wide slot that has no reset; every call site passes
/// [`default_theme()`](default_theme).
fn base_theme(seeded: Option<Theme>) -> Theme {
    seeded.unwrap_or_else(Theme::neutral)
}

/// The theme a cleared app-theme override reverts to: the seeded base
/// ([`base_theme`] — a design system's `set_default_theme`, else the built-in
/// fallback) at the platform's *current* brightness, never the cleared
/// override's own pinned one.
///
/// Extracted so the `clear_app_theme` arm below and its unit tests run one
/// implementation: a test that recomputed this in its own body would stay green
/// if the arm regressed to an unconditional `Theme::neutral()` — the exact
/// regression this ladder exists to prevent.
fn reverted_theme(seeded: Option<Theme>, platform: Brightness) -> Theme {
    let mut theme = base_theme(seeded);
    theme.brightness = platform;
    theme
}

/// The active-theme decision for one [`ThemeOverrideWatcher::poll`] result —
/// the precedence ladder's top two rungs as one pure function, shared by the
/// per-frame poll arm in `RedrawRequested` and its unit tests.
///
/// Returns `None` when the poll reported no change (the shell leaves its theme
/// alone), else the new active theme paired with whether an app-forced override
/// is now pinning it (`ShellHandler::theme_override_active`).
///
/// `seeded`/`platform` are suppliers rather than values because only the
/// cleared-override arm needs them: reading the process-global default slot
/// ([`default_theme`] — a `Mutex` lock plus a whole-`Theme` clone) and querying
/// the window's reported appearance would otherwise become per-frame cost for a
/// poll that reports "nothing changed" on all but a handful of frames.
fn theme_after_override_poll(
    polled: Option<Option<Theme>>,
    seeded: impl FnOnce() -> Option<Theme>,
    platform: impl FnOnce() -> Brightness,
) -> Option<(Theme, bool)> {
    match polled {
        // `set_app_theme`: the forced theme wins wholesale — neither the seeded
        // default nor the platform's brightness is even consulted (the
        // override-wins rule).
        Some(Some(theme)) => Some((theme, true)),
        // `clear_app_theme`: back to the seeded base at the platform's own
        // current brightness.
        Some(None) => Some((reverted_theme(seeded(), platform()), false)),
        None => None,
    }
}

/// Re-derive `theme`'s brightness from a platform appearance report, honouring
/// the override-wins rule ([`effective_brightness_for_platform_change`]): an
/// app-forced override pins brightness, a design-system-seeded default does not
/// — `theme` still IS that base, so flipping it in place re-derives light/dark
/// against the design system's own tokens.
///
/// Extracted for the same reason as [`reverted_theme`]: the
/// `WindowEvent::ThemeChanged` arm and the seed-ladder tests share one
/// implementation.
fn follow_platform_brightness(theme: &mut Theme, override_active: bool, platform: Brightness) {
    theme.brightness =
        effective_brightness_for_platform_change(override_active, theme.brightness, platform);
}

/// Applies the resolved window-size configuration ([`window_size_config`]) to
/// an in-progress [`WindowAttributes`]: the logical inner size always, plus
/// `with_maximized(true)` when the maximized knob is on. A free function
/// (rather than inlined in [`window_attributes`]) so the "known
/// size/maximized in, attributes out" step is unit-testable host-side without
/// touching the process environment.
fn apply_window_size(
    attributes: WindowAttributes,
    size: LogicalSize<u32>,
    maximized: bool,
) -> WindowAttributes {
    let attributes = attributes.with_inner_size(size);
    if maximized {
        attributes.with_maximized(true)
    } else {
        attributes
    }
}

/// Assemble the preview window's [`WindowAttributes`] from the app's
/// [`DesktopConfig`], then hand them to the extension's pre-create hook.
///
/// The core's own attributes come first so an extension can override any of
/// them:
/// * the title, from [`DesktopConfig::window_title`] (the configured
///   `app_name`, else the `"Frust"` fallback this shell used to hardcode);
/// * the initial logical size — [`INITIAL_SIZE`] by default, overridable via
///   the [`WINDOW_SIZE_VAR`]/[`WINDOW_MAXIMIZED_VAR`] measurement knobs (see
///   [`window_size_config`]);
/// * `visible(false)` — the accessibility adapter must be constructed before
///   the window is ever shown (see [`Adapter::with_event_loop_proxy`]'s
///   contract), so `resumed` makes it visible itself once that is done.
///
/// A free function taking the extension rather than a `ShellHandler` method so
/// the config→attributes→hook composition is unit-testable with a spy, without
/// a live event loop.
fn window_attributes(
    config: &DesktopConfig,
    extensions: &mut impl DesktopExtensions,
) -> WindowAttributes {
    let (size, maximized, _source) = window_size_config();
    let attributes = WindowAttributes::default()
        .with_title(config.window_title())
        .with_visible(false);
    let attributes = apply_window_size(attributes, size, maximized);
    extensions.on_window_attributes(attributes)
}

/// The brightness (if any) to report through
/// [`DesktopExtensions::on_theme_brightness_changed`], given the last value
/// reported and the shell's current resolved one.
///
/// `last` is `None` before the first report, so the initial resolution counts
/// as a change: a platform shell learns the starting brightness rather than
/// having to guess it and wait for a flip. Afterwards only a genuine difference
/// fires — the hook's "actually changed" contract, which matters because its
/// call site sits inside `apply_theme` (see there) and would otherwise fire on
/// every re-push, including ones that swap one dark theme for another.
fn brightness_change_to_notify(
    last: Option<Brightness>,
    current: Brightness,
) -> Option<Brightness> {
    (last != Some(current)).then_some(current)
}

/// Turns a post-loop `ShellHandler::fatal` into the `run_desktop` result.
///
/// Pulled out of `run_desktop` so the "surface a stashed init error as `Err`"
/// behavior is unit-testable without driving a real winit event loop.
fn finish(fatal: Option<anyhow::Error>) -> Result<()> {
    match fatal {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// Apply a [`ControlFlowIntent`] (the winit-free decision from the
/// `paced_wake` module) to the live event loop. [`ControlFlowIntent::Unchanged`]
/// is a deliberate no-op — the paint-time paced-only case leaves the loop
/// parked on whatever it already is, for the next `about_to_wait` to
/// resolve. Pulled out so the translation from intent to `set_control_flow`
/// is one place shared by both the paint and `about_to_wait` call sites.
fn apply_control_flow(event_loop: &ActiveEventLoop, intent: ControlFlowIntent) {
    match intent {
        ControlFlowIntent::Wait => event_loop.set_control_flow(ControlFlow::Wait),
        ControlFlowIntent::WaitUntil(deadline) => {
            event_loop.set_control_flow(ControlFlow::WaitUntil(deadline))
        }
        ControlFlowIntent::Unchanged => {}
    }
}

/// Convert a winit physical-pixel position into logical (density-independent)
/// pixels by the window's `scale_factor`.
///
/// winit reports `CursorMoved`/`PixelDelta` in **physical** pixels (verified
/// empirically on macOS — a HiDPI window reports positions at 2× the logical
/// point value); the widget tree lays out and hit-tests in the same logical
/// space the layout pass uses, so every pointer coordinate is
/// divided by the scale factor at the shell boundary. Pulled out as a free
/// function so the conversion is unit-testable without a live window.
fn physical_to_logical(x: f64, y: f64, scale: f64) -> Point {
    Point::new(x / scale, y / scale)
}

/// Map a winit [`MouseScrollDelta`] to our [`ScrollDelta`], bridging both the
/// unit and the sign convention at the shell boundary.
///
/// Units: wheel notches stay [`ScrollDelta::Lines`] (the ScrollView widget
/// converts to px at 40 px/line); a precision trackpad's `PixelDelta` is
/// physical, so it is divided by the scale factor into logical pixels like
/// every other coordinate.
///
/// Sign: winit reports the direction the **content** moves — a positive `y`
/// moves the content down, revealing what sits above it (a natural-scroll
/// preference is resolved by the OS before winit sees the delta, so this holds
/// on every platform). Frust's scrollable widgets accumulate `offset + dy`,
/// where a growing offset reveals **later** content — the opposite sense, on
/// both axes. Both axes are therefore negated here, at the one boundary that
/// knows winit's convention; everything downstream sees only frust's. The
/// negation is load-bearing, not redundant sign-juggling: dropping it inverts
/// scrolling in every desktop app, and correcting it in a widget instead would
/// invert the touch/fling and programmatic paths that already agree.
///
/// Pulled out as a free function (the call site reads `scale` off a live
/// `Window`) so the mapping stays a pure, directly unit-testable function.
fn map_scroll_delta(delta: MouseScrollDelta, scale: f64) -> ScrollDelta {
    match delta {
        MouseScrollDelta::LineDelta(x, y) => ScrollDelta::Lines(-(x as f64), -(y as f64)),
        MouseScrollDelta::PixelDelta(px) => ScrollDelta::Pixels(-px.x / scale, -px.y / scale),
    }
}

/// Map a winit [`MouseButton`] to our [`PointerButton`] vocabulary, or `None`
/// for a button we forward nothing for yet (Middle/Back/Forward — dropped
/// rather than misreported, until those gestures are specced; `PointerButton`
/// itself already models `Middle`, but nothing in this shell dispatches it
/// yet, so it stays out of the mapping alongside the two winit variants that
/// have no `PointerButton` counterpart at all).
///
/// Pulled out as a free function so the mapping stays pure and directly
/// unit-testable, the same shape as [`map_scroll_delta`] below.
fn map_mouse_button(button: MouseButton) -> Option<PointerButton> {
    match button {
        MouseButton::Left => Some(PointerButton::Primary),
        MouseButton::Right => Some(PointerButton::Secondary),
        _ => None,
    }
}

/// Whether a mapped button's press/release should reach the tree at all, given
/// the phase, whether a pointer gesture is currently captured
/// ([`RenderRoot::is_pointer_captured`]), and the delivery latch this gate owns
/// (`ShellHandler::secondary_down_delivered`, threaded in by `&mut`).
///
/// Primary is never gated: it is the button a capture belongs to, and its whole
/// gesture must reach the tree.
///
/// Secondary is gated to keep two invariants at once:
///
/// - **A secondary press never disturbs a live capture.** `RenderRoot` tracks
///   capture as a single root-level flag, not one per button, and releases it
///   on phase alone — never button-checked (see `RenderRoot::event`). A
///   secondary `Down` routed to a mid-drag capturer would hand it a transition
///   it never asked for, and a secondary `Up` would clear the flag out from
///   under a still-live primary gesture. So a secondary `Down` arriving while
///   captured is dropped.
/// - **Pairing: the tree never sees an unpaired secondary event.** A widget's
///   press contract is a `Down` followed by its own `Up`, so dropping a release
///   whose press *was* delivered is as wrong as delivering a release whose press
///   was not — a stateless "drop while captured" rule does exactly that when a
///   capture opens between the two (a right-click that opened a context menu
///   whose `Up` then vanishes, leaving the menu's own press state armed). The
///   latch therefore records what happened to the `Down`, and the `Up` follows
///   it regardless of what the capture flag says by then.
///
/// Pulled out as a free function so the decision stays directly unit-testable
/// without a live `RenderRoot`, the same shape as [`map_scroll_delta`].
fn mouse_button_should_dispatch(
    button: PointerButton,
    phase: PointerPhase,
    pointer_captured: bool,
    secondary_down_delivered: &mut bool,
) -> bool {
    if button != PointerButton::Secondary {
        return true;
    }
    match phase {
        PointerPhase::Down => {
            let deliver = !pointer_captured;
            *secondary_down_delivered = deliver;
            deliver
        }
        PointerPhase::Up => {
            let deliver = *secondary_down_delivered;
            *secondary_down_delivered = false;
            deliver
        }
        // `MouseInput` is the only caller and produces just the two phases
        // above; a pointer `Move`/`Cancel` is built elsewhere (`CursorMoved`
        // makes its own Primary event) and never reaches this gate.
        PointerPhase::Move | PointerPhase::Cancel => true,
    }
}

/// Map a winit [`WinitNamedKey`] to our editing-semantics [`NamedKey`] set,
/// or `None` for a named key we carry no editing semantics for
/// (function keys, media keys, etc. — those fall through to `KeyboardInput`
/// being dropped rather than misreported as text).
fn map_named_key(key: WinitNamedKey) -> Option<NamedKey> {
    Some(match key {
        WinitNamedKey::Enter => NamedKey::Enter,
        WinitNamedKey::Backspace => NamedKey::Backspace,
        WinitNamedKey::Delete => NamedKey::Delete,
        WinitNamedKey::ArrowLeft => NamedKey::ArrowLeft,
        WinitNamedKey::ArrowRight => NamedKey::ArrowRight,
        WinitNamedKey::ArrowUp => NamedKey::ArrowUp,
        WinitNamedKey::ArrowDown => NamedKey::ArrowDown,
        WinitNamedKey::Home => NamedKey::Home,
        WinitNamedKey::End => NamedKey::End,
        WinitNamedKey::Escape => NamedKey::Escape,
        WinitNamedKey::Tab => NamedKey::Tab,
        _ => return None,
    })
}

/// Map winit's window [`WinitTheme`] (`None` when the platform can't report a
/// preference) to a [`Brightness`], defaulting to [`Brightness::Light`].
///
/// Pulled out as a free function so the seed-and-flip mapping is unit-testable
/// without a live window (winit's `Window::theme()` needs a real window).
fn brightness_from_winit(theme: Option<WinitTheme>) -> Brightness {
    match theme {
        Some(WinitTheme::Dark) => Brightness::Dark,
        _ => Brightness::Light,
    }
}

/// Map winit's [`ModifiersState`] bitflags to our [`Modifiers`] chord.
fn map_modifiers(state: ModifiersState) -> Modifiers {
    Modifiers {
        shift: state.shift_key(),
        ctrl: state.control_key(),
        alt: state.alt_key(),
        meta: state.super_key(),
    }
}

/// Map a winit `KeyboardInput`'s fields into our [`KeyEvent`], or `None` when
/// it produces nothing worth dispatching.
///
/// Takes the individual fields off `winit::event::KeyEvent` (rather than the
/// struct itself, whose `platform_specific` field is private to winit and so
/// can't be constructed in this crate's tests) so the mapping stays a pure,
/// directly unit-testable function.
///
/// Named keys ([`WinitKey::Named`]) map through [`map_named_key`]; anything
/// else falls back to the key's resolved `text` (already dead-key/smart-quote
/// resolved by the platform) as [`Key::Character`]. The spacebar is the one
/// named key that resolves to a character instead: winit models it as
/// [`WinitNamedKey::Space`] (never `Character(" ")`), but it types a space
/// rather than carrying editing semantics, so it takes the character path with
/// its `text` payload — falling back to `" "` on a platform that sends none.
/// Either character path is dropped **while** `composing` is
/// set (the dedupe rule: while an IME
/// [`Ime::Preedit`] is active, `Ime::Commit` is the authoritative source for the
/// composed text, not `KeyboardInput.text`). `repeat` passes through unfiltered
/// — callers decide whether to honor auto-repeat. Only key-down (`Pressed`)
/// events map; releases produce `None` (spec: `KeyEvent` has no up/down phase).
fn map_key_event(
    logical_key: &WinitKey,
    text: Option<&str>,
    state: ElementState,
    repeat: bool,
    modifiers: Modifiers,
    composing: bool,
) -> Option<KeyEvent> {
    if state != ElementState::Pressed {
        return None;
    }
    let key = match logical_key {
        // Space is a named key that types text, so it resolves to a character
        // ahead of the editing-semantics mapping below — `map_named_key` carries
        // no `NamedKey` for it, and letting it fall through there would drop the
        // event and leave every text widget unable to receive a space.
        WinitKey::Named(WinitNamedKey::Space) => {
            if composing {
                return None;
            }
            Key::Character(text.unwrap_or(" ").to_string())
        }
        WinitKey::Named(named) => Key::Named(map_named_key(*named)?),
        _ => {
            if composing {
                return None;
            }
            Key::Character(text?.to_string())
        }
    };
    Some(KeyEvent {
        key,
        modifiers,
        repeat,
    })
}

/// Tracks whether an IME preedit (marked-text) composition is currently in
/// progress, purely from the `WindowEvent::Ime` stream — the dedupe latch
/// [`map_key_event`] consults so `KeyboardInput`-derived `Character` events are
/// suppressed while `Ime::Preedit`/`Ime::Commit` are the authoritative source
/// (winit reportedly withholds `KeyboardInput` during preedit on macOS, but we
/// dedupe defensively cross-platform per the task's research note).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct ComposeLatch {
    active: bool,
}

impl ComposeLatch {
    /// Observe one `WindowEvent::Ime` variant, updating the latch and mapping
    /// it to our [`ImeEvent`] in the same pass (the mapping and the
    /// dedupe-state transition are the same decision, so they live together).
    ///
    /// An empty-text `Preedit` closes the latch (per winit's contract it
    /// precedes a `Commit`); `Commit`/`Enabled`/`Disabled` also close it — each
    /// ends or restarts a composition session.
    fn observe(&mut self, ime: &Ime) -> ImeEvent {
        match ime {
            Ime::Preedit(text, cursor) => {
                self.active = !text.is_empty();
                ImeEvent::Compose {
                    text: text.clone(),
                    cursor: *cursor,
                }
            }
            Ime::Commit(text) => {
                self.active = false;
                ImeEvent::Commit(text.clone())
            }
            Ime::Enabled => {
                self.active = false;
                ImeEvent::Enabled
            }
            Ime::Disabled => {
                self.active = false;
                ImeEvent::Disabled
            }
        }
    }

    /// Whether a `KeyboardInput`-derived `Character` event should be dropped
    /// because a composition is currently in progress.
    fn is_composing(&self) -> bool {
        self.active
    }
}

/// Map a published [`ImeContentType`] onto the nearest winit [`ImePurpose`].
///
/// **This is the full extent of what pinned winit 0.30.13 offers for this
/// hint, and it is a weak one.** [`Window::set_ime_purpose`] docs itself as
/// unsupported on every backend *except* Wayland — iOS, Android, Web,
/// Windows, X11, and macOS are all listed explicitly as no-ops. Where it
/// *is* honoured (Wayland, via the compositor's own input-method/OSK), it is
/// a cosmetic hint, not a security boundary: winit has no secure-text-entry
/// concept, so plaintext still crosses the same `Ime`/`KeyboardInput` event
/// stream either way (see [`frust_core::event::ImeState`]'s own
/// "residual exposure" note, which already documents this for the mobile
/// shells — the same limit applies here, just with a much smaller platform
/// footprint that actually honours it).
///
/// `ImePurpose` distinguishes exactly `Normal`/`Password`/`Terminal`, so
/// [`NoSuggestions`](ImeContentType::NoSuggestions) has nothing more precise
/// to map onto than `Normal` — desktop has no channel to ask a platform IME
/// to suppress suggestions/learning without also claiming secure entry it
/// can't actually deliver on this backend. [`Terminal`](ImeContentType::Terminal)
/// is the one variant this backend can map exactly: winit's own `Terminal`
/// purpose exists for precisely this raw byte-entry case ("input into a
/// terminal" per its doc comment), so unlike `NoSuggestions` it does not fall
/// back to `Normal`.
///
/// Recorded in `docs/SHELLS_ARCHITECTURE.md`'s IME section.
fn ime_purpose_for(content_type: ImeContentType) -> ImePurpose {
    match content_type {
        ImeContentType::Terminal => ImePurpose::Terminal,
        other if other.is_secret() => ImePurpose::Password,
        _ => ImePurpose::Normal,
    }
}

/// Map a resolved framework [`CursorIcon`] onto winit's own cursor vocabulary.
///
/// The single place in the framework where a cursor name touches a platform:
/// core carries the request platform-neutrally (see
/// [`frust_core::event::EventCtx::set_cursor`]) and this is the desktop
/// translation. Every framework variant has an exact winit counterpart in the
/// pinned `winit 0.30.13` (which sources its icons from `cursor-icon`, whose
/// names follow CSS), so nothing here approximates.
///
/// The wildcard arm is not dead code: [`CursorIcon`] is `#[non_exhaustive]`, so a
/// variant added later must compile here and *degrade* to the platform arrow
/// rather than break the build or invent a shape.
fn winit_cursor_for(icon: CursorIcon) -> WinitCursorIcon {
    match icon {
        CursorIcon::Default => WinitCursorIcon::Default,
        CursorIcon::Pointer => WinitCursorIcon::Pointer,
        CursorIcon::Text => WinitCursorIcon::Text,
        CursorIcon::Grab => WinitCursorIcon::Grab,
        CursorIcon::Grabbing => WinitCursorIcon::Grabbing,
        CursorIcon::ColResize => WinitCursorIcon::ColResize,
        CursorIcon::RowResize => WinitCursorIcon::RowResize,
        CursorIcon::NotAllowed => WinitCursorIcon::NotAllowed,
        _ => WinitCursorIcon::Default,
    }
}

/// The winit cursor (if any) to push, given the shape last pushed and the one
/// [`RenderRoot::cursor`] now resolves to.
///
/// `None` means "say nothing to winit": the resolved cursor re-resolves on every
/// pointer `Move`, so an unguarded `set_cursor` would fire a platform call on
/// every mouse motion for a value that had not moved. Split out as a free
/// function for [`brightness_change_to_notify`]'s reason — the change decision is
/// then unit-testable without a live window.
fn cursor_change_to_apply(last: CursorIcon, current: CursorIcon) -> Option<WinitCursorIcon> {
    (last != current).then(|| winit_cursor_for(current))
}

/// Cached view of what we last told winit about the platform IME, so
/// [`ShellHandler::sync_ime`] only calls
/// `set_ime_allowed`/`set_ime_cursor_area`/`set_ime_purpose` on an actual
/// change rather than every dispatched event.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
struct ImeSync {
    /// The last `set_ime_allowed` value sent to winit.
    allowed: bool,
    /// The last `set_ime_cursor_area` position/size sent to winit, if any.
    cursor_area: Option<(LogicalPosition<f64>, LogicalSize<f64>)>,
    /// The last [`ImePurpose`] sent to winit via `set_ime_purpose` — see
    /// [`ime_purpose_for`] for how far that call actually reaches.
    purpose: ImePurpose,
}

/// Owns everything a running desktop app needs across frames.
struct ShellHandler<State: 'static, Logic, V: View<State>, E> {
    state: State,
    app_logic: Logic,
    /// The app's desktop identity, threaded in from [`run_desktop_with`]. Read
    /// by this core only for the window title (see [`window_attributes`]); the
    /// rest of it is the per-OS shells' business, and they hold their own copy.
    config: DesktopConfig,
    /// The per-OS extension set — [`NoExtensions`] for the zero-config preview.
    /// Static, not boxed (see [`crate::extensions`]'s module docs), so a hook
    /// that a shell leaves at its default costs nothing at runtime.
    extensions: E,
    /// The process-wide reactive runtime, initialized on this (UI) thread in
    /// [`run_desktop`]. Per-frame rebuilds run under its root [`Owner`], and
    /// [`ReactiveRuntime::pump_local`] drains the UI-thread local task queue on
    /// each wake and frame.
    runtime: &'static ReactiveRuntime,
    /// Records which signals the last rebuild read, so a later write to any of
    /// them dirties the scope and (via the frame waker) wakes the loop for one
    /// more frame. Re-tracked from scratch every rebuild.
    scope: TrackedScope,
    root: RenderRoot<State, V>,
    text_ctx: TextContext,
    /// The frame executor: either the render-thread split or
    /// the single-thread fallback, chosen once at startup from the
    /// `FRUST_NO_RENDER_THREAD` kill switch. Owns the `RenderContext` +
    /// `SurfaceRenderer` (inline) or the render-thread handle (split), and all
    /// the encode→present + perf recording that used to live inline here (see
    /// [`crate::render`]). Declared before `window` so its `Drop` (which joins
    /// the render thread) runs while the shell still holds a window `Arc`, so the
    /// window is destroyed on the main thread.
    executor: FrameExecutor,
    /// Reused across frames; `reset()` each frame rather than reallocated. In the
    /// split path a finished scene is moved out (replaced with a fresh one) to
    /// cross the handoff channel; inline reuses it in place.
    scene: Scene,
    /// Last known cursor position in logical pixels, updated on every
    /// `CursorMoved`. `MouseInput` (button press/release) carries no position of
    /// its own, so it reuses this — mirroring how winit models the two events.
    cursor: Point,
    /// Current modifier chord, updated by `ModifiersChanged` — which winit
    /// delivers *before* the `KeyboardInput` that uses it, so this is always
    /// current by the time a key event is mapped.
    modifiers: Modifiers,
    /// Whether the last secondary (right) `Down` was actually dispatched to the
    /// tree — the latch [`mouse_button_should_dispatch`] pairs a secondary
    /// release against, so the tree never sees an unpaired one.
    secondary_down_delivered: bool,
    /// Tracks whether an IME preedit composition is in progress, so
    /// `KeyboardInput`-derived `Character` events can be deduped against it.
    compose: ComposeLatch,
    /// What we last told winit about the platform IME (`set_ime_allowed`/
    /// `set_ime_cursor_area`), so [`ShellHandler::sync_ime`] only calls them on
    /// an actual change.
    ime_sync: ImeSync,
    /// The cursor shape last pushed to winit via `Window::set_cursor` — the change
    /// gate behind [`ShellHandler::sync_cursor`]. Seeded with
    /// [`CursorIcon::Default`], which is the shape a fresh window already has, so
    /// the first push happens only when a widget actually asks for something else.
    /// Named apart from `cursor` above deliberately: that one is the pointer's
    /// *position*, this one its *shape*.
    cursor_icon: CursorIcon,
    /// Created lazily in `resumed()` (macOS requires window creation there).
    window: Option<Arc<Window>>,
    /// A clone of the wake proxy handed to the `accesskit_winit` [`Adapter`] at
    /// creation — kept separate from the `FrameWaker`'s own clone
    /// (`run_desktop`) so `resumed` can construct the adapter without needing
    /// to unpick it from the (already-moved-into-a-closure) waker.
    accesskit_proxy: winit::event_loop::EventLoopProxy<ShellUserEvent>,
    /// The `accesskit_winit` platform adapter, created once in
    /// `resumed` alongside the window (it must be constructed before the
    /// window is first shown — see [`Adapter::with_event_loop_proxy`]'s
    /// contract). `None` only before the first `resumed` call.
    adapter: Option<Adapter>,
    /// The [`RenderRoot::semantics_generation`] value last pushed to the
    /// adapter (the dirty gate) — compared every `RedrawRequested`
    /// via [`RenderRoot::semantics_if_changed`] so an unchanged tree is never
    /// re-walked/re-pushed.
    semantics_seen: u64,
    /// Set when `resumed` hits an unrecoverable init error; `event_loop.exit()`
    /// only stops the loop (it can't return an `Err`), so the error is stashed
    /// here and re-raised by `run_desktop` once `run_app` returns.
    fatal: Option<anyhow::Error>,
    /// The shell-owned monotonic epoch the per-frame [`FrameTime`] is measured
    /// from. `frust-core` never reads a clock itself (time enters from
    /// the shell) — the desktop shell samples `epoch.elapsed()` at paint and hands
    /// the nanosecond delta to [`RenderRoot::paint`]. This stays the
    /// desktop clock; only the mobile shells swap in a platform vsync timestamp.
    epoch: Instant,
    /// The app's active theme — the seeded base ([`base_theme`]: a design
    /// system's `set_default_theme`, else the built-in fallback) until an
    /// app-forced override replaces it. The shell owns the appearance
    /// state: its `brightness` is seeded from the window's reported preference
    /// in `resumed` and flipped live on `WindowEvent::ThemeChanged`. On every
    /// change the shell re-boxes it into [`RenderRoot::set_theme`] (so widgets
    /// read it via `PaintCtx::theme_as`) and `provide_context`s a clone under
    /// the root owner (so app code reads it via `use_context::<Theme>()`).
    theme: Theme,
    /// Whether the initial theme has been pushed to the render root / context
    /// yet — seeded once in the first `resumed`, before the first rebuild.
    theme_seeded: bool,
    /// The brightness last reported through
    /// [`DesktopExtensions::on_theme_brightness_changed`], `None` before the
    /// first report. The change gate behind that hook's "fires only when the
    /// resolved brightness actually changes" contract — see
    /// [`brightness_change_to_notify`] and [`ShellHandler::apply_theme`].
    brightness_notified: Option<Brightness>,
    /// Polls the process-wide app-facing theme override slot
    /// (`frust::set_app_theme`/`clear_app_theme`) once per
    /// frame, before rebuild in `RedrawRequested` — see
    /// `frust_shell_common::theme_override`'s module docs.
    theme_override: ThemeOverrideWatcher,
    /// Whether an app-forced theme override is currently active. While `true`,
    /// `WindowEvent::ThemeChanged` must not flip `self.theme`'s brightness —
    /// the override wins entirely until `clear_app_theme` runs (see
    /// `effective_brightness_for_platform_change`).
    theme_override_active: bool,
    /// Polls the process-wide app-facing pending-font registry
    /// (`frust::register_app_fonts`) once per frame, before rebuild in
    /// `RedrawRequested` (beside `theme_override`) — draining any late
    /// registration into `text_ctx`. Also drained once at construction time (in
    /// `run_desktop`, before the first frame). See
    /// `frust_shell_common::font_registry`'s module docs.
    font_registry: FontRegistryWatcher,
    /// Animation pacing (frame-gate pacing): the desktop shell has no skip gate
    /// (it is dirty-driven under `ControlFlow::Wait`), so it paces a paced-only
    /// decorative loop by scheduling a *delayed* redraw instead of the immediate
    /// `request_redraw` a `needs_frame` normally triggers. When a paint returns
    /// `needs_frame_paced_only` and nothing else needs the next frame, this
    /// holds the `Instant` the next paced redraw is due; `about_to_wait` parks
    /// the loop on `ControlFlow::WaitUntil(that)` and fires the redraw when it
    /// elapses. `None` when no paced redraw is pending.
    paced_wake: Option<Instant>,
    /// Whether animation pacing is enabled (the [`FRUST_NO_ANIM_PACING`] kill
    /// switch, resolved once at construction). When `false` a paced-only frame
    /// falls back to the immediate `request_redraw` every-vsync path.
    ///
    /// [`FRUST_NO_ANIM_PACING`]: frust_shell_common::frame_gate::NO_ANIM_PACING_VAR
    anim_pacing: bool,
    /// Change detector for the app-facing [`WindowMetrics`](frust_core::WindowMetrics)
    /// context: seeded in `resumed` (before the first frame) and re-polled on
    /// `WindowEvent::Resized` — never per frame in `RedrawRequested`, which only
    /// *reads* `inner_size()`/`scale_factor()`. See
    /// [`ShellHandler::push_window_metrics`] for why the guard is load-bearing.
    ///
    /// Desktop is the shell that had **no** insets arm at all: winit 0.30
    /// exposes no cross-platform safe-area/inset accessor (its only `safe_area`
    /// support is internal to the iOS backend), so the metrics published here
    /// carry [`WindowInsets::default`] — zero occlusion, which is the truth for
    /// a decorated desktop window. Mobile's own standalone `WindowInsets`
    /// context is unaffected.
    window_metrics: WindowMetricsPublisher,
}

impl<State, Logic, V, E> ShellHandler<State, Logic, V, E>
where
    State: 'static,
    V: View<State>,
    Logic: FnMut(&mut State) -> V + 'static,
    E: DesktopExtensions,
{
    /// Deliver one input event to the tree and schedule a frame if it dirtied
    /// state. This is the dirty-driven half of the desktop model: the
    /// event pass never repaints, it only sets `needs_redraw`, which we turn into
    /// a single `request_redraw()` so the `Wait` loop wakes for exactly one frame.
    ///
    /// Also re-syncs the platform IME ([`ShellHandler::sync_ime`]) after every
    /// dispatch: a focus change, blur, or caret move can all happen as
    /// a side effect of any event, not just keyboard/IME ones.
    ///
    /// The event pass itself runs under the reactive root [`Owner`] — see
    /// [`event_under_owner`], which is what makes `use_context` work from a
    /// press handler.
    ///
    /// [`Owner`]: frust_reactive::Owner
    fn dispatch(&mut self, window: &Window, event: InputEvent) {
        let outcome = event_under_owner(self.runtime, &mut self.root, &mut self.state, &event);
        self.sync_ime(window);
        self.sync_cursor(window);
        if outcome.needs_redraw {
            window.request_redraw();
        }
    }

    /// Push the cursor shape [`RenderRoot::cursor`] resolved to winit, but only
    /// when it differs from the last one pushed.
    ///
    /// Called from [`ShellHandler::dispatch`] beside
    /// [`sync_ime`](ShellHandler::sync_ime), and for the same reason: the shape can
    /// move as a side effect of any event, not just a pointer one, since a handler
    /// on any pass can change what the *next* `Move` resolves. It is deliberately
    /// **not** tied to a frame — a cursor is a window property, not something
    /// painted, so a hover that changes nothing but the shape costs one platform
    /// call and no repaint.
    ///
    /// The change gate ([`cursor_change_to_apply`]) is load-bearing rather than
    /// cosmetic: the resolved cursor re-resolves on every pointer `Move`, so an
    /// unguarded push would call into the platform on every single mouse motion.
    fn sync_cursor(&mut self, window: &Window) {
        let resolved = self.root.cursor();
        if let Some(winit_icon) = cursor_change_to_apply(self.cursor_icon, resolved) {
            window.set_cursor(winit_icon);
            self.cursor_icon = resolved;
        }
    }

    /// Query [`RenderRoot::ime_state`] and push any *changed* IME-relevant
    /// state to winit: `set_ime_allowed` on an active-transition,
    /// `set_ime_cursor_area` (logical caret rect) when active and the caret
    /// moved, and `set_ime_purpose` when [`ImeState::content_type`] changes
    /// (see [`ime_purpose_for`] for exactly how little that last call reaches
    /// — pinned winit honours it on Wayland only, and even there it is a
    /// cosmetic hint, not secure entry). All three calls are gated behind
    /// [`ImeSync`]'s cache so a steady-state focused field with an unmoving
    /// caret and unchanged content type doesn't re-issue them every event.
    ///
    /// [`ImeState::content_type`]: frust_core::event::ImeState::content_type
    fn sync_ime(&mut self, window: &Window) {
        let ime_state = self.root.ime_state();
        let active = ime_state.as_ref().is_some_and(|s| s.active);
        let purpose = ime_state
            .as_ref()
            .map_or(ImePurpose::Normal, |s| ime_purpose_for(s.content_type));

        if active != self.ime_sync.allowed {
            window.set_ime_allowed(active);
            self.ime_sync.allowed = active;
            if !active {
                self.ime_sync.cursor_area = None;
            }
        }

        if purpose != self.ime_sync.purpose {
            window.set_ime_purpose(purpose);
            self.ime_sync.purpose = purpose;
        }

        if active && let Some(caret) = ime_state.and_then(|s| s.caret) {
            let area = (
                LogicalPosition::new(caret.x0, caret.y0),
                LogicalSize::new(caret.width(), caret.height()),
            );
            if self.ime_sync.cursor_area != Some(area) {
                window.set_ime_cursor_area(area.0, area.1);
                self.ime_sync.cursor_area = Some(area);
            }
        }
    }

    /// Push the current [`Theme`] to both delivery paths and schedule a repaint.
    ///
    /// 1. `RenderRoot::set_theme` boxes a clone so widgets recover it during
    ///    layout/paint via `PaintCtx::theme_as` (the type-erased widget path).
    /// 2. `provide_context` under the reactive root owner re-provides a clone so
    ///    a `Component::build`'s `use_context::<Theme>()` resolves it on the next
    ///    rebuild (the app-code path). Re-providing under the same owner replaces
    ///    the previous value, so a live dark-mode flip is observed next frame.
    /// 3. [`DesktopExtensions::on_theme_brightness_changed`] fires when the
    ///    resolved brightness actually moved, gated by
    ///    [`brightness_change_to_notify`] — an override swapping one dark theme
    ///    for another moves the theme without moving the brightness.
    ///
    /// Called once to seed the theme in `resumed` and again on every
    /// `WindowEvent::ThemeChanged` and every per-frame app-override poll that
    /// reports a change — i.e. every point at which the shell's resolved
    /// brightness can move, which is why the hook fires from this single funnel
    /// rather than from three separately-remembered call sites.
    fn apply_theme(&mut self, window: &Window) {
        self.root.set_theme(Box::new(self.theme.clone()));
        let theme = self.theme.clone();
        self.runtime.with_owner(move || provide_context(theme));
        if let Some(brightness) =
            brightness_change_to_notify(self.brightness_notified, self.theme.brightness)
        {
            self.brightness_notified = Some(brightness);
            self.extensions.on_theme_brightness_changed(brightness);
        }
        window.request_redraw();
    }

    /// `provide_context` the window's [`WindowMetrics`](frust_core::WindowMetrics)
    /// under the reactive root owner — **only when it actually changed**
    /// ([`WindowMetricsPublisher::poll`] returns `None` otherwise) — so a
    /// `Component::build`'s `use_context::<WindowMetrics>()` resolves the current
    /// window shape on the next rebuild. The app-code delivery path
    /// [`ShellHandler::apply_theme`] already uses for [`Theme`], with the
    /// change guard added.
    ///
    /// **The guard is load-bearing, not an optimization.** `provide_context` is a
    /// plain map insert that notifies nothing on its own, but re-providing once
    /// per frame from `RedrawRequested` would still pay a lock write plus an
    /// allocation for no observable benefit — the value only ever becomes
    /// visible on the next rebuild, which a genuine input change already
    /// drives. So this is called only where an input genuinely moves: `resumed`
    /// (the seed, before the first frame) and `WindowEvent::Resized`. A
    /// scale-factor change needs no separate arm — winit guarantees a
    /// following `Resized`.
    ///
    /// `physical` is winit's device-pixel `inner_size`, divided by `scale` into
    /// the same logical space the layout pass below uses, so the published size
    /// is logical exactly as on Android/iOS. Insets are
    /// [`WindowInsets::default`] (see the `window_metrics` field doc).
    ///
    /// Unlike `apply_theme` this requests **no** redraw of its own: both call
    /// sites already do (`resumed`'s theme seed, `Resized`'s own
    /// `request_redraw`), and a metrics publish that forced a frame on its own
    /// would defeat the guard's purpose.
    fn push_window_metrics(&mut self, physical: (u32, u32), scale: f64) {
        let Some(metrics) = self
            .window_metrics
            .poll(physical, scale, WindowInsets::default())
        else {
            return; // unchanged — no re-provide, no app-wide rebuild
        };
        self.runtime.with_owner(move || provide_context(metrics));
    }

    /// Handle one `accesskit_winit` adapter event delivered through the
    /// event-loop proxy.
    ///
    /// * `InitialTreeRequested` — the adapter activated (an AT client
    ///   connected) and has no tree yet: pull one fresh
    ///   [`RenderRoot::semantics`] pass and push it, recording the generation so
    ///   the next `RedrawRequested` dirty-check doesn't immediately re-push the
    ///   same tree.
    /// * `ActionRequested` — forward straight to
    ///   [`RenderRoot::perform_accessibility_action`] (the same synthesized
    ///   pointer-event path a real tap/click would take — see
    ///   `docs/ARCHITECTURE.md`'s Semantics pass); request a redraw if it
    ///   mutated state.
    /// * `AccessibilityDeactivated` — the AT client detached; nothing to clean
    ///   up (`Adapter::update_if_active` already becomes a no-op on its own).
    fn handle_accessibility_event(&mut self, event: AccessibilityEvent) {
        let Some(adapter) = self.adapter.as_mut() else {
            return;
        };
        match event.window_event {
            AccessibilityWindowEvent::InitialTreeRequested => {
                let update = self.root.semantics();
                self.semantics_seen = self.root.semantics_generation();
                adapter.update_if_active(|| build_tree_update(&update));
            }
            AccessibilityWindowEvent::ActionRequested(request) => {
                // Same reactive-owner wrap as `dispatch`: the action is routed
                // through synthesized pointer events, so it lands in the very
                // same `Widget::event` handlers a real click would and needs the
                // ambient `Owner` just as much (see [`event_under_owner`]).
                let runtime = self.runtime;
                let outcome = runtime.with_owner(|| {
                    self.root.perform_accessibility_action(
                        &mut self.state,
                        request.target_node,
                        request.action,
                    )
                });
                if outcome.needs_redraw
                    && let Some(window) = self.window.as_ref()
                {
                    window.request_redraw();
                }
            }
            AccessibilityWindowEvent::AccessibilityDeactivated => {}
        }
    }
}

/// Deliver one input event to the widget tree **under the reactive runtime's
/// root [`Owner`]**, so context lookups work from inside an event handler.
///
/// # Why the wrap is needed
///
/// `reactive_graph`'s `use_context` resolves by walking up from
/// `Owner::current()`, and `Owner::with` *restores* the previous owner when it
/// returns — so outside the per-frame rebuild wrap there is no current owner at
/// all. An unwrapped event pass therefore gives every handler
/// `use_context::<T>() == None`, even for a context the shell itself provided
/// (`Theme`, `WindowMetrics`) — the whole app looks unthemed from a press
/// handler. See the `event_pass_*` tests below.
///
/// # Owner identity: the root owner, not a fresh child
///
/// The pass shares the rebuild's owner rather than opening a child scope. A
/// child scope would have to be created (and disposed) per input event —
/// including per pointer `Move` — which the "no per-event allocation" budget
/// rules out, and a handler's `provide_context` would silently evaporate on
/// dispose instead of being visible to the next rebuild. The cost of sharing is
/// that a handler calling `provide_context` writes into the app-wide root owner;
/// that matches what a root `Component::build` already does.
///
/// # No `TrackedScope`: deliberately untracked
///
/// The rebuild wrap pairs `with_owner` with `TrackedScope::track`; this one must
/// **not**. `track` clears the scope's recorded sources and its dirty flag on
/// entry, so tracking an event pass would (a) throw away the dependency set the
/// last rebuild recorded, silently unsubscribing the frame loop from every
/// signal the view reads, and (b) swallow a wake that arrived since that
/// rebuild. A handler that *writes* a signal still wakes the shell exactly as
/// before — the write notifies the rebuild scope, which is subscribed from its
/// own `track` pass.
///
/// [`Owner`]: frust_reactive::Owner
fn event_under_owner<State: 'static, V: View<State>>(
    runtime: &ReactiveRuntime,
    root: &mut RenderRoot<State, V>,
    state: &mut State,
    event: &InputEvent,
) -> EventOutcome {
    runtime.with_owner(|| root.event(state, event))
}

/// Build an accesskit [`TreeUpdate`] from one [`RenderRoot::semantics`] pull:
/// a full-tree push every time (stable node ids make this valid
/// — see `frust_core::semantics`'s module docs), rooted with
/// [`TreeId::ROOT`] and the update's already-root-defaulted focus id
/// ([`SemanticsUpdate::focus_id`]). Pure and unit-testable without a live
/// window/adapter.
fn build_tree_update(update: &SemanticsUpdate) -> TreeUpdate {
    TreeUpdate {
        nodes: update.nodes.clone(),
        tree: Some(Tree::new(update.root)),
        tree_id: TreeId::ROOT,
        focus: update.focus_id(),
    }
}

/// The devtools UI-thread view of this shell (see
/// `frust_shell_common::devtools`): read the retained tree, and deliver a
/// synthetic event through the shell's ordinary [`ShellHandler::dispatch`] —
/// the same helper every winit event goes through, so an injected tap is
/// hit-tested, IME-synced and redraw-scheduled exactly like a real one.
#[cfg(feature = "devtools")]
impl<State, Logic, V, E> frust_shell_common::devtools::DevtoolsUi
    for ShellHandler<State, Logic, V, E>
where
    State: 'static,
    V: View<State>,
    Logic: FnMut(&mut State) -> V + 'static,
    E: DesktopExtensions,
{
    fn inspect(&self) -> Vec<frust_core::InspectNode> {
        self.root.inspect()
    }

    fn dispatch(&mut self, event: InputEvent) {
        // No window yet (an injection racing startup): dropping the event is
        // the honest answer — there is nothing laid out to hit-test against,
        // and `dispatch` needs the window for the IME re-sync and the redraw.
        if let Some(window) = self.window.clone() {
            ShellHandler::dispatch(self, &window, event);
        }
    }
}

impl<State, Logic, V, E> ApplicationHandler<ShellUserEvent> for ShellHandler<State, Logic, V, E>
where
    State: 'static,
    V: View<State>,
    Logic: FnMut(&mut State) -> V + 'static,
    E: DesktopExtensions,
{
    /// A tracked-signal write from any thread routes here via the frame waker →
    /// [`winit::event_loop::EventLoopProxy::send_event`]. Pump the UI-thread
    /// local task queue first (a completing local task may have driven the
    /// write), then request a redraw so the next frame re-runs `app_logic` and
    /// re-tracks. The waker can fire before the window exists (an early
    /// background spawn), so a redraw is only requested when a window is present
    /// — the first rebuild after `resumed` re-tracks regardless.
    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: ShellUserEvent) {
        self.runtime.pump_local();
        match event {
            // A signal write, or a render-thread redraw request (stale-swapchain
            // reconfigure / SurfaceLost recovery in the split path) — both mean
            // "repaint" and both go through the proxy so the `Wait` loop stays
            // dirty-driven (idle CPU near zero).
            ShellUserEvent::SignalsDirty | ShellUserEvent::RenderNeedsRedraw => {
                if let Some(window) = self.window.as_ref() {
                    window.request_redraw();
                }
            }
            ShellUserEvent::Accessibility(event) => self.handle_accessibility_event(event),
            // The render thread lost its surface (split path): re-create it here
            // on the main thread (where winit yields the window handle), hand a
            // fresh surface across, and repaint. Rare path — correctness over
            // elegance.
            ShellUserEvent::RenderRecreateSurface => {
                if let Some(window) = self.window.clone() {
                    let size = window.inner_size();
                    self.executor.recreate_surface(
                        &window,
                        SurfaceSize {
                            width: size.width.max(1),
                            height: size.height.max(1),
                            scale: window.scale_factor(),
                        },
                    );
                    window.request_redraw();
                }
            }
            // The render thread hit a fatal init error: stash it and stop the
            // loop, mirroring the inline path's `resumed` fatal handling.
            ShellUserEvent::RenderFatal(err) => {
                self.fatal = Some(err);
                event_loop.exit();
            }
            // The devtools UI-thread hop: answer every queued request here, on
            // the thread that owns the tree. Injected events go through
            // `DevtoolsUi::dispatch` → this shell's own `dispatch`, i.e. the
            // identical path a real winit event takes.
            #[cfg(feature = "devtools")]
            ShellUserEvent::Devtools => frust_shell_common::devtools::pump(self),
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // The window is created once and reused; the surface, by contrast, is
        // (re)created here every time we resume — `suspended()` tears it down,
        // mirroring Android's surfaceDestroyed/surfaceCreated so the §8.1
        // machine stays exercised on desktop too.
        if self.window.is_none() {
            // Title/size/hidden from the app's `DesktopConfig`, then through the
            // extension's pre-create hook (`window_attributes`) — the only
            // chance a per-OS shell gets at attributes winit refuses to change
            // after creation (Wayland `app_id`, X11 icon).
            let attrs = window_attributes(&self.config, &mut self.extensions);
            match event_loop.create_window(attrs) {
                Ok(window) => {
                    self.adapter = Some(Adapter::with_event_loop_proxy(
                        event_loop,
                        &window,
                        self.accesskit_proxy.clone(),
                    ));
                    // Post-create, pre-show: an extension attaches its native
                    // menu to the live handle and retains the window here (the
                    // one hook that receives it — see `crate::extensions`).
                    // Before `set_visible` so a menu bar is in place the first
                    // time the window is drawn, and after the adapter for the
                    // same reason the window is created hidden at all: the
                    // accesskit adapter must exist before the window is ever
                    // shown (`Adapter::with_event_loop_proxy`'s contract).
                    let window = Arc::new(window);
                    self.extensions.on_window_created(&window);
                    window.set_visible(true);
                    self.window = Some(window);
                }
                Err(err) => {
                    self.fatal =
                        Some(anyhow::Error::from(err).context("frust: failed to create window"));
                    event_loop.exit();
                    return;
                }
            }
        }

        let window = self
            .window
            .clone()
            .expect("window was just created or already present");

        // Bring the surface online if it is not already (a redundant `resumed`
        // is a no-op). Where the renderer lives (UI thread inline, render thread
        // in the split) decides where the pipeline-cache load/persist and the
        // adapter/device/renderer startup spans are recorded — the executor owns
        // that. Inline surface-creation failure is fatal here; a split failure is
        // reported back through the proxy (`RenderFatal`).
        if !self.executor.has_surface() {
            let size = window.inner_size();
            let surface_size = SurfaceSize {
                width: size.width.max(1),
                height: size.height.max(1),
                scale: window.scale_factor(),
            };
            if let Err(err) = self.executor.ensure_surface(&window, surface_size) {
                self.fatal = Some(err);
                event_loop.exit();
                return;
            }
        }

        // Seed the theme once, before the first rebuild: read the window's
        // reported light/dark preference into the M3 baseline, then push it to
        // the render root and the reactive context (`apply_theme` also requests
        // the redraw that drives the first frame). Live changes arrive later via
        // `WindowEvent::ThemeChanged`.
        if !self.theme_seeded {
            self.theme.brightness = brightness_from_winit(window.theme());
            self.theme_seeded = true;
            self.apply_theme(&window);
        } else {
            window.request_redraw();
        }

        // Seed the app-facing window-shape context beside the theme, before the
        // first frame the redraw above drives, so a `Component::build` calling
        // `use_context::<WindowMetrics>()` in the very first rebuild resolves a
        // real value rather than `None`. Self-guarded, so a redundant `resumed`
        // (or one after a suspend/resume at unchanged dimensions) publishes
        // nothing. Live changes arrive via `WindowEvent::Resized`.
        let physical = window.inner_size();
        self.push_window_metrics((physical.width, physical.height), window.scale_factor());
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        // Drop the surface on suspend: rare on macOS, but keeps the
        // NoSurface path exercised on desktop and matches Android's lifecycle. In
        // the split path this is a barriered `SurfaceDestroyed` — the shell
        // blocks until the render thread has released its surface resources.
        self.executor.destroy_surface();
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Drain any UI-thread local tasks that became runnable while dispatching
        // this batch of events, before the loop parks on `Wait`. Cheap no-op
        // when the queue is empty; a task that writes a signal here re-dirties a
        // scope and fires the waker, which re-arms the loop rather than parking.
        self.runtime.pump_local();

        // Animation pacing (frame-gate pacing): a paced-only decorative loop
        // scheduled its follow-up redraw for a future instant rather than the
        // next vsync. Park the loop on `WaitUntil(deadline)` so it wakes then;
        // once the deadline passes, request the redraw and revert to `Wait`.
        // With nothing pending, defensively return to `Wait` too, so no stale
        // `WaitUntil` can ever survive a turn (the busy-spin bug).
        // Any other redraw source (input, a signal wake, a resize) still wakes
        // the loop immediately regardless of this timer — pacing only bounds the
        // cosmetic loop's own cadence. The decision itself lives in the pure,
        // winit-free `paced_wake` module (see its docs) — this applies its
        // result wholesale (field, redraw, control flow are one value, so the
        // field can never be updated without also deciding the control flow).
        let decision = paced_wake_action(self.paced_wake, Instant::now());
        self.paced_wake = decision.paced_wake;
        if decision.request_redraw
            && let Some(window) = self.window.as_ref()
        {
            window.request_redraw();
        }
        apply_control_flow(event_loop, decision.control_flow);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(window) = self.window.clone() else {
            return;
        };

        // Every `WindowEvent` must reach the accesskit adapter (its documented
        // `process_event` contract): it derives root-window
        // bounds/focus state from `Moved`/`Resized`/`Focused` regardless of
        // what the match below does with the same event.
        if let Some(adapter) = self.adapter.as_mut() {
            adapter.process_event(&window, &event);
        }

        match event {
            // The extension decides whether a close request ends the app: the
            // default (and the whole zero-config preview) is the unconditional
            // exit this arm always was, while a macOS shell may hide its
            // retained window instead and keep the loop alive. The
            // pipeline-cache persistence path is unaffected either way — it
            // hangs off the frame executor's drop once `run_app` returns, not
            // off this event.
            WindowEvent::CloseRequested => match self.extensions.on_close_requested() {
                CloseAction::Exit => event_loop.exit(),
                CloseAction::KeepRunning => {}
            },

            WindowEvent::Resized(size) => {
                // A resize with no live surface is dropped by the machine
                // (inline) or by the render thread's phase gate (split).
                self.executor.resize_surface(SurfaceSize {
                    width: size.width,
                    height: size.height,
                    scale: window.scale_factor(),
                });
                // Republish the app-facing window shape (new logical size, and
                // the derived portrait/landscape orientation with it). Reads the
                // event's own `size` rather than `inner_size()` — this IS the
                // authoritative new size, and it is also what winit delivers
                // after a `ScaleFactorChanged`, which is why that needs no arm
                // of its own. Self-guarded, so a resize that reports unchanged
                // dimensions publishes nothing.
                self.push_window_metrics((size.width, size.height), window.scale_factor());
                window.request_redraw();
            }

            // Live dark-mode toggle: flip the theme's brightness and re-push it
            // to both delivery paths (`apply_theme` requests the repaint). The
            // next paint resolves the dark scheme through `PaintCtx::theme_as`
            // and the next rebuild sees the new `use_context::<Theme>()` value.
            //
            // Override-wins rule: while an app-forced theme
            // override is active, this platform change must not flip
            // brightness — `effective_brightness_for_platform_change` passes
            // the current brightness through unchanged in that case. A seeded
            // *default* is deliberately not pinned that way: `self.theme` still
            // IS that base (`base_theme` seeded it and nothing replaced it), so
            // flipping its brightness in place re-derives light/dark from the
            // design system's own theme.
            WindowEvent::ThemeChanged(winit_theme) => {
                follow_platform_brightness(
                    &mut self.theme,
                    self.theme_override_active,
                    brightness_from_winit(Some(winit_theme)),
                );
                self.apply_theme(&window);
            }

            // Track the cursor in logical space and dispatch a `Move`. We
            // dispatch on every move (not just while a button is down) so hover
            // handling can land later without a shell change; a widget that only
            // cares about drags simply ignores moves with no capture.
            WindowEvent::CursorMoved { position, .. } => {
                let scale = window.scale_factor();
                self.cursor = physical_to_logical(position.x, position.y, scale);
                self.dispatch(
                    &window,
                    InputEvent::Pointer(PointerEvent {
                        phase: PointerPhase::Move,
                        position: self.cursor,
                        button: PointerButton::Primary,
                    }),
                );
            }

            // Primary (left) and secondary (right) buttons; Middle/Back/Forward
            // are ignored until those gestures are specced (`map_mouse_button`).
            // The press/release position is the last `CursorMoved` position
            // (winit carries none on the event). A Secondary press that arrives
            // while a gesture is captured is dropped, and its release follows
            // whatever happened to its press — see
            // `mouse_button_should_dispatch` for both invariants.
            WindowEvent::MouseInput { state, button, .. } => {
                let phase = match state {
                    ElementState::Pressed => PointerPhase::Down,
                    ElementState::Released => PointerPhase::Up,
                };
                if let Some(mapped_button) = map_mouse_button(button)
                    && mouse_button_should_dispatch(
                        mapped_button,
                        phase,
                        self.root.is_pointer_captured(),
                        &mut self.secondary_down_delivered,
                    )
                {
                    self.dispatch(
                        &window,
                        InputEvent::Pointer(PointerEvent {
                            phase,
                            position: self.cursor,
                            button: mapped_button,
                        }),
                    );
                }
            }

            // Both the unit and the sign conversion live in `map_scroll_delta`.
            WindowEvent::MouseWheel { delta, .. } => {
                let scroll = map_scroll_delta(delta, window.scale_factor());
                self.dispatch(
                    &window,
                    InputEvent::Scroll {
                        position: self.cursor,
                        delta: scroll,
                    },
                );
            }

            // winit delivers `ModifiersChanged` *before* the `KeyboardInput`
            // that relies on it, so tracking it here keeps `self.modifiers`
            // current by the time a key event is mapped.
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = map_modifiers(modifiers.state());
            }

            // Not repeat-filtered — `repeat` passes through to the framework
            // event so a widget can decide whether to honor
            // auto-repeat (e.g. held Backspace). Dropped entirely (`None`) on
            // key-up, an unmapped named key, or a composing-suppressed
            // character (see `map_key_event`'s dedupe rule).
            WindowEvent::KeyboardInput { event, .. } => {
                if let Some(key_event) = map_key_event(
                    &event.logical_key,
                    event.text.as_deref(),
                    event.state,
                    event.repeat,
                    self.modifiers,
                    self.compose.is_composing(),
                ) {
                    self.dispatch(&window, InputEvent::Key(key_event));
                }
            }

            // The compose latch both maps the winit variant and updates its own
            // dedupe state in one pass (`ComposeLatch::observe`).
            WindowEvent::Ime(ime) => {
                let mapped = self.compose.observe(&ime);
                self.dispatch(&window, InputEvent::Ime(mapped));
            }

            WindowEvent::RedrawRequested => {
                // Drain any UI-thread local tasks queued since the last turn
                // before rebuilding, so their signal writes are visible to this
                // frame. Cheap no-op when the queue is empty.
                self.runtime.pump_local();

                // Drain the per-OS extension's own once-per-frame queue beside
                // the polls below — a native menu's activations, pushed into
                // `frust_reactive::push_menu_event`. Top of frame, so an
                // activation drained here is visible to *this* frame's rebuild
                // rather than the next one; a no-op for `NoExtensions`.
                self.extensions.pump();

                // Poll the app-facing theme override slot once
                // per frame, before rebuild — mirrors the mobile shells'
                // frame-callback poll. `Some(Some(theme))` is a new forced
                // theme; `Some(None)` is a `clear_app_theme` reverting to the
                // platform-derived default; `None` means nothing changed.
                //
                // Reverting an override lands on the base this shell seeded
                // itself from — the design-system default when one was
                // supplied, else the built-in fallback — with brightness
                // re-derived from the window's current preference rather than
                // inherited from the cleared override; that whole ladder lives
                // in `theme_after_override_poll` so this arm and its unit tests
                // share one implementation. Both inputs stay lazy: an
                // unchanged poll pays neither the default-slot read nor the
                // window appearance query.
                if let Some((theme, override_active)) =
                    theme_after_override_poll(self.theme_override.poll(), default_theme, || {
                        brightness_from_winit(window.theme())
                    })
                {
                    self.theme = theme;
                    self.theme_override_active = override_active;
                    self.apply_theme(&window);
                }

                // Poll the app-facing pending-font registry once per
                // frame, beside the theme poll above. `drain_into` applies any
                // late-registered fonts to `text_ctx` (clearing the shape cache
                // internally) and returns whether anything registered. On a
                // late drain, force the relayout `register_fonts` documents by
                // re-pushing the currently-active theme through `set_theme`
                // (the same LAYOUT|PAINT contract a theme swap uses — no new
                // core API), so text laid out before the drain re-shapes against
                // the new faces this frame. When nothing is pending this is one
                // cheap `Mutex` check returning an empty `Vec` (no allocation).
                if self.font_registry.drain_into(&mut self.text_ctx) {
                    self.root.set_theme(Box::new(self.theme.clone()));
                }

                // Rebuild the view tree every frame (app_logic is cheap by
                // construction). A real dirty-tracking loop would skip
                // this when state is unchanged; the on-demand `Wait` control
                // flow already keeps us from free-running.
                //
                // The rebuild runs under the reactive runtime's root `Owner`
                // (so signals created during it are root-owned) and inside the
                // `TrackedScope` (so every signal read subscribes this frame —
                // a later write dirties the scope and wakes the loop). Fields
                // are borrowed disjointly so the tracking closure captures only
                // what the rebuild needs, not all of `self`.
                let rebuild_start = Instant::now();
                let runtime = self.runtime;
                let scope = &self.scope;
                let root = &mut self.root;
                let app_logic = &mut self.app_logic;
                let state = &mut self.state;
                let _flags = runtime.with_owner(|| scope.track(|| root.rebuild(app_logic, state)));
                let rebuild_dur = rebuild_start.elapsed();
                // Perf instrumentation: the app's first-ever rebuild,
                // recorded once. Inline records it here; in the split path the
                // render thread owns the startup line, so this is a no-op there
                // (it records the milestone when the first scene arrives).
                self.executor.record_first_rebuild();
                // A rebuild can change which widget is focused / what it
                // publishes without an intervening event (e.g. state-driven
                // focus), so re-sync the platform IME here too.
                self.sync_ime(&window);

                // HiDPI: lay out in logical pixels, then scale
                // the whole scene by the device pixel ratio so glyph outlines
                // are re-rasterised sharp at physical resolution.
                let physical = window.inner_size();
                let scale = window.scale_factor();
                let logical = Size::new(
                    physical.width as f64 / scale,
                    physical.height as f64 / scale,
                );
                let text_ctx: &mut dyn Any = &mut self.text_ctx;
                let layout_start = Instant::now();
                self.root.layout_with_text(logical, text_ctx);
                let layout_dur = layout_start.elapsed();

                // Push a fresh semantics tree to the accesskit adapter if it may
                // have changed since the last push (the dirty gate —
                // `RenderRoot::semantics_if_changed`); post-layout so bounds are
                // valid. A no-op (`update_if_active` never runs the closure)
                // while no AT client has activated the adapter.
                if let Some(adapter) = self.adapter.as_mut()
                    && let Some(update) = self.root.semantics_if_changed(self.semantics_seen)
                {
                    self.semantics_seen = self.root.semantics_generation();
                    adapter.update_if_active(|| build_tree_update(&update));
                }

                self.scene.reset();
                // Sample the shell-owned monotonic clock once per frame and hand
                // it to paint; every animating widget differences it against its
                // own stored time (the mobile shells swap this single expression for a
                // platform vsync timestamp instead).
                let frame_time = FrameTime::from_nanos(self.epoch.elapsed().as_nanos() as u64);
                // Push the render side's presented-frame count so a widget
                // measuring FPS reports the presented rate, not its paint cadence.
                // A pure observation — `set_presented_frames` dirties
                // nothing, so it neither forces a relayout nor (as the setter's
                // contract notes) would ever feed a frame gate.
                self.root
                    .set_presented_frames(self.executor.presented_frames());
                let paint_start = Instant::now();
                let paint_outcome = {
                    let mut builder = SceneBuilder::new(&mut self.scene);
                    builder.push_transform(Affine::scale(scale));
                    let outcome = self.root.paint(&mut builder, frame_time);
                    builder.pop_transform();
                    outcome
                };
                let paint_dur = paint_start.elapsed();

                // Animation driver (spec v1 seam): if paint advanced animation
                // state (e.g. a scroll fling) it asks for another frame here.
                // `ControlFlow::Wait` would otherwise idle with no pending input,
                // so we keep frames coming with an explicit redraw request until
                // the animation reaches rest and stops signalling.
                //
                // Animation pacing (frame-gate pacing): when the request is a
                // paced-only decorative loop (`needs_frame_paced_only` — a
                // shimmer/pulse/spinner with no concurrent transition), schedule
                // the follow-up redraw a `cosmetic_loop_rate` interval out rather
                // than every vsync. The desktop shell has no skip gate, so it
                // paces via a delayed wake (`about_to_wait` parks on
                // `WaitUntil`). A `Transition` request (or the pacing kill
                // switch) keeps the immediate every-frame path. `needs_frame ==
                // false` (a settled loop) must clear any stale prior
                // `paced_wake` too, not just leave it assigned only inside a
                // `needs_frame` arm — `next_paced_wake` is driven every frame
                // (not just the `needs_frame` ones) precisely so its settle
                // path covers that case: it clears the field AND returns the
                // loop to `Wait`, never leaving a stale `WaitUntil` behind (the
                // busy-spin bug). The cap/fold arithmetic is
                // computed only inside the branch that schedules (in
                // `next_paced_wake`), so a settling frame never runs it; the
                // rate is kept finite by `CosmeticLoopRate`'s NaN-safe clamp so
                // that division can't panic. The decision bundles the field,
                // the redraw, and the control flow into one value, so the field
                // can never be updated without also deciding the control flow.
                //
                // Per-request pacing interval (A2): `paint_outcome.paced_interval`
                // is this same paint's MIN-aggregated per-request interval (a
                // ~500ms caret blink against a 30Hz shimmer cap) — fed straight
                // into `next_paced_wake` exactly as `needs_frame_paced_only` is
                // above, never stored in a struct field. Unlike the mobile
                // shells (whose gate decides *before* paint runs, so they must
                // latch the previous paint's interval into a field for the next
                // tick's decision — see `frust-shell-android`'s
                // `last_paced_interval`), desktop's decision runs synchronously
                // right after `RenderRoot::paint` returns, so this frame's own
                // outcome is always the freshest possible input: no persisted
                // value can ever go stale, and an interval change between
                // paints (a loop's cadence request changing) is re-derived from
                // scratch on every call rather than carried forward.
                // `next_paced_wake` folds it against the theme's cap with the
                // same `max(cap, requested)` semantics as
                // `FramePacing::effective_interval` — the cap is a ceiling, a
                // slower per-request interval widens the cadence, never
                // tightens it.
                let decision = next_paced_wake(
                    paint_outcome.needs_frame,
                    paint_outcome.needs_frame_paced_only,
                    self.anim_pacing,
                    Instant::now(),
                    self.theme.motion.cosmetic_loop_rate.hz(),
                    paint_outcome.paced_interval,
                );
                self.paced_wake = decision.paced_wake;
                if decision.request_redraw {
                    window.request_redraw();
                }
                apply_control_flow(event_loop, decision.control_flow);

                // A tracked signal written *during* this frame (e.g. a local
                // task pumped above, or a write racing in from a background
                // thread) already re-dirtied the scope after `track` cleared it.
                // The waker's clean→dirty edge fired inside `track`, so no user
                // event will arrive for it — request the follow-up frame here,
                // mirroring the `needs_frame` animation path above.
                if self.scope.is_dirty() {
                    window.request_redraw();
                }

                // Hand the finished frame to the executor. In the single-thread
                // fallback this runs encode→acquire→submit inline (clearing to
                // the live theme's surface color); in the split
                // path it moves the scene across the depth-1 latest-wins channel
                // and the render thread runs the tail, recording its own
                // `RenderSpans` folded with these `UiSpans` via
                // `FramePasses::from_split` (the single perf emitter). Either way
                // a render failure is log-and-continue, not fatal — one bad frame
                // must not kill the app (see `fatal` for the persistent case).
                let ui_spans = UiSpans {
                    rebuild: rebuild_dur,
                    layout: layout_dur,
                    paint: paint_dur,
                    skipped: false,
                };
                let base_color = self.theme.scheme().surface;
                let size = SurfaceSize {
                    width: physical.width,
                    height: physical.height,
                    scale,
                };
                self.executor.submit_frame(
                    &window,
                    &mut self.scene,
                    base_color,
                    ui_spans,
                    frame_time,
                    size,
                );
            }

            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ComposeLatch, DesktopConfig, DesktopExtensions, ElementState, Ime, ImeSync, LogicalSize,
        MouseScrollDelta, NoExtensions, Tree, TreeId, WindowKnobSource, WinitCursorIcon, WinitKey,
        WinitNamedKey, WinitTheme, apply_window_size, base_theme, brightness_change_to_notify,
        brightness_from_winit, build_tree_update, cursor_change_to_apply, default_theme, finish,
        follow_platform_brightness, ime_purpose_for, map_key_event, map_modifiers,
        map_mouse_button, map_named_key, map_scroll_delta, mouse_button_should_dispatch,
        parse_window_maximized, parse_window_size, physical_to_logical, resolved_window_knob,
        theme_after_override_poll, window_attributes, winit_cursor_for,
    };
    use frust_core::SemanticsUpdate;
    use frust_core::accesskit::{Node, NodeId, Role};
    use frust_core::event::{
        CursorIcon, ImeContentType, ImeEvent, Key, KeyEvent, Modifiers, NamedKey, PointerButton,
        ScrollDelta,
    };
    use frust_theme::{Brightness, DesignLanguage, Theme};
    use kurbo::Point;
    use winit::dpi::PhysicalPosition;
    use winit::event::MouseButton;
    use winit::keyboard::ModifiersState;
    use winit::window::{ImePurpose, WindowAttributes};

    // --- brightness_from_winit ---

    #[test]
    fn brightness_from_winit_maps_dark_light_and_defaults() {
        assert_eq!(
            brightness_from_winit(Some(WinitTheme::Dark)),
            Brightness::Dark
        );
        assert_eq!(
            brightness_from_winit(Some(WinitTheme::Light)),
            Brightness::Light
        );
        // No reported preference falls back to Light.
        assert_eq!(brightness_from_winit(None), Brightness::Light);
    }

    // --- provide_context replacement (the app-code theme delivery path) ---

    #[test]
    fn re_providing_theme_context_lets_a_live_flip_win() {
        // The desktop shell re-`provide_context`s the theme under the root owner
        // on every `ThemeChanged`. This proves reactive_graph 0.2's replacement
        // semantics: a second provide of the same type under one owner wins for
        // subsequent `use_context` reads — so a live dark-mode flip is observed
        // (no `RwSignal<Theme>` fallback needed; see the task's note).
        use frust_reactive::{Owner, provide_context, use_context};

        let owner = Owner::new();
        let resolved = owner.with(|| {
            let mut light = Theme::neutral();
            light.brightness = Brightness::Light;
            provide_context(light);

            // A live flip: re-provide a dark theme under the same owner. Same
            // baseline, opposite brightness — the brightness is what the
            // read-back below discriminates on.
            let mut dark = Theme::neutral();
            dark.brightness = Brightness::Dark;
            provide_context(dark);

            use_context::<Theme>()
        });
        assert_eq!(resolved.map(|t| t.brightness), Some(Brightness::Dark));
    }

    #[test]
    fn physical_to_logical_divides_by_scale() {
        // A 2× HiDPI display: a physical (200, 100) cursor is logical (100, 50).
        assert_eq!(
            physical_to_logical(200.0, 100.0, 2.0),
            Point::new(100.0, 50.0)
        );
    }

    #[test]
    fn physical_to_logical_is_identity_at_unit_scale() {
        // A non-HiDPI display: physical and logical coincide.
        assert_eq!(physical_to_logical(37.0, 12.0, 1.0), Point::new(37.0, 12.0));
    }

    // --- map_mouse_button ---

    #[test]
    fn map_mouse_button_maps_left_to_primary() {
        assert_eq!(
            map_mouse_button(MouseButton::Left),
            Some(PointerButton::Primary)
        );
    }

    #[test]
    fn map_mouse_button_maps_right_to_secondary() {
        assert_eq!(
            map_mouse_button(MouseButton::Right),
            Some(PointerButton::Secondary)
        );
    }

    #[test]
    fn map_mouse_button_drops_middle_back_and_forward() {
        // `PointerButton` already models `Middle`, but nothing dispatches it
        // yet — this shell forwards only what v1 specced.
        assert_eq!(map_mouse_button(MouseButton::Middle), None);
        assert_eq!(map_mouse_button(MouseButton::Back), None);
        assert_eq!(map_mouse_button(MouseButton::Forward), None);
    }

    // --- mouse_button_should_dispatch ---

    /// One secondary press/release through the gate, against a caller-owned
    /// latch — the shape `WindowEvent::MouseInput` calls it in.
    fn secondary(phase: PointerPhase, captured: bool, latch: &mut bool) -> bool {
        mouse_button_should_dispatch(PointerButton::Secondary, phase, captured, latch)
    }

    #[test]
    fn an_uncaptured_secondary_press_and_its_release_both_dispatch() {
        let mut latch = false;
        assert!(secondary(PointerPhase::Down, false, &mut latch));
        assert!(secondary(PointerPhase::Up, false, &mut latch));
        // The latch is spent by the release it paired.
        assert!(!latch);
    }

    #[test]
    fn a_secondary_press_is_dropped_while_captured_and_so_is_its_release() {
        // A right-click mid-drag must not disturb the Primary capture it can
        // never have opened — and dropping the press means dropping the
        // release too, or the tree sees an `Up` with no `Down`.
        let mut latch = false;
        assert!(!secondary(PointerPhase::Down, true, &mut latch));
        assert!(!secondary(PointerPhase::Up, true, &mut latch));
        assert!(!secondary(PointerPhase::Up, false, &mut latch));
    }

    #[test]
    fn a_delivered_secondary_press_gets_its_release_even_if_a_capture_opened() {
        // The pairing invariant, and the wedge this gate exists to prevent: the
        // press was delivered while nothing was captured, something captured in
        // between (the widget it opened, or an unrelated one), and the release
        // must still land — otherwise the capture never closes.
        let mut latch = false;
        assert!(secondary(PointerPhase::Down, false, &mut latch));
        assert!(secondary(PointerPhase::Up, true, &mut latch));
        assert!(!latch);
    }

    #[test]
    fn a_secondary_release_without_a_delivered_press_never_dispatches() {
        // A release arriving with a cold latch (a press consumed by a native
        // menu, or one that landed before the window was focused) is not the
        // second half of anything.
        let mut latch = false;
        assert!(!secondary(PointerPhase::Up, false, &mut latch));
        assert!(!secondary(PointerPhase::Up, true, &mut latch));
    }

    #[test]
    fn a_second_secondary_press_re_arms_the_latch_rather_than_stacking() {
        // Two presses in a row (a release lost to the platform) leave the latch
        // describing the *last* press, so exactly one release is ever paired.
        let mut latch = false;
        assert!(secondary(PointerPhase::Down, false, &mut latch));
        assert!(!secondary(PointerPhase::Down, true, &mut latch));
        assert!(!secondary(PointerPhase::Up, false, &mut latch));
    }

    #[test]
    fn primary_always_dispatches_captured_or_not_and_leaves_the_latch_alone() {
        // Primary is the button a capture belongs to, so its own `Down`/`Up`
        // must never be the one this gate drops — and a Primary gesture must
        // not disturb a secondary press waiting for its release.
        let mut latch = true;
        for captured in [false, true] {
            for phase in [PointerPhase::Down, PointerPhase::Up] {
                assert!(mouse_button_should_dispatch(
                    PointerButton::Primary,
                    phase,
                    captured,
                    &mut latch
                ));
            }
        }
        assert!(latch);
    }

    // --- map_scroll_delta ---

    #[test]
    fn map_scroll_delta_negates_wheel_lines() {
        // winit's positive y moves the content down (revealing what is above);
        // frust's scrollables add the delta to an offset that grows to reveal
        // later content, so the sign flips at this boundary.
        assert_eq!(
            map_scroll_delta(MouseScrollDelta::LineDelta(0.0, 1.0), 1.0),
            ScrollDelta::Lines(0.0, -1.0)
        );
        assert_eq!(
            map_scroll_delta(MouseScrollDelta::LineDelta(2.0, -3.0), 1.0),
            ScrollDelta::Lines(-2.0, 3.0)
        );
    }

    #[test]
    fn map_scroll_delta_negates_and_scales_pixel_deltas() {
        // A 2× HiDPI display: a physical (20, 60) trackpad delta is logical
        // (10, 30), negated onto frust's convention.
        assert_eq!(
            map_scroll_delta(
                MouseScrollDelta::PixelDelta(PhysicalPosition::new(20.0, 60.0)),
                2.0
            ),
            ScrollDelta::Pixels(-10.0, -30.0)
        );
    }

    #[test]
    fn finish_propagates_fatal_error_with_context() {
        let err = anyhow::anyhow!("boom").context("frust: failed to create window");
        let result = finish(Some(err));

        let err = result.expect_err("a stashed fatal error must surface as Err");
        assert_eq!(
            err.to_string(),
            "frust: failed to create window",
            "the outermost context string must be preserved"
        );
        assert_eq!(
            err.chain().last().unwrap().to_string(),
            "boom",
            "the underlying cause must still be reachable via the error chain"
        );
    }

    #[test]
    fn finish_is_ok_on_the_happy_path() {
        assert!(finish(None).is_ok());
    }

    // --- map_named_key ---

    #[test]
    fn map_named_key_covers_every_editing_named_key() {
        assert_eq!(map_named_key(WinitNamedKey::Enter), Some(NamedKey::Enter));
        assert_eq!(
            map_named_key(WinitNamedKey::Backspace),
            Some(NamedKey::Backspace)
        );
        assert_eq!(map_named_key(WinitNamedKey::Delete), Some(NamedKey::Delete));
        assert_eq!(
            map_named_key(WinitNamedKey::ArrowLeft),
            Some(NamedKey::ArrowLeft)
        );
        assert_eq!(
            map_named_key(WinitNamedKey::ArrowRight),
            Some(NamedKey::ArrowRight)
        );
        assert_eq!(
            map_named_key(WinitNamedKey::ArrowUp),
            Some(NamedKey::ArrowUp)
        );
        assert_eq!(
            map_named_key(WinitNamedKey::ArrowDown),
            Some(NamedKey::ArrowDown)
        );
        assert_eq!(map_named_key(WinitNamedKey::Home), Some(NamedKey::Home));
        assert_eq!(map_named_key(WinitNamedKey::End), Some(NamedKey::End));
        assert_eq!(map_named_key(WinitNamedKey::Escape), Some(NamedKey::Escape));
        assert_eq!(map_named_key(WinitNamedKey::Tab), Some(NamedKey::Tab));
    }

    #[test]
    fn map_named_key_is_none_for_unmapped_named_keys() {
        // No editing semantics for these — they must not be misreported.
        assert_eq!(map_named_key(WinitNamedKey::F1), None);
        assert_eq!(map_named_key(WinitNamedKey::Alt), None);
        assert_eq!(map_named_key(WinitNamedKey::CapsLock), None);
    }

    // --- map_modifiers ---

    #[test]
    fn map_modifiers_translates_each_flag_independently() {
        assert_eq!(
            map_modifiers(ModifiersState::SHIFT),
            Modifiers {
                shift: true,
                ctrl: false,
                alt: false,
                meta: false,
            }
        );
        assert_eq!(
            map_modifiers(ModifiersState::CONTROL | ModifiersState::SUPER),
            Modifiers {
                shift: false,
                ctrl: true,
                alt: false,
                meta: true,
            }
        );
        assert_eq!(map_modifiers(ModifiersState::empty()), Modifiers::default());
    }

    // --- map_key_event ---

    fn mods() -> Modifiers {
        Modifiers {
            shift: true,
            ..Modifiers::default()
        }
    }

    #[test]
    fn map_key_event_maps_a_named_key() {
        let event = map_key_event(
            &WinitKey::Named(WinitNamedKey::Backspace),
            None,
            ElementState::Pressed,
            false,
            mods(),
            false,
        );
        assert_eq!(
            event,
            Some(KeyEvent {
                key: Key::Named(NamedKey::Backspace),
                modifiers: mods(),
                repeat: false,
            })
        );
    }

    #[test]
    fn map_key_event_maps_resolved_text_as_character() {
        let event = map_key_event(
            &WinitKey::Character("a".into()),
            Some("a"),
            ElementState::Pressed,
            false,
            Modifiers::default(),
            false,
        );
        assert_eq!(
            event,
            Some(KeyEvent {
                key: Key::Character("a".to_string()),
                modifiers: Modifiers::default(),
                repeat: false,
            })
        );
    }

    #[test]
    fn map_key_event_passes_repeat_through_unfiltered() {
        let event = map_key_event(
            &WinitKey::Named(WinitNamedKey::ArrowLeft),
            None,
            ElementState::Pressed,
            true,
            Modifiers::default(),
            false,
        );
        assert!(event.expect("named key maps").repeat);
    }

    #[test]
    fn map_key_event_is_none_on_release() {
        let event = map_key_event(
            &WinitKey::Named(WinitNamedKey::Enter),
            None,
            ElementState::Released,
            false,
            Modifiers::default(),
            false,
        );
        assert_eq!(event, None);
    }

    #[test]
    fn map_key_event_drops_character_while_composing() {
        // The dedupe rule: a Preedit-active composition suppresses
        // KeyboardInput-derived Character events (Ime::Commit is authoritative).
        let event = map_key_event(
            &WinitKey::Character("a".into()),
            Some("a"),
            ElementState::Pressed,
            false,
            Modifiers::default(),
            true, // composing
        );
        assert_eq!(event, None);
    }

    #[test]
    fn map_key_event_still_maps_named_keys_while_composing() {
        // Only Character events are deduped; named editing keys pass through.
        let event = map_key_event(
            &WinitKey::Named(WinitNamedKey::Escape),
            None,
            ElementState::Pressed,
            false,
            Modifiers::default(),
            true, // composing
        );
        assert_eq!(
            event,
            Some(KeyEvent {
                key: Key::Named(NamedKey::Escape),
                modifiers: Modifiers::default(),
                repeat: false,
            })
        );
    }

    #[test]
    fn map_key_event_maps_space_to_a_character() {
        // winit models the spacebar as `Named(Space)` carrying a `" "` text
        // payload, never as `Character(" ")` — it must still reach a text widget
        // as the character it types.
        let event = map_key_event(
            &WinitKey::Named(WinitNamedKey::Space),
            Some(" "),
            ElementState::Pressed,
            false,
            Modifiers::default(),
            false,
        );
        assert_eq!(
            event,
            Some(KeyEvent {
                key: Key::Character(" ".to_string()),
                modifiers: Modifiers::default(),
                repeat: false,
            })
        );
    }

    #[test]
    fn map_key_event_maps_space_without_a_text_payload() {
        // A platform that reports no resolved text for the spacebar still types
        // a space.
        let event = map_key_event(
            &WinitKey::Named(WinitNamedKey::Space),
            None,
            ElementState::Pressed,
            false,
            Modifiers::default(),
            false,
        );
        assert_eq!(
            event,
            Some(KeyEvent {
                key: Key::Character(" ".to_string()),
                modifiers: Modifiers::default(),
                repeat: false,
            })
        );
    }

    #[test]
    fn map_key_event_drops_space_while_composing() {
        // Space takes the character path, so it takes the character dedupe rule
        // with it: `Ime::Commit` is authoritative mid-composition.
        let event = map_key_event(
            &WinitKey::Named(WinitNamedKey::Space),
            Some(" "),
            ElementState::Pressed,
            false,
            Modifiers::default(),
            true, // composing
        );
        assert_eq!(event, None);
    }

    #[test]
    fn map_key_event_is_none_for_unmapped_named_key_with_no_text() {
        let event = map_key_event(
            &WinitKey::Named(WinitNamedKey::F1),
            None,
            ElementState::Pressed,
            false,
            Modifiers::default(),
            false,
        );
        assert_eq!(event, None);
    }

    #[test]
    fn map_key_event_is_none_for_unmapped_named_key_carrying_text() {
        // The character carve-out is Space's alone: any other named key with no
        // editing semantics stays dropped, text payload or not.
        let event = map_key_event(
            &WinitKey::Named(WinitNamedKey::F1),
            Some("\u{f704}"),
            ElementState::Pressed,
            false,
            Modifiers::default(),
            false,
        );
        assert_eq!(event, None);
    }

    // --- ComposeLatch / Ime mapping ---

    #[test]
    fn compose_latch_opens_on_nonempty_preedit() {
        let mut latch = ComposeLatch::default();
        assert!(!latch.is_composing());
        let mapped = latch.observe(&Ime::Preedit("n".to_string(), Some((0, 1))));
        assert!(latch.is_composing());
        assert_eq!(
            mapped,
            ImeEvent::Compose {
                text: "n".to_string(),
                cursor: Some((0, 1)),
            }
        );
    }

    #[test]
    fn compose_latch_closes_on_empty_preedit_preceding_commit() {
        let mut latch = ComposeLatch::default();
        latch.observe(&Ime::Preedit("n".to_string(), Some((0, 1))));
        assert!(latch.is_composing());

        let mapped = latch.observe(&Ime::Preedit(String::new(), None));
        assert!(!latch.is_composing());
        assert_eq!(
            mapped,
            ImeEvent::Compose {
                text: String::new(),
                cursor: None,
            }
        );
    }

    #[test]
    fn compose_latch_closes_on_commit() {
        let mut latch = ComposeLatch::default();
        latch.observe(&Ime::Preedit("n".to_string(), Some((0, 1))));
        let mapped = latch.observe(&Ime::Commit("\u{306a}".to_string()));
        assert!(!latch.is_composing());
        assert_eq!(mapped, ImeEvent::Commit("\u{306a}".to_string()));
    }

    #[test]
    fn compose_latch_closes_on_enabled_and_disabled() {
        let mut latch = ComposeLatch::default();
        latch.observe(&Ime::Preedit("n".to_string(), Some((0, 1))));

        let mapped = latch.observe(&Ime::Disabled);
        assert!(!latch.is_composing());
        assert_eq!(mapped, ImeEvent::Disabled);

        latch.observe(&Ime::Preedit("n".to_string(), Some((0, 1))));
        let mapped = latch.observe(&Ime::Enabled);
        assert!(!latch.is_composing());
        assert_eq!(mapped, ImeEvent::Enabled);
    }

    // --- ime_purpose_for ---
    //
    // Pins the desktop shell's (deliberately thin) response to the
    // content-type hint: this is the honest "winit offers almost nothing"
    // outcome the task anticipated, recorded here so a later reader finds a
    // considered decision rather than a silently missed field. See
    // `ime_purpose_for`'s doc comment for why `Password` and `Terminal` are
    // the only variants that map to anything but `Normal`, and why even the
    // `Password` mapping only reaches Wayland.

    #[test]
    fn ime_purpose_maps_password_to_the_winit_password_purpose() {
        assert_eq!(
            ime_purpose_for(ImeContentType::Password),
            ImePurpose::Password
        );
    }

    #[test]
    fn ime_purpose_maps_terminal_to_the_winit_terminal_purpose() {
        assert_eq!(
            ime_purpose_for(ImeContentType::Terminal),
            ImePurpose::Terminal
        );
    }

    #[test]
    fn ime_purpose_has_no_distinct_mapping_for_normal_or_no_suggestions() {
        // `ImePurpose` has no "suppress suggestions" variant, so `NoSuggestions`
        // falls back to `Normal` just like `Normal` itself — only the secret
        // and terminal content types change the winit call.
        assert_eq!(ime_purpose_for(ImeContentType::Normal), ImePurpose::Normal);
        assert_eq!(
            ime_purpose_for(ImeContentType::NoSuggestions),
            ImePurpose::Normal
        );
    }

    /// Pins [`ImeSync`]'s default so a fresh shell starts believing it has
    /// already told winit `ImePurpose::Normal` — matching winit's own
    /// documented default, so the first real `Password` field is what
    /// triggers the first `set_ime_purpose` call, not startup.
    #[test]
    fn ime_sync_defaults_to_the_normal_purpose() {
        assert_eq!(ImeSync::default().purpose, ImePurpose::Normal);
    }

    // --- cursor: framework request -> winit shape, and the change gate ---
    //
    // The mapping is exhaustive over the framework's own vocabulary and the gate
    // is what keeps `Window::set_cursor` off the per-mouse-motion path; both are
    // pure, so neither needs a live window.

    #[test]
    fn every_framework_cursor_maps_to_its_winit_counterpart() {
        for (ours, theirs) in [
            (CursorIcon::Default, WinitCursorIcon::Default),
            (CursorIcon::Pointer, WinitCursorIcon::Pointer),
            (CursorIcon::Text, WinitCursorIcon::Text),
            (CursorIcon::Grab, WinitCursorIcon::Grab),
            (CursorIcon::Grabbing, WinitCursorIcon::Grabbing),
            (CursorIcon::ColResize, WinitCursorIcon::ColResize),
            (CursorIcon::RowResize, WinitCursorIcon::RowResize),
            (CursorIcon::NotAllowed, WinitCursorIcon::NotAllowed),
        ] {
            assert_eq!(
                winit_cursor_for(ours),
                theirs,
                "{ours:?} must reach the platform as {theirs:?}"
            );
        }
    }

    #[test]
    fn the_cursor_is_pushed_to_winit_only_on_a_change() {
        // A resolved cursor that has not moved says nothing to the platform — the
        // resolution re-runs on every pointer `Move`, so this gate is what stops a
        // platform call per mouse motion.
        assert_eq!(
            cursor_change_to_apply(CursorIcon::Default, CursorIcon::Default),
            None,
            "an unmoved cursor issues no winit call"
        );
        assert_eq!(
            cursor_change_to_apply(CursorIcon::Pointer, CursorIcon::Pointer),
            None
        );

        // Both directions of a real change fire, including the return to Default —
        // a widget releasing its request must actually restore the arrow.
        assert_eq!(
            cursor_change_to_apply(CursorIcon::Default, CursorIcon::Pointer),
            Some(WinitCursorIcon::Pointer)
        );
        assert_eq!(
            cursor_change_to_apply(CursorIcon::Pointer, CursorIcon::Default),
            Some(WinitCursorIcon::Default),
            "dropping a request restores the platform arrow"
        );
    }

    // --- build_tree_update ---
    //
    // Pure-function coverage of the `SemanticsUpdate` -> `accesskit::TreeUpdate`
    // assembly; no winit event loop or live adapter needed.

    #[test]
    fn build_tree_update_assembles_nodes_tree_and_focus() {
        let root_id = NodeId(1);
        let child_id = NodeId(2);
        let mut root_node = Node::new(Role::Window);
        root_node.set_children(vec![child_id]);
        let child_node = Node::new(Role::Button);

        let update = SemanticsUpdate {
            nodes: vec![(root_id, root_node), (child_id, child_node)],
            root: root_id,
            focus: Some(child_id),
        };

        let tree_update = build_tree_update(&update);
        assert_eq!(tree_update.nodes.len(), 2);
        assert_eq!(tree_update.tree, Some(Tree::new(root_id)));
        assert_eq!(tree_update.tree_id, TreeId::ROOT);
        assert_eq!(tree_update.focus, child_id);
    }

    #[test]
    fn build_tree_update_defaults_focus_to_root_when_nothing_focused() {
        let root_id = NodeId(1);
        let update = SemanticsUpdate {
            nodes: vec![(root_id, Node::new(Role::Window))],
            root: root_id,
            focus: None,
        };

        let tree_update = build_tree_update(&update);
        // SemanticsUpdate::focus_id() defaults to root — accesskit's `focus`
        // field is non-optional and must always name some target.
        assert_eq!(tree_update.focus, root_id);
    }

    // --- the default-theme precedence ladder (seed / appearance / override) ---
    //
    // Every assertion below drives the SAME ladder helpers (`base_theme`,
    // `reverted_theme` via `theme_after_override_poll`,
    // `follow_platform_brightness`) the production seed (`run_desktop`'s
    // `theme:` field), appearance (`WindowEvent::ThemeChanged`) and
    // override-poll (`RedrawRequested`) arms call. That sharing is the whole
    // point: a test that recomputed the ladder in its own body would stay green
    // if one of those arms regressed to an unconditional `Theme::neutral()`.
    //
    // What it does NOT cover: how a production arm *composes* those helpers —
    // `base_theme(default_theme())` at the seed site, versus a
    // default-discarding `base_theme(None)`. Both type-check and both drive the
    // same helper, so no assertion here can tell them apart.
    // `crates/frust/tests/theme_ladder_conformance.rs` pins that other half
    // instead: that each arm still calls these helpers, that the seed site
    // still passes `default_theme()` into `base_theme`, and that the two mobile
    // shells' helper copies stay identical to the ones exercised here.

    /// A stand-in for the base theme a design-system plugin seeds through
    /// `set_default_theme`.
    ///
    /// Deliberately **not** `Theme::neutral()`: the ladder's whole point is
    /// that a seeded base displaces the shell's built-in floor, so every
    /// assertion below that spells `assert_ne!(.., Theme::neutral())` — or
    /// reads a seeded value back — needs a theme the floor cannot be confused
    /// with. The visible edit is the design-language tag, the one field a
    /// third-party design system is expected to claim (`DesignLanguage` is
    /// `#[non_exhaustive]` with exactly this `Custom` variant for it); every
    /// token stays the neutral baseline's, so no catalog is named here.
    fn seeded_design_system_theme() -> Theme {
        Theme::builder(Theme::neutral())
            .design_language(DesignLanguage::Custom("test-design-system"))
            .build()
    }

    #[test]
    fn an_unseeded_shell_starts_on_the_builtin_fallback_at_the_platform_brightness() {
        // Behavior 1. Nothing seeded: the seed site lands on the shell's own
        // built-in, design-language-free floor...
        let mut theme = base_theme(None);
        assert_eq!(theme, Theme::neutral());

        // ...and the platform's first appearance report drives brightness
        // (`resumed`'s seed, then every `ThemeChanged`), so the fallback's own
        // starting brightness never leaks onto a dark-preference platform.
        follow_platform_brightness(&mut theme, false, brightness_from_winit(None));
        assert_eq!(theme, Theme::neutral().with_brightness(Brightness::Light));
        follow_platform_brightness(
            &mut theme,
            false,
            brightness_from_winit(Some(WinitTheme::Dark)),
        );
        assert_eq!(theme, Theme::neutral().with_brightness(Brightness::Dark));
    }

    #[test]
    fn a_seeded_default_is_the_base_and_still_follows_platform_brightness() {
        // Behavior 2. A design system's `set_default_theme` supplies the base...
        let seeded = seeded_design_system_theme();
        let mut theme = base_theme(Some(seeded.clone()));
        assert_eq!(theme, seeded);
        // ...in place of the built-in floor, not layered over it.
        assert_ne!(theme, Theme::neutral());

        // ...and unlike an app-forced override it does NOT pin brightness: the
        // appearance arm keeps flipping the seeded base in place, both ways.
        follow_platform_brightness(
            &mut theme,
            false,
            brightness_from_winit(Some(WinitTheme::Dark)),
        );
        assert_eq!(theme, seeded.clone().with_brightness(Brightness::Dark));
        follow_platform_brightness(
            &mut theme,
            false,
            brightness_from_winit(Some(WinitTheme::Light)),
        );
        assert_eq!(theme, seeded.with_brightness(Brightness::Light));
    }

    #[test]
    fn an_app_theme_override_beats_a_seeded_default_and_pins_brightness() {
        // Behavior 3. The override poll's `Some(Some(theme))` arm takes the
        // forced theme wholesale. The suppliers panic rather than answer, which
        // proves more than an inequality could: the arm cannot even observe the
        // seeded default or the platform brightness, so no seeded value and no
        // platform preference can influence what an override resolves to.
        let forced = Theme::neutral().with_brightness(Brightness::Light);
        let decided = theme_after_override_poll(
            Some(Some(forced.clone())),
            || panic!("an active override must not consult the seeded default"),
            || panic!("an active override must not consult the platform brightness"),
        );
        assert_eq!(decided, Some((forced, true)));

        // ...and it keeps winning against a *later* live platform flip
        // (`ThemeChanged`'s override-wins rule), exactly where the seeded
        // default of the test above followed the platform instead. The
        // distinction that must survive here is brightness, not identity:
        // `Light` forced against a `Dark` platform report.
        let mut active = Theme::neutral().with_brightness(Brightness::Light);
        follow_platform_brightness(&mut active, true, Brightness::Dark);
        assert_eq!(active.brightness, Brightness::Light);
    }

    #[test]
    fn clearing_an_override_reverts_to_the_seeded_default_not_the_builtin() {
        // Behavior 4. The `Some(None)` arm with a design system's default
        // seeded: the revert lands on THAT base at the window's current
        // brightness — not on the built-in fallback, and not on the cleared
        // override's pinned brightness.
        let seeded = seeded_design_system_theme();
        let decided = theme_after_override_poll(
            Some(None),
            || Some(seeded.clone()),
            || brightness_from_winit(Some(WinitTheme::Dark)),
        );
        assert_eq!(
            decided,
            Some((seeded.with_brightness(Brightness::Dark), false))
        );
        // Spelled out, since this is the arm the ladder exists for: a seeded
        // shell must NOT revert to the built-in fallback.
        assert_ne!(
            decided.map(|(theme, _)| theme),
            Some(Theme::neutral().with_brightness(Brightness::Dark))
        );
    }

    #[test]
    fn clearing_an_override_with_nothing_seeded_reverts_to_the_builtin() {
        // Behavior 5. The same arm with an empty slot: the built-in fallback at
        // the platform's brightness.
        let decided = theme_after_override_poll(
            Some(None),
            || None,
            || brightness_from_winit(Some(WinitTheme::Dark)),
        );
        assert_eq!(
            decided,
            Some((Theme::neutral().with_brightness(Brightness::Dark), false))
        );
    }

    #[test]
    fn a_poll_reporting_no_change_leaves_the_active_theme_alone() {
        // The `None` arm: no `set_app_theme`/`clear_app_theme` since the last
        // frame, so the shell must not touch its theme — and must pay neither
        // the process-global slot read nor the window appearance query, both of
        // which run on every frame if this arm ever stops being lazy.
        let decided = theme_after_override_poll(
            None,
            || panic!("an unchanged poll must not read the process-global default slot"),
            || panic!("an unchanged poll must not query the window's appearance"),
        );
        assert_eq!(decided, None);
    }

    #[test]
    fn a_seeded_default_reaches_both_ladder_sites_through_the_process_global_slot() {
        // The wiring the argument-taking helpers above cannot check: what the
        // production sites actually pass is `frust_shell_common::default_theme`,
        // so a plugin's `set_default_theme` really does reach both the seed and
        // the revert.
        //
        // `theme_default` deliberately exposes no reset (a shell needs the same
        // seeded base again when `clear_app_theme` reverts an override, so the
        // slot is non-destructive), which is why this must remain the ONLY test in this
        // binary that writes it — `the_default_slot_has_exactly_one_test_writer`
        // pins that. Asserting the pristine state first turns a future second
        // writer into a loud failure here instead of a silent order dependency
        // in whichever test happens to run after it.
        assert_eq!(
            default_theme(),
            None,
            "another test in this binary seeded the process-global default slot first — \
             the slot has no reset, so tests that read it cannot be order-independent"
        );

        let seeded = seeded_design_system_theme();
        frust_shell_common::set_default_theme(seeded.clone());

        // The same composition the seed site (`run_desktop`'s `theme:` field)
        // performs — re-typed here, not shared with it, so this asserts only
        // that the composition resolves the seeded default, never that the
        // production site still spells it this way. That second half is
        // `theme_ladder_conformance.rs`'s
        // `every_shell_seeds_base_theme_from_the_default_slot` source scan.
        assert_eq!(base_theme(default_theme()), seeded);
        // ...and the revert arm, reading that same slot through the same
        // supplier the production arm passes.
        assert_eq!(
            theme_after_override_poll(Some(None), default_theme, || Brightness::Dark),
            Some((seeded.with_brightness(Brightness::Dark), false))
        );
    }

    #[test]
    fn the_default_slot_has_exactly_one_test_writer() {
        // Pins the uniqueness the test above depends on. A plain substring scan
        // of this very file (the `print_free_cores.rs`/`surface_mode_conformance.rs`
        // idiom — this repo has no lint-plugin tooling), not a parser: every
        // real call sits on its own statement line and comment-only lines (every
        // doc mention of the seam) are stripped. The needle is assembled from
        // two pieces so this line is not itself a hit.
        let needle = concat!("set_default_theme", "(");
        let writers = include_str!("app_handler.rs")
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .filter(|line| line.contains(needle))
            .count();
        assert_eq!(
            writers, 1,
            "the process-global default-theme slot has no reset, so exactly one test in this \
             binary may write it (found {writers}); serialize them behind a lock the way \
             `frust-shell-common::theme_override`'s own tests do, or fold the new assertion \
             into the existing writer"
        );
    }

    // --- Event dispatch runs under the reactive root Owner -------------------
    //
    // Every shell wrapped its per-frame *rebuild* in
    // `ReactiveRuntime::with_owner` but left *event dispatch* unwrapped, so a
    // press/key/IME handler ran with `Owner::current() == None` and every
    // `use_context::<T>()` inside it resolved to `None` — including for the
    // `Theme`/`WindowMetrics` the shell itself provides. `Owner::with` restores
    // the previous owner on return, so nothing carried over from the rebuild.
    //
    // These drive `event_under_owner` (the production helper `dispatch` calls)
    // against a real `RenderRoot`, and pin the pre-fix shape as the negative
    // control — the `app_tree.rs` root-owner pair's idiom.

    use std::cell::RefCell;
    use std::rc::Rc;

    use std::sync::Arc;

    use frust_core::event::{EventCtx, EventResult, PointerEvent, PointerPhase};
    use frust_core::layout::BoxConstraints;
    use frust_core::view::BuildCtx;
    use frust_core::widget::{LayoutCtx, PaintCtx, Widget};
    use frust_core::{ChangeFlags, PaintScene, RenderRoot, View};
    use frust_reactive::{FrameWaker, ReactiveRuntime, RwSignal, TrackedScope, use_context};
    use kurbo::Size;
    use reactive_graph::owner::expect_context;
    use reactive_graph::traits::{Get, Set};

    use super::{InputEvent, event_under_owner, provide_context};

    /// The context value the shell provides under the root owner (a stand-in
    /// for the real `Theme`/`WindowMetrics` deliveries) and a handler reads back.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct ProbeCtx(u32);

    /// What one event pass observed, recorded by [`ProbeWidget::event`].
    #[derive(Debug, Default)]
    struct Observed {
        /// `use_context::<ProbeCtx>()` — the non-panicking form, and the only
        /// one app code can reach (the `frust` facade re-exports `use_context`
        /// but no `expect_context`).
        used: Option<u32>,
        /// The panic message `reactive_graph::expect_context` produced, or
        /// `None` if it returned normally.
        expect_panic: Option<String>,
    }

    /// Recording sink shared with the widget's handler.
    type Seen = Rc<RefCell<Observed>>;

    /// A leaf widget whose `event` handler does what an app's `.on_press`
    /// closure would: read a context, and read a signal.
    struct ProbeWidget {
        seen: Seen,
        /// Read with `.get()` from inside the handler — the read that must NOT
        /// register a dependency for the frame's rebuild scope.
        handler_signal: RwSignal<u32>,
    }

    impl Widget for ProbeWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(40.0, 20.0))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, _ctx: &mut EventCtx, _event: &InputEvent) -> EventResult {
            let used = use_context::<ProbeCtx>().map(|c| c.0);
            let expect_panic =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(expect_context::<ProbeCtx>))
                    .err()
                    .map(|payload| match payload.downcast::<String>() {
                        Ok(msg) => *msg,
                        Err(_) => "<non-string panic payload>".to_string(),
                    });
            let _ = self.handler_signal.get();
            *self.seen.borrow_mut() = Observed { used, expect_panic };
            EventResult::Handled
        }
    }

    struct ProbeView {
        seen: Seen,
        handler_signal: RwSignal<u32>,
    }

    impl View<()> for ProbeView {
        type Element = ProbeWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> ProbeWidget {
            ProbeWidget {
                seen: self.seen.clone(),
                handler_signal: self.handler_signal,
            }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut ProbeWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            element.seen = self.seen.clone();
            ChangeFlags::NONE
        }
    }

    /// One primary-button press at the widget's centre.
    fn press() -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Down,
            position: kurbo::Point::new(20.0, 10.0),
            button: PointerButton::Primary,
        })
    }

    /// Build the shell-shaped fixture: the runtime, a root-provided context, a
    /// laid-out `RenderRoot` over `ProbeView`, and the frame `TrackedScope`
    /// (already tracking `view_signal` through one rebuild, exactly as
    /// `RedrawRequested` leaves it).
    #[expect(
        clippy::type_complexity,
        reason = "test fixture tuple, read once below"
    )]
    fn fixture() -> (
        &'static ReactiveRuntime,
        RenderRoot<(), ProbeView>,
        Seen,
        TrackedScope,
        RwSignal<u32>,
        RwSignal<u32>,
    ) {
        let waker: FrameWaker = Arc::new(|| {});
        let runtime = ReactiveRuntime::init(waker);
        runtime.with_owner(|| provide_context(ProbeCtx(7)));

        let (view_signal, handler_signal) =
            runtime.with_owner(|| (RwSignal::new(0u32), RwSignal::new(0u32)));
        let seen: Seen = Rc::new(RefCell::new(Observed::default()));

        let mut root: RenderRoot<(), ProbeView> = RenderRoot::new();
        let scope = TrackedScope::new();
        let seen_for_logic = seen.clone();
        let mut app_logic = move |_state: &mut ()| {
            // A view read, the way a real `app_logic`/`Component::build` reads
            // state — this is the subscription the frame loop depends on.
            let _ = view_signal.get();
            ProbeView {
                seen: seen_for_logic.clone(),
                handler_signal,
            }
        };
        // The production rebuild wrap, verbatim.
        runtime.with_owner(|| scope.track(|| root.rebuild(&mut app_logic, &mut ())));
        root.layout(Size::new(100.0, 100.0));

        (runtime, root, seen, scope, view_signal, handler_signal)
    }

    /// The negative control, and the bug as it shipped: dispatching the event
    /// pass with no ambient owner (the pre-fix `self.root.event(...)` shape)
    /// resolves every context to `None`, and the panicking form panics.
    #[test]
    fn event_pass_without_the_owner_wrap_sees_no_context() {
        let (_runtime, mut root, seen, _scope, _view_signal, _handler_signal) = fixture();

        // `expect_context`'s panic is caught inside the handler; mute the
        // default hook so its backtrace line doesn't pollute the test output.
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let outcome = root.event(&mut (), &press());
        std::panic::set_hook(hook);

        assert!(outcome.handled, "the probe widget consumed the press");
        let seen = seen.borrow();
        assert_eq!(
            seen.used, None,
            "pre-fix: an unwrapped event pass has no current Owner, so \
             use_context resolves None even for a context the shell provided"
        );
        // The panicking form's message, as actually observed on this path:
        //
        //   Location { file: ".../core/src/ops/function.rs", line: 250, column: 5 }
        //   expected context of type
        //   "frust_shell_desktop::app_handler::tests::ProbeCtx" to be present
        //
        // (the `Location` names core's `FnOnce::call_once` shim because
        // `expect_context` is handed to `catch_unwind` as a function item, so
        // its `#[track_caller]` resolves there) — only the message tail is
        // pinned below, since the prefix is a std implementation detail.
        let panic_msg = seen
            .expect_panic
            .as_deref()
            .expect("pre-fix: expect_context must panic with no ambient Owner");
        assert!(
            panic_msg.ends_with("to be present") && panic_msg.contains("expected context of type"),
            "unexpected panic message: {panic_msg}"
        );
    }

    /// The fix: the same press delivered through `event_under_owner` resolves
    /// the shell-provided context.
    #[test]
    fn event_pass_under_the_owner_wrap_resolves_root_context() {
        let (runtime, mut root, seen, _scope, _view_signal, _handler_signal) = fixture();

        let outcome = event_under_owner(runtime, &mut root, &mut (), &press());

        assert!(outcome.handled, "the probe widget consumed the press");
        let seen = seen.borrow();
        assert_eq!(
            seen.used,
            Some(7),
            "a handler must resolve a context the shell provided under the root \
             owner"
        );
        assert_eq!(
            seen.expect_panic, None,
            "the panicking form must no longer panic from an event handler"
        );
    }

    /// The cost side of the wrap: it installs an `Owner`, never a
    /// `TrackedScope`. A signal read inside a handler must not become a
    /// dependency of the frame's rebuild scope (a rebuild storm), and the
    /// dependencies the rebuild recorded must survive the event pass intact.
    #[test]
    fn event_pass_neither_adds_nor_drops_rebuild_dependencies() {
        let (runtime, mut root, _seen, scope, view_signal, handler_signal) = fixture();

        assert!(
            !scope.is_dirty(),
            "fixture precondition: the rebuild scope is clean"
        );

        event_under_owner(runtime, &mut root, &mut (), &press());
        assert!(
            !scope.is_dirty(),
            "the event pass itself must not dirty the frame scope"
        );

        // (1) No new dependency: the handler read `handler_signal`, but it did
        // so with no observer installed, so a later write schedules nothing.
        handler_signal.set(1);
        assert!(
            !scope.is_dirty(),
            "a signal read only inside an event handler must not subscribe the \
             frame scope — that would schedule a rebuild the pre-fix shell never \
             scheduled (a rebuild storm)"
        );

        // (2) No dropped dependency: the rebuild's own subscription still
        // stands. `TrackedScope::track` clears recorded sources on entry, so
        // this is what would break if the event pass were wrapped in `track`.
        view_signal.set(1);
        assert!(
            scope.is_dirty(),
            "the rebuild's tracked read must still wake the frame loop after an \
             event pass"
        );
    }

    // --- the desktop config / extension seam ---
    //
    // What is reachable without a live event loop: the config→attributes→hook
    // composition (`window_attributes`) and the brightness change gate. The
    // three hooks whose call sites need a real window or a running loop
    // (`on_event_loop_builder` in `run_desktop_with`, `on_window_created` and
    // `pump`/`on_close_requested` inside the winit callbacks) are documented at
    // their call sites instead — winit exposes no way to construct an
    // `ActiveEventLoop` or a `Window` outside `run_app`.

    /// A recording extension: proves the production attribute path really runs
    /// the hook (and runs it *last*, so a per-OS shell can override what the
    /// core assembled).
    #[derive(Debug, Default)]
    struct SpyExtensions {
        attributes_calls: usize,
        override_title: Option<&'static str>,
    }

    impl DesktopExtensions for SpyExtensions {
        fn on_window_attributes(&mut self, attributes: WindowAttributes) -> WindowAttributes {
            self.attributes_calls += 1;
            match self.override_title {
                Some(title) => attributes.with_title(title),
                None => attributes,
            }
        }
    }

    #[test]
    fn window_attributes_default_to_the_historical_preview_window() {
        // The `WINDOW_TITLE` const this shell used to hardcode now arrives via
        // the default config, so the zero-config preview is unchanged.
        let attributes = window_attributes(&DesktopConfig::default(), &mut NoExtensions);
        assert_eq!(attributes.title, "Frust");
        assert_eq!(attributes.inner_size, Some(super::INITIAL_SIZE.into()));
        assert!(
            !attributes.visible,
            "the window must be created hidden — the accessibility adapter is \
             constructed before it is shown"
        );
    }

    #[test]
    fn window_attributes_take_the_title_from_the_config() {
        let config = DesktopConfig::new().with_app_name("Huddle");
        let attributes = window_attributes(&config, &mut NoExtensions);
        assert_eq!(attributes.title, "Huddle");
    }

    #[test]
    fn window_attributes_run_the_extension_hook_last() {
        let mut extensions = SpyExtensions {
            override_title: Some("overridden"),
            ..SpyExtensions::default()
        };
        let config = DesktopConfig::new().with_app_name("Huddle");
        let attributes = window_attributes(&config, &mut extensions);

        assert_eq!(extensions.attributes_calls, 1);
        assert_eq!(
            attributes.title, "overridden",
            "the hook sees the core's attributes and its result wins — the only \
             chance a per-OS shell gets at pre-create-only attributes"
        );
    }

    // --- the FRUST_WINDOW_SIZE / FRUST_WINDOW_MAXIMIZED measurement knobs ---
    //
    // The parsers take explicit `Option<&str>` rather than touching the
    // process environment (the `render_thread_kill_switch`/`frame_gate`
    // precedent) — env-var tests that mutated `std::env` would race every
    // other test in this binary sharing the same process.

    #[test]
    fn parse_window_size_valid() {
        assert_eq!(
            parse_window_size(Some("2560x1440")),
            LogicalSize::new(2560, 1440)
        );
        // Surrounding whitespace on the whole value is trimmed, matching
        // every other `FRUST_*` string knob's parser.
        assert_eq!(
            parse_window_size(Some("  1280x720  ")),
            LogicalSize::new(1280, 720)
        );
        // The smallest legal value: both components at least 1.
        assert_eq!(parse_window_size(Some("1x1")), LogicalSize::new(1, 1));
    }

    #[test]
    fn parse_window_size_missing_falls_back_silently() {
        assert_eq!(parse_window_size(None), super::INITIAL_SIZE);
        assert_eq!(parse_window_size(Some("")), super::INITIAL_SIZE);
        assert_eq!(parse_window_size(Some("   ")), super::INITIAL_SIZE);
    }

    #[test]
    fn parse_window_size_zero_component_falls_back() {
        assert_eq!(parse_window_size(Some("0x600")), super::INITIAL_SIZE);
        assert_eq!(parse_window_size(Some("800x0")), super::INITIAL_SIZE);
        assert_eq!(parse_window_size(Some("0x0")), super::INITIAL_SIZE);
    }

    #[test]
    fn parse_window_size_invalid_falls_back() {
        for invalid in [
            "800",                      // no separator
            "800x",                     // missing height
            "x600",                     // missing width
            "800x600x400",              // extra separator
            "800X600",                  // wrong case separator
            "800.5x600",                // not an integer
            "-800x600",                 // signed
            "800 x 600",                // inner whitespace around the separator
            "eightx600",                // non-digit
            "800xsix",                  // non-digit
            "99999999999999999999x600", // overflows u32
        ] {
            assert_eq!(
                parse_window_size(Some(invalid)),
                super::INITIAL_SIZE,
                "expected {invalid:?} to fall back to the default size"
            );
        }
    }

    #[test]
    fn parse_window_maximized_recognises_one_and_true() {
        assert!(parse_window_maximized(Some("1")));
        assert!(parse_window_maximized(Some("true")));
        assert!(parse_window_maximized(Some("TRUE")));
        assert!(parse_window_maximized(Some("  True  ")));
    }

    #[test]
    fn parse_window_maximized_defaults_to_false() {
        assert!(!parse_window_maximized(None));
        assert!(!parse_window_maximized(Some("")));
        assert!(!parse_window_maximized(Some("0")));
        assert!(!parse_window_maximized(Some("yes")));
        assert!(!parse_window_maximized(Some("2")));
    }

    #[test]
    fn resolved_window_knob_runtime_wins_over_compile_time() {
        assert_eq!(
            resolved_window_knob(Some("800x600"), Some("2560x1440".to_string())),
            (Some("2560x1440".to_string()), WindowKnobSource::Env)
        );
    }

    #[test]
    fn resolved_window_knob_empty_runtime_falls_through_to_compile_time() {
        assert_eq!(
            resolved_window_knob(Some("800x600"), Some(String::new())),
            (Some("800x600".to_string()), WindowKnobSource::Define)
        );
    }

    #[test]
    fn resolved_window_knob_neither_set_is_default() {
        assert_eq!(
            resolved_window_knob(None, None),
            (None, WindowKnobSource::Default)
        );
    }

    #[test]
    fn apply_window_size_sets_inner_size_and_leaves_maximized_off_by_default() {
        let attributes = apply_window_size(
            WindowAttributes::default(),
            LogicalSize::new(1280, 720),
            false,
        );
        assert_eq!(
            attributes.inner_size,
            Some(LogicalSize::new(1280, 720).into())
        );
        assert!(!attributes.maximized);
    }

    #[test]
    fn apply_window_size_sets_maximized_when_requested() {
        let attributes = apply_window_size(WindowAttributes::default(), super::INITIAL_SIZE, true);
        assert!(
            attributes.maximized,
            "FRUST_WINDOW_MAXIMIZED=1 must set WindowAttributes::maximized — the size is still \
             applied (documented), but the OS ignores it for a window maximized at creation"
        );
    }

    // --- the theme-brightness hook's change gate ---

    #[test]
    fn the_first_resolved_brightness_is_always_reported() {
        // Nothing reported yet (`resumed`'s seed): the extension learns the
        // starting brightness rather than having to wait for a flip.
        assert_eq!(
            brightness_change_to_notify(None, Brightness::Light),
            Some(Brightness::Light)
        );
    }

    #[test]
    fn an_unchanged_brightness_reports_nothing() {
        // `apply_theme` also runs for theme re-pushes that move no brightness
        // (an override swapping one dark theme for another), and the hook's
        // contract is "actually changed".
        assert_eq!(
            brightness_change_to_notify(Some(Brightness::Dark), Brightness::Dark),
            None
        );
    }

    #[test]
    fn a_changed_brightness_is_reported() {
        assert_eq!(
            brightness_change_to_notify(Some(Brightness::Light), Brightness::Dark),
            Some(Brightness::Dark)
        );
        assert_eq!(
            brightness_change_to_notify(Some(Brightness::Dark), Brightness::Light),
            Some(Brightness::Light)
        );
    }
}
