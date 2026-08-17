//! `frust-shadcn`: shadcn/ui, packaged as a design-system plugin.
//!
//! This is the design-system tier's first **external-origin** catalog: shadcn/ui
//! is a third-party web design system, and this crate is a *port* of it — its
//! token tables and component designs are transcribed from the upstream registry
//! (shadcn/ui v4, rev `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved
//! 2026-08-17), not authored here. Every value carries its source; every place
//! the web original has no frust equivalent carries the decision that resolved
//! it.
//!
//! The crate is the whole system: the [`tokens`] module (the seven vendored base
//! presets, the 46-role `ColorScheme` fold, the [`ShadcnTokens`] extension, the
//! bundled Inter + JetBrains Mono faces, the assembled [`theme()`](fn@theme)), the shared
//! [`style`] vocabulary every component paints from, the [`overlay`] hosting seam
//! its panels mount through, and the component catalog itself.
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
//!   `Theme::from_paint_ctx`/`from_layout_ctx` and reads the shadcn token tables
//!   with an unthemed-fallback constant per resolved value, exactly like every
//!   baseline widget. It never matches on `DesignLanguage`; a shadcn theme is
//!   just a `Theme` whose token tables happen to be the shadcn ones (tagged
//!   `DesignLanguage::Custom("shadcn")` for identity, not for branching).
//! - **Desktop-first, mobile-friendly.** shadcn's own metrics are kept
//!   ([`style::HEIGHT_DEFAULT`] is `h-9` = 36px, below the mobile tap-target
//!   convention); touch correctness — press states, scrim taps, scrolling —
//!   is a requirement, a separate mobile design is not, and no density
//!   mechanism is invented.
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
//!         any(text("shadcn"))
//!     }
//! }
//!
//! frust::app!(MyApp, setup = { frust_shadcn::install(); });
//! # fn main() {}
//! ```
//!
//! An app that wants one of the other six base presets seeds it by hand instead:
//! `frust::set_default_theme(frust_shadcn::theme_stone())` plus the same
//! `register_app_fonts` loop [`install`] runs.
//!
//! # Namespace
//!
//! Every component module is re-exported wholesale from this root, so app code
//! names one flat namespace — `frust_shadcn::ButtonVariant`,
//! `frust_shadcn::theme()` — alongside the `frust::*` baseline widgets a shadcn
//! screen composes with. Public component symbols are therefore
//! component-prefixed (`src/components/mod.rs` records that convention); the
//! token/style/overlay modules stay behind their module names.
//!
//! # Fonts
//!
//! Inter Variable and JetBrains Mono Variable are bundled **unconditionally**
//! (`include_bytes!`) — depending on this plugin is itself the opt-in, so there
//! is no second feature to switch them off with. Both are OFL-1.1 with no
//! Reserved Font Name, and ship with their license text and provenance record
//! (`plugins/shadcn/fonts/README.md`). [`install`] registers both faces and the
//! theme binds Inter into the `NativeTypefaces` slots native controls read;
//! [`font_data`] exposes the raw bytes for a host that wants them directly.

mod components;
mod hit;
pub mod overlay;
pub mod style;
mod text;
pub mod tokens;

// The flat catalog namespace: every component module glob-re-exported from the
// crate root.
pub use components::{
    accordion::*, alert::*, alert_dialog::*, aspect_ratio::*, attachment::*, avatar::*, badge::*,
    breadcrumb::*, bubble::*, button::*, button_group::*, card::*, checkbox::*, collapsible::*,
    combobox::*, command::*, context_menu::*, dialog::*, drawer::*, dropdown_menu::*, empty::*,
    field::*, hover_card::*, input::*, input_group::*, item::*, kbd::*, label::*, marker::*,
    native_select::*, pagination::*, popover::*, progress::*, radio_group::*, scroll_area::*,
    select::*, separator::*, sheet::*, skeleton::*, slider::*, spinner::*, switch::*, table::*,
    tabs::*, textarea::*, toggle::*, toggle_group::*, tooltip::*,
};

/// The design language itself, flattened to the root alongside the catalog: the
/// assembled [`theme()`](fn@theme) and its six sibling presets, the [`ShadcnTokens`]
/// extension and its radius scale, the vendored [`ShadcnPalette`] tables, and
/// the bundled [`font_data`]. The per-scale constructors
/// ([`color_scheme`](tokens::color_scheme), [`type_scale`](tokens::type_scale),
/// [`shape_scale`](tokens::shape_scale)) stay behind [`tokens`].
pub use tokens::{
    FALLBACK_RING, RADIUS_BASE, SHADCN_DESIGN_LANGUAGE, ShadcnBase, ShadcnPalette, ShadcnRadius,
    ShadcnSidebar, ShadcnTokens, font_data, theme_for, theme_mauve, theme_mist, theme_olive,
    theme_stone, theme_taupe, theme_zinc,
};
// Named through its own module path rather than folded into the group above: a
// bare `tokens::theme` names both the module and the function (Rust keeps them
// in separate namespaces), and a single-name re-export would lift *both* to the
// crate root, publishing a second path to everything behind `tokens::theme`.
pub use tokens::theme::theme;

/// Make shadcn this app's starting point.
///
/// Two process-global pushes, both public `frust` seams:
///
/// 1. `frust::set_default_theme(`[`theme()`](fn@theme)`)` — the *base* a shell
///    seeds itself with instead of its built-in `Theme::neutral()` fallback.
///    Deliberately not `set_app_theme`: a seeded default does not pin
///    brightness, so a shadcn app still follows system dark mode (which is
///    exactly what shadcn's own `.dark` class variant does on the web).
/// 2. `frust::register_app_fonts` for both bundled faces, so the type scale's
///    Inter stack and [`mono_family`](tokens::mono_family)'s JetBrains Mono
///    stack actually resolve. The faces are compiled in unconditionally (see the
///    crate docs' *Fonts* section).
///
/// The seeded theme is the `neutral` base preset; an app wanting another calls
/// `set_default_theme` itself with one of the `theme_*` constructors.
///
/// # Timing: must run before the first frame
///
/// A shell reads the default-theme slot and drains the font registry **once, at
/// construction**, before its first rebuild. A call after that takes effect only
/// on a later `clear_app_theme`-driven reseed, which may never happen — so a late
/// call silently does nothing visible.
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
/// `set_default_theme` replaces an identical value, and the font bytes are pushed
/// (and later re-registered, shadowing the same family names) a second time. Call
/// it once.
pub fn install() {
    frust::set_default_theme(theme());
    for bytes in font_data() {
        frust::register_app_fonts(bytes.to_vec());
    }
}

#[cfg(test)]
mod tests {
    use frust::{Brightness, DesignLanguage, NativeTypefaces};

    /// The flat crate-root surface a consumer names: the theme constructors, the
    /// extension type and its parts, the vendored tables, the font bytes, and the
    /// `style`/`tokens`/`overlay` modules. A rename that breaks any of these
    /// breaks every downstream call site, so the surface is pinned here rather
    /// than only reachable through a component.
    #[test]
    fn the_crate_root_exposes_the_token_and_style_surface() {
        let theme = crate::theme();
        assert_eq!(
            theme.design_language,
            DesignLanguage::Custom(crate::SHADCN_DESIGN_LANGUAGE)
        );
        assert!(theme.extension::<crate::ShadcnTokens>().is_some());
        assert!(theme.extension::<NativeTypefaces>().is_some());

        // Reachable by their flat names.
        let _: crate::ShadcnPalette = crate::ShadcnBase::Neutral.light();
        let _: crate::ShadcnSidebar = crate::ShadcnBase::Zinc.dark().sidebar;
        let _: crate::ShadcnRadius = crate::ShadcnTokens::shadcn().radius;
        assert_eq!(crate::RADIUS_BASE, theme.shape.medium);
        assert_eq!(crate::font_data().len(), 2);

        // ...and the module paths the components consume read-only.
        assert_eq!(crate::style::HEIGHT_DEFAULT, 36.0);
        assert_eq!(
            crate::style::ring_color(None, Some(&theme)),
            crate::tokens::ShadcnTokens::shadcn().ring(theme.brightness)
        );
        assert_ne!(crate::tokens::theme_stone().light, theme.light);
    }

    #[test]
    fn every_preset_constructor_is_reachable_from_the_root() {
        let presets = [
            crate::theme(),
            crate::theme_stone(),
            crate::theme_zinc(),
            crate::theme_mauve(),
            crate::theme_olive(),
            crate::theme_mist(),
            crate::theme_taupe(),
        ];
        for t in &presets {
            assert_eq!(
                t.design_language,
                DesignLanguage::Custom(crate::SHADCN_DESIGN_LANGUAGE)
            );
        }
        assert_eq!(
            presets.len(),
            crate::ShadcnBase::ALL.len(),
            "one root constructor per base preset"
        );
        for base in crate::ShadcnBase::ALL {
            assert_eq!(
                crate::theme_for(base).light,
                crate::tokens::color_scheme(&base.light(), &base.dark(), Brightness::Light)
            );
        }
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
