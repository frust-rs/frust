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
//! The one name carrying two implementations is `RoundedPolygon`: the flat
//! re-export is [`shapes::RoundedPolygon`], the feature-point geometry engine
//! ported from `material_new_shapes`. [`mod@shape_morph`]'s older radial-model
//! polygon keeps its own module path until its two consumers
//! ([`mod@loading_indicator`], [`mod@button_group`]) move across.
//!
//! [`icons`] is the one deliberate exception: its 88 generated
//! [`frust::IconSource`] constants stay namespaced (`frust_material::icons::CHECK`),
//! not flat re-exported at the crate root — see that module's own doc for
//! provenance and the regen command.
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
pub mod icons;
pub mod interaction;
pub mod list_item;
pub mod list_view;
pub mod loading_indicator;
pub mod navbar;
mod press;
pub mod progress;
pub mod shape_morph;
pub mod shapes;
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
pub use interaction::{HapticSignal, InteractionState, MaterialHaptics};
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
pub use shape_morph::morph_path;
pub use shapes::{CornerRounding, RoundedPolygon};
pub use sheet::{BottomSheetView, BottomSheetWidget, bottom_sheet, show_bottom_sheet};
pub use split_button::{SplitButton, SplitButtonView, SplitButtonWidget, split_button};
pub use switch::{Switch, SwitchView, SwitchWidget, switch};
pub use toolbar::{
    DockedToolbar, FloatingToolbar, ToolbarVariant, ToolbarView, ToolbarWidget, docked_toolbar,
    floating_toolbar,
};

pub use tokens::{CorePalette, Hct, TonalPalette, from_seed, theme_from_seed};
pub use tokens::{
    MaterialMotion, MaterialSemanticColors, MaterialSpring, MaterialTokens, baseline,
    color_scheme_dark, color_scheme_light, elevation, motion_scheme, shape_scale, status_palette,
    type_scale,
};

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

/// Coverage for the generated [`icons`] module — deliberately hand-written
/// (not part of `scripts/gen_icons.py`'s output) since a generated file's
/// content is machine-owned end to end.
#[cfg(test)]
mod icons_tests {
    use frust::IconData;

    use crate::icons;

    /// The exact count `plugins/material/scripts/gen_icons.py`'s `ICON_SET`
    /// enumerates — the 88 distinct `M3EIcons.*` names `material_3_expressive`'s
    /// components and example app reference. A change here should come from
    /// a regen (a new/removed `ICON_SET` entry), never a hand-edit.
    const EXPECTED_ICON_COUNT: usize = 88;

    #[test]
    fn all_has_the_expected_count() {
        assert_eq!(icons::ALL.len(), EXPECTED_ICON_COUNT);
    }

    #[test]
    fn every_generated_icon_source_resolves_to_a_non_empty_path() {
        for source in icons::ALL {
            let data: IconData = (*source).into();
            let (path, design) = data.resolve();
            assert_eq!(design, 24.0);
            assert!(
                !path.elements().is_empty(),
                "generated icon `{}` resolved to an empty path",
                source.d
            );
        }
    }

    #[test]
    fn spot_check_check_search_arrow_back() {
        for source in [icons::CHECK, icons::SEARCH, icons::ARROW_BACK] {
            let data: IconData = source.into();
            let (path, design) = data.resolve();
            assert_eq!(design, 24.0);
            assert!(
                !path.elements().is_empty(),
                "spot-checked icon `{}` resolved to an empty path",
                source.d
            );
        }
    }

    #[test]
    fn no_two_entries_share_identical_path_data() {
        let mut seen = std::collections::HashSet::new();
        for source in icons::ALL {
            assert!(
                seen.insert(source.d),
                "two icons::ALL entries share identical path data: {}",
                source.d
            );
        }
    }
}
