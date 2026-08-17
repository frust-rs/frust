//! `frust-cupertino`: the Cupertino (iOS) design-system catalog plugin.
//!
//! A **design-system plugin**: it sits beside the `frust` facade in an app's
//! own dependency list (never inside it — see `docs/ARCHITECTURE.md`'s
//! Facade/plugin boundary), built entirely on `frust`'s public
//! `default-features = false` surface (`frust::authoring` plus the
//! whole-crate `kurbo`/`peniko` valves) so it proves the same external
//! design-system contract `examples/design-system-sample` does.
//!
//! # Widget catalog
//!
//! The Flutter-parity counterparts to a subset of the Material catalog — no
//! Cupertino equivalent exists for FAB/Chips/Card, so none are built here:
//!
//! * [`navbar`] — `CupertinoNavBar` (44pt).
//! * [`tabbar`] — `CupertinoTabBar` (49pt content height).
//! * [`switch`] — `CupertinoSwitch` (64×28pt, matching the current iOS UI Kit
//!   metric — an earlier revision used a community-approximate 51×31pt).
//! * [`alert_dialog`] — `CupertinoAlertDialog`.
//! * [`action_sheet`] — `CupertinoActionSheet`.
//! * [`activity_indicator`] — `CupertinoActivityIndicator` (20pt).
//! * [`button`] — `CupertinoButton` (kit-mined Small/Medium/Large size
//!   classes; Filled/Gray/Glass styles).
//!
//! # Flat symbol surface
//!
//! Every widget/spec type below is flat re-exported at the crate root — an
//! app never names a submodule directly, e.g. `frust_cupertino::CupertinoButton`,
//! `frust_cupertino::cupertino_switch`. This mirrors how the in-tree catalog
//! it was extracted from is consumed through the `frust` facade
//! (`frust::CupertinoButton`, `frust::cupertino_switch`, ...).
//!
//! # Glass degradation
//!
//! Several widgets ([`button`]'s `Glass` style, [`navbar`], [`tabbar`],
//! [`alert_dialog`], [`action_sheet`]) paint from `Theme.glass` — this
//! crate's own [`ios27`] Liquid Glass recipe copy on a Cupertino-tagged
//! theme. On any other design language (or an opaque
//! [`frust::GlassScale::opaque_material`]-style scale), the same widget code
//! degrades to an opaque fill instead: one widget API renders correctly
//! under either design language, per-widget doc comments cover the exact
//! opaque-path appearance.
//!
//! # `install()`: this design system's one-line installer
//!
//! [`install`] seeds [`frust::set_default_theme`] with this crate's
//! [`baseline`] token bundle — no bundled fonts (Cupertino's baseline
//! typography resolves to whatever system font family a `TextStyle` leaves
//! unset). Call it from [`frust::app!`](frust::app)'s `setup = { .. }` block,
//! **before** the shell is constructed — the one point a shell reads the
//! default-theme slot:
//!
//! ```no_run
//! # use frust::{AnyView, Component, any, text};
//! # #[derive(Default)]
//! # struct MyApp;
//! # impl Component for MyApp {
//! #     type State = ();
//! #     fn init(&self) -> Self::State {}
//! #     fn build(&self, _state: &mut Self::State) -> AnyView<Self::State> { any(text("hi")) }
//! # }
//! frust::app!(MyApp, setup = { frust_cupertino::install(); });
//! # fn main() {}
//! ```
//!
//! A call after shell construction takes effect only on a later
//! `clear_app_theme`-driven reseed, which may never happen — so a late call
//! silently does nothing visible (the same timing contract
//! `frust::glyph_theme::install` documents).

pub mod action_sheet;
pub mod activity_indicator;
pub mod alert_dialog;
pub mod button;
mod glass;
pub mod navbar;
pub mod switch;
pub mod tabbar;
mod tokens;

pub use action_sheet::{CupertinoActionSheetView, CupertinoActionSheetWidget, show_action_sheet};
pub use activity_indicator::{
    CupertinoActivityIndicator, CupertinoActivityIndicatorView, CupertinoActivityIndicatorWidget,
    cupertino_activity_indicator,
};
pub use alert_dialog::{
    CupertinoActionStyle, CupertinoAlertDialogView, CupertinoAlertDialogWidget,
    CupertinoDialogAction, action, show_cupertino_alert,
};
pub use button::{
    CupertinoButton, CupertinoButtonSize, CupertinoButtonStyle, CupertinoButtonView,
    CupertinoButtonWidget, cupertino_button,
};
pub use navbar::{CupertinoNavBar, CupertinoNavBarView, CupertinoNavBarWidget, cupertino_nav_bar};
pub use switch::{CupertinoSwitch, CupertinoSwitchView, CupertinoSwitchWidget, cupertino_switch};
pub use tabbar::{
    CupertinoTabBar, CupertinoTabBarView, CupertinoTabBarWidget, TabItem, cupertino_tab_bar,
    tab_item,
};

pub use glass::ios27;
pub use tokens::{baseline, color_scheme_dark, color_scheme_light, status_palette};

/// Make Cupertino this app's starting design system.
///
/// One process-global push, an already-public seam:
/// [`frust::set_default_theme`]`(`[`baseline`]`())` — the *base* a shell
/// seeds itself with instead of its built-in `Theme::neutral()` fallback.
/// Deliberately not `frust::set_app_theme`: a seeded default does not pin
/// brightness, so an app installed this way still follows system dark mode.
///
/// No font registration: unlike `frust::glyph_theme::install`, Cupertino's
/// baseline type scale carries no bundled font family of its own — a
/// `TextStyle` with no family set resolves to the platform's system font.
///
/// See the [module docs](self) for the full timing contract (call from
/// [`frust::app!`](frust::app)'s `setup = { .. }` block, before shell
/// construction).
pub fn install() {
    frust::set_default_theme(baseline());
}
