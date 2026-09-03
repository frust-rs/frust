//! `frust-beui`: beUI, packaged as a design-system plugin.
//!
//! This is the design-system tier's second **external-origin** catalog: beUI is
//! a third-party web design system, and this crate is a *port* of it — its token
//! tables and component designs are transcribed from the upstream repository
//! (beUI v2, `github.com/starc007/ui-components`, rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01), not
//! authored here. Upstream is MIT-licensed, © 2026 Saurabh Chauhan. Every value
//! carries its source; every place the web original has no frust equivalent
//! carries the decision that resolved it.
//!
//! The crate is the whole system: the [`tokens`] module (the two vendored
//! per-brightness tables, the 46-role `ColorScheme` fold, the [`BeuiTokens`]
//! extension, the three glass recipes, the easing/spring constants, the bundled
//! Geist + Geist Mono faces, the assembled [`theme()`](fn@theme)), the shared
//! [`style`] vocabulary every component paints from, and the component catalog
//! itself.
//!
//! # Charter
//!
//! - **Facade-only.** Widgets are `View`/`Widget` pairs authored against
//!   `frust::authoring` (plus `kurbo`/`peniko` for geometry and color); this
//!   crate names no other framework crate, and nothing reactive. That is what
//!   makes it a real proof of the external design-system contract rather than a
//!   special-cased in-tree exception — an app depends on it exactly the way it
//!   would depend on any third-party catalog.
//! - **Token-driven, never design-language-branching.** A component resolves
//!   `Theme::from_paint_ctx`/`from_layout_ctx` and reads the beUI token tables
//!   with an unthemed-fallback constant per resolved value, exactly like every
//!   baseline widget. It never matches on `DesignLanguage`; a beUI theme is just
//!   a `Theme` whose token tables happen to be the beUI ones (tagged
//!   `DesignLanguage::Custom("beui")` for identity, not for branching).
//! - **Motion is the point.** Upstream is a *motion* library first — nearly
//!   every component it ships is a static design plus an animation — so a port
//!   that drops the motion has not ported the component. The curves and springs
//!   are first-class tokens ([`tokens::motion`]) rather than per-component
//!   numbers, and reduced-motion is a requirement, not a nicety: upstream gates
//!   its own effects on `useReducedMotion`, and so must a port.
//! - **Desktop-first, touch-friendly by accident.** beUI's own metrics are kept:
//!   [`style::HEIGHT_MD`] is `h-10` = 40px and [`style::HEIGHT_INPUT`] is `h-11`
//!   = 44px, so its controls land at or near the mobile tap-target convention
//!   without a density mechanism being invented here.
//!
//! # Installing it
//!
//! [`install`] is the one-line entry point, and it must run **before the first
//! frame**: a shell reads the default-theme slot and drains the font registry
//! once, at construction. The supported place is `app!`'s `setup` block, which
//! runs before any shell construction:
//!
//! ```no_run
//! use frust::{AnyView, Component, any, text};
//!
//! #[derive(Default)]
//! struct MyApp;
//!
//! impl Component for MyApp {
//!     type State = ();
//!     fn init(&self) -> Self::State {}
//!     fn build(&self, _state: &mut Self::State) -> AnyView<Self::State> {
//!         any(text("beUI"))
//!     }
//! }
//!
//! frust::app!(MyApp, setup = { frust_beui::install(); });
//! # fn main() {}
//! ```
//!
//! # Namespace
//!
//! Unlike the sibling external-origin catalog, the component modules stay
//! **behind their family names** — `frust_beui::components::button`,
//! `frust_beui::agents::message`, `frust_beui::blocks::command_palette` — rather
//! than being glob-re-exported into one flat crate-root namespace.
//!
//! Upstream ships three catalogs, not one, and they cover overlapping ground: a
//! `swap` block and an `action-swap` component, an `expandable-tabs` block and a
//! `tabs` component, a `message` part and a `message-bubble` part. Flattening
//! all 81 into a single namespace would make every symbol need a family prefix
//! anyway, so the module path carries it instead. The design language itself —
//! the theme constructors, the extension, the tables and the font bytes — *is*
//! flattened to the root below, since there is only one of each.
//!
//! Public symbols within a module are still module-prefixed (`ButtonVariant`,
//! not a bare `Variant`), which each family's `mod.rs` records.
//!
//! # Fonts
//!
//! Geist Variable and Geist Mono Variable are bundled **unconditionally**
//! (`include_bytes!`) — depending on this plugin is itself the opt-in, so there
//! is no second feature to switch them off with. Both are OFL-1.1 with no
//! Reserved Font Name, and ship with their license text
//! (`plugins/beui/fonts/geist/OFL.txt`, `fonts/geist-mono/OFL.txt`). [`install`]
//! registers both faces and the theme binds Geist into the `NativeTypefaces`
//! slots native controls read; [`font_data`] exposes the raw bytes for a host
//! that wants them directly.
//!
//! # The `gpu-effects` feature
//!
//! Non-default, and the only feature this crate has. It adds `gpu_fx`, the
//! GPU substrate the catalog's true-3D component variants render through: a
//! perspective quad renderer driven from an `ExternalPass` the engine calls
//! once per frame ahead of its own scene pass, writing into a pooled offscreen
//! target the engine then composites. Turning it on pulls in the facade's
//! `gpu` feature and a `wgpu` edge of this crate's own (the facade re-exports
//! the GPU types but not `wgpu`, so a caller recording its own render pass
//! names it directly). A default build carries none of that and behaves
//! exactly as it did before the feature existed.
//!
//! What it buys is **real perspective**, which nothing above the render tier
//! can otherwise express: a scene texture composites under an `Affine`, so a
//! 2D "3D card" is a shear no matter how it is tuned, while a face this
//! substrate draws is projected through an actual frustum.
//!
//! What it still cannot do, and will not be made to:
//!
//! - **Arbitrary child subtrees are never perspective-transformed.** The
//!   substrate tilts faces *it* renders — a colour, a gradient, or a texture
//!   the caller produced. A widget subtree is composited under `Affine` and
//!   stays that way, so a 3D face carries no text and no child widgets.
//! - **Every 3D component keeps its 2D path.** Acquisition answers `None`
//!   whenever the GPU is unreachable — before a shell's first surface, on
//!   Android and iOS today, or with the substrate's kill switch set — and the
//!   component renders flat. The 3D path is an enhancement, never the only
//!   way a component draws.
//!
//! See `gpu_fx`'s own module docs for the acquisition/degrade contract, the
//! camera tuning, the pooling policy, and the one seam gap the variants built
//! on it inherit.
//!

pub mod agents;
pub mod blocks;
pub mod components;
/// The GPU-effect substrate behind the non-default `gpu-effects` feature —
/// perspective quad rendering, offscreen target pooling, and the per-frame
/// pass the engine records it through. See the crate docs' *The `gpu-effects`
/// feature* for what it adds and what it deliberately does not.
#[cfg(feature = "gpu-effects")]
pub mod gpu_fx;
/// The animation substrate every component in this catalog animates with:
/// stagger driving, per-character text cells, pointer tracking, scroll-effect
/// adaptation, and kept-mounted exit presence. The motion *tokens* it is timed
/// by — the curves and springs — stay in [`tokens::motion`].
pub mod motion;
/// The overlay hosting seam every panel that leaves its parent's box is built
/// on: anchored (trigger-relative) placement and modal (scrim + panel) hosting,
/// both staged by [`motion::Presence`].
pub mod overlay;
/// The ramp-driven scalars, pointer/key admission tests, and small paint
/// helpers every interactive component in this catalog shares — see the
/// [module docs](press).
pub(crate) mod press;
pub mod style;
/// The cached shaped-text run types every text-bearing component in this
/// catalog shapes its labels through — see the [module docs](text).
pub(crate) mod text;
pub mod tokens;

/// The design language itself, flattened to the root: the assembled
/// [`theme()`](fn@theme), the [`BeuiTokens`] extension, the vendored
/// [`BeuiPalette`] tables and their glass/gradient groups, and the bundled
/// [`font_data`]. The per-scale constructors
/// ([`color_scheme`](tokens::color_scheme), [`type_scale`](tokens::type_scale),
/// [`shape_scale`](tokens::shape_scale), [`glass_scale`](tokens::glass_scale))
/// stay behind [`tokens`], as do the motion constants
/// ([`tokens::motion`]) — a component names those through the module that
/// explains them.
pub use tokens::{
    BEUI_DARK, BEUI_DESIGN_LANGUAGE, BEUI_LIGHT, BeuiGlass, BeuiGradient, BeuiPalette, BeuiTokens,
    FALLBACK_RING, font_data, palette,
};
// Named through its own module path rather than folded into the group above: a
// bare `tokens::theme` names both the module and the function (Rust keeps them
// in separate namespaces), and a single-name re-export would lift *both* to the
// crate root, publishing a second path to everything behind `tokens::theme`.
pub use tokens::theme::theme;

/// Make beUI this app's starting point.
///
/// Two process-global pushes, both public `frust` seams:
///
/// 1. `frust::set_default_theme(`[`theme()`](fn@theme)`)` — the *base* a shell
///    seeds itself with instead of its built-in `Theme::neutral()` fallback.
///    Deliberately not `set_app_theme`: a seeded default does not pin
///    brightness, so a beUI app still follows system dark mode (which is exactly
///    what beUI's own `.dark` class variant does on the web).
/// 2. `frust::register_app_fonts` for both bundled faces, so the type scale's
///    Geist stack and [`mono_family`](tokens::mono_family)'s Geist Mono stack
///    actually resolve. The faces are compiled in unconditionally (see the crate
///    docs' *Fonts* section).
///
/// # Timing: must run before the first frame
///
/// A shell reads the default-theme slot and drains the font registry **once, at
/// construction**, before its first rebuild. A call after that takes effect only
/// on a later `clear_app_theme`-driven reseed, which may never happen — so a
/// late call silently does nothing visible.
///
/// The supported way to get the timing right on all three platforms is
/// `frust::app!`'s setup block, which runs immediately before the root
/// component's `Component::init` and therefore before any shell construction —
/// see the crate docs for the full example.
///
/// # Thread contract and repeat calls
///
/// Both underlying seams are plain `Mutex`-guarded process-globals callable from
/// any thread. Calling `install` twice is harmless but wasteful: the second
/// `set_default_theme` replaces an identical value, and the font bytes are
/// pushed (and later re-registered, shadowing the same family names) a second
/// time. Call it once.
pub fn install() {
    frust::set_default_theme(theme());
    for bytes in font_data() {
        frust::register_app_fonts(bytes.to_vec());
    }
}

#[cfg(test)]
mod tests {
    use frust::{Brightness, DesignLanguage, NativeTypefaces};

    /// The flat crate-root surface a consumer names: the theme constructor, the
    /// extension type and its parts, the vendored tables, the font bytes, and
    /// the `style`/`tokens` modules. A rename that breaks any of these breaks
    /// every downstream call site, so the surface is pinned here rather than
    /// only reachable through a component.
    #[test]
    fn the_crate_root_exposes_the_token_and_style_surface() {
        let theme = crate::theme();
        assert_eq!(
            theme.design_language,
            DesignLanguage::Custom(crate::BEUI_DESIGN_LANGUAGE)
        );
        assert!(theme.extension::<crate::BeuiTokens>().is_some());
        assert!(theme.extension::<NativeTypefaces>().is_some());

        // Reachable by their flat names.
        let _: crate::BeuiPalette = crate::palette(Brightness::Dark);
        let _: crate::BeuiGlass = crate::BEUI_LIGHT.glass;
        let _: crate::BeuiGradient = crate::BEUI_DARK.gradient_accent;
        assert_eq!(crate::BeuiTokens::beui().ring_light, crate::BEUI_LIGHT.ring);
        assert_eq!(crate::FALLBACK_RING, crate::BEUI_LIGHT.ring);
        assert_eq!(crate::font_data().len(), 2);

        // ...and the module paths the components consume read-only.
        assert_eq!(crate::style::HEIGHT_MD, 40.0);
        assert!(crate::style::RADIUS_CONTROL.is_infinite());
        assert_eq!(crate::tokens::SPRING_PRESS.stiffness, 500.0);
        assert_eq!(crate::tokens::shape_scale(), theme.shape);
    }

    /// The theme is genuinely beUI's, not the framework's neutral floor showing
    /// through — the one check that would catch an `install` wired to the wrong
    /// constructor.
    #[test]
    fn the_seeded_theme_is_not_the_neutral_floor() {
        let beui = crate::theme();
        let floor = frust::Theme::neutral();
        assert_ne!(beui.light, floor.light);
        assert_ne!(beui.dark, floor.dark);
        assert_ne!(beui.shape, floor.shape);
        assert_ne!(beui.glass, floor.glass, "beUI has real glass chrome");
        assert_ne!(beui.design_language, floor.design_language);
    }

    /// [`crate::install`] documents a repeat call as *harmless but wasteful*;
    /// this pins the harmless half. Both seams it pushes are process-globals, so
    /// the call is deliberately the only thing this test does with them — it
    /// reads nothing back (the facade exposes no default-theme read), it just
    /// proves the pair of pushes and their repeat are panic-free.
    #[test]
    fn install_is_safe_to_call_twice() {
        crate::install();
        crate::install();
    }
}
