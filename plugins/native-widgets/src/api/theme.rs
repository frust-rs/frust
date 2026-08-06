//! Theme ladder L2: platform-neutral token
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
//! This crate's approximation policy: this folds a *representative* subset
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
//! | `surface_bg` | `scheme().surface` | `Label`/`ProgressBar` background (explicit — see *Explicit backgrounds* below for why `Switch`/`Slider` are deliberately excluded) |
//! | `corner_radius_dp` | `shape.small` | `Button` background (via a `GradientDrawable`) |
//! | `button_text_size_sp` | `type_scale.label_large.size` | `Button` text size |
//! | `body_text_size_sp` | `type_scale.body_large.size` | `Label` text size |
//! | `dark` | `brightness == Brightness::Dark` | every control (L1's `Context` qualification) |
//! | `button_typeface` | `design_language == Glyph` ⇒ Space Mono, else the platform's own | `Button` `Typeface` (theme ladder L3) |
//! | `body_typeface` | `design_language == Glyph` ⇒ IBM Plex Mono, else the platform's own | `Label`/`Switch` `Typeface` (theme ladder L3) |
//!
//! # Theme ladder L3: typography, gated on `design_language`
//!
//! Unlike every other row above (folded unconditionally from whichever
//! `Theme` is active), the two typeface rows first check
//! [`Theme::design_language`]: only the Glyph baseline ships bundled fonts
//! (`frust-theme`'s `glyph-fonts` feature), so a Material3/Cupertino theme
//! resolves both to [`typeface::Typeface::System`] — imposing Glyph's
//! monospace faces on a theme that never asked for them would be a worse
//! regression than leaving the platform's own face alone. `Button` gets
//! Space Mono (Glyph's bolder display face); `Label`/`Switch` share IBM Plex
//! Mono (Glyph's body face) — `Switch` never actually shows text through
//! this plugin today, but it's still a `TextView` subclass under the hood
//! (`android.widget.Switch extends CompoundButton extends Button extends
//! TextView`), so setting it costs nothing and future-proofs against a later
//! on/off-text builder. Same "pin, not full fidelity" policy as every other
//! row above — a future task can widen this to per-slot `TypeScale` family
//! resolution if a control ever needs Glyph's display face specifically.
//!
//! `Image` folds only `dark` — tinting an app-supplied photo from the theme
//! would corrupt its content, and no builder method exposes an explicit tint
//! yet (`crate::api::builders`' own "left for a future task" note).
//!
//! # Explicit backgrounds: closing the light-theme dark-on-dark defect
//! without defeating the ripple
//!
//! L1's night-qualified `Context` ([`crate::android::theme`]) only resolves a
//! control's platform-default background/chrome at CREATE time — Android
//! bakes brightness at construction, unlike iOS which re-pins
//! `overrideUserInterfaceStyle` live on every `update` (a deliberate
//! per-platform asymmetry, `docs/CODE_STANDARDS.md`'s Plugin Conventions).
//! So only an EXPLICIT L2 setter re-applies live on a theme flip (the
//! ordinary Props-diff-then-setter path every property already rides);
//! `Button` already has one (`corner_radius_dp` row above, via
//! `Setter::ThemedBackground`).
//!
//! Folding `surface_bg` into every themed control unconditionally is wrong,
//! though: `android.widget.Switch` (`Widget.Material.CompoundButton.Switch`)
//! and `AbsSeekBar` (`Slider`'s superclass) both carry
//! `?attr/selectableItemBackgroundBorderless` as their platform-default
//! background, which paints the Material touch ripple — a flat
//! `View.setBackgroundColor(int)` (`Setter::BackgroundColor`) REPLACES that
//! drawable outright, silently killing the ripple. So only the controls with
//! nothing to lose get an explicit fold:
//!
//! ## Which controls get an explicit background, and why
//!
//! | Control | Explicit background? | Why |
//! |---|---|---|
//! | `Label` | **Yes** | The only control with visible text, and a `TextView`'s platform-default background is `null` (no ripple, nothing to lose). |
//! | `ProgressBar` | **Yes** | Never clickable, so no ripple to defeat; kept so it reads as part of the page rather than a floating rectangle, matching `examples/glyph-catalog`'s root `AppBackground` fill. |
//! | `Switch` | **No** | `?attr/selectableItemBackgroundBorderless` is its default background — an explicit fill would replace the ripple. Its `thumb_tint`/`track_tint` fold already carries the theme. |
//! | `Slider` | **No** | Same reasoning — `AbsSeekBar` carries the same borderless-ripple background attr. Its `progress_tint`/`thumb_tint` fold already carries the theme. |
//! | `Button` | N/A (already covered) | Gets `Setter::ThemedBackground` (accent-filled) instead — a deliberately opaque fill, not the page's `surface_bg` role. |
//! | `Image` | **No** | A tint or background fill would corrupt app-supplied photo content. |
//!
//! **If a future task wants `Switch`/`Slider` to carry a themed background
//! anyway**, the only acceptable route is a ripple-preserving
//! `RippleDrawable`/`LayerDrawable` setter layering a themed fill UNDER the
//! platform's own ripple foreground — a new [`crate::controls::Setter`]
//! variant, needing its own on-device pass. Do not re-add a bare
//! `Setter::BackgroundColor` fold for either control.
//!
//! `surface_bg` is the same role `examples/glyph-catalog`'s own root
//! `AppBackground` layer paints as the page's base fill, so a native
//! control's background matches the page it sits on rather than floating as
//! a mismatched rectangle; for `Label` specifically it also guarantees
//! contrast with `body_text` (`ColorScheme`'s surface/on_surface pairing is
//! authored to contrast by construction, both brightnesses — see this
//! module's own
//! `text_bearing_controls_pair_a_contrasting_background_and_foreground_in_both_brightnesses`
//! test, covering `Label`/`Button` only). `Image` is excluded for the same
//! tint-corruption reason as above.
//!
//! This folds through the same `Setter::BackgroundColor` (`Tier::Relayout`)
//! every other flat, non-`Button` background already would — no new
//! [`crate::controls::Setter`] variant needed.
//!
//! # Accent-role split and explicit-override precedence
//!
//! This module resolves both `Theme` precedence rules from
//! `docs/CODE_STANDARDS.md`'s Theming conventions. **Accent-role split**:
//! `primary`/`on_primary` is the accent's TEXT/ICON ink; `primary_container`/
//! `on_primary_container` is the bright FILL — a filled control's background
//! is therefore `primary_container` (never bare `primary`), and a small
//! accented part on an otherwise neutral surface (`Switch`/`Slider`'s
//! thumb) is `primary` (never `primary_container`); conflating the two is
//! the catalog's most common accent bug, and this module is the one place
//! this plugin resolves either role. **Precedence**: *explicit builder value
//! > theme > fallback* — no builder method sets a competing explicit
//! colour/radius/size yet, so this module folds theme tokens
//! unconditionally; a future `.text_color()`/`.background_color()` override
//! must thread its value through [`resolve`]'s callers ahead of the theme,
//! not into this module.

use frust::{Brightness, Color, DesignLanguage, Theme};

use crate::controls::typeface::Typeface;

/// Whether `theme`'s active brightness is [`Brightness::Dark`] — L1's input
/// (theme ladder): [`crate::android::theme::night_qualified_context`]
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
    /// `scheme().surface` — the page-background role a native control's
    /// EXPLICIT background resolves to (module doc's *Explicit backgrounds*
    /// section): only `Label`/
    /// `ProgressBar`'s background, so it re-paints live on a brightness flip
    /// instead of pinning to L1's creation-time `Context` the way an unset
    /// (platform default) background would. Deliberately NOT folded into
    /// `Switch`/`Slider` (module doc's *Which controls get an explicit
    /// background* table) — both carry a ripple-painting platform-default
    /// background (`?attr/selectableItemBackgroundBorderless`) an explicit
    /// fill would replace. Not used by `Button` (already has its own
    /// accent-filled `accent_fill` background) or `Image` (module doc's
    /// no-tint rule).
    pub(crate) surface_bg: u32,
    /// `shape.small`, dp.
    pub(crate) corner_radius_dp: f32,
    /// `type_scale.label_large.size`, sp.
    pub(crate) button_text_size_sp: f32,
    /// `type_scale.body_large.size`, sp.
    pub(crate) body_text_size_sp: f32,
    /// `Button`'s `Typeface` (theme ladder L3) — see the module doc's
    /// *typography, gated on `design_language`* section.
    pub(crate) button_typeface: Typeface,
    /// `Label`/`Switch`'s `Typeface` (theme ladder L3) — see the
    /// module doc's *typography, gated on `design_language`* section.
    pub(crate) body_typeface: Typeface,
}

/// Resolve `theme` into the packed primitives every builder folds into its
/// control's `params_json` — see the module doc's mapping table.
pub(crate) fn resolve(theme: &Theme) -> ResolvedTheme {
    publish_glyph_font_bytes();
    let scheme = theme.scheme();
    let is_glyph = theme.design_language == DesignLanguage::Glyph;
    ResolvedTheme {
        dark: is_dark(theme),
        accent_ink: argb_u32(scheme.primary),
        accent_fill: argb_u32(scheme.primary_container),
        on_accent_fill: argb_u32(scheme.on_primary_container),
        body_text: argb_u32(scheme.on_surface),
        surface_bg: argb_u32(scheme.surface),
        corner_radius_dp: theme.shape.small as f32,
        button_text_size_sp: theme.type_scale.label_large.size,
        body_text_size_sp: theme.type_scale.body_large.size,
        button_typeface: if is_glyph {
            Typeface::GlyphMono
        } else {
            Typeface::System
        },
        body_typeface: if is_glyph {
            Typeface::GlyphPlex
        } else {
            Typeface::System
        },
    }
}

/// Publish `frust-theme`'s embedded Glyph font bytes to the Android backend,
/// once per process (theme ladder L3) — the api→runtime seam
/// `crate::android::fonts`'s module doc describes: this crate's `Cargo.toml`
/// allows a `frust-theme` dependency only behind this crate's own
/// `frust-api` feature, and only this function ever names it, so the
/// platform half (`crate::android::fonts`) stays free of it regardless of
/// platform or feature state.
///
/// `frust_theme::glyph::font_data()` always exists — an empty slice with
/// `frust-theme`'s own `glyph-fonts` feature off
/// (`crates/frust-theme/src/glyph/mod.rs`'s own doc) — so this quietly does
/// nothing on that configuration rather than panicking on an out-of-bounds
/// index. Indices 0/3 are a documented coupling to `frust-theme`'s own
/// (private) `font_data()` array literal — Space Mono ×3
/// (Regular/Bold/Italic) then IBM Plex Mono ×4
/// (Regular/Medium/SemiBold/Italic), each family's first entry being its
/// Regular face; no public API names a face by weight, so a future reorder
/// there would silently pick a different (but still valid) face, never a
/// panic or crash.
#[cfg(target_os = "android")]
fn publish_glyph_font_bytes() {
    static PUBLISHED: std::sync::Once = std::sync::Once::new();
    PUBLISHED.call_once(|| {
        let faces = frust_theme::glyph::font_data();
        if let (Some(&mono), Some(&plex)) = (faces.first(), faces.get(3)) {
            crate::android::fonts::set_glyph_bytes(mono, plex);
        }
    });
}

/// iOS half of the same publish (theme ladder L3) —
/// `crate::apple::fonts::set_glyph_bytes` mirrors the Android call above
/// exactly (same indices into `frust_theme::glyph::font_data()`, same
/// idempotent-publish contract).
#[cfg(target_os = "ios")]
fn publish_glyph_font_bytes() {
    static PUBLISHED: std::sync::Once = std::sync::Once::new();
    PUBLISHED.call_once(|| {
        let faces = frust_theme::glyph::font_data();
        if let (Some(&mono), Some(&plex)) = (faces.first(), faces.get(3)) {
            crate::apple::fonts::set_glyph_bytes(mono, plex);
        }
    });
}

/// No other platform backend reads the published bytes at all (desktop
/// preview, wasm) — a no-op here rather than a platform-module reference
/// neither configuration can compile.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn publish_glyph_font_bytes() {}

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
        assert_eq!(tokens.surface_bg, 0xFF16_1A23, "surface (bg-surface, dark)");
        assert_eq!(tokens.corner_radius_dp, 6.0, "shape.small (Glyph)");
        assert_eq!(tokens.button_text_size_sp, 12.5, "type_scale.label_large");
        assert_eq!(tokens.body_text_size_sp, 13.0, "type_scale.body_large");
        assert_eq!(tokens.button_typeface, Typeface::GlyphMono);
        assert_eq!(tokens.body_typeface, Typeface::GlyphPlex);
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
        assert_eq!(
            tokens.surface_bg, 0xFFFF_FFFF,
            "surface (bg-surface, light — the lightest slot)"
        );
        // Shape/type scales don't vary by brightness.
        assert_eq!(tokens.corner_radius_dp, 6.0);
        assert_eq!(tokens.button_text_size_sp, 12.5);
        assert_eq!(tokens.body_text_size_sp, 13.0);
        // Neither does the typeface choice — `design_language`, not
        // `brightness`, gates it (module doc's *typography* section).
        assert_eq!(tokens.button_typeface, Typeface::GlyphMono);
        assert_eq!(tokens.body_typeface, Typeface::GlyphPlex);
    }

    #[test]
    fn non_glyph_baselines_resolve_the_system_typeface() {
        // Imposing Glyph's bundled monospace faces on a Material3/Cupertino
        // theme that never asked for them would be a worse regression than
        // leaving the platform's own face alone (module doc's *typography,
        // gated on `design_language`* section).
        let m3 = resolve(&Theme::m3_baseline());
        assert_eq!(m3.button_typeface, Typeface::System);
        assert_eq!(m3.body_typeface, Typeface::System);

        let cupertino = resolve(&Theme::cupertino_baseline());
        assert_eq!(cupertino.button_typeface, Typeface::System);
        assert_eq!(cupertino.body_typeface, Typeface::System);
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

    // --- The light-theme dark-on-dark regression guard ---------------------

    /// WCAG 2.x relative luminance, `[0.0, 1.0]`, of a packed ARGB colour —
    /// test-only, used solely to compute [`contrast_ratio`] below.
    fn relative_luminance(argb: u32) -> f64 {
        let [_a, r, g, b] = argb.to_be_bytes();
        let channel = |raw: u8| {
            let c = f64::from(raw) / 255.0;
            if c <= 0.039_28 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
    }

    /// WCAG contrast ratio between two packed ARGB colours (`>= 1.0`; `1.0`
    /// is identical colours, no contrast at all) — the standard
    /// `(lighter + 0.05) / (darker + 0.05)` formula, order-independent.
    fn contrast_ratio(a: u32, b: u32) -> f64 {
        let (la, lb) = (relative_luminance(a), relative_luminance(b));
        let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
        (hi + 0.05) / (lo + 0.05)
    }

    #[test]
    fn text_bearing_controls_pair_a_contrasting_background_and_foreground_in_both_brightnesses() {
        // An earlier device gate found flipping to a light theme left `Label`'s
        // background pinned dark (L1, baked at creation) while its text
        // colour (L2) followed the theme live, landing dark text on a dark
        // background — unreadable. A bare `!=` check (the test style
        // used elsewhere in this crate) would NOT catch this: two colours can
        // differ and still both be dark. A WCAG contrast-ratio floor would —
        // this is that test, for every control this crate actually renders
        // visible text on (`Button`, `Label`), across BOTH brightnesses.
        const MIN_CONTRAST: f64 = 4.5; // WCAG AA, normal text

        for brightness in [Brightness::Dark, Brightness::Light] {
            let theme = Theme::glyph_baseline().with_brightness(brightness);
            let t = resolve(&theme);

            let label = contrast_ratio(t.body_text, t.surface_bg);
            assert!(
                label >= MIN_CONTRAST,
                "{brightness:?}: Label's body_text {:#010x} over surface_bg \
                 {:#010x} contrasts only {label:.2}:1 (need >= {MIN_CONTRAST}:1) \
                 — exactly the dark-on-dark defect VERIFY-P1.md bar 3 found",
                t.body_text,
                t.surface_bg
            );

            // Button already had an explicit background
            // (`Setter::ThemedBackground`) — pinned here so a future change
            // can't silently regress the one control this bug never hit.
            let button = contrast_ratio(t.on_accent_fill, t.accent_fill);
            assert!(
                button >= MIN_CONTRAST,
                "{brightness:?}: Button's on_accent_fill {:#010x} over \
                 accent_fill {:#010x} contrasts only {button:.2}:1 (need >= \
                 {MIN_CONTRAST}:1)",
                t.on_accent_fill,
                t.accent_fill
            );
        }
    }
}
