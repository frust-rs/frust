//! Settings feature domain — the design-language + brightness selection spine.
//!
//! [`SettingsController`] owns the two selection signals ([`DesignChoice`] and
//! [`BrightnessChoice`]) and drives the [`SetTheme`] use case, which is the
//! **one and only** place the whole app forces its active [`Theme`] via
//! `forgekit::set_app_theme`/`clear_app_theme`. Every other screen just reads
//! the ambient theme (`use_context::<Theme>()` / `PaintCtx::theme_as`), so a
//! change here live-swaps all four tabs at once and, because the override is a
//! process-global (`docs/ARCHITECTURE.md`'s Theme delivery), survives navigation.
//!
//! The controller is hosted inside [`crate::screens::settings_appearance`]'s
//! `Component` via `clean_signals_forgekit::use_controller`. The *effect* is
//! app-global (the theme override is process-wide); the controller instance
//! itself is component-scoped and disposed on teardown.
//!
//! [`SetTheme`] is a *synchronous* `clean_signals::UseCase`: its `execute`
//! never awaits (no repository, no I/O) and never fails. It reuses the app's
//! single [`HuddleFailure`] enum as its `Failure` associated type purely to
//! satisfy the trait bound; the `Ok` path is the only one it ever takes.

use std::time::Duration;

use clean_signals::{ControllerCore, RunOptions, UseCase};
use forgekit::{
    AnimationController, Brightness, Color, Curve, DesignLanguage, FrameTime, GetUntracked,
    ImageSource, RwSignal, Set, Theme, TypeScale, clear_app_theme, set_app_theme,
};

use crate::failure::HuddleFailure;

pub mod accent;
pub mod notifications;

pub use accent::AccentChoice;
pub use notifications::{NotifFrequency, NotificationsController};

/// The smallest dynamic-type multiplier the type-scale slider can reach.
pub const TYPE_SCALE_MIN: f32 = 0.85;
/// The largest dynamic-type multiplier the type-scale slider can reach.
pub const TYPE_SCALE_MAX: f32 = 1.30;
/// The unscaled (×1.0) dynamic-type multiplier — the fresh-entry seed and the
/// value that keeps the baseline type scale byte-identical.
pub const TYPE_SCALE_DEFAULT: f32 = 1.0;

/// Duration of the theme-swap fade veil (approved decision 4: ~200ms).
pub const VEIL_FADE: Duration = Duration::from_millis(200);
/// Peak opacity the fade veil reaches at the midpoint of the swap.
pub const VEIL_PEAK_ALPHA: f64 = 0.55;
/// One veil animation tick (~60fps); the fade is driven off a synthetic clock,
/// not the shell's frame clock (see [`SettingsController::trigger_veil`]).
const VEIL_STEP: Duration = Duration::from_millis(16);

/// A 1×1 solid-color image source — the facade-only way to paint an arbitrary
/// (possibly translucent) filled rectangle: stretch it to fill with
/// [`ImageFit::Fill`](forgekit::ImageFit). Used for accent swatches, the current
/// user's avatar block, and the theme-swap fade veil, none of which map onto a
/// themed widget's own surface fill.
pub fn solid_source(color: Color) -> ImageSource {
    let c = color.components;
    let to_u8 = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    ImageSource::from_rgba8(
        vec![to_u8(c[0]), to_u8(c[1]), to_u8(c[2]), to_u8(c[3])],
        1,
        1,
    )
}

/// [`solid_source`] with an explicit `alpha` (`0.0..=1.0`) replacing the color's
/// own — the veil's translucent wash.
pub fn solid_source_alpha(color: Color, alpha: f64) -> ImageSource {
    let c = color.components;
    let to_u8 = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    ImageSource::from_rgba8(
        vec![
            to_u8(c[0]),
            to_u8(c[1]),
            to_u8(c[2]),
            to_u8(alpha.clamp(0.0, 1.0) as f32),
        ],
        1,
        1,
    )
}

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
/// `set_app_theme`. Only the `size` knob is scaled (the M3 spec's line-height /
/// letter-spacing tokens are left as authored), matching the task's "multiplies
/// every role's size" contract.
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
/// Extends the two-axis composition with the theming-engine axes (task 15): the
/// [`AccentChoice`] palette re-tint and the dynamic-type `type_factor`.
///
/// - Clear (follow the platform outright) only when *every* axis is at its
///   default: both design + brightness `System`, [`AccentChoice::Default`], and
///   a ×1.0 `type_factor`. Any non-default axis forces an [`ThemeDecision::Apply`].
/// - On `Apply`, the resolved baseline is re-tinted by [`accent::apply`] and its
///   type scale multiplied by `type_factor` ([`scale_type_scale`]) — the two new
///   axes compose *onto* the design+brightness result, so changing one leaves the
///   others intact (the independence the tests assert).
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

    let mut theme = base.with_brightness(resolved);
    accent::apply(&mut theme, accent);
    scale_type_scale(&mut theme.type_scale, type_factor);
    ThemeDecision::Apply(Box::new(theme))
}

/// Parameters for [`SetTheme`]: the four selections plus the currently-active
/// theme (so a `System` axis can resolve against the live value).
#[derive(Clone, Debug)]
pub struct SetThemeParams {
    pub design: DesignChoice,
    pub brightness: BrightnessChoice,
    /// The accent palette re-tint (task 15).
    pub accent: AccentChoice,
    /// The dynamic-type multiplier (task 15).
    pub type_factor: f32,
    /// The ambient theme active at the moment the change was requested.
    pub current: Theme,
}

/// The synchronous theme-application use case.
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
    type Failure = HuddleFailure;

    async fn execute(&self, params: SetThemeParams) -> Result<ThemeDecision, HuddleFailure> {
        let decision = compose(
            params.design,
            params.brightness,
            params.accent,
            params.type_factor,
            &params.current,
        );
        match &decision {
            ThemeDecision::Clear => clear_app_theme(),
            ThemeDecision::Apply(theme) => set_app_theme((**theme).clone()),
        }
        Ok(decision)
    }
}

/// View model for the appearance settings screen.
///
/// Holds the two selection signals the screen's selectors are controlled by,
/// embeds a [`ControllerCore`] by composition (the `templates/AGENTS.md`
/// controller rule), and runs [`SetTheme`] through it.
pub struct SettingsController {
    core: ControllerCore<HuddleFailure>,
    set_theme: SetTheme,
    /// The selected design language (drives the language selector).
    pub design: RwSignal<DesignChoice>,
    /// The selected brightness (drives the dark/light switch label).
    pub brightness: RwSignal<BrightnessChoice>,
    /// The selected accent palette (drives the swatch picker).
    pub accent: RwSignal<AccentChoice>,
    /// The dynamic-type multiplier (drives the type-scale slider).
    pub type_factor: RwSignal<f32>,
    /// The fade veil's live opacity (`0.0` = no veil), driven by
    /// [`trigger_veil`](Self::trigger_veil) and painted by the appearance screen.
    pub veil: RwSignal<f64>,
}

impl SettingsController {
    /// Seed the selectors from the ambient theme (design + brightness) plus the
    /// two theming-engine axes. The accent + type multiplier cannot be recovered
    /// from the ambient theme (the override slot can't report them), so they seed
    /// from their defaults ([`AccentChoice::Default`] / ×1.0) on a fresh entry.
    pub fn new(design: DesignChoice, brightness: BrightnessChoice) -> Self {
        Self {
            core: ControllerCore::new(),
            set_theme: SetTheme,
            design: RwSignal::new(design),
            brightness: RwSignal::new(brightness),
            accent: RwSignal::new(AccentChoice::default()),
            type_factor: RwSignal::new(TYPE_SCALE_DEFAULT),
            veil: RwSignal::new(0.0),
        }
    }

    /// Apply the current selections app-wide. `current` is the ambient theme the
    /// screen read this frame (used to resolve a `System` axis). Routed through
    /// [`ControllerCore::run`] so the clean-architecture spine (use case →
    /// controller → screen) stays visible.
    pub async fn apply(&self, current: Theme) {
        let params = SetThemeParams {
            design: self.design.get_untracked(),
            brightness: self.brightness.get_untracked(),
            accent: self.accent.get_untracked(),
            type_factor: self.type_factor.get_untracked(),
            current,
        };
        // Infallible by construction; the `Result` is ignored (there is no
        // failure path to surface).
        let _ = self
            .core
            .run(&self.set_theme, params, RunOptions::default())
            .await;
    }

    /// Kick off the ~200ms fade veil around a theme swap (approved decision 4).
    ///
    /// A translucent surface veil rises to [`VEIL_PEAK_ALPHA`] at the swap's
    /// midpoint then fades back out, so the frame the new theme lands on reads as
    /// a soft cross-fade rather than a hard cut. The eased profile is computed by
    /// an [`AnimationController`] advanced off a **synthetic** nanosecond clock on
    /// the background runtime (the paint-driven `PaintCtx::frame_time` path a
    /// widget would use is below-facade and unavailable to app code — see the
    /// screen's scope note), writing the eased opacity into [`veil`](Self::veil),
    /// which the appearance screen reads and paints over its own subtree.
    pub fn trigger_veil(&self) {
        let veil = self.veil;
        forgekit::spawn(async move {
            let mut ctrl = AnimationController::new(VEIL_FADE).with_curve(Curve::EaseInOut);
            ctrl.forward();
            let mut clock_nanos: u64 = 0;
            loop {
                let animating = ctrl.advance(FrameTime::from_nanos(clock_nanos));
                // There-and-back parabola: 0 at the ends, `VEIL_PEAK_ALPHA` at
                // the midpoint of the controller's 0→1 progress.
                let t = ctrl.value();
                let alpha = VEIL_PEAK_ALPHA * (1.0 - (2.0 * t - 1.0).powi(2)).max(0.0);
                veil.set(alpha);
                if !animating {
                    break;
                }
                clean_signals::time::sleep(VEIL_STEP).await;
                clock_nanos = clock_nanos.saturating_add(VEIL_STEP.as_nanos() as u64);
            }
            veil.set(0.0);
        });
    }
}

impl AsRef<ControllerCore<HuddleFailure>> for SettingsController {
    fn as_ref(&self) -> &ControllerCore<HuddleFailure> {
        &self.core
    }
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

    // --- Theming-engine axes (task 15) -----------------------------------

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
