//! P3: derives, rather than guesses, the perceptual-divergence budget (P2)
//! between the two oracle arms — `frust-render`'s classic (headless GPU)
//! pipeline and `vello_cpu` 0.2.0 — over every corpus case that has a stable
//! reference on both.
//!
//! ```text
//! WGPU_BACKEND=vulkan WGPU_ADAPTER_NAME=T400 FRUST_GOLDEN_EXPECT_ADAPTER=T400 \
//!   cargo test -p frust-testing --test calibration -- --ignored --nocapture
//! ```
//!
//! # What this measures, and why it is not a golden gate
//!
//! Unlike `tests/goldens.rs`/`tests/adversarial.rs`, this file never touches
//! `testing/goldens/<class>/`: it renders the SAME case on both arms and
//! diffs one against the other directly (never against a stored PNG), purely
//! to characterise how far the two legitimately drift. `testing/goldens/
//! CALIBRATION.md` (this crate's write scope alongside this file) is the
//! reviewed, committed record of one such run's table plus the derived P2
//! budget; this test's own job is to keep that table reproducible, not to
//! gate a build on it.
//!
//! # Corpus and exclusions
//!
//! Every [`unit_cases`]/[`adversarial_cases`] case is a candidate, minus:
//!
//! - a case marked [`CaseSpec::no_ref`] (`adv-nan-transform`, `adv-5k-layers`)
//!   — its own pixels are not a stable reference on ANY backend (an
//!   intentionally malformed transform; a depth-only memory-budget probe), so
//!   there is nothing here to diverge FROM;
//! - a case whose [`CaseSpec::skip`] set excludes one arm (`unit-shader-quad`,
//!   skipped on the CPU oracle — there is no shader pre-pass without a GPU) —
//!   with only one arm's image to look at, a divergence cannot be measured.
//!
//! # Metric and measurement tolerance
//!
//! [`frust_testing::diff::diff_images`] is the same comparator every golden
//! gate uses, called here with a fixed measurement [`Tolerance`]
//! (`channel: 8, alpha: 8`) so "percent of pixels over 8" means the same
//! thing for every case, and with each case's OWN [`CorpusCase::eroded_interior`]
//! flag — the antialiased-edge-jitter allowance every golden comparison
//! already applies is exactly the allowance a cross-arm divergence budget
//! should apply too. Three per-case scalars come out of each comparison:
//!
//! - **channel**: the largest per-channel absolute difference among only the
//!   pixels [`DiffReport::pixels`] actually recorded — i.e. the ones that
//!   survived erosion (when the case asks for it) AND exceeded
//!   [`MEASURE_TOLERANCE`]. Deliberately NOT [`DiffReport::max_difference`],
//!   which `diff.rs` documents as a WHOLE-IMAGE statistic "regardless of
//!   erosion" — a single antialiased edge pixel routinely swings that field
//!   to 255 (a coverage flip: one arm rounds a boundary pixel in, the other
//!   out) and would swamp every case's number with edge conflation rather
//!   than the interior divergence this budget is meant to bound ("channel …
//!   on eroded interior" is the task's own phrasing for exactly this reason).
//!   R, G, B and A all count — a budget that only bounded RGB would let an
//!   alpha-channel regression (e.g. a hole-punch rounding difference) hide.
//! - **mean**: the average of [`DiffReport::mean_abs_error`]'s four entries —
//!   a genuine whole-image statistic by `diff.rs`'s own design, so it is used
//!   as-is rather than restricted to the eroded set.
//! - **pct_over_8**: [`DiffReport::mismatched_percent`] under
//!   [`MEASURE_TOLERANCE`], post-erosion when the case asks for it.
//!
//! # Families and the derived budget
//!
//! [`family_of`] buckets every case into one of the seven families the task
//! names (fills, strokes, text, clips, layers, images, blur) so a reviewer
//! can see which kind of content drives the tail. [`percentile`] computes
//! p50/p95/max per metric, per family, and once more over the whole corpus;
//! the whole-corpus p95 of each metric, rounded up, is the P2 budget this
//! test prints and `CALIBRATION.md` records.
//!
//! # Legitimate disagreements
//!
//! A case whose own doc comment already documents an approximation the two
//! arms are not expected to agree on exactly (`unit-blur-rrect`'s "the edge
//! ramp is exactly where two rasterizers legitimately differ") is measured
//! like any other — its numbers are exactly the data point that justifies
//! widening the budget rather than a reason to exclude it. `CALIBRATION.md`
//! names these cases explicitly so a later phase does not re-litigate them.

use image::RgbaImage;

use frust_testing::case::Tolerance;
use frust_testing::corpus::{CorpusCase, adversarial_cases, render_case, unit_cases};
use frust_testing::diff::{DiffReport, diff_images};
use frust_testing::oracle_classic::ClassicOracle;
use frust_testing::oracle_cpu::CpuOracle;
use frust_testing::render::{RenderedImage, SceneRenderer};

/// The fixed per-channel/alpha threshold every case is measured under, so
/// "percent of pixels over 8" means the same threshold for every case
/// regardless of that case's own [`CorpusCase::spec`]`.tolerance` (which
/// exists for a different purpose — gating that case's OWN golden
/// comparison, at whatever threshold was reviewed for it).
const MEASURE_TOLERANCE: Tolerance = Tolerance {
    channel: 8,
    alpha: 8,
    diff_pixels: 0,
};

/// The seven case families the task names, in the order a reviewer reads the
/// table.
const FAMILIES: &[&str] = &[
    "fills", "strokes", "text", "clips", "layers", "images", "blur",
];

/// Buckets a case's name into one of [`FAMILIES`] by the primitive its own
/// doc comment (`corpus::unit`/`corpus::adversarial`) names as its subject.
///
/// A case exercising more than one primitive (`adv-degenerate-path` records
/// both a fill and a stroke; `adv-unbalanced-pops` unwinds a clip, a layer,
/// AND a snapshot) is bucketed by whichever primitive dominates its `about`
/// line — see each arm below for the specific call. Panics on an unmapped
/// name rather than falling back to a silent bucket, so a corpus addition
/// forces this mapping to be updated instead of the table quietly missing a
/// case.
fn family_of(name: &str) -> &'static str {
    match name {
        // fills: a plain, rounded, or arbitrary-path solid fill, and the two
        // degenerate/subpixel geometry cases whose primary contract is a fill
        // that must survive malformed or sub-pixel input.
        "unit-fill-rect"
        | "unit-rounded-rect"
        | "unit-path-fill"
        | "adv-degenerate-path"
        | "adv-subpixel-rrect"
        | "adv-empty-scene" => "fills",
        // strokes: lines, stroked/dashed paths, and the hairline-at-scale
        // divider family.
        "unit-stroke-line"
        | "unit-path-stroke"
        | "unit-path-dashed"
        | "adv-1px-divider-1x"
        | "adv-1px-divider-2x"
        | "adv-1px-divider-2-75x" => "strokes",
        // text: glyph runs, ordinary and at stress-test glyph counts.
        "unit-glyph-run" | "adv-10k-glyphs" => "text",
        // clips: PushClip/PushClipRounded/PopClip, balanced or nested deep.
        "unit-clip-rect" | "unit-clip-rounded" | "unit-clip-balance" | "adv-clip-nest-8" => "clips",
        // layers: PushLayer/PopLayer opacity groups, the snapshot bracket
        // (itself an opacity group plus an inline scale correction), the
        // hole-punch (its contract is specifically about erasing through an
        // enclosing group), and the multi-stack unbalanced-pop case (clip,
        // layer, AND snapshot unwound together — grouped here because the
        // layer/snapshot stack is what makes it distinct from the plainer
        // per-stack `clip_balance`/`layer_balance` cases).
        "unit-layer-alpha"
        | "unit-layer-balance"
        | "unit-layer-sibling-fan"
        | "unit-layer-nested-pair"
        | "unit-clear-rect"
        | "unit-snapshot-bracket"
        | "unit-snapshot-balance"
        | "adv-destout-in-layer"
        | "adv-snapshot-scale-alpha"
        | "adv-unbalanced-pops" => "layers",
        // images: Command::Image at ordinary and oversized source dimensions.
        "unit-image" | "adv-huge-image" => "images",
        // blur: the one blurred-rounded-rect case.
        "unit-blur-rrect" => "blur",
        // `unit-shader-quad` is excluded before `family_of` is ever called
        // (see the module docs); `adv-nan-transform`/`adv-5k-layers` likewise
        // (`no_ref`). Anything else reaching here is a corpus case this
        // mapping has not been taught about yet.
        other => panic!(
            "calibration: `{other}` has no family mapping — add it to `family_of` before running"
        ),
    }
}

/// One case's measured cross-arm divergence.
#[derive(Clone, Debug)]
struct CaseMetrics {
    name: &'static str,
    family: &'static str,
    /// The largest per-channel absolute difference among the eroded-interior
    /// mismatched pixels only (see the module docs for why this is not
    /// [`DiffReport::max_difference`]).
    channel: u8,
    /// The average of [`DiffReport::mean_abs_error`]'s four channels.
    mean: f64,
    /// [`DiffReport::mismatched_percent`] under [`MEASURE_TOLERANCE`],
    /// post-erosion when the case asks for it.
    pct_over_8: f64,
    report: DiffReport,
}

/// Converts a straight-alpha [`RenderedImage`] into the [`RgbaImage`]
/// [`diff_images`] compares — the same conversion `golden.rs` performs before
/// every comparison.
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

/// Renders `case` on both arms and measures their divergence, or `None` when
/// either arm skips the case (only `unit-shader-quad`, per the module docs).
fn measure(
    classic: &mut ClassicOracle,
    cpu: &mut CpuOracle,
    case: &CorpusCase,
) -> Option<CaseMetrics> {
    let classic_image = render_case(classic, case)
        .unwrap_or_else(|err| panic!("case `{}` failed on classic: {err:#}", case.spec.name));
    let cpu_image = render_case(cpu, case)
        .unwrap_or_else(|err| panic!("case `{}` failed on cpu: {err:#}", case.spec.name));

    let (classic_image, cpu_image) = match (classic_image, cpu_image) {
        (Some(c), Some(p)) => (c, p),
        _ => {
            println!(
                "calibration: `{}` skipped on at least one arm — no cross-arm comparison \
                 possible",
                case.spec.name
            );
            return None;
        }
    };

    let outcome = diff_images(
        &to_rgba_image(&classic_image),
        &to_rgba_image(&cpu_image),
        MEASURE_TOLERANCE,
        case.eroded_interior,
    );
    let report = outcome.report;
    assert!(
        !report.truncated,
        "case `{}` mismatched more than {} pixels — `channel`'s eroded-interior max would be \
         incomplete; widen MAX_RECORDED_PIXELS or investigate the mismatch before trusting this \
         run's numbers",
        case.spec.name,
        frust_testing::diff::MAX_RECORDED_PIXELS
    );
    let channel = report
        .pixels
        .iter()
        .flat_map(|p| p.difference)
        .map(|d| u8::try_from(d.unsigned_abs()).unwrap_or(u8::MAX))
        .max()
        .unwrap_or(0);
    let mean = report.mean_abs_error.iter().sum::<f64>() / report.mean_abs_error.len() as f64;

    Some(CaseMetrics {
        name: case.spec.name,
        family: family_of(case.spec.name),
        channel,
        mean,
        pct_over_8: report.mismatched_percent,
        report,
    })
}

/// The `p`-th percentile (0.0..=100.0) of `values` by linear interpolation
/// between the two nearest ranks — `values` need not be pre-sorted; this
/// clones and sorts its own copy.
///
/// `values` must be non-empty; returns `0.0` for an empty slice rather than
/// panicking, since an empty family (no cases measured) is reported as such
/// by the caller before this would ever be reached with real data.
fn percentile(values: &[f64], p: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    if sorted.len() == 1 {
        return sorted[0];
    }
    let rank = (p / 100.0) * (sorted.len() - 1) as f64;
    let lower = rank.floor() as usize;
    let upper = rank.ceil() as usize;
    if lower == upper {
        return sorted[lower];
    }
    let weight = rank - lower as f64;
    sorted[lower] * (1.0 - weight) + sorted[upper] * weight
}

/// Rounds `value` up to the nearest multiple of `step` — how the whole-corpus
/// p95 becomes the stated P2 budget (`docs/TESTING.md`'s Comparison section:
/// "reviewed, fixed tolerances", not a raw percentile carried verbatim).
fn round_up_to(value: f64, step: f64) -> f64 {
    (value / step).ceil() * step
}

/// Prints one metric's p50/p95/max, per family and overall — the table
/// `CALIBRATION.md` transcribes.
fn print_metric_table(
    label: &str,
    metrics: &[CaseMetrics],
    value_of: impl Fn(&CaseMetrics) -> f64,
) {
    println!("\n-- {label} --");
    println!(
        "{:<10} {:>6} {:>8} {:>8} {:>8}",
        "family", "n", "p50", "p95", "max"
    );
    for family in FAMILIES {
        let values: Vec<f64> = metrics
            .iter()
            .filter(|m| &m.family == family)
            .map(&value_of)
            .collect();
        if values.is_empty() {
            println!("{family:<10} {:>6} {:>8} {:>8} {:>8}", 0, "-", "-", "-");
            continue;
        }
        let max = values.iter().copied().fold(f64::MIN, f64::max);
        println!(
            "{family:<10} {:>6} {:>8.3} {:>8.3} {:>8.3}",
            values.len(),
            percentile(&values, 50.0),
            percentile(&values, 95.0),
            max
        );
    }
    let all: Vec<f64> = metrics.iter().map(&value_of).collect();
    let max = all.iter().copied().fold(f64::MIN, f64::max);
    println!(
        "{:<10} {:>6} {:>8.3} {:>8.3} {:>8.3}",
        "OVERALL",
        all.len(),
        percentile(&all, 50.0),
        percentile(&all, 95.0),
        max
    );
}

/// Derives the classic-vs-`vello_cpu` divergence budget (P2) over every
/// corpus case with a stable reference on both arms, and prints the
/// per-family/overall distribution table `CALIBRATION.md` records.
///
/// `#[ignore]`d like every other GPU test in this workspace — see
/// `tests/goldens.rs`'s identical test for the rationale.
#[test]
#[ignore = "requires a GPU; run locally with \
            `cargo test -p frust-testing --test calibration -- --ignored --nocapture`"]
fn calibrates_the_engine_vs_classic_perceptual_budget() {
    let mut classic = ClassicOracle::new(frust_render::HeadlessOptions::default())
        .expect("failed to create the classic (headless GPU) oracle");
    let mut cpu = CpuOracle::new();

    println!("calibration: classic arm on {}", classic.headless_meta());
    println!("calibration: classic golden class `{}`", classic.id());
    println!("calibration: cpu arm `{}`", cpu.id());

    let mut metrics: Vec<CaseMetrics> = Vec::new();
    let mut excluded_no_ref: Vec<&str> = Vec::new();

    for case in unit_cases().into_iter().chain(adversarial_cases()) {
        if case.spec.no_ref {
            excluded_no_ref.push(case.spec.name);
            println!(
                "calibration: `{}` excluded — no_ref, no reference to diverge from",
                case.spec.name
            );
            continue;
        }
        if let Some(measured) = measure(&mut classic, &mut cpu, &case) {
            println!(
                "calibration: [{}] family={} channel={} mean={:.3} pct_over_8={:.4}% bbox={:?}",
                measured.name,
                measured.family,
                measured.channel,
                measured.mean,
                measured.pct_over_8,
                measured.report.bounding_box
            );
            metrics.push(measured);
        }
    }

    assert!(
        !metrics.is_empty(),
        "calibration measured zero cases — every case was either no_ref or skipped on one arm"
    );

    print_metric_table("channel (max abs diff, 0-255)", &metrics, |m| {
        f64::from(m.channel)
    });
    print_metric_table("mean abs error (0-255)", &metrics, |m| m.mean);
    print_metric_table("% pixels over 8 (post-erosion)", &metrics, |m| m.pct_over_8);

    let channel_p95 = percentile(
        &metrics
            .iter()
            .map(|m| f64::from(m.channel))
            .collect::<Vec<_>>(),
        95.0,
    );
    let mean_p95 = percentile(&metrics.iter().map(|m| m.mean).collect::<Vec<_>>(), 95.0);
    let pct_p95 = percentile(
        &metrics.iter().map(|m| m.pct_over_8).collect::<Vec<_>>(),
        95.0,
    );

    println!(
        "\ncalibration: derived P2 budget (whole-corpus p95, rounded up) — channel <= {}, \
         mean <= {:.1}, % pixels over 8 <= {:.1}%",
        round_up_to(channel_p95, 1.0),
        round_up_to(mean_p95, 0.1),
        round_up_to(pct_p95, 0.1)
    );
    println!(
        "calibration: {} case(s) measured, {} excluded as no_ref ({:?})",
        metrics.len(),
        excluded_no_ref.len(),
        excluded_no_ref
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_measurable_case_has_a_family() {
        // GPU-free tripwire: every case this test would ever reach with a
        // real render already resolves through `family_of` without
        // panicking, so a corpus addition is caught here on any machine
        // rather than only on the pinned GPU runner.
        for case in unit_cases().into_iter().chain(adversarial_cases()) {
            if case.spec.no_ref || case.spec.name == "unit-shader-quad" {
                continue;
            }
            let family = family_of(case.spec.name);
            assert!(
                FAMILIES.contains(&family),
                "`{}` mapped to `{family}`, which is not one of {FAMILIES:?}",
                case.spec.name
            );
        }
    }

    #[test]
    fn percentile_matches_hand_worked_examples() {
        let values = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(percentile(&values, 0.0), 1.0);
        assert_eq!(percentile(&values, 50.0), 3.0);
        assert_eq!(percentile(&values, 100.0), 5.0);
        // Rank = 0.95 * 4 = 3.8 -> interpolates 80% of the way from index 3
        // (4.0) to index 4 (5.0).
        assert!((percentile(&values, 95.0) - 4.8).abs() < 1e-9);
    }

    #[test]
    fn percentile_of_a_single_value_is_itself() {
        assert_eq!(percentile(&[7.0], 50.0), 7.0);
        assert_eq!(percentile(&[7.0], 95.0), 7.0);
    }

    #[test]
    fn round_up_to_never_returns_below_the_input() {
        assert_eq!(round_up_to(7.1, 1.0), 8.0);
        assert_eq!(round_up_to(7.0, 1.0), 7.0);
        assert!((round_up_to(0.42, 0.1) - 0.5).abs() < 1e-9);
    }
}
