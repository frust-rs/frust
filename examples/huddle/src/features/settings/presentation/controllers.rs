//! `settings` presentation — [`SettingsController`] and
//! [`NotificationsController`], moved verbatim from the former flat
//! `features::settings`/`features::settings::notifications` modules.
//!
//! [`SettingsController`] owns the two selection signals ([`DesignChoice`] and
//! [`BrightnessChoice`]) and drives the [`SetTheme`] use case, which is the
//! **one and only** place the whole app forces its active [`Theme`] via
//! `frust::set_app_theme`/`clear_app_theme`. Every other screen just reads
//! the ambient theme (`use_context::<Theme>()` / `PaintCtx::theme_as`), so a
//! change here live-swaps all four tabs at once and, because the override is a
//! process-global (`docs/ARCHITECTURE.md`'s Theme delivery), survives navigation.
//!
//! The controller is hosted inside
//! [`crate::features::settings::presentation::pages::settings_appearance`]'s
//! `Component` via `clean_signals_frust::use_controller`. The *effect* is
//! app-global (the theme override is process-wide); the controller instance
//! itself is component-scoped and disposed on teardown.
//!
//! [`NotificationsController`] owns the notification-frequency selection plus
//! a handful of per-type toggle booleans as [`RwSignal`]s, hosted by the
//! notifications screen's `Component` via the same seam. Unlike
//! [`SettingsController`], nothing here is applied to the running app — these
//! are pure preference values the screen's [`radio`](frust::radio) group and
//! [`Switch`](frust_material::Switch)es read and write. It embeds a [`ControllerCore`]
//! by composition purely to satisfy the `use_controller`
//! `AsRef<ControllerCore>` bound and keep the clean-architecture spine
//! visible; it runs no use case.

use std::time::Duration;

use clean_signals::{ControllerCore, RunOptions};
use frust::{AnimationController, Curve, FrameTime, GetUntracked, RwSignal, Set, Theme};

use crate::failure::HuddleFailure;
use crate::features::settings::domain::{
    AccentChoice, BrightnessChoice, DesignChoice, NotifFrequency, SetTheme, SetThemeParams,
    TYPE_SCALE_DEFAULT,
};

/// Duration of the theme-swap fade veil (approved decision 4: ~200ms).
pub const VEIL_FADE: Duration = Duration::from_millis(200);
/// Peak opacity the fade veil reaches at the midpoint of the swap.
pub const VEIL_PEAK_ALPHA: f64 = 0.55;
/// One veil animation tick (~60fps); the fade is driven off a synthetic clock,
/// not the shell's frame clock (see [`SettingsController::trigger_veil`]).
const VEIL_STEP: Duration = Duration::from_millis(16);

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
        frust::spawn(async move {
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

/// View model for the notification-settings screen. See the [module docs](self).
pub struct NotificationsController {
    core: ControllerCore<HuddleFailure>,
    /// The selected notification frequency (drives the radio group).
    pub frequency: RwSignal<NotifFrequency>,
    /// Play a sound on a new notification.
    pub sound: RwSignal<bool>,
    /// Vibrate on a new notification.
    pub vibrate: RwSignal<bool>,
    /// Show a preview of the message body in the notification.
    pub previews: RwSignal<bool>,
}

impl NotificationsController {
    /// A controller seeded with sensible defaults (all-messages frequency,
    /// sound + previews on, vibrate off).
    pub fn new() -> Self {
        Self {
            core: ControllerCore::new(),
            frequency: RwSignal::new(NotifFrequency::default()),
            sound: RwSignal::new(true),
            vibrate: RwSignal::new(false),
            previews: RwSignal::new(true),
        }
    }
}

impl Default for NotificationsController {
    fn default() -> Self {
        Self::new()
    }
}

impl AsRef<ControllerCore<HuddleFailure>> for NotificationsController {
    fn as_ref(&self) -> &ControllerCore<HuddleFailure> {
        &self.core
    }
}
