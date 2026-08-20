//! The gallery's runtime theme choices, and how they reach the shell.
//!
//! The reference keeps these in a `ChangeNotifier` its root `MaterialApp`
//! listens to; this port keeps them in signals and pushes the rebuilt
//! [`Theme`] through the framework's two theme seams instead:
//!
//! * [`set_default_theme`] always carries the current base (seed + font +
//!   type style). It is what the shell re-seeds itself from, so it must stay
//!   current even while an override is active.
//! * [`set_app_theme`]/[`clear_app_theme`] carry the *brightness decision*: an
//!   override pins light/dark, a clear hands brightness back to the platform.
//!   A `clear_app_theme` call also makes the shell re-read the default slot,
//!   which is what applies a new seed/font while following the platform.
//!
//! # `auto_theming`, ported
//!
//! The reference's controller has three states and this port keeps all three:
//! follow the platform (`auto_theming`, not inverted), follow it inverted
//! (`auto_theming` + `invert_platform`), or pin one brightness outright
//! (`auto_theming` off). The one degrade: an *inverted* auto theme is applied
//! as a pinned override computed from the platform brightness observed when
//! the toggle was pressed, because the framework's default-theme seam has no
//! invert hook — while inverted, a live platform light/dark flip is therefore
//! not re-inverted until the toggle is pressed again (the shell's
//! override-wins-over-appearance rule).
//!
//! # Type styles
//!
//! The reference offers seven Roboto Flex axis presets (Condensed, Wide,
//! Round, ...). `frust-text` exposes no variable-axis seam beyond weight
//! (`docs/LIMITATIONS.md`'s `frust-text-no-variable-font-axes`), so this
//! picker carries the two the type scale can actually express: the baseline
//! scale and its emphasized companion.
//!
//! Dynamic (device-sourced) coloring is not modelled at all: frust has no
//! platform dynamic-color source to read, so there is nothing to toggle.

use frust::authoring::text::{FontFamily, GenericSlot, TextStyle};
use frust::{
    Brightness, Color, Get, RwSignal, Set, Theme, TypeScale, clear_app_theme, set_app_theme,
    set_default_theme,
};
use frust_material::theme_from_seed;

/// The seed colors the gallery offers, each with its picker label — the
/// reference's own `seedOptions`/`seedLabels`, paired. The first is the
/// Material 3 default seed.
pub const SEED_OPTIONS: [(Color, &str); 5] = [
    (Color::from_rgb8(0x67, 0x50, 0xA4), "Default"),
    (Color::from_rgb8(0x00, 0x65, 0x8F), "Ocean"),
    (Color::from_rgb8(0x00, 0x6D, 0x3D), "Forest"),
    (Color::from_rgb8(0x8F, 0x4C, 0x00), "Amber"),
    (Color::from_rgb8(0xA1, 0x00, 0x3C), "Rose"),
];

/// The family name `frust_material::install()` registers Roboto Flex under.
/// A literal because the plugin keeps its own family constants private; it
/// must match what the installer registers or the stack falls through to the
/// generic slot.
const ROBOTO_FLEX_FAMILY: &str = "Roboto Flex";

/// The family name `frust_material::install()` registers Roboto Mono under —
/// same contract as [`ROBOTO_FLEX_FAMILY`].
const ROBOTO_MONO_FAMILY: &str = "Roboto Mono";

/// The gallery's font choices.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum DemoFont {
    /// The catalog's own bundled Roboto Flex — what the Material type scale
    /// resolves to untouched, and the gallery's starting point.
    #[default]
    RobotoFlex,
    /// The catalog's bundled Roboto Mono.
    RobotoMono,
    /// The platform's system UI font (the reference's null `fontFamily`).
    System,
}

impl DemoFont {
    /// Every choice, in picker order.
    pub const ALL: [DemoFont; 3] = [DemoFont::RobotoFlex, DemoFont::RobotoMono, DemoFont::System];

    /// Picker label.
    pub fn label(self) -> &'static str {
        match self {
            DemoFont::RobotoFlex => "Roboto Flex",
            DemoFont::RobotoMono => "Roboto Mono",
            DemoFont::System => "System",
        }
    }

    /// The family stack to put on every type-scale role. The Roboto Flex arm
    /// restates what the Material type scale resolves to on its own, so all
    /// three choices read from one table.
    fn family(self) -> FontFamily {
        match self {
            DemoFont::RobotoFlex => {
                FontFamily::stack_with_generic([ROBOTO_FLEX_FAMILY], GenericSlot::SansSerif)
            }
            DemoFont::RobotoMono => {
                FontFamily::stack_with_generic([ROBOTO_MONO_FAMILY], GenericSlot::Monospace)
            }
            DemoFont::System => FontFamily::SystemUi,
        }
    }
}

/// The gallery's type-style choices — the expressible subset of the
/// reference's seven axis presets (see this module's docs).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum DemoTypeStyle {
    /// The baseline M3 scale.
    #[default]
    Regular,
    /// The M3 Expressive emphasized scale (a weight step-up per role).
    Emphasized,
}

impl DemoTypeStyle {
    /// Every choice, in picker order.
    pub const ALL: [DemoTypeStyle; 2] = [DemoTypeStyle::Regular, DemoTypeStyle::Emphasized];

    /// Picker label.
    pub fn label(self) -> &'static str {
        match self {
            DemoTypeStyle::Regular => "Regular",
            DemoTypeStyle::Emphasized => "Emphasized",
        }
    }
}

/// The gallery's live theme settings.
///
/// `Copy + Send + Sync` (signals and nothing else), so it rides
/// `provide_context` — the reference's `ExampleThemeScope`, which every route
/// reads through.
#[derive(Clone, Copy)]
pub struct ThemeSettings {
    seed: RwSignal<usize>,
    auto_theming: RwSignal<bool>,
    invert_platform: RwSignal<bool>,
    brightness_override: RwSignal<Option<Brightness>>,
    font: RwSignal<DemoFont>,
    type_style: RwSignal<DemoTypeStyle>,
}

impl ThemeSettings {
    /// Fresh settings at the reference's own defaults: the Material seed,
    /// following the platform's brightness, Roboto Flex, regular type.
    pub fn new() -> Self {
        Self {
            seed: RwSignal::new(0),
            auto_theming: RwSignal::new(true),
            invert_platform: RwSignal::new(false),
            brightness_override: RwSignal::new(None),
            font: RwSignal::new(DemoFont::default()),
            type_style: RwSignal::new(DemoTypeStyle::default()),
        }
    }

    /// The selected seed's index into [`SEED_OPTIONS`].
    pub fn seed_index(&self) -> usize {
        self.seed.get()
    }

    /// The selected seed color.
    pub fn seed_color(&self) -> Color {
        SEED_OPTIONS[self.seed_slot()].0
    }

    /// The selected seed's picker label.
    pub fn seed_label(&self) -> &'static str {
        SEED_OPTIONS[self.seed_slot()].1
    }

    /// The selected seed's index, clamped into [`SEED_OPTIONS`].
    fn seed_slot(&self) -> usize {
        self.seed_index().min(SEED_OPTIONS.len() - 1)
    }

    /// Whether the theme follows the platform's brightness.
    pub fn auto_theming(&self) -> bool {
        self.auto_theming.get()
    }

    /// Whether the platform's brightness is being inverted (only meaningful
    /// with [`ThemeSettings::auto_theming`] on).
    pub fn invert_platform(&self) -> bool {
        self.invert_platform.get()
    }

    /// The pinned brightness, if any (only meaningful with
    /// [`ThemeSettings::auto_theming`] off).
    pub fn brightness_override(&self) -> Option<Brightness> {
        self.brightness_override.get()
    }

    /// The selected font.
    pub fn font(&self) -> DemoFont {
        self.font.get()
    }

    /// The selected type style.
    pub fn type_style(&self) -> DemoTypeStyle {
        self.type_style.get()
    }

    /// Choose a seed by its [`SEED_OPTIONS`] index and apply the result.
    pub fn set_seed(&self, index: usize, platform: Brightness) {
        self.seed.set(index.min(SEED_OPTIONS.len() - 1));
        self.apply(platform);
    }

    /// Choose a font and apply the result.
    pub fn set_font(&self, font: DemoFont, platform: Brightness) {
        self.font.set(font);
        self.apply(platform);
    }

    /// Choose a type style and apply the result.
    pub fn set_type_style(&self, style: DemoTypeStyle, platform: Brightness) {
        self.type_style.set(style);
        self.apply(platform);
    }

    /// Turn platform-brightness following on or off. Turning it on drops both
    /// the pin and the inversion (the reference's `followSystem`); turning it
    /// off pins whatever brightness is showing right now.
    pub fn set_auto_theming(&self, enabled: bool, platform: Brightness) {
        self.auto_theming.set(enabled);
        if enabled {
            self.brightness_override.set(None);
            self.invert_platform.set(false);
        } else if self.brightness_override.get().is_none() {
            self.brightness_override.set(Some(platform));
        }
        self.apply(platform);
    }

    /// Flip light/dark — the app bar's brightness action.
    ///
    /// `platform` is the brightness showing right now (read from the ambient
    /// [`Theme`] at the call site), the reference's `fallback`.
    pub fn toggle_brightness(&self, platform: Brightness) {
        if self.auto_theming.get() {
            self.brightness_override.set(None);
            self.invert_platform.set(!self.invert_platform.get());
        } else {
            let effective = self.brightness_override.get().unwrap_or(platform);
            self.brightness_override.set(Some(flip(effective)));
        }
        self.apply(platform);
    }

    /// The brightness to pin, or `None` to let the platform decide.
    pub fn resolved_brightness(&self, platform: Brightness) -> Option<Brightness> {
        if self.auto_theming.get() {
            self.invert_platform.get().then(|| flip(platform))
        } else {
            Some(self.brightness_override.get().unwrap_or(platform))
        }
    }

    /// The theme these settings describe, at `brightness`.
    pub fn theme(&self, brightness: Brightness) -> Theme {
        let mut theme = theme_from_seed(self.seed_color(), brightness);
        if self.type_style.get() == DemoTypeStyle::Emphasized {
            emphasize(&mut theme.type_scale);
        }
        let family = self.font.get().family();
        for style in roles_mut(&mut theme.type_scale) {
            style.family = family.clone();
        }
        theme
    }

    /// Push the current settings to the running shell.
    ///
    /// Called on every change, and once from the root component's `init` (so
    /// the seeded base is in place before the shell reads the slot).
    pub fn apply(&self, platform: Brightness) {
        let resolved = self.resolved_brightness(platform);
        let theme = self.theme(resolved.unwrap_or(platform));
        set_default_theme(theme.clone());
        match resolved {
            Some(brightness) => set_app_theme(theme.with_brightness(brightness)),
            // Also what re-reads the default slot above, so a seed/font change
            // lands while the app follows the platform.
            None => clear_app_theme(),
        }
    }
}

impl Default for ThemeSettings {
    fn default() -> Self {
        Self::new()
    }
}

/// The other brightness.
fn flip(brightness: Brightness) -> Brightness {
    match brightness {
        Brightness::Light => Brightness::Dark,
        Brightness::Dark => Brightness::Light,
    }
}

/// Promote every baseline role to its emphasized counterpart — how this port
/// expresses the reference's app-wide `wght`/`GRAD` axis bump.
fn emphasize(scale: &mut TypeScale) {
    scale.display_large = scale.display_large_emphasized.clone();
    scale.display_medium = scale.display_medium_emphasized.clone();
    scale.display_small = scale.display_small_emphasized.clone();
    scale.headline_large = scale.headline_large_emphasized.clone();
    scale.headline_medium = scale.headline_medium_emphasized.clone();
    scale.headline_small = scale.headline_small_emphasized.clone();
    scale.title_large = scale.title_large_emphasized.clone();
    scale.title_medium = scale.title_medium_emphasized.clone();
    scale.title_small = scale.title_small_emphasized.clone();
    scale.body_large = scale.body_large_emphasized.clone();
    scale.body_medium = scale.body_medium_emphasized.clone();
    scale.body_small = scale.body_small_emphasized.clone();
    scale.label_large = scale.label_large_emphasized.clone();
    scale.label_medium = scale.label_medium_emphasized.clone();
    scale.label_small = scale.label_small_emphasized.clone();
}

/// Every role in a scale, mutably — the one place this crate enumerates all
/// thirty, so a family swap can't silently miss one.
fn roles_mut(scale: &mut TypeScale) -> [&mut TextStyle; 30] {
    [
        &mut scale.display_large,
        &mut scale.display_medium,
        &mut scale.display_small,
        &mut scale.headline_large,
        &mut scale.headline_medium,
        &mut scale.headline_small,
        &mut scale.title_large,
        &mut scale.title_medium,
        &mut scale.title_small,
        &mut scale.body_large,
        &mut scale.body_medium,
        &mut scale.body_small,
        &mut scale.label_large,
        &mut scale.label_medium,
        &mut scale.label_small,
        &mut scale.display_large_emphasized,
        &mut scale.display_medium_emphasized,
        &mut scale.display_small_emphasized,
        &mut scale.headline_large_emphasized,
        &mut scale.headline_medium_emphasized,
        &mut scale.headline_small_emphasized,
        &mut scale.title_large_emphasized,
        &mut scale.title_medium_emphasized,
        &mut scale.title_small_emphasized,
        &mut scale.body_large_emphasized,
        &mut scale.body_medium_emphasized,
        &mut scale.body_small_emphasized,
        &mut scale.label_large_emphasized,
        &mut scale.label_medium_emphasized,
        &mut scale.label_small_emphasized,
    ]
}

#[cfg(test)]
mod tests {
    use frust::Brightness;
    use frust::authoring::text::FontFamily;

    use super::{DemoFont, DemoTypeStyle, SEED_OPTIONS, ThemeSettings, roles_mut};

    #[test]
    fn defaults_follow_the_platform_brightness() {
        let settings = ThemeSettings::new();
        assert!(settings.auto_theming());
        assert_eq!(settings.resolved_brightness(Brightness::Dark), None);
        assert_eq!(settings.seed_color(), SEED_OPTIONS[0].0);
    }

    #[test]
    fn toggling_under_auto_theming_inverts_the_platform_instead_of_pinning() {
        let settings = ThemeSettings::new();
        settings.toggle_brightness(Brightness::Light);
        assert!(settings.invert_platform());
        assert_eq!(
            settings.resolved_brightness(Brightness::Light),
            Some(Brightness::Dark)
        );
        settings.toggle_brightness(Brightness::Dark);
        assert!(!settings.invert_platform());
        assert_eq!(settings.resolved_brightness(Brightness::Light), None);
    }

    #[test]
    fn toggling_with_auto_theming_off_pins_the_opposite_brightness() {
        let settings = ThemeSettings::new();
        settings.set_auto_theming(false, Brightness::Light);
        assert_eq!(settings.brightness_override(), Some(Brightness::Light));
        settings.toggle_brightness(Brightness::Light);
        assert_eq!(settings.brightness_override(), Some(Brightness::Dark));
        assert_eq!(
            settings.resolved_brightness(Brightness::Light),
            Some(Brightness::Dark)
        );
    }

    #[test]
    fn re_enabling_auto_theming_drops_the_pin_and_the_inversion() {
        let settings = ThemeSettings::new();
        settings.set_auto_theming(false, Brightness::Dark);
        settings.set_auto_theming(true, Brightness::Dark);
        assert_eq!(settings.brightness_override(), None);
        assert!(!settings.invert_platform());
    }

    #[test]
    fn a_seed_change_rebuilds_the_scheme() {
        let settings = ThemeSettings::new();
        let before = settings.theme(Brightness::Light).scheme().primary;
        settings.set_seed(2, Brightness::Light);
        let after = settings.theme(Brightness::Light).scheme().primary;
        assert_ne!(before, after, "a new seed must regenerate the scheme");
    }

    #[test]
    fn the_emphasized_style_promotes_every_baseline_role() {
        let settings = ThemeSettings::new();
        let regular = settings.theme(Brightness::Light).type_scale;
        settings.set_type_style(DemoTypeStyle::Emphasized, Brightness::Light);
        let emphasized = settings.theme(Brightness::Light).type_scale;
        assert_eq!(emphasized.body_large, regular.body_large_emphasized);
        assert_eq!(emphasized.title_medium, regular.title_medium_emphasized);
    }

    #[test]
    fn a_font_choice_reaches_every_role() {
        let settings = ThemeSettings::new();
        settings.set_font(DemoFont::System, Brightness::Light);
        let mut scale = settings.theme(Brightness::Light).type_scale;
        for style in roles_mut(&mut scale) {
            assert_eq!(style.family, FontFamily::SystemUi);
        }
    }
}
