//! Shared contracts for every golden/oracle/fuzz test in the render-testing
//! plan (`docs/TESTING.md`).
//!
//! `frust-testing` is `publish = false` and dev-only: no crate in this
//! workspace depends on it outside `[dev-dependencies]` (enforced by
//! `tests/deps.rs`'s guard), so nothing here ever reaches a production
//! dependency graph. Both arms of the oracle PAIR ship here — the GPU-free
//! [`oracle_cpu::CpuOracle`] and the `frust-render`-backed
//! [`oracle_classic::ClassicOracle`] — because [`corpus`] hands the same
//! case to both behind one [`render::SceneRenderer`]. That is why this crate
//! carries a `frust-render` edge (at its DEFAULT feature set); see the
//! manifest's comment for why it is sound. Only `oracle_classic` needs a GPU
//! at run time: every other module, the CPU arm included, runs on a machine
//! without one.
//!
//! - [`render`]: the renderer-agnostic [`render::SceneRenderer`] trait plus
//!   its [`render::RenderSpec`]/[`render::RenderedImage`]/
//!   [`render::BackendMeta`] types.
//! - [`case`]: a golden case's deterministic inputs and comparison
//!   thresholds ([`case::CaseSpec`]/[`case::Tolerance`]/[`case::BackendSet`]).
//! - [`meta`]: the per-golden JSON provenance record
//!   ([`meta::GoldenMeta`]) `docs/TESTING.md`'s Golden Image Policy
//!   requires alongside a promoted baseline.
//! - [`diff`]: the per-pixel per-channel comparator every golden gate uses
//!   ([`diff::diff_images`]/[`diff::DiffReport`]).
//! - [`golden`]: golden-image load/compare/store
//!   ([`golden::compare_golden`]), `UPDATE_GOLDENS=1`-gated writes.
//! - [`fonts`]: bundled, permissively-licensed test fonts
//!   ([`fonts::test_fonts`]/[`fonts::register_test_fonts`]) for a text golden
//!   that must not depend on whatever font the host happens to have
//!   installed — see that module's docs.
//! - [`oracle_cpu`]: [`oracle_cpu::CpuOracle`], the deterministic `vello_cpu`
//!   0.2.0 reference renderer a golden case is compared against.
//! - [`oracle_classic`]: [`oracle_classic::ClassicOracle`], the GPU arm of
//!   that pair over `frust-render`'s offscreen vello-classic renderer, plus
//!   the adapter-to-golden-class routing ([`oracle_classic::golden_class`]).
//! - [`corpus`]: the named golden cases themselves
//!   ([`corpus::CorpusCase`]/[`corpus::Probe`]), starting with
//!   [`corpus::unit`] — one case per `frust_scene::Command` variant.

pub mod case;
pub mod corpus;
pub mod diff;
pub mod fonts;
pub mod golden;
pub mod meta;
pub mod oracle_classic;
pub mod oracle_cpu;
pub mod render;

pub use case::{BackendSet, CaseSpec, Tolerance};
pub use corpus::{CorpusCase, Expect, Probe, render_case, straighten_alpha, unit_cases};
pub use diff::{DiffOutcome, DiffReport, PixelDiff};
pub use fonts::{register_test_fonts, test_fonts};
pub use golden::{GoldenOutcome, compare_golden};
pub use meta::GoldenMeta;
pub use oracle_classic::{ClassicOracle, UNCLASSIFIED_CLASS, golden_class};
pub use oracle_cpu::{CpuOracle, ORACLE_ID, SkipReport};
pub use render::{AlphaKind, BackendMeta, RenderSpec, RenderedImage, SceneRenderer};
