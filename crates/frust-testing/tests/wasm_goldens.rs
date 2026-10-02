#![cfg(all(target_arch = "wasm32", feature = "webgl"))]

//! The engine's downlevel profile proved in a REAL WebGL2 context: every unit
//! corpus case rendered by `frust-engine` inside a browser, on a
//! `wgpu::Backend::Gl` adapter, and held to the same bar the host engine gate
//! holds it to.
//!
//! ```text
//! CHROMEDRIVER_REMOTE=http://localhost:9517 \
//!   CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
//!   cargo test -p frust-testing --target wasm32-unknown-unknown \
//!     --features webgl --release --test wasm_goldens
//! ```
//!
//! Compiled ONLY for `wasm32` under the non-default `webgl` feature (this
//! file's own crate-level `cfg`), so the ordinary host gate compiles it to
//! nothing and no native graph changes shape.
//!
//! # What this proves that a desktop run cannot
//!
//! `FRUST_ENGINE_DOWNLEVEL=1` already rehearses the WebGL2 ceiling against a
//! desktop adapter — it relabels the profile and clamps the limits, but the
//! pixels still come out of Vulkan or Metal. Here the pixels come out of an
//! actual WebGL2 context: real GLSL, real GLES-3.0 texture/uniform ceilings,
//! real driver. `DownlevelProfile::WebGl2` engages automatically on the
//! `Gl` backend (`frust_gpu::caps`'s `DownlevelProfile::resolve`) rather than
//! being asked for, and [`unit_corpus_renders_on_webgl2`] asserts the resolved
//! profile off the probed [`TierCaps`] before it renders anything.
//!
//! # The three assertions each case carries
//!
//! 1. **Probes** — `CorpusCase::probes`, absolute pixel expectations that
//!    hold on every backend independently of any stored image, checked against
//!    the PREMULTIPLIED frame the engine natively produces. A golden answers
//!    "did this change?"; a probe answers "is this right?", and on a brand-new
//!    backend the second question is the load-bearing one.
//! 2. **The cross-arm comparison** — the WebGL2 frame against `vello_cpu`
//!    0.2.0, at each case's own `CaseSpec::tolerance` plus the reviewed
//!    [`ESCALATIONS`] rows below. This is exactly the comparison
//!    `tests/engine_goldens.rs` calls P1, re-run in the browser, and it is
//!    made on the premultiplied frames both arms natively produce for the same
//!    reason that file's `render_raw` does it that way: un-premultiplying
//!    divides each colour channel by its own alpha, so at the alpha 1-11
//!    coverage along a hairline's end cap it turns a one-level disagreement
//!    into a 255-level one, and no tolerance can tell that apart from a real
//!    regression.
//! 3. **Reference identity** — the browser's own `vello_cpu` arm, straightened,
//!    against the committed `testing/goldens/cpu/` baseline for the same case,
//!    PIXEL-EXACT. This is what makes assertion 2 mean something: without it,
//!    a browser-local drift in the reference arm would move the bar the engine
//!    is measured against and the suite would still come out green. With it,
//!    the frame the WebGL2 engine is compared to is provably the same frame the
//!    host gate compares against.
//!
//! # Why the goldens are inlined
//!
//! There is no filesystem in a browser. Every baseline this file needs is a
//! byte constant in [`GOLDENS`], `include_bytes!` of the committed PNG, so the
//! reference travels inside the wasm module rather than being fetched at run
//! time — no server layout to get right, no way for a run to silently compare
//! against a stale copy, and the bytes are the same ones `git` tracks.
//!
//! # Why the device is built here rather than through `HeadlessRenderer`
//!
//! `frust_render::HeadlessRenderer` is the harness every other engine golden
//! renders through, and this file deliberately mirrors it step for step —
//! the same instance descriptor, the same `TierCaps::probe`, the same
//! `frust_render::engine_support` capability gate in the same position, the
//! same production-shared `frust_gpu::test_device_limits` derivation, the same
//! `Rgba8Unorm` target, the same validation error scope around the render, the
//! same `frust_gpu::HeadlessTarget` readback. It cannot BE that harness on this
//! target for one structural reason: `HeadlessRenderer::new` resolves its
//! adapter through `wgpu::util::initialize_adapter_from_env_or_default(&
//! instance, None)`, and on wasm32 the GL backend enumerates no adapter at all
//! without a canvas-backed `compatible_surface`
//! (`wgpu_hal::gles::web`'s `enumerate_adapters` returns an empty vector for a
//! `None` surface hint, and `wgpu_core`'s `request_adapter` passes
//! `desc.compatible_surface` straight into it). A browser run therefore has to
//! own a canvas and hand it in as the hint, which that constructor has no
//! parameter for. Teaching `HeadlessOptions` a wasm32 surface hint is the
//! tracked follow-up; it is a `frust-render` change, out of this file's scope.
//!
//! # Alpha
//!
//! Frames come back PREMULTIPLIED, as `frust_gpu::HeadlessTarget` reads them
//! and as the engine's strip pipelines blend them. The only conversion is
//! [`straighten_alpha`], applied where a stored PNG (straight alpha, always)
//! is on the other side of the comparison.

use std::io::Cursor;

use image::{ImageFormat, RgbaImage};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

use frust_engine::{EngineRenderer, EngineTarget, OutputAlpha};
use frust_gpu::{DownlevelProfile, HeadlessTarget, TierCaps, test_device_limits};
use frust_testing::case::{CaseSpec, Tolerance};
use frust_testing::corpus::{straighten_alpha, unit_cases};
use frust_testing::diff::{DiffOutcome, DiffReport, diff_images};
use frust_testing::oracle_cpu::CpuOracle;
use frust_testing::render::{AlphaKind, BackendMeta, RenderSpec, RenderedImage, SceneRenderer};

wasm_bindgen_test_configure!(run_in_browser);

/// The colour format every frame here targets — `frust-engine`'s own
/// offscreen format, renderable on every wgpu backend, so a readback's channel
/// order needs no per-backend correction. The same constant
/// `frust_render::headless` and `frust_testing::oracle_engine` use.
const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// The size of the canvas that exists only so wgpu's WebGL2 backend has a
/// surface to enumerate an adapter against. Nothing is ever drawn to it —
/// every pixel this file asserts on comes from an offscreen
/// [`HeadlessTarget`] — so it is deliberately tiny.
const ADAPTER_PROBE_CANVAS: u32 = 1;

/// How many event-loop turns [`WebGl2Harness::read_back`] gives one frame's
/// readback before it gives up.
///
/// Generous on purpose: the bound exists so a genuinely stuck fence fails with
/// a message instead of hanging until the harness's own script timeout kills
/// the session with nothing to read, not to express how long a readback should
/// take. A 64x64 unit case settles in single-digit turns on every adapter this
/// has been run against.
const MAX_READBACK_TURNS: u32 = 600;

/// Yields to the browser's event loop once, so work the GPU has not finished
/// yet can progress before the next poll.
///
/// `setTimeout(0)` rather than `requestAnimationFrame`: a headless run, or a
/// backgrounded tab in a headed one, throttles animation frames hard or stops
/// delivering them altogether, and a readback loop that depends on them would
/// stall for a reason that has nothing to do with the GPU.
async fn yield_to_event_loop() {
    let promise = web_sys::js_sys::Promise::new(&mut |resolve, _reject| {
        web_sys::window()
            .expect("a `run_in_browser` test always has a window")
            .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 0)
            .expect("scheduling a zero-delay timeout");
    });
    let _ = JsFuture::from(promise).await;
}

/// Strips wgpu's mandatory `copy_texture_to_buffer` row padding, returning
/// tightly packed RGBA8 rows.
///
/// The same arithmetic `frust_gpu::HeadlessTarget::read_back` does; see
/// [`WebGl2Harness::read_back`] for why this file cannot call that method.
fn strip_row_padding(padded: &[u8], width: u32, height: u32) -> Vec<u8> {
    let tight = (width * 4) as usize;
    let stride = padded_bytes_per_row(width) as usize;
    let mut rows = Vec::with_capacity(tight * height as usize);
    for row in 0..height as usize {
        let start = row * stride;
        rows.extend_from_slice(&padded[start..start + tight]);
    }
    rows
}

/// The `bytes_per_row` a `copy_texture_to_buffer` of a `width`-texel RGBA8 row
/// must use: the tight row length rounded up to wgpu's mandatory alignment.
fn padded_bytes_per_row(width: u32) -> u32 {
    (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
}

/// Every committed `cpu`-class baseline the unit corpus has, inlined as byte
/// constants.
///
/// One row per case in [`unit_cases`] that the engine draws in full. There is
/// no row for `unit-shader-quad`: its subject is `Command::ShaderQuad`, whose
/// pixels come from a fragment-shader pre-pass neither the engine's golden
/// path nor `vello_cpu` drives, so it is deferred on the host engine gate for
/// exactly the same reason and has no baseline on any class. A case in the
/// corpus with no row here is skipped and reported, never silently dropped —
/// see [`every_engine_drawable_unit_case_has_an_inlined_baseline`].
const GOLDENS: &[(&str, &[u8])] = &[
    (
        "unit-fill-rect",
        include_bytes!("../../../testing/goldens/cpu/unit-fill-rect.png"),
    ),
    (
        "unit-rounded-rect",
        include_bytes!("../../../testing/goldens/cpu/unit-rounded-rect.png"),
    ),
    (
        "unit-stroke-line",
        include_bytes!("../../../testing/goldens/cpu/unit-stroke-line.png"),
    ),
    (
        "unit-glyph-run",
        include_bytes!("../../../testing/goldens/cpu/unit-glyph-run.png"),
    ),
    (
        "unit-clip-rect",
        include_bytes!("../../../testing/goldens/cpu/unit-clip-rect.png"),
    ),
    (
        "unit-clip-rounded",
        include_bytes!("../../../testing/goldens/cpu/unit-clip-rounded.png"),
    ),
    (
        "unit-clip-balance",
        include_bytes!("../../../testing/goldens/cpu/unit-clip-balance.png"),
    ),
    (
        "unit-image",
        include_bytes!("../../../testing/goldens/cpu/unit-image.png"),
    ),
    (
        "unit-blur-rrect",
        include_bytes!("../../../testing/goldens/cpu/unit-blur-rrect.png"),
    ),
    (
        "unit-layer-alpha",
        include_bytes!("../../../testing/goldens/cpu/unit-layer-alpha.png"),
    ),
    (
        "unit-layer-balance",
        include_bytes!("../../../testing/goldens/cpu/unit-layer-balance.png"),
    ),
    (
        "unit-layer-sibling-fan",
        include_bytes!("../../../testing/goldens/cpu/unit-layer-sibling-fan.png"),
    ),
    (
        "unit-layer-nested-pair",
        include_bytes!("../../../testing/goldens/cpu/unit-layer-nested-pair.png"),
    ),
    (
        "unit-clear-rect",
        include_bytes!("../../../testing/goldens/cpu/unit-clear-rect.png"),
    ),
    (
        "unit-path-fill",
        include_bytes!("../../../testing/goldens/cpu/unit-path-fill.png"),
    ),
    (
        "unit-path-stroke",
        include_bytes!("../../../testing/goldens/cpu/unit-path-stroke.png"),
    ),
    (
        "unit-path-dashed",
        include_bytes!("../../../testing/goldens/cpu/unit-path-dashed.png"),
    ),
    (
        "unit-snapshot-bracket",
        include_bytes!("../../../testing/goldens/cpu/unit-snapshot-bracket.png"),
    ),
    (
        "unit-snapshot-balance",
        include_bytes!("../../../testing/goldens/cpu/unit-snapshot-balance.png"),
    ),
];

/// The one unit case whose subject the engine's golden path does not draw, and
/// why. Kept as a named constant rather than an inline string so
/// [`every_engine_drawable_unit_case_has_an_inlined_baseline`] can say which
/// omission is sanctioned and which is a corpus addition nobody wired up.
const DEFERRED_CASES: &[(&str, &str)] = &[(
    "unit-shader-quad",
    "a fragment-shader pre-pass neither this suite's engine arm nor `vello_cpu` drives, so there \
     is no reference on any backend — deferred on the host engine gate for the identical reason",
)];

/// Per-case widenings off each case's own `CaseSpec::tolerance`, each with
/// the reason it is not the default's fault.
///
/// Same contract as `tests/engine_goldens.rs`'s own escalation table, and
/// deliberately the same shape: `docs/TESTING.md`'s Comparison section puts
/// thresholds on "the golden class or named test, not an ad hoc retry path",
/// and nothing in this file re-runs a comparison at a looser threshold. A row
/// exists here or it does not exist.
///
/// # The hint-policy row, and why it is inherited rather than re-measured
///
/// `unit-glyph-run` is the one unit case where the two arms legitimately
/// disagree by more than a rounding step. `frust-engine`'s
/// `SceneCompiler::for_caps` turns hinting ON for a desktop-tier adapter
/// (`hint_text = !is_mobile_tier(caps)`) while `CpuOracle` hints OFF
/// unconditionally, so a reference cannot vary with a per-size, per-target
/// adjustment. That is the documented hint policy on both sides, not a defect
/// on either: a hinted outline snaps to the pixel grid a fraction of a pixel
/// away from an unhinted one, which a solid-colour glyph reads as a channel
/// delta at its own antialiased edge and nowhere else.
///
/// The number is the host engine gate's own row for this case, carried over
/// unchanged, and that is deliberate — this browser gate is measurably wider
/// than it needs to be on the two adapters it has run against:
///
/// | adapter | measured max delta |
/// |---|---|
/// | `ANGLE (NVIDIA T400 4GB/PCIe/SSE2, OpenGL 4.5.0)` | `[18, 18, 18, 0]` |
/// | `ANGLE (SwiftShader Device (Subzero))` | `[18, 18, 18, 0]` |
///
/// Both are far inside 104, and both report 0 px differing. The row is NOT
/// tightened to 18 on the strength of one rig: hinting is exactly the thing
/// that varies with the driver's own rasterization, this gate has seen two
/// ANGLE backends on one machine and no other browser at all, and a browser
/// gate that is STRICTER than the host gate for the identical rasterizer
/// disagreement would go red on a non-regression. Tighten it when a second
/// machine's numbers are in, and record them here the way these two are.
const ESCALATIONS: &[(&str, Tolerance, &str)] = &[(
    "unit-glyph-run",
    Tolerance {
        channel: 104,
        alpha: 2,
        diff_pixels: 0,
    },
    "hint-policy row: the engine hints this desktop-tier adapter's text while `vello_cpu` hints \
     nothing, so the two `Hello` outlines land a fraction of a pixel apart along their own \
     antialiased edges. The value is the host engine gate's reviewed row for this same case and \
     same comparison, inherited rather than re-derived — see this table's docs for the browser \
     numbers measured under it",
)];

/// The tolerance `name` is compared at: its own `CaseSpec::tolerance` unless
/// [`ESCALATIONS`] carries a reviewed row for it.
fn tolerance_for(spec: &CaseSpec) -> Tolerance {
    ESCALATIONS
        .iter()
        .find(|(name, _, _)| *name == spec.name)
        .map_or(spec.tolerance, |(_, tolerance, _)| *tolerance)
}

/// The inlined `cpu`-class baseline for `name`, or `None` when the case has no
/// row in [`GOLDENS`].
fn golden_for(name: &str) -> Option<&'static [u8]> {
    GOLDENS
        .iter()
        .find(|(case, _)| *case == name)
        .map(|(_, bytes)| *bytes)
}

/// Writes one line to the browser console.
///
/// `println!` is a silent no-op on `wasm32-unknown-unknown` — std's stdout is
/// the `unsupported` shim there, which accepts every byte and drops it — so a
/// line that is not written here is not written anywhere, and the run's
/// evidence (resolved adapter, downlevel profile, per-case measured delta)
/// would exist only in the head of whoever watched it.
fn log(line: &str) {
    web_sys::console::log_1(&wasm_bindgen::JsValue::from_str(line));
}

/// A [`RenderedImage`]'s pixels as the `image` buffer every comparator here
/// takes.
fn to_rgba(image: &RenderedImage) -> RgbaImage {
    RgbaImage::from_raw(image.width, image.height, image.rgba8.clone())
        .expect("a RenderedImage's buffer is exactly width * height * 4 bytes")
}

/// A committed baseline PNG decoded into the same buffer type.
fn decode_golden(name: &str, bytes: &[u8]) -> RgbaImage {
    image::load_from_memory_with_format(bytes, ImageFormat::Png)
        .unwrap_or_else(|err| panic!("inlined baseline for `{name}` is not a decodable PNG: {err}"))
        .to_rgba8()
}

/// A [`DiffReport`] as the one line every gate here prints.
fn summarize(report: &DiffReport) -> String {
    format!(
        "{} px differing ({:.4}%), max delta {:?}, mean abs {:?}",
        report.pixel_count,
        report.mismatched_percent,
        report.max_difference,
        report
            .mean_abs_error
            .map(|value| (value * 1000.0).round() / 1000.0),
    )
}

/// Appends a failing comparison's triptych to the document as an `<img>`, with
/// a caption naming the case and which comparison failed.
///
/// The whole point of running headed (`NO_HEADLESS=1`, or a browser opened at
/// the runner's URL): a headless run reports numbers, and a numeric report of
/// a rasterization disagreement is nearly unreadable. The triptych is
/// `expected | diff marker | actual`, exactly as [`diff_images`] lays it out.
///
/// Every step is best-effort and silent on failure. A missing document, a
/// refused `btoa`, an encoder error — none of them are the assertion, and a
/// panic raised while reporting a failure would replace the real message with
/// a worse one.
fn append_triptych(case: &str, comparison: &str, triptych: &RgbaImage) {
    let mut png = Vec::new();
    if triptych
        .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .is_err()
    {
        return;
    }
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };
    let Some(body) = document.body() else {
        return;
    };
    // `btoa` takes a string whose every code point fits in one byte, which is
    // precisely what mapping each PNG byte to the `char` of the same value
    // produces — no base64 crate, and no second encoder to keep correct.
    let latin1: String = png.iter().map(|&byte| char::from(byte)).collect();
    let Ok(base64) = window.btoa(&latin1) else {
        return;
    };

    if let Ok(caption) = document.create_element("p") {
        caption.set_text_content(Some(&format!(
            "{case} — {comparison} (expected | diff | actual)"
        )));
        let _ = body.append_child(&caption);
    }
    if let Ok(element) = document.create_element("img")
        && let Ok(image) = element.dyn_into::<web_sys::HtmlImageElement>()
    {
        image.set_alt(&format!("{case} {comparison} triptych"));
        image.set_src(&format!("data:image/png;base64,{base64}"));
        let _ = body.append_child(&image);
    }
}

/// One offscreen `frust-engine` renderer on a real WebGL2 adapter.
///
/// A step-for-step mirror of `frust_render::HeadlessRenderer` — see this
/// file's module docs for the one structural reason it cannot be that type on
/// this target, and for everything the two do identically.
struct WebGl2Harness {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: EngineRenderer,
    caps: TierCaps,
    meta: BackendMeta,
    /// Kept alive for the harness's whole life: it is the adapter's surface
    /// hint, and dropping it drops the WebGL2 context the device was created
    /// against.
    _surface: wgpu::Surface<'static>,
    target: Option<HeadlessTarget>,
}

impl WebGl2Harness {
    /// Resolves a WebGL2 adapter, runs it past the engine's own capability
    /// gate, and creates the device and renderer every later
    /// [`render`](Self::render) reuses.
    async fn new() -> Self {
        let document = web_sys::window()
            .and_then(|window| window.document())
            .expect("a `run_in_browser` test always has a document");
        let canvas = document
            .create_element("canvas")
            .expect("creating a canvas element")
            .dyn_into::<web_sys::HtmlCanvasElement>()
            .expect("a `canvas` element is an HtmlCanvasElement");
        canvas.set_width(ADAPTER_PROBE_CANVAS);
        canvas.set_height(ADAPTER_PROBE_CANVAS);

        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
        // GL only, deliberately. Chrome exposes WebGPU as well, and this whole
        // suite exists to exercise the WebGL2 profile: letting wgpu pick would
        // silently produce a `Full`-profile run that passes every assertion
        // below while proving none of what the suite is for.
        descriptor.backends = wgpu::Backends::GL;
        let instance = wgpu::Instance::new(descriptor);

        let surface = instance
            .create_surface(wgpu::SurfaceTarget::Canvas(canvas))
            .expect("wgpu can create a WebGL2 surface on a fresh canvas");
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .expect(
                "no WebGL2 adapter: this browser exposes no `wgpu::Backend::Gl` adapter for the \
                 probe canvas, so there is nothing for this suite to run against",
            );

        let info = adapter.get_info();
        let driver = if info.driver_info.is_empty() {
            info.driver.clone()
        } else if info.driver.is_empty() {
            info.driver_info.clone()
        } else {
            format!("{} ({})", info.driver, info.driver_info)
        };
        let meta = BackendMeta {
            backend: info.backend.to_str().to_string(),
            adapter: info.name.clone(),
            driver,
            device_kind: "gpu".to_string(),
        };

        // The same capability gate a real surface asks at surface creation, in
        // the same position `HeadlessRenderer::new` asks it: an adapter the
        // engine cannot drive is refused HERE, before it can produce a pixel,
        // and in the identical words.
        frust_render::engine_support(&frust_render::TierCaps {
            downlevel_flags: adapter.get_downlevel_capabilities().flags,
            adapter_name: info.name.clone(),
        })
        .unwrap_or_else(|refusal| panic!("the engine refused this WebGL2 adapter: {refusal}"));

        let caps = TierCaps::probe(&adapter);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("frust-testing webgl2 golden device"),
                required_features: wgpu::Features::empty(),
                // The production-shared derivation, not a hand-written limit
                // set: `test_device_limits` runs the probed profile through
                // the very same `base_device_limits` + `effective_limits` pair
                // `frust_gpu::create_device` uses, which for a `WebGl2`
                // profile is `Limits::downlevel_webgl2_defaults()
                // .using_resolution(adapter.limits())`. A fixture that
                // hard-coded that recipe could drift away from what the
                // production path would request for the same adapter; this
                // one cannot.
                required_limits: test_device_limits(&adapter, &caps),
                ..Default::default()
            })
            .await
            .expect("creating a device on the resolved WebGL2 adapter");

        let mut renderer = EngineRenderer::new(&device, &caps, TARGET_FORMAT, None)
            .unwrap_or_else(|err| panic!("the engine refused this WebGL2 device: {err}"));
        // Forced to completion here rather than left to a background worker:
        // the warm-up thread cannot be spawned on wasm32 at all (the engine
        // falls back to the inline path), and paying for it at construction
        // keeps that fallback from being read as a first-frame cost.
        renderer.finish_warm_up(&device);

        Self {
            device,
            queue,
            renderer,
            caps,
            meta,
            _surface: surface,
            target: None,
        }
    }

    /// Ensures the offscreen target matches `width` x `height`, telling the
    /// renderer about a resize so nothing sized against the old extent
    /// survives into the next frame.
    fn ensure_target(&mut self, width: u32, height: u32) {
        let matches = self
            .target
            .as_ref()
            .is_some_and(|target| target.width() == width && target.height() == height);
        if matches {
            return;
        }
        if self.target.is_some() {
            self.renderer.resize(&self.device, width, height);
        }
        self.target = Some(HeadlessTarget::new(
            &self.device,
            width,
            height,
            TARGET_FORMAT,
        ));
    }

    /// Renders `scene` offscreen and reads the pixels back, unpadded and
    /// PREMULTIPLIED.
    ///
    /// # Panics
    ///
    /// On a refused frame and on ANY wgpu validation error captured during the
    /// render — a harness that renders through a validation error is producing
    /// pixels nobody should trust, and on a downlevel profile a validation
    /// error is the single most likely way a WebGL2 regression announces
    /// itself.
    async fn render(&mut self, scene: &frust_scene::Scene, spec: &RenderSpec) -> RenderedImage {
        self.ensure_target(spec.width, spec.height);
        let target = self
            .target
            .as_ref()
            .expect("ensure_target leaves a target in place");

        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frust-testing webgl2 golden frame"),
            });
        let encoded = self.renderer.encode(
            &self.device,
            &self.queue,
            &mut encoder,
            scene,
            EngineTarget {
                view: target.view(),
                format: TARGET_FORMAT,
                width: spec.width,
                height: spec.height,
                // The engine owns its own depth attachment here: this harness
                // composites no pass of its own, so there is no surface-owned
                // buffer for the frame to test against.
                depth: None,
                output: OutputAlpha::Premultiplied,
            },
            spec.base_color,
            // `scale` applies AHEAD of `root`, the same composition every other
            // arm folds — the arms must agree on this or a scaled case would
            // diverge for a reason that is not the renderer's.
            kurbo::Affine::scale(spec.scale) * spec.root,
        );
        // Submitted whether or not the frame encoded: a refused frame leaves
        // the encoder exactly as it was found, and finishing it keeps the
        // device's own bookkeeping in step before the error scope is drained.
        self.queue.submit([encoder.finish()]);
        self.renderer.end_frame(&self.queue);

        let validation = scope.pop().await;
        encoded.unwrap_or_else(|err| panic!("the frame was refused on WebGL2: {err}"));
        if let Some(error) = validation {
            panic!("wgpu validation error during a WebGL2 render: {error}");
        }

        RenderedImage {
            width: spec.width,
            height: spec.height,
            rgba8: self.read_back(spec.width, spec.height).await,
            alpha: AlphaKind::Premultiplied,
            meta: self.meta.clone(),
        }
    }

    /// Copies the offscreen target into a mappable buffer and reads it back,
    /// tightly packed, driving the map across event-loop turns.
    ///
    /// # Why this is not `frust_gpu::HeadlessTarget::read_back`
    ///
    /// That method does the identical GPU work and then blocks:
    /// `device.poll(PollType::wait_indefinitely())` followed by a channel
    /// `recv()`. Both halves are unavailable in a browser. wgpu forces the
    /// `glClientWaitSync` timeout to ZERO on the webgl backend — deliberately,
    /// because Chromium's own `MAX_CLIENT_WAIT_TIMEOUT_WEBGL` is 0 and a
    /// blocking wait would freeze the page's only thread — so an indefinite
    /// wait is a single non-blocking check that returns `PollError::Timeout`
    /// the moment the fence is not already signalled, which is what it always
    /// is immediately after a submit. Observed exactly that way before this
    /// existed: `frust-gpu headless: device poll for readback map must
    /// succeed: Timeout`, on the first case of the corpus.
    ///
    /// So the wait has to be spelled the browser's way: poll, yield to the
    /// event loop, poll again, until the map callback fires. That makes the
    /// readback `async`, which `HeadlessTarget::read_back`'s signature is not,
    /// and this file cannot change that signature — `frust-gpu` is outside
    /// this card's scope. Giving `HeadlessTarget` an `async` readback arm for
    /// wasm32 is the tracked follow-up; it pairs with the
    /// `HeadlessOptions` surface hint named in this file's module docs, and
    /// together they are what would let a browser run go through
    /// `frust_render::HeadlessRenderer` unchanged.
    async fn read_back(&self, width: u32, height: u32) -> Vec<u8> {
        let target = self
            .target
            .as_ref()
            .expect("ensure_target leaves a target in place");
        let bytes_per_row = padded_bytes_per_row(width);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frust-testing webgl2 golden readback"),
            size: u64::from(bytes_per_row) * u64::from(height),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frust-testing webgl2 golden readback copy"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: target.texture(),
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);

        let slice = buffer.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });

        let mut mapped = None;
        for _ in 0..MAX_READBACK_TURNS {
            // A single non-blocking check, which is all the webgl backend can
            // honour; it also runs any callback whose fence has since
            // signalled.
            let _ = self.device.poll(wgpu::PollType::Poll);
            if let Ok(result) = receiver.try_recv() {
                mapped = Some(result);
                break;
            }
            yield_to_event_loop().await;
        }
        mapped
            .unwrap_or_else(|| {
                panic!(
                    "the readback buffer was never mapped after {MAX_READBACK_TURNS} event-loop                      turns — the GPU fence for this frame never signalled"
                )
            })
            .unwrap_or_else(|err| panic!("readback buffer map failed: {err}"));

        let view = slice
            .get_mapped_range()
            .expect("the readback buffer is mapped after map_async succeeded");
        let rows = strip_row_padding(&view, width, height);
        drop(view);
        buffer.unmap();
        rows
    }
}

/// The whole gate: every unit corpus case the engine draws, rendered on a real
/// WebGL2 adapter and held to its probes, to `vello_cpu`, and — through the
/// reference arm — to its committed baseline.
///
/// One test rather than one per case, deliberately: a `wgpu::Device` on a
/// WebGL2 context is expensive to build and the whole suite shares one, so
/// splitting the corpus across `#[wasm_bindgen_test]` functions would pay for
/// adapter resolution, device creation and pipeline warm-up nineteen times.
/// Every case is still reported by name, and the run collects EVERY failure
/// before asserting rather than stopping at the first — one browser round trip
/// should tell a reader everything that is wrong, not just the alphabetically
/// first thing.
#[wasm_bindgen_test]
async fn unit_corpus_renders_on_webgl2() {
    let mut harness = WebGl2Harness::new().await;

    log(&format!(
        "frust webgl2 golden suite: backend={} adapter={:?} driver={:?}",
        harness.meta.backend, harness.meta.adapter, harness.meta.driver
    ));
    log(&format!(
        "frust webgl2 golden suite: downlevel_profile={:?} storage_buffers={} \
         max_texture_dimension_2d={} max_texture_array_layers={} max_bind_groups={} \
         max_uniform_buffer_binding_size={} max_vertex_attributes={}",
        harness.caps.downlevel_profile,
        harness.caps.has_storage_buffers,
        harness.caps.max_texture_dimension_2d,
        harness.caps.max_texture_array_layers,
        harness.caps.max_bind_groups,
        harness.caps.max_uniform_buffer_binding_size,
        harness.caps.max_vertex_attributes,
    ));

    // The profile assertion this whole card exists for. `DownlevelProfile` is
    // derived, never chosen: `resolve` answers `WebGl2` for every
    // `wgpu::Backend::Gl` adapter, so asserting the backend and the resolved
    // profile together says both that this is a real WebGL2 context and that
    // the engine knows it is.
    assert_eq!(
        harness.meta.backend, "gl",
        "this suite must run on a `wgpu::Backend::Gl` adapter — anything else is not WebGL2 and \
         proves nothing the desktop `FRUST_ENGINE_DOWNLEVEL=1` rehearsal does not already prove",
    );
    assert_eq!(
        harness.caps.downlevel_profile,
        DownlevelProfile::WebGl2,
        "a `Gl` adapter must resolve `DownlevelProfile::WebGl2`",
    );
    assert!(
        !harness.caps.has_storage_buffers,
        "the WebGL2 profile has no storage buffers, and the engine's compile path branches on \
         exactly this flag",
    );

    let mut cpu = CpuOracle::new();
    let mut failures: Vec<String> = Vec::new();
    let mut compared = 0_usize;

    for case in unit_cases() {
        let name = case.spec.name;
        let Some(baseline) = golden_for(name) else {
            log(&format!(
                "frust webgl2 golden suite: [{name}] skipped, no inlined baseline"
            ));
            continue;
        };
        let spec = case.render_spec();
        let scene = case.scene();

        let engine = harness.render(&scene, &spec).await;

        // 1. Probes: absolute, backend-independent pixel arithmetic, on the
        //    premultiplied frame the engine natively produced.
        for message in case.failed_probes(&engine) {
            failures.push(format!("[{name}] probe failed on WebGL2: {message}"));
        }

        // 2. The cross-arm comparison, premultiplied on both sides.
        let cpu_frame = cpu
            .render(&scene, &spec)
            .unwrap_or_else(|err| panic!("case `{name}` failed to render on `vello_cpu`: {err:#}"));
        let tolerance = tolerance_for(&case.spec);
        let cross_arm = diff_images(
            &to_rgba(&cpu_frame),
            &to_rgba(&engine),
            tolerance,
            case.eroded_interior,
        );
        log(&format!(
            "frust webgl2 golden suite: [{name}] webgl2 vs vello_cpu under {tolerance:?} — {}",
            summarize(&cross_arm.report)
        ));
        if !cross_arm.passed {
            append_triptych(name, "webgl2 vs vello_cpu", &cross_arm.triptych);
            failures.push(format!(
                "[{name}] the WebGL2 engine and `vello_cpu` disagree beyond {tolerance:?}: {}; {}",
                summarize(&cross_arm.report),
                first_pixels(&cross_arm)
            ));
        }

        // 3. Reference identity: the arm the comparison above is measured
        //    against must be the committed baseline itself, pixel-exact.
        let expected = decode_golden(name, baseline);
        let reference = diff_images(
            &expected,
            &to_rgba(&straighten_alpha(&cpu_frame)),
            Tolerance::exact(),
            false,
        );
        if !reference.passed {
            append_triptych(
                name,
                "committed baseline vs browser vello_cpu",
                &reference.triptych,
            );
            failures.push(format!(
                "[{name}] the browser's `vello_cpu` arm does not reproduce the committed \
                 `testing/goldens/cpu/{name}.png` baseline: {}; {}. The engine comparison above \
                 is therefore measured against a reference this run invented, not the one the \
                 host gate uses",
                summarize(&reference.report),
                first_pixels(&reference)
            ));
        }

        compared += 1;
    }

    log(&format!(
        "frust webgl2 golden suite: {compared} cases compared, {} failures",
        failures.len()
    ));
    assert!(
        failures.is_empty(),
        "{} of {compared} WebGL2 unit-corpus cases failed:\n{}",
        failures.len(),
        failures.join("\n"),
    );
}

/// The first few differing pixels of an outcome, for a failure message that
/// names values rather than only counts.
fn first_pixels(outcome: &DiffOutcome) -> String {
    let sample: Vec<String> = outcome
        .report
        .pixels
        .iter()
        .take(4)
        .map(|pixel| {
            format!(
                "({}, {}) expected {:?} actual {:?}",
                pixel.x, pixel.y, pixel.expected, pixel.actual
            )
        })
        .collect();
    if sample.is_empty() {
        "no individual pixel recorded".to_string()
    } else {
        sample.join("; ")
    }
}

/// A corpus addition must not be able to slip past this suite by simply not
/// having a row in [`GOLDENS`].
///
/// GPU-free and browser-free in everything but its attribute: it reads the
/// corpus and the two tables and compares names. It exists because
/// [`unit_corpus_renders_on_webgl2`] SKIPS a case with no inlined baseline —
/// which is right for the one deferred case and silently wrong for a new one.
#[wasm_bindgen_test]
fn every_engine_drawable_unit_case_has_an_inlined_baseline() {
    let mut missing = Vec::new();
    for case in unit_cases() {
        let name = case.spec.name;
        let deferred = DEFERRED_CASES.iter().any(|(case, _)| *case == name);
        if golden_for(name).is_none() && !deferred {
            missing.push(name);
        }
        if golden_for(name).is_some() && deferred {
            missing.push(name);
        }
    }
    assert!(
        missing.is_empty(),
        "these unit cases are neither inlined in `GOLDENS` nor listed in `DEFERRED_CASES` (or are \
         wrongly in both): {missing:?}",
    );

    for (name, _) in GOLDENS {
        assert!(
            unit_cases().iter().any(|case| case.spec.name == *name),
            "`GOLDENS` inlines a baseline for `{name}`, which is not a case in the unit corpus",
        );
    }
    for (name, _, _) in ESCALATIONS {
        assert!(
            golden_for(name).is_some(),
            "`ESCALATIONS` widens `{name}`, which this suite does not compare",
        );
    }
}
