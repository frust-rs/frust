//! Settings feature domain (wave-2 task 06) — the design-language +
//! brightness selection spine.
//!
//! [`SettingsController`] owns the two selection signals ([`DesignChoice`] and
//! [`BrightnessChoice`]) and drives the [`SetTheme`] use case, which is the
//! **one and only** place the whole app forces its active [`Theme`] via
//! `forgekit::set_app_theme`/`clear_app_theme`. Every other screen just reads
//! the ambient theme (`use_context::<Theme>()` / `PaintCtx::theme_as`), so a
//! change here live-swaps all four tabs at once and, because the override is a
//! process-global (`docs/ARCHITECTURE.md`'s Theme delivery), survives navigation.
//!
//! The controller is hosted inside [`crate::screens::settings`]'s `Component`
//! via `clean_signals_forgekit::use_controller` — exactly how
//! [`TeamController`](crate::features::team::presentation) reaches its own
//! screen (task 02's registration seam). The *effect* is app-global (the theme
//! override is process-wide); the controller instance itself is component-scoped
//! and disposed on teardown.
//!
//! [`SetTheme`] mirrors
//! [`UpdateMember`](crate::features::team::domain::use_cases::UpdateMember)'s
//! shape — a `clean_signals::UseCase` — but is a *synchronous* use case: its
//! `execute` never awaits (no repository, no I/O) and never fails. It reuses the
//! app's single [`TeamFailure`] enum as its `Failure` associated type purely to
//! satisfy the trait bound; the `Ok` path is the only one it ever takes.

use clean_signals::{ControllerCore, RunOptions, UseCase};
use forgekit::{
    Brightness, DesignLanguage, GetUntracked, RwSignal, Theme, clear_app_theme, set_app_theme,
};

use crate::failure::TeamFailure;

/// The design-language selection: follow the platform (`System`) or force one
/// of the two baselines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DesignChoice {
    /// Follow the platform's own Material-vs-Cupertino default (paired with
    /// `System` brightness, this clears the override entirely).
    #[default]
    System,
    /// Force the Material 3 baseline app-wide.
    Material3,
    /// Force the Cupertino (iOS) baseline app-wide.
    Cupertino,
}

impl DesignChoice {
    /// Every choice, in selector order.
    pub const ALL: [DesignChoice; 3] = [
        DesignChoice::System,
        DesignChoice::Material3,
        DesignChoice::Cupertino,
    ];

    /// The selector label.
    pub fn label(self) -> &'static str {
        match self {
            DesignChoice::System => "System",
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
            DesignLanguage::Material3 => DesignChoice::Material3,
            DesignLanguage::Cupertino => DesignChoice::Cupertino,
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

/// What [`SetTheme`] resolved the two choices into — the app-global action to
/// take. Returned as the use case's `Output` so the application step is
/// observable (and the composition is unit-testable via [`compose`] without
/// touching the process-global override).
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
pub fn compose(
    design: DesignChoice,
    brightness: BrightnessChoice,
    current: &Theme,
) -> ThemeDecision {
    if design == DesignChoice::System && brightness == BrightnessChoice::System {
        return ThemeDecision::Clear;
    }

    let base = match design {
        DesignChoice::Material3 => Theme::m3_baseline(),
        DesignChoice::Cupertino => Theme::cupertino_baseline(),
        DesignChoice::System => match current.design_language {
            DesignLanguage::Material3 => Theme::m3_baseline(),
            DesignLanguage::Cupertino => Theme::cupertino_baseline(),
        },
    };

    let resolved = match brightness {
        BrightnessChoice::Light => Brightness::Light,
        BrightnessChoice::Dark => Brightness::Dark,
        BrightnessChoice::System => current.brightness,
    };

    ThemeDecision::Apply(Box::new(base.with_brightness(resolved)))
}

/// Parameters for [`SetTheme`]: the two selections plus the currently-active
/// theme (so a `System` axis can resolve against the live value).
#[derive(Clone, Debug)]
pub struct SetThemeParams {
    pub design: DesignChoice,
    pub brightness: BrightnessChoice,
    /// The ambient theme active at the moment the change was requested.
    pub current: Theme,
}

/// The synchronous theme-application use case (mirrors `UpdateMember`'s shape).
///
/// [`compose`]s the decision, then applies it — the single call site of
/// `set_app_theme`/`clear_app_theme` in the whole app. Never awaits, never
/// fails.
pub struct SetTheme;

#[cfg_attr(not(target_arch = "wasm32"), clean_signals::async_trait)]
#[cfg_attr(target_arch = "wasm32", clean_signals::async_trait(?Send))]
impl UseCase for SetTheme {
    type Params = SetThemeParams;
    type Output = ThemeDecision;
    type Failure = TeamFailure;

    async fn execute(&self, params: SetThemeParams) -> Result<ThemeDecision, TeamFailure> {
        let decision = compose(params.design, params.brightness, &params.current);
        match &decision {
            ThemeDecision::Clear => clear_app_theme(),
            ThemeDecision::Apply(theme) => set_app_theme((**theme).clone()),
        }
        Ok(decision)
    }
}

/// View model for the settings screen.
///
/// Holds the two selection signals the screen's selectors are controlled by,
/// embeds a [`ControllerCore`] by composition (the `templates/AGENTS.md`
/// controller rule, same as [`TeamController`](crate::features::team::presentation)),
/// and runs [`SetTheme`] through it.
pub struct SettingsController {
    core: ControllerCore<TeamFailure>,
    set_theme: SetTheme,
    /// The selected design language (drives the language selector).
    pub design: RwSignal<DesignChoice>,
    /// The selected brightness (drives the dark/light switch label).
    pub brightness: RwSignal<BrightnessChoice>,
}

impl SettingsController {
    /// Seed the selectors from the ambient theme (via
    /// [`DesignChoice::from_design_language`]/[`BrightnessChoice::from_brightness`])
    /// so a re-entered screen reflects the live theme.
    pub fn new(design: DesignChoice, brightness: BrightnessChoice) -> Self {
        Self {
            core: ControllerCore::new(),
            set_theme: SetTheme,
            design: RwSignal::new(design),
            brightness: RwSignal::new(brightness),
        }
    }

    /// Apply the current selections app-wide. `current` is the ambient theme the
    /// screen read this frame (used to resolve a `System` axis). Routed through
    /// [`ControllerCore::run`] so the clean-architecture spine (use case →
    /// controller → screen) stays visible, exactly like `TeamController::rename`.
    pub async fn apply(&self, current: Theme) {
        let params = SetThemeParams {
            design: self.design.get_untracked(),
            brightness: self.brightness.get_untracked(),
            current,
        };
        // Infallible by construction; the `Result` is ignored (there is no
        // failure path to surface, unlike the roster's rename).
        let _ = self
            .core
            .run(&self.set_theme, params, RunOptions::default())
            .await;
    }
}

impl AsRef<ControllerCore<TeamFailure>> for SettingsController {
    fn as_ref(&self) -> &ControllerCore<TeamFailure> {
        &self.core
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_system_axes_clear_the_override() {
        let current = Theme::m3_baseline();
        assert_eq!(
            compose(DesignChoice::System, BrightnessChoice::System, &current),
            ThemeDecision::Clear,
        );
    }

    #[test]
    fn forcing_a_design_applies_that_baseline() {
        let current = Theme::m3_baseline();
        let decision = compose(DesignChoice::Cupertino, BrightnessChoice::System, &current);
        match decision {
            ThemeDecision::Apply(theme) => {
                assert_eq!(theme.design_language, DesignLanguage::Cupertino);
                // Brightness axis stayed `System` → inherits `current`'s value.
                assert_eq!(theme.brightness, Brightness::Light);
            }
            ThemeDecision::Clear => panic!("expected an applied theme, got Clear"),
        }
    }

    #[test]
    fn forcing_dark_keeps_the_platform_design_via_current() {
        // Design axis `System`, brightness forced Dark: the design must resolve
        // from `current`'s live language rather than discarding it.
        let current = Theme::cupertino_baseline();
        let decision = compose(DesignChoice::System, BrightnessChoice::Dark, &current);
        match decision {
            ThemeDecision::Apply(theme) => {
                assert_eq!(theme.design_language, DesignLanguage::Cupertino);
                assert_eq!(theme.brightness, Brightness::Dark);
            }
            ThemeDecision::Clear => panic!("expected an applied theme, got Clear"),
        }
    }

    #[test]
    fn forcing_material_dark_is_fully_explicit() {
        let current = Theme::cupertino_baseline().with_brightness(Brightness::Light);
        let decision = compose(DesignChoice::Material3, BrightnessChoice::Dark, &current);
        match decision {
            ThemeDecision::Apply(theme) => {
                assert_eq!(theme.design_language, DesignLanguage::Material3);
                assert_eq!(theme.brightness, Brightness::Dark);
            }
            ThemeDecision::Clear => panic!("expected an applied theme, got Clear"),
        }
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
            DesignChoice::from_design_language(DesignLanguage::Cupertino),
            DesignChoice::Cupertino,
        );
        assert_eq!(
            BrightnessChoice::from_brightness(Brightness::Dark),
            BrightnessChoice::Dark,
        );
    }
}
