//! `sidebar`: the collapsible app-shell sidebar — a provider, the panel itself,
//! and the header/content/footer/group/menu parts that fill it.
//!
//! Source: `apps/v4/registry/new-york-v4/ui/sidebar.tsx` (shadcn/ui v4, rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved 2026-08-17) — the
//! registry's largest single component: a React context, two CSS custom
//! properties, and twenty-odd `data-slot` parts whose whole look is expressed in
//! `group-data-[collapsible=icon]` / `peer-data-[variant=inset]` variants.
//!
//! Colors come from the theme extension's `--sidebar-*` group
//! ([`ShadcnTokens::sidebar`]), which is exactly what that group was vendored
//! for; every other metric is a Tailwind class translated in place.
//!
//! # The context, ported
//!
//! Upstream, `SidebarProvider` publishes `{state, open, setOpen, toggleSidebar}`
//! through React context and every part reads it. frust has no ambient context,
//! so this port pushes the same three facts down **at view-construction time**:
//! [`sidebar_provider`] takes the two halves as their *concrete* view types
//! ([`SidebarView`], [`SidebarInsetView`]) and stamps `open` plus the shared
//! toggle callback into the panel and the variant/side into the main-content
//! wrapper before erasing them. That makes the provider the single owner of the
//! open state, which is what the controlled contract wants anyway.
//!
//! Two facts do not travel that way:
//!
//! - **Icon mode is width-driven.** The parts inside the panel (`group-data-`
//!   selectors upstream) can be arbitrarily deep and are already erased by the
//!   time the provider sees them, so instead of a pushed flag each part reads
//!   the width it is laid out at: at or below [`SIDEBAR_WIDTH_ICON`] it renders
//!   its icon-mode form — a menu button collapses to a `size-8` square with no
//!   label, and a group label, menu action, badge, and sub-menu take zero space.
//!   The rail width is the panel's own collapsed width, so the rule fires
//!   exactly when `data-collapsible=icon` would.
//! - **A [`sidebar_trigger`] placed outside the panel** (the usual spot: the
//!   inset's own header bar) takes `open` and its toggle callback directly,
//!   since no construction-time path reaches it.
//!
//! # Controlled, and no cookie
//!
//! `open` is the app's: the panel, the rail, and the trigger all report the
//! *requested* state through `on_open_change` and never flip a flag of their
//! own. Upstream also writes a `sidebar_state` cookie on every toggle — this
//! port writes nothing anywhere; persistence is the app's business, and the app
//! already holds the value.
//!
//! # Keyboard
//!
//! Ctrl/Cmd+B toggles, handled by [`SidebarProviderWidget`] after routing. Key
//! events are focus-routed in frust (never hit-tested), so the shortcut fires
//! while focus is inside the provider's own subtree — there is no
//! process-global key hook to register for a genuinely app-wide accelerator, so
//! an app that wants one outside the provider wires its own menu accelerator.
//!
//! # Not in this port
//!
//! - **The mobile sheet.** Upstream swaps the whole panel for a `Sheet` under
//!   `useIsMobile()` (a 768px media query). There is no media/size seam a view
//!   can branch on before layout here, so this is desktop-only; a mobile app
//!   composes [`crate::components::sheet`] itself.
//! - **Icon-mode tooltips.** `SidebarMenuButton`'s `tooltip` prop shows the
//!   label in a hover card while collapsed. Skipped for now; the label is
//!   still the button's accessible name, so a collapsed rail is not mute.
//! - **`SidebarInput`/`SidebarMenuSkeleton`.** Both are one-line restyles of
//!   components this catalog already ships (`input`, `skeleton`); compose them
//!   directly.
//! - **Sub-menu disclosure.** Upstream's collapsible sub-menus are assembled by
//!   the *blocks*, not by `sidebar.tsx`, out of `Collapsible` — compose
//!   [`crate::components::collapsible`] with [`sidebar_menu_button`] as the
//!   trigger and [`sidebar_menu_sub`] as the content.
//! - **Peer recoloring.** `menu-badge`/`menu-action` recolor when the sibling
//!   menu button is hovered (`peer-hover/menu-button:`). Sibling state is not
//!   observable across widgets here, so each part paints its own hover only.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::text::{FontWeight, TextStyle};
use frust::authoring::{
    Action, AnyView, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color,
    ErasedArgCallback, EventCtx, EventResult, InputEvent, Key, LayoutCtx, PaintCtx, PaintScene,
    Point, PointerEvent, PointerPhase, Role, RoundedRect, SemanticsCtx, Shape, Size, ThemeTextType,
    Vec2, View, ViewSeq, Widget, any, build_child, erase_callback_arg, rebuild_child, route_event,
    route_event_single, teardown_child, visit_children,
};
use frust::{
    AnimationController, Axis, Column, CrossAxisAlignment, CursorIcon, Curve, EdgeInsets, FlexView,
    Padding, RubberBand, SizedBox, Theme, flexible, inflexible, scroll_view,
};

use crate::components::input::FALLBACK;
use crate::components::native_select::activates;
use crate::hit::{inside, presses};
use crate::style::{
    ACTIVE_CURSOR, BORDER_WIDTH, DISABLED_CURSOR, PATH_TOLERANCE, SHADOW_SM, TEXT_SM, TEXT_XS,
    disabled_tint, draw_focus_ring, draw_shadow, ring_color, scale_alpha,
};
use crate::text::{LabelRun, SHAPING_INK, themed_family};
use crate::tokens::{ShadcnSidebar, ShadcnTokens};

// ---- Metrics --------------------------------------------------------------

/// Expanded panel width: `--sidebar-width: 16rem`.
pub const SIDEBAR_WIDTH: f64 = 256.0;
/// Collapsed panel width in `icon` mode: `--sidebar-width-icon: 3rem`. Also the
/// threshold that puts every part inside the panel into its icon-mode form (see
/// the [module docs](self)).
pub const SIDEBAR_WIDTH_ICON: f64 = 48.0;

/// Inset of the panel inside its own box for the `floating`/`inset` variants:
/// `p-2`.
const PANEL_INSET: f64 = 8.0;
/// Collapse/expand duration: `transition-[width] duration-200 ease-linear`.
const COLLAPSE_MS: u64 = 200;
/// Width difference below which a collapse counts as settled.
const WIDTH_EPSILON: f64 = 1e-4;

/// The rail's grabbable band, in logical px.
///
/// Upstream's rail is `w-4` centered on the panel's trailing edge
/// (`-translate-x-1/2`), so half of it hangs outside the panel. A frust widget
/// is only hit-tested inside its own bounds, so this port keeps the **inward**
/// half — the band sits just inside the trailing edge, which is the panel's own
/// padding gutter.
const RAIL_WIDTH: f64 = 8.0;
/// The hairline the rail lights up on hover: `after:w-[2px]
/// hover:after:bg-sidebar-border`.
const RAIL_INDICATOR: f64 = 2.0;

/// `p-2` on the header/footer/group slots.
const SLOT_PAD: f64 = 8.0;
/// `gap-2` between the children of a slot.
const SLOT_GAP: f64 = 8.0;
/// `gap-1` between menu items.
const MENU_GAP: f64 = 4.0;

/// Trigger edge: `size-7`.
const TRIGGER_SIZE: f64 = 28.0;
/// Group/menu action edge: `w-5 aspect-square`.
const ACTION_SIZE: f64 = 20.0;
/// Menu badge box: `h-5 min-w-5 px-1`.
const BADGE_HEIGHT: f64 = 20.0;
/// Horizontal padding inside a badge: `px-1`.
const BADGE_PAD_X: f64 = 4.0;
/// Group label box: `h-8 px-2`.
const GROUP_LABEL_HEIGHT: f64 = 32.0;
/// Horizontal padding inside a group label: `px-2`.
const GROUP_LABEL_PAD_X: f64 = 8.0;
/// `text-sidebar-foreground/70` on a group label.
const GROUP_LABEL_ALPHA: f32 = 0.7;

/// Menu-button padding: `p-2`.
const MENU_BUTTON_PAD: f64 = 8.0;
/// Gap between a menu button's icon and its label: `gap-2`.
const MENU_BUTTON_GAP: f64 = 8.0;
/// A menu button's edge in icon mode: `group-data-[collapsible=icon]:size-8!`.
const MENU_BUTTON_ICON_EDGE: f64 = 32.0;

/// Sub-list left margin: `mx-3.5` (the rule rides this edge).
const SUB_MARGIN_X: f64 = 14.0;
/// Sub-list inner padding: `px-2.5`.
const SUB_PAD_X: f64 = 10.0;
/// Sub-list vertical padding: `py-0.5`.
const SUB_PAD_Y: f64 = 2.0;
/// Separator margin: `mx-2`.
const SEPARATOR_MARGIN_X: f64 = 8.0;

/// The box lucide draws its glyphs in (`viewBox="0 0 24 24"`).
const LUCIDE_VIEWBOX: f64 = 24.0;
/// Lucide's own stroke width, in `viewBox` units (`stroke-width="2"`).
const LUCIDE_STROKE: f64 = 2.0;

/// A view-held, typed open-state callback (erased on build).
type OnOpenChange<State> = Rc<dyn Fn(&mut State, bool)>;

// ---- Variant axes ---------------------------------------------------------

/// Which edge the panel is docked to (`side`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SidebarSide {
    /// `side="left"` — the default.
    #[default]
    Left,
    /// `side="right"`.
    Right,
}

/// The panel's chrome (`variant`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SidebarVariant {
    /// `variant="sidebar"` — a flush panel with a hairline on its trailing
    /// edge. The default.
    #[default]
    Sidebar,
    /// `variant="floating"` — a rounded, bordered, shadowed card inset from the
    /// window edges by `p-2`.
    Floating,
    /// `variant="inset"` — the panel is flush and unbordered; the *main
    /// content* takes the card look instead (see [`sidebar_inset`]).
    Inset,
}

/// How the panel collapses (`collapsible`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SidebarCollapsible {
    /// `collapsible="offcanvas"` — collapses to zero width. The default.
    #[default]
    Offcanvas,
    /// `collapsible="icon"` — collapses to a [`SIDEBAR_WIDTH_ICON`] rail.
    Icon,
    /// `collapsible="none"` — always expanded.
    None,
}

/// A menu button's size ladder (`size`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SidebarMenuButtonSize {
    /// `h-8 text-sm` — the default.
    #[default]
    Default,
    /// `h-7 text-xs`.
    Sm,
    /// `h-12 text-sm`.
    Lg,
}

impl SidebarMenuButtonSize {
    /// This size's row height, in logical px.
    fn height(self) -> f64 {
        match self {
            SidebarMenuButtonSize::Default => 32.0,
            SidebarMenuButtonSize::Sm => 28.0,
            SidebarMenuButtonSize::Lg => 48.0,
        }
    }

    /// This size's label size, in logical px.
    fn text(self) -> f64 {
        match self {
            SidebarMenuButtonSize::Sm => TEXT_XS,
            _ => TEXT_SM,
        }
    }
}

/// A sub-menu button's size ladder (`size`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SidebarMenuSubSize {
    /// `text-xs`.
    Sm,
    /// `text-sm` — the default.
    #[default]
    Md,
}

impl SidebarMenuSubSize {
    /// This size's label size, in logical px. Both sub sizes share `h-7`.
    fn text(self) -> f64 {
        match self {
            SidebarMenuSubSize::Sm => TEXT_XS,
            SidebarMenuSubSize::Md => TEXT_SM,
        }
    }
}

/// The resolved metrics a [`SidebarMenuButtonWidget`] lays itself out from —
/// what the two public button views (menu and sub-menu) each fold their own
/// size axis into, so one widget serves both.
#[derive(Clone, Copy, Debug, PartialEq)]
struct MenuMetrics {
    /// Row height (`h-8`/`h-7`/`h-12`).
    height: f64,
    /// Label size.
    text: f64,
    /// Horizontal padding (`p-2` / `px-2`).
    pad: f64,
    /// Whether the row disappears in icon mode — sub-menu rows do
    /// (`group-data-[collapsible=icon]:hidden`), top-level rows collapse to a
    /// square instead.
    hides_in_icon_mode: bool,
}

// ---- Token resolution -----------------------------------------------------

/// The `--sidebar-*` group for this pass: the theme extension's table at the
/// theme's own brightness, or the `neutral` light preset's when nothing is
/// threaded.
///
/// The documented ladder with no explicit rung — a sidebar color is never a
/// builder value on any part of this component.
pub(crate) fn sidebar_tokens(theme: Option<&Theme>) -> ShadcnSidebar {
    match theme.and_then(|t| t.extension::<ShadcnTokens>().map(|x| (x, t.brightness))) {
        Some((tokens, brightness)) => tokens.sidebar(brightness),
        None => FALLBACK.sidebar,
    }
}

/// Whether a part laid out under `bc` is inside a collapsed icon rail.
///
/// The port's stand-in for `group-data-[collapsible=icon]` — see the
/// [module docs](self).
fn icon_mode(bc: &BoxConstraints) -> bool {
    let width = bc.max().width;
    width.is_finite() && width <= SIDEBAR_WIDTH_ICON
}

/// Whether `key` is the sidebar shortcut's letter.
///
/// A platform that reports the unmodified text for a chorded key sends `b`/`B`;
/// one that reports the control character sends C0 `STX` (`0x02`), which is what
/// Ctrl+B produces. Both mean the same press.
fn is_shortcut_key(key: &Key) -> bool {
    match key {
        Key::Character(text) => text.eq_ignore_ascii_case("b") || text == "\u{2}",
        Key::Named(_) => false,
    }
}

/// A fresh collapse ramp at the source's own `duration-200 ease-linear`.
fn collapse_controller() -> AnimationController {
    AnimationController::new(Duration::from_millis(COLLAPSE_MS)).with_curve(Curve::Linear)
}

/// Paint lucide's `panel-left` glyph — a rounded square with a vertical rule a
/// quarter of the way across — centered on `center`, `extent` px on a side.
fn draw_panel_left(scene: &mut dyn PaintScene, center: Point, extent: f64, color: Color) {
    let scale = extent / LUCIDE_VIEWBOX;
    let stroke = LUCIDE_STROKE * scale;
    // `<rect x="3" y="3" width="18" height="18" rx="2" />`, viewBox-relative to
    // its center (12, 12).
    let half = 9.0 * scale;
    let rr = RoundedRect::new(-half, -half, half, half, 2.0 * scale);
    scene.stroke_path(
        center,
        &rr.to_path(PATH_TOLERANCE),
        stroke,
        &Brush::Solid(color),
    );
    // `<path d="M9 3v18" />`.
    let mut divider = BezPath::new();
    divider.move_to(Point::new(-3.0 * scale, -half));
    divider.line_to(Point::new(-3.0 * scale, half));
    scene.stroke_path(center, &divider, stroke, &Brush::Solid(color));
}

/// Interleave `children` with a `gap`-tall spacer — the flex `gap` stand-in this
/// catalog uses throughout (see [`crate::components::card`]).
fn interleave<State: 'static>(children: Vec<AnyView<State>>, gap: f64) -> Vec<AnyView<State>> {
    let mut out = Vec::with_capacity(children.len().saturating_mul(2));
    for (i, child) in children.into_iter().enumerate() {
        if i > 0 {
            out.push(any(SizedBox(None, Some(gap))));
        }
        out.push(child);
    }
    out
}

// ---- Provider -------------------------------------------------------------

/// A declarative sidebar provider: the app-shell row. See the
/// [module docs](self).
pub struct SidebarProviderView<State: 'static> {
    sidebar: AnyView<State>,
    inset: AnyView<State>,
    side: SidebarSide,
    variant: SidebarVariant,
    open: bool,
    on_open_change: OnOpenChange<State>,
}

/// Host `sidebar` beside `inset` as the app shell, with the panel's open state
/// controlled by `open` and every toggle reported through
/// `on_open_change(state, next_open)`.
///
/// The provider stamps `open` and the toggle callback into the panel (so its
/// rail and its width follow the app's value), and the panel's variant/side into
/// the main-content wrapper (so `variant="inset"` gets its card look). Ctrl/Cmd+B
/// toggles — see the [module docs](self) for the routing caveat.
pub fn sidebar_provider<State: 'static, F>(
    sidebar: SidebarView<State>,
    inset: SidebarInsetView<State>,
    open: bool,
    on_open_change: F,
) -> SidebarProviderView<State>
where
    F: Fn(&mut State, bool) + 'static,
{
    let on_open_change: OnOpenChange<State> = Rc::new(on_open_change);
    let side = sidebar.side;
    let variant = sidebar.variant;
    SidebarProviderView {
        sidebar: any(sidebar.wired(open, Rc::clone(&on_open_change))),
        inset: any(inset.wired(variant, side)),
        side,
        variant,
        open,
        on_open_change,
    }
}

/// The retained widget for a [`SidebarProviderView`].
pub struct SidebarProviderWidget {
    /// `[panel, inset]`. The two never overlap, so this order is the paint
    /// order and the hit-test order both.
    pods: Vec<ChildPod>,
    side: SidebarSide,
    variant: SidebarVariant,
    open: bool,
    on_open_change: ErasedArgCallback<bool>,
}

impl<State: 'static> View<State> for SidebarProviderView<State> {
    type Element = SidebarProviderWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> SidebarProviderWidget {
        SidebarProviderWidget {
            pods: vec![
                build_child(&self.sidebar, ctx),
                build_child(&self.inset, ctx),
            ],
            side: self.side,
            variant: self.variant,
            open: self.open,
            on_open_change: erase_callback_arg(&self.on_open_change),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SidebarProviderWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(&prev.sidebar, &self.sidebar, &mut element.pods[0], ctx);
        flags |= rebuild_child(&prev.inset, &self.inset, &mut element.pods[1], ctx);
        if element.side != self.side {
            element.side = self.side;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.variant != self.variant {
            element.variant = self.variant;
            flags |= ChangeFlags::PAINT;
        }
        element.open = self.open;
        element.on_open_change = erase_callback_arg(&self.on_open_change);
        flags
    }

    fn teardown(&self, element: &mut SidebarProviderWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.sidebar, &mut element.pods[0], ctx);
        teardown_child(&self.inset, &mut element.pods[1], ctx);
    }
}

impl Widget for SidebarProviderWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let max = bc.max();
        let width = if max.width.is_finite() {
            max.width
        } else {
            0.0
        };
        // `min-h-svh`: the shell row fills the window on both axes.
        let height = if max.height.is_finite() {
            max.height
        } else {
            0.0
        };

        let panel = self.pods[0].layout_child(
            ctx,
            &BoxConstraints::new(Size::new(0.0, height), Size::new(width, height)),
        );
        let rest = (width - panel.width).max(0.0);
        let inset_bc = BoxConstraints::new(Size::new(rest, height), Size::new(rest, height));
        self.pods[1].layout_child(ctx, &inset_bc);

        match self.side {
            SidebarSide::Left => {
                self.pods[0].set_origin(Point::ORIGIN);
                self.pods[1].set_origin(Point::new(panel.width, 0.0));
            }
            SidebarSide::Right => {
                self.pods[1].set_origin(Point::ORIGIN);
                self.pods[0].set_origin(Point::new(rest, 0.0));
            }
        }
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if self.variant == SidebarVariant::Inset {
            // `has-data-[variant=inset]:bg-sidebar`: the wrapper itself takes the
            // sidebar surface, so the inset card floats on it.
            let fill = {
                let theme = Theme::from_paint_ctx(ctx);
                sidebar_tokens(theme).background
            };
            scene.fill_rect(ctx.origin(), ctx.size(), fill);
        }
        for pod in &mut self.pods {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let routed = route_event(&mut self.pods, ctx, event);
        if event.is_broadcast() || routed == EventResult::Handled {
            return routed;
        }
        let InputEvent::Key(key) = event else {
            return EventResult::Ignored;
        };
        // Cmd on macOS, Ctrl elsewhere — upstream accepts either.
        if !(key.modifiers.ctrl || key.modifiers.meta) || !is_shortcut_key(&key.key) {
            return EventResult::Ignored;
        }
        let next = !self.open;
        (self.on_open_change)(ctx, next);
        ctx.request_redraw();
        EventResult::Handled
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::Group,
            |_node| {},
            |ctx| {
                for pod in &self.pods {
                    pod.semantics_child(ctx);
                }
            },
        );
    }

    visit_children!(pods);
}

// ---- The panel ------------------------------------------------------------

/// A declarative sidebar panel. See the [module docs](self).
pub struct SidebarView<State: 'static> {
    header: AnyView<State>,
    content: AnyView<State>,
    footer: AnyView<State>,
    side: SidebarSide,
    variant: SidebarVariant,
    collapsible: SidebarCollapsible,
    rail: bool,
    /// Stamped by [`sidebar_provider`]; a bare `sidebar(…)` is always expanded.
    open: bool,
    /// Stamped by [`sidebar_provider`]; without it the rail is inert.
    on_open_change: Option<OnOpenChange<State>>,
}

/// Create a sidebar panel around `content` — normally a [`sidebar_content`],
/// which is the scrolling middle band the header and footer bracket.
pub fn sidebar<State: 'static, V: View<State>>(content: V) -> SidebarView<State> {
    SidebarView {
        header: empty_slot(),
        content: any(content),
        footer: empty_slot(),
        side: SidebarSide::default(),
        variant: SidebarVariant::default(),
        collapsible: SidebarCollapsible::default(),
        rail: false,
        open: true,
        on_open_change: None,
    }
}

/// A zero-sized stand-in for an unset header/footer.
///
/// The panel always holds three pods so the slot structure never changes across
/// a rebuild — an `Option<ChildPod>` appearing or vanishing would be a
/// structural change for every sibling behind it.
fn empty_slot<State: 'static>() -> AnyView<State> {
    any(SizedBox(Some(0.0), Some(0.0)))
}

impl<State: 'static> SidebarView<State> {
    /// Set the header slot (`SidebarHeader`) — `flex-col gap-2 p-2`, natural
    /// height, pinned above the scrolling content.
    pub fn header<V: View<State>>(mut self, header: V) -> Self {
        self.header = any(header);
        self
    }

    /// Set the footer slot (`SidebarFooter`) — the header's mirror, pinned
    /// below the scrolling content.
    pub fn footer<V: View<State>>(mut self, footer: V) -> Self {
        self.footer = any(footer);
        self
    }

    /// Dock the panel to `side` (default [`SidebarSide::Left`]).
    pub fn side(mut self, side: SidebarSide) -> Self {
        self.side = side;
        self
    }

    /// Select the chrome `variant` (default [`SidebarVariant::Sidebar`]).
    pub fn variant(mut self, variant: SidebarVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Select how the panel collapses (default
    /// [`SidebarCollapsible::Offcanvas`]).
    pub fn collapsible(mut self, collapsible: SidebarCollapsible) -> Self {
        self.collapsible = collapsible;
        self
    }

    /// Show the edge rail (`SidebarRail`): a grabbable band just inside the
    /// panel's trailing edge that toggles the panel.
    ///
    /// Upstream's rail is an absolutely-positioned child of `<Sidebar>`; this
    /// port makes it a property of the panel because absolute positioning has no
    /// equivalent in the panel's own column layout, and because the rail needs
    /// the panel's toggle wiring anyway. An offcanvas panel keeps the rail's
    /// width when collapsed, so there is still something to grab.
    pub fn rail(mut self, rail: bool) -> Self {
        self.rail = rail;
        self
    }

    /// Stamp the provider's controlled state and shared toggle into the panel.
    fn wired(mut self, open: bool, on_open_change: OnOpenChange<State>) -> Self {
        self.open = open;
        self.on_open_change = Some(on_open_change);
        self
    }
}

/// The retained widget for a [`SidebarView`].
///
/// The collapse animates the panel's own width, so a running collapse asks for
/// `request_layout` (the provider re-splits the row from it) rather than a bare
/// repaint — the same shape [`crate::components::collapsible`]'s reveal has.
pub struct SidebarWidget {
    /// `[header, content, footer]`, always three.
    pods: Vec<ChildPod>,
    side: SidebarSide,
    variant: SidebarVariant,
    collapsible: SidebarCollapsible,
    rail: bool,
    open: bool,
    /// The width `layout` last used, advanced in `paint`.
    width: f64,
    /// The width the running collapse started from.
    from: f64,
    /// The width the running collapse is heading for.
    target: f64,
    anim: AnimationController,
    rail_hovered: bool,
    rail_pressed: bool,
    rail_captured: bool,
    on_open_change: Option<ErasedArgCallback<bool>>,
}

impl<State: 'static> View<State> for SidebarView<State> {
    type Element = SidebarWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> SidebarWidget {
        let mut widget = SidebarWidget {
            pods: vec![
                build_child(&self.header, ctx),
                build_child(&self.content, ctx),
                build_child(&self.footer, ctx),
            ],
            side: self.side,
            variant: self.variant,
            collapsible: self.collapsible,
            rail: self.rail,
            open: self.open,
            width: 0.0,
            from: 0.0,
            target: 0.0,
            anim: collapse_controller(),
            rail_hovered: false,
            rail_pressed: false,
            rail_captured: false,
            on_open_change: self.on_open_change.as_ref().map(erase_callback_arg),
        };
        // A panel built collapsed starts collapsed rather than animating shut.
        widget.width = widget.target_width();
        widget.from = widget.width;
        widget.target = widget.width;
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SidebarWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(&prev.header, &self.header, &mut element.pods[0], ctx);
        flags |= rebuild_child(&prev.content, &self.content, &mut element.pods[1], ctx);
        flags |= rebuild_child(&prev.footer, &self.footer, &mut element.pods[2], ctx);

        let geometry_changed = element.open != self.open
            || element.collapsible != self.collapsible
            || element.variant != self.variant
            || element.rail != self.rail;
        element.open = self.open;
        element.collapsible = self.collapsible;
        element.variant = self.variant;
        element.rail = self.rail;
        if element.side != self.side {
            element.side = self.side;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if geometry_changed {
            element.retarget();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        element.on_open_change = self.on_open_change.as_ref().map(erase_callback_arg);
        flags
    }

    fn teardown(&self, element: &mut SidebarWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.header, &mut element.pods[0], ctx);
        teardown_child(&self.content, &mut element.pods[1], ctx);
        teardown_child(&self.footer, &mut element.pods[2], ctx);
    }
}

impl SidebarWidget {
    /// The panel's inset inside its own box: `p-2` for the variants that float
    /// the panel off the window edges.
    fn panel_inset(&self) -> f64 {
        match self.variant {
            SidebarVariant::Floating | SidebarVariant::Inset => PANEL_INSET,
            SidebarVariant::Sidebar => 0.0,
        }
    }

    /// The outer width the current open/collapsible pair asks for.
    fn target_width(&self) -> f64 {
        let inset = self.panel_inset() * 2.0;
        if self.open || self.collapsible == SidebarCollapsible::None {
            return SIDEBAR_WIDTH + inset;
        }
        match self.collapsible {
            SidebarCollapsible::Icon => SIDEBAR_WIDTH_ICON + inset,
            // A rail-less offcanvas panel really does vanish; with a rail, the
            // band stays behind so the panel can be pulled back out.
            SidebarCollapsible::Offcanvas if self.rail => RAIL_WIDTH,
            SidebarCollapsible::Offcanvas => 0.0,
            SidebarCollapsible::None => SIDEBAR_WIDTH + inset,
        }
    }

    /// Start (or restart) the collapse toward whatever the current state wants,
    /// from wherever the last one had reached.
    fn retarget(&mut self) {
        let target = self.target_width();
        if (target - self.target).abs() <= WIDTH_EPSILON {
            return;
        }
        self.from = self.width;
        self.target = target;
        self.anim = collapse_controller();
        self.anim.forward();
    }

    /// The panel's own box inside the widget: the whole widget for the flush
    /// variant, inset by `p-2` for the others.
    fn panel_box(&self, size: Size) -> (Point, Size) {
        let inset = self.panel_inset();
        (
            Point::new(inset, inset),
            Size::new(
                (size.width - inset * 2.0).max(0.0),
                (size.height - inset * 2.0).max(0.0),
            ),
        )
    }

    /// Whether widget-local `pos` is on the rail band.
    fn over_rail(&self, pos: Point, size: Size) -> bool {
        if !self.rail || !inside(pos, size) {
            return false;
        }
        match self.side {
            SidebarSide::Left => pos.x >= size.width - RAIL_WIDTH,
            SidebarSide::Right => pos.x < RAIL_WIDTH,
        }
    }

    /// Report the open state the rail asks for. The widget never flips its own
    /// flag — the next rebuild does, from the app.
    fn toggle(&mut self, ctx: &mut EventCtx) {
        let next = !self.open;
        if let Some(callback) = &mut self.on_open_change {
            callback(ctx, next);
        }
    }
}

impl Widget for SidebarWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let max = bc.max();
        let inset = self.panel_inset();
        let width = self.width.min(if max.width.is_finite() {
            max.width
        } else {
            self.width
        });
        let inner_width = (width - inset * 2.0).max(0.0);
        // The slots live inside the panel box, so a `p-2` variant's inset comes
        // off both axes before anything is divided up.
        let inner_height = if max.height.is_finite() {
            (max.height - inset * 2.0).max(0.0)
        } else {
            f64::INFINITY
        };
        let slot_bc = |height: f64| {
            BoxConstraints::new(
                Size::new(inner_width, 0.0),
                Size::new(inner_width, if height.is_finite() { height } else { 0.0 }),
            )
        };

        let header = self.pods[0].layout_child(ctx, &slot_bc(inner_height));
        let footer = self.pods[2].layout_child(ctx, &slot_bc(inner_height));
        // `flex-1 min-h-0`: the content band takes whatever the header and footer
        // leave. With no bounded height to divide, it falls back to its own.
        let content_bc = if inner_height.is_finite() {
            let rest = (inner_height - header.height - footer.height).max(0.0);
            BoxConstraints::new(Size::new(inner_width, rest), Size::new(inner_width, rest))
        } else {
            slot_bc(f64::INFINITY)
        };
        let content = self.pods[1].layout_child(ctx, &content_bc);

        self.pods[0].set_origin(Point::new(inset, inset));
        self.pods[1].set_origin(Point::new(inset, inset + header.height));
        self.pods[2].set_origin(Point::new(inset, inset + header.height + content.height));

        let height = if max.height.is_finite() {
            max.height
        } else {
            header.height + content.height + footer.height + inset * 2.0
        };
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if !ctx.is_hovered() {
            // Authoritative: no pointer anywhere in the panel means no rail hover,
            // whatever the last `Move` latched.
            self.rail_hovered = false;
        }
        let (origin, size) = (ctx.origin(), ctx.size());
        let now = ctx.frame_time();
        // One scope for every theme read: `&Theme` borrows the context, and the
        // child paints below need it mutably.
        let (reduce_motion, radius, tokens) = {
            let theme = Theme::from_paint_ctx(ctx);
            (
                theme.is_some_and(|t| t.motion.reduce_motion),
                ShadcnTokens::resolve_radius(None, theme).lg,
                sidebar_tokens(theme),
            )
        };

        let next = if reduce_motion {
            if self.anim.is_animating() {
                self.anim.stop();
            }
            self.target
        } else if self.anim.is_animating() {
            if self.anim.advance(now) {
                // Width is layout, not paint: the provider re-splits the row from
                // it, so a bare `request_frame` would leave the shell unresized.
                ctx.request_layout();
            }
            let ramp = self.anim.value_clamped();
            self.from + (self.target - self.from) * ramp
        } else {
            self.width
        };
        if (next - self.width).abs() > WIDTH_EPSILON {
            self.width = next;
            ctx.request_layout();
        }

        // The freshly advanced width, not the one the last layout used: the
        // relayout this paint just asked for lands after it, so painting the
        // stale width would leave the panel a frame behind its own animation —
        // in both directions, collapsing and expanding alike. A mid-expand
        // frame can briefly draw wider than this widget's own laid-out box,
        // but never visibly: `[panel, inset]` is also the paint order (see
        // `SidebarProviderWidget::pods`), and the inset repaints its own full
        // box, uncropped, right after — covering exactly the strip the panel
        // spilled into.
        let live = Size::new(self.width, size.height);
        let (panel_origin, panel_size) = self.panel_box(live);
        let panel_origin = origin + panel_origin.to_vec2();
        if panel_size.width > 0.0 {
            match self.variant {
                SidebarVariant::Floating => {
                    draw_shadow(scene, panel_origin, panel_size, radius, SHADOW_SM, None);
                    scene.fill_rounded_rect(panel_origin, panel_size, radius, tokens.background);
                    let half = BORDER_WIDTH / 2.0;
                    let rr = RoundedRect::new(
                        half,
                        half,
                        panel_size.width - half,
                        panel_size.height - half,
                        (radius - half).max(0.0),
                    );
                    scene.stroke_path(
                        panel_origin,
                        &rr.to_path(PATH_TOLERANCE),
                        BORDER_WIDTH,
                        &Brush::Solid(tokens.border),
                    );
                }
                SidebarVariant::Sidebar => {
                    scene.fill_rect(panel_origin, panel_size, tokens.background);
                    // `border-r` on a left panel, `border-l` on a right one.
                    let x = match self.side {
                        SidebarSide::Left => panel_origin.x + panel_size.width - BORDER_WIDTH,
                        SidebarSide::Right => panel_origin.x,
                    };
                    scene.fill_rect(
                        Point::new(x, panel_origin.y),
                        Size::new(BORDER_WIDTH, panel_size.height),
                        tokens.border,
                    );
                }
                SidebarVariant::Inset => {
                    // Flush and unbordered: the provider's wrapper already paints
                    // this same surface behind it.
                    scene.fill_rect(panel_origin, panel_size, tokens.background);
                }
            }

            // `overflow-hidden`: a mid-collapse panel clips its slots rather than
            // spilling them into the main content.
            scene.push_clip(panel_origin, panel_size);
            for pod in &mut self.pods {
                pod.paint_child(ctx, scene);
            }
            scene.pop_clip();
        }

        if self.rail && (self.rail_hovered || self.rail_pressed) {
            let x = match self.side {
                SidebarSide::Left => origin.x + live.width - RAIL_WIDTH / 2.0,
                SidebarSide::Right => origin.x + RAIL_WIDTH / 2.0,
            };
            scene.fill_rect(
                Point::new(x - RAIL_INDICATOR / 2.0, origin.y),
                Size::new(RAIL_INDICATOR, live.height),
                tokens.border,
            );
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // The rail is the one part that goes *before* routing: upstream's is
        // `z-20`, sitting over the panel's own content (and mostly outside it), so
        // the band belongs to the rail rather than to whatever scrolls underneath.
        // Everything else routes to the children first, and the panel claims
        // nothing of its own afterwards.
        if !event.is_broadcast()
            && let InputEvent::Pointer(p) = event
            && self.rail
            && (self.rail_captured || self.over_rail(p.position, ctx.size()))
        {
            return self.rail_event(ctx, p);
        }
        route_event(&mut self.pods, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::Group,
            |node| node.set_expanded(self.open),
            |ctx| {
                // A fully collapsed panel reaches no input, so it publishes no
                // controls either — the same input-parity rule the navigator
                // follows.
                if self.width > RAIL_WIDTH {
                    for pod in &self.pods {
                        pod.semantics_child(ctx);
                    }
                }
            },
        );
    }

    visit_children!(pods);
}

impl SidebarWidget {
    /// The rail's own pointer handling — the band's press/hover/cursor contract,
    /// split out so [`Widget::event`] reads as the one routing decision it makes.
    fn rail_event(&mut self, ctx: &mut EventCtx, p: &PointerEvent) -> EventResult {
        let (size, position) = (ctx.size(), p.position);
        match p.phase {
            PointerPhase::Move => {
                if self.rail_captured {
                    ctx.set_cursor(CursorIcon::ColResize);
                    let over = self.over_rail(position, size);
                    if self.rail_pressed != over {
                        self.rail_pressed = over;
                        ctx.request_redraw();
                    }
                    return EventResult::Handled;
                }
                let over = self.over_rail(position, size);
                if over {
                    ctx.claim_hover();
                    // `cursor-w-resize`/`cursor-e-resize` upstream; frust's cursor
                    // set has no directional resize shapes, so both fold onto the
                    // horizontal-resize one.
                    ctx.set_cursor(CursorIcon::ColResize);
                }
                if self.rail_hovered != over {
                    self.rail_hovered = over;
                    ctx.request_redraw();
                }
                EventResult::Ignored
            }
            PointerPhase::Down => {
                if !presses(p) || !self.over_rail(position, size) {
                    return EventResult::Ignored;
                }
                self.rail_pressed = true;
                self.rail_captured = true;
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.rail_captured {
                    return EventResult::Ignored;
                }
                self.rail_captured = false;
                let armed = self.rail_pressed;
                self.rail_pressed = false;
                if armed && self.over_rail(position, size) {
                    self.toggle(ctx);
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.rail_captured {
                    return EventResult::Ignored;
                }
                // Flags and a redraw only: a Cancel arm never touches app state.
                self.rail_captured = false;
                self.rail_pressed = false;
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }
}

// ---- The main-content wrapper ---------------------------------------------

/// A declarative main-content wrapper (`SidebarInset`). See [`sidebar_inset`].
pub struct SidebarInsetView<State: 'static> {
    child: AnyView<State>,
    variant: SidebarVariant,
    side: SidebarSide,
}

/// Wrap the app's main content as the sidebar's sibling: `bg-background`,
/// filling whatever the panel leaves.
///
/// Under [`SidebarVariant::Inset`] it also takes the card look the variant is
/// named for — `m-2 rounded-xl shadow-sm`, with the margin dropped on the edge
/// facing the panel (whose own `p-2` supplies that gap).
///
/// **The single-child case (every shipped call site) receives the inset's own
/// bounded constraints directly** — [`SidebarInsetWidget::layout`] always hands
/// its child a finite `0..=height` on the main axis, and a lone child is laid
/// out against that box with nothing in between. A multi-child call falls back
/// to stacking the children in a plain [`Column`], whose *inflexible* children
/// are laid out under an unbounded main axis (`FlexWidget`'s documented
/// shrink-wrap semantics) — wrapping more than one child that itself hosts a
/// `scroll_view` or an overlay host is unsupported; give `sidebar_inset` a
/// single child (composing internally, e.g. with `Column`/`FlexView`) instead.
///
/// The list is any [`ViewSeq`] — a tuple of mixed view types (`(a, b, c)`),
/// a `Vec`/array of one type, an `Option`, or `views(iter)` — erased once here,
/// so no element needs `any(..)`.
pub fn sidebar_inset<State: 'static, M>(
    children: impl ViewSeq<State, M>,
) -> SidebarInsetView<State> {
    let mut erased = Vec::new();
    children.extend_views(&mut erased);
    let mut children = erased;
    let child = if children.len() == 1 {
        children.remove(0)
    } else {
        any(Column(children))
    };
    SidebarInsetView {
        child,
        variant: SidebarVariant::default(),
        side: SidebarSide::default(),
    }
}

impl<State: 'static> SidebarInsetView<State> {
    /// Stamp the panel's variant and side in — [`sidebar_provider`]'s job.
    fn wired(mut self, variant: SidebarVariant, side: SidebarSide) -> Self {
        self.variant = variant;
        self.side = side;
        self
    }
}

/// The retained widget for a [`SidebarInsetView`].
pub struct SidebarInsetWidget {
    child: ChildPod,
    variant: SidebarVariant,
    side: SidebarSide,
}

impl<State: 'static> View<State> for SidebarInsetView<State> {
    type Element = SidebarInsetWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> SidebarInsetWidget {
        SidebarInsetWidget {
            child: build_child(&self.child, ctx),
            variant: self.variant,
            side: self.side,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SidebarInsetWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(&prev.child, &self.child, &mut element.child, ctx);
        if element.variant != self.variant || element.side != self.side {
            element.variant = self.variant;
            element.side = self.side;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut SidebarInsetWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl SidebarInsetWidget {
    /// The wrapper's own margins: none at all except under the `inset` variant.
    fn margins(&self) -> EdgeInsets {
        if self.variant != SidebarVariant::Inset {
            return EdgeInsets::all(0.0);
        }
        let mut insets = EdgeInsets::all(PANEL_INSET);
        match self.side {
            SidebarSide::Left => insets.left = 0.0,
            SidebarSide::Right => insets.right = 0.0,
        }
        insets
    }
}

impl Widget for SidebarInsetWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let max = bc.max();
        let width = if max.width.is_finite() {
            max.width
        } else {
            0.0
        };
        let height = if max.height.is_finite() {
            max.height
        } else {
            0.0
        };
        let m = self.margins();
        let inner_width = (width - m.left - m.right).max(0.0);
        let inner_height = (height - m.top - m.bottom).max(0.0);
        let child = self.child.layout_child(
            ctx,
            &BoxConstraints::new(
                Size::new(inner_width, 0.0),
                Size::new(inner_width, inner_height),
            ),
        );
        self.child.set_origin(Point::new(m.left, m.top));
        let height = if max.height.is_finite() {
            height
        } else {
            child.height + m.top + m.bottom
        };
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let (origin, size) = (ctx.origin(), ctx.size());
        let (fill, radius) = {
            let theme = Theme::from_paint_ctx(ctx);
            (
                theme.map_or(FALLBACK.background, |t| t.scheme().surface),
                ShadcnTokens::resolve_radius(None, theme).xl,
            )
        };
        let m = self.margins();
        let box_origin = Point::new(origin.x + m.left, origin.y + m.top);
        let box_size = Size::new(
            (size.width - m.left - m.right).max(0.0),
            (size.height - m.top - m.bottom).max(0.0),
        );
        if self.variant == SidebarVariant::Inset {
            draw_shadow(scene, box_origin, box_size, radius, SHADOW_SM, None);
            scene.fill_rounded_rect(box_origin, box_size, radius, fill);
        } else {
            scene.fill_rect(box_origin, box_size, fill);
        }
        self.child.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        route_event_single(&mut self.child, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::Group,
            |_node| {},
            |ctx| self.child.semantics_child(ctx),
        );
    }

    visit_children!(child);
}

// ---- Composed slots -------------------------------------------------------

/// Create the header slot: `flex-col gap-2 p-2`.
///
/// List parameter: any [`ViewSeq`], as for [`sidebar_inset`].
pub fn sidebar_header<State: 'static, M>(children: impl ViewSeq<State, M>) -> AnyView<State> {
    let mut erased = Vec::new();
    children.extend_views(&mut erased);
    let children = erased;
    any(Padding(
        EdgeInsets::all(SLOT_PAD),
        Column(interleave(children, SLOT_GAP)),
    ))
}

/// Create the footer slot: the header's mirror (`flex-col gap-2 p-2`).
///
/// List parameter: any [`ViewSeq`], as for [`sidebar_inset`].
pub fn sidebar_footer<State: 'static, M>(children: impl ViewSeq<State, M>) -> AnyView<State> {
    sidebar_header(children)
}

/// Create the scrolling middle band: `flex-1 gap-2 overflow-auto`.
///
/// Scrolling itself is the framework's own [`frust::scroll_view`]; this slot
/// contributes the stacking and the gap. Pinned to [`RubberBand`] rather than
/// the workspace's platform-adaptive default — the same deliberate,
/// desktop-first choice [`scroll_area`](crate::components::scroll_area)
/// documents.
///
/// List parameter: any [`ViewSeq`], as for [`sidebar_inset`].
pub fn sidebar_content<State: 'static, M>(children: impl ViewSeq<State, M>) -> AnyView<State> {
    let mut erased = Vec::new();
    children.extend_views(&mut erased);
    let children = erased;
    any(scroll_view(Column(interleave(children, SLOT_GAP))).physics(RubberBand::new()))
}

/// Create a group: `flex-col p-2`, the unit a label plus a menu lives in.
///
/// List parameter: any [`ViewSeq`], as for [`sidebar_inset`].
pub fn sidebar_group<State: 'static, M>(children: impl ViewSeq<State, M>) -> AnyView<State> {
    let mut erased = Vec::new();
    children.extend_views(&mut erased);
    let children = erased;
    any(Padding(EdgeInsets::all(SLOT_PAD), Column(children)))
}

/// Create a menu: a `gap-1` column of [`sidebar_menu_item`] rows.
///
/// List parameter: any [`ViewSeq`], as for [`sidebar_inset`].
pub fn sidebar_menu<State: 'static, M>(items: impl ViewSeq<State, M>) -> AnyView<State> {
    let mut erased = Vec::new();
    items.extend_views(&mut erased);
    let items = erased;
    any(Column(interleave(items, MENU_GAP)))
}

/// Create one menu row: the first child takes every pixel the rest don't want.
///
/// Upstream stacks a menu action and a badge on top of the button with absolute
/// positioning; this port lays the same three parts out as a centered row, so
/// the button is `flexible` and the trailing parts keep their natural width.
///
/// List parameter: any [`ViewSeq`], as for [`sidebar_inset`].
pub fn sidebar_menu_item<State: 'static, M>(children: impl ViewSeq<State, M>) -> FlexView<State> {
    let mut erased = Vec::new();
    children.extend_views(&mut erased);
    let children = erased;
    let children = children
        .into_iter()
        .enumerate()
        .map(|(i, child)| {
            if i == 0 {
                flexible(1, child)
            } else {
                inflexible(child)
            }
        })
        .collect();
    FlexView::new(Axis::Horizontal, children).cross_axis(CrossAxisAlignment::Center)
}

/// Create one sub-menu row — a [`sidebar_menu_item`] inside a
/// [`sidebar_menu_sub`].
///
/// List parameter: any [`ViewSeq`], as for [`sidebar_inset`].
pub fn sidebar_menu_sub_item<State: 'static, M>(
    children: impl ViewSeq<State, M>,
) -> FlexView<State> {
    sidebar_menu_item(children)
}

// ---- Separator ------------------------------------------------------------

/// A declarative sidebar separator. See [`sidebar_separator`].
pub struct SidebarSeparatorView;

/// Create the sidebar's own separator: a `bg-sidebar-border` hairline inset
/// `mx-2` from both edges.
pub fn sidebar_separator() -> SidebarSeparatorView {
    SidebarSeparatorView
}

/// The retained widget for a [`SidebarSeparatorView`].
pub struct SidebarSeparatorWidget;

impl<State: 'static> View<State> for SidebarSeparatorView {
    type Element = SidebarSeparatorWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> SidebarSeparatorWidget {
        SidebarSeparatorWidget
    }

    fn rebuild(
        &self,
        _prev: &Self,
        _element: &mut SidebarSeparatorWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

impl Widget for SidebarSeparatorWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        bc.constrain(Size::new(width, BORDER_WIDTH))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let (origin, size) = (ctx.origin(), ctx.size());
        let color = sidebar_tokens(Theme::from_paint_ctx(ctx)).border;
        let width = (size.width - SEPARATOR_MARGIN_X * 2.0).max(0.0);
        scene.fill_rect(
            Point::new(origin.x + SEPARATOR_MARGIN_X, origin.y),
            Size::new(width, BORDER_WIDTH),
            color,
        );
    }
}

// ---- Group label ----------------------------------------------------------

/// A declarative group label. See [`sidebar_group_label`].
pub struct SidebarGroupLabelView {
    label: String,
}

/// Create a group label: `h-8 px-2 text-xs font-medium
/// text-sidebar-foreground/70`, and nothing at all in icon mode (upstream's
/// `-mt-8 opacity-0`).
pub fn sidebar_group_label(label: impl Into<String>) -> SidebarGroupLabelView {
    SidebarGroupLabelView {
        label: label.into(),
    }
}

/// The retained widget for a [`SidebarGroupLabelView`].
pub struct SidebarGroupLabelWidget {
    label: LabelRun,
    hidden: bool,
}

impl<State: 'static> View<State> for SidebarGroupLabelView {
    type Element = SidebarGroupLabelWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> SidebarGroupLabelWidget {
        SidebarGroupLabelWidget {
            label: LabelRun::new(self.label.clone()),
            hidden: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SidebarGroupLabelWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        if prev.label != self.label {
            element.label.set_content(self.label.clone());
            return ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        ChangeFlags::NONE
    }
}

impl Widget for SidebarGroupLabelWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        self.hidden = icon_mode(bc);
        if self.hidden {
            return Size::ZERO;
        }
        let style = themed_family(
            TextStyle {
                weight: FontWeight::MEDIUM,
                ..TextStyle::new(TEXT_XS as f32, SHAPING_INK)
            },
            Theme::from_layout_ctx(ctx),
            ThemeTextType::LabelMedium,
        );
        self.label.layout(ctx, &style);
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            self.label.size().width + GROUP_LABEL_PAD_X * 2.0
        };
        bc.constrain(Size::new(width, GROUP_LABEL_HEIGHT))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if self.hidden {
            return;
        }
        let ink = {
            let theme = Theme::from_paint_ctx(ctx);
            scale_alpha(sidebar_tokens(theme).foreground, GROUP_LABEL_ALPHA)
        };
        let size = ctx.size();
        let text = self.label.size();
        let origin = ctx.origin() + Vec2::new(GROUP_LABEL_PAD_X, (size.height - text.height) / 2.0);
        self.label.paint(origin, ink, scene);
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        if self.hidden {
            return;
        }
        ctx.push_node(Role::Label, |node| node.set_label(self.label.content()));
    }
}

// ---- Menu badge -----------------------------------------------------------

/// A declarative menu badge. See [`sidebar_menu_badge`].
pub struct SidebarMenuBadgeView {
    label: String,
}

/// Create a trailing menu badge: `h-5 min-w-5 px-1 text-xs tabular-nums`,
/// inert (`pointer-events-none`), and nothing at all in icon mode.
pub fn sidebar_menu_badge(label: impl Into<String>) -> SidebarMenuBadgeView {
    SidebarMenuBadgeView {
        label: label.into(),
    }
}

/// The retained widget for a [`SidebarMenuBadgeView`].
pub struct SidebarMenuBadgeWidget {
    label: LabelRun,
    hidden: bool,
}

impl<State: 'static> View<State> for SidebarMenuBadgeView {
    type Element = SidebarMenuBadgeWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> SidebarMenuBadgeWidget {
        SidebarMenuBadgeWidget {
            label: LabelRun::new(self.label.clone()),
            hidden: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SidebarMenuBadgeWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        if prev.label != self.label {
            element.label.set_content(self.label.clone());
            return ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        ChangeFlags::NONE
    }
}

impl Widget for SidebarMenuBadgeWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        self.hidden = icon_mode(bc);
        if self.hidden {
            return Size::ZERO;
        }
        let style = themed_family(
            TextStyle {
                weight: FontWeight::MEDIUM,
                ..TextStyle::new(TEXT_XS as f32, SHAPING_INK)
            },
            Theme::from_layout_ctx(ctx),
            ThemeTextType::LabelMedium,
        );
        let text = self.label.layout(ctx, &style);
        let width = (text.width + BADGE_PAD_X * 2.0).max(BADGE_HEIGHT);
        bc.constrain(Size::new(width, BADGE_HEIGHT))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if self.hidden {
            return;
        }
        let ink = sidebar_tokens(Theme::from_paint_ctx(ctx)).foreground;
        let size = ctx.size();
        let text = self.label.size();
        let origin = ctx.origin()
            + Vec2::new(
                (size.width - text.width) / 2.0,
                (size.height - text.height) / 2.0,
            );
        self.label.paint(origin, ink, scene);
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        if self.hidden {
            return;
        }
        ctx.push_node(Role::Label, |node| node.set_label(self.label.content()));
    }
}

// ---- Sub-menu list --------------------------------------------------------

/// A declarative sub-menu list. See [`sidebar_menu_sub`].
pub struct SidebarMenuSubView<State: 'static> {
    child: AnyView<State>,
}

/// Create a nested sub-menu: a `gap-1` column indented `mx-3.5 px-2.5 py-0.5`
/// behind a `border-l` rule, and nothing at all in icon mode.
///
/// The disclosure that opens and closes it is the caller's — compose
/// [`crate::components::collapsible`] with a [`sidebar_menu_button`] trigger
/// (upstream's blocks do exactly that; `sidebar.tsx` itself only styles the
/// list).
///
/// List parameter: any [`ViewSeq`], as for [`sidebar_inset`].
pub fn sidebar_menu_sub<State: 'static, M>(
    children: impl ViewSeq<State, M>,
) -> SidebarMenuSubView<State> {
    let mut erased = Vec::new();
    children.extend_views(&mut erased);
    let children = erased;
    SidebarMenuSubView {
        child: any(Column(interleave(children, MENU_GAP))),
    }
}

/// The retained widget for a [`SidebarMenuSubView`].
pub struct SidebarMenuSubWidget {
    child: ChildPod,
    hidden: bool,
}

impl<State: 'static> View<State> for SidebarMenuSubView<State> {
    type Element = SidebarMenuSubWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> SidebarMenuSubWidget {
        SidebarMenuSubWidget {
            child: build_child(&self.child, ctx),
            hidden: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SidebarMenuSubWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        rebuild_child(&prev.child, &self.child, &mut element.child, ctx)
    }

    fn teardown(&self, element: &mut SidebarMenuSubWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for SidebarMenuSubWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        self.hidden = icon_mode(bc);
        let outer = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        let indent = SUB_MARGIN_X + BORDER_WIDTH + SUB_PAD_X;
        let inner = if self.hidden {
            0.0
        } else {
            (outer - indent - SUB_MARGIN_X).max(0.0)
        };
        let child = self.child.layout_child(
            ctx,
            &BoxConstraints::new(Size::new(inner, 0.0), Size::new(inner, f64::INFINITY)),
        );
        self.child.set_origin(Point::new(indent, SUB_PAD_Y));
        if self.hidden {
            return Size::ZERO;
        }
        bc.constrain(Size::new(outer, child.height + SUB_PAD_Y * 2.0))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if self.hidden {
            return;
        }
        let color = sidebar_tokens(Theme::from_paint_ctx(ctx)).border;
        let (origin, size) = (ctx.origin(), ctx.size());
        scene.fill_rect(
            Point::new(origin.x + SUB_MARGIN_X, origin.y),
            Size::new(BORDER_WIDTH, size.height),
            color,
        );
        self.child.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if self.hidden && !event.is_broadcast() {
            return EventResult::Ignored;
        }
        route_event_single(&mut self.child, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        if self.hidden {
            return;
        }
        ctx.push_container(
            Role::Group,
            |_node| {},
            |ctx| self.child.semantics_child(ctx),
        );
    }

    visit_children!(child);
}

// ---- Trigger --------------------------------------------------------------

/// A declarative sidebar trigger. See [`sidebar_trigger`].
pub struct SidebarTriggerView<State: 'static> {
    open: bool,
    on_open_change: OnOpenChange<State>,
}

/// Create the toggle button: a `size-7` ghost button carrying lucide's
/// `panel-left` glyph, reporting the requested state through
/// `on_open_change(state, next_open)`.
///
/// Unlike the panel and its rail, a trigger takes the open state directly — it
/// is normally placed in the main content's own header bar, which no
/// construction-time path from [`sidebar_provider`] reaches.
pub fn sidebar_trigger<State: 'static, F>(
    open: bool,
    on_open_change: F,
) -> SidebarTriggerView<State>
where
    F: Fn(&mut State, bool) + 'static,
{
    SidebarTriggerView {
        open,
        on_open_change: Rc::new(on_open_change),
    }
}

/// The retained widget for a [`SidebarTriggerView`].
pub struct SidebarTriggerWidget {
    open: bool,
    hovered: bool,
    pressed: bool,
    captured: bool,
    on_open_change: ErasedArgCallback<bool>,
}

impl<State: 'static> View<State> for SidebarTriggerView<State> {
    type Element = SidebarTriggerWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> SidebarTriggerWidget {
        SidebarTriggerWidget {
            open: self.open,
            hovered: false,
            pressed: false,
            captured: false,
            on_open_change: erase_callback_arg(&self.on_open_change),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SidebarTriggerWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_open_change = erase_callback_arg(&self.on_open_change);
        if prev.open != self.open {
            element.open = self.open;
            return ChangeFlags::PAINT;
        }
        ChangeFlags::NONE
    }
}

impl SidebarTriggerWidget {
    /// Report the open state an activation asks for.
    fn toggle(&mut self, ctx: &mut EventCtx) {
        let next = !self.open;
        (self.on_open_change)(ctx, next);
    }
}

impl Widget for SidebarTriggerWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(TRIGGER_SIZE, TRIGGER_SIZE))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.hovered = ctx.is_hovered();
        let (origin, size) = (ctx.origin(), ctx.size());
        let focused = ctx.has_focus();
        let (tokens, radius, ring) = {
            let theme = Theme::from_paint_ctx(ctx);
            (
                sidebar_tokens(theme),
                ShadcnTokens::resolve_radius(None, theme).md,
                ring_color(None, theme),
            )
        };
        let mut ink = tokens.foreground;
        if self.hovered || self.pressed {
            // `variant="ghost"`, resolved against the sidebar's own accent so the
            // trigger reads the same inside the panel or beside it.
            scene.fill_rounded_rect(origin, size, radius, tokens.accent);
            ink = tokens.accent_foreground;
        }
        if focused {
            draw_focus_ring(scene, origin, size, radius, ring);
        }
        let center = Point::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
        draw_panel_left(scene, center, crate::style::ICON_SIZE, ink);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if let InputEvent::Key(key) = event {
            if !activates(&key.key) {
                return EventResult::Ignored;
            }
            self.toggle(ctx);
            ctx.request_redraw();
            return EventResult::Handled;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        let size = ctx.size();
        match p.phase {
            PointerPhase::Move => {
                if self.captured {
                    let over = inside(p.position, size);
                    ctx.set_cursor(ACTIVE_CURSOR);
                    if self.pressed != over {
                        self.pressed = over;
                        ctx.request_redraw();
                    }
                    return EventResult::Handled;
                }
                let over = inside(p.position, size);
                if over {
                    ctx.claim_hover();
                    ctx.set_cursor(ACTIVE_CURSOR);
                }
                if self.hovered != over {
                    self.hovered = over;
                    ctx.request_redraw();
                }
                EventResult::Ignored
            }
            PointerPhase::Down => {
                if !presses(p) || !inside(p.position, size) {
                    return EventResult::Ignored;
                }
                self.pressed = true;
                self.captured = true;
                ctx.capture_pointer();
                ctx.request_focus();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.captured = false;
                let armed = self.pressed;
                self.pressed = false;
                if armed && inside(p.position, size) {
                    self.toggle(ctx);
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.captured = false;
                self.pressed = false;
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::Button, |node| {
            node.set_label("Toggle Sidebar");
            node.set_expanded(self.open);
            node.add_action(Action::Click);
        });
    }
}

// ---- Menu buttons ---------------------------------------------------------

/// A declarative menu button. See [`sidebar_menu_button`].
pub struct SidebarMenuButtonView<State: 'static> {
    label: String,
    icon: Option<AnyView<State>>,
    active: bool,
    disabled: bool,
    size: SidebarMenuButtonSize,
    on_press: Rc<dyn Fn(&mut State)>,
}

/// Create a menu button: the sidebar's row control, `w-full gap-2 p-2
/// rounded-md`, with hover/active fills off the `--sidebar-accent` pair.
///
/// In icon mode it collapses to a `size-8` square showing only its
/// [`icon`](SidebarMenuButtonView::icon) child; the label stays its accessible
/// name.
pub fn sidebar_menu_button<State: 'static, F>(
    label: impl Into<String>,
    on_press: F,
) -> SidebarMenuButtonView<State>
where
    F: Fn(&mut State) + 'static,
{
    SidebarMenuButtonView {
        label: label.into(),
        icon: None,
        active: false,
        disabled: false,
        size: SidebarMenuButtonSize::default(),
        on_press: Rc::new(on_press),
    }
}

impl<State: 'static> SidebarMenuButtonView<State> {
    /// Set the leading icon (`[&>svg]:size-4`) — the only thing an icon-mode
    /// row shows.
    pub fn icon<V: View<State>>(mut self, icon: V) -> Self {
        self.icon = Some(any(icon));
        self
    }

    /// Mark this row as the current one (`data-[active=true]`): the accent fill
    /// plus `font-medium`.
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// Disable the row: inert, [`crate::style::DISABLED_CURSOR`], and dimmed by
    /// [`crate::style::DISABLED_OPACITY`].
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Select the size (default [`SidebarMenuButtonSize::Default`]).
    pub fn size(mut self, size: SidebarMenuButtonSize) -> Self {
        self.size = size;
        self
    }

    /// This view's resolved metrics.
    fn metrics(&self) -> MenuMetrics {
        MenuMetrics {
            height: self.size.height(),
            text: self.size.text(),
            pad: MENU_BUTTON_PAD,
            hides_in_icon_mode: false,
        }
    }
}

/// A declarative sub-menu button. See [`sidebar_menu_sub_button`].
pub struct SidebarMenuSubButtonView<State: 'static> {
    label: String,
    icon: Option<AnyView<State>>,
    active: bool,
    disabled: bool,
    size: SidebarMenuSubSize,
    on_press: Rc<dyn Fn(&mut State)>,
}

/// Create a sub-menu button: `h-7 px-2 rounded-md`, the row inside a
/// [`sidebar_menu_sub`]. Hidden in icon mode along with the list it sits in.
pub fn sidebar_menu_sub_button<State: 'static, F>(
    label: impl Into<String>,
    on_press: F,
) -> SidebarMenuSubButtonView<State>
where
    F: Fn(&mut State) + 'static,
{
    SidebarMenuSubButtonView {
        label: label.into(),
        icon: None,
        active: false,
        disabled: false,
        size: SidebarMenuSubSize::default(),
        on_press: Rc::new(on_press),
    }
}

impl<State: 'static> SidebarMenuSubButtonView<State> {
    /// Set the leading icon (`[&>svg]:size-4`).
    pub fn icon<V: View<State>>(mut self, icon: V) -> Self {
        self.icon = Some(any(icon));
        self
    }

    /// Mark this row as the current one (`data-[active=true]`).
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// Disable the row.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Select the size (default [`SidebarMenuSubSize::Md`]).
    pub fn size(mut self, size: SidebarMenuSubSize) -> Self {
        self.size = size;
        self
    }

    /// This view's resolved metrics — `h-7` for both sizes.
    fn metrics(&self) -> MenuMetrics {
        MenuMetrics {
            height: SidebarMenuButtonSize::Sm.height(),
            text: self.size.text(),
            pad: MENU_BUTTON_PAD,
            hides_in_icon_mode: true,
        }
    }
}

/// The retained widget behind both [`SidebarMenuButtonView`] and
/// [`SidebarMenuSubButtonView`] — the two differ only in the [`MenuMetrics`]
/// they fold their size axis into.
pub struct SidebarMenuButtonWidget {
    label: LabelRun,
    /// The leading icon slot. Always a pod (a zero box when unset) so the child
    /// structure never changes across a rebuild.
    icon: ChildPod,
    has_icon: bool,
    metrics: MenuMetrics,
    active: bool,
    disabled: bool,
    /// Whether the last layout was an icon-mode one.
    collapsed: bool,
    hovered: bool,
    pressed: bool,
    captured: bool,
    on_press: frust::authoring::ErasedCallback,
}

/// The shared build/rebuild body for the two button views.
fn build_menu_button<State: 'static>(
    label: &str,
    icon: &Option<AnyView<State>>,
    metrics: MenuMetrics,
    active: bool,
    disabled: bool,
    on_press: &Rc<dyn Fn(&mut State)>,
    ctx: &mut BuildCtx<'_>,
) -> SidebarMenuButtonWidget {
    let placeholder: AnyView<State> = empty_slot();
    let icon_view = icon.as_ref().unwrap_or(&placeholder);
    SidebarMenuButtonWidget {
        label: LabelRun::new(label.to_string()),
        icon: build_child(icon_view, ctx),
        has_icon: icon.is_some(),
        metrics,
        active,
        disabled,
        collapsed: false,
        hovered: false,
        pressed: false,
        captured: false,
        on_press: frust::authoring::erase_callback(on_press),
    }
}

/// The shared rebuild body for the two button views.
#[allow(clippy::too_many_arguments)]
fn rebuild_menu_button<State: 'static>(
    prev_label: &str,
    prev_icon: &Option<AnyView<State>>,
    label: &str,
    icon: &Option<AnyView<State>>,
    metrics: MenuMetrics,
    active: bool,
    disabled: bool,
    on_press: &Rc<dyn Fn(&mut State)>,
    element: &mut SidebarMenuButtonWidget,
    ctx: &mut BuildCtx<'_>,
) -> ChangeFlags {
    let placeholder: AnyView<State> = empty_slot();
    let mut flags = rebuild_child(
        prev_icon.as_ref().unwrap_or(&placeholder),
        icon.as_ref().unwrap_or(&placeholder),
        &mut element.icon,
        ctx,
    );
    element.has_icon = icon.is_some();
    element.on_press = frust::authoring::erase_callback(on_press);
    if prev_label != label {
        element.label.set_content(label.to_string());
        flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
    }
    if element.metrics != metrics {
        element.metrics = metrics;
        flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
    }
    if element.active != active {
        element.active = active;
        flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
    }
    if element.disabled != disabled {
        element.disabled = disabled;
        if disabled {
            element.pressed = false;
            element.captured = false;
            element.hovered = false;
        }
        flags |= ChangeFlags::PAINT;
    }
    flags
}

impl<State: 'static> View<State> for SidebarMenuButtonView<State> {
    type Element = SidebarMenuButtonWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> SidebarMenuButtonWidget {
        build_menu_button(
            &self.label,
            &self.icon,
            self.metrics(),
            self.active,
            self.disabled,
            &self.on_press,
            ctx,
        )
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SidebarMenuButtonWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        rebuild_menu_button(
            &prev.label,
            &prev.icon,
            &self.label,
            &self.icon,
            self.metrics(),
            self.active,
            self.disabled,
            &self.on_press,
            element,
            ctx,
        )
    }

    fn teardown(&self, element: &mut SidebarMenuButtonWidget, ctx: &mut BuildCtx<'_>) {
        if let Some(icon) = &self.icon {
            teardown_child(icon, &mut element.icon, ctx);
        }
    }
}

impl<State: 'static> View<State> for SidebarMenuSubButtonView<State> {
    type Element = SidebarMenuButtonWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> SidebarMenuButtonWidget {
        build_menu_button(
            &self.label,
            &self.icon,
            self.metrics(),
            self.active,
            self.disabled,
            &self.on_press,
            ctx,
        )
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SidebarMenuButtonWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        rebuild_menu_button(
            &prev.label,
            &prev.icon,
            &self.label,
            &self.icon,
            self.metrics(),
            self.active,
            self.disabled,
            &self.on_press,
            element,
            ctx,
        )
    }

    fn teardown(&self, element: &mut SidebarMenuButtonWidget, ctx: &mut BuildCtx<'_>) {
        if let Some(icon) = &self.icon {
            teardown_child(icon, &mut element.icon, ctx);
        }
    }
}

impl SidebarMenuButtonWidget {
    /// Fire the press callback unless the row is disabled.
    fn activate(&mut self, ctx: &mut EventCtx) {
        if self.disabled {
            return;
        }
        (self.on_press)(ctx);
    }
}

impl Widget for SidebarMenuButtonWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        self.collapsed = icon_mode(bc);
        let icon_bc = BoxConstraints::new(
            Size::ZERO,
            Size::new(crate::style::ICON_SIZE, crate::style::ICON_SIZE),
        );
        let icon = self.icon.layout_child(ctx, &icon_bc);

        if self.collapsed {
            if self.metrics.hides_in_icon_mode {
                self.icon.set_origin(Point::ORIGIN);
                return Size::ZERO;
            }
            // `size-8! p-2!`: the label goes, the icon centers.
            let edge = MENU_BUTTON_ICON_EDGE;
            self.icon.set_origin(Point::new(
                (edge - icon.width) / 2.0,
                (edge - icon.height) / 2.0,
            ));
            return bc.constrain(Size::new(edge, edge));
        }

        let weight = if self.active {
            FontWeight::MEDIUM
        } else {
            FontWeight::REGULAR
        };
        let style = themed_family(
            TextStyle {
                weight,
                ..TextStyle::new(self.metrics.text as f32, SHAPING_INK)
            },
            Theme::from_layout_ctx(ctx),
            ThemeTextType::LabelLarge,
        );
        let label = self.label.layout(ctx, &style);
        let icon_span = if self.has_icon {
            icon.width + MENU_BUTTON_GAP
        } else {
            0.0
        };
        let natural = self.metrics.pad * 2.0 + icon_span + label.width;
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            natural
        };
        let height = self.metrics.height;
        self.icon
            .set_origin(Point::new(self.metrics.pad, (height - icon.height) / 2.0));
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if self.collapsed && self.metrics.hides_in_icon_mode {
            return;
        }
        if !self.disabled {
            self.hovered = ctx.is_hovered();
        }
        let (origin, size) = (ctx.origin(), ctx.size());
        let focused = !self.disabled && ctx.has_focus();
        let (tokens, radius, ring) = {
            let theme = Theme::from_paint_ctx(ctx);
            (
                sidebar_tokens(theme),
                ShadcnTokens::resolve_radius(None, theme).md,
                ring_color(None, theme),
            )
        };

        // `data-[active=true]:bg-sidebar-accent` and `hover:bg-sidebar-accent`
        // resolve to the same fill; a resting row is `bg-transparent`.
        let lit = self.active || ((self.hovered || self.pressed) && !self.disabled);
        if lit {
            scene.fill_rounded_rect(
                origin,
                size,
                radius,
                disabled_tint(tokens.accent, self.disabled),
            );
        }
        if focused {
            draw_focus_ring(scene, origin, size, radius, ring);
        }

        self.icon.paint_child(ctx, scene);
        if self.collapsed {
            return;
        }
        let ink = disabled_tint(
            if lit {
                tokens.accent_foreground
            } else {
                tokens.foreground
            },
            self.disabled,
        );
        let label = self.label.size();
        let icon_span = if self.has_icon {
            crate::style::ICON_SIZE + MENU_BUTTON_GAP
        } else {
            0.0
        };
        let label_origin = origin
            + Vec2::new(
                self.metrics.pad + icon_span,
                (size.height - label.height) / 2.0,
            );
        self.label.paint(label_origin, ink, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if self.collapsed && self.metrics.hides_in_icon_mode {
            return EventResult::Ignored;
        }
        if let InputEvent::Key(key) = event {
            if self.disabled || !activates(&key.key) {
                return EventResult::Ignored;
            }
            self.activate(ctx);
            ctx.request_redraw();
            return EventResult::Handled;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        let size = ctx.size();
        match p.phase {
            PointerPhase::Move => {
                if self.captured {
                    ctx.set_cursor(ACTIVE_CURSOR);
                    let over = inside(p.position, size);
                    if self.pressed != over {
                        self.pressed = over;
                        ctx.request_redraw();
                    }
                    return EventResult::Handled;
                }
                let over = inside(p.position, size);
                if over {
                    ctx.claim_hover();
                    ctx.set_cursor(if self.disabled {
                        DISABLED_CURSOR
                    } else {
                        ACTIVE_CURSOR
                    });
                }
                let lit = over && !self.disabled;
                if self.hovered != lit {
                    self.hovered = lit;
                    ctx.request_redraw();
                }
                EventResult::Ignored
            }
            PointerPhase::Down => {
                if self.disabled || !presses(p) || !inside(p.position, size) {
                    return EventResult::Ignored;
                }
                self.pressed = true;
                self.captured = true;
                ctx.capture_pointer();
                ctx.request_focus();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.captured = false;
                let armed = self.pressed;
                self.pressed = false;
                if armed && inside(p.position, size) {
                    self.activate(ctx);
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.captured = false;
                self.pressed = false;
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        if self.collapsed && self.metrics.hides_in_icon_mode {
            return;
        }
        ctx.push_container(
            Role::Button,
            |node| {
                // The label survives icon mode: a collapsed rail is navigable by
                // name even with nothing drawn but the glyph.
                node.set_label(self.label.content());
                node.set_selected(self.active);
                if self.disabled {
                    node.set_disabled();
                } else {
                    node.add_action(Action::Click);
                }
            },
            |ctx| self.icon.semantics_child(ctx),
        );
    }

    visit_children!(icon);
}

// ---- Group and menu actions -----------------------------------------------

/// A declarative trailing action button. See [`sidebar_menu_action`] /
/// [`sidebar_group_action`].
pub struct SidebarActionView<State: 'static> {
    icon: AnyView<State>,
    label: String,
    on_press: Rc<dyn Fn(&mut State)>,
}

/// Create a menu row's trailing action: a `w-5 aspect-square rounded-md` icon
/// button, gone in icon mode.
///
/// `label` is the accessible name only (upstream's `sr-only` span); the button
/// draws `icon` and nothing else.
pub fn sidebar_menu_action<State: 'static, V, F>(
    icon: V,
    label: impl Into<String>,
    on_press: F,
) -> SidebarActionView<State>
where
    V: View<State>,
    F: Fn(&mut State) + 'static,
{
    SidebarActionView {
        icon: any(icon),
        label: label.into(),
        on_press: Rc::new(on_press),
    }
}

/// Create a group's trailing action — the same control as
/// [`sidebar_menu_action`], one row up.
///
/// Upstream pins it to the group's top-right corner absolutely; this port
/// leaves the placement to the caller (a horizontal flex beside the group
/// label is the usual spot).
pub fn sidebar_group_action<State: 'static, V, F>(
    icon: V,
    label: impl Into<String>,
    on_press: F,
) -> SidebarActionView<State>
where
    V: View<State>,
    F: Fn(&mut State) + 'static,
{
    sidebar_menu_action(icon, label, on_press)
}

/// The retained widget for a [`SidebarActionView`].
pub struct SidebarActionWidget {
    icon: ChildPod,
    label: String,
    hidden: bool,
    hovered: bool,
    pressed: bool,
    captured: bool,
    on_press: frust::authoring::ErasedCallback,
}

impl<State: 'static> View<State> for SidebarActionView<State> {
    type Element = SidebarActionWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> SidebarActionWidget {
        SidebarActionWidget {
            icon: build_child(&self.icon, ctx),
            label: self.label.clone(),
            hidden: false,
            hovered: false,
            pressed: false,
            captured: false,
            on_press: frust::authoring::erase_callback(&self.on_press),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SidebarActionWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(&prev.icon, &self.icon, &mut element.icon, ctx);
        element.on_press = frust::authoring::erase_callback(&self.on_press);
        if prev.label != self.label {
            element.label = self.label.clone();
            flags |= ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut SidebarActionWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.icon, &mut element.icon, ctx);
    }
}

impl Widget for SidebarActionWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        self.hidden = icon_mode(bc);
        let icon = self.icon.layout_child(
            ctx,
            &BoxConstraints::new(
                Size::ZERO,
                Size::new(crate::style::ICON_SIZE, crate::style::ICON_SIZE),
            ),
        );
        if self.hidden {
            self.icon.set_origin(Point::ORIGIN);
            return Size::ZERO;
        }
        self.icon.set_origin(Point::new(
            (ACTION_SIZE - icon.width) / 2.0,
            (ACTION_SIZE - icon.height) / 2.0,
        ));
        bc.constrain(Size::new(ACTION_SIZE, ACTION_SIZE))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if self.hidden {
            return;
        }
        self.hovered = ctx.is_hovered();
        let (origin, size) = (ctx.origin(), ctx.size());
        let (tokens, radius, ring) = {
            let theme = Theme::from_paint_ctx(ctx);
            (
                sidebar_tokens(theme),
                ShadcnTokens::resolve_radius(None, theme).md,
                ring_color(None, theme),
            )
        };
        if self.hovered || self.pressed {
            scene.fill_rounded_rect(origin, size, radius, tokens.accent);
        }
        if ctx.has_focus() {
            draw_focus_ring(scene, origin, size, radius, ring);
        }
        self.icon.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if self.hidden {
            return EventResult::Ignored;
        }
        if let InputEvent::Key(key) = event {
            if !activates(&key.key) {
                return EventResult::Ignored;
            }
            (self.on_press)(ctx);
            ctx.request_redraw();
            return EventResult::Handled;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        let size = ctx.size();
        match p.phase {
            PointerPhase::Move => {
                if self.captured {
                    ctx.set_cursor(ACTIVE_CURSOR);
                    let over = inside(p.position, size);
                    if self.pressed != over {
                        self.pressed = over;
                        ctx.request_redraw();
                    }
                    return EventResult::Handled;
                }
                let over = inside(p.position, size);
                if over {
                    ctx.claim_hover();
                    ctx.set_cursor(ACTIVE_CURSOR);
                }
                if self.hovered != over {
                    self.hovered = over;
                    ctx.request_redraw();
                }
                EventResult::Ignored
            }
            PointerPhase::Down => {
                if !presses(p) || !inside(p.position, size) {
                    return EventResult::Ignored;
                }
                self.pressed = true;
                self.captured = true;
                ctx.capture_pointer();
                ctx.request_focus();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.captured = false;
                let armed = self.pressed;
                self.pressed = false;
                if armed && inside(p.position, size) {
                    (self.on_press)(ctx);
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.captured = false;
                self.pressed = false;
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        if self.hidden {
            return;
        }
        ctx.push_container(
            Role::Button,
            |node| {
                node.set_label(self.label.as_str());
                node.add_action(Action::Click);
            },
            |ctx| self.icon.semantics_child(ctx),
        );
    }

    visit_children!(icon);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{
        KeyEvent, Modifiers, PointerButton, PointerEvent, Rect, scene::GlyphRun,
    };
    use frust::column;
    use frust::{Brightness, CursorIcon, FrameTime};
    use frust_core::RenderRoot;
    use std::any::Any;
    use std::cell::Cell;

    const WINDOW: Size = Size::new(900.0, 600.0);
    /// Header/footer height used by the harness shell, so slot origins are
    /// predictable.
    const SLOT_H: f64 = 40.0;

    /// Records every primitive the sidebar parts paint.
    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<(Rect, f64, Color)>,
        shadows: Vec<(Point, Size, Color)>,
        clips: Vec<(Point, Size)>,
        glyphs: Vec<Color>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, origin: Point, size: Size, color: Color) {
            self.rects.push((origin, size, color));
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn fill_rounded_rect(&mut self, origin: Point, size: Size, radius: f64, color: Color) {
            self.rrects.push((origin, size, radius, color));
        }
        fn stroke_path(&mut self, origin: Point, path: &BezPath, width: f64, brush: &Brush) {
            let color = match brush {
                Brush::Solid(c) => *c,
                _ => Color::TRANSPARENT,
            };
            self.strokes
                .push((path.bounding_box() + origin.to_vec2(), width, color));
        }
        fn draw_shadow(&mut self, origin: Point, size: Size, _r: f64, _std: f64, color: Color) {
            self.shadows.push((origin, size, color));
        }
        fn push_clip(&mut self, origin: Point, size: Size) {
            self.clips.push((origin, size));
        }
        fn pop_clip(&mut self) {}
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            if let Brush::Solid(color) = run.brush {
                self.glyphs.push(color);
            }
        }
    }

    impl Recorder {
        /// The panel's own clip band — the outermost clip the sidebar pushes, and
        /// therefore its live width.
        fn panel_width(&self) -> f64 {
            self.clips.first().map_or(0.0, |(_, size)| size.width)
        }

        /// How many rounded fills were painted in `color`.
        fn rrects_in(&self, color: Color) -> usize {
            self.rrects
                .iter()
                .filter(|(_, _, _, c)| *c == color)
                .count()
        }

        /// Whether any plain fill used `color`.
        fn filled_with(&self, color: Color) -> bool {
            self.rects.iter().any(|(_, _, c)| *c == color)
        }
    }

    /// The constraints the provider hands the panel: a tight window height with
    /// the width left free for the panel to choose.
    fn panel_bc() -> BoxConstraints {
        BoxConstraints::new(
            Size::new(0.0, WINDOW.height),
            Size::new(WINDOW.width, WINDOW.height),
        )
    }

    fn theme_light() -> Theme {
        crate::theme().with_brightness(Brightness::Light)
    }

    fn tokens() -> ShadcnSidebar {
        sidebar_tokens(Some(&theme_light()))
    }

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    /// A fixed-size leaf generic over the app state.
    struct Block(Size);

    /// The retained half of [`Block`].
    struct BlockWidget(Size);

    impl<S: 'static> View<S> for Block {
        type Element = BlockWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> BlockWidget {
            BlockWidget(self.0)
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut BlockWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            element.0 = self.0;
            ChangeFlags::NONE
        }
    }

    impl Widget for BlockWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(self.0)
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    }

    /// A leaf that records the [`BoxConstraints`] it was laid out under, so a
    /// test can pin what a `sidebar_inset` child actually receives.
    struct ConstraintProbe {
        captured: Rc<Cell<BoxConstraints>>,
    }

    /// The retained half of [`ConstraintProbe`].
    struct ConstraintProbeWidget {
        captured: Rc<Cell<BoxConstraints>>,
    }

    impl<S: 'static> View<S> for ConstraintProbe {
        type Element = ConstraintProbeWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> ConstraintProbeWidget {
            ConstraintProbeWidget {
                captured: self.captured.clone(),
            }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut ConstraintProbeWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            element.captured = self.captured.clone();
            ChangeFlags::NONE
        }
    }

    impl Widget for ConstraintProbeWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            self.captured.set(*bc);
            bc.constrain(Size::ZERO)
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    }

    #[derive(Default)]
    struct AppState {
        open: bool,
        requests: Vec<bool>,
        presses: Vec<String>,
    }

    /// The shell shape a test builds: the panel's three axes, its rail, and
    /// whether the app actually *applies* the open state it is handed.
    #[derive(Clone, Copy)]
    struct Cfg {
        side: SidebarSide,
        variant: SidebarVariant,
        collapsible: SidebarCollapsible,
        rail: bool,
        apply: bool,
    }

    impl Default for Cfg {
        fn default() -> Self {
            Cfg {
                side: SidebarSide::Left,
                variant: SidebarVariant::Sidebar,
                collapsible: SidebarCollapsible::Offcanvas,
                rail: false,
                apply: true,
            }
        }
    }

    /// The panel the harness builds: a header, a group with a label and two menu
    /// rows (the first one active and badged), and a footer.
    fn panel(cfg: Cfg) -> SidebarView<AppState> {
        let menu = sidebar_menu(vec![
            sidebar_menu_item(vec![
                any(sidebar_menu_button("Inbox", |s: &mut AppState| {
                    s.presses.push("Inbox".to_string())
                })
                .icon(Block(Size::new(16.0, 16.0)))
                .active(true)),
                any(sidebar_menu_badge("3")),
            ]),
            sidebar_menu_item(vec![
                sidebar_menu_button("Drafts", |s: &mut AppState| {
                    s.presses.push("Drafts".to_string())
                })
                .icon(Block(Size::new(16.0, 16.0))),
            ]),
        ]);
        sidebar(sidebar_content(vec![sidebar_group(vec![
            any(sidebar_group_label("Platform")),
            menu,
        ])]))
        .header(Block(Size::new(0.0, SLOT_H)))
        .footer(Block(Size::new(0.0, SLOT_H)))
        .side(cfg.side)
        .variant(cfg.variant)
        .collapsible(cfg.collapsible)
        .rail(cfg.rail)
    }

    fn shell(open: bool, cfg: Cfg) -> SidebarProviderView<AppState> {
        let apply = cfg.apply;
        sidebar_provider(
            panel(cfg),
            sidebar_inset(vec![Block(Size::new(10.0, 10.0))]),
            open,
            move |s: &mut AppState, next| {
                s.requests.push(next);
                if apply {
                    s.open = next;
                }
            },
        )
    }

    fn build_provider(view: &SidebarProviderView<AppState>) -> SidebarProviderWidget {
        let mut counter = 0u64;
        View::<AppState>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn build_widget<V: View<AppState>>(view: &V) -> V::Element {
        let mut counter = 0u64;
        View::<AppState>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout_widget<W: Widget>(w: &mut W, bc: &BoxConstraints, theme: Option<&Theme>) -> Size {
        let mut tcx = TextContext::new();
        let mut ctx =
            LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), theme.map(|t| t as &dyn Any));
        w.layout(&mut ctx, bc)
    }

    fn paint_widget<W: Widget>(w: &mut W, size: Size, theme: Option<&Theme>) -> Recorder {
        let mut rec = Recorder::default();
        let base = PaintCtx::for_test(Point::ORIGIN, size, FrameTime::ZERO);
        let mut ctx = match theme {
            Some(t) => base.with_theme(t as &dyn Any),
            None => base,
        };
        w.paint(&mut ctx, &mut rec);
        rec
    }

    fn dispatch<W: Widget>(w: &mut W, state: &mut AppState, size: Size, event: &InputEvent) {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ORIGIN, size);
        w.event(&mut ctx, event);
    }

    fn pointer(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    /// A whole shell driven through a real `RenderRoot` — the only place hover,
    /// focus routing, and the cursor are real.
    struct Harness {
        root: RenderRoot<AppState, SidebarProviderView<AppState>>,
        state: AppState,
        tcx: TextContext,
        cfg: Cfg,
    }

    impl Harness {
        fn new(cfg: Cfg, open: bool, reduce_motion: bool) -> Self {
            let mut h = Harness {
                root: RenderRoot::new(),
                state: AppState {
                    open,
                    ..AppState::default()
                },
                tcx: TextContext::new(),
                cfg,
            };
            let mut theme = theme_light();
            theme.motion.reduce_motion = reduce_motion;
            h.root.set_theme(Box::new(theme));
            h.pass();
            h
        }

        fn pass(&mut self) -> Size {
            let cfg = self.cfg;
            let mut logic = move |state: &mut AppState| shell(state.open, cfg);
            self.root.rebuild(&mut logic, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any)
        }

        fn frame(&mut self, ms: f64) -> Recorder {
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, ft_ms(ms));
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
            rec
        }

        fn pointer(&mut self, phase: PointerPhase, x: f64, y: f64) {
            self.root.event(&mut self.state, &pointer(phase, x, y));
        }

        fn key(&mut self, key: Key, modifiers: Modifiers) {
            self.root.event(
                &mut self.state,
                &InputEvent::Key(KeyEvent {
                    key,
                    modifiers,
                    repeat: false,
                }),
            );
        }
    }

    fn ctrl() -> Modifiers {
        Modifiers {
            ctrl: true,
            ..Modifiers::default()
        }
    }

    fn meta() -> Modifiers {
        Modifiers {
            meta: true,
            ..Modifiers::default()
        }
    }

    // ---- The shell row ----------------------------------------------------

    #[test]
    fn the_shell_row_docks_the_panel_and_gives_the_rest_to_the_content() {
        let mut w = build_provider(&shell(true, Cfg::default()));
        let size = layout_widget(&mut w, &BoxConstraints::tight(WINDOW), Some(&theme_light()));
        assert_eq!(size, WINDOW);
        assert_eq!(w.pods[0].origin(), Point::ORIGIN);
        assert_eq!(w.pods[0].size().width, SIDEBAR_WIDTH);
        assert_eq!(w.pods[0].size().height, WINDOW.height, "the panel fills");
        assert_eq!(w.pods[1].origin(), Point::new(SIDEBAR_WIDTH, 0.0));
        assert_eq!(w.pods[1].size().width, WINDOW.width - SIDEBAR_WIDTH);

        let cfg = Cfg {
            side: SidebarSide::Right,
            ..Cfg::default()
        };
        let mut w = build_provider(&shell(true, cfg));
        layout_widget(&mut w, &BoxConstraints::tight(WINDOW), Some(&theme_light()));
        assert_eq!(w.pods[1].origin(), Point::ORIGIN, "content leads");
        assert_eq!(
            w.pods[0].origin(),
            Point::new(WINDOW.width - SIDEBAR_WIDTH, 0.0)
        );
    }

    #[test]
    fn the_panel_stacks_header_content_and_footer_and_gives_the_rest_to_content() {
        let mut w = build_widget(&panel(Cfg::default()));
        let size = layout_widget(&mut w, &panel_bc(), Some(&theme_light()));
        assert_eq!(size.width, SIDEBAR_WIDTH);
        assert_eq!(w.pods[0].origin(), Point::ORIGIN);
        assert_eq!(w.pods[1].origin(), Point::new(0.0, SLOT_H));
        assert_eq!(w.pods[2].origin(), Point::new(0.0, WINDOW.height - SLOT_H));
        assert_eq!(
            w.pods[1].size().height,
            WINDOW.height - SLOT_H * 2.0,
            "`flex-1 min-h-0`: the content band takes what is left"
        );
        assert_eq!(w.pods[1].size().width, SIDEBAR_WIDTH);
    }

    // ---- Controlled open --------------------------------------------------

    #[test]
    fn the_open_state_is_controlled_and_a_toggle_only_reports() {
        // The app records the request but never applies it.
        let cfg = Cfg {
            apply: false,
            ..Cfg::default()
        };
        let mut h = Harness::new(cfg, true, false);
        h.frame(0.0);
        h.key(Key::Character("b".to_string()), ctrl());
        assert_eq!(h.state.requests, vec![false], "the requested state");
        h.pass();
        assert_eq!(
            h.frame(0.0).panel_width(),
            SIDEBAR_WIDTH,
            "the panel never collapsed itself"
        );

        // ...and once the app confirms, it does.
        h.state.open = false;
        h.pass();
        h.frame(0.0);
        assert!(h.frame(COLLAPSE_MS as f64 * 2.0).panel_width() < SIDEBAR_WIDTH);
    }

    #[test]
    fn ctrl_or_cmd_b_toggles_and_nothing_else_does() {
        let mut h = Harness::new(Cfg::default(), true, false);
        h.key(Key::Character("b".to_string()), ctrl());
        h.pass();
        h.key(Key::Character("B".to_string()), meta());
        h.pass();
        assert_eq!(h.state.requests, vec![false, true]);

        // A bare `b`, a chorded other letter, and a named key all pass through.
        h.key(Key::Character("b".to_string()), Modifiers::default());
        h.key(Key::Character("a".to_string()), ctrl());
        h.key(Key::Named(frust::authoring::NamedKey::Escape), ctrl());
        assert_eq!(h.state.requests.len(), 2);
    }

    #[test]
    fn the_control_character_form_of_the_chord_counts_too() {
        // Some platforms report the C0 control byte rather than the letter.
        let mut h = Harness::new(Cfg::default(), true, false);
        h.key(Key::Character("\u{2}".to_string()), ctrl());
        assert_eq!(h.state.requests, vec![false]);
    }

    // ---- Collapse ---------------------------------------------------------

    #[test]
    fn the_collapse_animates_the_width_and_reduce_motion_jumps() {
        let mut h = Harness::new(Cfg::default(), true, false);
        h.frame(0.0);
        h.key(Key::Character("b".to_string()), ctrl());
        h.pass();
        h.frame(0.0); // seeds the clock
        let mid = h.frame(COLLAPSE_MS as f64 / 2.0).panel_width();
        assert!(mid > 0.0 && mid < SIDEBAR_WIDTH, "mid-collapse: {mid}");
        let done = h.frame(COLLAPSE_MS as f64 * 2.0).panel_width();
        assert_eq!(done, 0.0, "offcanvas ends at zero width");

        // Reduce motion: the very first frame after the toggle is already shut.
        let mut h = Harness::new(Cfg::default(), true, true);
        h.frame(0.0);
        h.key(Key::Character("b".to_string()), ctrl());
        h.pass();
        h.frame(0.0);
        assert_eq!(h.frame(0.0).panel_width(), 0.0);
    }

    #[test]
    fn the_expand_animates_the_width_without_lagging_a_frame_behind_the_box() {
        // Mirrors `the_collapse_animates_the_width_and_reduce_motion_jumps` in the
        // other direction: starting closed and toggling open, the panel's own
        // clip band must track the animation's fresh value every frame, not the
        // stale box the prior frame's layout produced (that regression showed as
        // the panel visibly not opening at all until a frame after the toggle).
        let mut h = Harness::new(Cfg::default(), false, false);
        h.frame(0.0);
        h.key(Key::Character("b".to_string()), ctrl());
        h.pass();
        h.frame(0.0); // seeds the clock
        let mid = h.frame(COLLAPSE_MS as f64 / 2.0).panel_width();
        assert!(mid > 0.0 && mid < SIDEBAR_WIDTH, "mid-expand: {mid}");
        let done = h.frame(COLLAPSE_MS as f64 * 2.0).panel_width();
        assert_eq!(done, SIDEBAR_WIDTH, "offcanvas ends fully open");
    }

    #[test]
    fn a_panel_built_collapsed_starts_collapsed() {
        let view = panel(Cfg::default()).wired(false, Rc::new(|_: &mut AppState, _| {}));
        let mut w = build_widget(&view);
        assert_eq!(
            layout_widget(&mut w, &panel_bc(), None).width,
            0.0,
            "no opening animation on the first frame"
        );

        let view = panel(Cfg {
            collapsible: SidebarCollapsible::None,
            ..Cfg::default()
        })
        .wired(false, Rc::new(|_: &mut AppState, _| {}));
        let mut w = build_widget(&view);
        assert_eq!(
            layout_widget(&mut w, &panel_bc(), None).width,
            SIDEBAR_WIDTH,
            "`collapsible=none` ignores the open state"
        );
    }

    // ---- Icon mode --------------------------------------------------------

    #[test]
    fn icon_mode_collapses_the_panel_to_the_rail_width() {
        let cfg = Cfg {
            collapsible: SidebarCollapsible::Icon,
            ..Cfg::default()
        };
        let view = panel(cfg).wired(false, Rc::new(|_: &mut AppState, _| {}));
        let mut w = build_widget(&view);
        let size = layout_widget(&mut w, &panel_bc(), Some(&theme_light()));
        assert_eq!(size.width, SIDEBAR_WIDTH_ICON);
        assert_eq!(w.pods[1].size().width, SIDEBAR_WIDTH_ICON);
    }

    #[test]
    fn icon_mode_squares_a_menu_button_and_drops_its_label() {
        let rail_bc = BoxConstraints::new(Size::ZERO, Size::new(SIDEBAR_WIDTH_ICON, 400.0));
        let wide_bc = BoxConstraints::new(Size::ZERO, Size::new(SIDEBAR_WIDTH - 16.0, 400.0));
        let theme = theme_light();

        let view = sidebar_menu_button("Inbox", |_: &mut AppState| {}).icon(Block(Size::new(
            crate::style::ICON_SIZE,
            crate::style::ICON_SIZE,
        )));
        let mut w = build_widget(&view);
        let wide = layout_widget(&mut w, &wide_bc, Some(&theme));
        assert_eq!(wide.height, SidebarMenuButtonSize::Default.height());
        assert_eq!(wide.width, SIDEBAR_WIDTH - 16.0, "`w-full`");
        assert!(
            !paint_widget(&mut w, wide, Some(&theme)).glyphs.is_empty(),
            "the expanded row draws its label"
        );

        let narrow = layout_widget(&mut w, &rail_bc, Some(&theme));
        assert_eq!(
            narrow,
            Size::new(MENU_BUTTON_ICON_EDGE, MENU_BUTTON_ICON_EDGE),
            "`size-8!`"
        );
        assert!(
            paint_widget(&mut w, narrow, Some(&theme)).glyphs.is_empty(),
            "the collapsed row draws no label"
        );
        // The icon stays, centered in the square.
        assert_eq!(
            w.icon.origin(),
            Point::new(
                (MENU_BUTTON_ICON_EDGE - crate::style::ICON_SIZE) / 2.0,
                (MENU_BUTTON_ICON_EDGE - crate::style::ICON_SIZE) / 2.0
            )
        );
    }

    #[test]
    fn icon_mode_removes_the_parts_that_have_no_collapsed_form() {
        let rail_bc = BoxConstraints::new(Size::ZERO, Size::new(SIDEBAR_WIDTH_ICON, 400.0));
        let wide_bc = BoxConstraints::new(Size::ZERO, Size::new(SIDEBAR_WIDTH - 16.0, 400.0));
        let theme = theme_light();

        let mut label = build_widget(&sidebar_group_label("Platform"));
        assert_eq!(
            layout_widget(&mut label, &wide_bc, Some(&theme)).height,
            GROUP_LABEL_HEIGHT
        );
        assert_eq!(
            layout_widget(&mut label, &rail_bc, Some(&theme)),
            Size::ZERO
        );

        let mut badge = build_widget(&sidebar_menu_badge("3"));
        assert_eq!(
            layout_widget(&mut badge, &wide_bc, Some(&theme)).height,
            BADGE_HEIGHT
        );
        assert_eq!(
            layout_widget(&mut badge, &rail_bc, Some(&theme)),
            Size::ZERO
        );

        let mut action = build_widget(&sidebar_menu_action(
            Block(Size::new(16.0, 16.0)),
            "More",
            |_: &mut AppState| {},
        ));
        assert_eq!(
            layout_widget(&mut action, &wide_bc, Some(&theme)),
            Size::new(ACTION_SIZE, ACTION_SIZE)
        );
        assert_eq!(
            layout_widget(&mut action, &rail_bc, Some(&theme)),
            Size::ZERO
        );

        let mut sub = build_widget(&sidebar_menu_sub(vec![sidebar_menu_sub_button(
            "Nested",
            |_: &mut AppState| {},
        )]));
        assert!(layout_widget(&mut sub, &wide_bc, Some(&theme)).height > 0.0);
        assert_eq!(layout_widget(&mut sub, &rail_bc, Some(&theme)), Size::ZERO);
    }

    // ---- Variant chrome ---------------------------------------------------

    #[test]
    fn the_flush_variant_fills_the_panel_and_rules_its_trailing_edge() {
        let theme = theme_light();
        let tokens = tokens();
        let view = panel(Cfg::default()).wired(true, Rc::new(|_: &mut AppState, _| {}));
        let mut w = build_widget(&view);
        let size = layout_widget(&mut w, &panel_bc(), Some(&theme));
        let rec = paint_widget(&mut w, size, Some(&theme));

        assert!(rec.filled_with(tokens.background), "bg-sidebar");
        let rule = rec
            .rects
            .iter()
            .find(|(_, s, c)| *c == tokens.border && s.width == BORDER_WIDTH)
            .expect("border-r");
        assert_eq!(rule.0.x, SIDEBAR_WIDTH - BORDER_WIDTH);
        assert!(rec.shadows.is_empty(), "the flush panel casts none");
    }

    #[test]
    fn the_floating_variant_is_a_rounded_bordered_shadowed_card() {
        let theme = theme_light();
        let tokens = tokens();
        let cfg = Cfg {
            variant: SidebarVariant::Floating,
            ..Cfg::default()
        };
        let view = panel(cfg).wired(true, Rc::new(|_: &mut AppState, _| {}));
        let mut w = build_widget(&view);
        let size = layout_widget(&mut w, &panel_bc(), Some(&theme));
        assert_eq!(
            size.width,
            SIDEBAR_WIDTH + PANEL_INSET * 2.0,
            "`p-2` around the card"
        );
        // The slots live inside the card, not the outer box.
        assert_eq!(w.pods[0].origin(), Point::new(PANEL_INSET, PANEL_INSET));
        assert_eq!(
            w.pods[1].size().height,
            WINDOW.height - PANEL_INSET * 2.0 - SLOT_H * 2.0
        );
        assert_eq!(
            w.pods[2].origin().y,
            WINDOW.height - PANEL_INSET - SLOT_H,
            "the footer sits on the card's inner bottom edge"
        );

        let rec = paint_widget(&mut w, size, Some(&theme));
        assert_eq!(rec.shadows.len(), 1, "shadow-sm");
        let card = rec
            .rrects
            .iter()
            .find(|(_, _, _, c)| *c == tokens.background)
            .expect("the rounded panel fill");
        assert_eq!(card.0, Point::new(PANEL_INSET, PANEL_INSET));
        assert_eq!(card.1.width, SIDEBAR_WIDTH);
        assert!(
            rec.strokes
                .iter()
                .any(|(_, w, c)| *w == BORDER_WIDTH && *c == tokens.border),
            "border-sidebar-border"
        );
    }

    #[test]
    fn the_inset_variant_moves_the_card_look_onto_the_main_content() {
        let theme = theme_light();
        let tokens = tokens();
        let cfg = Cfg {
            variant: SidebarVariant::Inset,
            ..Cfg::default()
        };
        let mut w = build_provider(&shell(true, cfg));
        let size = layout_widget(&mut w, &BoxConstraints::tight(WINDOW), Some(&theme));
        let rec = paint_widget(&mut w, size, Some(&theme));

        // The wrapper takes the sidebar surface (`has-data-[variant=inset]`)...
        assert_eq!(rec.rects[0].2, tokens.background);
        assert_eq!(rec.rects[0].1, WINDOW);
        // ...and the content is a rounded, shadowed card inset from three edges.
        let card = rec
            .rrects
            .iter()
            .find(|(_, _, _, c)| *c == theme.scheme().surface)
            .expect("the inset card");
        assert_eq!(card.0.y, PANEL_INSET);
        assert!(!rec.shadows.is_empty(), "shadow-sm under the card");

        // The flush variant leaves the main content plain.
        let mut plain = build_provider(&shell(true, Cfg::default()));
        let size = layout_widget(&mut plain, &BoxConstraints::tight(WINDOW), Some(&theme));
        let rec = paint_widget(&mut plain, size, Some(&theme));
        assert_eq!(rec.rrects_in(theme.scheme().surface), 0);
    }

    #[test]
    fn the_inset_hands_a_single_child_a_finite_max_height() {
        // Regression test for the bug class where `sidebar_inset` wrapped its
        // child in a plain `Column`: `FlexWidget`'s inflexible-child pass hands
        // out `f64::INFINITY` on the main axis regardless of the `Column`'s own
        // received constraints, so a scroll surface or overlay host nested
        // under the inset saw an unbounded height. A single child (every
        // shipped call site) must instead see the inset's own finite bound.
        let captured = Rc::new(Cell::new(BoxConstraints::tight(Size::ZERO)));
        let probe = ConstraintProbe {
            captured: captured.clone(),
        };
        let view = sidebar_inset::<(), _>(vec![any(probe)]);
        let mut counter = 0u64;
        let mut w = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));

        layout_widget(
            &mut w,
            &BoxConstraints::new(Size::new(400.0, 0.0), Size::new(400.0, 300.0)),
            None,
        );

        let bc = captured.get();
        assert!(
            bc.max().height.is_finite(),
            "a sidebar_inset single child must be laid out under a bounded main-axis height"
        );
        assert_eq!(bc.max().height, 300.0);
    }

    // ---- Menu rows --------------------------------------------------------

    #[test]
    fn a_menu_button_lights_when_active_and_on_hover() {
        let accent = tokens().accent;
        let mut h = Harness::new(Cfg::default(), true, false);
        // Only the active row is lit at rest.
        assert_eq!(h.frame(0.0).rrects_in(accent), 1);

        // The panel's second menu row: the header, then the group's `p-2`, its
        // label, the first row, and the `gap-1` between them.
        let row_y = SLOT_H
            + SLOT_PAD
            + GROUP_LABEL_HEIGHT
            + SidebarMenuButtonSize::Default.height()
            + MENU_GAP
            + 2.0;
        h.pointer(PointerPhase::Move, 40.0, row_y);
        assert_eq!(h.root.cursor(), CursorIcon::Pointer);
        assert_eq!(
            h.frame(0.0).rrects_in(accent),
            2,
            "the hovered row lights too"
        );

        // ...and a press on it fires on release, not on the press.
        h.pointer(PointerPhase::Down, 40.0, row_y);
        assert!(h.state.presses.is_empty());
        h.pointer(PointerPhase::Up, 40.0, row_y);
        assert_eq!(h.state.presses, vec!["Drafts".to_string()]);
    }

    #[test]
    fn a_disabled_menu_button_is_inert() {
        let theme = theme_light();
        let view = sidebar_menu_button("Inbox", |s: &mut AppState| {
            s.presses.push("Inbox".to_string())
        })
        .disabled(true);
        let mut w = build_widget(&view);
        let size = layout_widget(
            &mut w,
            &BoxConstraints::new(Size::ZERO, Size::new(200.0, 100.0)),
            Some(&theme),
        );
        let mut state = AppState::default();
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, 5.0, 5.0),
        );
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Up, 5.0, 5.0),
        );
        assert!(state.presses.is_empty());
    }

    #[test]
    fn a_sub_button_rides_the_same_widget_at_the_sub_metrics() {
        let theme = theme_light();
        let mut w = build_widget(
            &sidebar_menu_sub_button("Nested", |_: &mut AppState| {}).size(SidebarMenuSubSize::Sm),
        );
        let size = layout_widget(
            &mut w,
            &BoxConstraints::new(Size::ZERO, Size::new(200.0, 100.0)),
            Some(&theme),
        );
        assert_eq!(size.height, SidebarMenuButtonSize::Sm.height(), "`h-7`");
        assert!(w.metrics.hides_in_icon_mode, "sub rows vanish in icon mode");
    }

    #[test]
    fn the_menu_row_gives_the_button_the_space_the_badge_leaves() {
        let theme = theme_light();
        let row = sidebar_menu_item::<AppState, _>(vec![
            any(sidebar_menu_button("Inbox", |_: &mut AppState| {})),
            any(sidebar_menu_badge("3")),
        ]);
        let mut w = build_widget(&row);
        let size = layout_widget(
            &mut w,
            &BoxConstraints::new(Size::ZERO, Size::new(240.0, 200.0)),
            Some(&theme),
        );
        assert_eq!(size.width, 240.0);
        let mut widths = Vec::new();
        Widget::visit_children(&w, &mut |pod| widths.push(pod.size().width));
        assert_eq!(widths.len(), 2);
        assert!(widths[1] >= BADGE_HEIGHT, "the badge keeps `min-w-5`");
        assert_eq!(
            widths[0] + widths[1],
            240.0,
            "the button takes the remainder"
        );
    }

    // ---- Rail -------------------------------------------------------------

    #[test]
    fn the_rail_toggles_and_asks_for_the_resize_cursor() {
        let cfg = Cfg {
            rail: true,
            ..Cfg::default()
        };
        let mut h = Harness::new(cfg, true, false);
        let on_rail = SIDEBAR_WIDTH - RAIL_WIDTH / 2.0;

        h.pointer(PointerPhase::Move, on_rail, 300.0);
        assert_eq!(h.root.cursor(), CursorIcon::ColResize);
        h.pointer(PointerPhase::Down, on_rail, 300.0);
        assert!(h.state.requests.is_empty(), "never on down");
        h.pointer(PointerPhase::Up, on_rail, 300.0);
        assert_eq!(h.state.requests, vec![false]);

        // A press cancelled by a platform gesture steal fires nothing.
        h.pass();
        h.pointer(PointerPhase::Down, on_rail, 300.0);
        h.pointer(PointerPhase::Cancel, on_rail, 300.0);
        assert_eq!(h.state.requests.len(), 1);
    }

    #[test]
    fn a_rail_survives_an_offcanvas_collapse_so_the_panel_can_come_back() {
        let cfg = Cfg {
            rail: true,
            ..Cfg::default()
        };
        let view = panel(cfg).wired(false, Rc::new(|_: &mut AppState, _| {}));
        let mut w = build_widget(&view);
        assert_eq!(layout_widget(&mut w, &panel_bc(), None).width, RAIL_WIDTH);

        // Without one, the collapsed panel really is gone.
        let bare = panel(Cfg::default()).wired(false, Rc::new(|_: &mut AppState, _| {}));
        let mut w = build_widget(&bare);
        assert_eq!(layout_widget(&mut w, &panel_bc(), None).width, 0.0);
    }

    #[test]
    fn a_secondary_press_on_the_rail_neither_toggles_nor_captures() {
        let cfg = Cfg {
            rail: true,
            ..Cfg::default()
        };
        let mut h = Harness::new(cfg, true, false);
        let on_rail = SIDEBAR_WIDTH - RAIL_WIDTH / 2.0;

        h.root.event(
            &mut h.state,
            &InputEvent::Pointer(PointerEvent {
                phase: PointerPhase::Down,
                position: Point::new(on_rail, 300.0),
                button: PointerButton::Secondary,
            }),
        );
        assert!(
            !h.root.is_pointer_captured(),
            "a right-click must leave the root uncaptured — a capture it can \
             never release is what wedges the whole shell"
        );
        h.pointer(PointerPhase::Up, on_rail, 300.0);
        assert!(h.state.requests.is_empty());

        // The primary press still toggles through the same band.
        h.pointer(PointerPhase::Down, on_rail, 300.0);
        assert!(h.root.is_pointer_captured());
        h.pointer(PointerPhase::Up, on_rail, 300.0);
        assert_eq!(h.state.requests, vec![false]);
    }

    #[test]
    fn a_rail_less_panel_ignores_the_edge_band() {
        let mut h = Harness::new(Cfg::default(), true, false);
        let on_edge = SIDEBAR_WIDTH - RAIL_WIDTH / 2.0;
        h.pointer(PointerPhase::Down, on_edge, 300.0);
        h.pointer(PointerPhase::Up, on_edge, 300.0);
        assert!(h.state.requests.is_empty());
    }

    // ---- Trigger ----------------------------------------------------------

    #[test]
    fn the_trigger_reports_the_requested_state_on_release_inside() {
        let mut w = build_widget(&sidebar_trigger::<AppState, _>(
            true,
            |s: &mut AppState, next| s.requests.push(next),
        ));
        let size = layout_widget(&mut w, &BoxConstraints::loose(WINDOW), None);
        assert_eq!(size, Size::new(TRIGGER_SIZE, TRIGGER_SIZE));

        let mut state = AppState::default();
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, 5.0, 5.0),
        );
        assert!(state.requests.is_empty(), "never on down");
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Up, 5.0, 5.0),
        );
        assert_eq!(state.requests, vec![false]);

        // A release outside the button fires nothing.
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, 5.0, 5.0),
        );
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Up, TRIGGER_SIZE + 10.0, 5.0),
        );
        assert_eq!(state.requests.len(), 1);

        // Space activates it too.
        dispatch(
            &mut w,
            &mut state,
            size,
            &InputEvent::Key(KeyEvent {
                key: Key::Character(" ".to_string()),
                modifiers: Modifiers::default(),
                repeat: false,
            }),
        );
        assert_eq!(state.requests.len(), 2);
    }

    #[test]
    fn the_trigger_draws_the_panel_left_glyph() {
        let theme = theme_light();
        let mut w = build_widget(&sidebar_trigger::<AppState, _>(true, |_, _| {}));
        let size = layout_widget(&mut w, &BoxConstraints::loose(WINDOW), Some(&theme));
        let rec = paint_widget(&mut w, size, Some(&theme));
        // The rounded frame plus its vertical rule, in the sidebar ink.
        assert_eq!(rec.strokes.len(), 2);
        assert!(
            rec.strokes
                .iter()
                .all(|(_, _, c)| *c == tokens().foreground)
        );
    }

    // ---- Separator, sub-list, semantics ------------------------------------

    #[test]
    fn the_separator_is_a_hairline_inset_from_both_edges() {
        let theme = theme_light();
        let mut w = build_widget(&sidebar_separator());
        let size = layout_widget(
            &mut w,
            &BoxConstraints::new(Size::ZERO, Size::new(SIDEBAR_WIDTH, 40.0)),
            Some(&theme),
        );
        assert_eq!(size, Size::new(SIDEBAR_WIDTH, BORDER_WIDTH));
        let rec = paint_widget(&mut w, size, Some(&theme));
        let (origin, painted, color) = rec.rects[0];
        assert_eq!(origin.x, SEPARATOR_MARGIN_X);
        assert_eq!(painted.width, SIDEBAR_WIDTH - SEPARATOR_MARGIN_X * 2.0);
        assert_eq!(color, tokens().border);
    }

    #[test]
    fn the_sub_list_indents_behind_a_rule() {
        let theme = theme_light();
        let mut w = build_widget(&sidebar_menu_sub(vec![sidebar_menu_sub_button(
            "Nested",
            |_: &mut AppState| {},
        )]));
        let size = layout_widget(
            &mut w,
            &BoxConstraints::new(Size::ZERO, Size::new(SIDEBAR_WIDTH, 200.0)),
            Some(&theme),
        );
        assert_eq!(
            w.child.origin(),
            Point::new(SUB_MARGIN_X + BORDER_WIDTH + SUB_PAD_X, SUB_PAD_Y)
        );
        let rec = paint_widget(&mut w, size, Some(&theme));
        let (origin, painted, color) = rec.rects[0];
        assert_eq!(origin.x, SUB_MARGIN_X);
        assert_eq!(painted.width, BORDER_WIDTH);
        assert_eq!(color, tokens().border);
    }

    #[test]
    fn semantics_names_every_row_and_omits_a_collapsed_panel() {
        let mut h = Harness::new(Cfg::default(), true, false);
        let update = h.root.semantics();
        let labels: Vec<String> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::Button)
            .filter_map(|(_, n)| n.label().map(|l| l.to_string()))
            .collect();
        assert!(labels.iter().any(|l| l == "Inbox"), "{labels:?}");
        assert!(labels.iter().any(|l| l == "Drafts"), "{labels:?}");

        h.state.open = false;
        h.pass();
        h.frame(0.0);
        h.frame(COLLAPSE_MS as f64 * 2.0);
        h.pass();
        let update = h.root.semantics();
        let labels: Vec<String> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::Button)
            .filter_map(|(_, n)| n.label().map(|l| l.to_string()))
            .collect();
        assert!(
            labels.is_empty(),
            "a shut panel offers nothing to activate: {labels:?}"
        );
    }

    #[test]
    fn every_slot_is_published_to_the_tree_walkers() {
        let provider = build_provider(&shell(true, Cfg::default()));
        let mut seen = 0usize;
        Widget::visit_children(&provider, &mut |_| seen += 1);
        assert_eq!(seen, 2, "the panel and the main content");

        let panel = build_widget(&panel(Cfg::default()));
        let mut seen = 0usize;
        Widget::visit_children(&panel, &mut |_| seen += 1);
        assert_eq!(seen, 3, "header, content, footer");
    }

    #[test]
    fn the_token_ladder_falls_back_to_the_neutral_light_group() {
        assert_eq!(sidebar_tokens(None), FALLBACK.sidebar);
        let dark = crate::theme().with_brightness(Brightness::Dark);
        assert_eq!(
            sidebar_tokens(Some(&dark)),
            ShadcnTokens::shadcn().sidebar(Brightness::Dark)
        );
        assert_ne!(sidebar_tokens(Some(&dark)), sidebar_tokens(None));
    }

    // ---- Typeface: the labels follow the live theme -----------------------

    /// A group label over a menu holding an active button with a badge, a
    /// plain button, and a sub-menu with one sub-button: every run the sidebar
    /// shapes itself.
    #[cfg(feature = "bundled-fonts")]
    fn nav(_: &mut ()) -> FlexView<()> {
        column().child(sidebar_group(vec![
            any(sidebar_group_label("Platform")),
            sidebar_menu(vec![
                any(sidebar_menu_item(vec![
                    any(sidebar_menu_button("Inbox", |_: &mut ()| {}).active(true)),
                    any(sidebar_menu_badge("24")),
                ])),
                any(sidebar_menu_item(vec![sidebar_menu_button(
                    "Drafts",
                    |_: &mut ()| {},
                )])),
                any(sidebar_menu_sub(vec![sidebar_menu_sub_item(vec![
                    sidebar_menu_sub_button("Starred", |_: &mut ()| {}),
                ])])),
            ]),
        ]))
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_labels_paint_in_the_theme_face() {
        crate::text::typeface_probe::assert_paints_in_the_theme_face(
            "the sidebar's group label, menu buttons and badge",
            nav,
        );
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_labels_follow_a_live_theme_swap() {
        crate::text::typeface_probe::assert_follows_a_live_theme_swap(
            "the sidebar's group label, menu buttons and badge",
            nav,
        );
    }
}
