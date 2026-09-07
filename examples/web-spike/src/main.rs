//! Frust web-shell Phase 0 spike: the **browser render probe** (w0-03).
//!
//! Builds on w0-02's compile probe (same crate, same standalone workspace —
//! see `Cargo.toml`) and takes it the rest of the way: a static
//! [`frust_scene::Scene`] with shapes and shaped text, rendered through
//! `frust-engine` onto a real browser canvas, once per backend arm.
//!
//! `frust-engine` is the *only* renderer — there is no tier to select, and
//! this file contains no switch that could pick another one. The one thing
//! the probe does select is the **wgpu backend**, which is a different axis
//! entirely: `?arm=webgpu` (the default) restricts the `wgpu::Instance` to
//! [`wgpu::Backends::BROWSER_WEBGPU`], and `?arm=webgl` restricts it to
//! [`wgpu::Backends::GL`] — wgpu's WebGL2 backend on this target, where
//! `frust-gpu`'s [`frust_gpu::DownlevelProfile::WebGl2`] must engage on its
//! own from the resolved `wgpu::Backend::Gl`. Both arms run the identical
//! scene through the identical renderer; one `.wasm` serves both.
//!
//! The seam that makes that possible without touching `crates/` is
//! [`frust_gpu::ContextOptions::backends`] +
//! [`frust_gpu::RenderContext::with_options`]: `RenderContext::new()` falls
//! back to `wgpu::Backends::from_env()` (the `WGPU_BACKEND` knob), which can
//! never resolve in a browser — there is no environment — so a web host that
//! wants a specific backend must pass one programmatically. It can.
//!
//! Structure: everything above the `web` module is target-independent and is
//! also what a plain `cargo check` (no `--target`) of this spike still type-
//! checks, so w0-02's native compile probe keeps working unchanged. The
//! browser half — winit event loop, canvas, surface bring-up, logging — is
//! entirely inside [`mod web`], gated on `wasm32`.
//!
//! See `RESULTS.md` for the per-arm outcome, the console lines each arm
//! produced verbatim, and the exact build/serve/drive commands.

use kurbo::{Affine, Point, Rect};
use peniko::{Brush, Color};

/// The bundled probe font, compiled into the `.wasm`.
///
/// fontique's wasm backend enumerates **no** system fonts by design, so a
/// browser build that does not carry its own font shapes nothing at all: the
/// probe's whole text half would silently render zero glyphs and still look
/// like a "successful" frame. Bundling removes that failure mode.
///
/// `testing/fonts/NotoSans-Subset.ttf` is the workspace's own deterministic
/// Latin test face (8.5 KB — see `testing/fonts/LICENSES.md` for provenance,
/// the subsetting command, and its OFL-1.1 terms). Its coverage is *exactly*
/// `U+0020 U+002C U+0048 U+0065 U+006C U+006F U+00E9 U+0302` — space, comma,
/// `H`, `e`, `l`, `o`, `é`, combining circumflex — which is why
/// [`PROBE_TEXT`] below is what it is.
const PROBE_FONT: &[u8] = include_bytes!("../../../testing/fonts/NotoSans-Subset.ttf");

/// The probe's visible string, chosen to sit inside [`PROBE_FONT`]'s subset
/// coverage. Every character here has a real outline in the bundled face, so
/// a missing glyph in a screenshot means a shaping/atlas/raster failure and
/// never a coverage gap — the property that makes the screenshot evidence.
const PROBE_TEXT: &str = "Hello";

/// A second, smaller line exercising a non-ASCII codepoint (`é`, U+00E9) and
/// the subset's punctuation, so the probe's text half is not a single
/// all-ASCII run.
///
/// Every letter is capital `H` rather than lowercase: [`PROBE_FONT`]'s subset
/// carries U+0048 (`H`) and **not** U+0068 (`h`). An earlier draft of this
/// line read `"Hello, hello, héllo"` and rendered two `.notdef` boxes on both
/// arms — correct font behaviour, but indistinguishable at a glance from the
/// genuine glyph-coverage failure this probe found on the WebGL2 arm. Keeping
/// the string inside the subset keeps every box in a screenshot meaningful.
const PROBE_TEXT_SMALL: &str = "Hello, Hello, Héllo";

/// Canvas size the probe renders at, matching `index.html`'s `<canvas>`
/// attributes. Fixed rather than read from the layout box: a probe wants the
/// same pixels every run, and device-pixel-ratio scaling is a web-shell
/// concern (w1/w2), not a "does the engine draw at all" one.
const CANVAS_WIDTH: u32 = 800;
const CANVAS_HEIGHT: u32 = 600;

/// The frame's clear colour, distinct from every colour the scene paints so
/// an all-clear frame (engine drew nothing) is instantly distinguishable
/// from a correct one in a screenshot.
const BASE_COLOR: Color = Color::from_rgb8(0x0b, 0x0e, 0x14);

/// Builds the probe scene: an opaque card, three primitive families
/// (rounded rect, axis-aligned rects, a stroked line) and two shaped text
/// runs.
///
/// Deliberately covers more than one command kind. The engine's strip
/// pipeline rasterises fills, strokes and glyph runs down different paths, so
/// a scene of one rectangle could go green while text or strokes were broken;
/// a screenshot of this scene cannot.
fn build_scene(
    text_ctx: &mut frust_text::TextContext,
    family: frust_text::FontFamily,
    width: f64,
    height: f64,
) -> frust_scene::Scene {
    let mut scene = frust_scene::Scene::new();
    {
        let mut builder = frust_scene::SceneBuilder::new(&mut scene);

        // The card: a rounded rect inset from the canvas edge, so a frame
        // that renders the card but clips it wrongly is visible as a
        // geometry error rather than as "the whole canvas is one colour".
        builder.fill_rounded_rect(
            Rect::new(32.0, 32.0, width - 32.0, height - 32.0),
            28.0,
            Brush::Solid(Color::from_rgb8(0x1a, 0x22, 0x33)),
        );

        // Accent bar above the headline.
        builder.fill_rect(
            Rect::new(72.0, 96.0, 272.0, 112.0),
            Brush::Solid(Color::from_rgb8(0x4f, 0xc3, 0xf7)),
        );

        // Three swatches, drawn as separate rects so the strip generator has
        // more than one disjoint span per scanline to merge.
        for (index, rgb) in [
            (0xefu8, 0x53u8, 0x50u8),
            (0x66, 0xbb, 0x6a),
            (0xff, 0xca, 0x28),
        ]
        .into_iter()
        .enumerate()
        {
            let x = 72.0 + (index as f64) * 104.0;
            builder.fill_rounded_rect(
                Rect::new(x, height - 208.0, x + 88.0, height - 120.0),
                12.0,
                Brush::Solid(Color::from_rgb8(rgb.0, rgb.1, rgb.2)),
            );
        }

        // A stroked line: the stroke path is separate from the fill path in
        // the engine's compiler, so this covers a family the rects do not.
        builder.stroke_line(
            Point::new(72.0, height - 88.0),
            Point::new(width - 72.0, height - 88.0),
            6.0,
            Brush::Solid(Color::from_rgb8(0xff, 0x8a, 0x65)),
        );

        // The headline, shaped against the bundled face.
        let mut headline = frust_text::TextStyle::new(112.0, Color::from_rgb8(0xff, 0xff, 0xff));
        headline.family = family.clone();
        let layout = text_ctx.layout(PROBE_TEXT, &headline, None);
        for run in layout.to_scene_runs(Point::new(72.0, 152.0)) {
            builder.draw_glyph_run(run);
        }

        // A smaller second run, at a size where hinting/antialiasing
        // differences between the two backends would show up.
        let mut caption = frust_text::TextStyle::new(28.0, Color::from_rgb8(0x9e, 0xc5, 0xe8));
        caption.family = family;
        // `max_width` is `f32` on this API; the scene's own geometry is
        // `f64` (kurbo), so the wrap width is the one place the two meet.
        let layout = text_ctx.layout(PROBE_TEXT_SMALL, &caption, Some((width - 144.0) as f32));
        for run in layout.to_scene_runs(Point::new(72.0, 300.0)) {
            builder.draw_glyph_run(run);
        }
    }
    scene
}

/// Compiles the scene into `frust-engine`'s sparse-strip display list on the
/// CPU, answering the draw count.
///
/// Kept from w0-02: it is the deepest engine call reachable without a device,
/// so a probe that fails at surface creation can still report whether the CPU
/// half of the frame path produced work — the difference between "the engine
/// compiled nothing" and "the engine compiled N draws the GPU never showed".
fn compile_frame(scene: &frust_scene::Scene, width: u32, height: u32) -> usize {
    // `SceneCompiler` is `u16`-sized (its strip coordinates are), while the
    // surface/canvas dimensions everything else here carries are `u32`.
    // Saturating rather than `try_into().unwrap()`: an oversized canvas is a
    // reason to compile a clipped frame and say so, never a reason to panic
    // inside a probe whose whole job is to report what happened.
    let width16 = u16::try_from(width).unwrap_or(u16::MAX);
    let height16 = u16::try_from(height).unwrap_or(u16::MAX);
    let mut compiler = frust_engine::SceneCompiler::new(width16, height16);
    match compiler.compile(scene, Affine::IDENTITY, (width16, height16)) {
        Ok(frame) => frame.draws().len(),
        Err(err) => {
            report_error(&format!("web-spike: scene compile failed: {err:?}"));
            0
        }
    }
}

/// Reports a non-fatal failure on whichever sink this target has.
///
/// Shared code cannot reach for `log::error!`, because `log` is one of the
/// `wasm32`-gated rows in `Cargo.toml` and does not exist in a native build
/// of this spike; and it must not reach for `eprintln!` alone, because that
/// is a silent no-op on `wasm32-unknown-unknown` — which would make the one
/// failure a browser run most needs to see the one it cannot.
fn report_error(message: &str) {
    #[cfg(target_arch = "wasm32")]
    web::log_line(message);
    #[cfg(not(target_arch = "wasm32"))]
    eprintln!("{message}");
}

/// Registers [`PROBE_FONT`] and answers the family to shape against.
///
/// A registered family shadows a same-named system family (fontique 0.11
/// semantics), and on wasm there are no system families at all, so the
/// returned family is the only one that can resolve. Falls back to
/// [`frust_text::FontFamily::SystemUi`] on failure so the caller can still
/// render the shapes and *report* the text failure, rather than aborting the
/// whole frame over it.
fn register_probe_font(
    text_ctx: &mut frust_text::TextContext,
) -> Result<frust_text::FontFamily, String> {
    let families = text_ctx
        .register_fonts(PROBE_FONT.to_vec())
        .map_err(|err| format!("{err}"))?;
    let first = families
        .first()
        .ok_or_else(|| "register_fonts accepted the blob but named no family".to_string())?;
    Ok(frust_text::FontFamily::named(first.name.clone()))
}

fn main() {
    // The browser arm is driven by `web::start`, which wasm-bindgen wires
    // into the module's start section — `main` is never the wasm entry point
    // (see `mod web`). On a native host this stays w0-02's render-less CPU
    // probe, so `cargo check`/`cargo build` with no `--target` still walks
    // real call sites in all five graph crates.
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut text_ctx = frust_text::TextContext::new();
        let family = register_probe_font(&mut text_ctx).unwrap_or_else(|err| {
            eprintln!("web-spike: bundled font rejected: {err}");
            frust_text::FontFamily::SystemUi
        });
        let scene = build_scene(
            &mut text_ctx,
            family,
            f64::from(CANVAS_WIDTH),
            f64::from(CANVAS_HEIGHT),
        );
        let draws = compile_frame(&scene, CANVAS_WIDTH, CANVAS_HEIGHT);
        println!(
            "web-spike: built a {}-command scene, compiled to {draws} engine draws",
            scene.commands().len()
        );
    }
}

/// The browser half: canvas, winit event loop, surface bring-up, frame
/// submission and the logging that turns all of it into screenshot-legible
/// evidence.
///
/// Everything in here is `wasm32`-only. It is a module rather than a pile of
/// `#[cfg]` attributes so the target gate is stated once, and so the
/// wasm-only imports (`wasm_bindgen`, `web_sys`, `winit`'s web extension
/// traits) have somewhere to live that a native build never parses names
/// from.
#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;

    use frust_gpu::{ContextOptions, DownlevelProfile, RenderContext};
    use frust_reactive::{FrameWaker, ReactiveRuntime, RwSignal, TrackedScope};
    use frust_render::{
        AcquireOutcome, EncodeOutcome, FrameOutcome, SurfaceAlphaRequest, SurfaceRenderer,
    };
    use reactive_graph::traits::{Get, Set};
    use wasm_bindgen::prelude::*;
    use winit::application::ApplicationHandler;
    use winit::dpi::PhysicalSize;
    use winit::event::WindowEvent;
    use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
    use winit::platform::web::{EventLoopExtWebSys, WindowAttributesExtWebSys};
    use winit::window::{Window, WindowId};

    use super::{BASE_COLOR, CANVAS_HEIGHT, CANVAS_WIDTH, build_scene, compile_frame};

    /// The canvas `index.html` provides and `?arm=` reads off `location`.
    const CANVAS_ID: &str = "frust-canvas";
    /// The `<pre>` every logged line is mirrored into, so a screenshot or a
    /// DOM dump carries the same evidence the console does.
    const LOG_ID: &str = "log";

    /// How many times surface bring-up is attempted before the arm is called
    /// dead, and how long to wait between attempts.
    ///
    /// This is not defensive padding — it is the workaround for a measured
    /// Chrome behaviour. On Chrome 151 the **first** `navigator.gpu
    /// .requestAdapter()` on a freshly loaded page resolves to `null`, and
    /// every subsequent call on that same page resolves to the real adapter
    /// (measured directly over CDP on this rig: `default -> null`, then
    /// `{} -> nvidia/turing`, `low-power -> nvidia/turing`,
    /// `high-performance -> nvidia/turing`, `default#2 -> nvidia/turing`).
    /// It is a GPU-process warm-up race, not a capability answer, and it does
    /// not depend on the requested power preference.
    ///
    /// `frust-gpu` asks once — `RenderContext::ensure_device` goes through
    /// `wgpu::util::initialize_adapter_from_env_or_default`, which is
    /// single-shot — so a cold page reports "no compatible GPU adapter" for a
    /// machine that has one. Retrying is a *host* responsibility today, which
    /// is why it lives here rather than behind a `crates/` edit; RESULTS.md
    /// records the seam the shell will want instead.
    const BRINGUP_ATTEMPTS: u32 = 8;
    /// Delay between bring-up attempts. One task-queue turn is usually
    /// enough; this is deliberately longer so a slower rig (or a cold shader
    /// cache) is not misreported as a missing adapter.
    const BRINGUP_RETRY_MS: i32 = 150;

    /// Which wgpu backend the `wgpu::Instance` is restricted to for this run.
    ///
    /// **Not** a renderer choice: `frust-engine` renders both arms. This is
    /// the graphics API underneath it, and it is exactly the axis w0-03 was
    /// asked to compare.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Arm {
        /// `?arm=webgpu` (the default) — `wgpu::Backends::BROWSER_WEBGPU`.
        WebGpu,
        /// `?arm=webgl` — `wgpu::Backends::GL`, wgpu's WebGL2 backend on
        /// `wasm32-unknown-unknown`. `frust-gpu` must resolve
        /// [`DownlevelProfile::WebGl2`] from the reported
        /// `wgpu::Backend::Gl` with no help from this file; the probe logs
        /// the resolved profile so that is checkable rather than assumed.
        WebGl2,
    }

    impl Arm {
        /// Reads `?arm=` off `window.location.search`, defaulting to
        /// [`Arm::WebGpu`]. Unknown values fall back to the default and say
        /// so, rather than failing the run on a typo.
        fn from_query() -> Self {
            let raw = web_sys::window()
                .and_then(|window| window.location().search().ok())
                .and_then(|search| web_sys::UrlSearchParams::new_with_str(&search).ok())
                .and_then(|params| params.get("arm"))
                .unwrap_or_default();
            match raw.as_str() {
                "webgl" | "webgl2" | "gl" => Arm::WebGl2,
                "" | "webgpu" => Arm::WebGpu,
                other => {
                    log_line(&format!(
                        "arm: unrecognised ?arm={other:?}; falling back to webgpu"
                    ));
                    Arm::WebGpu
                }
            }
        }

        /// The `wgpu::Backends` mask handed to
        /// [`ContextOptions::backends`]. A single-bit mask on purpose: the
        /// point of the GL arm is that WebGPU is *not* silently picked when
        /// it happens to be available, which a two-bit mask would allow.
        fn backends(self) -> wgpu::Backends {
            match self {
                Arm::WebGpu => wgpu::Backends::BROWSER_WEBGPU,
                Arm::WebGl2 => wgpu::Backends::GL,
            }
        }

        fn label(self) -> &'static str {
            match self {
                Arm::WebGpu => "webgpu",
                Arm::WebGl2 => "webgl",
            }
        }
    }

    /// w0-04's proof mode, orthogonal to [`Arm`] (either backend can carry
    /// it): reads `?signal=timer` off `window.location.search`. When set,
    /// [`ProbeApp`] wires a real `frust_reactive::ReactiveRuntime`, gates off
    /// the continuous per-frame `request_redraw` loop the two w0-03 arms use
    /// (see [`ProbeApp::window_event`]'s `RedrawRequested` arm), and proves
    /// that a signal write — not a redraw request this file makes directly —
    /// is what wakes the one extra repaint. See RESULTS.md's w0-04 section
    /// for the full design and evidence.
    fn signal_mode_enabled() -> bool {
        web_sys::window()
            .and_then(|window| window.location().search().ok())
            .and_then(|search| web_sys::UrlSearchParams::new_with_str(&search).ok())
            .and_then(|params| params.get("signal"))
            .is_some_and(|value| value == "timer")
    }

    /// Delay before the one-shot signal write, started once the baseline
    /// frame has rendered. Long enough that a screenshot/log capture taken
    /// right after bring-up unambiguously catches the "baseline, not yet
    /// written" state before the write happens.
    const SIGNAL_DELAY_MS: i32 = 800;

    /// Wraps a wasm [`EventLoopProxy`] so it satisfies
    /// [`frust_reactive::FrameWaker`]'s `Send + Sync` bound.
    ///
    /// # Safety
    ///
    /// `wasm32-unknown-unknown` (this build has no `atomics` target feature —
    /// see `crates/frust-gpu/Cargo.toml`'s `fragile-send-sync-non-atomic-wasm`
    /// precedent, the same reasoning) has no real OS threads: this process
    /// runs entirely on the one JS event-loop thread, so nothing can ever
    /// race across a thread boundary this proxy is used from. Winit's wasm
    /// `EventLoopProxy` is not `Send`/`Sync` upstream only because `winit` is
    /// a cross-platform crate whose *native* platform impls genuinely do
    /// cross real OS threads (`frust-shell-desktop`'s own waker relies on
    /// exactly that native `Send + Sync`-ness — see `app_handler.rs`); this
    /// wrapper is the wasm-specific closing of that same gap.
    struct SendSyncProxy(EventLoopProxy<()>);
    unsafe impl Send for SendSyncProxy {}
    unsafe impl Sync for SendSyncProxy {}

    /// The live GPU state, built asynchronously and then owned by the event
    /// loop.
    struct Gpu {
        ctx: RenderContext,
        renderer: SurfaceRenderer,
        scene: frust_scene::Scene,
    }

    /// The winit application: the canvas and the arm are known up front, the
    /// window arrives at `resumed`, and the [`Gpu`] arrives later still
    /// (surface creation is `async` and the browser has no blocking
    /// executor, so it lands via `spawn_local`).
    struct ProbeApp {
        arm: Arm,
        canvas: web_sys::HtmlCanvasElement,
        window: Option<Arc<Window>>,
        gpu: Rc<RefCell<Option<Gpu>>>,
        /// `resumed` can fire more than once; the window/surface must be
        /// built exactly once.
        started: bool,
        /// Frames submitted since bring-up. Drives the log/title budget in
        /// [`ProbeApp::window_event`] — the first few frames are worth a log
        /// line each, and after that only the running count matters.
        frames: u32,
        /// w0-04 proof mode ([`signal_mode_enabled`]). When set, the
        /// `RedrawRequested` handler gates off the continuous
        /// `request_redraw` loop the two w0-03 arms use, and the counter
        /// signal / timer below drive the one expected extra repaint instead.
        signal_mode: bool,
        /// The tracked counter signal `signal_mode` writes once, `Some` only
        /// when `signal_mode` is set. `RwSignal` is `Copy`, so this is cheap
        /// to hand into the timer's `spawn_local` closure.
        counter: Option<RwSignal<u32>>,
        /// Keeps the [`TrackedScope`] that subscribed to `counter` alive for
        /// the life of the app — dropping it would unsubscribe before the
        /// write ever arrives. Never read again after construction; its job
        /// is entirely to stay alive and fire on the signal write.
        #[allow(dead_code)]
        scope: Option<TrackedScope>,
        /// Guards [`ProbeApp::start_signal_timer`] so the one-shot timer is
        /// scheduled exactly once (the baseline frame can only fire it the
        /// first time `frames` reaches 1).
        signal_timer_started: bool,
        /// Pointer/keyboard events observed while `signal_mode` is set — the
        /// acceptance criterion's "zero input events delivered" is asserted
        /// against this count, not assumed.
        input_events: u32,
        /// `now_ms()` read at `start()`'s entry, before the canvas/event-loop
        /// are touched (w0-05). The wasm-bindgen start section this runs in
        /// executes synchronously *inside* `init()`'s underlying work and
        /// returns almost immediately (it only spawns the async event
        /// loop) — so this is within a tick of `init()`'s JS promise
        /// resolving, and is what [`ProbeApp::window_event`]'s "first-frame"
        /// timing line is measured from. See RESULTS.md's w0-05 section for
        /// the exact caveat and the measured numbers.
        t_start: f64,
    }

    /// How many frames get their own log line before the mirror goes quiet.
    /// Two is enough to show the loop is a loop rather than a one-shot, and
    /// keeps the `<pre>` short enough to fit in a screenshot.
    const LOGGED_FRAMES: u32 = 2;
    /// How often (in frames) the running count is refreshed into the title.
    const TITLE_EVERY: u32 = 60;

    /// `document`, or a panic with a message the panic hook turns into a
    /// readable console error. Every caller runs after `init()` has resolved,
    /// so a missing document is a broken host page, not a race.
    fn document() -> web_sys::Document {
        web_sys::window()
            .expect("web-spike: no `window` — not running in a browser")
            .document()
            .expect("web-spike: no `document`")
    }

    /// `window.performance().now()` — a monotonic, sub-millisecond clock, in
    /// milliseconds since navigation start. `NaN` if `window`/`Performance`
    /// are unavailable (never happens under the WebDriver rig this spike is
    /// measured on, but a probe reports rather than panics).
    ///
    /// **Why this and not [`std::time::Instant`]:** `Instant::now()` panics
    /// on `wasm32-unknown-unknown` (this target has no clock syscall — see
    /// `crates/frust-engine/src/compile/mod.rs`'s `PhaseClock`, which is
    /// `perf-trace`-feature-gated and `pub(crate)`, i.e. not a hook this
    /// spike's write scope can reach even with that feature turned on). See
    /// RESULTS.md's w0-05 section for the full "no reachable `strip_us` hook"
    /// finding this timing code stands in for.
    fn now_ms() -> f64 {
        web_sys::window()
            .and_then(|window| window.performance())
            .map_or(f64::NAN, |performance| performance.now())
    }

    /// Emits one evidence line three ways: the devtools console (which
    /// Chrome's `--enable-logging=stderr` also mirrors to the process's
    /// stderr), the on-page `<pre id="log">` (so a *screenshot* carries it),
    /// and — for the most recent line — nothing else. The title is set
    /// separately by [`set_verdict`], because a title that churns per line is
    /// useless in a headless capture.
    pub(crate) fn log_line(message: &str) {
        web_sys::console::log_1(&JsValue::from_str(message));
        if let Some(pre) = document().get_element_by_id(LOG_ID) {
            let existing = pre.text_content().unwrap_or_default();
            pre.set_text_content(Some(&format!("{existing}{message}\n")));
        }
    }

    /// Writes the one-line verdict into `document.title`, which a headless
    /// screenshot run can read back over CDP without parsing the DOM.
    fn set_verdict(verdict: &str) {
        document().set_title(verdict);
        log_line(&format!("verdict: {verdict}"));
    }

    /// wasm-bindgen start section: the browser's entry point. `init()` in
    /// `index.html` resolves once this has returned, which is why it only
    /// *starts* the event loop rather than blocking on it.
    #[wasm_bindgen(start)]
    pub fn start() {
        // Read first, before anything else in this function: the earliest
        // timestamp this file can capture, and the base the w0-05
        // first-frame measurement is computed from (see `ProbeApp::t_start`
        // and RESULTS.md's w0-05 section).
        let t_start = now_ms();
        // Installed first: without it a Rust panic reaches JS as a bare
        // `unreachable` trap with no message, which would make every failure
        // mode in this probe indistinguishable.
        console_error_panic_hook::set_once();
        // Routes `frust-render`/`frust-gpu`/`wgpu`'s own `log` records into
        // the console, at Debug, so an adapter refusal or a surface
        // configuration warning is captured verbatim rather than inferred.
        let _ = console_log::init_with_level(log::Level::Debug);

        let arm = Arm::from_query();
        log_line(&format!(
            "frust w0-03 browser render probe | arm={} | renderer=frust-engine (the only one)",
            arm.label()
        ));
        set_verdict(&format!("frust w0-03 {} | starting", arm.label()));

        let canvas = match document().get_element_by_id(CANVAS_ID) {
            Some(element) => match element.dyn_into::<web_sys::HtmlCanvasElement>() {
                Ok(canvas) => canvas,
                Err(_) => {
                    set_verdict(&format!(
                        "frust w0-03 {} | FAIL: #{CANVAS_ID} is not a <canvas>",
                        arm.label()
                    ));
                    return;
                }
            },
            None => {
                set_verdict(&format!(
                    "frust w0-03 {} | FAIL: no #{CANVAS_ID} in the document",
                    arm.label()
                ));
                return;
            }
        };
        canvas.set_width(CANVAS_WIDTH);
        canvas.set_height(CANVAS_HEIGHT);

        let event_loop = match EventLoop::new() {
            Ok(event_loop) => event_loop,
            Err(err) => {
                set_verdict(&format!(
                    "frust w0-03 {} | FAIL: winit EventLoop::new: {err}",
                    arm.label()
                ));
                return;
            }
        };
        // The scene is static, so there is nothing to poll for: redraws are
        // requested explicitly after bring-up and on resize.
        event_loop.set_control_flow(ControlFlow::Wait);

        // w0-04 proof mode: wire a real `ReactiveRuntime` before the app is
        // handed to the event loop, so its `FrameWaker` can capture a proxy
        // created from this still-owned `event_loop`. Left entirely off
        // (`counter`/`scope` stay `None`) for the two w0-03 arms, so their
        // documented RESULTS.md console traces are unaffected.
        let signal_mode = signal_mode_enabled();
        let (counter, scope) = if signal_mode {
            log_line("signal: mode enabled (?signal=timer) — wiring ReactiveRuntime");
            let proxy = SendSyncProxy(event_loop.create_proxy());
            let waker: FrameWaker = Arc::new(move || {
                // `EventLoopClosed` (the loop already shut down) is the same
                // benign shutdown race `frust-shell-desktop`'s own waker
                // ignores — see `app_handler.rs`.
                let _ = proxy.0.send_event(());
            });
            let rt = ReactiveRuntime::init(waker);
            let counter = rt.with_owner(|| RwSignal::new(0u32));
            let scope = TrackedScope::new();
            // Establishes the subscription: `notify_dirty` fires the
            // FrameWaker above on this signal's next write (see
            // `frust_reactive::tracked`'s clean→dirty edge).
            scope.track(|| {
                let _ = counter.get();
            });
            log_line("signal: counter signal created and tracked (initial value 0)");
            (Some(counter), Some(scope))
        } else {
            (None, None)
        };

        // `spawn_app`, not `run_app`: winit's web backend implements
        // `run_app` by throwing a JS exception to unwind out of the caller's
        // stack, which would surface as an uncaught error out of `init()`.
        // `spawn_app` hands control back normally and drives the loop from
        // the browser's own task queue.
        event_loop.spawn_app(ProbeApp {
            arm,
            canvas,
            window: None,
            gpu: Rc::new(RefCell::new(None)),
            started: false,
            frames: 0,
            signal_mode,
            counter,
            scope,
            signal_timer_started: false,
            input_events: 0,
            t_start,
        });
    }

    impl ApplicationHandler for ProbeApp {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            if self.started {
                return;
            }
            self.started = true;

            let attributes = Window::default_attributes()
                // Adopt the page's own canvas rather than letting winit
                // create and append one: the host page owns the layout, and
                // a web shell will always be handed an element.
                .with_canvas(Some(self.canvas.clone()))
                .with_inner_size(PhysicalSize::new(CANVAS_WIDTH, CANVAS_HEIGHT));
            let window = match event_loop.create_window(attributes) {
                Ok(window) => Arc::new(window),
                Err(err) => {
                    set_verdict(&format!(
                        "frust w0-03 {} | FAIL: winit create_window: {err}",
                        self.arm.label()
                    ));
                    return;
                }
            };
            log_line("winit: window created over the page canvas");
            self.window = Some(Arc::clone(&window));

            let arm = self.arm;
            let slot = Rc::clone(&self.gpu);
            wasm_bindgen_futures::spawn_local(async move {
                // w0-05: the RenderContext/SurfaceRenderer bring-up span —
                // context creation, the retrying surface bring-up (§ 6's
                // cold-adapter race), and `frust-gpu`'s inline pipeline
                // warm-up fallback (§ 3/§ 8's "could not spawn the pipeline
                // warm-up thread ... building the listed variants inline"),
                // all inside `bring_up` below — as one span, per the card's
                // own instruction. See RESULTS.md's w0-05 section.
                let bringup_start = now_ms();
                match bring_up(arm, &window).await {
                    Ok(mut gpu) => {
                        log_line(&format!(
                            "timing: bring-up (context create + surface retries + \
                             inline pipeline warm-up fallback + font register + \
                             scene build) = {:.3}ms",
                            now_ms() - bringup_start
                        ));
                        // Draw here rather than only asking for a redraw:
                        // this guarantees at least one submitted frame even
                        // if the redraw request is coalesced away, which is
                        // the frame the screenshot is taken of.
                        let outcome = draw(&mut gpu, true);
                        *slot.borrow_mut() = Some(gpu);
                        set_verdict(&format!("frust w0-03 {} | {outcome}", arm.label()));
                        window.request_redraw();
                    }
                    Err(err) => {
                        log_line(&format!(
                            "timing: bring-up FAILED after {:.3}ms",
                            now_ms() - bringup_start
                        ));
                        set_verdict(&format!("frust w0-03 {} | FAIL: {err}", arm.label()));
                    }
                }
            });
        }

        fn window_event(
            &mut self,
            _event_loop: &ActiveEventLoop,
            _window_id: WindowId,
            event: WindowEvent,
        ) {
            match event {
                WindowEvent::RedrawRequested => {
                    // `try_borrow_mut`: bring-up owns the slot until it
                    // publishes, and a redraw can be delivered while that
                    // future is still in flight. Skipping is correct — the
                    // future draws the first frame itself.
                    if let Ok(mut slot) = self.gpu.try_borrow_mut()
                        && let Some(gpu) = slot.as_mut()
                    {
                        let outcome = draw(gpu, self.frames < LOGGED_FRAMES);
                        self.frames += 1;
                        if self.frames <= LOGGED_FRAMES {
                            log_line(&format!("frame {}: {outcome}", self.frames));
                        }
                        if self.frames == 1 {
                            // w0-05 first-frame time: `t_start` (`start()`'s
                            // entry, ~= init() resolution — see the field's
                            // doc comment) to this, the first RAF-driven
                            // frame `RedrawRequested` presented. The earlier
                            // draw `resumed`'s own continuation performs
                            // (this file's "guarantee at least one submitted
                            // frame" comment) is not itself counted as a
                            // `frame N` — its cost is inside the `timing:
                            // bring-up` line above instead, since it runs
                            // before this handler ever sees a
                            // `RedrawRequested` event.
                            log_line(&format!(
                                "timing: first-frame (start() entry -> first \
                                 RAF-driven `frame 1` presented) = {:.3}ms",
                                now_ms() - self.t_start
                            ));
                        }
                        if self.frames % TITLE_EVERY == 0 {
                            document().set_title(&format!(
                                "frust w0-03 {} | {outcome} | frame {}",
                                self.arm.label(),
                                self.frames
                            ));
                        }

                        if self.signal_mode {
                            // The baseline frame: schedule the one-shot
                            // signal write and stop here — no
                            // `request_redraw` below, unlike the two w0-03
                            // arms (see the `if !self.signal_mode` guard
                            // after this block).
                            if self.frames == 1 && !self.signal_timer_started {
                                self.signal_timer_started = true;
                                self.start_signal_timer();
                            } else if self.frames == 2 {
                                // The signal-triggered extra repaint. Report
                                // the whole acceptance criterion in one line:
                                // exactly one baseline frame, exactly one
                                // frame after the write, and how many
                                // pointer/key events (expected zero) arrived
                                // while waiting for it.
                                set_verdict(&format!(
                                    "frust w0-04 signal | PROVEN: baseline=1 frame, \
                                     +1 repaint after the signal write, \
                                     input_events={}",
                                    self.input_events
                                ));
                            }
                        }
                    }
                    // Keep asking — but ONLY for the two w0-03 arms
                    // (`signal_mode` unset). A canvas is only guaranteed to
                    // hold the pixels the compositor last consumed: with
                    // WebGL's default `preserveDrawingBuffer: false` — and in
                    // practice with WebGPU's presented texture too — a
                    // single one-shot frame drawn outside the browser's
                    // animation callback can be composited away before a
                    // screenshot is taken, which is exactly what a
                    // one-frame version of this probe produced (a blank
                    // canvas under a `RENDERED` verdict). winit's web
                    // backend services `request_redraw` from
                    // `requestAnimationFrame`, so re-requesting here is the
                    // ordinary animation loop, and it is also what any real
                    // shell does.
                    //
                    // `signal_mode` gates this off on purpose: the proof is
                    // that the ONLY extra repaint comes from the signal
                    // write's `FrameWaker` → `user_event` path below, not
                    // from this file re-requesting one itself. Without the
                    // gate, this continuous loop would mask the signal path
                    // entirely — `frame 2` would arrive regardless of
                    // whether the write ever happened.
                    if !self.signal_mode
                        && let Some(window) = self.window.as_ref()
                    {
                        window.request_redraw();
                    }
                }
                WindowEvent::CloseRequested => {
                    log_line("winit: close requested (ignored — the probe has no exit path)");
                }
                // The acceptance criterion's other half: the extra repaint
                // must come with zero pointer/key events, not merely arrive
                // alongside an unrelated one. Only counted (and only worth
                // counting) in `signal_mode` — the two w0-03 arms never
                // claimed anything about input, and fall through to the
                // catch-all arm below unchanged.
                WindowEvent::CursorMoved { .. }
                | WindowEvent::MouseInput { .. }
                | WindowEvent::MouseWheel { .. }
                | WindowEvent::KeyboardInput { .. }
                | WindowEvent::Touch(_)
                | WindowEvent::CursorEntered { .. }
                | WindowEvent::CursorLeft { .. }
                    if self.signal_mode =>
                {
                    self.input_events += 1;
                    log_line(&format!(
                        "signal: unexpected input event observed (count={})",
                        self.input_events
                    ));
                }
                _ => {}
            }
        }

        /// The `signal_mode` proof's other half: `frust_reactive`'s
        /// `FrameWaker` (installed in `start()`) sends this through the
        /// `EventLoopProxy` when the counter signal's write fires
        /// `TrackedScope`'s clean→dirty edge — this is the ONLY place in
        /// `signal_mode` that calls `request_redraw`, so a `frame 2` log
        /// line is evidence the signal path (not this file re-asking on its
        /// own, which is gated off — see `window_event`) produced it.
        fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: ()) {
            log_line(
                "signal: FrameWaker fired via the EventLoopProxy -> requesting the \
                 one post-write repaint",
            );
            if let Some(window) = self.window.as_ref() {
                window.request_redraw();
            }
        }
    }

    impl ProbeApp {
        /// Schedules the `signal_mode` proof's one-shot signal write, called
        /// once the baseline frame has rendered (see `window_event`'s
        /// `RedrawRequested` arm). The write itself never calls
        /// `request_redraw` — that only happens in
        /// [`ApplicationHandler::user_event`] above, reached solely through
        /// the installed `FrameWaker`.
        fn start_signal_timer(&self) {
            let Some(counter) = self.counter else {
                return;
            };
            log_line(&format!(
                "signal: baseline frame rendered; scheduling a one-shot \
                 {SIGNAL_DELAY_MS}ms timer to write the tracked counter signal"
            ));
            wasm_bindgen_futures::spawn_local(async move {
                sleep_ms(SIGNAL_DELAY_MS).await;
                // The write: `TrackedScope::track`'s subscription (in
                // `start()`) turns this into a `mark_dirty` call, which fires
                // the process-wide `FrameWaker` on the clean→dirty edge (see
                // `frust_reactive::tracked`) — nothing here touches winit
                // directly.
                counter.set(1);
                log_line(
                    "signal: wrote counter signal (0 -> 1) — this write's TrackedScope \
                     dirty edge is what fires the FrameWaker",
                );
            });
        }
    }

    /// Awaits `ms` milliseconds of wall-clock time.
    ///
    /// `std::thread::sleep` does not exist on `wasm32-unknown-unknown`, and
    /// nothing in the graph carries a timer future, so this is the
    /// `setTimeout`-into-`Promise`-into-`JsFuture` idiom spelled out. Used
    /// only by the bring-up retry.
    async fn sleep_ms(ms: i32) {
        let promise = js_sys::Promise::new(&mut |resolve, _reject| {
            if let Some(window) = web_sys::window() {
                let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
            }
        });
        let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
    }

    /// Brings the surface online, retrying past Chrome's cold-page adapter
    /// race (see [`BRINGUP_ATTEMPTS`]).
    ///
    /// Every attempt is logged with its verbatim error, so the retry can
    /// never disguise a genuine refusal as a slow one: a real NO-GO shows the
    /// same message [`BRINGUP_ATTEMPTS`] times and says which attempt it gave
    /// up on.
    async fn create_surface_with_retry(
        ctx: &mut RenderContext,
        renderer: &mut SurfaceRenderer,
        window: &Arc<Window>,
    ) -> Result<u32, String> {
        let mut last = String::new();
        for attempt in 1..=BRINGUP_ATTEMPTS {
            match renderer
                .on_surface_created(
                    ctx,
                    Arc::clone(window),
                    CANVAS_WIDTH,
                    CANVAS_HEIGHT,
                    SurfaceAlphaRequest::Opaque,
                )
                .await
            {
                Ok(()) => return Ok(attempt),
                Err(err) => {
                    last = format!("{err:#}");
                    log_line(&format!(
                        "surface: attempt {attempt}/{BRINGUP_ATTEMPTS} failed: {last}"
                    ));
                    if attempt < BRINGUP_ATTEMPTS {
                        sleep_ms(BRINGUP_RETRY_MS).await;
                    }
                }
            }
        }
        Err(format!(
            "on_surface_created: gave up after {BRINGUP_ATTEMPTS} attempts; last error: {last}"
        ))
    }

    /// Creates the backend-restricted context, brings the canvas surface
    /// online, logs everything the card asked to be recorded per arm, and
    /// builds the scene.
    ///
    /// Returns the error text a failing layer produced, verbatim, so the
    /// NO-GO half of the acceptance criterion is satisfiable without
    /// guesswork about which layer refused.
    async fn bring_up(arm: Arm, window: &Arc<Window>) -> Result<Gpu, String> {
        let mut ctx = RenderContext::with_options(ContextOptions {
            device_label: format!("frust w0-03 {} device", arm.label()),
            // The whole point of the GL arm. `RenderContext::new()` would
            // take `wgpu::Backends::from_env()`, which cannot resolve in a
            // browser (no environment), so it would fall back to every
            // compiled-in backend and quietly prefer WebGPU where available.
            backends: Some(arm.backends()),
        });
        log_line(&format!(
            "wgpu: instance restricted to {:?}",
            arm.backends()
        ));

        let mut renderer = SurfaceRenderer::new();
        let attempts = create_surface_with_retry(&mut ctx, &mut renderer, &window).await?;
        log_line(&format!(
            "surface: online after {attempts} attempt(s), phase={:?}",
            renderer.phase()
        ));

        let caps = ctx
            .caps()
            .ok_or_else(|| "no TierCaps after surface creation".to_string())?;
        log_line(&format!("adapter: {}", caps.adapter_name));
        log_line(&format!("backend: {:?}", caps.backend));
        log_line(&format!("downlevel profile: {:?}", caps.downlevel_profile));
        log_line(&format!(
            "limits: max_texture_dimension_2d={} max_texture_array_layers={} \
             max_bind_groups={} max_uniform_buffer_binding_size={} \
             min_uniform_buffer_offset_alignment={} max_vertex_attributes={}",
            caps.max_texture_dimension_2d,
            caps.max_texture_array_layers,
            caps.max_bind_groups,
            caps.max_uniform_buffer_binding_size,
            caps.min_uniform_buffer_offset_alignment,
            caps.max_vertex_attributes,
        ));
        log_line(&format!(
            "caps: storage_buffers={} timestamp_query={} resource_texture_dim={} \
             downlevel_flags={:?}",
            caps.has_storage_buffers,
            caps.has_timestamp_query,
            caps.resource_texture_dim,
            caps.downlevel_flags,
        ));
        // The GL arm's real assertion: the WebGl2 profile must engage from
        // the resolved backend alone, with nothing in this file selecting it.
        let expected = match arm {
            Arm::WebGpu => DownlevelProfile::Full,
            Arm::WebGl2 => DownlevelProfile::WebGl2,
        };
        if caps.downlevel_profile == expected {
            log_line(&format!(
                "profile check: OK — {:?} on {:?}, resolved by frust-gpu, not by the probe",
                caps.downlevel_profile, caps.backend
            ));
        } else {
            log_line(&format!(
                "profile check: UNEXPECTED — got {:?}, expected {expected:?} for arm={}",
                caps.downlevel_profile,
                arm.label()
            ));
        }

        let mut text_ctx = frust_text::TextContext::new();
        let family = match super::register_probe_font(&mut text_ctx) {
            Ok(family) => {
                log_line(&format!("font: registered bundled face -> {family:?}"));
                family
            }
            Err(err) => {
                // Not fatal: the shapes still prove the raster path, and the
                // text failure is now on the record instead of invisible.
                log_line(&format!(
                    "font: FAILED to register the bundled face ({err}); \
                     falling back to SystemUi (fontique has no system fonts on wasm, \
                     so expect zero glyphs)"
                ));
                frust_text::FontFamily::SystemUi
            }
        };

        let scene = build_scene(
            &mut text_ctx,
            family,
            f64::from(CANVAS_WIDTH),
            f64::from(CANVAS_HEIGHT),
        );
        let draws = compile_frame(&scene, CANVAS_WIDTH, CANVAS_HEIGHT);
        log_line(&format!(
            "scene: {} commands -> {draws} engine draws (CPU compile)",
            scene.commands().len()
        ));

        Ok(Gpu {
            ctx,
            renderer,
            scene,
        })
    }

    /// One frame through the engine's three-phase seam — `encode` →
    /// `acquire` → `submit` — mirroring `frust-shell-desktop`'s
    /// `InlineExecutor` on a single thread, which is all a browser has.
    ///
    /// Answers a short human-readable outcome rather than a `Result`: every
    /// arm of the state machine is reportable evidence here, including the
    /// ones a shell would treat as "retry later".
    ///
    /// w0-05 timing: each of the three phases is wrapped in `now_ms()` and,
    /// when `log_timing` is set, reported as one `timing:` line. The card's
    /// own instruction was to wrap `encode()` (per
    /// `crates/frust-render/src/renderer.rs`'s own doc comment, `encode` is a
    /// scene memcpy and nothing else — `encode_us` here should read close to
    /// zero); `acquire`/`submit` are timed alongside it because that same doc
    /// comment says the engine's real per-frame CPU work — strip generation
    /// and paint encoding — plus the GPU queue-submit run inside `submit`,
    /// which is why `submit_us` is this spike's proxy for the card's
    /// `strip_us` (no public, spike-reachable hook isolates strip generation
    /// alone from the GPU submit — see RESULTS.md's w0-05 section).
    fn draw(gpu: &mut Gpu, log_timing: bool) -> String {
        let Gpu {
            ctx,
            renderer,
            scene,
        } = gpu;
        let encode_start = now_ms();
        let encode_outcome = renderer.encode(ctx, scene, BASE_COLOR);
        let encode_ms = now_ms() - encode_start;
        match encode_outcome {
            Err(err) => format!("FAIL: encode: {err:#}"),
            Ok(EncodeOutcome::Skipped) => "skipped: no renderable surface at encode".to_string(),
            Ok(EncodeOutcome::Encoded) => {
                let acquire_start = now_ms();
                let acquire_outcome = renderer.acquire(ctx);
                let acquire_ms = now_ms() - acquire_start;
                match acquire_outcome {
                    Err(err) => format!("FAIL: acquire: {err:#}"),
                    Ok(AcquireOutcome::Reconfigured) => {
                        "reconfigured: swapchain was stale".to_string()
                    }
                    Ok(AcquireOutcome::Lost) => "surface lost at acquire".to_string(),
                    Ok(AcquireOutcome::Skipped) => "skipped: transient acquire failure".to_string(),
                    Ok(AcquireOutcome::Acquired) => {
                        let submit_start = now_ms();
                        let submit_outcome = renderer.submit(ctx);
                        let submit_ms = now_ms() - submit_start;
                        if log_timing {
                            log_line(&format!(
                                "timing: encode={encode_ms:.3}ms (scene memcpy) \
                                 acquire={acquire_ms:.3}ms (swapchain/vsync wait) \
                                 submit={submit_ms:.3}ms (strip generation + GPU \
                                 encode/queue-submit — this spike's `strip_us` proxy)"
                            ));
                        }
                        match submit_outcome {
                            Err(err) => format!("FAIL: submit: {err:#}"),
                            Ok(FrameOutcome::Rendered) => "RENDERED".to_string(),
                            Ok(other) => format!("submitted but not presented: {other:?}"),
                        }
                    }
                }
            }
        }
    }
}
