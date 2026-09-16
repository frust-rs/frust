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
//! `RoundedPolygon` is the flat re-export of [`shapes::RoundedPolygon`], the
//! feature-point geometry engine ported from `material_new_shapes` — every
//! shape-morphing consumer in this crate, including
//! [`mod@loading_indicator`]'s shape sequence, builds on this one engine;
//! the older, since-removed radial-model polygon module has no remaining
//! callers.
//!
//! [`icons`] is the one deliberate exception: its 88 generated
//! [`frust::IconSource`] constants stay namespaced (`frust_material::icons::CHECK`),
//! not flat re-exported at the crate root — see that module's own doc for
//! provenance and the regen command.
//!
//! [`selection`]'s pure index-set controller functions (`is_selected`,
//! `toggle`, `select_all`, ...) stay namespaced the same way
//! (`frust_material::selection::toggle`) — their names are too generic to
//! flatten safely without shadowing. The module's own widget types
//! ([`selection_host`], [`selection_app_bar`]) are flat re-exported as
//! usual.
//!
//! [`date_picker`]'s pure grid-geometry functions (`year_grid_height`,
//! `calendar_day_view_height`, `year_span`, ...) stay namespaced for the same
//! reason (`frust_material::date_picker::grid::year_span`); its widgets, value
//! types ([`MaterialDate`], [`DateRange`], [`DatePickerState`]) and metric
//! constants are flat re-exported as usual.
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
pub mod badge;
pub mod button;
pub mod button_group;
pub mod card;
pub mod card_list;
pub mod carousel;
pub mod checkbox;
pub mod chips;
pub mod date_picker;
pub mod dialog;
pub mod dismissible;
pub mod divider;
pub mod dropdown;
pub mod expandable_list;
pub mod fab;
pub mod fab_menu;
pub mod icon_button;
pub mod icons;
pub mod interaction;
pub mod list_item;
pub mod list_view;
pub mod loading_indicator;
pub mod menu;
pub mod navbar;
pub mod navigation_drawer;
pub mod navigation_rail;
pub mod overlay;
mod press;
pub mod progress;
pub mod radio;
pub mod refresh_indicator;
pub mod search;
pub mod segmented_button;
pub mod selection;
pub mod shapes;
pub mod sheet;
pub mod side_sheet;
pub mod slider;
pub mod snackbar;
pub mod split_button;
pub mod state_layer;
pub mod switch;
pub mod tabs;
pub mod text_field;
pub mod time_picker;
pub mod toggle_button;
mod tokens;
pub mod toolbar;
pub mod tooltip;

pub use appbar::{
    AppBar, AppBarCollapse, AppBarDensity, AppBarMetrics, AppBarShapeFamily, AppBarVariant,
    AppBarView, AppBarWidget, BottomAppBar, BottomAppBarView, BottomAppBarWidget, SearchAppBar,
    SliverAppBar, SliverAppBarView, SliverAppBarWidget, app_bar, bottom_app_bar, search_app_bar,
    sliver_app_bar,
};
pub use badge::{BadgeAlignment, BadgeView, BadgeWidget, badge};
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
pub use card_list::{
    CARD_LIST_GAP, CARD_LIST_INNER_RADIUS, CARD_LIST_ITEM_PADDING, CARD_LIST_OUTER_RADIUS,
    CardListView, CardListWidget, CardPosition, card_list, card_list_items, card_position,
    card_radii,
};
pub use carousel::{
    CAROUSEL_ITEM_PADDING, CAROUSEL_ITEM_RADIUS, CarouselAxis, CarouselChange, CarouselLayout,
    CarouselView, CarouselWidget, HeroAlignment, UNCONTAINED_ITEM_EXTENT,
    UNCONTAINED_SHRINK_EXTENT, carousel, contained_carousel, hero_carousel, uncontained_carousel,
};
pub use checkbox::{Checkbox, CheckboxView, CheckboxWidget, checkbox, tristate_checkbox};
pub use chips::{
    AssistChip, AssistChipView, AssistChipWidget, FilterChip, FilterChipView, FilterChipWidget,
    InputChip, InputChipView, InputChipWidget, SuggestionChip, SuggestionChipView,
    SuggestionChipWidget, assist_chip, filter_chip, input_chip, suggestion_chip,
};
pub use date_picker::{
    ACTIONS_MIN_HEIGHT, ARROW_ICON_SIZE, ARROW_PADDING, CALENDAR_PADDING, CALENDAR_WIDTH,
    CalendarDatePicker, CalendarDatePickerWidget, DAY_GRID_TOP_PADDING, DAY_ROW_HEIGHT, DAY_SIZE,
    DAYS_PER_WEEK, DIALOG_PORTRAIT_CALENDAR_WIDTH, DIALOG_PORTRAIT_INPUT_WIDTH,
    DISABLED_DAY_OPACITY, DateInputError, DateInputField, DateInputFieldWidget, DatePickerDialog,
    DatePickerEntryMode, DatePickerMode, DatePickerState, DatePickerStrings, DateRange,
    HEADER_PORTRAIT_HEIGHT, MAX_DAY_PICKER_HEIGHT, MAX_DAY_PICKER_ROW_COUNT, MAX_YEAR, MIN_YEAR,
    MONTH_NAV_BUTTONS_WIDTH, MONTH_SCROLL_DURATION, MaterialDate, MonthGrid, OnDatePickerChange,
    RANGE_HIGHLIGHT_ALPHA, SUB_HEADER_HEIGHT, SUB_HEADER_START_INSET, WEEKDAY_ROW_HEIGHT,
    YEAR_COLUMN_COUNT, YEAR_GRID_PADDING, YEAR_ROW_HEIGHT, YEAR_ROW_SPACING, calendar_date_picker,
    date_input_field, date_picker_dialog, days_in_month, is_leap_year, parse_bounded,
    show_date_picker,
};
pub use dialog::{
    DialogView, FullScreenDialogView, SelectionDialogView, dialog, full_screen_dialog,
    selection_dialog, show_dialog, show_full_screen_dialog, show_selection_dialog,
};
pub use dismissible::{
    DISMISS_ACTION_GAP, DISMISS_BACKGROUND_RADIUS, DISMISS_COLLAPSE_HANDOFF,
    DISMISS_FLING_VELOCITY, DISMISS_FLY_OVERSHOOT, DISMISS_ICON_SIZE, DISMISS_THRESHOLD,
    DismissBackground, DismissDirection, DismissibleView, DismissibleWidget, dismiss_background,
    dismissible, speed_multiplier,
};
pub use divider::{DividerView, DividerWidget, divider};
pub use dropdown::{
    DROPDOWN_CHIP_H_PADDING, DROPDOWN_CHIP_ICON_SIZE, DROPDOWN_CHIP_LABEL_GAP,
    DROPDOWN_CHIP_RADIUS, DROPDOWN_CHIP_RUN_SPACING, DROPDOWN_CHIP_SPACING,
    DROPDOWN_CHIP_V_PADDING, DROPDOWN_CONTENT_PADDING, DROPDOWN_EMPTY_TEXT,
    DROPDOWN_FIELD_H_PADDING, DROPDOWN_FIELD_HOVER_ALPHA, DROPDOWN_FIELD_LOADING_SIZE,
    DROPDOWN_FIELD_LOADING_STROKE, DROPDOWN_FIELD_PRESSED_ALPHA, DROPDOWN_FIELD_V_PADDING,
    DROPDOWN_HINT, DROPDOWN_ICON_GAP, DROPDOWN_ICON_SIZE, DROPDOWN_ITEM_GAP,
    DROPDOWN_ITEM_H_PADDING, DROPDOWN_ITEM_HOVER_RADIUS, DROPDOWN_ITEM_INNER_RADIUS,
    DROPDOWN_ITEM_OUTER_RADIUS, DROPDOWN_ITEM_PRESSED_RADIUS, DROPDOWN_ITEM_V_PADDING,
    DROPDOWN_LOADING_PADDING, DROPDOWN_MESSAGE_PADDING, DROPDOWN_PANEL_GAP,
    DROPDOWN_PANEL_MAX_HEIGHT, DROPDOWN_SEARCH_HINT, DropdownColors, DropdownFieldView,
    DropdownFieldWidget, DropdownItem, DropdownPanelView, DropdownPanelWidget, DropdownView,
    dropdown, dropdown_field, dropdown_filter, dropdown_item, dropdown_matches, dropdown_panel,
};
pub use expandable_list::{
    EXPANDABLE_GAP, EXPANDABLE_HOVER_RADIUS, EXPANDABLE_ICON_ROTATION, EXPANDABLE_INNER_RADIUS,
    EXPANDABLE_OUTER_RADIUS, EXPANDABLE_PRESSED_RADIUS, ExpandMode, ExpandableItem,
    ExpandableListView, ExpandableListWidget, REVEAL_SPRING, expandable_item, expandable_list,
    is_expanded, toggle_expanded,
};
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
    LoadingIndicator, LoadingIndicatorVariant, LoadingIndicatorView, LoadingIndicatorWidget,
    loading_indicator,
};
pub use menu::{
    MenuAction, MenuColorStyle, MenuColors, MenuEntry, MenuGroup, MenuIcon, MenuNode,
    MenuPanelView, MenuPanelWidget, MenuSelectable, MenuSelection, MenuSubmenu, MenuToggleable,
    MenuView, menu, menu_entry, menu_group, menu_panel, menu_selectable, menu_submenu,
    menu_toggleable,
};
pub use navbar::{
    HEIGHT_MEDIUM as NAV_BAR_HEIGHT_MEDIUM, HEIGHT_SMALL as NAV_BAR_HEIGHT_SMALL,
    INDICATOR_H as NAV_BAR_INDICATOR_H, INDICATOR_W as NAV_BAR_INDICATOR_W,
    LEAD_SPRING as NAV_INDICATOR_LEAD_SPRING, NavBarIndicatorStyle, NavBarLabelBehavior,
    NavBarSize, NavItem, NavigationBar, NavigationBarView, NavigationBarWidget,
    TRAIL_SPRING as NAV_INDICATOR_TRAIL_SPRING, UNDERLINE_THICKNESS as NAV_BAR_UNDERLINE_THICKNESS,
    nav_item, navigation_bar,
};
pub use navigation_drawer::{
    DRAWER_WIDTH, DrawerContentView, DrawerContentWidget, DrawerDestination, DrawerSection,
    NavigationDrawer, NavigationDrawerView, drawer_destination, drawer_section, navigation_drawer,
    navigation_drawer_content, show_navigation_drawer,
};
pub use navigation_rail::{
    NavigationRail, NavigationRailModality, NavigationRailType, NavigationRailView,
    NavigationRailWidget, RAIL_COLLAPSED_WIDTH, RAIL_EXPAND_CURVE, RAIL_EXPAND_DURATION,
    RAIL_EXPANDED_MAX_WIDTH, RAIL_EXPANDED_MIN_WIDTH, RAIL_ITEM_COLLAPSED_HEIGHT,
    RAIL_ITEM_EXPANDED_HEIGHT, RAIL_LEAD_SPRING, RAIL_TRAIL_SPRING, RailDestination, RailFab,
    RailLabelBehavior, RailSection, navigation_rail, rail_destination, rail_fab, rail_section,
};
pub use overlay::{
    ANCHORED_ENTER_SCALE, AnchoredOverlayView, AnchoredOverlayWidget, ModalDismiss,
    OVERLAY_ANCHOR_GAP, OVERLAY_DIALOG_MAX_WIDTH, OVERLAY_DIALOG_MIN_WIDTH, OVERLAY_EDGE_FRACTION,
    OVERLAY_FLING_VELOCITY, OVERLAY_HANDLE_RESERVE, OVERLAY_SCRIM_ALPHA,
    OVERLAY_SHEET_MAX_HEIGHT_FRACTION, OVERLAY_SIDE_SHEET_MAX_WIDTH, OverlayAlign, OverlayAnchor,
    OverlayAnchorView, OverlayAnchorWidget, OverlayBorder, OverlayContainer, OverlayCorners,
    OverlayElevation, OverlayEntrance, OverlayExtent, OverlayGeometry, OverlayLimit,
    OverlayModalConfig, OverlayModalContent, OverlayModalView, OverlayModalWidget,
    OverlayPlacement, OverlayRole, OverlaySide, anchored_overlay, overlay_anchor, overlay_modal,
    place_anchored, show_overlay_modal,
};
pub use progress::{
    AmplitudeCurve, CircularProgress, CircularProgressView, CircularProgressWidget,
    CircularWavyProgress, LinearProgress, LinearProgressView, LinearProgressWidget,
    LinearWavyProgress, ProgressSize, ProgressValue, circular_progress, circular_wavy_progress,
    linear_progress, linear_wavy_progress,
};
pub use radio::{Radio, RadioView, RadioWidget, radio};
pub use refresh_indicator::{RefreshIndicatorView, RefreshIndicatorWidget, refresh_indicator};
pub use search::{
    DockedSearchViewView, FULL_SCREEN_HEADER_HEIGHT, SEARCH_BAR_MIN_HEIGHT, SEARCH_BAR_MIN_WIDTH,
    SEARCH_VIEW_COMPACT_MAX_WIDTH, SEARCH_VIEW_HEIGHT_FRACTION, SEARCH_VIEW_MIN_HEIGHT,
    SEARCH_VIEW_MIN_WIDTH, SearchBarView, SearchBarWidget, SearchView, SearchViewContentWidget,
    SearchViewMode, SearchViewView, search_bar, search_view, show_search_view,
};
pub use segmented_button::{
    MAX_SEGMENTS, MIN_SEGMENTS, Segment, SegmentedButton, SegmentedButtonView,
    SegmentedButtonWidget, segment, segmented_button,
};
pub use selection::{
    SelectionAppBarView, SelectionAppBarWidget, SelectionHostView, SelectionHostWidget,
    selection_app_bar, selection_host,
};
pub use shapes::{CornerRounding, RoundedPolygon};
pub use sheet::{BottomSheetView, BottomSheetWidget, bottom_sheet, show_bottom_sheet};
pub use side_sheet::{SIDE_SHEET_WIDTH, SideSheetView, show_side_sheet, side_sheet};
pub use slider::{
    RangeSliderView, RangeSliderWidget, Slider, SliderIconPosition, SliderRange, SliderTrackIcons,
    SliderTrackKind, SliderView, SliderWidget, centered_slider, range_slider, slider,
    vertical_centered_slider, vertical_slider, wavy_centered_slider, wavy_range_slider,
    wavy_slider,
};
pub use snackbar::{
    SnackbarController, SnackbarHostView, SnackbarHostWidget, SnackbarMessage, snackbar,
    snackbar_host,
};
pub use split_button::{
    CHEVRON_OPEN_TURNS, CHEVRON_ROTATION_DURATION, SPLIT_ELEVATED_INNER_GAP,
    SPLIT_FOCUS_RING_OUTSET, SPLIT_INNER_GAP, SPLIT_MIN_TAP_TARGET, SPLIT_SHAPE_SPRING,
    SplitButton, SplitButtonItem, SplitButtonMenuRoute, SplitButtonMenuStyle, SplitButtonShape,
    SplitButtonSize, SplitButtonTrailingAlignment, SplitButtonVariant, SplitButtonView,
    SplitButtonWidget, TRAILING_SELECTED_CORNER_PERCENT, split_button, split_button_item,
};
pub use switch::{Switch, SwitchView, SwitchWidget, switch};
pub use tabs::{MIN_TABS, Tab, Tabs, TabsVariant, TabsView, TabsWidget, tab, tabs};
pub use text_field::{TextField, TextFieldVariant, TextFieldView, TextFieldWidget, text_field};
pub use time_picker::{
    DIAL_SIZE, DIAL_SLOTS, DIALOG_LANDSCAPE_WIDTH, DIALOG_PORTRAIT_WIDTH, HEADER_LANDSCAPE_WIDTH,
    TimeDialView, TimeDialWidget, TimeEntry, TimeInputView, TimeInputWidget, TimeOfDay, TimePicker,
    TimePickerEntryMode, TimePickerMode, TimePickerOrientation, TimePickerStrings, TimePickerView,
    show_time_picker, time_dial, time_input, time_picker,
};
pub use toggle_button::{
    ToggleButton, ToggleButtonSize, ToggleButtonView, ToggleButtonWidget, elevated_toggle_button,
    filled_toggle_button, outlined_toggle_button, text_toggle_button, toggle_button,
    tonal_toggle_button,
};
pub use toolbar::{
    DockedToolbar, FloatingToolbar, TOOLBAR_DOCKED_PAD_X, TOOLBAR_EXPAND_SPRING,
    TOOLBAR_FAB_BASELINE, TOOLBAR_FAB_MEDIUM, TOOLBAR_FLOATING_PAD, TOOLBAR_GAP, TOOLBAR_HEIGHT,
    TOOLBAR_MAX_INLINE_ACTIONS, TOOLBAR_REVEAL_FADE_START, TOOLBAR_SCREEN_OFFSET,
    TOOLBAR_SETTLE_VELOCITY, TOOLBAR_TO_FAB_GAP, ToolbarAction, ToolbarColorStyle, ToolbarColors,
    ToolbarElevation, ToolbarExitDirection, ToolbarFabPosition, ToolbarMorph, ToolbarPartition,
    ToolbarScrollHide, ToolbarSize, ToolbarVariant, ToolbarView, ToolbarWidget, docked_toolbar,
    floating_toolbar, partition_actions, toolbar_action,
};
pub use tooltip::{
    RichTooltipView, TooltipAction, TooltipHover, TooltipTriggerView, TooltipTriggerWidget,
    TooltipView, rich_tooltip, tooltip, tooltip_action, tooltip_trigger,
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

    /// Read a big-endian `u16` out of an sfnt-family byte slice at `offset`.
    fn read_u16(bytes: &[u8], offset: usize) -> u16 {
        u16::from_be_bytes([bytes[offset], bytes[offset + 1]])
    }

    /// Read a big-endian `u32` out of an sfnt-family byte slice at `offset`.
    fn read_u32(bytes: &[u8], offset: usize) -> u32 {
        u32::from_be_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ])
    }

    /// Minimal sfnt `fvar` table reader: locates the `fvar` table via the
    /// sfnt table directory, then returns the axis tag of every axis record
    /// (`fvar+8` axisCount, `fvar+4` offsetToAxesArray, 20-byte axis records —
    /// see OpenType spec "Variable Fonts" § `fvar`). No third-party font
    /// parser: the whole point of this check is a dependency-free regression
    /// tripwire on `frust-material`'s own bundled bytes, mirroring the
    /// existing hand-rolled TTF-magic sniff above.
    ///
    /// Returns an empty `Vec` for a font with no `fvar` table (i.e. a static,
    /// non-variable font) rather than panicking.
    fn fvar_axis_tags(bytes: &[u8]) -> Vec<[u8; 4]> {
        let num_tables = read_u16(bytes, 4) as usize;
        let mut fvar_offset = None;
        for i in 0..num_tables {
            let record = 12 + i * 16;
            let tag = &bytes[record..record + 4];
            if tag == b"fvar" {
                fvar_offset = Some(read_u32(bytes, record + 8) as usize);
                break;
            }
        }
        let Some(fvar_offset) = fvar_offset else {
            return Vec::new();
        };

        let axes_array_offset = read_u16(bytes, fvar_offset + 4) as usize;
        let axis_count = read_u16(bytes, fvar_offset + 8) as usize;
        let axis_size = read_u16(bytes, fvar_offset + 10) as usize;
        assert_eq!(axis_size, 20, "fvar axis record size must be 20 bytes");

        (0..axis_count)
            .map(|i| {
                let record = fvar_offset + axes_array_offset + i * axis_size;
                let mut tag = [0u8; 4];
                tag.copy_from_slice(&bytes[record..record + 4]);
                tag
            })
            .collect()
    }

    /// Roboto Flex must be the `wght`-only instance `FONTS-LICENSE`'s
    /// "Modification" record documents, not the 13-axis upstream font: one
    /// `fvar` axis, tagged `wght`. A regression back to the full variable
    /// font (or the loss of `wght` variability) must fail this test, not
    /// silently re-bloat the plugin's bundled bytes.
    ///
    /// Indexes `font_data()[0]` directly rather than through
    /// `tokens::ROBOTO_FLEX_VARIABLE_INDEX` (private to the `tokens` module):
    /// `font_data`'s own doc comment documents the array's fixed order as
    /// "Roboto Flex Variable, then Roboto Mono Variable", so `[0]` is Roboto
    /// Flex by that public contract, not an incidental array position.
    #[test]
    fn roboto_flex_is_a_wght_only_instance() {
        let bytes = crate::tokens::font_data()[0];
        let axes = fvar_axis_tags(bytes);
        assert_eq!(
            axes,
            vec![*b"wght"],
            "Roboto Flex must carry exactly one fvar axis, `wght`; got {axes:?}"
        );
    }

    /// Length ceiling on the vendored Roboto Flex bytes: well above the
    /// ~176 KB instanced size, but far below the ~1.68 MB upstream 13-axis
    /// font, so a regression to the full variable font fails this test
    /// instead of silently landing in a release build. See
    /// `roboto_flex_is_a_wght_only_instance` for why `[0]` is Roboto Flex.
    #[test]
    fn roboto_flex_is_smaller_than_the_upstream_variable_font() {
        let bytes = crate::tokens::font_data()[0];
        assert!(
            bytes.len() < 200_000,
            "Roboto Flex must be the wght-only instance (< 200,000 B); got {} B \
             — did the bundled font regress to the full upstream variable font?",
            bytes.len()
        );
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
