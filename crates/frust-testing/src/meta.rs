//! The per-golden JSON metadata contract.
//!
//! `docs/TESTING.md`'s Golden Image Policy requires every promoted baseline
//! to carry enough context to judge whether a later mismatch is a real
//! regression or an environment drift: which backend/adapter/driver
//! produced it, which OS, which Frust commit, which named case, and under
//! what tolerance. [`GoldenMeta`] is that record — a comparator/golden-IO
//! layer (a later card in this plan) serializes one alongside every
//! promoted expected/actual/diff artifact.

use serde::{Deserialize, Serialize};

use crate::case::Tolerance;
use crate::render::BackendMeta;

/// The metadata JSON stored beside a golden image.
///
/// Field-for-field, this is the subset of `docs/TESTING.md`'s Golden Image
/// Policy that identifies *this one comparison*: backend identity
/// ([`BackendMeta`]'s three fields plus the OS the run happened on), the
/// Frust commit the baseline was produced from, which named case it belongs
/// to, the [`Tolerance`] it was compared under, and when it was captured.
/// Deliberately excludes render-input fields (viewport, theme, locale, …)
/// that already live on [`crate::case::CaseSpec`] — this struct is the
/// *provenance* record, not a second copy of the case's own inputs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GoldenMeta {
    /// The backend's stable id (`SceneRenderer::id`, e.g. `"cpu"`,
    /// `"vulkan-nvidia-t400"`).
    pub backend: String,
    /// The GPU/software adapter name that rendered this image.
    pub adapter: String,
    /// Driver/API version string.
    pub driver: String,
    /// The operating system the render ran on (e.g. `std::env::consts::OS`).
    pub os: String,
    /// The Frust git commit the baseline was produced from.
    pub frust_commit: String,
    /// The [`crate::case::CaseSpec::name`] this metadata belongs to.
    pub case: String,
    /// The tolerance the comparison was run under.
    pub tolerance: Tolerance,
    /// Capture time, RFC 3339 — an explicit timestamp, never a value a
    /// reader has to reconstruct from filesystem mtimes.
    pub timestamp: String,
}

impl GoldenMeta {
    /// Builds a [`GoldenMeta`] from a rendered backend's own
    /// [`BackendMeta`] plus the run-level fields it doesn't carry (OS,
    /// commit, case name, tolerance, timestamp).
    pub fn new(
        backend_meta: &BackendMeta,
        os: impl Into<String>,
        frust_commit: impl Into<String>,
        case: impl Into<String>,
        tolerance: Tolerance,
        timestamp: impl Into<String>,
    ) -> Self {
        Self {
            backend: backend_meta.backend.clone(),
            adapter: backend_meta.adapter.clone(),
            driver: backend_meta.driver.clone(),
            os: os.into(),
            frust_commit: frust_commit.into(),
            case: case.into(),
            tolerance,
            timestamp: timestamp.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_backend_meta() -> BackendMeta {
        BackendMeta {
            backend: "cpu".into(),
            adapter: "vello_cpu".into(),
            driver: "0.2.0".into(),
            device_kind: "software".into(),
        }
    }

    #[test]
    fn new_copies_backend_identity_and_sets_run_fields() {
        let meta = GoldenMeta::new(
            &sample_backend_meta(),
            "linux",
            "abc1234",
            "solid_fill",
            Tolerance::new(),
            "2026-08-29T00:00:00Z",
        );
        assert_eq!(meta.backend, "cpu");
        assert_eq!(meta.adapter, "vello_cpu");
        assert_eq!(meta.driver, "0.2.0");
        assert_eq!(meta.os, "linux");
        assert_eq!(meta.frust_commit, "abc1234");
        assert_eq!(meta.case, "solid_fill");
        assert_eq!(meta.tolerance, Tolerance::new());
        assert_eq!(meta.timestamp, "2026-08-29T00:00:00Z");
    }

    #[test]
    fn roundtrips_through_json_with_every_field_present() {
        let meta = GoldenMeta::new(
            &sample_backend_meta(),
            "linux",
            "abc1234",
            "solid_fill",
            Tolerance::new(),
            "2026-08-29T00:00:00Z",
        );
        let json = serde_json::to_value(&meta).expect("serialize");
        for field in [
            "backend",
            "adapter",
            "driver",
            "os",
            "frust_commit",
            "case",
            "tolerance",
            "timestamp",
        ] {
            assert!(
                json.get(field).is_some(),
                "missing field `{field}` in {json}"
            );
        }

        let back: GoldenMeta = serde_json::from_value(json).expect("deserialize");
        assert_eq!(meta, back);
    }
}
