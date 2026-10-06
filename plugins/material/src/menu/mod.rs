// Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
// `lib/components/menus/` — `m3e_menus.dart` (the anchored menu),
// `styles/m3e_menu_theme.dart` (every metric and both color resolutions) and
// `enums/m3e_menu_color_style.dart`, retrieved 2026-08-19.
// Upstream: <https://github.com/paadevelopments/material_3_expressive>
// Porting decisions: the reference's `showM3EMenu` inserts its own Flutter
// `OverlayEntry` and hand-rolls placement, a transparent dismiss layer and the
// expand/collapse spring (`m3e_menu_popup.dart`, `utils/m3e_menu_placer.dart`,
// `utils/m3e_menu_spring_motion.dart`); **none of that is ported** — this
// catalog already merged one anchored host (`crate::overlay::anchored`), which
// owns placement, light dismiss, Escape and the enter/exit ramp, so the menu
// composes under it rather than beside it. See each submodule's own header.

//! The Material 3 Expressive menu: the node tree, its two color styles, and
//! the anchored menu that presents them.
//!
//! # Three layers
//!
//! 1. [`mod@items`] — the node vocabulary a caller composes ([`menu_entry`],
//!    [`menu_selectable`], [`menu_toggleable`], [`MenuNode::Divider`],
//!    [`menu_group`], [`menu_submenu`]) and what an activation reports back
//!    ([`MenuSelection`]).
//! 2. [`mod@panel`] — [`menu_panel`], the reusable item-list surface: it
//!    renders the tree as a stack of elevated containers, runs the hover/press
//!    machine and owns the submenu chain. A dropdown or a button-group popup
//!    mounts this directly.
//! 3. [`menu`] — the panel inside [`crate::overlay::anchored`], which is what
//!    turns it into a menu: trigger-relative placement, light dismiss, Escape,
//!    and the entrance/exit ramp.
//!
//! # The host owns the ramp
//!
//! [`crate::overlay::anchored`] composites the panel under its own fade and
//! anchor-pivoted scale, so nothing here choreographs an entrance or an exit —
//! and an app that wants the *exit* keeps the menu mounted and toggles
//! [`MenuView::open`] rather than unmounting it (the kept-mounted pattern that
//! host documents).
//!
//! # Standard and vibrant
//!
//! [`MenuColorStyle`] picks between the reference's two container/content
//! resolutions (`m3e_menu_theme.dart:239-261`): **standard** is a
//! `surfaceContainerLow` surface with `onSurface` ink and a `tertiaryContainer`
//! selection; **vibrant** makes the whole surface `tertiaryContainer` and
//! promotes the selection to `tertiary`. Both resolve from plain `ColorScheme`
//! roles — none of the nine [`crate::MaterialTokens`] extension roles applies
//! here — which is the same role-partition line [`mod@crate::divider`] draws.

pub mod items;
pub mod panel;

pub use items::{
    MenuAction, MenuEntry, MenuGroup, MenuIcon, MenuNode, MenuSelectable, MenuSelection,
    MenuSubmenu, MenuToggleable, menu_entry, menu_group, menu_selectable, menu_submenu,
    menu_toggleable, partition_surfaces,
};
pub use panel::{MenuPanelView, MenuPanelWidget, menu_panel};

use std::rc::Rc;

use frust::authoring::{BuildCtx, ChangeFlags, TypedArgCallback, View};
use frust::{Color, Theme};

use crate::interaction::DISABLED_CONTENT_OPACITY;
use crate::overlay::{
    AnchoredOverlayView, AnchoredOverlayWidget, OVERLAY_ANCHOR_GAP, OverlayAlign, OverlayAnchor,
    OverlayPlacement, OverlaySide, anchored_overlay, with_alpha,
};
use crate::tokens::{MaterialDimensions, MaterialSpacing};

// ---- Metrics (`m3e_menu_theme.dart:82`'s `M3EMenuTheme` defaults) ----------
//
// Every value below is that constructor's own default; the ones that land
// exactly on a Material spacing/shape token resolve to it rather than repeating
// a literal.

/// A menu surface's minimum width, in logical px (`minWidth: 112`).
pub const MENU_MIN_WIDTH: f64 = 112.0;
/// A menu surface's maximum width (`maxWidth: 280`).
pub const MENU_MAX_WIDTH: f64 = 280.0;
/// The height past which a menu scrolls instead of growing (`maxHeight: 320`).
pub const MENU_MAX_HEIGHT: f64 = 320.0;
/// A row's minimum height (`entryHeight: 48`).
pub const MENU_ROW_MIN_HEIGHT: f64 = 48.0;
/// A leading/trailing icon's side length (`iconSize: 24`).
pub const MENU_ICON_SIZE: f64 = 24.0;

/// A surface's top/bottom padding (`verticalPadding: 8`).
pub(crate) const MENU_SURFACE_V_PADDING: f64 = MaterialSpacing::SM;
/// A surface's left/right padding (`contentHorizontalPadding: 8`).
pub(crate) const MENU_SURFACE_H_PADDING: f64 = MaterialSpacing::SM;
/// The gap between two elevated surfaces (`sectionGap: 8`).
pub(crate) const MENU_SECTION_GAP: f64 = MaterialSpacing::SM;
/// A row's left/right padding (`entryHorizontalPadding: 12`).
pub(crate) const MENU_ROW_H_PADDING: f64 = MaterialSpacing::MD;
/// The gap between a row's icon and its text (`iconGap: 12`).
pub(crate) const MENU_ICON_GAP: f64 = MaterialSpacing::MD;
/// The vertical space between rows inside a surface (`itemGap: 4`).
pub(crate) const MENU_ITEM_GAP: f64 = MaterialSpacing::XS;
/// A section label's left/right padding (`groupLabelHorizontalPadding: 12`).
pub(crate) const MENU_GROUP_LABEL_H_PADDING: f64 = MaterialSpacing::MD;
/// A section label's top/bottom padding (`groupLabelVerticalPadding: 8`).
pub(crate) const MENU_GROUP_LABEL_V_PADDING: f64 = MaterialSpacing::SM;
/// Unthemed-fallback surface corner radius (`containerRadius: 16`) — a themed
/// pass resolves [`container_radius`] instead.
pub(crate) const MENU_CONTAINER_RADIUS: f64 = MaterialDimensions::RADIUS_LARGE;
/// Unthemed-fallback row-highlight corner radius (`itemRadius: 12`) — a themed
/// pass resolves [`item_radius`] instead.
pub(crate) const MENU_ITEM_RADIUS: f64 = MaterialDimensions::RADIUS_MEDIUM;
/// The extra top/bottom padding a row with supporting text takes
/// (`m3e_menu_item.dart:98`'s `vertical: supportingText == null ? 0 : 8`).
pub(crate) const MENU_SUPPORTING_V_PADDING: f64 = MaterialSpacing::SM;
/// A divider's own top/bottom padding (`m3e_menu_divider.dart:18`).
pub(crate) const MENU_DIVIDER_V_PADDING: f64 = MaterialSpacing::XS;
/// A divider's rule thickness (`m3e_menu_divider.dart:22`'s `height: 1`).
pub(crate) const MENU_DIVIDER_THICKNESS: f64 = 1.0;
/// The gap between a submenu row and the panel it opens (`anchorOffset: 4`) —
/// the same 4dp the anchored host uses between a trigger and its panel.
pub(crate) const MENU_SUBMENU_GAP: f64 = OVERLAY_ANCHOR_GAP;
/// The implicit selected check's size relative to [`MENU_ICON_SIZE`]
/// (`m3e_menu_node_builders.dart:58`'s `iconSize * 0.9`).
pub(crate) const SELECTED_CHECK_SCALE: f64 = 0.9;

/// A menu surface's corner radius, resolved from the theme's shape scale.
///
/// The reference's `containerRadius: 16` lands exactly on the `large` shape
/// token, so the token is what a themed pass reads and
/// [`MENU_CONTAINER_RADIUS`] is only the unthemed fallback — the
/// **explicit > theme > fallback** ladder the workspace's theming conventions
/// require, and the same shape [`crate::overlay::radius`] takes for a panel.
pub(crate) fn container_radius(theme: Option<&Theme>) -> f64 {
    theme.map_or(MENU_CONTAINER_RADIUS, |t| t.shape.large)
}

/// A menu row's highlight radius — the `medium` shape token, which the
/// reference's `itemRadius: 12` lands on. See [`container_radius`].
pub(crate) fn item_radius(theme: Option<&Theme>) -> f64 {
    theme.map_or(MENU_ITEM_RADIUS, |t| t.shape.medium)
}

// ---- Colors (`m3e_menu_theme.dart:234`'s `colors`) ------------------------

/// Unthemed-fallback `surfaceContainerLow`.
const FALLBACK_SURFACE_CONTAINER_LOW: Color = Color::from_rgb8(0xF7, 0xF2, 0xFA);
/// Unthemed-fallback `onSurface`.
const FALLBACK_ON_SURFACE: Color = Color::from_rgb8(0x1D, 0x1B, 0x20);
/// Unthemed-fallback `onSurfaceVariant`.
const FALLBACK_ON_SURFACE_VARIANT: Color = Color::from_rgb8(0x49, 0x45, 0x4F);
/// Unthemed-fallback `tertiaryContainer`.
const FALLBACK_TERTIARY_CONTAINER: Color = Color::from_rgb8(0xFF, 0xD8, 0xE4);
/// Unthemed-fallback `onTertiaryContainer`.
const FALLBACK_ON_TERTIARY_CONTAINER: Color = Color::from_rgb8(0x31, 0x11, 0x1D);
/// Unthemed-fallback `tertiary`.
const FALLBACK_TERTIARY: Color = Color::from_rgb8(0x7D, 0x52, 0x60);
/// Unthemed-fallback `onTertiary`.
const FALLBACK_ON_TERTIARY: Color = Color::from_rgb8(0xFF, 0xFF, 0xFF);
/// Unthemed-fallback `outlineVariant`.
const FALLBACK_OUTLINE_VARIANT: Color = Color::from_rgb8(0xCA, 0xC4, 0xD0);
/// Unthemed-fallback `error` — a destructive row's ink.
const FALLBACK_ERROR: Color = Color::from_rgb8(0xB3, 0x26, 0x1E);

/// The alpha a vibrant menu's divider takes
/// (`m3e_menu_theme.dart:259`'s `onTertiaryContainer.withValues(alpha: 0.24)`).
const VIBRANT_DIVIDER_ALPHA: f32 = 0.24;

/// Standard versus vibrant container/content mapping
/// (`enums/m3e_menu_color_style.dart`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MenuColorStyle {
    /// `surfaceContainerLow` surface, `onSurface`/`onSurfaceVariant` content,
    /// `tertiaryContainer` selection — the default.
    #[default]
    Standard,
    /// `tertiaryContainer` surface throughout, with a `tertiary` selection.
    Vibrant,
}

/// One [`MenuColorStyle`]'s resolved roles (`M3EMenuColors`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MenuColors {
    /// The elevated surface's fill.
    pub container: Color,
    /// An idle row's label ink.
    pub content: Color,
    /// An idle row's leading/trailing icon ink.
    pub icon_content: Color,
    /// Supporting text, shortcuts and section labels.
    pub supporting_content: Color,
    /// A selected row's fill.
    pub selected_container: Color,
    /// A selected row's label and icon ink.
    pub selected_content: Color,
    /// The hover/focus/pressed overlay ink.
    pub state_layer: Color,
    /// A divider's rule.
    pub divider: Color,
}

impl MenuColors {
    /// Resolve `style`'s eight roles from `theme`, or from the M3 baseline
    /// light values when no theme is threaded into the pass.
    pub fn resolve(theme: Option<&Theme>, style: MenuColorStyle) -> Self {
        let (
            surface_container_low,
            on_surface,
            on_surface_variant,
            tertiary_container,
            on_tertiary_container,
            tertiary,
            on_tertiary,
            outline_variant,
        ) = match theme {
            Some(theme) => {
                let s = theme.scheme();
                (
                    s.surface_container_low,
                    s.on_surface,
                    s.on_surface_variant,
                    s.tertiary_container,
                    s.on_tertiary_container,
                    s.tertiary,
                    s.on_tertiary,
                    s.outline_variant,
                )
            }
            None => (
                FALLBACK_SURFACE_CONTAINER_LOW,
                FALLBACK_ON_SURFACE,
                FALLBACK_ON_SURFACE_VARIANT,
                FALLBACK_TERTIARY_CONTAINER,
                FALLBACK_ON_TERTIARY_CONTAINER,
                FALLBACK_TERTIARY,
                FALLBACK_ON_TERTIARY,
                FALLBACK_OUTLINE_VARIANT,
            ),
        };
        match style {
            MenuColorStyle::Standard => MenuColors {
                container: surface_container_low,
                content: on_surface,
                icon_content: on_surface_variant,
                supporting_content: on_surface_variant,
                selected_container: tertiary_container,
                selected_content: on_tertiary_container,
                state_layer: on_surface,
                divider: outline_variant,
            },
            MenuColorStyle::Vibrant => MenuColors {
                container: tertiary_container,
                content: on_tertiary_container,
                icon_content: on_tertiary_container,
                supporting_content: on_tertiary_container,
                selected_container: tertiary,
                selected_content: on_tertiary,
                state_layer: on_tertiary_container,
                divider: with_alpha(on_tertiary_container, VIBRANT_DIVIDER_ALPHA),
            },
        }
    }

    /// A row label's ink (`entryForegroundColor`): disabled dims
    /// [`Self::content`] to 38%, destructive takes `error`, selected takes
    /// [`Self::selected_content`].
    pub fn entry_foreground(&self, enabled: bool, destructive: bool, selected: bool) -> Color {
        if !enabled {
            return with_alpha(self.content, DISABLED_CONTENT_OPACITY);
        }
        if destructive {
            return self.error();
        }
        if selected {
            self.selected_content
        } else {
            self.content
        }
    }

    /// A row icon's ink (`entryIconForegroundColor`) — the same ladder over
    /// [`Self::icon_content`].
    pub fn icon_foreground(&self, enabled: bool, destructive: bool, selected: bool) -> Color {
        if !enabled {
            return with_alpha(self.icon_content, DISABLED_CONTENT_OPACITY);
        }
        if destructive {
            return self.error();
        }
        if selected {
            self.selected_content
        } else {
            self.icon_content
        }
    }

    /// Supporting/shortcut text ink (`supportingTextStyle`): a disabled row
    /// dims **`content`**, not `supporting_content` — the reference's own
    /// asymmetry (`m3e_menu_theme.dart:361`), kept.
    pub fn supporting_foreground(&self, enabled: bool, selected: bool) -> Color {
        if !enabled {
            return with_alpha(self.content, DISABLED_CONTENT_OPACITY);
        }
        if selected {
            self.selected_content
        } else {
            self.supporting_content
        }
    }

    /// The destructive ink. The reference reads `scheme.error` directly, which
    /// no `M3EMenuColors` field carries; this port keeps it out of the struct
    /// for the same reason and resolves the M3 baseline light value, matching
    /// every other unthemed fallback here.
    fn error(&self) -> Color {
        FALLBACK_ERROR
    }
}

/// A declarative Material menu: a [`menu_panel`] inside the anchored host. See
/// [`menu`].
///
/// The host is composed fresh in each pass from the props below rather than
/// retained: [`AnchoredOverlayView`] type-erases its content at construction,
/// so there is no way to reach back into the panel and re-set one of its own
/// props once it is inside.
pub struct MenuView<State: 'static> {
    nodes: Vec<MenuNode>,
    style: MenuColorStyle,
    selected: Option<String>,
    open: bool,
    placement: OverlayPlacement,
    anchor: OverlayAnchor,
    on_select: TypedArgCallback<State, MenuSelection>,
    on_dismiss: Option<OnDismiss<State>>,
}

/// A view-held, typed dismiss callback — the same shape the anchored host's own
/// takes, held here because the host is composed fresh per pass.
type OnDismiss<State> = Rc<dyn Fn(&mut State)>;

/// Build a menu over `nodes`, to be mounted while the app's own open flag is
/// set — or kept mounted with the flag handed to [`MenuView::open`] for an exit
/// ramp (see [`crate::overlay::anchored`] for both mount contracts).
///
/// `on_select(state, selection)` reports an activation; a submenu row opens its
/// panel instead of reporting, exactly as upstream.
///
/// The default placement is the reference's `bottomStart`
/// (`m3e_menus.dart:38`): below the trigger, leading edges flush, at the 4dp
/// anchor gap.
pub fn menu<State: 'static, F: Fn(&mut State, MenuSelection) + 'static>(
    nodes: Vec<MenuNode>,
    on_select: F,
) -> MenuView<State> {
    MenuView {
        nodes,
        style: MenuColorStyle::default(),
        selected: None,
        open: true,
        placement: OverlayPlacement::default()
            .align(OverlayAlign::Start)
            .offset(OVERLAY_ANCHOR_GAP),
        anchor: OverlayAnchor::new(),
        on_select: Rc::new(on_select),
        on_dismiss: None,
    }
}

impl<State: 'static> MenuView<State> {
    /// Anchor the menu to the rect `anchor` carries (see
    /// [`crate::overlay::OverlayAnchor`]).
    pub fn anchor(mut self, anchor: &OverlayAnchor) -> Self {
        self.anchor = anchor.clone();
        self
    }

    /// Set the side the menu opens on (`bottom` by default).
    pub fn side(mut self, side: OverlaySide) -> Self {
        self.placement.side = side;
        self
    }

    /// Set the cross-axis alignment (`start` by default, the reference's
    /// `bottomStart`).
    pub fn align(mut self, align: OverlayAlign) -> Self {
        self.placement.align = align;
        self
    }

    /// Set the gap between trigger and menu (`anchorOffset`, 4dp by default).
    pub fn offset(mut self, offset: f64) -> Self {
        self.placement.offset = offset;
        self
    }

    /// Pick the standard or vibrant color resolution.
    pub fn color_style(mut self, style: MenuColorStyle) -> Self {
        self.style = style;
        self
    }

    /// Hand the menu the app-confirmed selected value its
    /// [`MenuSelectable`] rows check against.
    pub fn selected(mut self, value: Option<String>) -> Self {
        self.selected = value;
        self
    }

    /// Hand a **kept-mounted** menu the app's open flag, so closing it plays
    /// the host's exit ramp instead of vanishing — and closes the whole submenu
    /// cascade on the spot.
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Set the dismiss callback: a press outside the menu, or Escape once the
    /// host holds focus.
    pub fn on_dismiss<F: Fn(&mut State) + 'static>(mut self, on_dismiss: F) -> Self {
        self.on_dismiss = Some(Rc::new(on_dismiss));
        self
    }

    /// Compose the host and its panel from the current props.
    fn host(&self) -> AnchoredOverlayView<State> {
        let on_select = self.on_select.clone();
        let panel = menu_panel(self.nodes.clone(), move |state: &mut State, selection| {
            on_select(state, selection)
        })
        .color_style(self.style)
        .selected(self.selected.clone())
        .open(self.open);
        let mut host = anchored_overlay(panel)
            .anchor(&self.anchor)
            .placement(self.placement)
            .open(self.open);
        if let Some(on_dismiss) = self.on_dismiss.clone() {
            host = host.on_dismiss(move |state: &mut State| on_dismiss(state));
        }
        host
    }
}

impl<State: 'static> View<State> for MenuView<State> {
    type Element = AnchoredOverlayWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> AnchoredOverlayWidget {
        View::build(&self.host(), ctx)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AnchoredOverlayWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        View::rebuild(&self.host(), &prev.host(), element, ctx)
    }

    fn teardown(&self, element: &mut AnchoredOverlayWidget, ctx: &mut BuildCtx<'_>) {
        View::teardown(&self.host(), element, ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::{
        Color, InputEvent, PaintScene, Point, PointerButton, PointerEvent, PointerPhase, Rect,
        Size, text::TextContext,
    };
    use frust::{Brightness, FrameTime};
    use frust_core::RenderRoot;
    use std::any::Any;

    #[test]
    fn standard_resolves_the_reference_s_surface_roles() {
        let theme = crate::baseline().with_brightness(Brightness::Light);
        let s = theme.scheme();
        let colors = MenuColors::resolve(Some(&theme), MenuColorStyle::Standard);
        assert_eq!(colors.container, s.surface_container_low);
        assert_eq!(colors.content, s.on_surface);
        assert_eq!(colors.icon_content, s.on_surface_variant);
        assert_eq!(colors.supporting_content, s.on_surface_variant);
        assert_eq!(colors.selected_container, s.tertiary_container);
        assert_eq!(colors.selected_content, s.on_tertiary_container);
        assert_eq!(colors.state_layer, s.on_surface);
        assert_eq!(colors.divider, s.outline_variant);
    }

    #[test]
    fn vibrant_promotes_the_whole_surface_to_the_tertiary_family() {
        let theme = crate::baseline().with_brightness(Brightness::Light);
        let s = theme.scheme();
        let colors = MenuColors::resolve(Some(&theme), MenuColorStyle::Vibrant);
        assert_eq!(colors.container, s.tertiary_container);
        assert_eq!(colors.content, s.on_tertiary_container);
        assert_eq!(colors.icon_content, s.on_tertiary_container);
        assert_eq!(colors.selected_container, s.tertiary);
        assert_eq!(colors.selected_content, s.on_tertiary);
        assert_eq!(colors.state_layer, s.on_tertiary_container);
        assert_eq!(
            colors.divider,
            with_alpha(s.on_tertiary_container, VIBRANT_DIVIDER_ALPHA)
        );
    }

    #[test]
    fn the_two_styles_never_resolve_to_the_same_surface() {
        let theme = crate::baseline();
        let standard = MenuColors::resolve(Some(&theme), MenuColorStyle::Standard);
        let vibrant = MenuColors::resolve(Some(&theme), MenuColorStyle::Vibrant);
        assert_ne!(standard.container, vibrant.container);
        assert_ne!(standard.content, vibrant.content);
        assert_eq!(MenuColorStyle::default(), MenuColorStyle::Standard);
    }

    #[test]
    fn the_unthemed_fallbacks_are_the_m3_baseline_light_values() {
        let light = crate::baseline().with_brightness(Brightness::Light);
        for style in [MenuColorStyle::Standard, MenuColorStyle::Vibrant] {
            assert_eq!(
                MenuColors::resolve(None, style),
                MenuColors::resolve(Some(&light), style),
                "{style:?}"
            );
        }
    }

    #[test]
    fn the_colors_follow_a_live_brightness_flip() {
        let light = crate::baseline().with_brightness(Brightness::Light);
        let dark = crate::baseline().with_brightness(Brightness::Dark);
        assert_ne!(
            MenuColors::resolve(Some(&light), MenuColorStyle::Standard).container,
            MenuColors::resolve(Some(&dark), MenuColorStyle::Standard).container
        );
    }

    #[test]
    fn the_foreground_ladder_is_disabled_then_destructive_then_selected() {
        let colors = MenuColors::resolve(None, MenuColorStyle::Standard);
        assert_eq!(
            colors.entry_foreground(false, false, false),
            with_alpha(colors.content, DISABLED_CONTENT_OPACITY),
            "disabled outranks everything"
        );
        assert_eq!(
            colors.entry_foreground(false, true, true),
            with_alpha(colors.content, DISABLED_CONTENT_OPACITY)
        );
        assert_eq!(
            colors.entry_foreground(true, true, true),
            FALLBACK_ERROR,
            "destructive outranks selected"
        );
        assert_eq!(
            colors.entry_foreground(true, false, true),
            colors.selected_content
        );
        assert_eq!(colors.entry_foreground(true, false, false), colors.content);

        // Icons take the same ladder over their own idle role.
        assert_eq!(
            colors.icon_foreground(true, false, false),
            colors.icon_content
        );
        assert_eq!(
            colors.icon_foreground(true, false, true),
            colors.selected_content
        );
        assert_eq!(colors.icon_foreground(true, true, false), FALLBACK_ERROR);

        // Supporting text dims `content`, not `supporting_content` — the
        // reference's own asymmetry.
        assert_eq!(
            colors.supporting_foreground(false, false),
            with_alpha(colors.content, DISABLED_CONTENT_OPACITY)
        );
        assert_eq!(
            colors.supporting_foreground(true, false),
            colors.supporting_content
        );
        assert_eq!(
            colors.supporting_foreground(true, true),
            colors.selected_content
        );
    }

    #[test]
    fn a_non_material_theme_still_resolves_every_role() {
        // An app is free to thread a bare theme; nothing here may panic.
        let bare = Theme::neutral();
        let _ = MenuColors::resolve(Some(&bare), MenuColorStyle::Vibrant);
    }

    // ---- the anchored menu -----------------------------------------------

    const WINDOW: Size = Size::new(400.0, 600.0);
    const ANCHOR: Rect = Rect::new(40.0, 100.0, 160.0, 140.0);

    /// What the mounted menu reported.
    #[derive(Default)]
    struct AppState {
        dismissed: u32,
        selections: Vec<MenuSelection>,
    }

    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn fill_rounded_rect(&mut self, origin: Point, size: Size, _radius: f64, _color: Color) {
            self.rrects.push((origin, size));
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
    }

    /// A menu mounted the documented way: the top child of a full-area
    /// [`frust::Stack`], with its own anchor cell already captured.
    struct Harness {
        root: RenderRoot<AppState, frust::StackView<AppState>>,
        state: AppState,
        tcx: TextContext,
        anchor: OverlayAnchor,
    }

    impl Harness {
        fn new(nodes: Vec<MenuNode>) -> Self {
            let anchor = OverlayAnchor::new();
            anchor.set(ANCHOR);
            let mut h = Harness {
                root: RenderRoot::new(),
                state: AppState::default(),
                tcx: TextContext::new(),
                anchor,
            };
            h.pass(nodes);
            h
        }

        fn pass(&mut self, nodes: Vec<MenuNode>) {
            let anchor = self.anchor.clone();
            let mut logic = move |_s: &mut AppState| {
                frust::stack().child(
                    menu(nodes.clone(), |s: &mut AppState, selection| {
                        s.selections.push(selection)
                    })
                    .anchor(&anchor)
                    .on_dismiss(|s: &mut AppState| s.dismissed += 1),
                )
            };
            self.root.rebuild(&mut logic, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
        }

        fn paint(&mut self) -> Recorder {
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, FrameTime::ZERO);
            rec
        }

        fn pointer(&mut self, phase: PointerPhase, x: f64, y: f64) {
            self.root.event(
                &mut self.state,
                &InputEvent::Pointer(PointerEvent {
                    phase,
                    position: Point::new(x, y),
                    button: PointerButton::Primary,
                }),
            );
        }

        fn click(&mut self, x: f64, y: f64) {
            self.pointer(PointerPhase::Down, x, y);
            self.pointer(PointerPhase::Up, x, y);
        }
    }

    /// The window-space centre of the menu's first row, given the placement
    /// above: below the anchor, leading edges flush, at the anchor gap.
    fn first_row_center() -> Point {
        Point::new(
            ANCHOR.x0 + MENU_MIN_WIDTH / 2.0,
            ANCHOR.y1
                + OVERLAY_ANCHOR_GAP
                + MENU_SURFACE_V_PADDING
                + (MENU_ITEM_GAP + MENU_ROW_MIN_HEIGHT) / 2.0,
        )
    }

    #[test]
    fn the_menu_places_its_panel_under_the_trigger_at_the_anchor_gap() {
        let mut h = Harness::new(vec![menu_entry("Cut").into()]);
        let rec = h.paint();
        let surface = rec.rrects.first().expect("the menu surface");
        assert_eq!(
            surface.0,
            Point::new(ANCHOR.x0, ANCHOR.y1 + OVERLAY_ANCHOR_GAP),
            "the reference's `bottomStart`, at `anchorOffset`"
        );
    }

    #[test]
    fn a_press_on_a_row_reports_through_the_host_and_never_dismisses() {
        let mut h = Harness::new(vec![menu_entry("Cut").into()]);
        let c = first_row_center();
        h.click(c.x, c.y);
        assert_eq!(h.state.selections.len(), 1);
        assert_eq!(h.state.selections[0].label, "Cut");
        assert_eq!(
            h.state.dismissed, 0,
            "a row press is the panel's, not a dismiss"
        );
    }

    #[test]
    fn a_press_outside_the_panel_light_dismisses_the_whole_menu() {
        let mut h = Harness::new(vec![
            menu_submenu("Share", vec![menu_entry("Copy link").into()]).into(),
        ]);
        // Open the cascade first, so the dismissal has a chain to take.
        let c = first_row_center();
        h.click(c.x, c.y);
        assert!(h.state.selections.is_empty(), "opening is not selecting");

        h.pointer(PointerPhase::Down, 5.0, 5.0);
        assert_eq!(h.state.dismissed, 1);
        assert!(
            h.state.selections.is_empty(),
            "the dismissing press never reached a row"
        );
    }

    #[test]
    fn a_closed_kept_mounted_menu_paints_nothing_once_the_host_settles() {
        let anchor = OverlayAnchor::new();
        anchor.set(ANCHOR);
        let mut root: RenderRoot<AppState, MenuView<AppState>> = RenderRoot::new();
        let mut state = AppState::default();
        let mut tcx = TextContext::new();
        let cell = anchor.clone();
        let mut closed = move |_s: &mut AppState| {
            menu(vec![menu_entry("Cut").into()], |_s: &mut AppState, _| {})
                .anchor(&cell)
                .open(false)
        };
        root.rebuild(&mut closed, &mut state);
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);
        let mut rec = Recorder::default();
        root.paint(&mut rec, FrameTime::ZERO);
        assert!(
            rec.rrects.is_empty(),
            "a host mounted closed shows no panel at all"
        );
    }

    #[test]
    fn the_metrics_are_the_reference_s_menu_theme_defaults() {
        assert_eq!(MENU_MIN_WIDTH, 112.0);
        assert_eq!(MENU_MAX_WIDTH, 280.0);
        assert_eq!(MENU_MAX_HEIGHT, 320.0);
        assert_eq!(MENU_ROW_MIN_HEIGHT, 48.0);
        assert_eq!(MENU_ICON_SIZE, 24.0);
        assert_eq!(MENU_SURFACE_V_PADDING, 8.0);
        assert_eq!(MENU_SURFACE_H_PADDING, 8.0);
        assert_eq!(MENU_SECTION_GAP, 8.0);
        assert_eq!(MENU_ROW_H_PADDING, 12.0);
        assert_eq!(MENU_ICON_GAP, 12.0);
        assert_eq!(MENU_ITEM_GAP, 4.0);
        assert_eq!(MENU_GROUP_LABEL_H_PADDING, 12.0);
        assert_eq!(MENU_GROUP_LABEL_V_PADDING, 8.0);
        assert_eq!(MENU_CONTAINER_RADIUS, 16.0);
        assert_eq!(MENU_ITEM_RADIUS, 12.0);
        assert_eq!(MENU_SUBMENU_GAP, 4.0);
        assert_eq!(SELECTED_CHECK_SCALE, 0.9);
        // The reference's own 0.38, which this crate already names.
        assert_eq!(DISABLED_CONTENT_OPACITY, 0.38);
    }
}
