//! **Sample**: an out-of-tree Frust design system, built on public API only.
//!
//! This crate exists to prove a claim the framework makes but had nothing
//! demonstrating end to end — that a third party can ship a themed widget
//! catalog with **every built-in catalog compiled off**, depending on nothing
//! but the `frust` facade. It is a proof of existence, not a design system
//! worth shipping: three widgets, one transition pattern, one typed theme
//! extension. See this workspace's README for what it proves and what it
//! found.
//!
//! # Module map
//!
//! | Module | Holds |
//! |--------|-------|
//! | [`tokens`] | [`sample_theme`](tokens::sample_theme) and the [`SampleAccents`](tokens::SampleAccents) typed extension |
//! | [`badge`] | the **leaf** authoring shape — paint-only, no children |
//! | [`panel`] | the **single-child container** shape |
//! | [`chip`] | the **interactive control** shape |
//! | [`reveal`] | a [`TransitionPattern`](frust::motion::patterns::TransitionPattern) impl for `pattern_switcher` |
//!
//! # Installing
//!
//! ```no_run
//! # struct App;
//! # impl Default for App { fn default() -> Self { App } }
//! # impl frust::Component for App {
//! #     type State = ();
//! #     fn init(&self) -> Self::State {}
//! #     fn build(&self, _s: &mut Self::State) -> frust::AnyView<Self::State> {
//! #         frust::any(frust::text("hi"))
//! #     }
//! # }
//! frust::app!(App, setup = { sample_design::install(); });
//! # fn main() {}
//! ```
//!
//! [`install`]'s timing contract is the whole reason it is a free function
//! called from `app!`'s `setup` block rather than something a component does —
//! see its own docs.

pub mod badge;
pub mod chip;
pub mod panel;
pub mod reveal;
pub mod tokens;

#[cfg(test)]
mod testing;

pub use badge::{SampleBadgeView, SampleBadgeWidget, sample_badge};
pub use chip::{SampleChipView, SampleChipWidget, sample_chip};
pub use panel::{SamplePanelView, SamplePanelWidget, sample_panel};
pub use reveal::SampleReveal;
pub use tokens::{SAMPLE_DESIGN_LANGUAGE, SampleAccents, sample_theme};

/// Make Sample this app's starting design system.
///
/// # The install-timing contract
///
/// Call this from [`frust::app!`]'s `setup = { .. }` block, which runs
/// **before** the shell builds its first frame and before any
/// `Component::init`. That is the only placement with a kept ordering
/// guarantee:
///
/// - **Not `Component::init`** — no ordering contract exists between a
///   component's `init` and the shell's own theme seeding, so a theme
///   installed there may or may not be in place for the first frame.
/// - **Not [`frust::set_app_theme`]** — that is the app-facing *override*, and
///   it pins brightness: an app themed that way stops following the platform's
///   own light/dark flips. A design system wants the *base*, which is
///   [`frust::set_default_theme`]: the shell keeps re-deriving light/dark from
///   the platform appearance against whatever base was seeded.
///
/// The precedence the shell applies is: `set_app_theme` override →
/// `set_default_theme` base (this call) → the shell's own `Theme::neutral()`
/// fallback.
///
/// # Fonts
///
/// A design system with a bundled typeface pushes its bytes here too, through
/// [`frust::register_app_fonts`] — the other half of the same install seam
/// (the framework's own Glyph installer, `frust::glyph_theme::install`, is
/// exactly these two calls). Sample bundles no font on purpose: shipping a
/// megabyte of TTF would make the sample heavier without proving anything the
/// theme half doesn't already prove, and the framework's text stack resolves a
/// system fallback family regardless.
pub fn install() {
    frust::set_default_theme(sample_theme());
}

#[cfg(test)]
mod tests {
    use frust::DesignLanguage;

    use super::*;

    #[test]
    fn install_seeds_a_sample_tagged_theme() {
        // `set_default_theme` is a process-global with no public reader, so
        // what is assertable here is that the call is total (no panic) and that
        // the theme it seeds is the one this crate publishes.
        install();
        assert_eq!(
            sample_theme().design_language,
            DesignLanguage::Custom(SAMPLE_DESIGN_LANGUAGE)
        );
    }
}
