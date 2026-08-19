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
//! polygon keeps its own module path until its last remaining consumer
//! ([`mod@loading_indicator`]) moves across.
//!
//! [`icons`] is the one deliberate exception: its 88 generated
//! [`frust::IconSource`] constants stay namespaced (`frust_material::icons::CHECK`),
//! not flat re-exported at the crate root — see that module's own doc for
//! provenance and the regen command.
//!
//! # `install()`: this design system's one-line installer
//!
//! [`install`] seeds [`frust::set_default_theme`] with this crate's
//! [`baseline`] token bundle and registers its bundled Roboto Flex and Roboto
//! Mono typefaces so Material typography resolves out of the box. Call it from
//! [`frust::app!`](frust::app)'s `setup = { .. }` block, **before** the shell
//! is constructed — the one point a shell reads the default-theme slot:
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
//! `frust::glyph_theme::install` and `frust_shadcn::install` document).

pub mod appbar;
pub mod button;
pub mod button_group;
pub mod card;
pub mod checkbox;
pub mod chips;
pub mod dialog;
pub mod fab;
pub mod fab_menu;
pub mod icon_button;
pub mod icons;
pub mod interaction;
pub mod list_item;
pub mod list_view;
pub mod loading_indicator;
pub mod navbar;
mod press;
pub mod progress;
pub mod radio;
pub mod shape_morph;
pub mod shapes;
pub mod sheet;
pub mod slider;
pub mod split_button;
pub mod state_layer;
pub mod switch;
pub mod text_field;
pub mod toggle_button;
mod tokens;
pub mod toolbar;

pub use appbar::{AppBar, AppBarView, AppBarWidget, app_bar};
pub use button::{
    Button, ButtonDecoration, ButtonShape, ButtonSize, ButtonSurface, ButtonVariant, ButtonView,
    ButtonWidget, ContentMetrics, DecorationOutcome, GradientAlignment, GradientButtonDecoration,
    GradientProperty, GradientSpec, GradientStates, IconAlignment, LinearGradientSpec,
    OverflowObserver, OverflowStrategy, RadialGradientSpec, SweepGradientSpec, button,
    button_with_icon, constant_gradient, elevated_button, filled_button, implied_stops,
    outlined_button, text_button, tonal_button,
};
pub use button_group::{
    ButtonGroup, ButtonGroupAction, ButtonGroupDensity, ButtonGroupDirection, ButtonGroupOverflow,
    ButtonGroupType, ButtonGroupView, ButtonGroupWidget, button_group, button_group_action,
    button_group_actions, button_group_icon_action,
};
pub use card::{
    CardVariant, CardView, CardWidget, card, elevated_card, filled_card, outlined_card,
};
pub use checkbox::{Checkbox, CheckboxView, CheckboxWidget, checkbox, tristate_checkbox};
pub use chips::{
    AssistChip, AssistChipView, AssistChipWidget, FilterChip, FilterChipView, FilterChipWidget,
    InputChip, InputChipView, InputChipWidget, SuggestionChip, SuggestionChipView,
    SuggestionChipWidget, assist_chip, filter_chip, input_chip, suggestion_chip,
};
pub use dialog::{DialogView, DialogWidget, dialog, show_dialog};
pub use fab::{FabColor, FabSize, FabView, FabWidget, extended_fab, fab};
pub use fab_menu::{FabMenu, FabMenuItem, FabMenuView, FabMenuWidget, fab_menu, fab_menu_item};
pub use icon_button::{
    BadgeValue, IconButton, IconButtonShape, IconButtonSize, IconButtonVariant, IconButtonView,
    IconButtonWidget, IconButtonWidth, filled_icon_button, icon_button, outlined_icon_button,
    tonal_icon_button,
};
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
pub use radio::{Radio, RadioView, RadioWidget, radio};
pub use shape_morph::morph_path;
pub use shapes::{CornerRounding, RoundedPolygon};
pub use sheet::{BottomSheetView, BottomSheetWidget, bottom_sheet, show_bottom_sheet};
pub use slider::{
    RangeSliderView, RangeSliderWidget, Slider, SliderIconPosition, SliderRange, SliderTrackIcons,
    SliderTrackKind, SliderView, SliderWidget, centered_slider, range_slider, slider,
    vertical_centered_slider, vertical_slider, wavy_centered_slider, wavy_range_slider,
    wavy_slider,
};
pub use split_button::{SplitButton, SplitButtonView, SplitButtonWidget, split_button};
pub use switch::{Switch, SwitchView, SwitchWidget, switch};
pub use text_field::{TextField, TextFieldVariant, TextFieldView, TextFieldWidget, text_field};
pub use toggle_button::{
    ToggleButton, ToggleButtonSize, ToggleButtonView, ToggleButtonWidget, elevated_toggle_button,
    filled_toggle_button, outlined_toggle_button, text_toggle_button, toggle_button,
    tonal_toggle_button,
};
pub use toolbar::{
    DockedToolbar, FloatingToolbar, ToolbarVariant, ToolbarView, ToolbarWidget, docked_toolbar,
    floating_toolbar,
};

pub use tokens::{CorePalette, Hct, TonalPalette, from_seed, theme_from_seed};
pub use tokens::{
    MaterialDimensions, MaterialMotion, MaterialSemanticColors, MaterialSpacing, MaterialSpring,
    MaterialTokens, baseline, color_scheme_dark, color_scheme_light, elevation, motion_scheme,
    shape_scale, status_palette, type_scale,
};

/// Make Material 3 this app's starting design system.
///
/// Two process-global pushes, both public `frust` seams:
///
/// 1. `frust::set_default_theme(`[`baseline`]`())` — the *base* a shell
///    seeds itself with instead of its built-in `Theme::neutral()` fallback.
///    Deliberately not `frust::set_app_theme`: a seeded default does not pin
///    brightness, so an app installed this way still follows system dark mode.
/// 2. `frust::register_app_fonts` for both bundled faces (Roboto Flex, Roboto
///    Mono), so the type scale's Roboto Flex stack and any mono-text component
///    actually resolve. The faces are compiled in unconditionally (see the
///    crate docs' font coverage).
///
/// # Timing: must run before the first frame
///
/// A shell reads the default-theme slot and drains the font registry **once, at
/// construction**, before its first rebuild. A call after that takes effect only
/// on a later `clear_app_theme`-driven reseed, which may never happen — so a late
/// call silently does nothing visible.
///
/// The supported way to get the timing right on all three platforms is
/// `frust::app!`'s setup block, which runs immediately before the root
/// component's `Component::init` and therefore before any shell construction —
/// see the crate docs for the full example.
///
/// # Thread contract and repeat calls
///
/// Both underlying seams are plain `Mutex`-guarded process-globals callable from
/// any thread. Calling `install` twice is harmless but wasteful: the second
/// `set_default_theme` replaces an identical value, and the font bytes are pushed
/// (and later re-registered, shadowing the same family names) a second time. Call
/// it once.
pub fn install() {
    frust::set_default_theme(baseline());
    for bytes in tokens::font_data() {
        frust::register_app_fonts(bytes.to_vec());
    }
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

#[cfg(test)]
mod baseline_tests {
    use frust::NativeTypefaces;

    /// The baseline theme carries both NativeTypefaces extension (for native
    /// controls) and the Material tokens.
    #[test]
    fn baseline_attaches_native_typefaces_and_material_tokens() {
        let theme = crate::baseline();
        assert!(
            theme.extension::<NativeTypefaces>().is_some(),
            "baseline theme must attach NativeTypefaces for native-widget font resolution"
        );
        assert!(
            theme.extension::<crate::MaterialTokens>().is_some(),
            "baseline theme must attach MaterialTokens"
        );
    }

    /// Verify that font_data() returns non-empty bytes and basic TTF magic.
    #[test]
    fn font_bytes_are_non_empty_and_sniff_as_ttf() {
        let fonts = crate::tokens::font_data();
        assert_eq!(fonts.len(), 2, "expected Roboto Flex + Roboto Mono");
        for (i, bytes) in fonts.iter().enumerate() {
            assert!(!bytes.is_empty(), "font face {i} bytes must be non-empty");
            assert!(
                bytes.len() > 4,
                "font face {i} must have at least 4 bytes for magic number"
            );
            // Check for TTF magic: 0x00010000 (big-endian) or 0x74727565 ('true')
            let magic = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            assert!(
                magic == 0x00010000 || magic == 0x74727565,
                "font face {i} must start with TTF magic bytes"
            );
        }
    }

    /// install() is safe to call twice; the second call is idempotent in terms
    /// of theme identity (same value set twice) and font registration (faces
    /// re-registered under the same family names, shadowing the first).
    #[test]
    fn install_is_safe_to_call_twice() {
        crate::install();
        crate::install();
    }
}
