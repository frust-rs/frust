//! The named golden-case corpus every later phase regresses against.
//!
//! A corpus case is a [`CaseSpec`] (name, viewport, base color, scale,
//! tolerance, backend skips — the deterministic inputs) plus the one function
//! that records its scene and, optionally, a handful of [`Probe`]s: absolute
//! pixel expectations that hold on EVERY backend, independently of any stored
//! baseline.
//!
//! # Why probes exist alongside goldens
//!
//! A golden answers "did this change?"; a probe answers "is this right?". The
//! two promoted cases in [`unit`] came from `frust-render`'s `gpu_smoke`
//! tests as absolute arithmetic (an erase that must read `(0, 0, 0, 0)` one
//! pixel inside a tile boundary; a bracket that must composite to 191 and
//! 48), and that arithmetic is the actual contract — a golden alone would
//! happily freeze a wrong number. Promoting them into the corpus therefore
//! keeps both: the stored baseline AND the original assertions, at the
//! original tolerances.
//!
//! # Tolerances
//!
//! Every threshold is a field of the case's own [`CaseSpec::tolerance`], set
//! once where the case is declared. `docs/TESTING.md`'s Comparison section:
//! "Thresholds belong to the golden class or named test, not an ad hoc retry
//! path." Nothing in this module re-runs a comparison at a looser threshold.
//!
//! # Alpha
//!
//! The two oracle arms disagree about alpha representation — [`CpuOracle`]
//! hands back its `vello_cpu` pixmap PREMULTIPLIED, while
//! [`ClassicOracle`](crate::oracle_classic::ClassicOracle) hands back what
//! vello writes, STRAIGHT — and golden IO refuses a premultiplied image
//! outright ([`crate::golden::compare_golden`]). [`straighten_alpha`] is the
//! single conversion site that closes that gap, and [`render_case`] is the
//! only caller: every image that reaches a comparator or a stored PNG has
//! passed through it.

pub mod unit;

use anyhow::Result;
use frust_scene::Scene;

use crate::case::CaseSpec;
use crate::render::{AlphaKind, RenderSpec, RenderedImage, SceneRenderer};

pub use unit::unit_cases;

/// What a [`Probe`] asserts about one pixel.
///
/// Deliberately three narrow shapes rather than a general predicate: a
/// promoted case's expectation has to survive review as a *number*, and a
/// closure would let a later edit weaken it invisibly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expect {
    /// Exactly these RGBA bytes — no tolerance at all. The shape a
    /// deliberately integer-aligned erase or solid fill is asserted with.
    Exact([u8; 4]),
    /// Channel `channel` (0 = R, 1 = G, 2 = B, 3 = A) within `tolerance` of
    /// `value` — the shape composite arithmetic is asserted with, where the
    /// tolerance is an 8-bit rounding step and nothing more.
    Channel {
        /// Channel index: 0 = R, 1 = G, 2 = B, 3 = A.
        channel: usize,
        /// The exact value the arithmetic predicts.
        value: u8,
        /// Maximum allowed absolute deviation from `value`.
        tolerance: u8,
    },
    /// Channel `channel` is at least `min` — the shape a "this survived"
    /// check takes, where the exact value depends on a backend's blending
    /// but the qualitative answer does not.
    AtLeast {
        /// Channel index: 0 = R, 1 = G, 2 = B, 3 = A.
        channel: usize,
        /// Inclusive lower bound.
        min: u8,
    },
}

/// One absolute pixel expectation, checked on every backend a case renders
/// on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Probe {
    /// Pixel column.
    pub x: u32,
    /// Pixel row.
    pub y: u32,
    /// What must hold there.
    pub expect: Expect,
    /// Why — quoted verbatim into the failure message, so a red probe reads
    /// as the contract it broke rather than as a coordinate pair.
    pub why: &'static str,
}

impl Probe {
    /// Checks this probe against `image`, returning a human-readable failure
    /// when it does not hold.
    ///
    /// A probe outside the image is itself a failure (a stale coordinate
    /// after a viewport change), reported rather than panicking on the
    /// index.
    fn check(&self, image: &RenderedImage) -> Option<String> {
        if self.x >= image.width || self.y >= image.height {
            return Some(format!(
                "probe ({}, {}) is outside the {}x{} frame — {}",
                self.x, self.y, image.width, image.height, self.why
            ));
        }
        let at = ((self.y * image.width + self.x) * 4) as usize;
        let pixel = [
            image.rgba8[at],
            image.rgba8[at + 1],
            image.rgba8[at + 2],
            image.rgba8[at + 3],
        ];
        match self.expect {
            Expect::Exact(expected) if pixel != expected => Some(format!(
                "pixel ({}, {}) is {pixel:?}, expected exactly {expected:?} — {}",
                self.x, self.y, self.why
            )),
            Expect::Channel {
                channel,
                value,
                tolerance,
            } if pixel[channel].abs_diff(value) > tolerance => Some(format!(
                "pixel ({}, {}) channel {channel} is {}, expected {value} +/- {tolerance} \
                 (whole pixel {pixel:?}) — {}",
                self.x, self.y, pixel[channel], self.why
            )),
            Expect::AtLeast { channel, min } if pixel[channel] < min => Some(format!(
                "pixel ({}, {}) channel {channel} is {}, expected at least {min} \
                 (whole pixel {pixel:?}) — {}",
                self.x, self.y, pixel[channel], self.why
            )),
            _ => None,
        }
    }
}

/// One named corpus case: its deterministic inputs, the scene it records, and
/// the backend-independent pixel expectations it carries.
pub struct CorpusCase {
    /// Name, viewport, base color, scale, tolerance and backend skips.
    pub spec: CaseSpec,
    /// Records the case's commands into a fresh [`Scene`], through
    /// [`frust_scene::SceneBuilder`] only — the corpus records no
    /// [`frust_scene::Command`] by hand, so a case can never encode a
    /// display list the widget layer could not have produced.
    pub record: fn(&mut Scene),
    /// Absolute pixel expectations checked on every backend (see the module
    /// docs). Empty for a case whose only contract is its baseline.
    pub probes: &'static [Probe],
    /// Whether the comparator erodes the raw above-threshold mask by 1 px
    /// before counting mismatches (see [`crate::diff::diff_images`]) — set
    /// on a case whose expected difference shape is antialiased edge jitter
    /// rather than an interior change.
    pub eroded_interior: bool,
    /// One line on what this case exists to pin, for the corpus listing and
    /// for a reviewer reading a promoted PNG cold.
    pub about: &'static str,
}

impl CorpusCase {
    /// This case's [`RenderSpec`], derived from its [`CaseSpec`] — the two
    /// carry the same deterministic inputs under different names, and this
    /// is the one place they are mapped.
    #[must_use]
    pub fn render_spec(&self) -> RenderSpec {
        RenderSpec {
            width: self.spec.width,
            height: self.spec.height,
            base_color: self.spec.base_color,
            scale: self.spec.scale,
            root: kurbo::Affine::IDENTITY,
        }
    }

    /// The scene this case records, built fresh.
    #[must_use]
    pub fn scene(&self) -> Scene {
        let mut scene = Scene::new();
        (self.record)(&mut scene);
        scene
    }

    /// Every failing [`Probe`] on `image`, as review-ready messages. Empty
    /// when the case's absolute expectations all hold.
    #[must_use]
    pub fn failed_probes(&self, image: &RenderedImage) -> Vec<String> {
        self.probes
            .iter()
            .filter_map(|probe| probe.check(image))
            .collect()
    }
}

/// Converts a premultiplied frame to straight alpha, leaving an already
/// straight one untouched.
///
/// # The conversion, and its rounding
///
/// For each pixel, `straight = round(premultiplied * 255 / alpha)`, computed
/// in `u32` with the `+ alpha / 2` round-half-up bias and clamped to 255 (a
/// channel above its own alpha is malformed, not a reason to wrap). A fully
/// transparent pixel keeps `[0, 0, 0, 0]`: there is no color information left
/// to divide out, and `Command::ClearRect`'s contract is exactly that an
/// erased pixel reads all-zero, so inventing anything else would break the
/// alpha assertion `diff.rs` exists to make.
///
/// # Why here rather than in the oracle
///
/// [`CpuOracle`](crate::oracle_cpu::CpuOracle) deliberately hands back its
/// pixmap unconverted, because un-premultiplying is LOSSY — a low-alpha
/// pixel's color channels carry almost no information once divided out, and
/// quantizing the quotient back into 8 bits cannot recover it. That loss is
/// acceptable here and only here: golden storage is PNG, PNG is straight
/// alpha, and both arms of the pair must be stored the same way to be
/// comparable at all. The loss is deterministic (integer arithmetic, no
/// floats), so a baseline promoted through it is stable; it is NOT a
/// round-trippable transform, so nothing downstream should convert back.
#[must_use]
pub fn straighten_alpha(image: &RenderedImage) -> RenderedImage {
    if image.alpha == AlphaKind::Straight {
        return image.clone();
    }
    let mut rgba8 = image.rgba8.clone();
    for pixel in rgba8.chunks_exact_mut(4) {
        let alpha = u32::from(pixel[3]);
        if alpha == 0 {
            pixel[0] = 0;
            pixel[1] = 0;
            pixel[2] = 0;
            continue;
        }
        for channel in &mut pixel[..3] {
            let value = (u32::from(*channel) * 255 + alpha / 2) / alpha;
            *channel = u8::try_from(value.min(255)).expect("clamped to 255");
        }
    }
    RenderedImage {
        width: image.width,
        height: image.height,
        rgba8,
        alpha: AlphaKind::Straight,
        meta: image.meta.clone(),
    }
}

/// Renders `case` on `renderer`, returning `None` when the case's
/// [`CaseSpec::skip`] set names this backend.
///
/// The returned frame is always STRAIGHT alpha (see [`straighten_alpha`]), so
/// a caller can hand it straight to [`crate::golden::compare_golden`] without
/// each call site re-deciding what the backend's convention was.
///
/// # Errors
///
/// Propagates the backend's own render error.
pub fn render_case(
    renderer: &mut dyn SceneRenderer,
    case: &CorpusCase,
) -> Result<Option<RenderedImage>> {
    if case.spec.skip.contains(renderer.id()) {
        return Ok(None);
    }
    let scene = case.scene();
    let image = renderer.render(&scene, &case.render_spec())?;
    Ok(Some(straighten_alpha(&image)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::BackendMeta;

    fn image(pixels: &[[u8; 4]], alpha: AlphaKind) -> RenderedImage {
        RenderedImage {
            width: pixels.len() as u32,
            height: 1,
            rgba8: pixels.iter().flatten().copied().collect(),
            alpha,
            meta: BackendMeta::default(),
        }
    }

    #[test]
    fn straightening_an_opaque_pixel_is_the_identity() {
        let source = image(&[[10, 200, 255, 255]], AlphaKind::Premultiplied);
        let straight = straighten_alpha(&source);
        assert_eq!(straight.rgba8, vec![10, 200, 255, 255]);
        assert_eq!(straight.alpha, AlphaKind::Straight);
    }

    #[test]
    fn straightening_divides_out_the_alpha_and_rounds_half_up() {
        // Half-alpha green: premultiplied 64 over alpha 128 -> 64 * 255 / 128
        // = 127.5, rounded half up to 128.
        let source = image(&[[0, 64, 0, 128]], AlphaKind::Premultiplied);
        assert_eq!(straighten_alpha(&source).rgba8, vec![0, 128, 0, 128]);
    }

    #[test]
    fn straightening_keeps_a_fully_erased_pixel_all_zero() {
        let source = image(&[[0, 0, 0, 0]], AlphaKind::Premultiplied);
        assert_eq!(straighten_alpha(&source).rgba8, vec![0, 0, 0, 0]);
    }

    #[test]
    fn straightening_clamps_a_malformed_over_alpha_channel() {
        let source = image(&[[200, 0, 0, 100]], AlphaKind::Premultiplied);
        assert_eq!(straighten_alpha(&source).rgba8[0], 255);
    }

    #[test]
    fn a_straight_frame_passes_through_untouched() {
        let source = image(&[[1, 2, 3, 4]], AlphaKind::Straight);
        assert_eq!(straighten_alpha(&source).rgba8, vec![1, 2, 3, 4]);
    }

    #[test]
    fn probes_report_the_reason_they_failed() {
        let frame = image(&[[10, 20, 30, 255]], AlphaKind::Straight);
        let probe = Probe {
            x: 0,
            y: 0,
            expect: Expect::Exact([0, 0, 0, 0]),
            why: "the punch must erase",
        };
        let message = probe.check(&frame).expect("probe must fail");
        assert!(message.contains("the punch must erase"), "{message}");
    }

    #[test]
    fn a_channel_probe_honours_exactly_its_own_tolerance() {
        let frame = image(&[[0, 51, 0, 255]], AlphaKind::Straight);
        let within = Probe {
            x: 0,
            y: 0,
            expect: Expect::Channel {
                channel: 1,
                value: 48,
                tolerance: 3,
            },
            why: "at the edge of tolerance",
        };
        assert!(within.check(&frame).is_none());

        let outside = Probe {
            x: 0,
            y: 0,
            expect: Expect::Channel {
                channel: 1,
                value: 47,
                tolerance: 3,
            },
            why: "one step past tolerance",
        };
        assert!(outside.check(&frame).is_some());
    }

    #[test]
    fn a_probe_outside_the_frame_fails_rather_than_panicking() {
        let frame = image(&[[0, 0, 0, 0]], AlphaKind::Straight);
        let probe = Probe {
            x: 99,
            y: 0,
            expect: Expect::Exact([0, 0, 0, 0]),
            why: "stale coordinate",
        };
        assert!(probe.check(&frame).is_some());
    }
}
