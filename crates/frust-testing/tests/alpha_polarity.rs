//! Alpha polarity: what the engine's readback bytes actually MEAN.
//!
//! ```text
//! WGPU_BACKEND=vulkan WGPU_ADAPTER_NAME=T400 FRUST_GOLDEN_EXPECT_ADAPTER=T400 \
//!   cargo test -p frust-testing --test alpha_polarity -- --ignored --nocapture
//! ```
//!
//! The engine blends premultiplied and renders into a target declared
//! premultiplied, so a 50%-alpha white fill must read back `(128, 128, 128,
//! 128)` — colour channels already scaled by alpha — and NOT `(255, 255, 255,
//! 128)`. The two readings are equally plausible-looking bytes: a comparator
//! handed the wrong one silently compares in the wrong space, and a golden
//! promoted from it freezes the mistake. Three checks pin it down:
//!
//! 1. **Over a TRANSPARENT destination**, where the two conventions differ
//!    maximally (nothing to blend against, so the fill's own convention is the
//!    whole answer), the frame reads premultiplied.
//! 2. **Over an OPAQUE BLACK destination**, where the blend is a real
//!    composite, the frame matches `vello_cpu` 0.2.0 — whose pixmap is
//!    documented premultiplied — byte for byte at the corpus's tight default
//!    tolerance.
//! 3. **A negative control.** The same frame deliberately misread as straight
//!    alpha (exactly what [`straighten_alpha`] produces) must FAIL check 1.
//!    Without this, a check that accidentally accepted both conventions would
//!    look just as green as one that discriminates.

use std::sync::{Mutex, MutexGuard, PoisonError};

use image::RgbaImage;
use kurbo::{Affine, Rect};
use peniko::color::palette::css::{BLACK, WHITE};
use peniko::{Brush, Color};

use frust_scene::{Scene, SceneBuilder};
use frust_testing::case::Tolerance;
use frust_testing::corpus::straighten_alpha;
use frust_testing::diff::diff_images;
use frust_testing::oracle_cpu::CpuOracle;
use frust_testing::oracle_engine::{EngineOracle, EngineOracleOptions};
use frust_testing::render::{AlphaKind, BackendMeta, RenderSpec, RenderedImage, SceneRenderer};

/// Frame extent every case renders at — the corpus's own size, so a promoted
/// artifact from a failure reads beside the rest of the corpus.
const SIZE: u32 = 64;

/// The filled region. Well inside the frame, integer-aligned on every edge, so
/// its interior carries no antialiased coverage and the only thing the sampled
/// pixels can be about is alpha.
const FILL: Rect = Rect::new(16.0, 16.0, 48.0, 48.0);

/// Interior sample points, all strictly inside [`FILL`].
const INTERIOR: &[(u32, u32)] = &[(20, 20), (32, 32), (44, 44)];

/// A point outside [`FILL`], where nothing is drawn.
const OUTSIDE: (u32, u32) = (4, 4);

/// 50%-alpha white, premultiplied: `1.0 * 0.5 -> 127.5 -> 128` in every colour
/// channel, alpha likewise.
const PREMULTIPLIED: [u8; 4] = [128, 128, 128, 128];

/// The same pixel misread as straight alpha — the colour channels NOT scaled
/// by alpha. What check 3 must reject.
const STRAIGHT: [u8; 4] = [255, 255, 255, 128];

/// 50%-alpha white composited over opaque black: the colour channels are the
/// same 128, and the result is fully opaque because the destination was.
const OVER_BLACK: [u8; 4] = [128, 128, 128, 255];

/// The tolerance every polarity assertion is made at: the corpus's tight
/// default channel bar, which is an 8-bit rounding step and nothing more. It
/// is two orders of magnitude below the 127-level gap between the two
/// conventions, so it can never blur one into the other.
const POLARITY_TOLERANCE: u8 = Tolerance::new().channel;

/// Serializes the GPU tests in this binary — see `tests/engine_goldens.rs`'s
/// identical lock for the teardown race it avoids. Poison is ignored
/// deliberately: one test's failure must not cascade into every sibling.
static RENDER_LOCK: Mutex<()> = Mutex::new(());

fn render_lock() -> MutexGuard<'static, ()> {
    RENDER_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The one scene every case renders: a single 50%-alpha white rectangle.
fn half_alpha_white() -> Scene {
    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);
    builder.fill_rect(FILL, Brush::Solid(WHITE.with_alpha(0.5)));
    scene
}

/// The deterministic render inputs, over `base_color`.
fn spec(base_color: Color) -> RenderSpec {
    RenderSpec {
        width: SIZE,
        height: SIZE,
        base_color,
        scale: 1.0,
        root: Affine::IDENTITY,
    }
}

/// The RGBA bytes at `(x, y)`.
fn pixel(image: &RenderedImage, x: u32, y: u32) -> [u8; 4] {
    let at = ((y * image.width + x) * 4) as usize;
    image.rgba8[at..at + 4]
        .try_into()
        .expect("a readback row holds four bytes per pixel")
}

/// Every sampled pixel of `image` that is not `expected` within `tolerance`,
/// as review-ready messages.
///
/// Shared by the positive check and its negative control, so the control
/// exercises the very predicate the assertion rests on rather than a
/// look-alike written beside it.
fn polarity_failures(
    image: &RenderedImage,
    at: &[(u32, u32)],
    expected: [u8; 4],
    tolerance: u8,
) -> Vec<String> {
    at.iter()
        .filter_map(|&(x, y)| {
            let actual = pixel(image, x, y);
            let off = (0..4).any(|c| actual[c].abs_diff(expected[c]) > tolerance);
            off.then(|| {
                format!("pixel ({x}, {y}) is {actual:?}, expected {expected:?} +/- {tolerance}")
            })
        })
        .collect()
}

/// Converts a frame into the [`RgbaImage`] [`diff_images`] compares.
fn to_rgba_image(image: &RenderedImage) -> RgbaImage {
    RgbaImage::from_raw(image.width, image.height, image.rgba8.clone()).unwrap_or_else(|| {
        panic!(
            "rendered {}x{} frame ({} bytes) does not match width*height*4",
            image.width,
            image.height,
            image.rgba8.len()
        )
    })
}

/// Check 1 and its negative control (check 3): over a transparent
/// destination, the engine's readback is premultiplied, and the straight-alpha
/// misreading of the very same bytes is rejected.
#[test]
#[ignore = "requires a GPU; run locally with `cargo test -p frust-testing --test alpha_polarity -- --ignored`"]
fn half_alpha_white_over_a_transparent_destination_reads_premultiplied() {
    let _serialized = render_lock();
    let mut engine = EngineOracle::new(&EngineOracleOptions::default())
        .expect("failed to create the engine oracle");
    println!("alpha polarity: engine arm on {}", engine.adapter_meta());

    let image = engine
        .render(&half_alpha_white(), &spec(Color::TRANSPARENT))
        .expect("the polarity frame encodes");

    assert_eq!(
        image.alpha,
        AlphaKind::Premultiplied,
        "the engine renders into a premultiplied target and must say so — a mislabelled frame \
         sends every downstream comparison into the wrong space"
    );

    let failures = polarity_failures(&image, INTERIOR, PREMULTIPLIED, POLARITY_TOLERANCE);
    assert!(
        failures.is_empty(),
        "50%-alpha white over a transparent destination must read premultiplied {PREMULTIPLIED:?}, \
         not straight {STRAIGHT:?}:\n{}",
        failures.join("\n")
    );

    assert_eq!(
        pixel(&image, OUTSIDE.0, OUTSIDE.1),
        [0, 0, 0, 0],
        "a transparent destination outside the fill stays fully erased, all four channels zero"
    );

    // The negative control: straightening is exactly what a consumer that
    // misread this frame as straight alpha would be holding, and the check
    // above must reject it. A predicate that passed both would prove nothing.
    let misread = straighten_alpha(&image);
    assert_eq!(
        misread.alpha,
        AlphaKind::Straight,
        "the control must actually be in the other convention"
    );
    let control = polarity_failures(&misread, INTERIOR, PREMULTIPLIED, POLARITY_TOLERANCE);
    assert_eq!(
        control.len(),
        INTERIOR.len(),
        "every sampled pixel of the straight-alpha misreading must fail the premultiplied \
         check — got {control:?}"
    );
    let control_failures = polarity_failures(&misread, INTERIOR, STRAIGHT, POLARITY_TOLERANCE);
    assert!(
        control_failures.is_empty(),
        "and it must be the straight reading specifically, {STRAIGHT:?}:\n{}",
        control_failures.join("\n")
    );
}

/// Check 2: over an opaque black destination — a real composite rather than a
/// pass-through — the engine's premultiplied frame matches `vello_cpu`'s
/// premultiplied pixmap at the corpus's tight default tolerance.
#[test]
#[ignore = "requires a GPU; run locally with `cargo test -p frust-testing --test alpha_polarity -- --ignored`"]
fn half_alpha_white_over_an_opaque_black_destination_matches_vello_cpu() {
    let _serialized = render_lock();
    let mut engine = EngineOracle::new(&EngineOracleOptions::default())
        .expect("failed to create the engine oracle");
    let mut cpu = CpuOracle::new();

    let scene = half_alpha_white();
    let spec = spec(BLACK);
    let engine_image = engine
        .render(&scene, &spec)
        .expect("the engine frame encodes");
    let cpu_image = cpu
        .render(&scene, &spec)
        .expect("the CPU reference renders");

    // Neither is straightened: both arms hand back premultiplied bytes, so the
    // comparison is made in the space both actually produced rather than
    // through a lossy conversion that could hide a polarity difference.
    for (label, image) in [("engine", &engine_image), ("cpu", &cpu_image)] {
        assert_eq!(
            image.alpha,
            AlphaKind::Premultiplied,
            "the {label} arm must report premultiplied for this comparison to mean anything"
        );
        let failures = polarity_failures(image, INTERIOR, OVER_BLACK, POLARITY_TOLERANCE);
        assert!(
            failures.is_empty(),
            "the {label} arm must composite 50%-alpha white over opaque black to \
             {OVER_BLACK:?}:\n{}",
            failures.join("\n")
        );
    }

    let outcome = diff_images(
        &to_rgba_image(&cpu_image),
        &to_rgba_image(&engine_image),
        Tolerance::new(),
        false,
    );
    assert!(
        outcome.passed,
        "engine and `{}` disagree over an opaque destination beyond {:?}: {} px differ \
         ({:.4}%), max |delta| {:?}, bbox {:?}",
        cpu.id(),
        Tolerance::new(),
        outcome.report.pixel_count,
        outcome.report.mismatched_percent,
        outcome.report.max_difference,
        outcome.report.bounding_box
    );
}

/// The polarity predicate has teeth, proven on synthetic bytes with no GPU —
/// so a machine without one still catches a check that had stopped
/// discriminating.
#[test]
fn a_straight_alpha_misreading_of_a_premultiplied_frame_is_caught() {
    let premultiplied = RenderedImage {
        width: 1,
        height: 1,
        rgba8: PREMULTIPLIED.to_vec(),
        alpha: AlphaKind::Premultiplied,
        meta: BackendMeta::default(),
    };
    let at = &[(0, 0)];

    assert!(
        polarity_failures(&premultiplied, at, PREMULTIPLIED, POLARITY_TOLERANCE).is_empty(),
        "the premultiplied reading of a premultiplied pixel passes"
    );
    let misread = straighten_alpha(&premultiplied);
    assert_eq!(pixel(&misread, 0, 0), STRAIGHT);
    assert_eq!(
        polarity_failures(&misread, at, PREMULTIPLIED, POLARITY_TOLERANCE).len(),
        1,
        "and the straight-alpha misreading of the same pixel does not"
    );
}

/// The two conventions are separated by far more than the tolerance the checks
/// above are made at — the reason a rounding-step tolerance can never blur one
/// into the other.
#[test]
fn the_two_alpha_conventions_are_nowhere_near_each_other() {
    let gap = (0..3)
        .map(|c| PREMULTIPLIED[c].abs_diff(STRAIGHT[c]))
        .min()
        .expect("three colour channels");
    assert!(
        gap > POLARITY_TOLERANCE * 10,
        "the premultiplied/straight gap is {gap}, too close to the {POLARITY_TOLERANCE}-level \
         tolerance for these checks to discriminate"
    );
    assert_eq!(
        PREMULTIPLIED[3], STRAIGHT[3],
        "the conventions differ in the colour channels only — alpha itself is the same byte, \
         which is exactly why a wrong reading is not self-announcing"
    );
}
