//! The unit corpus's golden gate: every [`frust_testing::corpus::unit`] case
//! rendered by the CPU arm and compared against its own golden class.
//!
//! ```text
//! # no GPU, no environment, part of the ordinary gate:
//! cargo test -p frust-testing --test goldens
//! ```
//!
//! # What this arm gates
//!
//! **`cpu/` is baseline-REQUIRED.** `vello_cpu` 0.2.0 rasterizes identically
//! on any host of the same target (pinned SIMD level, zero worker threads —
//! see `oracle_cpu`'s Determinism section), so a missing or mismatched
//! baseline is a real failure on any machine. These are the PNGs this corpus
//! commits, and they are the P1 reference the GPU arm is held against in
//! `tests/engine_goldens.rs` — the unit corpus's GPU coverage lives there,
//! over the engine's own `engine-`-prefixed golden classes, and this file is
//! the GPU-free half.
//!
//! What makes a golden a gate rather than a snapshot is [`Probe`]s: the
//! absolute pixel arithmetic the two promoted cases carry (see
//! `corpus::unit`) is asserted on every backend, baseline or no baseline.
//!
//! # Promotion
//!
//! `UPDATE_GOLDENS=1` is the only path that writes a baseline
//! (`frust_testing::golden`), and this harness narrows it once more: a case
//! whose probes FAIL is never promoted — a baseline is only ever written from
//! a frame that already satisfies its absolute assertions.

use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use frust_core::{
    FrameTime, InputEvent, PointerPhase, RenderRoot, SelectionToolbarBuilder,
    SelectionToolbarPolicy, SelectionToolbarRequest, install_selection_toolbar_builder_if_unset,
    set_selection_toolbar_policy,
};
use frust_scene::Scene;
use frust_testing::ORACLE_ID;
use frust_testing::case::{CaseSpec, Tolerance};
use frust_testing::corpus::{CorpusCase, render_case, unit_cases};
use frust_testing::frame::{
    FrameSpec, SAMPLE_TEXT_LONG, foreign_font_runs, frame, glyph_run_count, physical_case,
    pin_type_scale, pinned_text_style, pointer, test_text_context,
};
use frust_testing::golden::{compare_golden, goldens_root, update_goldens_enabled};
use frust_testing::meta::GoldenMeta;
use frust_testing::oracle_cpu::CpuOracle;
use frust_testing::render::SceneRenderer;
use frust_text::{TextContext, TextStyle};
use frust_theme::Theme;
use frust_widgets::{EdgeInsets, Padding, PaddingView, text_input};
use kurbo::{Point, Size};
use peniko::Color;

/// The golden class the CPU oracle's baselines live in
/// (`testing/goldens/cpu/`).
///
/// Not the oracle's own [`ORACLE_ID`] (`vello-cpu-0.2`): `docs/TESTING.md`
/// names this class `cpu/`, and a class directory outlives the exact
/// rasterizer version behind it. The two are tied together by
/// [`the_cpu_class_is_backed_by_the_pinned_oracle`], so a version bump has to
/// notice this mapping instead of silently re-pointing the directory.
const CPU_CLASS: &str = "cpu";

/// Whether a run is allowed to write baselines, and whether a missing one is
/// a failure.
///
/// Both knobs read `true` for the one arm this file runs; they are kept as
/// knobs because `run_corpus` is written against the general policy every
/// corpus gate in this crate shares — an adapter-specific class where a
/// missing baseline means "not reviewed on this runner" is the shape
/// `tests/engine_goldens.rs` still needs.
struct RunPolicy {
    /// The golden class directory the case's baseline is read from/written
    /// to.
    class: &'static str,
    /// When `true`, a case with no stored baseline FAILS. When `false`, it
    /// records a review artifact and passes.
    require_baseline: bool,
    /// When `true`, `UPDATE_GOLDENS=1` may promote from this run.
    promotable: bool,
}

/// The repository commit this run's baselines would be attributed to.
///
/// Shelled out to `git` rather than baked in at build time: a `env!`-style
/// capture would be frozen at the last recompilation, which is exactly when
/// it would be wrong (a rebuild-free re-run after a commit). Falls back to
/// `"unknown"` — a missing commit is worth recording as missing, not worth
/// failing a render over.
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

/// The current UTC time as an RFC 3339 timestamp (`GoldenMeta::timestamp`'s
/// contract).
///
/// Hand-rolled from [`SystemTime`] rather than pulling a date crate into the
/// workspace for one field: the civil-date conversion below is Howard
/// Hinnant's `civil_from_days`, shifted so the era starts on 0000-03-01, and
/// this repository's Version-Pin Policy makes a new third-party pin the more
/// expensive of the two options. Always UTC, so no local-timezone state
/// leaks into a promoted baseline's provenance.
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

    // Days since 1970-01-01 -> civil date. 719_468 shifts the epoch onto
    // 0000-03-01, where a 400-year era's leap pattern is uniform.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    // `mp` counts months from March; fold it back onto a January start.
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

/// Renders every unit case on `renderer`, checks its probes, compares (or
/// records) its baseline, and panics with EVERY failure at once.
///
/// Reporting the whole corpus rather than the first red case is deliberate:
/// a rasterizer change usually moves many cases, and a one-at-a-time gate
/// turns that into as many edit-run cycles as there are cases.
fn run_corpus(renderer: &mut dyn SceneRenderer, policy: &RunPolicy) {
    let update = update_goldens_enabled() && policy.promotable;
    if update_goldens_enabled() && !policy.promotable {
        println!(
            "goldens: UPDATE_GOLDENS=1 ignored — backend `{}` has no reviewed golden class",
            renderer.id()
        );
    }

    let mut failures: Vec<String> = Vec::new();
    let mut compared = 0_usize;
    let mut recorded = 0_usize;
    let mut promoted = 0_usize;
    let mut skipped = 0_usize;

    for case in unit_cases() {
        // Font determinism first, before this case's frame is even rendered:
        // a run shaped against a HOST font is not a portable baseline on any
        // oracle arm, so it is rejected before anything looks at its pixels —
        // the same ordering `frust_testing::run_cpu_goldens` uses for the
        // widget/page corpus (`frame::foreign_font_runs`'s own docs).
        let foreign = foreign_font_runs(&case.scene());
        if !foreign.is_empty() {
            for message in foreign {
                failures.push(format!("[{}] font: {message}", case.spec.name));
            }
            continue;
        }

        let Some(image) = render_case(renderer, &case)
            .unwrap_or_else(|err| panic!("case `{}` failed to render: {err:#}", case.spec.name))
        else {
            skipped += 1;
            println!(
                "goldens: `{}` skipped on `{}`",
                case.spec.name,
                renderer.id()
            );
            continue;
        };

        // Probes first, and a red probe stops promotion: a baseline is only
        // ever written from a frame that already satisfies the case's own
        // absolute assertions.
        let probe_failures = case.failed_probes(&image);
        if !probe_failures.is_empty() {
            for message in probe_failures {
                failures.push(format!("[{}] probe: {message}", case.spec.name));
            }
            continue;
        }

        // A class nobody has promoted yet records instead of comparing; the
        // `cpu/` class, being baseline-required, never does.
        let mut spec = case.spec.clone();
        if !policy.require_baseline && !baseline_exists(policy.class, &case) {
            spec.no_ref = true;
        }

        let meta = golden_meta(renderer, &spec);
        let outcome = match compare_golden(
            policy.class,
            &spec,
            &image,
            &meta,
            case.eroded_interior,
            update,
        ) {
            Ok(outcome) => outcome,
            // A missing baseline, an unreadable PNG, a premultiplied frame:
            // recorded like any other case failure rather than aborting the
            // run, so one command reports the whole corpus (see this
            // function's docs).
            Err(err) => {
                failures.push(format!("[{}] {err:#}", case.spec.name));
                continue;
            }
        };

        if outcome.updated {
            promoted += 1;
        } else if spec.no_ref {
            recorded += 1;
        } else {
            compared += 1;
        }

        if !outcome.passed {
            let report = outcome
                .report
                .as_ref()
                .map(|r| {
                    format!(
                        "{} px differ ({:.4}%), max |delta| {:?}, bbox {:?}",
                        r.pixel_count, r.mismatched_percent, r.max_difference, r.bounding_box
                    )
                })
                .unwrap_or_else(|| "no report".to_string());
            let artifacts = outcome
                .artifact_dir
                .as_ref()
                .map(|dir| dir.display().to_string())
                .unwrap_or_else(|| "<none>".to_string());
            failures.push(format!(
                "[{}] golden mismatch under tolerance {:?}: {report}; artifacts in {artifacts}",
                case.spec.name, case.spec.tolerance
            ));
        }
    }

    println!(
        "goldens: class `{}` — {compared} compared, {recorded} recorded (no baseline yet), \
         {promoted} promoted, {skipped} skipped",
        policy.class
    );

    assert!(
        failures.is_empty(),
        "{} corpus failure(s) in golden class `{}`:\n{}",
        failures.len(),
        policy.class,
        failures.join("\n")
    );
}

/// The gate that runs everywhere: the whole unit corpus against the
/// committed `cpu/` baselines, with no GPU involved.
#[test]
fn cpu_corpus_matches_its_goldens() {
    let mut oracle = CpuOracle::new();
    run_corpus(
        &mut oracle,
        &RunPolicy {
            class: CPU_CLASS,
            require_baseline: true,
            promotable: true,
        },
    );
}

/// Every case the CPU arm actually compares must have rendered with NO
/// fidelity gap.
///
/// `CpuOracle` reports its downgrades (a shader quad with no GPU, an image
/// brush handed to a fill, an unparseable font, an unrenderable image)
/// through `SkipReport` instead of failing. A promoted `cpu/` baseline of a
/// downgraded frame would be a baseline of the downgrade, so this asserts the
/// report is empty for every case that reaches the comparator —
/// `unit-shader-quad`, the one case with a genuine CPU gap, is excluded by
/// its own skip set and never renders here at all.
#[test]
fn the_cpu_oracle_reports_no_fidelity_gap_on_any_compared_case() {
    let mut oracle = CpuOracle::new();
    for case in unit_cases() {
        let rendered = render_case(&mut oracle, &case)
            .unwrap_or_else(|err| panic!("case `{}` failed to render: {err:#}", case.spec.name));
        if rendered.is_none() {
            continue;
        }
        let skips = oracle.skips();
        assert!(
            skips.is_empty(),
            "case `{}` rendered with a CPU fidelity gap ({skips:?}) — its baseline would \
             record the downgrade, not the command",
            case.spec.name
        );
    }
}

/// The `cpu/` class directory is the pinned oracle's, and stays that way.
///
/// A tripwire on the class-name mapping rather than an assertion about
/// pixels: if the oracle's rasterizer identity ever changes, this is the test
/// that asks whether `testing/goldens/cpu/` still means what its committed
/// PNGs meant.
#[test]
fn the_cpu_class_is_backed_by_the_pinned_oracle() {
    assert_eq!(
        CpuOracle::new().id(),
        ORACLE_ID,
        "the `cpu/` golden class is defined as `{ORACLE_ID}`'s output"
    );
}

/// No case in the unit corpus shapes a glyph against a host font.
///
/// `run_corpus` above already refuses such a case as part of its own font
/// check, but it refuses it as one failure among many in a golden run. This
/// is the same property stated on its own — mirroring `tests/page_goldens.rs`'s
/// `no_case_shapes_against_a_host_font` for the widget/page corpus — so a font
/// regression reads as a font regression rather than as a baseline mismatch.
#[test]
fn no_case_shapes_against_a_host_font() {
    let mut failures = Vec::new();
    for case in unit_cases() {
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

/// The provenance timestamp is the shape `GoldenMeta::timestamp` promises.
#[test]
fn rfc3339_now_has_the_shape_golden_meta_promises() {
    let stamp = rfc3339_now();
    assert_eq!(stamp.len(), 20, "{stamp}");
    assert!(stamp.ends_with('Z'), "{stamp}");
    assert_eq!(&stamp[4..5], "-", "{stamp}");
    assert_eq!(&stamp[7..8], "-", "{stamp}");
    assert_eq!(&stamp[10..11], "T", "{stamp}");
    let year: i64 = stamp[..4].parse().expect("year");
    assert!(year >= 2026, "{stamp} predates this test being written");
}

// -----------------------------------------------------------------------
// A long-press selection-toolbar golden through the real `RenderRoot` route
// -----------------------------------------------------------------------
//
// Everything above renders `unit_cases()` — scenes recorded by hand through a
// bare `SceneBuilder`, which cannot express a gesture. This one case drives a
// REAL `frust_widgets::text_input` through the same `rebuild`/`layout`/`paint`
// loop a shell runs, across several frames and a `Housekeeping` broadcast, and
// pins the result as its own dedicated `cpu/` baseline — deliberately outside
// `frust_testing::run_cpu_goldens`'s shared pipeline (see the font note below).
//
// `frust_testing::frame::frame` already interleaves events between two frames
// of the SAME root today (it takes `root: &mut RenderRoot` by reference, and
// `RenderRoot::event` is public and callable directly between calls, exactly
// as `frame::press_at`/`tap_at` already do) — so no new helper was needed in
// `src/frame.rs` for this case; it is driven with the existing `frame`/
// `pointer` exports alone.
//
// # Text: why "Hello, Hello", not the literal example text
//
// The bundled `NotoSans-Subset.ttf` face carries only the codepoints
// `testing/fonts/LICENSES.md`'s subset command lists: space, comma, `H`, `e`,
// `l`, `o`, `é`, U+0302 — i.e. exactly enough to shape "Hello" (capital H) and
// nothing else; `b`/`r`/`a`/`v`/`n`/`w`/`d`/`t` are not in the face at all. A
// field reading a longer sentence would shape those glyphs against a HOST
// font, which is exactly the non-portable frame `frame::foreign_font_runs`
// exists to reject. [`SAMPLE_TEXT_LONG`](frust_testing::frame::SAMPLE_TEXT_LONG)
// ("Hello, Hello") is this crate's own established stand-in for "a field with
// more than one word" (see `frust_testing::corpus::widget`'s `field_stack`,
// which already uses it for the identical reason) and plays the same
// structural role the card's own example does: a stationary long-press lands
// on the SECOND word, not the first, so the gesture is proven to select the
// word under the press point rather than always the first one.
//
// # The toolbar's own labels are a KNOWN, pre-existing font gap
//
// The baseline toolbar's four button labels ("Cut"/"Copy"/"Paste"/"Select
// all", `crates/frust-widgets/src/selection_toolbar.rs`'s `LABELS`) are drawn
// through `crate::text::text(label)`, which — like `frust_widgets::button`/
// `checkbox`/`radio`'s own labels — hardcodes `TextStyle::default()`
// (`FontFamily::SystemUi`) with no seam to override it. `docs/TESTING.md`'s
// Deterministic Inputs section and `frust_testing::corpus::widget`'s own
// module docs both name this residual gap and note the corpus's usual
// workaround (pass an empty label) does not apply here: the toolbar's labels
// are not this test's to set at all. This golden therefore captures whatever
// the host's `SystemUi` fallback draws for those four words — a pre-existing,
// out-of-scope (`frust-widgets`) limitation, not something this
// `frust-testing`-only task can close; it is also why this case is driven by
// hand rather than through `run_cpu_goldens`, whose shared pipeline would
// reject the frame outright on its own `foreign_font_runs` check before ever
// comparing a baseline.

/// Serialises every test in this file that writes the process-global
/// selection-toolbar policy/builder slots (`frust_core::selection_toolbar`).
/// `cargo test` runs one binary's tests on parallel threads sharing a single
/// process, so two of them installing a builder or flipping the policy would
/// see each other's writes — mirrors `frust_core::selection_toolbar`'s own
/// `TEST_LOCK` and `frust_widgets::textinput`'s `TOOLBAR_LOCK`, the identical
/// idiom for the identical reason; neither is reachable from this crate (both
/// are private to their own crate's test module), so this is its own lock
/// rather than a second name for one of theirs.
static TOOLBAR_LOCK: Mutex<()> = Mutex::new(());

/// The window every long-press toolbar frame lays out against, in LOGICAL px.
const TOOLBAR_WINDOW: Size = Size::new(320.0, 160.0);

/// How far the field is inset from the window's top, in LOGICAL px — the
/// room the toolbar needs to place itself ABOVE the field's own selection
/// (`crate::overlay::place`'s above-else-below rule) rather than falling back
/// below it. Also, combined with [`TOOLBAR_WINDOW`]'s height, exactly the
/// "sized 320x80" box the field lays out inside (a 320-wide, loose-height-80
/// deflated constraint — [`crates/frust-widgets/src/padding.rs`]'s `Padding`
/// deflates the incoming box by its insets).
const FIELD_TOP_INSET: f64 = 80.0;

/// The field's explicit horizontal/vertical inner padding
/// ([`frust_widgets::TextInputView::padding`]) — set explicitly here (rather
/// than left at the widget's own private defaults) so this test's own press-
/// point arithmetic is exact rather than dependent on an internal constant it
/// cannot name from outside the `frust-widgets` crate.
const FIELD_PAD_X: f64 = 12.0;
/// See [`FIELD_PAD_X`].
const FIELD_PAD_Y: f64 = 10.0;

/// The field's content text size, in logical px.
const TEXT_FONT_SIZE: f32 = 24.0;

/// Ink for the field's own (corpus-authored) text.
const TEXT_INK: Color = Color::from_rgb8(0x14, 0x14, 0x18);

/// A `FrameTime` `ms` milliseconds from the origin — mirrors
/// `frust_widgets::textinput`'s own private `ft_ms` test helper.
fn ft_ms(ms: f64) -> FrameTime {
    FrameTime::from_nanos((ms * 1_000_000.0) as u64)
}

/// Installs `frust_widgets::selection_toolbar` (the real baseline builder,
/// not a double) into the process-global builder slot if nothing is there
/// yet — the seam a design system's own bootstrap calls, and the one the card
/// asks this golden to install for itself since the facade bootstrap that
/// would otherwise do it never runs in a test binary. Call under
/// [`TOOLBAR_LOCK`].
fn install_baseline_toolbar_builder_if_unset() {
    let builder: SelectionToolbarBuilder =
        Arc::new(|request: &SelectionToolbarRequest, _window: Size| {
            frust_widgets::selection_toolbar(request)
        });
    install_selection_toolbar_builder_if_unset(builder);
}

/// Builds the one-field root [`record_text_input_selection_toolbar_long_press`]
/// drives, and the absolute (window-space) point a `Down` must land on to hit
/// the SECOND "Hello" — computed by shaping the identical prefix/word through
/// the SAME bundled font/style the field itself resolves at layout time,
/// rather than a hand-guessed pixel offset.
fn press_point_on_second_word(tcx: &mut TextContext, style: &TextStyle) -> Point {
    let prefix_width = tcx.layout("Hello, ", style, None).size().width;
    let word_width = tcx.layout("Hello", style, None).size().width;
    let content_height = tcx.layout(SAMPLE_TEXT_LONG, style, None).size().height;
    Point::new(
        FIELD_PAD_X + prefix_width + word_width / 2.0,
        FIELD_TOP_INSET + FIELD_PAD_Y + content_height / 2.0,
    )
}

/// Records the long-press sequence into `scene`, reading whatever
/// [`frust_core::selection_toolbar_policy`] is currently installed (set by the
/// caller, under [`TOOLBAR_LOCK`]) — a plain `fn(&mut Scene)`, so it doubles
/// as a [`CorpusCase::record`].
///
/// The sequence is exactly the card's own: `Down` on the second word, paint
/// at t=0 (seeds the long-press epoch), paint at t=550ms (past the widget's
/// 500ms threshold — crosses it and latches the deferred-callback flush),
/// dispatch [`InputEvent::Housekeeping`] (fires the hold: selects the word,
/// opens the toolbar), paint at t=600ms (mounts and paints the pod) — the
/// one frame this function actually records. A warm-up frame at t=0 runs
/// first, before the `Down`, because a pointer event can only hit geometry a
/// previous layout pass produced (`frust_testing::corpus::widget`'s own
/// `record_with_events` states the identical rule).
fn record_text_input_selection_toolbar_long_press(scene: &mut Scene) {
    let (mut tcx, family) = test_text_context();
    let style = pinned_text_style(&family, TEXT_FONT_SIZE, TEXT_INK);
    let press = press_point_on_second_word(&mut tcx, &style);

    let theme = pin_type_scale(Theme::neutral(), &family);
    let mut root: RenderRoot<(), PaddingView<()>> = RenderRoot::new();
    root.set_theme(Box::new(theme));
    let mut state = ();
    let field_style = style.clone();
    let mut logic = move |_: &mut ()| {
        Padding(
            EdgeInsets {
                left: 0.0,
                top: FIELD_TOP_INSET,
                right: 0.0,
                bottom: 0.0,
            },
            text_input::<(), _>(SAMPLE_TEXT_LONG.to_string(), |_: &mut (), _: String| {})
                .text_style(field_style.clone())
                .padding(FIELD_PAD_X, FIELD_PAD_Y),
        )
    };

    let spec_at = |ms: f64| FrameSpec {
        size: TOOLBAR_WINDOW,
        scale: 1.0,
        time: ft_ms(ms),
    };

    // Warm-up: lay out/paint once before the press lands on real geometry.
    let mut warmup = Scene::new();
    frame(
        &mut root,
        &mut logic,
        &mut state,
        &mut tcx,
        &spec_at(0.0),
        &mut warmup,
    );

    let _ = root.event(&mut state, &pointer(PointerPhase::Down, press));

    // This paint seeds the long-press epoch from its own frame time.
    let mut seeded = Scene::new();
    frame(
        &mut root,
        &mut logic,
        &mut state,
        &mut tcx,
        &spec_at(0.0),
        &mut seeded,
    );

    // Past the widget's 500ms threshold: marks the hold elapsed and latches
    // the flush the `Housekeeping` broadcast below rides out on.
    let mut crossing = Scene::new();
    frame(
        &mut root,
        &mut logic,
        &mut state,
        &mut tcx,
        &spec_at(550.0),
        &mut crossing,
    );

    let _ = root.event(&mut state, &InputEvent::Housekeeping);

    // The word is now selected and the toolbar wants to open; this final
    // rebuild mounts the pod and this paint registers/places it — the one
    // frame this function records.
    frame(
        &mut root,
        &mut logic,
        &mut state,
        &mut tcx,
        &spec_at(600.0),
        scene,
    );
}

/// The [`CorpusCase`] [`record_text_input_selection_toolbar_long_press`]
/// backs — named, sized and toleranced exactly as the card's deliverable
/// describes (320x160 at scale 1, the crate's tight default tolerance).
fn text_input_selection_toolbar_long_press_case() -> CorpusCase {
    CorpusCase {
        spec: physical_case(
            "text_input_selection_toolbar_long_press",
            &FrameSpec {
                size: TOOLBAR_WINDOW,
                scale: 1.0,
                time: FrameTime::ZERO,
            },
            Tolerance::new(),
        ),
        record: record_text_input_selection_toolbar_long_press,
        probes: &[],
        eroded_interior: false,
        about: "a stationary long-press on the second word of a focused field selects it and \
                floats the baseline selection toolbar above it, through the real RenderRoot \
                rebuild/layout/paint route and a Housekeeping broadcast",
    }
}

/// The golden itself: seeds under `UPDATE_GOLDENS=1`, otherwise compares
/// byte-for-byte (within the case's own tolerance) against
/// `testing/goldens/cpu/text_input_selection_toolbar_long_press.png`.
///
/// Driven by hand rather than through [`run_corpus`]/`run_cpu_goldens`'s
/// shared pipeline — see this section's own module docs for why the shared
/// pipeline's `foreign_font_runs` gate would reject this frame outright (the
/// toolbar's own labels are unavoidably drawn against a host font).
#[test]
fn text_input_selection_toolbar_long_press() {
    let _guard = TOOLBAR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    install_baseline_toolbar_builder_if_unset();
    set_selection_toolbar_policy(SelectionToolbarPolicy::Framework);

    let mut oracle = CpuOracle::new();
    let case = text_input_selection_toolbar_long_press_case();
    let image = render_case(&mut oracle, &case)
        .unwrap_or_else(|err| panic!("case `{}` failed to render: {err:#}", case.spec.name))
        .expect("this case renders on the CPU oracle (no backend skip set)");

    let probe_failures = case.failed_probes(&image);
    assert!(probe_failures.is_empty(), "{}", probe_failures.join("\n"));

    let update = update_goldens_enabled();
    let meta = golden_meta(&oracle, &case.spec);
    let outcome = compare_golden(
        CPU_CLASS,
        &case.spec,
        &image,
        &meta,
        case.eroded_interior,
        update,
    )
    .unwrap_or_else(|err| panic!("case `{}`: {err:#}", case.spec.name));

    if outcome.updated {
        println!("goldens: promoted `{}`", case.spec.name);
    }
    assert!(
        outcome.passed,
        "case `{}` mismatched its baseline under tolerance {:?}: {:?}; artifacts in {:?}",
        case.spec.name, case.spec.tolerance, outcome.report, outcome.artifact_dir
    );
}

/// A second, non-golden assertion: the identical gesture sequence under
/// [`SelectionToolbarPolicy::Native`] must paint no toolbar pod at all — the
/// platform draws its own menu instead. Checked by glyph-run count rather
/// than pixels: the framework route paints exactly 4 extra glyph runs (the
/// toolbar's Cut/Copy/Paste/Select all labels) that the native route must
/// never paint, whatever those 4 runs look like on this host (see this
/// section's module docs on why their font is not pinned).
///
/// Verified locally (three scratch edits to
/// `crates/frust-widgets/src/textinput.rs`, each reverted before this test
/// was committed — that file is out of this task's write scope) that this
/// assertion actually bites, and exactly how far its sensitivity reaches:
///
/// - Deleting ONLY `TextInputWidget::sync_toolbar`'s `selection_toolbar_policy()
///   == SelectionToolbarPolicy::Framework` conjunct (the build-side gate) did
///   **not** fail this test.
/// - Deleting ONLY `TextInputWidget::paint`'s matching conjunct around
///   `self.toolbar.paint(..)` (the paint-side gate) also did **not** fail it.
/// - Deleting BOTH at once **did** fail it (`framework=5, native=5`).
///
/// The field enforces `Native` at two independently-sufficient layers, and
/// this test observes only their PAINTED, black-box effect — from outside the
/// `frust-widgets` crate there is no seam to see "a pod was mounted" other
/// than its own painted pixels, and a pod paint's registration is gated by
/// the SECOND (paint-side) check regardless of what the first decided. So,
/// contrary to a plausible-sounding but unverified claim that the build-side
/// conjunct alone "is the gate that decides", the two checks are redundant
/// from this vantage: either one alone still fully suppresses the toolbar,
/// and only defeating the native route's protection at BOTH layers is
/// observable here. That is still a real, non-vacuous regression this test
/// catches — it is just not sensitive to a single-layer regression, which a
/// `frust-widgets`-internal test (with access to the private `toolbar`/
/// `toolbar_view` fields) would need to cover on its own side of the
/// boundary.
#[test]
fn text_input_selection_toolbar_long_press_native_policy_floats_no_pod() {
    let _guard = TOOLBAR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    install_baseline_toolbar_builder_if_unset();

    set_selection_toolbar_policy(SelectionToolbarPolicy::Framework);
    let mut framework_scene = Scene::new();
    record_text_input_selection_toolbar_long_press(&mut framework_scene);
    let framework_labels = glyph_run_count(&framework_scene);

    set_selection_toolbar_policy(SelectionToolbarPolicy::Native);
    let mut native_scene = Scene::new();
    record_text_input_selection_toolbar_long_press(&mut native_scene);
    let native_labels = glyph_run_count(&native_scene);

    // Leave the process-global policy as the rest of the suite expects to
    // find it, whatever happens above.
    set_selection_toolbar_policy(SelectionToolbarPolicy::Framework);

    assert!(
        framework_labels > native_labels,
        "the framework route must paint strictly more glyph runs than native \
         (framework={framework_labels}, native={native_labels})"
    );
    assert_eq!(
        native_labels + 4,
        framework_labels,
        "the framework route paints exactly 4 extra glyph runs — the toolbar's own \
         Cut/Copy/Paste/Select all labels — that the native route (the platform draws its \
         own menu) must never paint: framework={framework_labels}, native={native_labels}"
    );
}
