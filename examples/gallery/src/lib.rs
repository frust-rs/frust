//! `frust-gallery`: the shared widget/page CASE REGISTRY both the static
//! snapshot generator (`crates/frust-testing`'s widget-snapshots bin) and the
//! future browser gallery app (`examples/web-gallery`) read.
//!
//! # The pure-`View` constraint
//!
//! Every [`Case::build`] is a plain `fn() -> `[`frust_core::AnyView`]`<()>` —
//! it is called fresh on every rebuild pass and never touches a reactive
//! runtime or a wall clock. That mirrors `crates/frust-testing/src/frame.rs`'s
//! `record_view`, the recorder both consumers of this registry drive a case
//! through: it builds a `RenderRoot<(), V>`, calls `build()` again on every
//! rebuild pass (`layout_with_text` shapes text against a supplied
//! `TextContext`, `paint` records into a `SceneBuilder`), with no reactive
//! runtime and no ambient owner attached at all — `record_view`'s own doc
//! comment says so directly. That does not mean retained state panics here:
//! nothing in `reactive_graph` 0.2.14 (the version this workspace pins)
//! panics for want of an ambient owner — `Owner::new()` simply creates a
//! parentless owner when there is none to nest under. The constraint that IS
//! real is a CLOCK, not reactivity: the recorder normally paints a case
//! exactly once, and `AnimationController::advance` contributes a zero delta
//! on the first call after a motion starts (`crates/frust-core/src/anim.rs`),
//! so an unconditionally-started ramp records at progress 0 by default. Time
//! is captured instead through [`Case::time_ms`], a fixed
//! [`frust_core::FrameTime`] input an animating widget differences itself
//! against, never sampled live — and a case that pins `time_ms` above
//! [`Case::DEFAULT_TIME_MS`] can ask the recorder for a warm pass to actually
//! land on that instant instead of progress 0; see [`Case::warm_frames`] for
//! the full contract.
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
//! # The live-host escape hatch
//!
//! That constraint is what makes a recorded frame deterministic, and it is
//! also why nothing in this registry can be *operated*: frust's input widgets
//! are controlled, so a `()`-stated case's `on_toggle`/`on_change` has nowhere
//! to write and every one of them is a no-op closure. A host that is live
//! rather than recording (`examples/web-gallery`) may build a case from the
//! optional slug-keyed [`interactive`] side table instead, whose constructors
//! carry retained [`frust_core::Component`] state. It is a table beside the
//! registry, never a field on [`Case`], precisely so [`Case::build`] — and
//! therefore the snapshot oracle — cannot be reached from it; see that
//! module's own docs.
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
pub mod interactive;
pub mod material;
pub mod shadcn;
pub mod theme;

pub use case::{Case, Design, Variant};
pub use interactive::find as find_interactive;
pub use theme::theme;

/// Every design-system case module's `CASES` slice, in the website
/// sidebar's order (`apps/website/widgets/design-systems/<design>/_category_.json`
/// `position`: material, cupertino, glyph, shadcn, beui); the `Base` set
/// comes first via [`base::cases`]. The order is observable — it is the
/// `--list` order and a fresh manifest's row order — but nothing depends on
/// it. Adding a design system means adding its module above and its slice
/// here at the sidebar position — nothing else.
const DESIGN_PARTS: &[&[Case]] = &[
    material::CASES,
    cupertino::CASES,
    glyph::CASES,
    shadcn::CASES,
    beui::CASES,
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
