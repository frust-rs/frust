//! A single golden-case's deterministic inputs and comparison thresholds.
//!
//! Mirrors the per-test escalation shape a golden-test attribute macro would
//! carry (name, viewport, tolerance, backend skip-list, no-reference
//! marker) without depending on any macro crate — a test builds a
//! [`CaseSpec`] directly and hands it to a [`crate::render::SceneRenderer`]
//! via [`crate::render::RenderSpec`].

use peniko::Color;

/// Minimum period the frame is rendered before comparison — the same idea
/// vello's own dev-test harness escalates through per-test attributes
/// (name, dimensions, per-channel/alpha tolerance, diff-pixel budget). The
/// defaults below match that shape: a 100x100 viewport, channel tolerance 2,
/// alpha tolerance 2, and zero tolerated mismatched pixels — an exact match
/// unless a case explicitly widens it.
const DEFAULT_DIMENSION: u32 = 100;

/// Per-channel and whole-image mismatch thresholds a comparator enforces
/// against a golden case.
///
/// Defaults mirror upstream's per-test escalation attributes (channel 2,
/// alpha 2, zero tolerated mismatched pixels) — a case widens these
/// explicitly rather than a comparator guessing a looser default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Tolerance {
    /// Maximum allowed absolute difference per color channel (0-255 scale).
    pub channel: u8,
    /// Maximum allowed absolute difference in the alpha channel (0-255 scale).
    pub alpha: u8,
    /// Maximum number of pixels allowed to exceed the channel/alpha
    /// thresholds before the comparison fails.
    pub diff_pixels: u32,
}

impl Tolerance {
    /// The tight default every case starts from: channel 2, alpha 2, zero
    /// tolerated mismatched pixels.
    pub const fn new() -> Self {
        Self {
            channel: 2,
            alpha: 2,
            diff_pixels: 0,
        }
    }

    /// Exact-match tolerance (channel 0, alpha 0, zero mismatched pixels) —
    /// for deliberately integer-aligned solid geometry on the same CPU
    /// backend (`docs/TESTING.md`'s Comparison section).
    pub const fn exact() -> Self {
        Self {
            channel: 0,
            alpha: 0,
            diff_pixels: 0,
        }
    }

    /// The same tolerance with `channel` overridden.
    pub const fn with_channel(mut self, channel: u8) -> Self {
        self.channel = channel;
        self
    }

    /// The same tolerance with `alpha` overridden.
    pub const fn with_alpha(mut self, alpha: u8) -> Self {
        self.alpha = alpha;
        self
    }

    /// The same tolerance with `diff_pixels` overridden.
    pub const fn with_diff_pixels(mut self, diff_pixels: u32) -> Self {
        self.diff_pixels = diff_pixels;
        self
    }
}

/// A set of backend ids a golden case should be skipped on.
///
/// Matched against [`crate::render::SceneRenderer::id`] — a case that names
/// a backend here is never rendered/compared against that backend at all
/// (not rendered-and-ignored), the same "escalate per test" shape upstream's
/// dev-macro attributes use for a known-divergent backend.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BackendSet(Vec<&'static str>);

impl BackendSet {
    /// No backends skipped — the default every case starts from.
    pub const fn none() -> Self {
        Self(Vec::new())
    }

    /// Skips exactly the named backends.
    pub fn new(ids: impl IntoIterator<Item = &'static str>) -> Self {
        Self(ids.into_iter().collect())
    }

    /// Whether `id` is in this skip set.
    pub fn contains(&self, id: &str) -> bool {
        self.0.contains(&id)
    }

    /// Whether this set skips no backend.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// A named golden case: fixed viewport/color/scale inputs plus the
/// comparison thresholds and backend exceptions a comparator applies.
///
/// `name` is the only field with no default — every other field starts at
/// the tight defaults [`CaseSpec::new`] sets, mirroring upstream's per-test
/// escalation-attribute shape (100x100, tolerance `channel: 2, alpha: 2,
/// diff_pixels: 0`, no backend skips, a reference image expected).
#[derive(Clone, Debug, PartialEq)]
pub struct CaseSpec {
    /// The case's stable, human-readable name — also the golden-artifact
    /// stem a comparator/baseline-updater keys off.
    pub name: &'static str,
    /// Output width in physical pixels.
    pub width: u32,
    /// Output height in physical pixels.
    pub height: u32,
    /// The color the surface is cleared to before the scene is drawn.
    pub base_color: Color,
    /// Uniform device-pixel scale factor.
    pub scale: f64,
    /// Per-channel/whole-image mismatch thresholds.
    pub tolerance: Tolerance,
    /// Backends this case is never rendered or compared against.
    pub skip: BackendSet,
    /// When `true`, this case has no stored reference image yet — a
    /// comparator records/attaches the rendered output as an artifact
    /// without diffing it against a baseline (e.g. a case awaiting its
    /// first `UPDATE_GOLDENS=1` run).
    pub no_ref: bool,
}

impl CaseSpec {
    /// A new case named `name`, with every other field at its tight default:
    /// a 100x100 viewport, transparent base color, unit scale, the default
    /// [`Tolerance`], no backend skips, and a reference image expected.
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            width: DEFAULT_DIMENSION,
            height: DEFAULT_DIMENSION,
            base_color: Color::TRANSPARENT,
            scale: 1.0,
            tolerance: Tolerance::new(),
            skip: BackendSet::none(),
            no_ref: false,
        }
    }

    /// The same case with `width`/`height` overridden.
    pub const fn with_size(mut self, width: u32, height: u32) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    /// The same case with `base_color` overridden.
    pub const fn with_base_color(mut self, base_color: Color) -> Self {
        self.base_color = base_color;
        self
    }

    /// The same case with `scale` overridden.
    pub const fn with_scale(mut self, scale: f64) -> Self {
        self.scale = scale;
        self
    }

    /// The same case with `tolerance` overridden.
    pub const fn with_tolerance(mut self, tolerance: Tolerance) -> Self {
        self.tolerance = tolerance;
        self
    }

    /// The same case with `skip` overridden.
    pub fn with_skip(mut self, skip: BackendSet) -> Self {
        self.skip = skip;
        self
    }

    /// The same case marked as having no stored reference image yet.
    pub const fn with_no_ref(mut self, no_ref: bool) -> Self {
        self.no_ref = no_ref;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tolerance_defaults_match_upstream_shape() {
        let tolerance = Tolerance::new();
        assert_eq!(tolerance.channel, 2);
        assert_eq!(tolerance.alpha, 2);
        assert_eq!(tolerance.diff_pixels, 0);
    }

    #[test]
    fn tolerance_exact_is_zero_everywhere() {
        let tolerance = Tolerance::exact();
        assert_eq!(tolerance.channel, 0);
        assert_eq!(tolerance.alpha, 0);
        assert_eq!(tolerance.diff_pixels, 0);
    }

    #[test]
    fn tolerance_builders_override_one_field_at_a_time() {
        let tolerance = Tolerance::new()
            .with_channel(5)
            .with_alpha(7)
            .with_diff_pixels(3);
        assert_eq!(tolerance.channel, 5);
        assert_eq!(tolerance.alpha, 7);
        assert_eq!(tolerance.diff_pixels, 3);
    }

    #[test]
    fn tolerance_roundtrips_through_json() {
        let tolerance = Tolerance::new().with_channel(9);
        let json = serde_json::to_string(&tolerance).expect("serialize");
        let back: Tolerance = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(tolerance, back);
    }

    #[test]
    fn backend_set_none_is_empty_and_contains_nothing() {
        let set = BackendSet::none();
        assert!(set.is_empty());
        assert!(!set.contains("cpu"));
    }

    #[test]
    fn backend_set_contains_named_backends_only() {
        let set = BackendSet::new(["cpu", "vulkan-nvidia-t400"]);
        assert!(!set.is_empty());
        assert!(set.contains("cpu"));
        assert!(set.contains("vulkan-nvidia-t400"));
        assert!(!set.contains("android-emulator-api36-host"));
    }

    #[test]
    fn case_spec_new_matches_default_dimensions_and_tolerance() {
        let case = CaseSpec::new("solid_fill");
        assert_eq!(case.name, "solid_fill");
        assert_eq!(case.width, 100);
        assert_eq!(case.height, 100);
        assert_eq!(case.base_color, Color::TRANSPARENT);
        assert_eq!(case.scale, 1.0);
        assert_eq!(case.tolerance, Tolerance::new());
        assert!(case.skip.is_empty());
        assert!(!case.no_ref);
    }

    #[test]
    fn case_spec_builders_compose() {
        let case = CaseSpec::new("rounded_rect")
            .with_size(200, 150)
            .with_scale(2.0)
            .with_tolerance(Tolerance::exact())
            .with_skip(BackendSet::new(["android-emulator-api36-swiftshader"]))
            .with_no_ref(true);
        assert_eq!(case.width, 200);
        assert_eq!(case.height, 150);
        assert_eq!(case.scale, 2.0);
        assert_eq!(case.tolerance, Tolerance::exact());
        assert!(case.skip.contains("android-emulator-api36-swiftshader"));
        assert!(case.no_ref);
    }
}
