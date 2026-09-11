//! The iOS Simulator engine gate (`engine-p6-ios-simulator-renders`): the one
//! automated test proving the engine tier draws correct, non-black pixels on
//! the Simulator's own Metal adapter.
//!
//! # Why this exists
//!
//! The iOS Simulator's Metal feature set is `Apple2` only — it never exposes
//! [`wgpu::DownlevelFlags::INDIRECT_EXECUTION`]. That flag is why this gate
//! exists: the now-removed vello-classic renderer required it
//! unconditionally and rendered black on the Simulator BY CONSTRUCTION
//! (`docs/TESTING.md`'s iOS section — "do not classify a black Simulator
//! surface as a golden result"). Classic vello is gone now, but
//! the Simulator adapter's constraint is unchanged, and it is still the
//! reason this gate has to exist: `frust-engine`'s sparse-strip pipeline
//! needs none of those downlevel flags at all (`tier.rs`'s
//! `ENGINE_REQUIRED_DOWNLEVEL_FLAGS` — deliberately empty), so it is the one
//! pipeline that can actually draw on this adapter. This file is the proof,
//! run for real against the Simulator's own Metal device rather than argued
//! from adapter flags alone.
//!
//! # Why this is a separate, `target_os`-gated integration test
//!
//! The whole file compiles to nothing outside iOS (`#![cfg(target_os =
//! "ios")]`), so the standard workspace gate (`cargo test --workspace` on a
//! Linux/macOS host) builds and runs an empty test binary here — zero cost,
//! zero GPU requirement, no `#[ignore]` needed. On the Simulator itself the
//! recipe is the one target-gated mobile-test shape this repo already uses
//! for `frust-iap`'s own iOS unit tests (`docs/DEVELOPMENT.md`'s
//! `CARGO_TARGET_AARCH64_APPLE_IOS_SIM_RUNNER` paragraph):
//!
//! ```text
//! CARGO_TARGET_AARCH64_APPLE_IOS_SIM_RUNNER="xcrun simctl spawn booted" \
//!   cargo test -p frust-testing --target aarch64-apple-ios-sim --test ios_sim
//! ```
//!
//! # Why this drives `frust-engine` directly instead of through `EngineOracle`
//!
//! [`frust_testing::oracle_engine::EngineOracle::new`] requests
//! `wgpu::Limits::default()` unconditionally when it creates its own device.
//! Measured live against the Simulator's actual Metal adapter (a real
//! Simulator boot, not the fake-adapter host unit tests every other
//! `frust-gpu`/`frust-engine` test runs against), that request is refused:
//! `Limit 'max_inter_stage_shader_variables' value 16 is better than allowed
//! 15` — this rig's adapter offers fewer inter-stage shader variables than
//! `wgpu::Limits::default()` demands, a constraint no test exercising
//! `EngineOracle` had ever run against real Simulator hardware before this
//! card. That constructor lives in `crates/frust-testing/src/oracle_engine.rs`
//! (outside this file's write scope), so rather than route around it with an
//! out-of-scope edit, this file builds its OWN device the way
//! [`ios_simulator_metal_context_forces_the_256_byte_uniform_alignment`]
//! already does — through [`frust_gpu::RenderContext`], whose device request is
//! built from the ADAPTER's own reported limits
//! (`frust_gpu::context::create_device`) rather than a hard-coded default, so
//! it never over-asks — and drives [`frust_engine::EngineRenderer`] over that
//! device directly, following the exact same encode contract
//! `EngineOracle::render` documents (one encoder per frame, one submit, drain
//! the validation error scope before reading pixels back). The stored-golden
//! comparison this file performs is otherwise identical to what
//! `EngineOracle` would have produced. **Finding for a follow-up card:**
//! `EngineOracle::new`'s device request should likely derive its limits from
//! `TierCaps`/the adapter the same way `frust_gpu::context::create_device`
//! does, so a future test that DOES want the full `SceneRenderer` trait
//! object on the Simulator does not hit this same refusal. **Resolved** by
//! `p9-f2-fixture-limits`: `EngineOracle::new` and every device-suite fixture
//! now request [`frust_gpu::context::test_device_limits`], the same
//! adapter/`TierCaps`-derived limits `create_device` itself requests, so the
//! `SceneRenderer` trait object is no longer refused here either — this
//! file's own device stays a deliberate, direct `frust_gpu::RenderContext`
//! build for the reasons above, not a `EngineOracle` workaround any more.
//!
//! # Why the CPU reference is embedded bytes, not a live oracle
//!
//! [`frust_testing::oracle_cpu::CpuOracle`] (`vello_cpu`) could in principle
//! run inside this same Simulator process — it needs no GPU — but the
//! reference this file holds the engine to is the ALREADY-PROMOTED
//! `testing/goldens/cpu/` baseline: the one every other CPU-anchored gate in
//! this crate already trusts, captured once and reviewed rather than
//! re-derived per run. The obstacle is that
//! [`frust_testing::golden::compare_golden`] resolves that baseline from
//! `CARGO_MANIFEST_DIR` at RUN time (`fs::read`/`image::open`), and the
//! binary `xcrun simctl spawn booted` executes is copied into the
//! Simulator's own sandbox with no access back to this repository checkout —
//! there is no filesystem here to resolve that path against.
//! `include_bytes!` sidesteps it entirely: the PNG bytes are embedded into
//! the test binary at COMPILE time (on the host, which does have the
//! checkout), so the Simulator process never touches a path outside itself.
//!
//! # Selection
//!
//! Seven cases out of [`frust_testing::unit_cases`], spanning every shape
//! category that corpus actually carries: solid rect fills
//! (`unit-fill-rect`, uniform/per-corner rounded rects in
//! `unit-rounded-rect`), rectangular and rounded clips (`unit-clip-rect`,
//! `unit-clip-rounded`), filled/stroked arbitrary paths (`unit-path-fill`,
//! `unit-path-stroke`), and a glyph run shaped against the bundled test fonts
//! (`unit-glyph-run`) — the same `Command::GlyphRun` lowering
//! `docs/TESTING.md`'s Deterministic Inputs section requires a portable
//! (non-system-font) reference for. The unit corpus carries no dedicated
//! gradient-brush case (that lives in the text corpus as
//! `text-gradient-brush`, outside this file's scope); every brush drawn here
//! is [`peniko::Brush::Solid`]. Every selected case shares the corpus's fixed
//! 64x64 frame size, so one [`frust_gpu::HeadlessTarget`] is created once and
//! reused across all seven — the same reuse `EngineOracle::render`'s own
//! `ensure_target` applies when a case asks for a size already in hand.
//!
//! # Tolerance
//!
//! The six geometry cases compare at the corpus's own tight default
//! ([`Tolerance::new`]) — no antialiasing-sensitive text hinting is in play,
//! so the engine's strip fill and `vello_cpu`'s agree almost exactly,
//! measured 0 px differing at max |delta| [0, 0, 0, 1] or better across all
//! six on this rig, the same near-zero result `engine_goldens.rs`'s P1 gate
//! measures for this shape of case on every reviewed rig. `unit-glyph-run`
//! carries a widened, MEASURED tolerance instead (see its row in
//! [`SELECTED`]), for the same reason `engine_goldens.rs`'s `ESCALATIONS`
//! table widens its own hint-policy rows (see that file's module docs): the
//! engine's `SceneCompiler::for_caps` reads
//! `wgpu::AdapterInfo::transient_saves_memory` to decide mobile vs. desktop
//! tier, true for Apple silicon's TBDR hardware including the Simulator's own
//! Metal device, so this rig hints text OFF like
//! [`frust_testing::oracle_cpu::CpuOracle`] does too — measured max |delta|
//! [18, 18, 18, 0] rather than the much wider hinted-vs-unhinted gap a
//! desktop-tier rig's own glyph-run row carries, and 0 px differing under the
//! widened tolerance `SELECTED`'s row records.

#![cfg(target_os = "ios")]

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::task::{Context as TaskContext, Poll, Waker};

use frust_engine::{EngineRenderer, EngineTarget, OutputAlpha};
use frust_gpu::{HeadlessTarget, RenderContext};
use frust_testing::diff::diff_images;
use frust_testing::{
    AlphaKind, BackendMeta, RenderedImage, Tolerance, straighten_alpha, unit_cases,
};
use image::RgbaImage;
use kurbo::Affine;

/// The uniform-buffer offset alignment the Simulator's Metal validation
/// actually enforces, regardless of what the adapter reports
/// ([`frust_gpu::context`]'s `effective_limits`/wgpu#7057 doc comment).
const EXPECTED_MIN_UNIFORM_BUFFER_OFFSET_ALIGNMENT: u32 = 256;

/// The target format every frame in this file is rendered into — the same
/// choice `oracle_engine.rs` makes and for the same reason (`Rgba8Unorm`
/// reads back R, G, B, A in the order a golden PNG stores them, with no
/// implicit gamma conversion the strip shader's own sRGB-encoded output would
/// disagree with).
const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// One selected case: its name (looked up in [`unit_cases`] for its scene and
/// probes) and the tolerance its embedded CPU baseline is held to.
struct GoldenCase {
    name: &'static str,
    tolerance: Tolerance,
    png: &'static [u8],
}

/// See the module docs' Selection/Tolerance sections.
const SELECTED: &[GoldenCase] = &[
    GoldenCase {
        name: "unit-fill-rect",
        tolerance: Tolerance::new(),
        png: include_bytes!("../../../testing/goldens/cpu/unit-fill-rect.png"),
    },
    GoldenCase {
        name: "unit-rounded-rect",
        tolerance: Tolerance::new(),
        png: include_bytes!("../../../testing/goldens/cpu/unit-rounded-rect.png"),
    },
    GoldenCase {
        name: "unit-clip-rect",
        tolerance: Tolerance::new(),
        png: include_bytes!("../../../testing/goldens/cpu/unit-clip-rect.png"),
    },
    GoldenCase {
        name: "unit-clip-rounded",
        tolerance: Tolerance::new(),
        png: include_bytes!("../../../testing/goldens/cpu/unit-clip-rounded.png"),
    },
    GoldenCase {
        name: "unit-path-fill",
        tolerance: Tolerance::new(),
        png: include_bytes!("../../../testing/goldens/cpu/unit-path-fill.png"),
    },
    GoldenCase {
        name: "unit-path-stroke",
        tolerance: Tolerance::new(),
        png: include_bytes!("../../../testing/goldens/cpu/unit-path-stroke.png"),
    },
    GoldenCase {
        name: "unit-glyph-run",
        // Hint-policy row (see the module docs' Tolerance section): measured
        // max |delta| [18, 18, 18, 0] on this Simulator boot, past the
        // corpus-tight default's own channel budget of 2 — a small subpixel
        // rasterization difference the mobile-tier/unhinted pairing still
        // leaves, not the wide hinted-vs-unhinted gap `engine_goldens.rs`'s
        // desktop-tier rigs measure for this same case. Widened to a small
        // margin past the measured number rather than left at the corpus
        // default it would not actually pass at.
        tolerance: Tolerance {
            channel: 20,
            alpha: 2,
            diff_pixels: 0,
        },
        png: include_bytes!("../../../testing/goldens/cpu/unit-glyph-run.png"),
    },
];

/// Serializes the two GPU-touching tests in this binary.
///
/// `engine_goldens.rs`'s own `RENDER_LOCK` doc comment: tearing down two
/// `wgpu::Device`s at once from concurrently-run tests has been observed to
/// hang at process teardown roughly one run in five, and `cargo test` runs
/// this file's two `#[test]` functions on separate threads by default. Poison
/// is ignored deliberately: one test's failure must not cascade into its
/// sibling.
static RENDER_LOCK: Mutex<()> = Mutex::new(());

fn render_lock() -> MutexGuard<'static, ()> {
    RENDER_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Pops a validation error scope, pumping the device until the pop resolves —
/// copied from `oracle_engine.rs`'s identical private helper (not exported),
/// since this file drives `frust-engine`'s encode contract directly rather
/// than through `EngineOracle` (see the module docs).
fn drain_error_scope(device: &wgpu::Device, scope: wgpu::ErrorScopeGuard) -> Option<wgpu::Error> {
    let waker = Waker::noop();
    let mut cx = TaskContext::from_waker(waker);
    let mut future = std::pin::pin!(scope.pop());
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(error) => return error,
            Poll::Pending => {
                let _ = device.poll(wgpu::PollType::wait_indefinitely());
            }
        }
    }
}

/// wgpu#7057, live: the Simulator is Metal-backed and its Metal validation
/// enforces 256-byte `min_uniform_buffer_offset_alignment` regardless of what
/// the adapter's own reported limits say, so `frust_gpu::context`'s
/// `effective_limits` forces every device REQUEST made here up to 256 (see
/// that function's doc comment). This is the first time that mitigation runs
/// against real Simulator hardware rather than only the fake-adapter host
/// unit tests in `frust-gpu/src/context.rs`. [gfx-rs/wgpu PR
/// #10189](https://github.com/gfx-rs/wgpu/pull/10189) (open, unmerged as of
/// this workspace's current wgpu pin) removes the need for the mitigation
/// once a later pinned wgpu release contains it — this assertion (and
/// `effective_limits` itself) can drop then.
///
/// Also confirms, straight off the resolved adapter, the premise the whole
/// file exists to act on: this device lacks
/// [`wgpu::DownlevelFlags::INDIRECT_EXECUTION`] — the flag the deleted
/// vello-classic renderer could not run without, and the engine tier never
/// needs.
#[test]
fn ios_simulator_metal_context_forces_the_256_byte_uniform_alignment() {
    let _serialized = render_lock();
    let mut context = RenderContext::new();
    let handle = pollster::block_on(context.device()).expect(
        "frust-gpu must be able to create a device against the Simulator's own Metal adapter",
    );

    println!(
        "ios-sim: adapter={:?} backend={:?} downlevel_flags={:?}",
        handle.caps.adapter_name, handle.caps.backend, handle.caps.downlevel_flags
    );
    assert!(
        !handle
            .caps
            .downlevel_flags
            .contains(wgpu::DownlevelFlags::INDIRECT_EXECUTION),
        "the Simulator's adapter now reports INDIRECT_EXECUTION — the constraint this gate \
         exists to work around is gone, and this file's whole premise (the engine renders \
         where nothing else could) needs revisiting, not just this assertion"
    );

    let alignment = handle.device.limits().min_uniform_buffer_offset_alignment;
    println!("ios-sim: min_uniform_buffer_offset_alignment={alignment}");
    assert_eq!(
        alignment, EXPECTED_MIN_UNIFORM_BUFFER_OFFSET_ALIGNMENT,
        "frust_gpu::context::effective_limits must still force 256-byte \
         min_uniform_buffer_offset_alignment on the Simulator (wgpu#7057)"
    );
}

/// Renders [`SELECTED`] through [`frust_engine::EngineRenderer`] into one
/// shared [`HeadlessTarget`] on the Simulator's own Metal adapter (see the
/// module docs for why this drives the engine directly rather than through
/// `EngineOracle`), and holds each frame to two independent bars: its case's
/// own backend-independent [`frust_testing::Probe`]s, and a pixel diff
/// against the embedded `testing/goldens/cpu/` baseline under [`SELECTED`]'s
/// tolerance. Passing both on an adapter classic is black on by construction
/// is the signal this file exists to produce.
#[test]
fn engine_renders_the_unit_corpus_matching_the_cpu_goldens_on_the_simulator() {
    let _serialized = render_lock();

    let mut context = RenderContext::new();
    let handle = pollster::block_on(context.device()).expect(
        "frust-gpu must be able to create a device against the Simulator's own Metal adapter",
    );
    let device = handle.device.clone();
    let queue = handle.queue.clone();
    let caps = handle.caps.clone();
    println!(
        "ios-sim: engine device on adapter={:?} backend={:?}",
        caps.adapter_name, caps.backend
    );

    let mut renderer = EngineRenderer::new(&device, &caps, TARGET_FORMAT, None)
        .expect("the engine refused this device");
    renderer.finish_warm_up(&device);

    let corpus = unit_cases();
    let mut failures: Vec<String> = Vec::new();

    // Every selected case shares the corpus's fixed 64x64 frame size (see the
    // module docs' Selection section), so one target serves all of them.
    let first_spec = corpus
        .iter()
        .find(|case| case.spec.name == SELECTED[0].name)
        .expect("SELECTED's first case is in unit_cases()")
        .render_spec();
    let target = HeadlessTarget::new(&device, first_spec.width, first_spec.height, TARGET_FORMAT);

    for selected in SELECTED {
        let case = corpus
            .iter()
            .find(|case| case.spec.name == selected.name)
            .unwrap_or_else(|| panic!("`{}` is no longer in unit_cases()", selected.name));
        let spec = case.render_spec();
        assert_eq!(
            (spec.width, spec.height),
            (target.width(), target.height()),
            "case `{}` does not share SELECTED's fixed frame size — give it its own \
             HeadlessTarget instead of reusing this one",
            case.spec.name
        );

        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("ios-sim engine frame"),
        });
        let encoded = renderer.encode(
            &device,
            &queue,
            &mut encoder,
            &case.scene(),
            EngineTarget {
                view: target.view(),
                format: TARGET_FORMAT,
                width: spec.width,
                height: spec.height,
                depth: None,
                output: OutputAlpha::Premultiplied,
            },
            spec.base_color,
            Affine::scale(spec.scale) * spec.root,
        );
        // Submitted whether or not the frame encoded, exactly as
        // `EngineOracle::render` does: a refused frame leaves the encoder
        // exactly as it was found, and finishing it keeps the device's own
        // bookkeeping in step before the error scope is drained.
        queue.submit([encoder.finish()]);
        renderer.end_frame(&queue);
        let validation = drain_error_scope(&device, scope);

        if let Err(err) = encoded {
            failures.push(format!(
                "[{}] the engine refused the frame: {err}",
                case.spec.name
            ));
            continue;
        }
        if let Some(error) = validation {
            failures.push(format!(
                "[{}] wgpu validation error during render: {error}",
                case.spec.name
            ));
            continue;
        }

        let rgba8 = target.read_back(&device, &queue);
        let rendered = RenderedImage {
            width: spec.width,
            height: spec.height,
            rgba8,
            // Premultiplied, as the strip pipelines blend and the target was
            // declared — see `oracle_engine.rs`'s own Alpha module docs.
            alpha: AlphaKind::Premultiplied,
            meta: BackendMeta {
                backend: "ios-simulator-engine".to_string(),
                adapter: caps.adapter_name.clone(),
                driver: String::new(),
                device_kind: "gpu".to_string(),
            },
        };
        let image = straighten_alpha(&rendered);

        let probe_failures = case.failed_probes(&image);
        if !probe_failures.is_empty() {
            for message in probe_failures {
                failures.push(format!("[{}] probe: {message}", case.spec.name));
            }
            continue;
        }

        let expected = image::load_from_memory(selected.png)
            .unwrap_or_else(|err| {
                panic!(
                    "embedded golden `{}.png` failed to decode: {err}",
                    selected.name
                )
            })
            .to_rgba8();
        let actual = RgbaImage::from_raw(image.width, image.height, image.rgba8.clone())
            .unwrap_or_else(|| {
                panic!(
                    "case `{}`: rendered {}x{} frame ({} bytes) doesn't match width*height*4",
                    case.spec.name,
                    image.width,
                    image.height,
                    image.rgba8.len()
                )
            });

        let outcome = diff_images(&expected, &actual, selected.tolerance, case.eroded_interior);
        println!(
            "ios-sim: [{}] {} px differ ({:.4}%), max |delta| {:?}",
            case.spec.name,
            outcome.report.pixel_count,
            outcome.report.mismatched_percent,
            outcome.report.max_difference
        );
        if !outcome.passed {
            failures.push(format!(
                "[{}] engine vs cpu golden disagree beyond {:?}: {} px differ ({:.4}%), max \
                 |delta| {:?}",
                case.spec.name,
                selected.tolerance,
                outcome.report.pixel_count,
                outcome.report.mismatched_percent,
                outcome.report.max_difference
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "{} case(s) failed on the iOS Simulator engine gate:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
