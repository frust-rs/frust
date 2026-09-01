//! Painter order across a `ClearRect` hole punch: what survives an erase that
//! was recorded UNDER it.
//!
//! ```text
//! WGPU_BACKEND=metal FRUST_GOLDEN_EXPECT_BACKEND=metal \
//!   cargo test -p frust-testing --test aa_over_punch -- --ignored --nocapture
//! ```
//!
//! A platform-view slot is a `ClearRect` punched through the app's own opaque
//! backdrop, and the app then draws its own chrome — a chip, a scrim, a sheet —
//! back over the slot. The display list says that chrome is painted after the
//! erase and must therefore survive it. The engine hoists the punch to the
//! frame root ([`frust_engine::compile::clear`]) and issues it at that
//! painter-order position; this file is the pixel half of that contract, and
//! `frust-engine`'s own `renderer` unit tests are the pass-plan half.
//!
//! `vello_cpu` is the reference for every case that has one: the shared command
//! walk executes the punch as a `Compose::DestOut` layer exactly where the
//! display list recorded it, so agreement with it *is* agreement with painter
//! order. The four cases are chosen so that agreement cannot be reached by
//! accident:
//!
//! - **A, punch then chip.** The chip straddles the slot's edge, so the SAME
//!   antialiased arc is graded over the erased region on one side and over the
//!   untouched backdrop on the other. An erase issued after the chip binarizes
//!   the inside half and leaves the outside half alone, which no tolerance can
//!   confuse with a rasterizer disagreement.
//! - **B, punch then chip inside a translucent layer.** The chip reaches the
//!   surface as a page composite instead of as strips, and a composite writes
//!   no depth — so this case is served by painter order alone, with no depth
//!   test to fall back on. An erase issued last removes the composite whole.
//! - **C, chip then punch.** The control that the fix did not simply stop
//!   erasing: content recorded BEFORE the clear must still be erased, and the
//!   engine must still agree with `vello_cpu` about that.
//! - **D, opaque presentation.** Destination-out darkens colour as well as
//!   erasing alpha, so on a target whose alpha is disregarded the pass is
//!   dropped whole (`compile::clear`'s contract point 4). Its reference is not
//!   `vello_cpu` — which has no such rule and erases anyway — but the engine's
//!   own render of the same scene with the clear deleted, which is exactly the
//!   no-op the display list mandates there.

use std::sync::{Mutex, MutexGuard, PoisonError};

use image::RgbaImage;
use kurbo::{Affine, Rect};
use peniko::{Brush, Color};

use frust_scene::{Scene, SceneBuilder};
use frust_testing::case::Tolerance;
use frust_testing::diff::diff_images;
use frust_testing::oracle_cpu::CpuOracle;
use frust_testing::oracle_engine::{EngineOracle, EngineOracleOptions};
use frust_testing::render::{AlphaKind, BackendMeta, RenderSpec, RenderedImage, SceneRenderer};

/// Frame extent every case renders at — the corpus's own size, so an artifact
/// captured from a failure reads beside the rest of the corpus.
const SIZE: u32 = 64;

/// The platform-view slot: the rectangle erased to exactly `(0, 0, 0, 0)`.
///
/// Its right edge is the one the chip straddles, and it falls on a whole pixel
/// but on no 16-px strip-tile boundary, so a whole-tile erase would announce
/// itself here as well.
const SLOT: Rect = Rect::new(4.0, 4.0, 34.0, 60.0);

/// The chip drawn over the slot: rounded, so it carries a genuinely partial
/// antialiased arc, and wide enough to cross [`SLOT`]'s right edge with the
/// same arc on both sides of it.
const CHIP: Rect = Rect::new(14.0, 16.0, 54.0, 44.0);
const CHIP_RADIUS: f64 = 10.0;

/// The opacity that makes the chip's layer isolating in case B — strictly
/// between transparent and opaque, so it becomes a page composited over the
/// surface rather than a clip.
const LAYER_OPACITY: f32 = 0.5;

/// The tolerance the engine arm is compared to `vello_cpu` at: the corpus's
/// tight default, an 8-bit rounding step per channel with no pixel budget.
///
/// Deliberately not loosened for these cases. The two arms rasterize the same
/// rounded rectangle over the same erase, and the defect this file exists for
/// moves whole channels — 255 to 0 across an arc, and a whole composite — so a
/// rounding-step bar is both sufficient to catch it and, measured on the Metal
/// rig, met.
const TOLERANCE: Tolerance = Tolerance::new();

/// Serializes the GPU tests in this binary — see `tests/engine_goldens.rs`'s
/// identical lock for the teardown race it avoids. Poison is ignored
/// deliberately: one test's failure must not cascade into every sibling.
static RENDER_LOCK: Mutex<()> = Mutex::new(());

fn render_lock() -> MutexGuard<'static, ()> {
    RENDER_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The opaque backdrop the slot is punched through — what an app root paints.
fn backdrop() -> Brush {
    Brush::Solid(Color::from_rgb8(18, 18, 22))
}

/// The chip's own colour: opaque, so its interior joins the depth-writing pass
/// and only its ARC is left for painter order to protect.
fn chip() -> Brush {
    Brush::Solid(Color::from_rgb8(203, 190, 231))
}

fn scene_of(record: impl FnOnce(&mut SceneBuilder<'_>)) -> Scene {
    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);
    record(&mut builder);
    scene
}

fn fill_backdrop(builder: &mut SceneBuilder<'_>) {
    let edge = f64::from(SIZE);
    builder.fill_rect(Rect::new(0.0, 0.0, edge, edge), backdrop());
}

/// Case A: backdrop, slot, then the chip straight over it.
fn punch_then_chip() -> Scene {
    scene_of(|builder| {
        fill_backdrop(builder);
        builder.clear_rect(SLOT);
        builder.fill_rounded_rect(CHIP, CHIP_RADIUS, chip());
    })
}

/// Case B: the same chip inside a translucent layer, so it reaches the surface
/// as a composite rather than as strips.
fn punch_then_chip_in_layer() -> Scene {
    scene_of(|builder| {
        fill_backdrop(builder);
        builder.clear_rect(SLOT);
        builder.push_layer(CHIP, LAYER_OPACITY);
        builder.fill_rounded_rect(CHIP, CHIP_RADIUS, chip());
        builder.pop_layer();
    })
}

/// Case C: the chip recorded BEFORE the slot, which must therefore erase it.
fn chip_then_punch() -> Scene {
    scene_of(|builder| {
        fill_backdrop(builder);
        builder.fill_rounded_rect(CHIP, CHIP_RADIUS, chip());
        builder.clear_rect(SLOT);
    })
}

/// Case D's reference: case A with the clear deleted and nothing else changed.
fn chip_without_the_punch() -> Scene {
    scene_of(|builder| {
        fill_backdrop(builder);
        builder.fill_rounded_rect(CHIP, CHIP_RADIUS, chip());
    })
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

/// Every pixel the reference paints something at and the engine reads FULLY
/// ERASED, as review-ready messages.
///
/// This is the defect's own signature rather than a restatement of the
/// comparison below it: destination-out drives a covered pixel to exactly
/// `(0, 0, 0, 0)`, so a punch issued out of order does not perturb the pixels
/// it should have left alone — it empties them. An antialiased arc erased this
/// way binarizes (case A), and a composite erased this way disappears (case B),
/// and both land here as the same predicate. Reported before the whole-frame
/// diff because "1076 pixels the reference paints are empty" diagnoses a
/// misordered pass, while "1076 pixels differ" could be anything.
fn erased_where_the_reference_paints(
    engine: &RenderedImage,
    reference: &RenderedImage,
) -> Vec<String> {
    let mut failures = Vec::new();
    for y in 0..reference.height {
        for x in 0..reference.width {
            let expected = pixel(reference, x, y);
            if expected == [0, 0, 0, 0] || pixel(engine, x, y) != [0, 0, 0, 0] {
                continue;
            }
            failures.push(format!(
                "pixel ({x}, {y}) is fully erased, but the reference paints {expected:?} there"
            ));
        }
    }
    failures
}

/// Renders `scene` on both arms over a translucent presentation and asserts
/// they agree — first that the engine erased nothing the reference paints, then
/// that the two frames match within [`TOLERANCE`].
///
/// Both arms hand back PREMULTIPLIED bytes, so the comparison is made in the
/// space both actually produced rather than through a lossy conversion (the
/// footing `tests/alpha_polarity.rs` pins).
fn assert_arms_agree(case: &str, scene: &Scene) {
    let mut engine =
        EngineOracle::new(&EngineOracleOptions::default()).expect("failed to create the oracle");
    let mut cpu = CpuOracle::new();
    println!("[{case}] engine arm on {}", engine.adapter_meta());

    let spec = spec(Color::TRANSPARENT);
    let engine_image = engine
        .render(scene, &spec)
        .expect("the engine frame encodes");
    let cpu_image = cpu.render(scene, &spec).expect("the CPU reference renders");

    for (label, image) in [("engine", &engine_image), ("cpu", &cpu_image)] {
        assert_eq!(
            image.alpha,
            AlphaKind::Premultiplied,
            "the {label} arm must report premultiplied for this comparison to mean anything"
        );
    }

    let erased = erased_where_the_reference_paints(&engine_image, &cpu_image);
    assert!(
        erased.is_empty(),
        "[{case}] the punch erased {} pixel(s) `{}` paints — the pass ran out of painter \
         order:\n{}",
        erased.len(),
        cpu.id(),
        erased
            .iter()
            .take(12)
            .cloned()
            .collect::<Vec<String>>()
            .join("\n")
    );

    let outcome = diff_images(
        &to_rgba_image(&cpu_image),
        &to_rgba_image(&engine_image),
        TOLERANCE,
        false,
    );
    println!(
        "[{case}] engine vs {}: {} px differ ({:.4}%), max |delta| {:?}, mean |error| {:?}",
        cpu.id(),
        outcome.report.pixel_count,
        outcome.report.mismatched_percent,
        outcome.report.max_difference,
        outcome.report.mean_abs_error
    );
    assert!(
        outcome.passed,
        "[{case}] engine and `{}` disagree beyond {TOLERANCE:?}: {} px differ ({:.4}%), max \
         |delta| {:?}, bbox {:?}",
        cpu.id(),
        outcome.report.pixel_count,
        outcome.report.mismatched_percent,
        outcome.report.max_difference,
        outcome.report.bounding_box
    );
}

/// Case A: the chip's antialiased arc survives the erase it was drawn over, on
/// both sides of the slot's edge.
#[test]
#[ignore = "requires a GPU; run locally with `cargo test -p frust-testing --test aa_over_punch -- --ignored`"]
fn a_chip_drawn_over_a_punch_keeps_its_antialiased_edge() {
    let _serialized = render_lock();
    assert_arms_agree("A punch then chip", &punch_then_chip());
}

/// Case B: a translucent layer recorded over the slot composites onto it
/// instead of being erased by it — the shape that carries no depth at all.
#[test]
#[ignore = "requires a GPU; run locally with `cargo test -p frust-testing --test aa_over_punch -- --ignored`"]
fn a_translucent_layer_drawn_over_a_punch_composites_onto_it() {
    let _serialized = render_lock();
    assert_arms_agree("B punch then chip in layer", &punch_then_chip_in_layer());

    // The composite plainly did reach the target INSIDE the slot, rather than
    // the arms agreeing because both drew nothing there. Sampled well inside
    // both the chip and the slot, where the erased backdrop leaves the layer
    // alone against transparency: a half-opacity opaque chip, premultiplied.
    let mut engine =
        EngineOracle::new(&EngineOracleOptions::default()).expect("failed to create the oracle");
    let image = engine
        .render(&punch_then_chip_in_layer(), &spec(Color::TRANSPARENT))
        .expect("the engine frame encodes");
    let inside = pixel(&image, 24, 30);
    assert!(
        (i32::from(inside[3]) - 128).abs() <= 2,
        "inside both the layer and the slot the composite must land at about half alpha, \
         got {inside:?}"
    );
}

/// Case C: the control. Content recorded BEFORE the clear is still erased by
/// it, and the engine still agrees with the reference about that.
#[test]
#[ignore = "requires a GPU; run locally with `cargo test -p frust-testing --test aa_over_punch -- --ignored`"]
fn a_chip_recorded_before_a_punch_is_still_erased_by_it() {
    let _serialized = render_lock();
    assert_arms_agree("C chip then punch", &chip_then_punch());

    // And the erase really happened: the chip's own interior, well inside the
    // slot, is empty. Without this the case would pass just as well if the
    // punch had stopped erasing anything at all.
    let mut engine =
        EngineOracle::new(&EngineOracleOptions::default()).expect("failed to create the oracle");
    let image = engine
        .render(&chip_then_punch(), &spec(Color::TRANSPARENT))
        .expect("the engine frame encodes");
    assert_eq!(
        pixel(&image, 24, 30),
        [0, 0, 0, 0],
        "a chip recorded before the clear is erased inside the slot, colour and alpha alike"
    );
    assert_ne!(
        pixel(&image, 48, 30),
        [0, 0, 0, 0],
        "and the half of it outside the slot is untouched"
    );
}

/// Case D: on an opaque presentation the pass is dropped whole, so the frame
/// reads exactly as the same scene with no clear in it at all.
#[test]
#[ignore = "requires a GPU; run locally with `cargo test -p frust-testing --test aa_over_punch -- --ignored`"]
fn an_opaque_presentation_reads_exactly_as_if_the_clear_were_absent() {
    let _serialized = render_lock();
    let mut engine =
        EngineOracle::new(&EngineOracleOptions::default()).expect("failed to create the oracle");

    // An opaque base colour seals every pixel of the surface, which is the
    // presentation `compile::clear`'s contract point 4 skips the pass on.
    let opaque = spec(Color::from_rgb8(18, 18, 22));
    let punched = engine
        .render(&punch_then_chip(), &opaque)
        .expect("the punching frame encodes");
    let plain = engine
        .render(&chip_without_the_punch(), &opaque)
        .expect("the clear-free frame encodes");

    let outcome = diff_images(
        &to_rgba_image(&plain),
        &to_rgba_image(&punched),
        Tolerance::exact(),
        false,
    );
    assert!(
        outcome.passed,
        "an opaque presentation must read EXACTLY as it would without the clear — the punch \
         is dropped, not moved: {} px differ ({:.4}%), max |delta| {:?}, bbox {:?}",
        outcome.report.pixel_count,
        outcome.report.mismatched_percent,
        outcome.report.max_difference,
        outcome.report.bounding_box
    );

    // The frame is the one the case is about: an opaque surface carrying a chip
    // whose arc is graded rather than binarized, which is what proves the
    // comparison above was not two identically-broken frames.
    assert_eq!(
        pixel(&punched, 24, 30)[3],
        255,
        "an opaque presentation stays opaque inside the slot"
    );
}

/// The erasure predicate has teeth, proven on synthetic bytes with no GPU — so
/// a machine without one still catches a check that had stopped
/// discriminating.
#[test]
fn the_erasure_predicate_catches_an_emptied_pixel_and_nothing_else() {
    let image = |rgba: [u8; 4]| RenderedImage {
        width: 1,
        height: 1,
        rgba8: rgba.to_vec(),
        alpha: AlphaKind::Premultiplied,
        meta: BackendMeta::default(),
    };
    let painted = image([203, 190, 231, 255]);
    let fringe = image([101, 95, 115, 128]);
    let empty = image([0, 0, 0, 0]);

    assert_eq!(
        erased_where_the_reference_paints(&empty, &painted).len(),
        1,
        "an emptied pixel the reference paints is the whole point"
    );
    assert_eq!(
        erased_where_the_reference_paints(&empty, &fringe).len(),
        1,
        "an emptied ANTIALIASED pixel counts the same way — a binarized arc is \
         exactly this, one pixel at a time"
    );
    assert!(
        erased_where_the_reference_paints(&empty, &empty).is_empty(),
        "a pixel the reference leaves erased is not a failure — case C's whole \
         subject is the punch still erasing"
    );
    assert!(
        erased_where_the_reference_paints(&fringe, &painted).is_empty(),
        "and an ordinary rasterizer disagreement is left to the comparison \
         below the predicate, which is what measures it"
    );
}
