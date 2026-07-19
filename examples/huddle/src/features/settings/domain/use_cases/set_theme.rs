//! [`SetTheme`] — the synchronous theme-application use case (huddle
//! clean-architecture refactor, task 05; moved as-is from the former flat
//! `features::settings` module per the task's scope note: "keep
//! SetTheme/notifications ops as-is — they're infallible ops, not new
//! ceremony").
//!
//! [`compose`](super::super::models::compose)s the decision, then applies it
//! — the single call site of `set_app_theme`/`clear_app_theme` in the whole
//! app. Never awaits, never fails.
//!
//! # Domain → `frust` exception (flagged for task 06's conformance-scan allowlist)
//!
//! `clear_app_theme`/`set_app_theme` ARE the effect this use case exists to
//! apply — there is no repository/platform seam to go through in this app's
//! architecture, and the module doc above is explicit that this is *the*
//! single call site. Companion exception to
//! [`super::super::models`]'s `frust::` value-type imports; preserved
//! verbatim per PLAN Design Decision 5's hard behavior-preserving bar.

use clean_signals::UseCase;
use frust::{clear_app_theme, set_app_theme};

use crate::failure::HuddleFailure;
use crate::features::settings::domain::models::{SetThemeParams, ThemeDecision, compose};

/// The synchronous theme-application use case. See the [module docs](self).
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
