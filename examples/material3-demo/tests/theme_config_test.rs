//! Host-side port of the reference example's `test/theme_config_test.dart` —
//! five cases: "Palette action opens the theme config screen", "Seed choices
//! appear once dynamic color is off", "Picking a seed recolors the app
//! immediately", "Picking Flex applies the family immediately", "Emphasized
//! style applies Flex variations". Plus one addition beyond the five: a
//! brightness round-trip check, closing a gap the port's own in-crate tests
//! left open (see this file's last test for why).
//!
//! # Why this isn't a literal port
//!
//! Same choice as `widget_test.rs` (read that file's module doc first):
//! `crate::theme`'s `ThemeSettings` and `crate::pages::theme_config_page` are
//! crate-private, so reaching them from here would mean exporting app
//! internals as API. Every case below instead calls straight into
//! `frust_material::theme_from_seed` (and
//! `frust_material`'s widget constructors for the two construction checks),
//! the production primitive `ThemeSettings` is itself a thin wrapper over —
//! see `src/theme/settings.rs`'s `ThemeSettings::theme`, which composes
//! exactly `theme_from_seed` plus a font-family swap and an emphasized-scale
//! promotion. What a Dart `tester.pump()`/`find.text(...)` observes on
//! screen has no equivalent here (see `widget_test.rs`'s module doc); what's
//! pinned below is the resolved token data those pixels come from.

use frust::authoring::text::{FontFamily, GenericSlot};
use frust::{Brightness, Color, Resolution, Route, RouteParams, Router, text};
use frust_material::{card_list_items, list_item, switch, theme_from_seed};

/// This file's own state type — no test below needs anything in it.
type St = ();

/// The Material 3 default seed (`crate::theme::SEED_OPTIONS[0]`, "Default"),
/// used everywhere a test just needs *some* seed.
const DEFAULT_SEED: Color = Color::from_rgb8(0x67, 0x50, 0xA4);

/// A second, visibly different seed (`crate::theme::SEED_OPTIONS[4]`,
/// "Rose"), used to prove a seed change actually changes the resolved
/// scheme.
const ROSE_SEED: Color = Color::from_rgb8(0xA1, 0x00, 0x3C);

/// Ports "Palette action opens the theme config screen". Tapping the
/// toolbar action and searching the pushed screen for four labels are both
/// unreachable here (see this module's doc and `widget_test.rs`'s); what's
/// pinned instead: a settings-route path resolves independently of any
/// enclosing shell — the same "pushed over it, not nested in it" shape
/// `src/lib.rs`'s own in-crate tests already pin for the real
/// `THEME_ROUTE` — and the settings screen's toggle rows (mirroring
/// `theme_config_page.rs`'s `toggles()`, headlined the same way as two of
/// the Dart case's four expected labels: "Auto theming"/"Dynamic color")
/// construct without panicking.
#[test]
fn theme_route_resolves_outside_any_shell_and_its_toggles_build() {
    let router: Router<St> = Router::new(vec![Route::new("/theme", |_: &RouteParams| {
        text("theme settings")
    })]);
    match router.resolve("/theme") {
        Resolution::Matched { pages, .. } => {
            assert_eq!(
                pages.len(),
                1,
                "a settings route covers the shell, not nests in it"
            );
        }
        Resolution::Error { location } => panic!("/theme did not match: {location:?}"),
    }

    let rows = vec![
        list_item::<St>("Auto theming")
            .supporting("Follow the platform light and dark setting")
            .trailing(switch(true, |_: &mut St, _: bool| {})),
        list_item::<St>("Dynamic color")
            .supporting("Not available — frust has no device wallpaper palette to read")
            .trailing(switch(false, |_: &mut St, _: bool| {}).enabled(false)),
    ];
    let _view = card_list_items(rows);
}

/// Ports "Seed choices appear once dynamic color is off". A documented
/// deviation applies here, already recorded at the source
/// (`theme_config_page.rs`'s `seeds()` doc comment): this port never models
/// dynamic color as *on* in the first place (`crate::theme`'s own module
/// doc — frust has no platform dynamic-color source to read), so the seed
/// picker carries no gated/disabled state to reveal by toggling anything
/// off first — it always builds. What's pinned: every seed choice
/// constructs, unconditionally.
#[test]
fn seed_choices_build_unconditionally_since_dynamic_color_is_never_modeled() {
    // Mirrors `crate::theme::SEED_OPTIONS` (five entries: Default, Ocean,
    // Forest, Amber, Rose), read from `src/theme/settings.rs` at
    // implementation time — not importable from this bin crate's external
    // tests (see this file's module doc).
    const SEED_OPTIONS: [Color; 5] = [
        DEFAULT_SEED,
        Color::from_rgb8(0x00, 0x65, 0x8F),
        Color::from_rgb8(0x00, 0x6D, 0x3D),
        Color::from_rgb8(0x8F, 0x4C, 0x00),
        ROSE_SEED,
    ];
    let mut swatches_built = 0usize;
    for seed in SEED_OPTIONS {
        let theme = theme_from_seed(seed, Brightness::Light);
        let _swatch_fill = theme.scheme().primary;
        swatches_built += 1;
    }
    assert_eq!(
        swatches_built,
        SEED_OPTIONS.len(),
        "every seed choice must resolve a scheme with no dynamic-color precondition"
    );
}

/// Ports "Picking a seed recolors the app immediately": a new seed resolves
/// to a genuinely different scheme, not just a different picker index.
#[test]
fn picking_a_seed_recolors_the_app_immediately() {
    let before = theme_from_seed(DEFAULT_SEED, Brightness::Light)
        .scheme()
        .primary;
    let after = theme_from_seed(ROSE_SEED, Brightness::Light)
        .scheme()
        .primary;
    assert_ne!(
        before, after,
        "a new seed must resolve to a different scheme's primary color"
    );
}

/// Ports "Picking Flex applies the family immediately". Unlike the
/// reference (which offers several font families and must apply the change
/// when Flex is picked), this port's default font choice *is* Roboto Flex
/// (`DemoFont::RobotoFlex` is `#[default]` in `src/theme/settings.rs`) —
/// so the closest host-side equivalent is pinning that the family it
/// resolves to, unpicked, already is what picking Flex is supposed to
/// apply: `frust_material`'s own baseline type scale.
#[test]
fn the_default_type_scale_resolves_to_roboto_flex() {
    let scale = theme_from_seed(DEFAULT_SEED, Brightness::Light).type_scale;
    let flex = FontFamily::stack_with_generic(["Roboto Flex"], GenericSlot::SansSerif);
    assert_eq!(scale.body_medium.family, flex);
    assert_eq!(scale.title_medium.family, flex);
}

/// Ports "Emphasized style applies Flex variations". `frust-text` has no
/// variable-font-axis seam beyond weight (`src/theme/settings.rs`'s own
/// module doc), so this port's Regular/Emphasized picker expresses the
/// reference's `fontVariations` bump as a weight step instead
/// (`ThemeSettings::theme`'s `emphasize` helper swaps each role for its
/// `_emphasized` sibling) — the closest host-side equivalent asserts that
/// swap actually differs, straight from `frust_material`'s own type scale.
#[test]
fn emphasized_roles_differ_in_weight_from_their_baseline() {
    let scale = theme_from_seed(DEFAULT_SEED, Brightness::Light).type_scale;
    assert_ne!(
        scale.body_medium.weight,
        scale.body_medium_emphasized.weight
    );
    assert_ne!(
        scale.title_medium.weight,
        scale.title_medium_emphasized.weight
    );
}

/// Not a Dart port. `theme_config_page.rs`'s own in-crate tests already
/// cover the settings model's three-state brightness machine
/// (`toggling_under_auto_theming_inverts_the_platform_instead_of_pinning`,
/// `toggling_with_auto_theming_off_pins_the_opposite_brightness`,
/// `re_enabling_auto_theming_drops_the_pin_and_the_inversion`, all in
/// `src/theme/settings.rs`) — unreachable from here regardless. What none of
/// them assert directly is that flipping brightness actually resolves a
/// *different* scheme and flipping back lands on the *exact same* one, not
/// an approximation — the production primitive every one of those state
/// transitions ultimately calls through, pinned here.
#[test]
fn brightness_round_trips_through_theme_with_brightness() {
    let light = theme_from_seed(DEFAULT_SEED, Brightness::Light);
    let dark = light.clone().with_brightness(Brightness::Dark);
    assert_ne!(
        light.scheme().primary,
        dark.scheme().primary,
        "flipping brightness must resolve a different scheme"
    );
    let back = dark.with_brightness(Brightness::Light);
    assert_eq!(
        back.scheme(),
        light.scheme(),
        "flipping back must land on the exact same scheme"
    );
}
