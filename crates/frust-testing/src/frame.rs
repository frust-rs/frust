//! GPU-free capture of a REAL widget tree as a [`frust_scene::Scene`].
//!
//! [`corpus::unit`](crate::corpus::unit) records its scenes by hand through a
//! [`frust_scene::SceneBuilder`] — one case per [`frust_scene::Command`]
//! variant, no widget involved. This module is the other half of the corpus's
//! input surface: it drives a real [`frust_core::RenderRoot`] — build,
//! rebuild, layout with text shaping, paint — and captures what the widget
//! tree itself emits, with no window, no shell, no GPU and no wall clock.
//!
//! # The frame, and why it is the shell's frame
//!
//! [`frame`] is `examples/huddle`'s headless one-frame helper
//! (`tests/support/mod.rs`) with its counting `RecScene` replaced by a real
//! `SceneBuilder` — `frust-core` already implements
//! [`PaintScene`](frust_core::PaintScene) for it, which is the same bridge the
//! desktop shell paints through. The sequence is the shell's, in the shell's
//! order:
//!
//! ```text
//! root.rebuild(logic, state);                    // app_logic -> view diff
//! root.layout_with_text(logical_size, tcx);      // LOGICAL px, text shaped
//! builder.push_transform(Affine::scale(scale));  // exactly what the desktop
//! root.paint(&mut builder, frame_time);          // shell pushes at paint time
//! builder.pop_transform();
//! ```
//!
//! Layout stays LOGICAL and the device scale composes at PAINT — that split is
//! the shell's, not an approximation of it
//! (`frust-shell-desktop`'s `app_handler.rs`: `builder.push_transform(
//! Affine::scale(scale))` around `self.root.paint(..)`). A case therefore
//! declares a logical viewport (412x892, a phone) and a scale (2.0), and its
//! [`CaseSpec`](crate::case::CaseSpec) declares the PHYSICAL pixel size that
//! pair implies — with [`CaseSpec::scale`](crate::case::CaseSpec::scale) left
//! at 1.0, because the scale is already baked into the recorded display list
//! and a second [`RenderSpec::scale`](crate::render::RenderSpec::scale) would
//! apply it twice ([`physical_case`] is the one place that mapping is made).
//!
//! # Determinism
//!
//! `docs/TESTING.md`'s Deterministic Inputs binds every input that can move a
//! pixel. The ones this module owns:
//!
//! - **Time.** [`FrameSpec::time`] is an explicit [`FrameTime`], never a
//!   clock. Animating widgets difference it against their own stored time, so
//!   a fixed timestamp is a fixed frame.
//! - **Fonts.** [`test_text_context`] registers ONLY the bundled
//!   [`crate::fonts`] faces, and [`pin_type_scale`] rewrites every slot of a
//!   theme's type scale onto the bundled family so a design-system widget
//!   resolving `type_scale.label_large` cannot reach a host font. Text a case
//!   authors itself must ALSO pin its family — `frust_widgets::text` takes its
//!   family from `TextStyle::default()` (i.e. `FontFamily::SystemUi`) and
//!   takes only its *color* from the theme.
//! - **Proof, not intent.** Pinning is a claim; [`foreign_font_runs`] is the
//!   check. It walks the captured scene and reports every
//!   [`frust_scene::Command::GlyphRun`] whose font bytes are not one of the
//!   bundled faces, so a leak fails the gate instead of being committed as a
//!   runner-local baseline. The bundled Latin face is a SUBSET (`H`, `e`,
//!   `l`, `o`, `,`, space, `é`, U+0302 — see `testing/fonts/LICENSES.md`), so
//!   corpus text is drawn from that alphabet: any other codepoint would send
//!   fontique to a system fallback and this check would catch it.

use std::any::Any;

use frust_core::{
    FrameTime, InputEvent, PaintScene, PointerButton, PointerEvent, PointerPhase, RenderRoot, View,
};
use frust_scene::{Command, Scene, SceneBuilder};
use frust_text::{FontFamily, TextContext, TextStyle};
use frust_theme::Theme;
use kurbo::{Affine, Point, Size};
use peniko::Color;

use crate::case::{CaseSpec, Tolerance};
use crate::fonts::{register_test_fonts, test_fonts};
use crate::render::SceneRenderer;

/// The logical viewport every widget and page case is laid out against: a
/// 412x892 phone, the same logical size `docs/TESTING.md`'s App-screens row
/// is written for.
pub const PHONE_LOGICAL: Size = Size::new(412.0, 892.0);

/// The device-pixel scale every widget and page case paints at — 2.0, an
/// ordinary phone density, and the factor [`frame`] pushes as the root
/// transform.
pub const PHONE_SCALE: f64 = 2.0;

/// The alphabet corpus text is drawn from: exactly the codepoints the bundled
/// Latin face carries (`testing/fonts/LICENSES.md`'s subset command). Text
/// outside it would fall back to a host font, which
/// [`foreign_font_runs`] refuses.
pub const SAMPLE_TEXT: &str = "Hello";

/// A longer label from the same alphabet, for a case that needs two words.
pub const SAMPLE_TEXT_LONG: &str = "Hello, Hello";

/// The deterministic inputs a captured frame is produced under: a LOGICAL
/// viewport, a device-pixel scale, and an explicit frame timestamp.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameSpec {
    /// Logical viewport the root is laid out against — the window size a
    /// shell would pass to [`RenderRoot::layout_with_text`].
    pub size: Size,
    /// Device-pixel scale pushed as the root paint transform (see the module
    /// docs).
    pub scale: f64,
    /// The frame's timestamp. Explicit, never a clock: an animating widget
    /// differences it against its own stored time.
    pub time: FrameTime,
}

impl FrameSpec {
    /// The phone frame every corpus case uses: [`PHONE_LOGICAL`] at
    /// [`PHONE_SCALE`], at [`FrameTime::ZERO`].
    #[must_use]
    pub const fn phone() -> Self {
        Self {
            size: PHONE_LOGICAL,
            scale: PHONE_SCALE,
            time: FrameTime::ZERO,
        }
    }

    /// The same frame at a different timestamp — how an animating case pins
    /// its "small set of meaningful fixed timestamps" (`docs/TESTING.md`).
    #[must_use]
    pub const fn at(mut self, time: FrameTime) -> Self {
        self.time = time;
        self
    }

    /// Physical output width in pixels: logical width times scale.
    #[must_use]
    pub fn physical_width(&self) -> u32 {
        (self.size.width * self.scale).round() as u32
    }

    /// Physical output height in pixels: logical height times scale.
    #[must_use]
    pub fn physical_height(&self) -> u32 {
        (self.size.height * self.scale).round() as u32
    }
}

/// A [`CaseSpec`] sized in PHYSICAL pixels for `spec`, with
/// [`CaseSpec::scale`] deliberately left at 1.0.
///
/// The device scale is already inside the recorded scene ([`frame`] pushes it
/// as the root transform, exactly as the shell does), so the renderer must
/// apply none of its own — see the module docs. `tolerance` is the case's
/// own, set where the case is declared, never widened by a retry path.
#[must_use]
pub fn physical_case(name: &'static str, spec: &FrameSpec, tolerance: Tolerance) -> CaseSpec {
    CaseSpec::new(name)
        .with_size(spec.physical_width(), spec.physical_height())
        .with_scale(1.0)
        .with_base_color(Color::WHITE)
        .with_tolerance(tolerance)
}

/// A [`TextContext`] carrying ONLY the bundled test faces, plus the Latin
/// family's resolved name as a [`FontFamily`].
///
/// The returned family is what every corpus text style must name (see the
/// module docs' Determinism section) — resolved from fontique's own answer
/// rather than hard-coded, so a re-subsetted face that reports a different
/// family name moves this in one place.
///
/// # Panics
///
/// Panics if the bundled faces register no family at all — a broken bundled
/// fixture, not a caller-recoverable error.
#[must_use]
pub fn test_text_context() -> (TextContext, FontFamily) {
    let mut cx = TextContext::new();
    let registered = register_test_fonts(&mut cx);
    let latin = registered
        .first()
        .expect("the bundled Latin face registers at least one family")
        .name
        .clone();
    (cx, FontFamily::named(latin))
}

/// A [`TextStyle`] at `size`/`color` pinned to `family` — the only shape
/// corpus-authored text is allowed to take.
#[must_use]
pub fn pinned_text_style(family: &FontFamily, size: f32, color: Color) -> TextStyle {
    TextStyle {
        family: family.clone(),
        ..TextStyle::new(size, color)
    }
}

/// Rewrites EVERY slot of `theme`'s type scale onto `family`.
///
/// A design-system widget resolves its label style from
/// `Theme::type_scale.<slot>` at layout time, so one un-pinned slot is one
/// runner-local glyph run. Every slot is listed rather than mapped, because
/// there is no iterator over the scale and a silently-missed slot is exactly
/// the failure this exists to prevent — [`foreign_font_runs`] is the backstop
/// that catches one anyway.
#[must_use]
pub fn pin_type_scale(mut theme: Theme, family: &FontFamily) -> Theme {
    macro_rules! pin {
        ($($slot:ident),+ $(,)?) => {
            $( theme.type_scale.$slot.family = family.clone(); )+
        };
    }
    pin!(
        display_large,
        display_medium,
        display_small,
        headline_large,
        headline_medium,
        headline_small,
        title_large,
        title_medium,
        title_small,
        body_large,
        body_medium,
        body_small,
        label_large,
        label_medium,
        label_small,
        display_large_emphasized,
        display_medium_emphasized,
        display_small_emphasized,
        headline_large_emphasized,
        headline_medium_emphasized,
        headline_small_emphasized,
        title_large_emphasized,
        title_medium_emphasized,
        title_small_emphasized,
        body_large_emphasized,
        body_medium_emphasized,
        body_small_emphasized,
        label_large_emphasized,
        label_medium_emphasized,
        label_small_emphasized,
    );
    theme
}

/// One frame of `root`, captured into `scene`.
///
/// The port of `examples/huddle`'s `tests/support/mod.rs::frame` onto a real
/// [`SceneBuilder`] — see the module docs for the sequence and why the scale
/// is pushed at paint rather than folded into layout. The paint pass's
/// [`PaintOutcome`](frust_core::PaintOutcome) is deliberately dropped: a
/// golden is one frame, and "this tree would like another frame" is a
/// scheduling signal, not a pixel.
pub fn frame<S: 'static, V: View<S>>(
    root: &mut RenderRoot<S, V>,
    logic: &mut impl FnMut(&mut S) -> V,
    state: &mut S,
    tcx: &mut TextContext,
    spec: &FrameSpec,
    scene: &mut Scene,
) {
    root.rebuild(logic, state);
    let tcx_any: &mut dyn Any = tcx;
    root.layout_with_text(spec.size, tcx_any);
    let mut builder = SceneBuilder::new(scene);
    builder.push_transform(Affine::scale(spec.scale));
    let _outcome = root.paint(&mut builder as &mut dyn PaintScene, spec.time);
    builder.pop_transform();
}

/// Lays out and paints a single `()`-state view into `scene` under `theme` —
/// the shape almost every corpus case takes.
///
/// `build` is called on every rebuild pass, exactly as an app's `app_logic`
/// is. `theme` is installed on the root
/// ([`RenderRoot::set_theme`](frust_core::RenderRoot::set_theme)), which is
/// the type-erased path widgets recover through `Theme::from_layout_ctx` /
/// `Theme::from_paint_ctx` — no reactive runtime and no ambient owner are
/// involved, so a case stays a pure function of its inputs.
pub fn record_view<V: View<()>>(
    scene: &mut Scene,
    spec: &FrameSpec,
    theme: Theme,
    tcx: &mut TextContext,
    build: impl Fn() -> V,
) {
    let mut root: RenderRoot<(), V> = RenderRoot::new();
    root.set_theme(Box::new(theme));
    let mut state = ();
    let mut logic = move |_: &mut ()| build();
    frame(&mut root, &mut logic, &mut state, tcx, spec, scene);
}

/// A synthetic pointer event at `p` (LOGICAL coordinates) — the shape every
/// platform boundary normalizes to before it reaches a
/// [`RenderRoot`](frust_core::RenderRoot).
#[must_use]
pub fn pointer(phase: PointerPhase, p: Point) -> InputEvent {
    InputEvent::Pointer(PointerEvent {
        phase,
        position: p,
        button: PointerButton::Primary,
    })
}

/// A pointer-DOWN at `p` with no matching up — how a case pins a *pressed*
/// visual state (`docs/TESTING.md`'s Inputs row: rest/pressed/disabled), as
/// opposed to a full tap, which fires the press and returns to rest.
pub fn press_at<S: 'static, V: View<S>>(root: &mut RenderRoot<S, V>, state: &mut S, p: Point) {
    let _ = root.event(state, &pointer(PointerPhase::Down, p));
}

/// A full tap (Down then Up) at `p` — fires an `on_press` only when both land
/// inside the target's bounds (`docs/CODE_STANDARDS.md`'s fire-on-up-inside
/// convention). Used by a case that needs a widget's *committed* state
/// (a focused field, a toggled control), not its pressed chrome.
pub fn tap_at<S: 'static, V: View<S>>(root: &mut RenderRoot<S, V>, state: &mut S, p: Point) {
    let _ = root.event(state, &pointer(PointerPhase::Down, p));
    let _ = root.event(state, &pointer(PointerPhase::Up, p));
}

/// Every glyph run in `scene` whose font is NOT one of the bundled test faces,
/// as review-ready messages.
///
/// The proof half of this module's font determinism (see the module docs): a
/// pinned family is a claim about what fontique *should* resolve, and this is
/// the check on what it actually did. A non-empty result means the frame
/// shaped against whatever font the host happens to have installed, so its
/// bytes are runner-local and must not be promoted as a portable baseline.
///
/// Compares the run's font BLOB against the bundled bytes rather than a family
/// name: a host font that happens to share a name would pass a name check and
/// still change every glyph.
#[must_use]
pub fn foreign_font_runs(scene: &Scene) -> Vec<String> {
    let bundled = test_fonts();
    let mut out = Vec::new();
    for (index, command) in scene.commands().iter().enumerate() {
        let Command::GlyphRun(run) = command else {
            continue;
        };
        let bytes: &[u8] = run.font.font().data.as_ref();
        if bundled.iter().any(|(_, face)| *face == bytes) {
            continue;
        }
        out.push(format!(
            "command #{index}: glyph run of {} glyph(s) at {}px shaped against a \
             {}-byte font that is not one of the {} bundled faces — the frame is \
             runner-local, not a portable baseline",
            run.glyphs.len(),
            run.font_size,
            bytes.len(),
            bundled.len()
        ));
    }
    out
}

/// How many glyph runs `scene` carries — a cheap "this case actually shaped
/// its text" assertion for a corpus case whose whole point is a labelled
/// widget.
#[must_use]
pub fn glyph_run_count(scene: &Scene) -> usize {
    scene
        .commands()
        .iter()
        .filter(|c| matches!(c, Command::GlyphRun(_)))
        .count()
}

/// A stable textual fingerprint of `scene`'s command list, for a test that
/// asks whether two frames are the same WITHOUT rendering them.
///
/// `Debug` on the raw command list is not that fingerprint. A
/// [`frust_scene::Command::GlyphRun`] carries a `peniko::FontData`, whose
/// `Blob` prints an interned, ALLOCATION-ORDER id — two captures of the same
/// case shape against the same bundled bytes and still print different ids,
/// because `TextContext::register_fonts` records each registration
/// process-wide and hands out a fresh blob each time. That id is not a pixel
/// input (the rasterizer sees the bytes), so it is normalized out here rather
/// than being allowed to make an identical frame look changed — or, worse, to
/// make a "these two frames must differ" assertion pass for the wrong reason.
#[must_use]
pub fn scene_fingerprint(scene: &Scene) -> String {
    const MARKER: &str = "Blob { id: ";
    let raw = format!("{:?}", scene.commands());
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw.as_str();
    while let Some(at) = rest.find(MARKER) {
        out.push_str(&rest[..at]);
        out.push_str("Blob { id: _");
        let tail = &rest[at + MARKER.len()..];
        // Every `Blob` Debug is `Blob { id: N, .. }`; resume at the comma so
        // the rest of the record is compared normally. A shape without one is
        // left as-is rather than silently swallowing the remainder.
        match tail.find(',') {
            Some(end) => rest = &tail[end..],
            None => {
                rest = tail;
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The golden class the widget/page baselines live in
/// (`testing/goldens/cpu/`) — `docs/TESTING.md`'s Golden Classes names it,
/// and `crate::oracle_cpu::CpuOracle` is what defines its contents.
pub const CPU_CLASS: &str = "cpu";

/// What [`run_cpu_goldens`] did, and everything that went wrong.
#[derive(Debug, Default)]
pub struct GateReport {
    /// Cases compared against a committed baseline.
    pub compared: usize,
    /// Cases whose baseline this run wrote (`UPDATE_GOLDENS=1` only).
    pub promoted: usize,
    /// Every failure, as a review-ready message. Empty means the gate passed.
    pub failures: Vec<String>,
}

impl GateReport {
    /// Panics with EVERY failure at once when the gate did not pass.
    ///
    /// Reporting the whole corpus rather than the first red case is the same
    /// choice `tests/goldens.rs` makes for the unit corpus: one widget-layer
    /// change usually moves many cases, and a one-at-a-time gate turns that
    /// into as many edit-run cycles as there are cases.
    ///
    /// # Panics
    ///
    /// Panics if [`GateReport::failures`] is non-empty.
    pub fn assert_passed(&self, label: &str) {
        assert!(
            self.failures.is_empty(),
            "{} failure(s) in {label}:\n{}",
            self.failures.len(),
            self.failures.join("\n")
        );
    }
}

/// The repository commit a promoted baseline is attributed to, or
/// `"unknown"`.
///
/// Shelled out to `git` rather than baked in at build time: an `env!`-style
/// capture would freeze at the last recompilation, which is exactly when it
/// would be wrong (a rebuild-free re-run after a commit).
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

/// The current UTC time as the RFC 3339 timestamp
/// [`GoldenMeta::timestamp`](crate::meta::GoldenMeta) promises.
///
/// Hand-rolled from [`std::time::SystemTime`] rather than pulling a date
/// crate in for one field (this repository's Version-Pin Policy makes a new
/// third-party pin the more expensive option); the civil-date conversion is
/// Howard Hinnant's `civil_from_days`, shifted so the era starts on
/// 0000-03-01. Always UTC, so no local-timezone state leaks into a promoted
/// baseline's provenance.
fn rfc3339_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (days, rest) = ((secs / 86_400) as i64, secs % 86_400);
    let (hour, minute, second) = (rest / 3600, (rest % 3600) / 60, rest % 60);
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

/// Runs `cases` through the CPU oracle against the committed `cpu/` class:
/// font determinism, fidelity gaps, probes, then the baseline comparison.
///
/// The one gate both widget/page harnesses call — `crates/frust-testing`'s own
/// `tests/page_goldens.rs` and, across a path dev-dependency,
/// `examples/material3-demo`'s. That EXCLUDED workspace cannot be imported
/// from the root graph, so the only way its pages share this pipeline is for
/// the pipeline to live in the library rather than in a test file.
///
/// The order of the checks is the contract:
///
/// 1. **Font determinism** ([`foreign_font_runs`]) — a frame shaped against a
///    host font is not a portable baseline, so it is rejected BEFORE anything
///    looks at its pixels.
/// 2. **Fidelity** ([`crate::oracle_cpu::SkipReport`]) — a downgraded frame
///    would promote a baseline of the downgrade.
/// 3. **Probes** — a case's absolute expectations, which hold with or without
///    a baseline.
/// 4. **Baseline** — compare, or write under `UPDATE_GOLDENS=1`.
///
/// A red case at any step is never promoted: the failure is recorded and the
/// case is abandoned before the comparison it would have written.
#[must_use]
pub fn run_cpu_goldens(cases: &[crate::corpus::CorpusCase]) -> GateReport {
    let update = crate::golden::update_goldens_enabled();
    let mut oracle = crate::oracle_cpu::CpuOracle::new();
    let commit = frust_commit();
    let stamp = rfc3339_now();
    let mut report = GateReport::default();

    for case in cases {
        let name = case.spec.name;

        let foreign = foreign_font_runs(&case.scene());
        if !foreign.is_empty() {
            for message in foreign {
                report.failures.push(format!("[{name}] font: {message}"));
            }
            continue;
        }

        let rendered = match crate::corpus::render_case(&mut oracle, case) {
            Ok(Some(image)) => image,
            Ok(None) => continue,
            Err(err) => {
                report.failures.push(format!("[{name}] render: {err:#}"));
                continue;
            }
        };

        let skips = oracle.skips();
        if !skips.is_empty() {
            report.failures.push(format!(
                "[{name}] the CPU oracle downgraded this frame ({skips:?}) — its baseline \
                 would record the downgrade, not the widget"
            ));
            continue;
        }

        let probe_failures = case.failed_probes(&rendered);
        if !probe_failures.is_empty() {
            for message in probe_failures {
                report.failures.push(format!("[{name}] probe: {message}"));
            }
            continue;
        }

        let meta = crate::meta::GoldenMeta::new(
            &oracle.meta(),
            std::env::consts::OS,
            commit.clone(),
            name,
            case.spec.tolerance,
            stamp.clone(),
        );
        match crate::golden::compare_golden(
            CPU_CLASS,
            &case.spec,
            &rendered,
            &meta,
            case.eroded_interior,
            update,
        ) {
            Ok(outcome) => {
                if outcome.updated {
                    report.promoted += 1;
                } else {
                    report.compared += 1;
                }
                if !outcome.passed {
                    let detail = outcome.report.as_ref().map_or_else(
                        || "no report".to_string(),
                        |r| {
                            format!(
                                "{} px differ ({:.4}%), max |delta| {:?}, bbox {:?}",
                                r.pixel_count,
                                r.mismatched_percent,
                                r.max_difference,
                                r.bounding_box
                            )
                        },
                    );
                    let artifacts = outcome
                        .artifact_dir
                        .as_ref()
                        .map_or_else(|| "<none>".to_string(), |dir| dir.display().to_string());
                    report.failures.push(format!(
                        "[{name}] golden mismatch under tolerance {:?}: {detail}; \
                         artifacts in {artifacts}",
                        case.spec.tolerance
                    ));
                }
            }
            Err(err) => report.failures.push(format!("[{name}] {err:#}")),
        }
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_phone_frame_is_412x892_logical_at_scale_2() {
        let spec = FrameSpec::phone();
        assert_eq!(spec.size, Size::new(412.0, 892.0));
        assert_eq!(spec.scale, 2.0);
        assert_eq!(spec.time, FrameTime::ZERO);
        assert_eq!(spec.physical_width(), 824);
        assert_eq!(spec.physical_height(), 1784);
    }

    #[test]
    fn a_physical_case_carries_the_physical_size_and_no_second_scale() {
        let case = physical_case("widget-sample", &FrameSpec::phone(), Tolerance::new());
        assert_eq!(case.width, 824);
        assert_eq!(case.height, 1784);
        assert_eq!(
            case.scale, 1.0,
            "the device scale is already inside the recorded scene"
        );
    }

    #[test]
    fn the_bundled_family_resolves_to_a_named_family() {
        let (_cx, family) = test_text_context();
        assert!(
            matches!(&family, FontFamily::Named(names) if !names.is_empty()),
            "expected a named family, got {family:?}"
        );
    }

    #[test]
    fn pinning_rewrites_every_slot_of_the_type_scale() {
        let family = FontFamily::named("Frust Corpus Face");
        let theme = pin_type_scale(Theme::neutral(), &family);
        for slot in [
            &theme.type_scale.display_large,
            &theme.type_scale.headline_medium,
            &theme.type_scale.title_large,
            &theme.type_scale.body_large,
            &theme.type_scale.label_small,
            &theme.type_scale.body_large_emphasized,
            &theme.type_scale.label_large_emphasized,
        ] {
            assert_eq!(slot.family, family);
        }
    }

    #[test]
    fn an_empty_scene_has_no_glyph_runs_and_no_foreign_fonts() {
        let scene = Scene::new();
        assert_eq!(glyph_run_count(&scene), 0);
        assert!(foreign_font_runs(&scene).is_empty());
    }

    #[test]
    fn a_frame_of_a_bare_text_view_shapes_against_the_bundled_face_only() {
        let (mut tcx, family) = test_text_context();
        let mut scene = Scene::new();
        let style = pinned_text_style(&family, 24.0, Color::BLACK);
        record_view(
            &mut scene,
            &FrameSpec::phone(),
            pin_type_scale(Theme::neutral(), &family),
            &mut tcx,
            move || frust_widgets::text(SAMPLE_TEXT).style(style.clone()),
        );
        assert!(
            glyph_run_count(&scene) > 0,
            "the text view shapes at least one run"
        );
        assert!(
            foreign_font_runs(&scene).is_empty(),
            "{:?}",
            foreign_font_runs(&scene)
        );
    }

    #[test]
    fn the_root_scale_reaches_the_recorded_geometry() {
        // The same view captured at scale 1 and scale 2: the scaled frame's
        // fill must cover four times the area, which is only true if the
        // paint-time root transform actually composed.
        fn area(scene: &Scene) -> f64 {
            scene
                .commands()
                .iter()
                .filter_map(|c| match c {
                    Command::FillRect {
                        rect, transform, ..
                    } => Some(transform.transform_rect_bbox(*rect).area()),
                    _ => None,
                })
                .sum()
        }
        let (mut tcx, family) = test_text_context();
        let theme = pin_type_scale(Theme::neutral(), &family);
        let build = || frust_widgets::test_support::leaf(100.0, 50.0);

        let mut unscaled = Scene::new();
        let spec1 = FrameSpec {
            size: PHONE_LOGICAL,
            scale: 1.0,
            time: FrameTime::ZERO,
        };
        record_view(&mut unscaled, &spec1, theme.clone(), &mut tcx, build);

        let mut scaled = Scene::new();
        record_view(&mut scaled, &FrameSpec::phone(), theme, &mut tcx, build);

        let (a1, a2) = (area(&unscaled), area(&scaled));
        assert!(a1 > 0.0, "the leaf fills a rect");
        assert!(
            (a2 - a1 * 4.0).abs() < 1e-6,
            "scale 2.0 must quadruple the painted area: {a1} -> {a2}"
        );
    }
}
