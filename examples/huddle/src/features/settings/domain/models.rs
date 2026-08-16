//! `settings` domain — the design-language + brightness selection model plus
//! the [`compose`] decision function (moved verbatim from the former flat
//! `features::settings` module).
//!
//! # Domain → `frust` exception
//!
//! This file imports `frust::{Brightness, DesignLanguage, Theme, TypeScale}`
//! directly — the first domain file in the crate to mention `frust::` at
//! all, allowlisted by the crate's architecture-conformance test. These are
//! plain value types [`compose`] composes (not view/widget/reactive
//! vocabulary), so the import stays within the domain layer's data-only
//! contract; `SetTheme`/the notification ops stay infallible ops rather than
//! being wrapped in new ceremony to route around the import ban. See
//! [`super::use_cases::set_theme`] for the companion (and sharper)
//! exception: the single call site of `set_app_theme`/`clear_app_theme`.

use frust::{Brightness, DesignLanguage, Theme, TypeScale};

use super::accent::AccentChoice;

/// The smallest dynamic-type multiplier the type-scale slider can reach.
pub const TYPE_SCALE_MIN: f32 = 0.85;
/// The largest dynamic-type multiplier the type-scale slider can reach.
pub const TYPE_SCALE_MAX: f32 = 1.30;
/// The unscaled (×1.0) dynamic-type multiplier — the fresh-entry seed and the
/// value that keeps the baseline type scale byte-identical.
pub const TYPE_SCALE_DEFAULT: f32 = 1.0;

/// Map a type-scale multiplier to a `0.0..=1.0` slider position.
pub fn type_factor_to_slider(factor: f32) -> f64 {
    (((factor - TYPE_SCALE_MIN) / (TYPE_SCALE_MAX - TYPE_SCALE_MIN)) as f64).clamp(0.0, 1.0)
}

/// Map a `0.0..=1.0` slider position back to a type-scale multiplier.
pub fn slider_to_type_factor(value: f64) -> f32 {
    TYPE_SCALE_MIN + (value.clamp(0.0, 1.0) as f32) * (TYPE_SCALE_MAX - TYPE_SCALE_MIN)
}

/// Multiply every [`TypeScale`] role's font size by `factor` in place — the
/// dynamic-type transform composed onto the active baseline's type scale before
/// `set_app_theme`. Only the `size` knob is scaled uniformly across every role
/// (the M3 spec's line-height / letter-spacing tokens are left as authored).
pub fn scale_type_scale(type_scale: &mut TypeScale, factor: f32) {
    macro_rules! scale_roles {
        ($ts:ident, $factor:ident, $($role:ident),+ $(,)?) => {
            $( $ts.$role.size *= $factor; )+
        };
    }
    scale_roles!(
        type_scale,
        factor,
        display_large,
        display_medium,
        display_small,
        headline_large,
        headline_medium,
        headline_small,
        title_large,
        title_medium,
        title_small,
        body_large,
        body_medium,
        body_small,
        label_large,
        label_medium,
        label_small,
        display_large_emphasized,
        display_medium_emphasized,
        display_small_emphasized,
        headline_large_emphasized,
        headline_medium_emphasized,
        headline_small_emphasized,
        title_large_emphasized,
        title_medium_emphasized,
        title_small_emphasized,
        body_large_emphasized,
        body_medium_emphasized,
        body_small_emphasized,
        label_large_emphasized,
        label_medium_emphasized,
        label_small_emphasized,
    );
}

/// The design-language selection: follow the platform (`System`) or force one
/// of the three baselines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DesignChoice {
    /// Follow the platform's own Material-vs-Cupertino default (paired with
    /// `System` brightness, this clears the override entirely).
    #[default]
    System,
    /// Force the Glyph (terminal-native) baseline app-wide.
    Glyph,
    /// Force the Material 3 baseline app-wide.
    Material3,
    /// Force the Cupertino (iOS) baseline app-wide.
    Cupertino,
}

impl DesignChoice {
    /// Every choice, in selector order.
    pub const ALL: [DesignChoice; 4] = [
        DesignChoice::System,
        DesignChoice::Glyph,
        DesignChoice::Material3,
        DesignChoice::Cupertino,
    ];

    /// The selector label.
    pub fn label(self) -> &'static str {
        match self {
            DesignChoice::System => "System",
            DesignChoice::Glyph => "Glyph",
            DesignChoice::Material3 => "Material 3",
            DesignChoice::Cupertino => "Cupertino",
        }
    }

    /// This choice's index in [`DesignChoice::ALL`] (a `button_group`'s
    /// `selected` position).
    pub fn index(self) -> usize {
        DesignChoice::ALL
            .iter()
            .position(|c| *c == self)
            .expect("self is always a member of ALL")
    }

    /// The choice at selector index `idx` (falls back to the default on an
    /// out-of-range index).
    pub fn from_index(idx: usize) -> Self {
        DesignChoice::ALL.get(idx).copied().unwrap_or_default()
    }

    /// The concrete choice matching an active theme's [`DesignLanguage`] — used
    /// to seed the selector from the ambient theme on screen (re)entry. Never
    /// returns `System`: the override slot cannot report whether it is cleared,
    /// so a re-entered screen shows the live concrete language rather than
    /// `System` (see the module docs' navigation-survival note).
    pub fn from_design_language(lang: DesignLanguage) -> Self {
        match lang {
            DesignLanguage::Glyph => DesignChoice::Glyph,
            DesignLanguage::Material3 => DesignChoice::Material3,
            DesignLanguage::Cupertino => DesignChoice::Cupertino,
            _ => {
                // external design systems (DesignLanguage::Custom) fall back to Material chrome here
                DesignChoice::Material3
            }
        }
    }
}

/// The brightness selection: follow the platform (`System`) or force one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BrightnessChoice {
    /// Follow the platform's own light/dark preference.
    #[default]
    System,
    /// Force the light color scheme.
    Light,
    /// Force the dark color scheme.
    Dark,
}

impl BrightnessChoice {
    /// The selector label.
    pub fn label(self) -> &'static str {
        match self {
            BrightnessChoice::System => "System",
            BrightnessChoice::Light => "Light",
            BrightnessChoice::Dark => "Dark",
        }
    }

    /// The concrete choice matching an active theme's [`Brightness`] — used to
    /// seed the selector on (re)entry. Never returns `System` (same reason as
    /// [`DesignChoice::from_design_language`]).
    pub fn from_brightness(brightness: Brightness) -> Self {
        match brightness {
            Brightness::Light => BrightnessChoice::Light,
            Brightness::Dark => BrightnessChoice::Dark,
        }
    }
}

/// What [`SetTheme`](super::use_cases::SetTheme) resolved the two choices
/// into — the app-global action to take. Returned as the use case's `Output`
/// so the application step is observable (and the composition is
/// unit-testable via [`compose`] without touching the process-global
/// override).
#[derive(Clone, Debug, PartialEq)]
pub enum ThemeDecision {
    /// Clear the override — follow the platform's own design + light/dark
    /// default (`clear_app_theme`).
    Clear,
    /// Force this exact theme app-wide (`set_app_theme`). Boxed because a
    /// [`Theme`] is large (~4KB) and `Clear` carries nothing (clippy
    /// `large_enum_variant`).
    Apply(Box<Theme>),
}

/// Pure composition of the two selections against the currently-active
/// `current` theme.
///
/// - Both axes `System` → [`ThemeDecision::Clear`] (true platform following).
/// - Otherwise → [`ThemeDecision::Apply`] of a baseline (resolved from the
///   design choice, or from `current`'s live language when the design axis is
///   `System`) carrying the resolved brightness (from the brightness choice, or
///   `current`'s live brightness when that axis is `System`).
///
/// Resolving a `System` axis against `current` is what lets "force Dark while
/// keeping the platform's Material/Cupertino default" work without discarding
/// the other axis — the `with_brightness` footgun `examples/catalog` documents.
/// Extends the two-axis composition with the theming-engine axes: the
/// [`AccentChoice`] palette re-tint and the dynamic-type `type_factor`.
///
/// - Clear (follow the platform outright) only when *every* axis is at its
///   default: both design + brightness `System`, [`AccentChoice::Default`], and
///   a ×1.0 `type_factor`. Any non-default axis forces an [`ThemeDecision::Apply`].
/// - On `Apply`, the resolved baseline is re-tinted by [`super::accent::apply`]
///   and its type scale multiplied by `type_factor` ([`scale_type_scale`]) —
///   the two new axes compose *onto* the design+brightness result, so changing
///   one leaves the others intact (the independence the tests assert).
pub fn compose(
    design: DesignChoice,
    brightness: BrightnessChoice,
    accent: AccentChoice,
    type_factor: f32,
    current: &Theme,
) -> ThemeDecision {
    let unity_type = (type_factor - TYPE_SCALE_DEFAULT).abs() < f32::EPSILON;
    if design == DesignChoice::System
        && brightness == BrightnessChoice::System
        && accent == AccentChoice::Default
        && unity_type
    {
        return ThemeDecision::Clear;
    }

    let base = match design {
        DesignChoice::Glyph => Theme::glyph_baseline(),
        DesignChoice::Material3 => Theme::m3_baseline(),
        DesignChoice::Cupertino => Theme::cupertino_baseline(),
        DesignChoice::System => match current.design_language {
            DesignLanguage::Glyph => Theme::glyph_baseline(),
            DesignLanguage::Material3 => Theme::m3_baseline(),
            DesignLanguage::Cupertino => Theme::cupertino_baseline(),
            _ => {
                // external design systems (DesignLanguage::Custom) fall back to Material chrome here
                Theme::m3_baseline()
            }
        },
    };

    let resolved = match brightness {
        BrightnessChoice::Light => Brightness::Light,
        BrightnessChoice::Dark => Brightness::Dark,
        BrightnessChoice::System => current.brightness,
    };

    let mut theme = base.with_brightness(resolved);
    super::accent::apply(&mut theme, accent);
    scale_type_scale(&mut theme.type_scale, type_factor);
    ThemeDecision::Apply(Box::new(theme))
}

/// Parameters for [`SetTheme`](super::use_cases::SetTheme): the four
/// selections plus the currently-active theme (so a `System` axis can
/// resolve against the live value).
#[derive(Clone, Debug)]
pub struct SetThemeParams {
    pub design: DesignChoice,
    pub brightness: BrightnessChoice,
    /// The accent palette re-tint.
    pub accent: AccentChoice,
    /// The dynamic-type multiplier.
    pub type_factor: f32,
    /// The ambient theme active at the moment the change was requested.
    pub current: Theme,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compose with the two theming-engine axes at their defaults — the shape
    /// the original two-axis tests exercise.
    fn compose2(
        design: DesignChoice,
        brightness: BrightnessChoice,
        current: &Theme,
    ) -> ThemeDecision {
        compose(
            design,
            brightness,
            AccentChoice::Default,
            TYPE_SCALE_DEFAULT,
            current,
        )
    }

    /// Unwrap an [`ThemeDecision::Apply`] or fail the test.
    fn applied(decision: ThemeDecision) -> Box<Theme> {
        match decision {
            ThemeDecision::Apply(theme) => theme,
            ThemeDecision::Clear => panic!("expected an applied theme, got Clear"),
        }
    }

    #[test]
    fn all_default_axes_clear_the_override() {
        let current = Theme::m3_baseline();
        assert_eq!(
            compose2(DesignChoice::System, BrightnessChoice::System, &current),
            ThemeDecision::Clear,
        );
    }

    #[test]
    fn forcing_a_design_applies_that_baseline() {
        let current = Theme::m3_baseline();
        let theme = applied(compose2(
            DesignChoice::Cupertino,
            BrightnessChoice::System,
            &current,
        ));
        assert_eq!(theme.design_language, DesignLanguage::Cupertino);
        // Brightness axis stayed `System` → inherits `current`'s value.
        assert_eq!(theme.brightness, Brightness::Light);
    }

    #[test]
    fn forcing_dark_keeps_the_platform_design_via_current() {
        // Design axis `System`, brightness forced Dark: the design must resolve
        // from `current`'s live language rather than discarding it.
        let current = Theme::cupertino_baseline();
        let theme = applied(compose2(
            DesignChoice::System,
            BrightnessChoice::Dark,
            &current,
        ));
        assert_eq!(theme.design_language, DesignLanguage::Cupertino);
        assert_eq!(theme.brightness, Brightness::Dark);
    }

    #[test]
    fn forcing_material_dark_is_fully_explicit() {
        let current = Theme::cupertino_baseline().with_brightness(Brightness::Light);
        let theme = applied(compose2(
            DesignChoice::Material3,
            BrightnessChoice::Dark,
            &current,
        ));
        assert_eq!(theme.design_language, DesignLanguage::Material3);
        assert_eq!(theme.brightness, Brightness::Dark);
    }

    #[test]
    fn forcing_glyph_applies_the_glyph_baseline() {
        let current = Theme::m3_baseline();
        let theme = applied(compose2(
            DesignChoice::Glyph,
            BrightnessChoice::System,
            &current,
        ));
        assert_eq!(theme.design_language, DesignLanguage::Glyph);
        // Brightness axis stayed `System` → inherits `current`'s (light) value,
        // not Glyph's own dark-first default.
        assert_eq!(theme.brightness, Brightness::Light);
    }

    #[test]
    fn system_design_resolves_glyph_from_the_ambient_theme() {
        // Design axis `System` against a live Glyph theme must re-resolve the
        // Glyph baseline, not silently fall back to Material3.
        let current = Theme::glyph_baseline();
        let theme = applied(compose2(
            DesignChoice::System,
            BrightnessChoice::Dark,
            &current,
        ));
        assert_eq!(theme.design_language, DesignLanguage::Glyph);
        assert_eq!(theme.brightness, Brightness::Dark);
    }

    #[test]
    fn choice_index_round_trips() {
        for choice in DesignChoice::ALL {
            assert_eq!(DesignChoice::from_index(choice.index()), choice);
        }
    }

    #[test]
    fn from_ambient_never_yields_system() {
        assert_eq!(
            DesignChoice::from_design_language(DesignLanguage::Glyph),
            DesignChoice::Glyph,
        );
        assert_eq!(
            DesignChoice::from_design_language(DesignLanguage::Cupertino),
            DesignChoice::Cupertino,
        );
        assert_eq!(
            BrightnessChoice::from_brightness(Brightness::Dark),
            BrightnessChoice::Dark,
        );
    }

    // --- Theming-engine axes ----------------------------------------------

    #[test]
    fn an_accent_forces_apply_even_with_system_axes() {
        // A non-default accent at otherwise-default axes must NOT clear.
        let current = Theme::m3_baseline();
        let theme = applied(compose(
            DesignChoice::System,
            BrightnessChoice::System,
            AccentChoice::ForgeRed,
            TYPE_SCALE_DEFAULT,
            &current,
        ));
        // The expected primary is the accent's own light-mode primary (current
        // is a light M3 theme).
        assert_eq!(
            theme.scheme().primary,
            AccentChoice::ForgeRed.primary(Brightness::Light),
        );
    }

    #[test]
    fn a_type_factor_forces_apply_and_scales_a_role() {
        let current = Theme::m3_baseline();
        let baseline_body = Theme::m3_baseline().type_scale.body_large.size;
        let theme = applied(compose(
            DesignChoice::System,
            BrightnessChoice::System,
            AccentChoice::Default,
            1.30,
            &current,
        ));
        let scaled = theme.type_scale.body_large.size;
        assert!(
            (scaled - baseline_body * 1.30).abs() < 1e-3,
            "body_large scaled by the type factor",
        );
    }

    #[test]
    fn brightness_accent_and_scale_compose_independently() {
        // Change all three at once; each axis lands independently in the result.
        let current = Theme::m3_baseline();
        let theme = applied(compose(
            DesignChoice::System,
            BrightnessChoice::Dark,
            AccentChoice::MossGreen,
            1.15,
            &current,
        ));
        // Brightness axis.
        assert_eq!(theme.brightness, Brightness::Dark);
        // Accent axis (dark table, since the resolved brightness is Dark).
        assert_eq!(
            theme.scheme().primary,
            AccentChoice::MossGreen.primary(Brightness::Dark),
        );
        // Type axis.
        let baseline = Theme::m3_baseline().type_scale.title_large.size;
        assert!((theme.type_scale.title_large.size - baseline * 1.15).abs() < 1e-3);
    }

    #[test]
    fn changing_one_axis_leaves_the_others_at_default() {
        // Accent-only change: brightness stays light, type scale unchanged.
        let current = Theme::m3_baseline();
        let baseline_body = Theme::m3_baseline().type_scale.body_large.size;
        let theme = applied(compose(
            DesignChoice::System,
            BrightnessChoice::System,
            AccentChoice::OceanBlue,
            TYPE_SCALE_DEFAULT,
            &current,
        ));
        assert_eq!(theme.brightness, Brightness::Light);
        assert!((theme.type_scale.body_large.size - baseline_body).abs() < 1e-3);
        assert_eq!(
            theme.scheme().primary,
            AccentChoice::OceanBlue.primary(Brightness::Light),
        );
    }

    #[test]
    fn slider_and_type_factor_round_trip() {
        for factor in [TYPE_SCALE_MIN, 1.0, 1.10, TYPE_SCALE_MAX] {
            let v = type_factor_to_slider(factor);
            let back = slider_to_type_factor(v);
            assert!((back - factor).abs() < 1e-3, "round-trip {factor}");
        }
    }
}
