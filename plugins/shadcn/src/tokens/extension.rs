//! [`ShadcnTokens`]: the typed `ThemeExtensions` payload carrying every shadcn
//! token that has no `ColorScheme` role to live in.
//!
//! Four groups qualify, and they qualify for two different reasons:
//!
//! - **`--ring`** — a focus-ring color. `ColorScheme` has no focus-ring role at
//!   all, and the nearest-looking candidate (`surface_tint`) is spoken for: every
//!   design system in the tree, frust's own neutral floor included, keeps M3's
//!   `surface_tint == primary` convention, so writing a grey ring into it would
//!   make `surface_tint` mean two different things depending on the theme. The
//!   ring rides here instead, resolved through [`ShadcnTokens::resolve_ring`].
//! - **`--chart-1..5`** — a five-slot categorical data palette. No role, and no
//!   ambition to invent one.
//! - **`--sidebar-*`** — eight tokens for a component (shadcn's collapsible
//!   sidebar) that is itself a recorded follow-on; the tokens ship now so the
//!   component needs no token work later.
//! - **The 7-step radius scale** — `ShapeScale`'s ten slots already carry the
//!   same numbers ([`shape_scale`](super::theme::shape_scale)), but they carry
//!   them under M3 names. A component porting `rounded-md`/`rounded-xl` from a
//!   shadcn class list wants the shadcn *names*, so the scale rides here too,
//!   in its own vocabulary. This is the one deliberate duplication in the token
//!   layer, and it is a naming convenience, not a second source of truth: the
//!   two are pinned equal by test.
//!
//! Everything here is per-brightness where the source differs (ring, chart,
//! sidebar) and brightness-invariant where it does not (radius) — a shadcn theme
//! carries **both** brightnesses' values at once, exactly like `Theme.light`/
//! `Theme.dark`, because a shell may flip brightness at any time without
//! rebuilding the theme.
//!
//! # Resolution precedence
//!
//! Consumers resolve through the framework's documented ladder — **explicit
//! builder value > theme extension > fallback constant** — via
//! [`ShadcnTokens::resolve_ring`]/[`ShadcnTokens::resolve_radius`], the two
//! places that ladder is written down for this catalog (the `SampleAccents`
//! precedent in `examples/design-system-sample`). A theme with the extension
//! cleared — an app is free to build one — falls through to the fallback rather
//! than panicking or painting nothing.

use frust::{Brightness, Color, Theme};

use super::palette::{ShadcnBase, ShadcnSidebar};

/// shadcn's single authored radius, `--radius: 0.625rem` at the CSS default
/// 16px root font size = **10 logical px**.
///
/// Source: the `neutral` preset's `radius` key (`apps/v4/registry/themes.ts`) —
/// identical in all seven base presets.
pub const RADIUS_BASE: f64 = 10.0;

/// The unthemed fallback ring color: the `neutral` preset's light-mode `--ring`
/// (`oklch(0.708 0 0)`).
///
/// The bottom rung of [`ShadcnTokens::resolve_ring`]'s ladder — used when no
/// theme is threaded into the pass *and* no explicit value was given.
pub const FALLBACK_RING: Color = Color::from_rgb8(0xA1, 0xA1, 0xA1);

/// shadcn's derived radius scale, in logical px.
///
/// Source: `apps/v4/app/globals.css`'s `@theme inline` block (retrieved
/// 2026-08-17) derives every step from the single `--radius` by a multiplier —
/// `sm ×0.6`, `md ×0.8`, `lg ×1.0`, `xl ×1.4`, `2xl ×1.8`, `3xl ×2.2`,
/// `4xl ×2.6` — which is why this is a *derived* scale rather than seven
/// authored numbers, and why [`ShadcnRadius::scaled`] can regenerate it for a
/// different base radius.
///
/// Note these are shadcn's own overrides of the Tailwind default radius scale,
/// not the Tailwind defaults (which are `0.125rem`/`0.25rem`/…).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadcnRadius {
    /// `rounded-sm` — `--radius × 0.6`.
    pub sm: f64,
    /// `rounded-md` — `--radius × 0.8`. The catalog's workhorse radius (buttons,
    /// inputs, popovers).
    pub md: f64,
    /// `rounded-lg` — `--radius` itself.
    pub lg: f64,
    /// `rounded-xl` — `--radius × 1.4`. Cards.
    pub xl: f64,
    /// `rounded-2xl` — `--radius × 1.8`. Named `xl2` because a Rust identifier
    /// cannot start with a digit.
    pub xl2: f64,
    /// `rounded-3xl` — `--radius × 2.2`.
    pub xl3: f64,
    /// `rounded-4xl` — `--radius × 2.6`.
    pub xl4: f64,
}

impl ShadcnRadius {
    /// shadcn's shipped scale: the seven steps derived from
    /// [`RADIUS_BASE`] (10px) — 6 / 8 / 10 / 14 / 18 / 22 / 26.
    pub const fn shadcn() -> Self {
        Self {
            sm: 6.0,   // 10 × 0.6
            md: 8.0,   // 10 × 0.8
            lg: 10.0,  // 10 × 1.0
            xl: 14.0,  // 10 × 1.4
            xl2: 18.0, // 10 × 1.8
            xl3: 22.0, // 10 × 2.2
            xl4: 26.0, // 10 × 2.6
        }
    }

    /// The same scale re-derived from a different base radius — the escape hatch
    /// for an app that wants shadcn's *proportions* at a sharper or rounder base
    /// (upstream's own knob is exactly this: one `--radius` value).
    pub fn scaled(base: f64) -> Self {
        Self {
            sm: base * 0.6,
            md: base * 0.8,
            lg: base,
            xl: base * 1.4,
            xl2: base * 1.8,
            xl3: base * 2.2,
            xl4: base * 2.6,
        }
    }
}

/// The shadcn theme extension: ring, charts, sidebar, radius scale.
///
/// Attached by [`theme_for`](super::theme::theme_for) (and therefore by every
/// `theme*()` constructor); recovered by a widget with
/// `theme.extension::<ShadcnTokens>()`. See the [module docs](self) for why each
/// group is here and how resolution is ordered.
///
/// A `Theme` is cloned across the framework's two delivery paths, so an
/// extension must be `Any + Send + Sync` — this is plain `Copy` data and
/// satisfies that for free.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadcnTokens {
    /// `--ring`, light mode.
    pub ring_light: Color,
    /// `--ring`, dark mode.
    pub ring_dark: Color,
    /// `--chart-1..5`, light mode, in order.
    pub chart_light: [Color; 5],
    /// `--chart-1..5`, dark mode, in order.
    pub chart_dark: [Color; 5],
    /// The `--sidebar-*` group, light mode.
    pub sidebar_light: ShadcnSidebar,
    /// The `--sidebar-*` group, dark mode.
    pub sidebar_dark: ShadcnSidebar,
    /// The derived radius scale (brightness-invariant — the source authors one
    /// `--radius` for both).
    pub radius: ShadcnRadius,
}

impl ShadcnTokens {
    /// The extension for the `neutral` base preset — what
    /// [`theme()`](fn@super::theme::theme) attaches.
    pub const fn shadcn() -> Self {
        Self::for_base(ShadcnBase::Neutral)
    }

    /// The extension for any base preset.
    pub const fn for_base(base: ShadcnBase) -> Self {
        let light = base.light();
        let dark = base.dark();
        Self {
            ring_light: light.ring,
            ring_dark: dark.ring,
            chart_light: light.chart,
            chart_dark: dark.chart,
            sidebar_light: light.sidebar,
            sidebar_dark: dark.sidebar,
            radius: ShadcnRadius::shadcn(),
        }
    }

    /// The ring color for `brightness`.
    pub fn ring(&self, brightness: Brightness) -> Color {
        match brightness {
            Brightness::Light => self.ring_light,
            Brightness::Dark => self.ring_dark,
        }
    }

    /// The five chart colors for `brightness`, in `chart-1..5` order.
    pub fn chart(&self, brightness: Brightness) -> [Color; 5] {
        match brightness {
            Brightness::Light => self.chart_light,
            Brightness::Dark => self.chart_dark,
        }
    }

    /// The sidebar token group for `brightness`.
    pub fn sidebar(&self, brightness: Brightness) -> ShadcnSidebar {
        match brightness {
            Brightness::Light => self.sidebar_light,
            Brightness::Dark => self.sidebar_dark,
        }
    }

    /// Resolve a focus-ring color under the framework's documented precedence:
    /// **explicit builder value > theme extension > fallback constant**.
    ///
    /// This and [`resolve_radius`](Self::resolve_radius) are the only places the
    /// ladder is spelled out for this catalog; every component that paints a
    /// focus ring calls through [`crate::style::draw_focus_ring`], which calls
    /// through here, so two components can never disagree about precedence.
    ///
    /// The theme rung reads the extension **and** the theme's own brightness, so
    /// a ring follows a live light/dark flip with no component involvement.
    pub fn resolve_ring(explicit: Option<Color>, theme: Option<&Theme>) -> Color {
        if let Some(color) = explicit {
            return color;
        }
        match theme.and_then(|t| t.extension::<ShadcnTokens>().map(|x| (x, t.brightness))) {
            Some((tokens, brightness)) => tokens.ring(brightness),
            None => FALLBACK_RING,
        }
    }

    /// Resolve the radius scale under the same ladder as
    /// [`resolve_ring`](Self::resolve_ring): **explicit > theme extension >
    /// [`ShadcnRadius::shadcn`]**.
    ///
    /// Brightness plays no part — the source authors one `--radius` for both.
    pub fn resolve_radius(explicit: Option<ShadcnRadius>, theme: Option<&Theme>) -> ShadcnRadius {
        if let Some(radius) = explicit {
            return radius;
        }
        match theme.and_then(|t| t.extension::<ShadcnTokens>()) {
            Some(tokens) => tokens.radius,
            None => ShadcnRadius::shadcn(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::theme::theme;
    use super::*;

    #[test]
    fn radius_scale_is_the_documented_multiplier_ladder() {
        let r = ShadcnRadius::shadcn();
        assert_eq!(r.lg, RADIUS_BASE);
        assert_eq!(r.sm, RADIUS_BASE * 0.6);
        assert_eq!(r.md, RADIUS_BASE * 0.8);
        assert_eq!(r.xl, RADIUS_BASE * 1.4);
        assert_eq!(r.xl2, RADIUS_BASE * 1.8);
        assert_eq!(r.xl3, RADIUS_BASE * 2.2);
        assert_eq!(r.xl4, RADIUS_BASE * 2.6);
        // Monotonic, so a component picking a "bigger" step always gets one.
        let steps = [r.sm, r.md, r.lg, r.xl, r.xl2, r.xl3, r.xl4];
        assert!(steps.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn scaled_reproduces_the_shipped_scale_at_the_shipped_base() {
        assert_eq!(ShadcnRadius::scaled(RADIUS_BASE), ShadcnRadius::shadcn());
        // And genuinely rescales at another base.
        assert_eq!(ShadcnRadius::scaled(5.0).lg, 5.0);
        assert_eq!(ShadcnRadius::scaled(5.0).md, 4.0);
    }

    #[test]
    fn per_brightness_getters_select_the_matching_table() {
        let tokens = ShadcnTokens::shadcn();
        let (light, dark) = (ShadcnBase::Neutral.light(), ShadcnBase::Neutral.dark());
        assert_eq!(tokens.ring(Brightness::Light), light.ring);
        assert_eq!(tokens.ring(Brightness::Dark), dark.ring);
        assert_eq!(tokens.chart(Brightness::Light), light.chart);
        assert_eq!(tokens.chart(Brightness::Dark), dark.chart);
        assert_eq!(tokens.sidebar(Brightness::Light), light.sidebar);
        assert_eq!(tokens.sidebar(Brightness::Dark), dark.sidebar);
        // The two brightnesses really are different tables.
        assert_ne!(tokens.ring_light, tokens.ring_dark);
    }

    #[test]
    fn for_base_tracks_the_named_preset() {
        for base in ShadcnBase::ALL {
            let tokens = ShadcnTokens::for_base(base);
            assert_eq!(tokens.ring_light, base.light().ring, "{}", base.id());
            assert_eq!(tokens.sidebar_dark, base.dark().sidebar, "{}", base.id());
            // Radius is base-independent.
            assert_eq!(tokens.radius, ShadcnRadius::shadcn());
        }
        assert_eq!(
            ShadcnTokens::shadcn(),
            ShadcnTokens::for_base(ShadcnBase::Neutral)
        );
    }

    #[test]
    fn ring_precedence_is_explicit_then_extension_then_fallback() {
        let explicit = Color::from_rgb8(0x00, 0x00, 0xFF);
        let t = theme();

        // 1. explicit wins over everything.
        assert_eq!(
            ShadcnTokens::resolve_ring(Some(explicit), Some(&t)),
            explicit
        );
        // 2. no explicit value: the theme extension wins over the fallback, at
        //    the theme's own brightness.
        assert_eq!(
            ShadcnTokens::resolve_ring(None, Some(&t)),
            ShadcnTokens::shadcn().ring(t.brightness)
        );
        // 3. no theme at all: the fallback constant.
        assert_eq!(ShadcnTokens::resolve_ring(None, None), FALLBACK_RING);
    }

    #[test]
    fn ring_follows_the_theme_s_brightness() {
        let light = theme().with_brightness(Brightness::Light);
        let dark = theme().with_brightness(Brightness::Dark);
        assert_eq!(
            ShadcnTokens::resolve_ring(None, Some(&light)),
            ShadcnTokens::shadcn().ring_light
        );
        assert_eq!(
            ShadcnTokens::resolve_ring(None, Some(&dark)),
            ShadcnTokens::shadcn().ring_dark
        );
    }

    #[test]
    fn radius_precedence_is_explicit_then_extension_then_fallback() {
        let explicit = ShadcnRadius::scaled(2.0);
        let t = theme();
        assert_eq!(
            ShadcnTokens::resolve_radius(Some(explicit), Some(&t)),
            explicit
        );
        assert_eq!(
            ShadcnTokens::resolve_radius(None, Some(&t)),
            ShadcnRadius::shadcn()
        );
        assert_eq!(
            ShadcnTokens::resolve_radius(None, None),
            ShadcnRadius::shadcn()
        );
    }

    #[test]
    fn a_theme_with_the_extension_cleared_falls_through_to_the_fallbacks() {
        // An app is free to build a theme carrying no ShadcnTokens; both
        // resolvers must degrade, not panic.
        let bare = Theme::neutral();
        assert!(bare.extension::<ShadcnTokens>().is_none());
        assert_eq!(
            ShadcnTokens::resolve_ring(None, Some(&bare)),
            FALLBACK_RING,
            "a non-shadcn theme resolves the fallback ring"
        );
        assert_eq!(
            ShadcnTokens::resolve_radius(None, Some(&bare)),
            ShadcnRadius::shadcn()
        );
    }
}
