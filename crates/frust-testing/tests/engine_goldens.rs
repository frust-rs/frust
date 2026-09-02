//! The engine's primary correctness gate: every corpus case the engine can
//! draw, rendered by [`EngineOracle`] and held against two references —
//! `vello_cpu` 0.2.0 (P1) and the engine's own committed golden class. Both
//! are HARD: nothing here is advisory.
//!
//! ```text
//! WGPU_BACKEND=vulkan WGPU_ADAPTER_NAME=T400 FRUST_GOLDEN_EXPECT_ADAPTER=T400 \
//!   cargo test -p frust-testing --test engine_goldens -- --ignored --nocapture
//! ```
//!
//! The host-only tests in this file (scope coverage, class routing, the
//! command-support tripwire) run in the ordinary gate with no GPU.
//!
//! # The P1 comparison, and the bar it is held to
//!
//! **Engine vs `vello_cpu` is near-exact.** Both rasterize the same geometry
//! with the same `vello_common` 0.2.0 strip generator at the same flattening
//! tolerance; only the *fill* differs (a GPU strip pass against a CPU one).
//! So the bar is each case's OWN [`CaseSpec::tolerance`] — the corpus default
//! of channel 2, alpha 2, zero tolerated mismatched pixels for most cases,
//! and [`Tolerance::exact`] for the two whose subject is integer-aligned
//! erase arithmetic (`unit-clear-rect`, `adv-destout-in-layer`), which
//! therefore compare PIXEL-EXACT here as well as against their baseline. A
//! per-case widening exists only through [`ESCALATIONS`] and only with a
//! stated reason. The comparison is made on the PREMULTIPLIED frames both
//! arms natively produce rather than on their straightened forms — see
//! [`render_raw`], where that is the difference between measuring the
//! rasterizers and measuring an unpremultiply's own information loss.
//!
//! # Which cases are in scope
//!
//! The engine compiles axis-aligned rectangles, rounded rectangles, lines,
//! arbitrary filled/stroked paths, rect and rounded clips, opacity layers,
//! snapshot brackets, `ClearRect` hole punches, atlas-resident images,
//! blurred rounded rects, and glyph runs (incl. COLR colour glyphs and the
//! atlas-vs-outline route), and resolves solid and gradient paints. One
//! command remains recognised-and-skipped by its compiler —
//! [`Command::ShaderQuad`] — so a case that draws it would compare an engine
//! frame legitimately missing the very thing the case exists to pin.
//!
//! [`PHASE_CASES`] names the cases the engine draws in full, across all five
//! corpora ([`unit_cases`], [`adversarial_cases`], [`widget_cases`],
//! [`page_cases`], [`text_cases`]), and [`DEFERRED_CASES`] every other one
//! with why it waits. Two GPU-free tripwires keep those lists honest rather
//! than aspirational: [`every_corpus_case_is_either_in_scope_or_deferred`]
//! fails if a corpus addition appears in neither, and
//! [`every_scoped_case_draws_only_commands_the_engine_compiles`] re-derives
//! the membership rule from the scenes themselves, so a case cannot be listed
//! in scope while recording a command the engine skips.
//!
//! # Promotion
//!
//! `UPDATE_GOLDENS=1` is the only path that writes a baseline
//! (`frust_testing::golden`), narrowed twice more here, exactly as the other
//! golden gates narrow it: a case whose probes or cross-arm comparison FAIL is
//! never promoted, and an adapter with no reviewed engine class
//! ([`frust_testing::ENGINE_UNCLASSIFIED_CLASS`]) refuses to promote at all
//! rather than inventing a directory for an unreviewed machine.

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use image::RgbaImage;

use frust_scene::Command;
use frust_testing::case::{CaseSpec, Tolerance};
use frust_testing::corpus::{
    CorpusCase, adversarial_cases, page_cases, straighten_alpha, text_cases, unit_cases,
    widget_cases,
};
use frust_testing::diff::{DiffReport, diff_images};
use frust_testing::frame::foreign_font_runs;
use frust_testing::golden::{compare_golden, goldens_root, update_goldens_enabled};
use frust_testing::meta::GoldenMeta;
use frust_testing::oracle_cpu::CpuOracle;
use frust_testing::oracle_engine::{EngineOracle, EngineOracleOptions};
use frust_testing::render::{RenderedImage, SceneRenderer};
use frust_testing::{ENGINE_UNCLASSIFIED_CLASS, engine_golden_class};

/// Every corpus case the engine draws in full — rectangles, rounded
/// rectangles, lines, arbitrary filled/stroked/dashed paths, rect and rounded
/// clips, opacity layers, snapshot brackets, `ClearRect` hole punches,
/// atlas-resident images, blurred rounded rects, glyph runs (Latin, RTL,
/// CJK, combining marks, COLR colour, gradient-brushed, clipped), and the
/// widget/page/text frames built out of those same commands.
///
/// Membership is not a judgement call: a case belongs here exactly when its
/// scene records no [`Command::ShaderQuad`], and
/// [`every_scoped_case_draws_only_commands_the_engine_compiles`] re-derives
/// that from the scenes on every run.
const PHASE_CASES: &[&str] = &[
    // Unit — one command at a time, recorded by hand.
    "unit-fill-rect",
    "unit-rounded-rect",
    "unit-stroke-line",
    "unit-glyph-run",
    "unit-clip-rect",
    "unit-clip-rounded",
    "unit-clip-balance",
    "unit-image",
    "unit-blur-rrect",
    "unit-layer-alpha",
    "unit-layer-balance",
    "unit-layer-sibling-fan",
    "unit-layer-nested-pair",
    "unit-clear-rect",
    "unit-path-fill",
    "unit-path-stroke",
    "unit-path-dashed",
    "unit-snapshot-bracket",
    "unit-snapshot-balance",
    // Adversarial — the degenerate and stress shapes of the same commands.
    "adv-degenerate-path",
    "adv-subpixel-rrect",
    "adv-empty-scene",
    "adv-1px-divider-1x",
    "adv-1px-divider-2x",
    "adv-1px-divider-2-75x",
    "adv-clip-nest-8",
    "adv-destout-in-layer",
    "adv-snapshot-scale-alpha",
    "adv-unbalanced-pops",
    "adv-huge-image",
    "adv-10k-glyphs",
    // Widget frames — a real `frust-core` render tree painted at phone size.
    "widget-button-rest",
    "widget-button-pressed",
    "widget-button-disabled",
    "widget-flex-layout",
    "widget-text-field-rest",
    "widget-text-field-caret",
    "widget-list-20-rows",
    "widget-checkbox-radio",
    "widget-slider",
    "widget-scaffold-chrome",
    "widget-stack-align",
    "widget-scroll-clipped",
    "widget-text-wrap",
    // Page frames — catalog pages from the four design-system plugin crates.
    "page-material-home",
    "page-material-dialog",
    "page-cupertino-settings",
    "page-cupertino-controls",
    "page-glyph-dashboard",
    "page-glyph-surfaces",
    "page-shadcn-form",
    "page-shadcn-controls",
    // Text — script/paint/clip coverage for `Command::GlyphRun` on its own.
    "text-latin-mixed-sizes",
    "text-rtl-arabic",
    "text-cjk",
    "text-combining",
    "text-colr-emoji",
    "text-gradient-brush",
    "text-clipped",
];

/// Every corpus case NOT in [`PHASE_CASES`], with why it waits.
///
/// Exactly two reasons are admissible: the case's subject is a command the
/// compiler recognises and skips (so the frame it produces is deliberately
/// missing the very thing the case exists to pin), or the case is `no_ref`
/// and has no stable reference on any backend at all. Neither is a defect to
/// chase here.
/// [`every_deferred_case_is_deferred_for_a_reason_the_corpus_can_confirm`]
/// checks that each row is one of those two and not a habit.
const DEFERRED_CASES: &[(&str, &str)] = &[
    (
        "unit-shader-quad",
        "a shader pre-pass the engine does not own; also skipped on the CPU arm, so there \
         would be no reference either way",
    ),
    (
        "adv-nan-transform",
        "`no_ref`: deliberately malformed transform components, no stable reference on any \
         backend — and a non-finite transform is a refusal on the engine's frame path",
    ),
    (
        "adv-5k-layers",
        "`no_ref`: a depth-only memory-budget probe over 5,000 nested layers whose own pixels \
         are not a reference on any backend",
    ),
    (
        "adv-unbalanced-pop-in-snapshot",
        "`no_ref`: an unbalanced pop inside a translucent snapshot bracket, whose recovery \
         shape is deliberately not pinned to any backend's pixels",
    ),
    (
        "text-10k",
        "`no_ref`: a wrapped ~10,000-glyph block shaped to stress the layout/wrap path — most \
         of it renders off the bottom of its deliberately small viewport, so there is nothing \
         stable on any backend for a stored baseline to pin",
    ),
];

/// Per-case escalations off the corpus's tight default, each with the reason
/// it is not the default's fault.
///
/// `docs/TESTING.md`'s Comparison section puts thresholds on "the golden
/// class or named test, not an ad hoc retry path", and nothing in this file
/// re-runs a comparison at a looser threshold: an escalation is a reviewed
/// row here or it does not exist. It lives beside the harness rather than on
/// the shared [`CaseSpec`] because it is specific to the ENGINE-vs-CPU
/// pairing — the same case's own `cpu/` baseline comparison is unaffected by
/// anything written here.
///
/// # The hint-policy rows (text phase, p5-04)
///
/// Every scoped case that shapes a glyph run carries one of these, `text-colr-
/// emoji` excepted: a COLR glyph is hinted on neither side, so its row is a
/// colour-boundary one measured on its own terms and is much the narrowest of
/// the group. The engine's `SceneCompiler::for_caps` turns hinting ON for a desktop-tier
/// adapter — `hint_text = !is_mobile_tier(caps)`, this pairing's own T400 rig
/// included — while [`CpuOracle`](crate::oracle_cpu::CpuOracle) hints OFF
/// UNCONDITIONALLY (`oracle_cpu.rs`'s `draw_glyph_run`: "hinting is a
/// per-size, per-target adjustment, and a golden reference must not vary with
/// it"). That is not a bug on either side — it is exactly p5-03's documented
/// hint policy (`docs/RENDER_ARCHITECTURE.md`/`RENDER_DEVELOPMENT.md`: hinted
/// on desktop for on-screen quality, unhinted on mobile/CPU-reference paths
/// for determinism) — but it means a hinted glyph's outline snaps to the
/// pixel grid a fraction of a pixel away from `vello_cpu`'s unhinted one,
/// which a solid-colour glyph reads as a channel delta at its own antialiased
/// edge and nowhere else (every row's bounding box sits exactly on glyph
/// ink). Each row's `channel` is that case's own measured `max |delta|`
/// (`R`/`G`/`B`, checked together since these are luminance-only glyphs
/// bar `text-gradient-brush`/`text-colr-emoji`), captured on the pinned T400
/// rig with `cargo test -p frust-testing --test engine_goldens -- --ignored
/// --nocapture`; `alpha` only widens past the corpus default where that run
/// also measured an alpha delta above it. None of these touch `diff_pixels`:
/// the widened channel/alpha already brings every measured pixel back inside
/// tolerance (each case's own gate line reports 0 px differing once its row
/// here applies).
///
/// # The metal-macos rig's own answer (p6-d1)
///
/// The Apple M4 answers MOBILE tier (`transient_saves_memory` is true on
/// Apple silicon's TBDR hardware — `frust_engine::cache::images::
/// is_mobile_tier` reads the capability, not the adapter name), so the
/// engine hints OFF on that rig and the hint-policy rows above are slack
/// there: every hint-policy case measured its M4 delta at or under
/// [55,55,54,1] (`widget-text-field-rest`/`-caret`, the widest) with most at
/// [1,1,1,0], and `unit-glyph-run` measures [0,0,0,0] under
/// `FRUST_ENGINE_NO_ATLAS=1`. What that rig DOES measure is the atlas route
/// itself — `adv-10k-glyphs`' row below is the one row of the M4's own, and
/// it is an atlas subpixel-bucket row rather than a hint-policy one.
const ESCALATIONS: &[(&str, Tolerance, &str)] = &[
    (
        "widget-button-disabled",
        Tolerance {
            channel: 3,
            alpha: 3,
            diff_pixels: 0,
        },
        "32 pixels (0.0022%) on the button's corner arcs differ by one level beyond the corpus \
         tolerance (max delta [3,3,3,2]): a coverage-rounding difference between the GPU strip \
         fill and `vello_cpu` on stacked TRANSLUCENT draws — the disabled state dims every colour's \
         alpha, and the same geometry with opaque colours (`widget-button-rest`/`-pressed`) is 0 \
         pixels differing. The identical 32 pixels, deltas and values reproduce on a tree predating \
         the engine's layer execution, so this is the shared strip fill's rounding, not layer \
         compositing",
    ),
    (
        "unit-glyph-run",
        Tolerance {
            channel: 104,
            alpha: 2,
            diff_pixels: 0,
        },
        "hint-policy row (see this table's module docs): measured max delta [104,104,104,0], 231 \
         px (5.6396%) of this 64x64 frame's own glyph ink, all inside the text block's bounding \
         box — a hinted vs. unhinted `H`/`e`/`l`/`l`/`o` outline, not a rasterizer disagreement",
    ),
    (
        "adv-10k-glyphs",
        Tolerance {
            channel: 57,
            alpha: 2,
            diff_pixels: 0,
        },
        "atlas subpixel-bucket row, measured on the metal-macos rig (Apple M4) — see this \
         table's module docs, and NOT a hint-policy row (that rig hints OFF): the atlas renders \
         the pack's one repeated 8px `l` at glifo's quarter-pixel horizontal buckets while \
         `vello_cpu` rasterizes every exact fraction of the pack's 0.55px grid, and the worst \
         phase reads as 5 px (0.1221%) at delta [57,57,57,0] on the eroded interior — x = 2, 13, \
         24, 35, 46 on row y=59, the 11px period the 0.55px step's fractional phase repeats at, \
         on the sparse bottom row where overlap no longer saturates the difference away (the \
         whole-image max [126,126,126,0] sits on the eroded border). `FRUST_ENGINE_NO_ATLAS=1` \
         drops the case to [3,3,3,0] / 0 px differing, isolating the bucketing; the T400 rig \
         passes this case at the corpus default, so this row carries the metal rig's own \
         measured number rather than a copied one",
    ),
    (
        "widget-text-field-rest",
        Tolerance {
            channel: 230,
            alpha: 2,
            diff_pixels: 0,
        },
        "hint-policy row (see this table's module docs): measured max delta [230,230,226,1], 1681 \
         px (0.1144%) of an 824x1784 frame, confined to the field's placeholder label glyphs",
    ),
    (
        "widget-text-field-caret",
        Tolerance {
            channel: 230,
            alpha: 2,
            diff_pixels: 0,
        },
        "hint-policy row (see this table's module docs): identical measured delta and pixel count \
         to `widget-text-field-rest` — the same label glyphs, this frame's own caret adds no glyph \
         of its own",
    ),
    (
        "widget-list-20-rows",
        Tolerance {
            channel: 153,
            alpha: 2,
            diff_pixels: 0,
        },
        "hint-policy row (see this table's module docs): measured max delta [152,153,153,0], 3941 \
         px (0.2681%) of an 824x1784 frame, confined to the twenty rows' own label glyphs",
    ),
    (
        "widget-checkbox-radio",
        Tolerance {
            channel: 159,
            alpha: 2,
            diff_pixels: 0,
        },
        "hint-policy row (see this table's module docs): measured max delta [159,159,157,0], 337 \
         px (0.0229%) of an 824x1784 frame, confined to the row's own text labels",
    ),
    (
        "widget-slider",
        Tolerance {
            channel: 159,
            alpha: 2,
            diff_pixels: 0,
        },
        "hint-policy row (see this table's module docs): identical measured delta and pixel count \
         to `widget-checkbox-radio` — both frames share the same labelled-row chrome",
    ),
    (
        "widget-scaffold-chrome",
        Tolerance {
            channel: 235,
            alpha: 2,
            diff_pixels: 0,
        },
        "hint-policy row (see this table's module docs): measured max delta [235,235,231,0], 1301 \
         px (0.0885%) of an 824x1784 frame, confined to the scaffold's app-bar/nav-label glyphs",
    ),
    (
        "widget-stack-align",
        Tolerance {
            channel: 159,
            alpha: 2,
            diff_pixels: 0,
        },
        "hint-policy row (see this table's module docs): identical measured delta and pixel count \
         to `widget-checkbox-radio` — both frames share the same labelled-row chrome",
    ),
    (
        "widget-scroll-clipped",
        Tolerance {
            channel: 225,
            alpha: 2,
            diff_pixels: 0,
        },
        "hint-policy row (see this table's module docs): measured max delta [223,225,225,1], 4338 \
         px (0.2951%) of an 824x1784 frame, confined to the scrolled content's own text labels",
    ),
    (
        "widget-text-wrap",
        Tolerance {
            channel: 235,
            alpha: 2,
            diff_pixels: 0,
        },
        "hint-policy row (see this table's module docs): measured max delta [235,235,231,0], 4138 \
         px (0.2815%) of an 824x1784 frame, confined to the wrapped paragraph's own glyphs",
    ),
    (
        "page-material-home",
        Tolerance {
            channel: 174,
            alpha: 2,
            diff_pixels: 0,
        },
        "hint-policy row (see this table's module docs): measured max delta [174,169,172,0], 2153 \
         px (0.1465%) of an 824x1784 catalog page, confined to its own text labels",
    ),
    (
        "page-material-dialog",
        Tolerance {
            channel: 182,
            alpha: 2,
            diff_pixels: 0,
        },
        "hint-policy row (see this table's module docs): measured max delta [180,181,182,0], 840 \
         px (0.0571%) of an 824x1784 catalog page, confined to its own text labels",
    ),
    (
        "page-cupertino-settings",
        Tolerance {
            channel: 235,
            alpha: 3,
            diff_pixels: 0,
        },
        "hint-policy row (see this table's module docs): measured max delta [235,235,231,3], 1392 \
         px (0.0947%) of an 824x1784 catalog page, confined to its own text labels — the one row \
         here whose alpha delta (1) also needs a step past the corpus default",
    ),
    (
        "page-cupertino-controls",
        Tolerance {
            channel: 151,
            alpha: 2,
            diff_pixels: 0,
        },
        "hint-policy row (see this table's module docs): measured max delta [150,150,151,1], 347 \
         px (0.0236%) of an 824x1784 catalog page, confined to its own text labels",
    ),
    (
        "page-glyph-dashboard",
        Tolerance {
            channel: 213,
            alpha: 2,
            diff_pixels: 0,
        },
        "hint-policy row (see this table's module docs): measured max delta [208,210,213,1], 2425 \
         px (0.1650%) of an 824x1784 catalog page, confined to its own text labels",
    ),
    (
        "page-glyph-surfaces",
        Tolerance {
            channel: 214,
            alpha: 2,
            diff_pixels: 0,
        },
        "hint-policy row (see this table's module docs): measured max delta [209,211,214,0], 681 \
         px (0.0463%) of an 824x1784 catalog page, confined to its own text labels",
    ),
    (
        "page-shadcn-form",
        Tolerance {
            channel: 224,
            alpha: 2,
            diff_pixels: 0,
        },
        "hint-policy row (see this table's module docs): measured max delta [224,224,221,1], 1136 \
         px (0.0773%) of an 824x1784 catalog page, confined to its own text labels",
    ),
    (
        "page-shadcn-controls",
        Tolerance {
            channel: 159,
            alpha: 2,
            diff_pixels: 0,
        },
        "hint-policy row (see this table's module docs): measured max delta [159,159,157,1], 816 \
         px (0.0555%) of an 824x1784 catalog page, confined to its own text labels",
    ),
    (
        "text-latin-mixed-sizes",
        Tolerance {
            channel: 243,
            alpha: 2,
            diff_pixels: 0,
        },
        "hint-policy row (see this table's module docs): measured max delta [243,243,243,0], 820 \
         px (2.1579%) of this case's own five stacked Latin lines — the smallest sizes (8-16px) \
         are the most hinting-sensitive, which is exactly what this case exists to cover",
    ),
    (
        "text-rtl-arabic",
        Tolerance {
            channel: 187,
            alpha: 2,
            diff_pixels: 0,
        },
        "hint-policy row (see this table's module docs): measured max delta [187,187,187,0], 323 \
         px (1.6150%) of this case's own shaped word",
    ),
    (
        "text-cjk",
        Tolerance {
            channel: 233,
            alpha: 2,
            diff_pixels: 0,
        },
        "hint-policy row (see this table's module docs): measured max delta [233,233,233,0], 713 \
         px (3.5650%) of this case's own shaped word — CJK strokes are dense enough that hinting \
         moves a larger share of a small frame than a Latin word does",
    ),
    (
        "text-combining",
        Tolerance {
            channel: 243,
            alpha: 2,
            diff_pixels: 0,
        },
        "hint-policy row (see this table's module docs): measured max delta [243,243,243,0], 243 \
         px (2.4300%) of this case's own stacked-diacritic glyph",
    ),
    (
        "text-colr-emoji",
        Tolerance {
            channel: 35,
            alpha: 2,
            diff_pixels: 0,
        },
        "colour-boundary row rather than a hint-policy one: a COLR glyph is hinted on neither \
         side, so what remains is the two rasterizers' own edge placement where the glyph's \
         layers meet at sharp colour boundaries (yellow face, black eyes/mouth) — measured max \
         delta [2,14,35,0], 0 px differing, mean [0.026,0.021,0.026,0.0], the blue channel \
         carrying effectively all of it. `alpha` stays at the corpus default, which the measured \
         0 sits inside. This is a narrow band and is meant to stay one: it read [189,212,255,0] \
         over 2043 px while a colour face still took the glyph-atlas route, where the tier could \
         not replay the COLR command stream `glifo` recorded and the glyph's own page went \
         unwritten. That route is now refused before insertion \
         (`frust_engine::text::atlas_policy`), so this case draws the glyph through the same \
         layer recombination `vello_cpu` does",
    ),
    (
        "text-gradient-brush",
        Tolerance {
            channel: 173,
            alpha: 2,
            diff_pixels: 0,
        },
        "hint-policy row (see this table's module docs), amplified by colour: the run's own \
         red-to-blue gradient means a hinted edge shifts across a colour transition rather than a \
         flat grey one, so the same sub-pixel shift that reads as a small grey delta on solid \
         text reads as a large cross-colour one here — measured max delta [105,173,158,0], 338 \
         px (1.5364%) of this case's own run",
    ),
    (
        "text-clipped",
        Tolerance {
            channel: 173,
            alpha: 2,
            diff_pixels: 0,
        },
        "hint-policy row (see this table's module docs): measured max delta [173,173,173,0], 395 \
         px (1.7955%) of this case's own run, entirely on ink inside the clip — the clip boundary \
         itself is a plain rounded rect and contributes no delta of its own",
    ),
];

/// Serializes every test in this binary that creates a GPU device.
///
/// Two ignored GPU tests running concurrently have been observed to hang at
/// process teardown roughly one run in five: each holds its own device and
/// pipeline-warm-up state, and tearing both down at once races inside the
/// driver. Serializing costs nothing here (the whole scoped corpus is a
/// handful of 64x64 frames) and keeps a teardown race from being read as a
/// rendering fault. Poison is ignored deliberately: one test's failure must
/// not cascade into every sibling.
static RENDER_LOCK: Mutex<()> = Mutex::new(());

fn render_lock() -> MutexGuard<'static, ()> {
    RENDER_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Renders `case` on `renderer` WITHOUT the straight-alpha conversion
/// [`render_case`](frust_testing::corpus::render_case) applies, or `None`
/// when the case skips this backend.
///
/// The engine-vs-CPU comparison is made on these bytes, in the premultiplied
/// space both arms natively produce, and that is load-bearing rather than a
/// shortcut. Un-premultiplying divides each colour channel by the pixel's own
/// alpha, so at the low coverage along a hairline's end cap or a dash boundary
/// (alpha 1-11 of 255) it multiplies a difference by up to 255: two frames one
/// 8-bit level apart in the space they were computed in come out as `[0, 255,
/// 0, 1]` against `[0, 0, 0, 1]` once straightened. Comparing before that
/// conversion measures the rasterizers; comparing after it measures the
/// conversion's own documented information loss
/// ([`straighten_alpha`]'s docs), which no tolerance can distinguish from a
/// real regression.
///
/// The golden path still goes through [`straighten_alpha`]: a stored PNG is
/// straight alpha, and both arms' baselines have to be stored the same way to
/// be comparable at all.
fn render_raw(renderer: &mut dyn SceneRenderer, case: &CorpusCase) -> Option<RenderedImage> {
    if case.spec.skip.contains(renderer.id()) {
        return None;
    }
    let image = renderer
        .render(&case.scene(), &case.render_spec())
        .unwrap_or_else(|err| {
            panic!(
                "case `{}` failed to render on `{}`: {err:#}",
                case.spec.name,
                renderer.id()
            )
        });
    Some(image)
}

/// Every corpus case, across all five corpora, in corpus order.
fn all_cases() -> Vec<CorpusCase> {
    unit_cases()
        .into_iter()
        .chain(adversarial_cases())
        .chain(widget_cases())
        .chain(page_cases())
        .chain(text_cases())
        .collect()
}

/// Every corpus case the engine gate compares, in corpus order.
fn scoped_cases() -> Vec<CorpusCase> {
    all_cases()
        .into_iter()
        .filter(|case| PHASE_CASES.contains(&case.spec.name))
        .collect()
}

/// Whether `scene` records a command the engine's compiler recognises and
/// skips — the mechanical membership rule [`PHASE_CASES`] is checked against.
///
/// `Command::GlyphRun` left this list in the text phase (p5): the compiler
/// now lowers every glyph run in full, atlas or outline route alike, so a
/// case that only draws glyph runs is no longer missing anything an engine
/// frame of it would need to pin. `Command::ShaderQuad` remains the one
/// command still recognised-and-skipped.
fn engine_skipped_commands(scene: &frust_scene::Scene) -> Vec<&'static str> {
    let mut kinds = Vec::new();
    for command in scene.commands() {
        let kind = match command {
            Command::ShaderQuad { .. } => "ShaderQuad",
            _ => continue,
        };
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    kinds
}

/// The tolerance `case` is compared against the CPU arm under: the case's own
/// [`CaseSpec::tolerance`] unless [`ESCALATIONS`] names it.
///
/// Taken off the case rather than hard-coded at the corpus default so a case
/// declared [`Tolerance::exact`] — `unit-clear-rect`'s tile-unaligned erase,
/// `adv-destout-in-layer`'s punch inside a layer group — is held pixel-exact
/// across the arms as well as against its baseline. Those two cases assert
/// erase arithmetic, where a one-level rounding difference is not a rounding
/// difference at all but a pixel the engine failed to clear.
fn engine_tolerance(spec: &CaseSpec) -> (Tolerance, Option<&'static str>) {
    ESCALATIONS
        .iter()
        .find(|(case, _, _)| *case == spec.name)
        .map_or((spec.tolerance, None), |(_, tolerance, why)| {
            (*tolerance, Some(*why))
        })
}

/// The repository commit this run's baselines would be attributed to — see
/// `tests/goldens.rs`'s identical helper for why this shells out rather than
/// baking the SHA in at build time.
fn frust_commit() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|sha| sha.trim().to_string())
        .filter(|sha| !sha.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// The current UTC time as an RFC 3339 timestamp — see `tests/goldens.rs`'s
/// identical helper for the civil-date conversion this hand-rolls rather than
/// pulling in a date crate.
fn rfc3339_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let days = (secs / 86_400) as i64;
    let time_of_day = secs % 86_400;
    let (hour, minute, second) = (
        time_of_day / 3600,
        (time_of_day % 3600) / 60,
        time_of_day % 60,
    );

    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = era * 400 + yoe + i64::from(month <= 2);

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// The `GoldenMeta` provenance record stored beside a promoted baseline or a
/// review artifact.
fn golden_meta(renderer: &dyn SceneRenderer, spec: &CaseSpec) -> GoldenMeta {
    GoldenMeta::new(
        &renderer.meta(),
        std::env::consts::OS,
        frust_commit(),
        spec.name,
        spec.tolerance,
        rfc3339_now(),
    )
}

/// Whether a baseline PNG already exists for `case` in `class`.
fn baseline_exists(class: &str, case: &CorpusCase) -> bool {
    goldens_root()
        .join(class)
        .join(format!("{}.png", case.spec.name))
        .is_file()
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

/// A one-line summary of a comparison, for both the failure message and the
/// recorded run log.
fn summarize(report: &DiffReport) -> String {
    format!(
        "{} px differ ({:.4}%), max |delta| {:?}, mean {:?}, bbox {:?}",
        report.pixel_count,
        report.mismatched_percent,
        report.max_difference,
        report.mean_abs_error.map(|v| (v * 1000.0).round() / 1000.0),
        report.bounding_box
    )
}

/// The first few differing pixels, expected against actual.
///
/// A cross-arm failure between two rasterizers sharing one geometry core is
/// almost always a handful of pixels with a specific shape (one end of a
/// hairline, one edge of a dash), and the counts alone do not say which — so
/// the failure message carries the actual bytes rather than only how many of
/// them there were.
fn first_pixels(report: &DiffReport) -> String {
    const SHOWN: usize = 6;
    let listed: Vec<String> = report
        .pixels
        .iter()
        .take(SHOWN)
        .map(|pixel| {
            format!(
                "({}, {}) cpu {:?} vs engine {:?}",
                pixel.x, pixel.y, pixel.expected, pixel.actual
            )
        })
        .collect();
    format!(
        "first {} of {}: {}",
        listed.len(),
        report.pixel_count,
        listed.join("; ")
    )
}

/// Renders every scoped case on the engine, compares it against the CPU arm at
/// the P1 bar and against the engine's own golden class, and reports EVERY
/// failure at once — the same one-run-reports-everything shape the other
/// golden gates use, so a rasterizer change surfaces every affected case in
/// one command.
///
/// ```text
/// WGPU_BACKEND=vulkan WGPU_ADAPTER_NAME=T400 FRUST_GOLDEN_EXPECT_ADAPTER=T400 \
///   cargo test -p frust-testing --test engine_goldens -- --ignored --nocapture
/// ```
#[test]
#[ignore = "requires a GPU; run locally with `cargo test -p frust-testing --test engine_goldens -- --ignored`"]
fn engine_corpus_matches_vello_cpu_and_its_goldens() {
    let _serialized = render_lock();
    let mut engine = EngineOracle::new(&EngineOracleOptions::default())
        .expect("failed to create the engine oracle");
    let mut cpu = CpuOracle::new();

    let class = engine.id();
    let classified = engine.is_classified();
    println!("engine goldens: engine arm on {}", engine.adapter_meta());
    println!("engine goldens: golden class `{class}`");
    println!("engine goldens: cpu arm `{}`", cpu.id());
    if !classified {
        println!(
            "engine goldens: adapter has no reviewed engine golden class — comparing against \
             the CPU arm and probing every case, promoting nothing"
        );
    }

    let update = update_goldens_enabled() && classified;
    if update_goldens_enabled() && !classified {
        println!(
            "engine goldens: UPDATE_GOLDENS=1 ignored — backend `{class}` has no reviewed \
             golden class"
        );
    }

    let mut failures: Vec<String> = Vec::new();
    let mut compared = 0_usize;
    let mut recorded = 0_usize;
    let mut promoted = 0_usize;

    for case in scoped_cases() {
        let name = case.spec.name;

        // Font determinism first, before this case's frame is even rendered —
        // the same ordering every other corpus gate uses. No scoped case
        // carries a glyph run today; the check is what keeps that true.
        let foreign = foreign_font_runs(&case.scene());
        if !foreign.is_empty() {
            for message in foreign {
                failures.push(format!("[{name}] font: {message}"));
            }
            continue;
        }

        let Some(rendered) = render_raw(&mut engine, &case) else {
            failures.push(format!(
                "[{name}] the case skips backend `{class}` — a scoped case must be one the \
                 engine renders"
            ));
            continue;
        };
        let Some(reference) = render_raw(&mut cpu, &case) else {
            failures.push(format!(
                "[{name}] the case skips the CPU arm — a scoped case needs a reference"
            ));
            continue;
        };
        if rendered.alpha != reference.alpha {
            failures.push(format!(
                "[{name}] the two arms report different alpha conventions ({:?} vs {:?}) — a \
                 cross-arm diff would be comparing two different spaces",
                reference.alpha, rendered.alpha
            ));
            continue;
        }
        // Straight alpha for everything downstream of the cross-arm diff: the
        // probes were stated against straight frames and a golden PNG is
        // straight (see `render_raw`).
        let image = straighten_alpha(&rendered);

        // Probes first, and a red probe stops promotion: a baseline is only
        // ever written from a frame that already satisfies the case's own
        // absolute assertions.
        let probe_failures = case.failed_probes(&image);
        if !probe_failures.is_empty() {
            for message in probe_failures {
                failures.push(format!("[{name}] probe: {message}"));
            }
            continue;
        }

        // The primary comparison: the two arms share one geometry core, so
        // this is the assertion that the GPU fill agrees with the CPU one —
        // made on the premultiplied frames both produced (see `render_raw`).
        let (tolerance, escalation) = engine_tolerance(&case.spec);
        let outcome = diff_images(
            &to_rgba_image(&reference),
            &to_rgba_image(&rendered),
            tolerance,
            case.eroded_interior,
        );
        println!(
            "engine goldens: [{name}] vs cpu under {tolerance:?}{} — {}",
            escalation
                .map(|why| format!(" (escalated: {why})"))
                .unwrap_or_default(),
            summarize(&outcome.report)
        );
        if !outcome.passed {
            failures.push(format!(
                "[{name}] engine and `{}` disagree beyond {tolerance:?}: {}; {}",
                cpu.id(),
                summarize(&outcome.report),
                first_pixels(&outcome.report)
            ));
            continue;
        }

        // A class nobody has promoted yet records instead of comparing: a
        // GPU baseline is adapter/driver-specific, promoted only by a
        // reviewed, deliberate act, so a missing one is never a failure.
        let mut spec = case.spec.clone();
        if !baseline_exists(class, &case) {
            spec.no_ref = true;
        }

        let meta = golden_meta(&engine, &spec);
        let golden = match compare_golden(class, &spec, &image, &meta, case.eroded_interior, update)
        {
            Ok(golden) => golden,
            Err(err) => {
                failures.push(format!("[{name}] {err:#}"));
                continue;
            }
        };

        if golden.updated {
            promoted += 1;
        } else if spec.no_ref {
            recorded += 1;
        } else {
            compared += 1;
        }

        if !golden.passed {
            let report = golden
                .report
                .as_ref()
                .map(summarize)
                .unwrap_or_else(|| "no report".to_string());
            let artifacts = golden
                .artifact_dir
                .as_ref()
                .map(|dir| dir.display().to_string())
                .unwrap_or_else(|| "<none>".to_string());
            failures.push(format!(
                "[{name}] golden mismatch under tolerance {:?}: {report}; artifacts in {artifacts}",
                spec.tolerance
            ));
        }
    }

    println!(
        "engine goldens: class `{class}` — {compared} compared, {recorded} recorded (no \
         baseline yet), {promoted} promoted, out of {} scoped case(s)",
        PHASE_CASES.len()
    );

    assert!(
        failures.is_empty(),
        "{} engine corpus failure(s) in golden class `{class}`:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Every corpus case is either in scope for the engine gate or deferred with a
/// reason — a GPU-free tripwire, so a corpus addition is caught on any machine
/// rather than only on the pinned runner.
#[test]
fn every_corpus_case_is_either_in_scope_or_deferred() {
    let mut unmapped = Vec::new();
    let mut names = Vec::new();
    for case in all_cases() {
        let name = case.spec.name;
        names.push(name);
        let scoped = PHASE_CASES.contains(&name);
        let deferred = DEFERRED_CASES.iter().any(|(case, _)| *case == name);
        if scoped == deferred {
            unmapped.push(format!(
                "`{name}` is {}",
                if scoped {
                    "both in scope AND deferred"
                } else {
                    "in neither PHASE_CASES nor DEFERRED_CASES"
                }
            ));
        }
    }

    assert!(
        unmapped.is_empty(),
        "{} corpus case(s) with no clear engine-phase decision — add each to PHASE_CASES (the \
         engine draws its subject) or to DEFERRED_CASES with the phase it waits for:\n{}",
        unmapped.len(),
        unmapped.join("\n")
    );

    for name in PHASE_CASES
        .iter()
        .chain(DEFERRED_CASES.iter().map(|(c, _)| c))
    {
        assert!(
            names.contains(name),
            "`{name}` is named by this file but no longer exists in the corpus"
        );
    }
}

/// Every deferred case states a reason, and no reason is a bare placeholder.
#[test]
fn every_deferred_case_states_why_it_waits() {
    for (name, why) in DEFERRED_CASES {
        assert!(
            why.len() > 16,
            "`{name}`'s deferral reason is too thin to review: {why:?}"
        );
    }
}

/// Every scoped case draws only commands the engine's compiler actually
/// compiles — the membership rule behind [`PHASE_CASES`], re-derived from the
/// scenes rather than trusted.
///
/// A case listed in scope while recording a [`Command::GlyphRun`] or a
/// [`Command::ShaderQuad`] would compare an engine frame that legitimately
/// omits its subject, and would read as an engine defect. GPU-free, so the
/// mistake is caught on any machine rather than only on the pinned runner.
#[test]
fn every_scoped_case_draws_only_commands_the_engine_compiles() {
    let mut wrongly_scoped = Vec::new();
    for case in scoped_cases() {
        let skipped = engine_skipped_commands(&case.scene());
        if !skipped.is_empty() {
            wrongly_scoped.push(format!(
                "`{}` records {} — the engine's compiler recognises and skips it, so an engine \
                 frame of this case is missing what the case exists to pin",
                case.spec.name,
                skipped.join(" and ")
            ));
        }
    }
    assert!(
        wrongly_scoped.is_empty(),
        "{} case(s) in PHASE_CASES draw a command the engine does not compile — move each to \
         DEFERRED_CASES until the phase that adds it:\n{}",
        wrongly_scoped.len(),
        wrongly_scoped.join("\n")
    );
}

/// Every deferred case is deferred for one of the two reasons the corpus can
/// confirm: it draws a command the engine skips, or it is `no_ref` and has no
/// stable reference on any backend.
///
/// The complement of the tripwire above, and the one that keeps deferral from
/// outliving its cause: once the engine compiles a command, every case built
/// on it stops matching either clause here and this test says so, instead of
/// the case quietly sitting out of the gate for another phase.
#[test]
fn every_deferred_case_is_deferred_for_a_reason_the_corpus_can_confirm() {
    let cases = all_cases();
    let mut stale = Vec::new();
    for (name, _) in DEFERRED_CASES {
        let Some(case) = cases.iter().find(|case| case.spec.name == *name) else {
            continue;
        };
        let skipped = engine_skipped_commands(&case.scene());
        if skipped.is_empty() && !case.spec.no_ref {
            stale.push(format!(
                "`{name}` draws only commands the engine compiles and is not `no_ref`"
            ));
        }
    }
    assert!(
        stale.is_empty(),
        "{} deferred case(s) whose deferral no longer has a cause — move each into \
         PHASE_CASES:\n{}",
        stale.len(),
        stale.join("\n")
    );
}

/// Every escalation names a case that exists and is actually looser than the
/// tolerance it replaces — a row that matched nothing, or that tightened
/// rather than widened, would be a silent no-op.
#[test]
fn every_escalation_is_a_real_widening_of_a_scoped_case() {
    let cases = all_cases();
    for (name, tolerance, why) in ESCALATIONS {
        assert!(
            PHASE_CASES.contains(name),
            "escalation for `{name}` names a case the gate does not compare"
        );
        let base = cases
            .iter()
            .find(|case| case.spec.name == *name)
            .map_or(Tolerance::new(), |case| case.spec.tolerance);
        assert!(
            tolerance.channel > base.channel
                || tolerance.alpha > base.alpha
                || tolerance.diff_pixels > base.diff_pixels,
            "escalation for `{name}` is not looser than the case's own {base:?}"
        );
        assert!(
            why.len() > 16,
            "`{name}`'s escalation reason is too thin to review: {why:?}"
        );
    }
}

/// The two cases whose subject is erase arithmetic are compared PIXEL-EXACT
/// against the CPU arm, not at the corpus's rounding-step default.
///
/// `unit-clear-rect` punches its hole at a deliberately tile-unaligned edge
/// and `adv-destout-in-layer` punches one inside a layer group; in both, a
/// pixel that is one level off is a pixel the engine failed to clear, not a
/// rounding difference. Both carry [`Tolerance::exact`] on their own
/// [`CaseSpec`], and [`engine_tolerance`] is what carries it into the
/// cross-arm comparison — this states that, so a change to either end is
/// caught here rather than silently loosening the erase contract.
#[test]
fn the_erase_cases_are_compared_pixel_exact_against_the_cpu_arm() {
    let cases = all_cases();
    for name in ["unit-clear-rect", "adv-destout-in-layer"] {
        let case = cases
            .iter()
            .find(|case| case.spec.name == name)
            .unwrap_or_else(|| panic!("`{name}` is no longer in the corpus"));
        assert!(
            PHASE_CASES.contains(&name),
            "`{name}` must be in scope for the engine gate to hold its erase contract at all"
        );
        let (tolerance, escalation) = engine_tolerance(&case.spec);
        assert_eq!(
            tolerance,
            Tolerance::exact(),
            "`{name}` is compared against the CPU arm at {tolerance:?}, not pixel-exact{}",
            escalation
                .map(|why| format!(" (escalated: {why})"))
                .unwrap_or_default()
        );
    }
}

/// The engine's unclassified fallback is never a committed golden directory.
///
/// A cheap, GPU-free guard on the safety property this gate's `classified`
/// flag depends on: even with `UPDATE_GOLDENS=1` set, an unreviewed adapter
/// must not create `testing/goldens/<something>/`.
#[test]
fn the_unclassified_engine_class_is_not_a_committed_golden_directory() {
    assert!(
        !goldens_root().join(ENGINE_UNCLASSIFIED_CLASS).exists(),
        "`{ENGINE_UNCLASSIFIED_CLASS}` is the refuse-to-promote fallback — it must never become \
         a committed golden class directory"
    );
}

/// The engine class this rig promotes into is its own `engine-`-prefixed
/// directory, never a bare adapter class shared with another rasterizer.
#[test]
fn the_pinned_runners_engine_class_is_its_own_directory() {
    let class = engine_golden_class("vulkan", "NVIDIA T400 4GB");
    assert_eq!(class, "engine-vulkan-nvidia-t400");
    assert!(
        class.starts_with("engine-"),
        "an engine class is prefixed so a baseline of another pipeline can \
         never be read as one of the engine's"
    );
}

/// No scoped case shapes a glyph against a host font.
///
/// The gate above already refuses such a case as part of its own font check,
/// but it refuses it as one failure among many in a GPU run. This is the same
/// property stated on its own, on any machine — mirroring the identical test
/// in the other corpus gates.
#[test]
fn no_scoped_case_shapes_against_a_host_font() {
    let mut failures = Vec::new();
    for case in scoped_cases() {
        for message in foreign_font_runs(&case.scene()) {
            failures.push(format!("[{}] {message}", case.spec.name));
        }
    }
    assert!(
        failures.is_empty(),
        "{} non-portable frame(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

// -----------------------------------------------------------------------------
// The filter golden family (p6-03b)
// -----------------------------------------------------------------------------
//
// `frust_testing::corpus::filters::FilterCase` is deliberately not a
// `CorpusCase` (`frust_scene` carries no filter command — see that module's
// own doc), so it cannot go through [`scoped_cases`]/[`render_raw`]/
// [`EngineOracle`] at all: the engine arm below drives `push_filter_layer`/
// `Schedule`/`FilterResources` directly instead, exactly the way
// `frust-engine`'s own `tests/filters.rs::FilterHarness` does — this is a
// second copy of that shape rather than a shared one because it lives in a
// different crate, over the family's own case list, geometry and tolerances.
//
// # The shared geometry
//
// Every case's engine and CPU arms draw [`FilterGeomCONTENT`]'s footprint (or
// its rounded form) at the SAME absolute coordinates on a
// [`filter_geom::CANVAS`]-square field. `push_filter_layer`/`pop_layer` (the
// engine's own recorder) hand back a tile-aligned `bounds` — the layer's own
// device-space rectangle, already grown by the filter's spread — and both
// arms are cropped to exactly that window before they are compared: the
// engine reads back its page at `bounds`' extent from the page's own origin
// (`bounds` is the RECORDED value, so it needs no further transform), and the
// CPU arm crops its full canvas pixmap at `bounds`' own `(x0, y0)` offset.
// Nothing here re-derives or hand-tunes that offset — one `bounds` value,
// computed once, drives both crops.
//
// # Why the two pooled pages never need to be resized per case
//
// `frust_testing::corpus::filters::CONTENT`'s placement keeps every case's
// spread (σ 32's 3σ reach, or the drop shadow's blur spread plus its own
// offset) under `DEFAULT_MIN_PAGE_SIZE` — the scheduler's own page floor — so
// `Schedule::build` floors every case's page at exactly the same 512 square.
// [`FilterEngineHarness`] creates its two ping-pong pages at that fixed
// extent once and reuses them for the whole family, and
// `render_engine_filter_case` asserts the floor held rather than assuming it,
// so a future case that grows past it fails loudly here instead of silently
// under-allocating a texture.
//
// # Tolerance
//
// P1's own bar (this file's module doc) — the engine and `vello_cpu` share
// `vello_common`'s kernels, so the comparison is near-exact, at each case's
// own [`FilterCase::tolerance`]. `filter-blur-clipped` is the one case
// widened past the corpus default: its two arms rasterize rounded corners
// through different antialiasing (a supersampled coverage mask fed to the
// engine's synthetic source page vs `vello_cpu`'s own analytic rasterizer),
// a difference the blur that follows only softens rather than erases — see
// that case's own row in `frust_testing::corpus::filters::filter_cases` for
// the reason and `FILTER_TOLERANCE_NOTE` below for the measured number.
//
// # Golden classes
//
// `cpu` is committable after visual inspection of the promoted PNGs (a plain
// CPU render, no GPU involved). The engine class this rig routes to promotes
// under the SAME convention as the corpus gate above — `UPDATE_GOLDENS=1`,
// and only on an adapter whose engine class is reviewed (`classified`) —
// since p6-d1, the Mac verification round that took the reviewed promotion
// step for `engine-metal-macos`'s filter baselines. p6-03b itself (the task
// that authored this family on the T400) never promoted: its write scope
// excluded `testing/goldens/engine-vulkan-nvidia-t400`, whose `filter-*`
// baselines therefore remain unpromoted, recording artifacts, until a
// reviewed T400 round deliberately takes the same step.

use frust_engine::EngineDraw;
use frust_engine::error::EngineError as FilterFamilyEngineError;
use frust_engine::filters::blur::{FilterInstanceData, GpuFilterData, GpuGaussianBlur};
use frust_engine::filters::drop_shadow::GpuDropShadow;
use frust_engine::filters::{
    LayerFilter, MAX_BLUR_SIGMA, push_filter_layer, served_blur, served_drop_shadow,
};
use frust_engine::gpu::pipelines::{EnginePipeline, EngineShaders, INTERMEDIATE_FORMAT};
use frust_engine::gpu::targets::INTERMEDIATE_USAGE;
use frust_engine::renderer::{FilterPassPlan, FilterResources};
use frust_engine::schedule::{PageConfig, PageParity, Round, Schedule};
use frust_gpu::{PipelineCache, ShaderLibrary, TierCaps};
use kurbo::{Affine, RoundedRect, Shape};
use peniko::Fill;
use vello_common::geometry::{RectU16, SizeU16};
use vello_common::paint::Paint;
use vello_common::peniko::BlendMode;
use vello_common::record::{CommandRecorder, LayerProps, RecordedLayerKind};
use vello_common::strip::Strip;
use vello_common::tile::Tile;
use vello_cpu_oracle::{Level, Pixmap, RenderContext, RenderSettings, Resources};

use frust_testing::render::AlphaKind;

use frust_testing::corpus::filters::{
    CANVAS, CLIP_RADIUS, CONTENT, CONTENT_COLOR, ContentShape, FilterCase, FilterKind, filter_cases,
};

/// Fixed extent every filter-family case's two ping-pong pages are created
/// at — see this section's own module doc for why every case floors here.
const FILTER_PAGE_EXTENT: u32 = frust_engine::schedule::pages::DEFAULT_MIN_PAGE_SIZE;

/// `filter-blur-clipped`'s own widened tolerance, and why: measured on the
/// pinned T400 rig, the rounded-corner antialiasing difference between the
/// engine's synthetic (supersampled) source page and `vello_cpu`'s own
/// analytic rasterizer, softened by the σ 8 blur both arms then apply.
const FILTER_TOLERANCE_NOTE: &str = "engine-vs-cpu antialiasing at CLIP_RADIUS's rounded \
    corners, softened by the case's own blur; see filter_cases()'s own row";

/// One command stream's recorded filter layer, its bounds, and the strips
/// that gave it that bbox — the single source of truth both arms crop to.
fn case_recorder(case: &FilterCase) -> CommandRecorder<EngineDraw> {
    let mut recorder = CommandRecorder::<EngineDraw>::new(CANVAS, CANVAS);
    let layer_filter = match case.kind {
        FilterKind::Blur { sigma } => LayerFilter::Blur { sigma },
        FilterKind::DropShadow { sigma } => LayerFilter::DropShadow {
            offset: frust_testing::corpus::filters::SHADOW_OFFSET,
            sigma,
            color: frust_testing::corpus::filters::SHADOW_COLOR,
        },
    };
    push_filter_layer(
        &mut recorder,
        filter_layer_props(),
        layer_filter,
        Affine::IDENTITY,
    )
    .unwrap_or_else(|err| {
        panic!(
            "case `{}` failed to record its filter layer: {err:?}",
            case.name
        )
    });
    let strips = strip_rows(CONTENT.0, CONTENT.1, CONTENT.2, CONTENT.3);
    recorder.push_draw(
        EngineDraw::new(Paint::from(CONTENT_COLOR), 0, 0..strips.len()),
        &strips,
    );
    recorder.pop_layer();
    recorder
}

/// A layer composited source-over at full opacity, with no mask and no layer
/// clip path — the shape every case in this family opens its filter layer
/// with.
fn filter_layer_props() -> LayerProps {
    LayerProps {
        blend_mode: BlendMode::default(),
        opacity: 1.0,
        mask: None,
        clip_path: None,
    }
}

/// Strips covering `(x, y, width, height)`'s full footprint, one tile row
/// (`Tile::HEIGHT` texels) at a time — [`CommandRecorder::push_draw`] folds
/// each row's coverage into the enclosing layer's bbox
/// (`vello_common::util::strip_bbox`), so this many-row shape is what makes
/// the recorded bbox cover the WHOLE rect rather than only its first row (the
/// single-tile-row shape `frust-engine`'s own `tests/filters.rs::strips`
/// helper uses, sufficient there because its own cases only assert on
/// WIDTH).
///
/// One REAL [`Strip`] per row plus exactly one trailing [`Strip::sentinel`] —
/// never a sentinel between rows.
/// [`vello_common::strip::visit_strip_fill_segments`] walks the list in
/// consecutive PAIRS and reads a row's width from the DELTA between two
/// adjacent entries' `alpha_idx` (`Strip::width_to`), so each row's
/// `alpha_idx` is `row_index * width * Tile::HEIGHT` — a running total, not
/// reset per row — and the trailing sentinel continues that same sequence one
/// step further, closing the last row's width the same way every other row's
/// own NEXT entry closes its own. A sentinel used as any PAIR's first element
/// (a sentinel between rows) fails that walk's own tile-alignment assertion
/// on the sentinel's `x` (`u16::MAX`), which is what this shape avoids.
fn strip_rows(x: u16, y: u16, width: u16, height: u16) -> Vec<Strip> {
    let rows = height.div_ceil(Tile::HEIGHT);
    let step = u32::from(width) * u32::from(Tile::HEIGHT);
    let mut strips = Vec::with_capacity(usize::from(rows) + 1);
    for row in 0..rows {
        let row_y = y + row * Tile::HEIGHT;
        let alpha_idx = u32::from(row) * step;
        strips.push(Strip::new(x, row_y, alpha_idx, false));
    }
    let last_y = y + rows.saturating_sub(1) * Tile::HEIGHT;
    strips.push(Strip::sentinel(last_y, u32::from(rows) * step));
    strips
}

/// One page texture, with the pool's own format and usage plus `COPY_DST`
/// (this harness seeds a page's contents by upload rather than by a real
/// strip pass — see [`FilterEngineHarness`]'s own doc).
struct FilterPage {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

fn filter_page(device: &wgpu::Device, label: &str) -> FilterPage {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: FILTER_PAGE_EXTENT,
            height: FILTER_PAGE_EXTENT,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: INTERMEDIATE_FORMAT,
        usage: INTERMEDIATE_USAGE | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    FilterPage { texture, view }
}

/// The engine arm's GPU harness: one device, one filter pipeline, one
/// [`FilterResources`], and the two pooled pages every case's filter
/// sequence ping-pongs between — mirroring `frust-engine`'s own
/// `tests/filters.rs::FilterHarness` (this section's own module doc explains
/// why a second copy lives here). A page's contents are seeded by a raw
/// texture upload rather than by a real strip-rasterization pass, exactly as
/// that harness's own `upload_source` does: this family's claim is about the
/// FILTER pipeline, not the strip one, and `frust_scene` cannot record a
/// filter layer to rasterize through the real one regardless (this module's
/// own doc, and `frust_engine::filters`' own).
struct FilterEngineHarness {
    device: wgpu::Device,
    queue: wgpu::Queue,
    caps: TierCaps,
    pipeline: wgpu::RenderPipeline,
    filters: FilterResources,
    pages: [FilterPage; 2],
    _cache: PipelineCache,
}

impl FilterEngineHarness {
    fn new(device: wgpu::Device, queue: wgpu::Queue, caps: TierCaps) -> Self {
        let mut library = ShaderLibrary::new();
        let shaders = EngineShaders::register(&mut library, &device);
        let mut cache = PipelineCache::new(std::sync::Arc::new(library), None);
        let desc = EnginePipeline::Filter.desc(&shaders, wgpu::TextureFormat::Bgra8Unorm);
        let pipeline = cache.get_or_create(&device, &desc).clone();
        let pages = [
            filter_page(&device, "frust-testing filter family page a"),
            filter_page(&device, "frust-testing filter family page b"),
        ];
        let filters = FilterResources::new(&device);
        Self {
            device,
            queue,
            caps,
            pipeline,
            filters,
            pages,
            _cache: cache,
        }
    }
}

/// A device/queue/caps triple resolved the same env-aware way every other
/// GPU harness in this workspace is (`WGPU_BACKEND`/`WGPU_ADAPTER_NAME`).
fn filter_gpu_device() -> (wgpu::Device, wgpu::Queue, TierCaps) {
    pollster::block_on(async {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = wgpu::util::initialize_adapter_from_env_or_default(&instance, None)
            .await
            .expect("no compatible GPU adapter");
        let caps = TierCaps::probe(&adapter);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("frust-testing filter family device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            })
            .await
            .expect("failed to create the device");
        (device, queue, caps)
    })
}

/// The `GpuFilterData` block `case`'s recorded filter (`kind`) erases to —
/// `served_blur`/`served_drop_shadow` read it back exactly as
/// `Schedule::build`/the real renderer's own `filter_block` do, so this
/// harness prepares the SAME parameters the scheduler already planned passes
/// against rather than re-deriving them.
fn filter_block(case: &FilterCase, kind: &RecordedLayerKind) -> GpuFilterData {
    match case.kind {
        FilterKind::Blur { .. } => {
            let blur = served_blur(0, kind)
                .unwrap_or_else(|reason| panic!("case `{}`: {reason}", case.name));
            GpuFilterData::from(GpuGaussianBlur::from(&blur))
        }
        FilterKind::DropShadow { .. } => {
            let shadow = served_drop_shadow(0, kind)
                .unwrap_or_else(|reason| panic!("case `{}`: {reason}", case.name));
            GpuFilterData::from(GpuDropShadow::from(&shadow))
        }
    }
}

/// A full [`FILTER_PAGE_EXTENT`]-square, premultiplied RGBA8 image: transparent
/// everywhere but [`CONTENT`]'s own footprint (rounded to [`CLIP_RADIUS`] for
/// [`ContentShape::Rounded`]), placed at `bounds`' own local offset — the
/// SAME placement `push_filter_layer`'s recorded `bounds` implies (see this
/// section's own module doc on the shared geometry). Writing the WHOLE page
/// extent every case, rather than only the written sub-rect, is deliberate:
/// this harness reuses its two page textures across every case in the
/// family, and a fresh GPU texture's initial content is unspecified, so
/// anything short of a full-extent write would let a session's very first
/// case sample undefined memory at the margin a filter's own taps can reach.
fn seed_image(shape: ContentShape, bounds: RectU16) -> Vec<u8> {
    let extent = usize::try_from(FILTER_PAGE_EXTENT).expect("512 fits usize");
    let mut out = vec![0_u8; extent * extent * 4];
    let color = CONTENT_COLOR.premultiply().to_rgba8().to_u8_array();
    let local_x = i64::from(CONTENT.0) - i64::from(bounds.x0);
    let local_y = i64::from(CONTENT.1) - i64::from(bounds.y0);

    for row in 0..i64::from(CONTENT.3) {
        for col in 0..i64::from(CONTENT.2) {
            let coverage = match shape {
                ContentShape::Rect => 1.0_f32,
                ContentShape::Rounded => {
                    rounded_rect_coverage(col, row, CONTENT.2, CONTENT.3, CLIP_RADIUS)
                }
            };
            if coverage <= 0.0 {
                continue;
            }
            let px = local_x + col;
            let py = local_y + row;
            if px < 0 || py < 0 || px as usize >= extent || py as usize >= extent {
                continue;
            }
            let at = (py as usize * extent + px as usize) * 4;
            for channel in 0..4 {
                out[at + channel] = (f32::from(color[channel]) * coverage)
                    .round()
                    .clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

/// Fractional coverage of texel `(x, y)` (top-left origin) by a `w`x`h`
/// rounded rect of corner radius `r`, 4x4-supersampled — close enough to
/// `vello_cpu`'s own analytic antialiasing that the σ 8 blur `filter-blur-
/// clipped` runs afterward brings the two arms back inside its own widened
/// tolerance (see this section's own module doc).
fn rounded_rect_coverage(x: i64, y: i64, w: u16, h: u16, r: f64) -> f32 {
    const SAMPLES: i64 = 4;
    let mut hits = 0_i64;
    for sy in 0..SAMPLES {
        for sx in 0..SAMPLES {
            let px = x as f64 + (sx as f64 + 0.5) / SAMPLES as f64;
            let py = y as f64 + (sy as f64 + 0.5) / SAMPLES as f64;
            if inside_rounded_rect(px, py, f64::from(w), f64::from(h), r) {
                hits += 1;
            }
        }
    }
    hits as f32 / (SAMPLES * SAMPLES) as f32
}

/// Whether `(x, y)` sits inside a `w`x`h` rounded rect (top-left origin) of
/// corner radius `r`: the ordinary clamp-to-nearest-corner-center distance
/// test, valid for `r <= w / 2` and `r <= h / 2` (both hold for every use
/// here — [`CLIP_RADIUS`] is a quarter of [`CONTENT`]'s own extent).
fn inside_rounded_rect(x: f64, y: f64, w: f64, h: f64, r: f64) -> bool {
    let cx = x.clamp(r, w - r);
    let cy = y.clamp(r, h - r);
    let dx = x - cx;
    let dy = y - cy;
    dx * dx + dy * dy <= r * r
}

/// Runs `case`'s engine arm to completion, returning the layer's own recorded
/// bounds and the premultiplied RGBA8 bytes of exactly that region, read back
/// from the page holding the filtered result.
fn render_engine_filter_case(
    harness: &mut FilterEngineHarness,
    case: &FilterCase,
) -> (RectU16, Vec<u8>) {
    let recorder = case_recorder(case);
    let config = PageConfig::default();
    let rounds = Schedule::build(&recorder, &harness.caps, &config)
        .unwrap_or_else(|err| panic!("case `{}` failed to schedule: {err:?}", case.name));

    let contents = rounds
        .first()
        .and_then(Round::page)
        .unwrap_or_else(|| panic!("case `{}` planned no contents round", case.name))
        .clone();
    assert_eq!(
        (contents.size.width, contents.size.height),
        (FILTER_PAGE_EXTENT, FILTER_PAGE_EXTENT),
        "case `{}`: this family's own CONTENT/CANVAS geometry was sized to floor every case's \
         page at DEFAULT_MIN_PAGE_SIZE — a case whose page no longer floors there needs \
         FILTER_PAGE_EXTENT (and this harness's fixed page textures) revisited",
        case.name
    );

    let image = seed_image(case.shape, contents.bounds);
    harness.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &harness.pages[contents.parity.index()].texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &image,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(FILTER_PAGE_EXTENT * 4),
            rows_per_image: Some(FILTER_PAGE_EXTENT),
        },
        wgpu::Extent3d {
            width: FILTER_PAGE_EXTENT,
            height: FILTER_PAGE_EXTENT,
            depth_or_array_layers: 1,
        },
    );

    let filter_rounds: Vec<&Round> = rounds[1..rounds.len() - 1].iter().collect();
    assert!(
        !filter_rounds.is_empty(),
        "case `{}` planned no filter pass",
        case.name
    );

    let block = filter_block(case, &recorder.layers[0].kind);
    let passes = u32::try_from(filter_rounds.len()).expect("a filter costs a handful of passes");
    harness.filters.prepare(
        &harness.device,
        &harness.queue,
        &harness.pipeline,
        &[block],
        passes,
    );

    let mut encoder = harness
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frust-testing filter family case"),
        });
    let mut result_parity = contents.parity;
    for (index, round) in filter_rounds.iter().enumerate() {
        let pass = round.filter_pass().expect("a filter round runs a pass");
        let page = round.page().expect("a filter pass writes a page");
        let instance = u32::try_from(index).expect("a filter costs a handful of passes");
        harness.filters.write_instance(
            &harness.queue,
            instance,
            &FilterInstanceData::new(
                &pass.step,
                0,
                (0, 0),
                (0, 0),
                SizeU16::from_wh(
                    u16::try_from(contents.size.width).unwrap_or(u16::MAX),
                    u16::try_from(contents.size.height).unwrap_or(u16::MAX),
                ),
                SizeU16::from(contents.bounds),
            ),
        );
        harness.filters.record_pass(
            &harness.device,
            &mut encoder,
            &FilterPassPlan {
                label: "frust-testing filter family pass",
                pipeline: &harness.pipeline,
                dest: &harness.pages[page.parity.index()].view,
                source: &harness.pages[pass.source.index()].view,
                instance,
            },
        );
        result_parity = page.parity;
    }
    harness.queue.submit([encoder.finish()]);

    let bytes = read_page_region(harness, result_parity, contents.bounds);
    (contents.bounds, bytes)
}

/// `bounds`' own extent of `parity`'s page, as premultiplied RGBA8 bytes.
fn read_page_region(harness: &FilterEngineHarness, parity: PageParity, bounds: RectU16) -> Vec<u8> {
    let width = u32::from(bounds.width());
    let height = u32::from(bounds.height());
    let bytes_per_row = (width * 4).next_multiple_of(256);
    let buffer = harness.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("frust-testing filter family readback"),
        size: u64::from(bytes_per_row) * u64::from(height),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let mut encoder = harness
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frust-testing filter family readback copy"),
        });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &harness.pages[parity.index()].texture,
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
    harness.queue.submit([encoder.finish()]);

    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    harness
        .device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("the readback poll must succeed");
    rx.recv()
        .expect("the readback channel must stay open")
        .expect("the readback buffer must map");

    let mapped = slice.get_mapped_range();
    let stride = bytes_per_row as usize;
    let row_len = width as usize * 4;
    let mut out = vec![0_u8; row_len * height as usize];
    for row in 0..height as usize {
        let src = row * stride;
        let dst = row * row_len;
        out[dst..dst + row_len].copy_from_slice(&mapped[src..src + row_len]);
    }
    drop(mapped);
    buffer.unmap();
    out
}

/// `case`'s CPU arm: a REAL `vello_cpu` render through `RenderContext::
/// push_layer`'s own `filter` parameter, cropped to `bounds` — the layer's
/// own recorded extent, shared with the engine arm (see this section's own
/// module doc).
fn render_cpu_filter_case(case: &FilterCase, bounds: RectU16) -> Vec<u8> {
    let settings = RenderSettings {
        level: Level::baseline(),
        num_threads: 0,
    };
    let mut ctx = RenderContext::new_with(CANVAS, CANVAS, settings);
    let mut resources = Resources::new();
    let mut pixmap = Pixmap::new(CANVAS, CANVAS);

    ctx.set_transform(Affine::IDENTITY);
    let filter = case.kind.filter();
    ctx.push_layer(None, None, None, None, Some(filter));
    ctx.set_transform(Affine::IDENTITY);
    ctx.set_fill_rule(Fill::NonZero);
    ctx.set_paint(CONTENT_COLOR);
    let rect = kurbo::Rect::new(
        f64::from(CONTENT.0),
        f64::from(CONTENT.1),
        f64::from(CONTENT.0 + CONTENT.2),
        f64::from(CONTENT.1 + CONTENT.3),
    );
    match case.shape {
        ContentShape::Rect => ctx.fill_rect(&rect),
        ContentShape::Rounded => {
            let path = RoundedRect::from_rect(rect, CLIP_RADIUS).to_path(0.1);
            ctx.fill_path(&path);
        }
    }
    ctx.pop_layer();

    ctx.flush();
    ctx.render(&mut pixmap, &mut resources);

    crop_to_bounds(pixmap.data_as_u8_slice(), usize::from(CANVAS), bounds)
}

/// `bounds`' own window of a `canvas`x`canvas` premultiplied RGBA8 image.
fn crop_to_bounds(full: &[u8], canvas: usize, bounds: RectU16) -> Vec<u8> {
    let width = usize::from(bounds.width());
    let height = usize::from(bounds.height());
    let x0 = usize::from(bounds.x0);
    let y0 = usize::from(bounds.y0);
    let mut out = vec![0_u8; width * height * 4];
    for row in 0..height {
        let src = ((y0 + row) * canvas + x0) * 4;
        let dst = row * width * 4;
        out[dst..dst + width * 4].copy_from_slice(&full[src..src + width * 4]);
    }
    out
}

/// The family's five rendered cases: engine vs a REAL `vello_cpu` render at
/// P1's own bar, `cpu`-class baselines committable after visual inspection,
/// the engine class this rig routes to recording-only (see this section's
/// own module doc).
///
/// ```text
/// WGPU_BACKEND=vulkan WGPU_ADAPTER_NAME=T400 FRUST_GOLDEN_EXPECT_ADAPTER=T400 \
///   cargo test -p frust-testing --test engine_goldens -- --ignored filter- --nocapture
/// ```
#[test]
#[ignore = "requires a GPU; run locally with `cargo test -p frust-testing --test engine_goldens -- --ignored filter-`"]
fn filter_family_matches_vello_cpu_and_its_goldens() {
    let _serialized = render_lock();

    // Resolves and validates the adapter identity and the golden class this
    // run routes into exactly as every other engine gate does — the filter
    // harness below opens its OWN device for the GPU work rather than
    // reaching into this oracle's internals (this section's own module doc).
    let class_probe = EngineOracle::new(&EngineOracleOptions::default())
        .expect("failed to create the engine oracle");
    let class = class_probe.id();
    let classified = class_probe.is_classified();
    println!(
        "filter family: engine arm on {}",
        class_probe.adapter_meta()
    );
    println!("filter family: golden class `{class}`");
    drop(class_probe);

    let (device, queue, caps) = filter_gpu_device();
    let mut harness = FilterEngineHarness::new(device, queue, caps);

    let update_cpu = update_goldens_enabled();
    // Since p6-d1 (the Mac verification round, which took the reviewed
    // promotion step this comment used to reserve), the engine class follows
    // the same convention as the corpus gate above: `UPDATE_GOLDENS=1`, and
    // only on an adapter whose engine class is reviewed. The T400's own
    // filter baselines remain unpromoted until a reviewed T400 round
    // deliberately sets `UPDATE_GOLDENS=1` and inspects what it writes.
    let update_engine = update_goldens_enabled() && classified;

    let mut failures: Vec<String> = Vec::new();
    let mut cpu_promoted = 0_usize;
    let mut engine_promoted = 0_usize;
    let mut engine_recorded = 0_usize;

    for case in filter_cases() {
        let name = case.name;
        let (bounds, engine_bytes) = render_engine_filter_case(&mut harness, &case);
        let cpu_bytes = render_cpu_filter_case(&case, bounds);
        let width = u32::from(bounds.width());
        let height = u32::from(bounds.height());

        let engine_image = RenderedImage {
            width,
            height,
            rgba8: engine_bytes,
            alpha: AlphaKind::Premultiplied,
            meta: class_probe_meta(class),
        };
        let cpu_image = RenderedImage {
            width,
            height,
            rgba8: cpu_bytes,
            alpha: AlphaKind::Premultiplied,
            meta: cpu_arm_meta(),
        };

        // P1: engine vs the REAL vello_cpu render, on the premultiplied
        // frames both arms natively produce — this file's own `render_raw`
        // does the identical thing for the rest of the corpus, and for the
        // identical reason (straightening first would measure that
        // conversion's own information loss, not the two rasterizers).
        let outcome = diff_images(
            &to_rgba_image(&cpu_image),
            &to_rgba_image(&engine_image),
            case.tolerance,
            false,
        );
        println!(
            "filter family: [{name}] vs cpu under {:?} — {}{}",
            case.tolerance,
            summarize(&outcome.report),
            if name == "filter-blur-clipped" {
                format!(" ({FILTER_TOLERANCE_NOTE})")
            } else {
                String::new()
            }
        );
        if !outcome.passed {
            failures.push(format!(
                "[{name}] engine and vello_cpu disagree beyond {:?}: {}; {}",
                case.tolerance,
                summarize(&outcome.report),
                first_pixels(&outcome.report)
            ));
            continue;
        }

        // The `cpu` class: a plain CPU-only baseline, committable from this
        // task after visual inspection.
        let cpu_straight = straighten_alpha(&cpu_image);
        let cpu_spec = CaseSpec::new(name)
            .with_size(width, height)
            .with_tolerance(case.tolerance);
        let cpu_meta = golden_meta_for(&cpu_image.meta, name, case.tolerance);
        match compare_golden(
            "cpu",
            &cpu_spec,
            &cpu_straight,
            &cpu_meta,
            false,
            update_cpu,
        ) {
            Ok(golden) => {
                if golden.updated {
                    cpu_promoted += 1;
                }
                if !golden.passed {
                    failures.push(format!(
                        "[{name}] cpu-class mismatch: {}",
                        golden
                            .report
                            .as_ref()
                            .map(summarize)
                            .unwrap_or_else(|| "no report".to_string())
                    ));
                }
            }
            Err(err) => failures.push(format!("[{name}] cpu class: {err:#}")),
        }

        // The engine class (see this section's own module doc): `spec.no_ref`
        // records a review artifact when the class has no baseline yet, and
        // `update_engine` promotes one only under `UPDATE_GOLDENS=1` on a
        // reviewed (classified) class.
        let mut engine_spec = CaseSpec::new(name)
            .with_size(width, height)
            .with_tolerance(case.tolerance);
        engine_spec.no_ref = !filter_baseline_exists(class, name);
        let engine_straight = straighten_alpha(&engine_image);
        let engine_meta = golden_meta_for(&engine_image.meta, name, case.tolerance);
        match compare_golden(
            class,
            &engine_spec,
            &engine_straight,
            &engine_meta,
            false,
            update_engine,
        ) {
            Ok(golden) => {
                if golden.updated {
                    engine_promoted += 1;
                } else if engine_spec.no_ref {
                    engine_recorded += 1;
                }
                if !golden.passed {
                    failures.push(format!(
                        "[{name}] engine-class mismatch: {}",
                        golden
                            .report
                            .as_ref()
                            .map(summarize)
                            .unwrap_or_else(|| "no report".to_string())
                    ));
                }
            }
            Err(err) => failures.push(format!("[{name}] engine class: {err:#}")),
        }
    }

    println!(
        "filter family: class `{class}` (classified={classified}) — {cpu_promoted} cpu \
         baseline(s) promoted, {engine_promoted} engine baseline(s) promoted, \
         {engine_recorded} engine artifact(s) recorded"
    );

    assert!(
        failures.is_empty(),
        "{} filter-family failure(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Whether a baseline PNG already exists for `name` in `class` — the filter
/// family's own counterpart of this file's `baseline_exists`, taking a plain
/// name rather than a [`CorpusCase`] since [`FilterCase`] is not one.
fn filter_baseline_exists(class: &str, name: &str) -> bool {
    goldens_root()
        .join(class)
        .join(format!("{name}.png"))
        .is_file()
}

/// A [`GoldenMeta`] record for `name`, sharing this file's own `frust_commit`/
/// `rfc3339_now` helpers.
fn golden_meta_for(
    meta: &frust_testing::render::BackendMeta,
    name: &str,
    tolerance: Tolerance,
) -> GoldenMeta {
    GoldenMeta::new(
        meta,
        std::env::consts::OS,
        frust_commit(),
        name,
        tolerance,
        rfc3339_now(),
    )
}

/// The [`BackendMeta`] the engine-class golden's provenance is stored under —
/// `class` doubles as the backend id here since the filter family's engine
/// arm is not an [`EngineOracle`] render (this section's own module doc).
fn class_probe_meta(class: &str) -> frust_testing::render::BackendMeta {
    frust_testing::render::BackendMeta {
        backend: class.to_string(),
        adapter: "frust-engine filter family harness".to_string(),
        driver: String::new(),
        device_kind: "gpu".to_string(),
    }
}

/// The [`BackendMeta`] the `cpu`-class golden's provenance is stored under.
fn cpu_arm_meta() -> frust_testing::render::BackendMeta {
    frust_testing::render::BackendMeta {
        backend: "cpu".to_string(),
        adapter: "vello_cpu 0.2.0 (vello_common 0.2.0, glifo 0.3.0)".to_string(),
        driver: "filter family harness".to_string(),
        device_kind: "cpu".to_string(),
    }
}

/// `filter-blur-oversized`, the family's one refusal member — GPU-free and
/// host-only, because its whole claim is that neither arm gets the chance to
/// render: a σ whose 3σ spread alone carries the padded page request past
/// even `TierCaps::fake`'s own adapter-free device ceiling (8192, this
/// crate's `pages.rs` unit tests pin the same number). It is not one of
/// [`filter_cases`]'s five rows because it renders nothing on either arm —
/// see that function's own doc.
#[test]
fn filter_blur_oversized_is_refused_rather_than_rendered() {
    let mut recorder = CommandRecorder::<EngineDraw>::new(CANVAS, CANVAS);
    push_filter_layer(
        &mut recorder,
        filter_layer_props(),
        LayerFilter::Blur {
            sigma: MAX_BLUR_SIGMA,
        },
        Affine::IDENTITY,
    )
    .expect("MAX_BLUR_SIGMA is finite and within push_filter_layer's own accepted range");
    let strips = strip_rows(CONTENT.0, CONTENT.1, CONTENT.2, CONTENT.3);
    recorder.push_draw(
        EngineDraw::new(Paint::from(CONTENT_COLOR), 0, 0..strips.len()),
        &strips,
    );
    recorder.pop_layer();

    let caps = TierCaps::fake(frust_gpu::DownlevelProfile::Full);
    assert!(
        matches!(
            Schedule::build(&recorder, &caps, &PageConfig::default()),
            Err(FilterFamilyEngineError::IntermediateTextureTooLarge)
        ),
        "filter-blur-oversized must be refused as IntermediateTextureTooLarge, not rendered or \
         panicked (E17)"
    );
}
