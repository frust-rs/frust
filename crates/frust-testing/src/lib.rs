//! Shared contracts for every golden/oracle/fuzz test in the render-testing
//! plan (`docs/TESTING.md`).
//!
//! `frust-testing` is `publish = false` and dev-only: no crate in this
//! workspace depends on it outside `[dev-dependencies]` (enforced by
//! `tests/deps.rs`'s guard). It stays a leaf — no `frust-render`, `vello`,
//! or `wgpu` dependency — so a headless golden test can pull it in without
//! dragging a GPU backend along; a concrete [`render::SceneRenderer`]
//! implementation (a `vello_cpu` oracle, an adapter over the real
//! `frust-render` pipeline, …) lives in whichever crate's tests construct
//! it.
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

pub mod case;
pub mod diff;
pub mod fonts;
pub mod golden;
pub mod meta;
pub mod render;

pub use case::{BackendSet, CaseSpec, Tolerance};
pub use diff::{DiffOutcome, DiffReport, PixelDiff};
pub use fonts::{register_test_fonts, test_fonts};
pub use golden::{GoldenOutcome, compare_golden};
pub use meta::GoldenMeta;
pub use render::{AlphaKind, BackendMeta, RenderSpec, RenderedImage, SceneRenderer};
