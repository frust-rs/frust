//! Integration coverage for the theming engine + notification prefs.
//!
//! These drive the public [`huddle::features::settings`] API at the crate
//! boundary — the same `compose` test seam the appearance screen's `SetTheme`
//! use case runs through, plus the accent/type/notification axes.
//! No render harness is needed: the theming decision is a pure function of the
//! four selections against the ambient theme.

use frust::{Brightness, Theme};
use huddle::features::settings::{
    AccentChoice, BrightnessChoice, DesignChoice, NotifFrequency, TYPE_SCALE_DEFAULT,
    TYPE_SCALE_MAX, ThemeDecision, compose, slider_to_type_factor, type_factor_to_slider,
};

/// Unwrap an applied theme or fail.
fn applied(decision: ThemeDecision) -> Box<Theme> {
    match decision {
        ThemeDecision::Apply(theme) => theme,
        ThemeDecision::Clear => panic!("expected an applied theme, got Clear"),
    }
}

#[test]
fn accent_swap_composes_the_expected_primary_into_the_theme() {
    let current = Theme::m3_baseline(); // light M3
    for accent in AccentChoice::SWATCHES {
        let theme = applied(compose(
            DesignChoice::System,
            BrightnessChoice::System,
            accent,
            TYPE_SCALE_DEFAULT,
            &current,
        ));
        assert_eq!(
            theme.scheme().primary,
            accent.primary(Brightness::Light),
            "{} composes its light primary",
            accent.label(),
        );
    }
}

#[test]
fn accent_dark_table_applies_under_forced_dark() {
    let current = Theme::m3_baseline();
    let theme = applied(compose(
        DesignChoice::System,
        BrightnessChoice::Dark,
        AccentChoice::ForgeRed,
        TYPE_SCALE_DEFAULT,
        &current,
    ));
    assert_eq!(theme.brightness, Brightness::Dark);
    assert_eq!(
        theme.scheme().primary,
        AccentChoice::ForgeRed.primary(Brightness::Dark),
    );
}

#[test]
fn type_factor_scales_a_known_role_size() {
    let current = Theme::m3_baseline();
    let baseline = Theme::m3_baseline().type_scale.headline_small.size;
    for factor in [TYPE_SCALE_MAX, 1.15_f32] {
        let theme = applied(compose(
            DesignChoice::System,
            BrightnessChoice::System,
            AccentChoice::Default,
            factor,
            &current,
        ));
        let scaled = theme.type_scale.headline_small.size;
        assert!(
            (scaled - baseline * factor).abs() < 1e-3,
            "headline_small scaled by {factor}",
        );
    }
}

#[test]
fn default_axes_and_unity_type_clear_the_override() {
    let current = Theme::m3_baseline();
    assert_eq!(
        compose(
            DesignChoice::System,
            BrightnessChoice::System,
            AccentChoice::Default,
            TYPE_SCALE_DEFAULT,
            &current,
        ),
        ThemeDecision::Clear,
    );
}

#[test]
fn the_three_axes_compose_independently() {
    // Change brightness, accent, and type all at once; each lands independently.
    let current = Theme::m3_baseline();
    let base_title = Theme::m3_baseline().type_scale.title_large.size;
    let theme = applied(compose(
        DesignChoice::System,
        BrightnessChoice::Dark,
        AccentChoice::OceanBlue,
        1.20,
        &current,
    ));
    assert_eq!(theme.brightness, Brightness::Dark, "brightness axis");
    assert_eq!(
        theme.scheme().primary,
        AccentChoice::OceanBlue.primary(Brightness::Dark),
        "accent axis",
    );
    assert!(
        (theme.type_scale.title_large.size - base_title * 1.20).abs() < 1e-3,
        "type axis",
    );
}

#[test]
fn changing_the_accent_leaves_brightness_and_type_at_default() {
    let current = Theme::m3_baseline();
    let base_body = Theme::m3_baseline().type_scale.body_large.size;
    let theme = applied(compose(
        DesignChoice::System,
        BrightnessChoice::System,
        AccentChoice::MossGreen,
        TYPE_SCALE_DEFAULT,
        &current,
    ));
    assert_eq!(theme.brightness, Brightness::Light, "brightness untouched");
    assert!(
        (theme.type_scale.body_large.size - base_body).abs() < 1e-3,
        "type scale untouched",
    );
}

#[test]
fn notification_frequency_round_trips() {
    for choice in NotifFrequency::ALL {
        assert_eq!(NotifFrequency::from_index(choice.index()), choice);
    }
}

#[test]
fn type_factor_slider_round_trips() {
    for factor in [0.85_f32, 1.0, 1.30] {
        let back = slider_to_type_factor(type_factor_to_slider(factor));
        assert!((back - factor).abs() < 1e-3, "round-trip {factor}");
    }
}
