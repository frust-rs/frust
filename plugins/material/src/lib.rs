//! `frust-material`: the Material 3 (+Expressive) design-system catalog
//! plugin.
//!
//! A **design-system plugin**: it sits beside the `frust` facade in an app's
//! own dependency list (never inside it — see `docs/ARCHITECTURE.md`'s
//! Facade/plugin boundary), built entirely on `frust`'s public
//! `default-features = false` surface (`frust::authoring` plus the
//! whole-crate `kurbo`/`peniko` valves) so it proves the same external
//! design-system contract `examples/design-system-sample` does.
//!
//! # Flat symbol surface
//!
//! Every widget/spec type below is flat re-exported at the crate root — an
//! app never names a submodule directly, e.g. `frust_material::AppBar`,
//! `frust_material::card`, `frust_material::show_dialog`. This mirrors how
//! the in-tree catalog it was extracted from is consumed through the `frust`
//! facade (`frust::AppBar`, `frust::card`, ...).
//!
//! # `install()`: this design system's one-line installer
//!
//! [`install`] seeds [`frust::set_default_theme`] with this crate's
//! [`baseline`] token bundle — no bundled fonts (Material's baseline
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
//! frust::app!(MyApp, setup = { frust_material::install(); });
//! # fn main() {}
//! ```
//!
//! A call after shell construction takes effect only on a later
//! `clear_app_theme`-driven reseed, which may never happen — so a late call
//! silently does nothing visible (the same timing contract
//! `frust::glyph_theme::install` documents).

pub mod appbar;
pub mod button_group;
pub mod card;
pub mod chips;
pub mod dialog;
pub mod fab;
pub mod fab_menu;
pub mod list_item;
pub mod list_view;
pub mod loading_indicator;
pub mod navbar;
pub mod progress;
pub mod shape_morph;
pub mod sheet;
pub mod split_button;
pub mod state_layer;
pub mod switch;
mod tokens;
pub mod toolbar;

pub use appbar::{AppBar, AppBarView, AppBarWidget, app_bar};
pub use button_group::{ButtonGroup, ButtonGroupView, ButtonGroupWidget, button_group};
pub use card::{
    CardVariant, CardView, CardWidget, card, elevated_card, filled_card, outlined_card,
};
pub use chips::{
    AssistChip, AssistChipView, AssistChipWidget, FilterChip, FilterChipView, FilterChipWidget,
    assist_chip, filter_chip,
};
pub use dialog::{DialogView, DialogWidget, dialog, show_dialog};
pub use fab::{FabSize, FabView, FabWidget, extended_fab, fab};
pub use fab_menu::{FabMenu, FabMenuItem, FabMenuView, FabMenuWidget, fab_menu, fab_menu_item};
pub use list_item::{
    ListItem, ListItemLines, ListItemWidget, ONE_LINE_HEIGHT, THREE_LINE_HEIGHT, TWO_LINE_HEIGHT,
    list_item,
};
pub use loading_indicator::{
    LoadingIndicator, LoadingIndicatorView, LoadingIndicatorWidget, loading_indicator,
};
pub use navbar::{
    NavItem, NavigationBar, NavigationBarView, NavigationBarWidget, nav_item, navigation_bar,
};
pub use progress::{
    CircularProgress, CircularProgressView, CircularProgressWidget, LinearProgress,
    LinearProgressView, LinearProgressWidget, ProgressValue, circular_progress, linear_progress,
};
pub use shape_morph::{RoundedPolygon, morph_path};
pub use sheet::{BottomSheetView, BottomSheetWidget, bottom_sheet, show_bottom_sheet};
pub use split_button::{SplitButton, SplitButtonView, SplitButtonWidget, split_button};
pub use switch::{Switch, SwitchView, SwitchWidget, switch};
pub use toolbar::{
    DockedToolbar, FloatingToolbar, ToolbarVariant, ToolbarView, ToolbarWidget, docked_toolbar,
    floating_toolbar,
};

pub use tokens::{baseline, color_scheme_dark, color_scheme_light, status_palette};

/// Make Material 3 this app's starting design system.
///
/// One process-global push, an already-public seam:
/// [`frust::set_default_theme`]`(`[`baseline`]`())` — the *base* a shell
/// seeds itself with instead of its built-in `Theme::neutral()` fallback.
/// Deliberately not `frust::set_app_theme`: a seeded default does not pin
/// brightness, so an app installed this way still follows system dark mode.
///
/// No font registration: unlike `frust::glyph_theme::install`, Material's
/// baseline type scale carries no bundled font family of its own — a
/// `TextStyle` with no family set resolves to the platform's system font.
///
/// See the [module docs](self) for the full timing contract (call from
/// [`frust::app!`](frust::app)'s `setup = { .. }` block, before shell
/// construction).
pub fn install() {
    frust::set_default_theme(baseline());
}
