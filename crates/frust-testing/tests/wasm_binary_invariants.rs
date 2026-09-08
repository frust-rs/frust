#![cfg(all(target_arch = "wasm32", feature = "webgl"))]

//! Properties of the wasm MODULE the browser golden suite ships in, as opposed
//! to the pixels it produces: how big it is, which target features it was
//! built with, and that nothing on its render path reaches for a clock that
//! does not exist on this target.
//!
//! ```text
//! CHROMEDRIVER_REMOTE=http://localhost:9517 \
//!   CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
//!   cargo test -p frust-testing --target wasm32-unknown-unknown \
//!     --features webgl --release --test wasm_binary_invariants
//! ```
//!
//! Needs no GPU: every test here runs to completion in a browser with no
//! WebGL2 adapter at all, which is deliberate — these are the invariants that
//! should still be checkable on a rig where `tests/wasm_goldens.rs` cannot run.
//!
//! # Size
//!
//! [`wasm_module_size_is_recorded`] fetches the very module it is running
//! inside and records its byte length, the same self-introspection upstream's
//! `vello_sparse_tests/tests/wasm_binary_invariants.rs` does. Recording is the
//! point: this graph pulls the whole engine tier, `vello_cpu`, four
//! design-system catalogs and nineteen inlined baselines into one wasm module,
//! and a number in the run log is what makes a later growth visible.
//! [`SIZE_CEILING_BYTES`] is a tripwire around that recorded number, not a
//! budget anybody optimised toward.
//!
//! # `std::time`
//!
//! `std::time::Instant::now()` and `SystemTime::now()` PANIC on
//! `wasm32-unknown-unknown` — std has no clock there. So "the render path uses
//! no `std::time`" is not something to assert, it is something a completed run
//! proves: [`the_gpu_free_render_path_uses_no_std_time`] drives scene
//! recording, `vello_cpu` rasterization, the alpha straightening and the
//! comparator end to end, and reaching the final assertion at all is the
//! proof. `tests/wasm_goldens.rs` extends the same proof across the GPU half.
//!
//! The one shape deliberately NOT written here is a negative control that
//! calls `Instant::now()` and expects a panic: a panic inside a wasm test
//! poisons the module for every test after it in the same run, so the control
//! would cost more than it proves. Validating the module's import and
//! instruction sections directly — a `wasmparser` pass over the fetched bytes,
//! which would also turn the SIMD note below into a real check — is the
//! tracked follow-up.
//!
//! # SIMD128
//!
//! wasm SIMD is off in this build unless an operator asked for it explicitly:
//!
//! ```text
//! RUSTFLAGS='-C target-feature=+simd128' cargo test -p frust-testing \
//!   --target wasm32-unknown-unknown --features webgl --release \
//!   --test wasm_binary_invariants
//! ```
//!
//! It is an explicit-opt-in flag rather than a default because a wasm module
//! carrying a single SIMD instruction is invalid for every runtime without
//! SIMD support — the whole module, not just the function that used it. This
//! file records which of the two builds produced the size it reports, so the
//! two numbers are never compared across build modes by accident.

use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

use frust_testing::case::Tolerance;
use frust_testing::corpus::{straighten_alpha, unit_cases};
use frust_testing::diff::diff_images;
use frust_testing::oracle_cpu::CpuOracle;
use frust_testing::render::{RenderedImage, SceneRenderer};

wasm_bindgen_test_configure!(run_in_browser);

/// The URL `wasm-bindgen-test-runner` serves this test module's own wasm
/// binary at. Fixed by the runner's page layout, not by anything in this
/// repository — the same path upstream's own self-introspection test fetches.
const SELF_MODULE_URL: &str = "/wasm-bindgen-test_bg.wasm";

/// The ceiling [`wasm_module_size_is_recorded`] holds the fetched module to.
///
/// A tripwire, not a budget. Measured when this suite landed: **2,799,374
/// bytes (2.67 MiB)** for `tests/wasm_binary_invariants.rs` in a `--release`,
/// scalar (no `simd128`) build, of which 18,205 bytes are the inlined
/// baselines. 8 MiB leaves roughly 3x headroom, so ordinary churn never trips
/// it while a change that triples the module — a heavyweight dependency
/// reaching the browser graph, or a debug build slipping into a gate — fails
/// here with a number instead of being noticed months later. Raise it
/// deliberately, with the new measured size recorded in the same change.
const SIZE_CEILING_BYTES: usize = 8 * 1024 * 1024;

/// Whether this build was compiled with wasm SIMD, as the one word the size
/// line is qualified by.
const fn simd_mode() -> &'static str {
    if cfg!(target_feature = "simd128") {
        "simd128"
    } else {
        "scalar"
    }
}

/// Writes one line to the browser console.
///
/// `println!` is a silent no-op on `wasm32-unknown-unknown` — std's stdout is
/// the `unsupported` shim there — so a recorded number that is not written
/// here is not recorded at all.
fn log(line: &str) {
    web_sys::console::log_1(&wasm_bindgen::JsValue::from_str(line));
}

/// Records this wasm module's own byte size and holds it under
/// [`SIZE_CEILING_BYTES`].
#[wasm_bindgen_test]
async fn wasm_module_size_is_recorded() {
    let window = web_sys::window().expect("a `run_in_browser` test always has a window");
    let response = JsFuture::from(window.fetch_with_str(SELF_MODULE_URL))
        .await
        .expect("fetching this test's own wasm module");
    let response: web_sys::Response = response.dyn_into().expect("a fetch resolves to a Response");
    assert!(
        response.ok(),
        "the test module could not be fetched from `{SELF_MODULE_URL}` (status {}) — the runner's \
         page layout is what defines this path, so a failure here means the harness moved, not \
         that the binary is wrong",
        response.status(),
    );

    let buffer = JsFuture::from(
        response
            .array_buffer()
            .expect("a Response body can be read as an ArrayBuffer"),
    )
    .await
    .expect("reading the wasm module's bytes");
    let bytes = web_sys::js_sys::Uint8Array::new(&buffer).length() as usize;

    log(&format!(
        "frust wasm binary invariants: module {bytes} bytes ({:.2} MiB), build={}, \
         inlined baselines {} bytes",
        bytes as f64 / (1024.0 * 1024.0),
        simd_mode(),
        inlined_baseline_bytes(),
    ));

    assert!(
        bytes > 0,
        "the fetched wasm module is empty, which is not a size this suite can have",
    );
    assert!(
        bytes <= SIZE_CEILING_BYTES,
        "the wasm test module is {bytes} bytes, past the {SIZE_CEILING_BYTES}-byte tripwire — \
         either something heavyweight reached the browser graph or the ceiling needs raising \
         deliberately with the new measurement recorded",
    );
}

/// The byte total the golden suite's inlined baselines contribute, reported
/// alongside the module size so the part of it this suite owns outright is
/// separable from the part its dependencies own.
///
/// Deliberately re-derived from the committed PNGs rather than shared with
/// `tests/wasm_goldens.rs`: a `tests/` target cannot import another one, and
/// duplicating an `include_bytes!` costs nothing at run time (the linker keeps
/// one copy of identical read-only data) while keeping each test target
/// standalone.
fn inlined_baseline_bytes() -> usize {
    const BASELINES: &[&[u8]] = &[
        include_bytes!("../../../testing/goldens/cpu/unit-fill-rect.png"),
        include_bytes!("../../../testing/goldens/cpu/unit-rounded-rect.png"),
        include_bytes!("../../../testing/goldens/cpu/unit-stroke-line.png"),
        include_bytes!("../../../testing/goldens/cpu/unit-glyph-run.png"),
        include_bytes!("../../../testing/goldens/cpu/unit-clip-rect.png"),
        include_bytes!("../../../testing/goldens/cpu/unit-clip-rounded.png"),
        include_bytes!("../../../testing/goldens/cpu/unit-clip-balance.png"),
        include_bytes!("../../../testing/goldens/cpu/unit-image.png"),
        include_bytes!("../../../testing/goldens/cpu/unit-blur-rrect.png"),
        include_bytes!("../../../testing/goldens/cpu/unit-layer-alpha.png"),
        include_bytes!("../../../testing/goldens/cpu/unit-layer-balance.png"),
        include_bytes!("../../../testing/goldens/cpu/unit-layer-sibling-fan.png"),
        include_bytes!("../../../testing/goldens/cpu/unit-layer-nested-pair.png"),
        include_bytes!("../../../testing/goldens/cpu/unit-clear-rect.png"),
        include_bytes!("../../../testing/goldens/cpu/unit-path-fill.png"),
        include_bytes!("../../../testing/goldens/cpu/unit-path-stroke.png"),
        include_bytes!("../../../testing/goldens/cpu/unit-path-dashed.png"),
        include_bytes!("../../../testing/goldens/cpu/unit-snapshot-bracket.png"),
        include_bytes!("../../../testing/goldens/cpu/unit-snapshot-balance.png"),
    ];
    BASELINES.iter().map(|bytes| bytes.len()).sum()
}

/// Drives the whole GPU-free half of a golden run — scene recording,
/// `vello_cpu` rasterization, alpha straightening, comparison — and reaches
/// its final assertion, which on this target is the proof that none of it
/// touched `std::time`.
///
/// See this file's module docs for why the proof takes this shape rather than
/// a panicking negative control.
#[wasm_bindgen_test]
fn the_gpu_free_render_path_uses_no_std_time() {
    let mut cpu = CpuOracle::new();
    let mut rendered = 0_usize;

    for case in unit_cases() {
        let name = case.spec.name;
        let spec = case.render_spec();
        let frame: RenderedImage = cpu
            .render(&case.scene(), &spec)
            .unwrap_or_else(|err| panic!("case `{name}` failed to render on `vello_cpu`: {err:#}"));
        assert_eq!(
            frame.rgba8.len(),
            (spec.width * spec.height * 4) as usize,
            "case `{name}` produced a frame whose buffer is not its own viewport",
        );

        // Straightening and the comparator are on the same no-clock path, and
        // an image compared against itself is the cheapest way to walk every
        // branch of both without asserting anything about pixels here — that
        // is `tests/wasm_goldens.rs`'s job.
        let straight = straighten_alpha(&frame);
        let buffer = image::RgbaImage::from_raw(straight.width, straight.height, straight.rgba8)
            .expect("a RenderedImage's buffer is exactly width * height * 4 bytes");
        let outcome = diff_images(&buffer, &buffer, Tolerance::exact(), false);
        assert!(
            outcome.passed,
            "case `{name}` does not compare equal to itself, which would make every comparison \
             this suite makes meaningless",
        );
        rendered += 1;
    }

    log(&format!(
        "frust wasm binary invariants: {rendered} cases rendered on the GPU-free path with no \
         clock available, build={}",
        simd_mode(),
    ));
    assert!(
        rendered > 0,
        "the unit corpus is empty, so this test proved nothing",
    );
}

/// Records the target-feature build mode, so a size or timing figure from this
/// run is never read as belonging to the other one.
#[wasm_bindgen_test]
fn simd128_build_mode_is_recorded() {
    log(&format!(
        "frust wasm binary invariants: target_feature simd128 = {}, build={}",
        cfg!(target_feature = "simd128"),
        simd_mode(),
    ));
    assert_eq!(
        cfg!(target_feature = "simd128"),
        simd_mode() == "simd128",
        "the recorded build mode and the compiled target feature must be the same fact",
    );
}
