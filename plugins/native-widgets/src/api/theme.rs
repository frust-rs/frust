//! Theme ladder L2 (native-widgets Phase 1, p1-07): platform-neutral token
//! resolution. [`resolve`] folds the active [`frust::Theme`] into the packed
//! primitives ([`u32`] ARGB, [`f32`] dp/sp) [`crate::api::builders`] writes
//! into each control's `params_json` and Android's `NativeWidget::update`/
//! `create` setters ([`crate::controls::platform::apply`]) apply, unchanged,
//! via the exact same direct-setter path every other property already rides.
//! See [`crate::android::theme`] for L1 (the night-qualified `Context` this
//! module's `dark` flag drives) — the two live in different modules because
//! this one is host-testable (no `target_os` gate) and that one is JNI-only.
//!
//! # Mapping table (pinning, not full token fidelity)
//!
//! Per PLAN.md's approximation policy: this folds a *representative* subset
//! of `Theme` into each control, not full design-token fidelity (no
//! elevation/motion/glass, no per-state — hover/pressed/disabled — variants
//! the platform's own drawables already provide for free).
//!
//! | Token | `ColorScheme`/`ShapeScale`/`TypeScale` source | Controls |
//! |---|---|---|
//! | `accent_ink` | `scheme().primary` | `Switch`/`Slider` thumb tint |
//! | `accent_fill` | `scheme().primary_container` | `Button` background, `Switch` track tint, `Slider`/`ProgressBar` progress tint |
//! | `on_accent_fill` | `scheme().on_primary_container` | `Button` text colour |
//! | `body_text` | `scheme().on_surface` | `Label` text colour |
//! | `corner_radius_dp` | `shape.small` | `Button` background (via a `GradientDrawable`) |
//! | `button_text_size_sp` | `type_scale.label_large.size` | `Button` text size |
//! | `body_text_size_sp` | `type_scale.body_large.size` | `Label` text size |
//! | `dark` | `brightness == Brightness::Dark` | every control (L1's `Context` qualification) |
//!
//! `Image` folds only `dark` — tinting an app-supplied photo from the theme
//! would corrupt its content, and no builder method exposes an explicit tint
//! yet (`crate::api::builders`' own "left for a future task" note).
//!
//! # Accent-role split (the catalog's most common accent bug —
//! `docs/CODE_STANDARDS.md`'s Theming conventions)
//!
//! `primary`/`on_primary` is the accent's TEXT/ICON ink; `primary_container`/
//! `on_primary_container` is the bright FILL. A filled control's background
//! is therefore `primary_container` (+ `on_primary_container` ink on top of
//! it) — never `primary` — and a control's small accented part sitting on an
//! otherwise neutral surface (`Switch`/`Slider`'s thumb) is `primary` — never
//! `primary_container`. `docs/CODE_STANDARDS.md` names conflating the two as
//! the catalog's most common accent bug; this module is the one place this
//! plugin resolves either role, so getting the split right here is
//! load-bearing for every control that folds a colour token.
//!
//! # No explicit-override precedence yet
//!
//! `docs/CODE_STANDARDS.md`'s Theming conventions rank precedence as
//! *explicit builder value > theme > fallback*. This task folds theme tokens
//! unconditionally (no builder method sets a competing explicit colour/
//! radius/size yet — `crate::api::builders`' p1-06 completion summary
//! deliberately left those out), so there is no explicit value to rank above
//! the theme here; a future task adding `.text_color()`/`.background_color()`
//! overrides must thread an explicit value through [`resolve`]'s callers
//! ahead of the theme, not into this module.

use frust::{Brightness, Color, Theme};

/// Whether `theme`'s active brightness is [`Brightness::Dark`] — L1's input
/// (theme ladder, p1-07): [`crate::android::theme::night_qualified_context`]
/// wraps a control's construction `Context` off this same bit.
pub(crate) fn is_dark(theme: &Theme) -> bool {
    theme.brightness == Brightness::Dark
}

/// Pack a [`Color`] (`frust::Color`, a straight re-export of `peniko::Color`
/// — used here rather than depending on `peniko` directly, since `frust` is
/// already this feature's required dependency) as the ARGB `u32` every
/// native-widgets colour setter already round-trips (`crate::controls::color`'s
/// doc: "the signed colour int Java uses ... and the unsigned u32 ... differ
/// only above the sign bit").
pub(crate) fn argb_u32(color: Color) -> u32 {
    let [r, g, b, a] = color.to_rgba8().to_u8_array();
    u32::from_be_bytes([a, r, g, b])
}

/// Every token a control's `params_for` may fold in, resolved once per
/// [`Component::build`](frust_core::Component::build) from the active
/// [`Theme`] — see the module doc's mapping table for the *why* of each
/// field. `Copy`: cheap to thread through every builder's `params_for` by
/// value, and the exact shape a host test compares for the "unchanged theme
/// yields `PartialEq`-equal Props" acceptance criterion (identical
/// `ResolvedTheme` values fold into byte-identical `params_json`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ResolvedTheme {
    /// L1's input — see [`is_dark`].
    pub(crate) dark: bool,
    /// `scheme().primary` — accent TEXT/ICON ink (module doc's accent-role
    /// split).
    pub(crate) accent_ink: u32,
    /// `scheme().primary_container` — the bright accent FILL.
    pub(crate) accent_fill: u32,
    /// `scheme().on_primary_container` — ink atop [`Self::accent_fill`].
    pub(crate) on_accent_fill: u32,
    /// `scheme().on_surface` — ordinary body-text ink.
    pub(crate) body_text: u32,
    /// `shape.small`, dp.
    pub(crate) corner_radius_dp: f32,
    /// `type_scale.label_large.size`, sp.
    pub(crate) button_text_size_sp: f32,
    /// `type_scale.body_large.size`, sp.
    pub(crate) body_text_size_sp: f32,
}

/// Resolve `theme` into the packed primitives every builder folds into its
/// control's `params_json` — see the module doc's mapping table.
pub(crate) fn resolve(theme: &Theme) -> ResolvedTheme {
    let scheme = theme.scheme();
    ResolvedTheme {
        dark: is_dark(theme),
        accent_ink: argb_u32(scheme.primary),
        accent_fill: argb_u32(scheme.primary_container),
        on_accent_fill: argb_u32(scheme.on_primary_container),
        body_text: argb_u32(scheme.on_surface),
        corner_radius_dp: theme.shape.small as f32,
        button_text_size_sp: theme.type_scale.label_large.size,
        body_text_size_sp: theme.type_scale.body_large.size,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- mapping-table snapshots, both brightnesses (real, source-cited
    // Glyph hex values — not a tautological re-derivation of `resolve`
    // itself) --------------------------------------------------------------

    #[test]
    fn resolve_snapshots_the_glyph_dark_baseline() {
        // Glyph dark (`crates/frust-theme/src/glyph/color.rs`'s
        // `ColorScheme::glyph_dark`): primary == primary_container == the
        // amber fill (dark's accent split collapses text/fill to the same
        // hex), on_primary_container the near-black amber ink, on_surface
        // the warm-white body ink.
        let theme = Theme::glyph_baseline();
        assert_eq!(theme.brightness, Brightness::Dark, "dark-first default");

        let tokens = resolve(&theme);
        assert!(tokens.dark);
        assert_eq!(tokens.accent_ink, 0xFFFF_B627, "primary (accent ink)");
        assert_eq!(tokens.accent_fill, 0xFFFF_B627, "primary_container (fill)");
        assert_eq!(
            tokens.on_accent_fill, 0xFF24_1A04,
            "on_primary_container (ink atop the fill)"
        );
        assert_eq!(tokens.body_text, 0xFFF2_EAD9, "on_surface (body ink)");
        assert_eq!(tokens.corner_radius_dp, 6.0, "shape.small (Glyph)");
        assert_eq!(tokens.button_text_size_sp, 12.5, "type_scale.label_large");
        assert_eq!(tokens.body_text_size_sp, 13.0, "type_scale.body_large");
    }

    #[test]
    fn resolve_snapshots_the_glyph_light_variant() {
        // Glyph light (`ColorScheme::glyph_light`): the accent role split
        // pulls apart here — primary (text ink) darkens for AA on paper
        // while primary_container (fill) stays the same bright amber.
        let theme = Theme::glyph_baseline().with_brightness(Brightness::Light);

        let tokens = resolve(&theme);
        assert!(!tokens.dark);
        assert_eq!(
            tokens.accent_ink, 0xFFA3_650A,
            "primary (accent ink, AA-darkened)"
        );
        assert_eq!(
            tokens.accent_fill, 0xFFFF_B627,
            "primary_container (fill, unchanged from dark)"
        );
        assert_eq!(
            tokens.on_accent_fill, 0xFF2A_1C04,
            "on_primary_container (ink atop the fill)"
        );
        assert_eq!(tokens.body_text, 0xFF22_1D12, "on_surface (body ink)");
        // Shape/type scales don't vary by brightness.
        assert_eq!(tokens.corner_radius_dp, 6.0);
        assert_eq!(tokens.button_text_size_sp, 12.5);
        assert_eq!(tokens.body_text_size_sp, 13.0);
    }

    // --- the zero-FFI property: an unchanged theme resolves identically ---

    #[test]
    fn an_unchanged_theme_resolves_to_partial_eq_equal_tokens() {
        let theme = Theme::glyph_baseline();
        assert_eq!(
            resolve(&theme),
            resolve(&theme),
            "resolving the same Theme value twice must produce PartialEq-equal \
             ResolvedTheme — the whole-struct gate that keeps an unchanged theme \
             off the FFI boundary depends on it"
        );
    }

    #[test]
    fn argb_u32_matches_the_wire_s_established_packing() {
        // Matches `crate::controls::color`'s own round-trip doc/test: the
        // unsigned spelling of opaque black-alpha-full-red is
        // 0xFFFF0000 == 4_294_901_760.
        let red = Color::from_rgb8(0xFF, 0x00, 0x00);
        assert_eq!(argb_u32(red), 0xFFFF_0000);
    }
}
