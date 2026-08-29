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

pub mod case;
pub mod meta;
pub mod render;

pub use case::{BackendSet, CaseSpec, Tolerance};
pub use meta::GoldenMeta;
pub use render::{AlphaKind, BackendMeta, RenderSpec, RenderedImage, SceneRenderer};
