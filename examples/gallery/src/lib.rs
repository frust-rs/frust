//! `frust-gallery`: the shared widget/page CASE REGISTRY both the static
//! snapshot generator (`crates/frust-testing`'s widget-snapshots bin) and the
//! future browser gallery app (`examples/web-gallery`) read.
//!
//! # The pure-`View` constraint
//!
//! Every [`Case::build`] is a plain `fn() -> `[`frust_core::AnyView`]`<()>` —
//! no reactive runtime, no owner, and no wall clock. That mirrors
//! `crates/frust-testing/src/frame.rs`'s `record_view`, the recorder both
//! consumers of this registry drive a case through: it builds a
//! `RenderRoot<(), V>` with no runtime attached at all
//! (`root.rebuild(logic, state)` just calls `build()` again, `layout_with_text`
//! shapes text against a supplied `TextContext`, `paint` records into a
//! `SceneBuilder`) — so a case that reached for `use_signal`/`use_context`/a
//! system clock would panic the moment it tried to record a frame. Time is
//! captured instead through [`Case::time_ms`], a fixed [`frust_core::FrameTime`]
//! input an animating widget differences itself against, never sampled live.
//!
//! # The slug rule
//!
//! A case's [`Case::slug`] is the website page's file stem it corresponds to
//! (`button`, `icon-button`, `animated-opacity`, ...). A design-system
//! variant of a Base page prefixes that stem with its own tag:
//! `material/button`, `cupertino/button`, `glyph/button`, `shadcn/button`,
//! `beui/button`. Slugs are unique across the whole registry — see
//! `tests/registry.rs`.
//!
//! # Dev-only
//!
//! Like `crates/frust-testing`, this crate exists to be read by test/tooling
//! binaries, never shipped in an app's own dependency graph.
//!
//! # What this crate actually depends on
//!
//! Its own manifest declares no renderer edge — the case modules build
//! `View` trees out of `frust-core`/`frust-widgets`/`frust-theme`/
//! `frust-text` and never touch a surface, a device or a frame. The five
//! design-system plugins it depends on for [`theme`] each depend on the
//! `frust` facade, though, and the facade pulls `frust-render` ->
//! `frust-gpu` -> `wgpu`, so the renderer graph IS in this crate's
//! transitive dependencies (`cargo tree -p frust-gallery -e normal -i wgpu`
//! shows the path). Compiling this crate for `wasm32` is therefore not yet
//! established: it depends on that graph being made wasm-clean, which the
//! web-shell work owns.
//!
//! See `README.md` for how to add a case.

pub mod base;
pub mod beui;
pub mod case;
pub mod cupertino;
pub mod glyph;
pub mod material;
pub mod shadcn;
pub mod theme;

pub use case::{Case, Design, Variant};
pub use theme::theme;

/// Every design-system case module's `CASES` slice, in the order their
/// slugs sort on the website (the `Base` set comes first via
/// [`base::cases`]). Adding a design system means adding its module above
/// and its slice here — nothing else.
const DESIGN_PARTS: &[&[Case]] = &[
    material::CASES,
    shadcn::CASES,
    glyph::CASES,
    beui::CASES,
    cupertino::CASES,
];

/// The whole case registry: the `Base` page set ([`base::cases`]) followed by
/// every design-system module's `CASES` slice ([`DESIGN_PARTS`]),
/// concatenated once and cached for the process — `Case` is `Copy`, so the
/// slices are stitched, never rebuilt. Slug uniqueness across the whole
/// registry is enforced by `tests/registry.rs`.
pub fn cases() -> &'static [Case] {
    static ALL: std::sync::OnceLock<Vec<Case>> = std::sync::OnceLock::new();
    ALL.get_or_init(|| {
        let mut all = base::cases().to_vec();
        for part in DESIGN_PARTS {
            all.extend_from_slice(part);
        }
        all
    })
}

/// Look up a single case by its [`Case::slug`]. `None` if no case in the
/// registry carries that slug.
pub fn find(slug: &str) -> Option<&'static Case> {
    cases().iter().find(|case| case.slug == slug)
}
