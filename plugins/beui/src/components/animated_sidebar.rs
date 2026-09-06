//! Ports beUI's `animated-sidebar` component.
//!
//! **Source:** `components/motion/animated-sidebar.tsx` of the beUI monorepo,
//! rev `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01.
//!
//! | upstream | here |
//! |---|---|
//! | `--sidebar-width: 16rem` | [`SIDEBAR_WIDTH`] |
//! | `--sidebar-width-icon: 4.25rem` | [`SIDEBAR_WIDTH_ICON`] |
//! | `SIDEBAR_MORPH_TRANSITION` (spring 380/35/0.75) | [`SIDEBAR_MORPH_SPRING`] |
//! | menu button `min-h-9 gap-2.5 px-3 rounded-xl text-sm font-medium` | [`SIDEBAR_ITEM_HEIGHT`], [`SIDEBAR_ITEM_INNER_GAP`], [`SIDEBAR_ITEM_PADDING_X`], [`style::RADIUS_XL`] |
//! | active `<motion.span layoutId>` `absolute inset-0 rounded-xl bg-muted` + `SPRING_LAYOUT` | the pill's rect springs between items |
//! | label `{opacity, x: collapsed ? -4 : 0}` | [`SIDEBAR_LABEL_SHIFT`] with the reveal |
//! | `LABEL_ENTER_TRANSITION` `0.2s ease-out, delay 0.08` | [`SIDEBAR_LABEL_DELAY`] + [`SIDEBAR_LABEL_ENTER`] |
//! | `LABEL_EXIT_TRANSITION` `0.12s ease-out` | [`SIDEBAR_LABEL_EXIT`] |
//! | icon `size-5`, badge `text-xs text-muted-foreground` | [`SIDEBAR_ICON_SIZE`], [`style::TEXT_XS`] |
//! | item `text-muted-foreground`, active `text-foreground` | the resolved item inks |
//!
//! # What this port covers, and what it does not
//!
//! Upstream is not one component but a **provider plus twelve parts** —
//! `AnimatedSidebarProvider`, the panel, a portalled mobile drawer behind a
//! `(max-width: 767px)` media query, header/footer/content/group slots, a menu,
//! a submenu with its own clip-path stagger, a trigger button and a keyboard
//! shortcut. This module ports the **desktop panel and its menu**, which is what
//! carries the component's motion: the width morph, the staggered label
//! reveal/hide, and the shared active pill.
//!
//! Deliberately unported, each because it needs machinery outside one widget's
//! reach:
//!
//! - **The mobile drawer.** It is a `createPortal` sheet gated on a media query.
//!   frust's equivalent is [`crate::overlay::modal`] plus a
//!   `WindowMetrics`-driven decision that belongs to the *app*, not to a panel —
//!   an app picks a sidebar or a drawer, it does not hand one widget both.
//! - **`Ctrl/Cmd-B` and the trigger button.** Both are app-level: the panel here
//!   is **controlled** (`open` is a prop), so a host binds whatever chord and
//!   whatever button it likes and flips the flag.
//! - **Submenus.** Upstream's `SUBMENU_VARIANTS` is a `clipPath: inset(...)`
//!   reveal with `staggerChildren`; `PaintScene` has rectangular and
//!   rounded-rect clips but no inset-with-radius animation, and a nested
//!   disclosure inside a morphing rail is a component of its own. A host nests
//!   [`crate::components::file_tree`] in the content area for that shape today.
//! - **`side="right"`, `variant="floating"`/`"inset"`, `collapsible="offcanvas"`.**
//!   One geometry is ported: the left-hand `variant="sidebar"`,
//!   `collapsible="icon"` rail, which is upstream's own default and the only one
//!   whose collapse is a *width* animation rather than a translate.
//!
//! # The width morph drives layout, so paint asks for relayout
//!
//! The rail's reported width is `lerp(SIDEBAR_WIDTH_ICON, SIDEBAR_WIDTH,
//! fraction)`, and the fraction advances on a clock that only `paint` has. A
//! paint-only `request_frame` is therefore not enough: on the mobile intra-frame
//! layout skip (`docs/ARCHITECTURE.md`'s frame gate) layout would not re-run and
//! the width would freeze mid-morph. While the morph is in flight — and on the
//! frame it lands, and on the `reduce_motion` snap, neither of which reports
//! "still animating" — `paint` calls `PaintCtx::request_layout` instead, the
//! same rule `frust_glyph::accordion` states for the same reason.
//!
//! [`SIDEBAR_MORPH_SPRING`] is upstream's own local spring rather than a shared
//! token, and its own comment says why: the rail "settles at a hard zero-width
//! boundary", so it is authored just past critical damping and cannot overshoot.
//! Substituting [`SPRING_LAYOUT`] would put a rebound into a width that has a
//! wall at one end.
//!
//! # Degradations against the web original
//!
//! - **The label reveal is staggered, upstream's is uniform.** `LABEL_ENTER_TRANSITION`
//!   applies one `delay: 0.08` to every label at once. This port keeps that
//!   lead-in and then walks the labels [`SIDEBAR_LABEL_STAGGER`] apart down the
//!   rail, per the porting card, so the reveal reads as a cascade rather than a
//!   block. `reduce_motion` collapses it back to upstream's single beat.
//! - **No focus ring and no roving focus.** Upstream's item carries
//!   `focus-visible:ring-2`; the whole rail is one widget here, so there is no
//!   per-item focus target to ring.
//! - **A collapsed label is clipped, not `aria-hidden`.** Upstream hides the
//!   label and moves its text into the button's `aria-label`; here the label is
//!   always the item's accessible name and only its *painting* is clipped away.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, Color, EventCtx, EventResult,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, PointerPhase, Rect, Role, SemanticsCtx,
    Size, View, Widget, any, build_child, erase_callback_arg, rebuild_children, route_event,
    teardown_child,
    text::{FontWeight, TextStyle},
    visit_children,
};
use frust::{ChildKey, FrameTime, SpringDescription, Theme};

use crate::motion::{Ramp, Stagger};
use crate::press::presses;
use crate::style;
use crate::text::LabelRun;
use crate::tokens::motion::{EASE_OUT, SPRING_LAYOUT};

/// The expanded rail's width, in logical px (`--sidebar-width: 16rem`).
pub const SIDEBAR_WIDTH: f64 = 256.0;

/// The collapsed rail's width, in logical px (`--sidebar-width-icon: 4.25rem`).
pub const SIDEBAR_WIDTH_ICON: f64 = 68.0;

/// One menu item's height, in logical px (`min-h-9`).
pub const SIDEBAR_ITEM_HEIGHT: f64 = 36.0;

/// The gap between menu items, in logical px (the menu's `gap-1`).
pub const SIDEBAR_ITEM_GAP: f64 = 4.0;

/// A menu item's horizontal padding, in logical px (`px-3`).
pub const SIDEBAR_ITEM_PADDING_X: f64 = 12.0;

/// The gap between an item's icon, label and badge, in logical px (`gap-2.5`).
pub const SIDEBAR_ITEM_INNER_GAP: f64 = 10.0;

/// A menu icon's box, in logical px (`size-5`).
pub const SIDEBAR_ICON_SIZE: f64 = 20.0;

/// The rail's own padding, in logical px (the menu's `p-2`).
pub const SIDEBAR_PADDING: f64 = 8.0;

/// How far a hidden label sits from its resting x, in logical px
/// (`x: collapsed ? -4 : 0`).
pub const SIDEBAR_LABEL_SHIFT: f64 = 4.0;

/// The lead-in before labels start arriving (`LABEL_ENTER_TRANSITION`'s
/// `delay: 0.08`).
pub const SIDEBAR_LABEL_DELAY: Duration = Duration::from_millis(80);

/// A label's own reveal (`LABEL_ENTER_TRANSITION`: `0.2s`, `EASE_OUT`).
pub const SIDEBAR_LABEL_ENTER: Ramp = Ramp::eased(Duration::from_millis(200), EASE_OUT);

/// A label's own hide (`LABEL_EXIT_TRANSITION`: `0.12s`, `EASE_OUT`).
pub const SIDEBAR_LABEL_EXIT: Ramp = Ramp::eased(Duration::from_millis(120), EASE_OUT);

/// How far apart consecutive labels start — this port's addition to upstream's
/// single uniform delay. See the [module docs](self)' degradations.
pub const SIDEBAR_LABEL_STAGGER: Duration = Duration::from_millis(25);

/// The rail's width morph — upstream's own `SIDEBAR_MORPH_TRANSITION`, authored
/// just past critical damping so a width with a hard zero boundary cannot
/// rebound off it.
pub const SIDEBAR_MORPH_SPRING: SpringDescription = SpringDescription {
    mass: 0.75,
    stiffness: 380.0,
    damping: 35.0,
};

/// One menu entry: an icon view, its label, an optional trailing badge, and
/// whether it is the active one.
pub struct SidebarItem<State: 'static> {
    icon: AnyView<State>,
    label: String,
    badge: Option<String>,
    active: bool,
    disabled: bool,
}

/// Create a menu entry rendering `icon` beside `label`.
///
/// The icon is required rather than optional (upstream's is optional) so the
/// entry list and the child-pod list stay index-for-index aligned; pass an empty
/// leaf for an item that genuinely has none.
pub fn sidebar_item<State: 'static, V: View<State>>(
    icon: V,
    label: impl Into<String>,
) -> SidebarItem<State> {
    SidebarItem {
        icon: any(icon),
        label: label.into(),
        badge: None,
        active: false,
        disabled: false,
    }
}

impl<State: 'static> SidebarItem<State> {
    /// Add a trailing badge, shown only while the rail is expanded (upstream's
    /// `badge && !panel.collapsed`).
    pub fn badge(mut self, badge: impl Into<String>) -> Self {
        self.badge = Some(badge.into());
        self
    }

    /// Mark this entry active, so the shared pill rests behind it.
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// Make this entry inert: dimmed and unpressable.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// A view-held selection callback, erased on build.
type OnSelect<State> = Rc<dyn Fn(&mut State, usize)>;

/// A declarative beUI sidebar rail. See the [module docs](self).
pub struct AnimatedSidebarView<State: 'static> {
    open: bool,
    items: Vec<SidebarItem<State>>,
    on_select: OnSelect<State>,
}

/// Create a sidebar rail that is expanded while `open`, reporting the index of a
/// pressed entry through `on_select` — a **controlled** component: the rail
/// never flips its own `open`.
pub fn animated_sidebar<State: 'static, F: Fn(&mut State, usize) + 'static>(
    open: bool,
    items: Vec<SidebarItem<State>>,
    on_select: F,
) -> AnimatedSidebarView<State> {
    AnimatedSidebarView {
        open,
        items,
        on_select: Rc::new(on_select),
    }
}

/// The resolved rail palette.
struct SidebarColors {
    /// The rail's own surface (`bg-background`).
    surface: Color,
    /// The trailing border (`border-border`).
    border: Color,
    /// The active pill (`bg-muted`).
    pill: Color,
    /// The active item's ink (`text-foreground`).
    active_ink: Color,
    /// An inactive item's ink (`text-muted-foreground`).
    ink: Color,
}

/// Resolve the palette, falling back to the vendored **light** table unthemed.
fn resolve_colors(theme: Option<&Theme>) -> SidebarColors {
    match theme {
        Some(theme) => {
            let s = theme.scheme();
            SidebarColors {
                surface: s.surface,
                border: s.outline_variant,
                pill: s.surface_container_highest,
                active_ink: s.on_surface,
                ink: s.on_surface_variant,
            }
        }
        None => {
            let p = crate::BEUI_LIGHT;
            SidebarColors {
                surface: p.background,
                border: p.border,
                pill: p.muted,
                active_ink: p.foreground,
                ink: p.muted_foreground,
            }
        }
    }
}

/// The item label style (`text-sm font-medium`).
fn label_style(theme: Option<&Theme>) -> TextStyle {
    let family = theme.map_or_else(crate::tokens::sans_family, |t| {
        t.type_scale.label_large.family.clone()
    });
    TextStyle {
        family,
        weight: FontWeight::MEDIUM,
        ..TextStyle::new(style::TEXT_SM as f32, Color::BLACK)
    }
}

/// The badge style (`text-xs`).
fn badge_style(theme: Option<&Theme>) -> TextStyle {
    let family = theme.map_or_else(crate::tokens::sans_family, |t| {
        t.type_scale.label_small.family.clone()
    });
    TextStyle {
        family,
        ..TextStyle::new(style::TEXT_XS as f32, Color::BLACK)
    }
}

/// Linear interpolation between two rects, `t` unclamped.
fn lerp_rect(from: Rect, to: Rect, t: f64) -> Rect {
    let lerp = |a: f64, b: f64| a + (b - a) * t;
    Rect::new(
        lerp(from.x0, to.x0),
        lerp(from.y0, to.y0),
        lerp(from.x1, to.x1),
        lerp(from.y1, to.y1),
    )
}

/// One retained entry.
struct Entry {
    label: LabelRun,
    name: String,
    badge: Option<LabelRun>,
    badge_text: Option<String>,
    active: bool,
    disabled: bool,
}

impl Entry {
    fn from_item<State: 'static>(item: &SidebarItem<State>) -> Self {
        Self {
            label: LabelRun::new(item.label.clone()),
            name: item.label.clone(),
            badge: item.badge.clone().map(LabelRun::new),
            badge_text: item.badge.clone(),
            active: item.active,
            disabled: item.disabled,
        }
    }
}

impl<State: 'static> View<State> for AnimatedSidebarView<State> {
    type Element = AnimatedSidebarWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> AnimatedSidebarWidget {
        AnimatedSidebarWidget {
            entries: self.items.iter().map(Entry::from_item).collect(),
            icons: self
                .items
                .iter()
                .map(|item| build_child(&item.icon, ctx))
                .collect(),
            open: self.open,
            fraction: if self.open { 1.0 } else { 0.0 },
            morph_from: None,
            morph_started: None,
            pill_from: None,
            pill_shown: None,
            pill_started: None,
            hovered: None,
            captured: None,
            on_select: erase_callback_arg(&self.on_select),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AnimatedSidebarWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_select = erase_callback_arg(&self.on_select);
        let mut flags = ChangeFlags::NONE;

        if prev.items.len() != self.items.len() {
            element.entries = self.items.iter().map(Entry::from_item).collect();
            element.captured = None;
            element.hovered = None;
            element.pill_from = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else {
            let active_before = element.active_index();
            for (entry, item) in element.entries.iter_mut().zip(self.items.iter()) {
                if entry.name != item.label {
                    entry.label = LabelRun::new(item.label.clone());
                    entry.name = item.label.clone();
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
                if entry.badge_text != item.badge {
                    entry.badge = item.badge.clone().map(LabelRun::new);
                    entry.badge_text = item.badge.clone();
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
                if entry.active != item.active || entry.disabled != item.disabled {
                    entry.active = item.active;
                    entry.disabled = item.disabled;
                    flags |= ChangeFlags::PAINT;
                }
            }
            if active_before != element.active_index() {
                element.pill_from = element.pill_shown.or_else(|| element.pill_target());
                element.pill_started = None;
                flags |= ChangeFlags::PAINT;
            }
        }

        if prev.open != self.open {
            // Launch the morph from the width actually on screen, so a toggle
            // mid-morph reverses smoothly instead of jumping to an end stop.
            element.morph_from = Some(element.fraction);
            element.morph_started = None;
            element.open = self.open;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        flags |= rebuild_children(
            &prev.items,
            &self.items,
            &mut element.icons,
            ctx,
            |item: &SidebarItem<State>| &item.icon,
            |_| None::<ChildKey>,
        );
        flags
    }

    fn teardown(&self, element: &mut AnimatedSidebarWidget, ctx: &mut BuildCtx<'_>) {
        for (item, pod) in self.items.iter().zip(element.icons.iter_mut()) {
            teardown_child(&item.icon, pod, ctx);
        }
    }
}

/// The retained widget for an [`AnimatedSidebarView`].
pub struct AnimatedSidebarWidget {
    entries: Vec<Entry>,
    icons: Vec<ChildPod>,
    /// The app-confirmed expanded flag (source of truth, adopted on `rebuild`).
    open: bool,
    /// How expanded the rail is right now, `0` collapsed to `1` expanded. Written
    /// in `paint`, read by `layout`.
    fraction: f64,
    /// The fraction the current morph started from.
    morph_from: Option<f64>,
    /// The frame the current morph was first painted at.
    morph_started: Option<FrameTime>,
    /// The rect the active pill's travel started from.
    pill_from: Option<Rect>,
    /// The rect the pill painted last frame — the retarget origin.
    pill_shown: Option<Rect>,
    /// The frame the current pill travel was first painted at.
    pill_started: Option<FrameTime>,
    /// The latched hovered item, self-corrected from `PaintCtx::is_hovered`.
    hovered: Option<usize>,
    /// The item a `Down` armed.
    captured: Option<usize>,
    on_select: frust::authoring::ErasedArgCallback<usize>,
}

impl AnimatedSidebarWidget {
    /// The active entry's index, if any.
    fn active_index(&self) -> Option<usize> {
        self.entries.iter().position(|e| e.active)
    }

    /// Whether entry `index` can be pressed.
    fn enabled(&self, index: usize) -> bool {
        self.entries.get(index).is_some_and(|e| !e.disabled)
    }

    /// The rail's current width.
    pub fn width(&self) -> f64 {
        SIDEBAR_WIDTH_ICON + (SIDEBAR_WIDTH - SIDEBAR_WIDTH_ICON) * self.fraction
    }

    /// Entry `index`'s row box.
    fn item_rect(&self, index: usize) -> Option<Rect> {
        if index >= self.entries.len() {
            return None;
        }
        let y = SIDEBAR_PADDING + index as f64 * (SIDEBAR_ITEM_HEIGHT + SIDEBAR_ITEM_GAP);
        Some(Rect::from_origin_size(
            Point::new(SIDEBAR_PADDING, y),
            Size::new(
                (self.width() - SIDEBAR_PADDING * 2.0).max(0.0),
                SIDEBAR_ITEM_HEIGHT,
            ),
        ))
    }

    /// The active pill's resting rect.
    fn pill_target(&self) -> Option<Rect> {
        self.item_rect(self.active_index()?)
    }

    /// The entry under a widget-local `pos`, if any.
    fn hit_item(&self, pos: Point) -> Option<usize> {
        (0..self.entries.len())
            .find(|index| self.item_rect(*index).is_some_and(|r| r.contains(pos)))
    }

    /// The label stagger, collapsed to a single beat under reduced motion.
    fn label_stagger(&self, reduce_motion: bool, entering: bool) -> Stagger {
        let ramp = if entering {
            SIDEBAR_LABEL_ENTER
        } else {
            SIDEBAR_LABEL_EXIT
        };
        let stagger = Stagger::new(SIDEBAR_LABEL_STAGGER, ramp);
        if reduce_motion {
            stagger.collapsed()
        } else {
            stagger
        }
    }

    /// How revealed entry `index`'s label is, `0` hidden to `1` shown, given the
    /// morph's `elapsed` (or `None` when nothing is in flight).
    fn label_reveal(&self, index: usize, elapsed: Option<Duration>, reduce_motion: bool) -> f64 {
        let Some(elapsed) = elapsed else {
            return if self.open { 1.0 } else { 0.0 };
        };
        let count = self.entries.len();
        if self.open {
            let stagger = self.label_stagger(reduce_motion, true);
            let since = elapsed.saturating_sub(if reduce_motion {
                Duration::ZERO
            } else {
                SIDEBAR_LABEL_DELAY
            });
            stagger.progress_clamped(since, index, count)
        } else {
            let stagger = self.label_stagger(reduce_motion, false);
            1.0 - stagger.progress_clamped(elapsed, index, count)
        }
    }

    /// Advance the width morph to `now`, returning whether it is still running.
    fn advance_morph(&mut self, now: FrameTime, reduce_motion: bool) -> (Option<Duration>, bool) {
        let target = if self.open { 1.0 } else { 0.0 };
        if reduce_motion {
            self.morph_from = None;
            self.morph_started = None;
            self.fraction = target;
            return (None, false);
        }
        let Some(from) = self.morph_from else {
            self.fraction = target;
            return (None, false);
        };
        let ramp = Ramp::spring(SIDEBAR_MORPH_SPRING);
        let started = *self.morph_started.get_or_insert(now);
        let elapsed = now.saturating_sub(started);
        // The labels run on the same clock and outlast the width on the way in
        // (the lead-in plus the last label's own slot), so the morph is not over
        // until both are.
        let labels_done = self.label_stagger(reduce_motion, self.open).is_settled(
            elapsed.saturating_sub(SIDEBAR_LABEL_DELAY),
            self.entries.len(),
        );
        if ramp.is_settled(elapsed) && labels_done {
            self.morph_from = None;
            self.morph_started = None;
            self.fraction = target;
            return (Some(elapsed), false);
        }
        self.fraction = (from + (target - from) * ramp.progress(elapsed)).clamp(0.0, 1.0);
        (Some(elapsed), true)
    }

    /// Advance the active pill to `now`, returning the rect and whether it is
    /// still travelling.
    fn advance_pill(&mut self, now: FrameTime, reduce_motion: bool) -> (Option<Rect>, bool) {
        let Some(target) = self.pill_target() else {
            self.pill_shown = None;
            return (None, false);
        };
        let Some(from) = self.pill_from.filter(|_| !reduce_motion) else {
            self.pill_from = None;
            self.pill_started = None;
            self.pill_shown = Some(target);
            return (Some(target), false);
        };
        let ramp = Ramp::spring(SPRING_LAYOUT);
        let started = *self.pill_started.get_or_insert(now);
        let elapsed = now.saturating_sub(started);
        if ramp.is_settled(elapsed) {
            self.pill_from = None;
            self.pill_started = None;
            self.pill_shown = Some(target);
            return (Some(target), false);
        }
        let shown = lerp_rect(from, target, ramp.progress(elapsed));
        self.pill_shown = Some(shown);
        (Some(shown), true)
    }
}

impl Widget for AnimatedSidebarWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let label = label_style(theme);
        let badge = badge_style(theme);
        for entry in &mut self.entries {
            entry.label.layout(ctx, &label);
            if let Some(run) = &mut entry.badge {
                run.layout(ctx, &badge);
            }
        }

        let icon_bc =
            BoxConstraints::new(Size::ZERO, Size::new(SIDEBAR_ICON_SIZE, SIDEBAR_ICON_SIZE));
        for index in 0..self.entries.len() {
            let rect = self.item_rect(index);
            let Some(pod) = self.icons.get_mut(index) else {
                continue;
            };
            let size = pod.layout_child(ctx, &icon_bc);
            if let Some(rect) = rect {
                pod.set_origin(Point::new(
                    rect.x0 + SIDEBAR_ITEM_PADDING_X + (SIDEBAR_ICON_SIZE - size.width) / 2.0,
                    rect.y0 + (SIDEBAR_ITEM_HEIGHT - size.height) / 2.0,
                ));
            }
        }

        let count = self.entries.len();
        let height = SIDEBAR_PADDING * 2.0
            + count as f64 * SIDEBAR_ITEM_HEIGHT
            + count.saturating_sub(1) as f64 * SIDEBAR_ITEM_GAP;
        bc.constrain(Size::new(self.width(), height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if !ctx.is_hovered() {
            self.hovered = None;
        }

        let theme = Theme::from_paint_ctx(ctx);
        let colors = resolve_colors(theme);
        let reduce_motion = theme.is_some_and(|t| t.motion.reduce_motion);
        let origin = ctx.origin();

        let fraction_before = self.fraction;
        let (elapsed, morph_running) = self.advance_morph(ctx.frame_time(), reduce_motion);
        let (pill, pill_running) = self.advance_pill(ctx.frame_time(), reduce_motion);

        let size = Size::new(self.width(), ctx.size().height);

        // The rail's surface and its trailing hairline (`variant="sidebar"`).
        scene.fill_rect(origin, size, colors.surface);
        scene.fill_rect(
            Point::new(origin.x + size.width - style::BORDER_WIDTH, origin.y),
            Size::new(style::BORDER_WIDTH, size.height),
            colors.border,
        );

        // Everything below is clipped to the rail, so a label sliding out of a
        // collapsing panel disappears at its edge (`overflow-hidden`).
        scene.push_clip(origin, size);

        if let Some(rect) = pill {
            scene.fill_rounded_rect(
                Point::new(origin.x + rect.x0, origin.y + rect.y0),
                Size::new(rect.width().max(0.0), rect.height()),
                style::RADIUS_XL,
                colors.pill,
            );
        }

        for index in 0..self.entries.len() {
            let Some(rect) = self.item_rect(index) else {
                continue;
            };
            let dimmed = !self.enabled(index);
            let entry = &self.entries[index];
            let ink = if entry.active || self.hovered == Some(index) && !dimmed {
                colors.active_ink
            } else {
                colors.ink
            };
            let ink = style::disabled_tint(ink, dimmed, style::DISABLED_OPACITY);

            if let Some(pod) = self.icons.get_mut(index) {
                pod.paint_child(ctx, scene);
            }

            let reveal = self.label_reveal(index, elapsed, reduce_motion);
            if reveal > 0.0 {
                let entry = &self.entries[index];
                let label = entry.label.size();
                let x =
                    rect.x0 + SIDEBAR_ITEM_PADDING_X + SIDEBAR_ICON_SIZE + SIDEBAR_ITEM_INNER_GAP
                        - SIDEBAR_LABEL_SHIFT * (1.0 - reveal);
                entry.label.paint(
                    Point::new(
                        origin.x + x,
                        origin.y + rect.y0 + (SIDEBAR_ITEM_HEIGHT - label.height) / 2.0,
                    ),
                    style::scale_alpha(ink, reveal as f32),
                    scene,
                );

                // The badge rides the same reveal — upstream drops it outright
                // while collapsed (`badge && !panel.collapsed`), which is the
                // same thing once the reveal has reached zero.
                if let Some(run) = &entry.badge {
                    let badge = run.size();
                    run.paint(
                        Point::new(
                            origin.x + rect.x1 - SIDEBAR_ITEM_PADDING_X - badge.width,
                            origin.y + rect.y0 + (SIDEBAR_ITEM_HEIGHT - badge.height) / 2.0,
                        ),
                        style::scale_alpha(colors.ink, reveal as f32),
                        scene,
                    );
                }
            }
        }

        scene.pop_clip();

        // The width is computed in `layout` from `fraction`, so a bare frame
        // request would let the rail freeze mid-morph on the intra-frame layout
        // skip. Ask for layout while running, and once more on the frame the
        // value actually landed (which reports "not running").
        if morph_running {
            ctx.request_layout();
        }
        if self.fraction != fraction_before {
            ctx.request_layout();
        }
        if pill_running {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            for pod in &mut self.icons {
                pod.event_child(ctx, event);
            }
            return EventResult::Ignored;
        }
        if route_event(&mut self.icons, ctx, event) == EventResult::Handled {
            return EventResult::Handled;
        }

        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                if !presses(p) {
                    return EventResult::Ignored;
                }
                let Some(index) = self.hit_item(p.position).filter(|i| self.enabled(*i)) else {
                    return EventResult::Ignored;
                };
                self.captured = Some(index);
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if self.captured.is_some() {
                    ctx.set_cursor(style::ACTIVE_CURSOR);
                    return EventResult::Handled;
                }
                let over = self.hit_item(p.position).filter(|i| self.enabled(*i));
                if over.is_some() {
                    ctx.claim_hover();
                    ctx.set_cursor(style::ACTIVE_CURSOR);
                }
                if self.hovered != over {
                    self.hovered = over;
                    ctx.request_redraw();
                }
                EventResult::Ignored
            }
            PointerPhase::Up => {
                let Some(armed) = self.captured.take() else {
                    return EventResult::Ignored;
                };
                if self.hit_item(p.position) == Some(armed) {
                    (self.on_select)(ctx, armed);
                }
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if self.captured.take().is_none() {
                    return EventResult::Ignored;
                }
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::Navigation,
            |node| node.set_expanded(self.open),
            |ctx| {
                for entry in &self.entries {
                    ctx.push_node(Role::Button, |node| {
                        // The label is the accessible name in both states: a
                        // collapsed rail clips the painting, not the name.
                        node.set_label(entry.name.as_str());
                        node.set_selected(entry.active);
                        if entry.disabled {
                            node.set_disabled();
                        } else {
                            node.add_action(Action::Click);
                        }
                    });
                }
            },
        );
    }

    visit_children!(icons);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{BezPath, Brush, PointerButton, PointerEvent};
    use frust::text;
    use std::any::Any;

    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        rrects: Vec<(Point, Size, f64, Color)>,
        inks: Vec<Color>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.rects.push((o, s, c));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, r: f64, c: Color) {
            self.rrects.push((o, s, r, c));
        }
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, _w: f64, _b: &Brush) {}
        fn draw_glyph_run(&mut self, run: frust::authoring::scene::GlyphRun) {
            if let Brush::Solid(color) = run.brush {
                self.inks.push(color);
            }
        }
    }

    #[derive(Default)]
    struct Picked {
        last: Option<usize>,
        count: u32,
    }

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn items(active: usize) -> Vec<SidebarItem<Picked>> {
        vec![
            sidebar_item(text("H"), "Home").active(active == 0),
            sidebar_item(text("I"), "Inbox")
                .badge("12")
                .active(active == 1),
            sidebar_item(text("S"), "Settings").active(active == 2),
            sidebar_item(text("A"), "Archive").disabled(true),
        ]
    }

    fn view(open: bool, active: usize) -> AnimatedSidebarView<Picked> {
        animated_sidebar::<Picked, _>(open, items(active), |s: &mut Picked, i: usize| {
            s.last = Some(i);
            s.count += 1;
        })
    }

    fn build(open: bool, active: usize) -> AnimatedSidebarWidget {
        let mut counter = 0u64;
        View::<Picked>::build(&view(open, active), &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut AnimatedSidebarWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(
            &mut ctx,
            &BoxConstraints::new(Size::ZERO, Size::new(400.0, 600.0)),
        )
    }

    fn laid_out(open: bool, active: usize) -> (AnimatedSidebarWidget, Size) {
        let mut w = build(open, active);
        let size = layout(&mut w);
        (w, size)
    }

    fn paint_at(
        w: &mut AnimatedSidebarWidget,
        size: Size,
        theme: Option<&Theme>,
        ms: f64,
    ) -> (Recorder, bool, bool) {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(ms));
        if let Some(t) = theme {
            ctx = ctx.with_theme(t);
        }
        w.paint(&mut ctx, &mut rec);
        (rec, ctx.needs_frame(), ctx.needs_layout())
    }

    fn toggle(w: &mut AnimatedSidebarWidget, from: bool, to: bool, active: usize) {
        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        View::<Picked>::rebuild(&view(to, active), &view(from, active), w, &mut ctx);
    }

    fn pointer(phase: PointerPhase, position: Point) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position,
            button: PointerButton::Primary,
        })
    }

    fn dispatch(w: &mut AnimatedSidebarWidget, size: Size, event: &InputEvent, state: &mut Picked) {
        let mut ctx = EventCtx::new(state as &mut dyn Any, Point::ZERO, size);
        w.event(&mut ctx, event);
    }

    /// The two resting widths are upstream's own two CSS variables.
    #[test]
    fn the_rail_rests_at_one_of_the_two_upstream_widths() {
        let (_, expanded) = laid_out(true, 0);
        assert_eq!(expanded.width, SIDEBAR_WIDTH);
        let (_, collapsed) = laid_out(false, 0);
        assert_eq!(collapsed.width, SIDEBAR_WIDTH_ICON);
    }

    /// Collapsing springs the width down monotonically, asks for **layout** (not
    /// just a frame) the whole way, and lands exactly on the icon width.
    #[test]
    fn the_width_morph_springs_between_the_two_widths_and_drives_layout() {
        let (mut w, size) = laid_out(true, 0);
        paint_at(&mut w, size, None, 0.0);
        toggle(&mut w, true, false, 0);

        let (_, _, needs_layout) = paint_at(&mut w, size, None, 100.0);
        assert!(needs_layout, "a morphing rail must ask for relayout");
        let mut previous = w.width();
        assert_eq!(previous, SIDEBAR_WIDTH, "the first frame is still at rest");

        for step in 1..=8 {
            layout(&mut w);
            paint_at(&mut w, size, None, 100.0 + step as f64 * 20.0);
            assert!(
                w.width() <= previous,
                "the width grew mid-collapse: {} after {}",
                w.width(),
                previous
            );
            previous = w.width();
        }
        assert!(previous < SIDEBAR_WIDTH, "nothing moved: {previous}");

        paint_at(&mut w, size, None, 5_000.0);
        assert_eq!(w.width(), SIDEBAR_WIDTH_ICON);
        let (_, _, still) = paint_at(&mut w, size, None, 5_100.0);
        assert!(!still, "a settled rail asks for nothing");
    }

    /// A toggle mid-morph reverses from the width on screen rather than
    /// snapping to an end stop first.
    #[test]
    fn a_toggle_mid_morph_reverses_from_the_current_width() {
        let (mut w, size) = laid_out(true, 0);
        paint_at(&mut w, size, None, 0.0);
        toggle(&mut w, true, false, 0);
        paint_at(&mut w, size, None, 100.0);
        paint_at(&mut w, size, None, 140.0);
        let mid = w.fraction;
        assert!(mid > 0.0 && mid < 1.0, "not mid-morph: {mid}");

        toggle(&mut w, false, true, 0);
        assert_eq!(w.morph_from, Some(mid));
    }

    /// The label reveal is a real cascade: mid-run an earlier label is strictly
    /// further along than a later one, everything starts hidden, and everything
    /// ends shown.
    #[test]
    fn the_labels_reveal_as_a_staggered_cascade() {
        let (w, _) = laid_out(true, 0);
        let at = |ms: u64| {
            (0..4)
                .map(|i| w.label_reveal(i, Some(Duration::from_millis(ms)), false))
                .collect::<Vec<_>>()
        };

        assert_eq!(
            at(0),
            vec![0.0; 4],
            "nothing has arrived before the lead-in"
        );
        let mid = at(160);
        assert!(
            mid[0] > mid[1] && mid[1] > mid[2] && mid[2] > mid[3],
            "not a cascade: {mid:?}"
        );
        assert!(mid[0] > 0.0 && mid[3] < 1.0);
        assert_eq!(at(2_000), vec![1.0; 4], "everything lands");
    }

    /// `reduce_motion` collapses the cascade to upstream's single beat: every
    /// label reads identically, and the width snaps without asking for frames.
    #[test]
    fn reduce_motion_collapses_the_cascade_and_snaps_the_width() {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;

        let (w, _) = laid_out(true, 0);
        let readings: Vec<_> = (0..4)
            .map(|i| w.label_reveal(i, Some(Duration::from_millis(60)), true))
            .collect();
        assert!(
            readings.windows(2).all(|pair| pair[0] == pair[1]),
            "the collapse left a stagger: {readings:?}"
        );

        let (mut w, size) = laid_out(true, 0);
        paint_at(&mut w, size, Some(&theme), 0.0);
        toggle(&mut w, true, false, 0);
        let (_, _, needs_layout) = paint_at(&mut w, size, Some(&theme), 100.0);
        assert_eq!(w.width(), SIDEBAR_WIDTH_ICON, "the width snaps");
        assert!(
            needs_layout,
            "the snap still owes the one relayout that publishes the new width"
        );
        layout(&mut w);
        let (_, needs_frame, still) = paint_at(&mut w, size, Some(&theme), 120.0);
        assert!(!needs_frame && !still, "and then owes nothing at all");
    }

    /// A collapsed rail hides its labels and its badges; an expanded one inks
    /// both.
    #[test]
    fn a_collapsed_rail_paints_no_labels() {
        // The fixture's icons are text leaves, so four runs are the icons
        // themselves and everything above that is chrome this widget inked.
        let (mut collapsed, size) = laid_out(false, 0);
        let (rec, _, _) = paint_at(&mut collapsed, size, None, 0.0);
        assert_eq!(rec.inks.len(), 4, "a collapsed rail inks only its icons");

        let (mut expanded, size) = laid_out(true, 0);
        let (rec, _, _) = paint_at(&mut expanded, size, None, 0.0);
        assert_eq!(
            rec.inks.len(),
            4 + 5,
            "the icons plus four labels and one badge"
        );
    }

    /// The active pill rests behind the active item and springs onto the new one
    /// when the app moves it.
    #[test]
    fn the_active_pill_springs_between_items() {
        let (mut w, size) = laid_out(true, 0);
        paint_at(&mut w, size, None, 0.0);
        let from = w.item_rect(0).unwrap();
        assert_eq!(w.pill_shown, Some(from));

        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        View::<Picked>::rebuild(&view(true, 2), &view(true, 0), &mut w, &mut ctx);
        layout(&mut w);

        let (_, needs_frame, _) = paint_at(&mut w, size, None, 100.0);
        assert!(needs_frame, "a travelling pill owes frames");
        paint_at(&mut w, size, None, 140.0);
        let mid = w.pill_shown.unwrap();
        let to = w.item_rect(2).unwrap();
        assert!(mid.y0 > from.y0 && mid.y0 < to.y0, "mid-flight {mid:?}");

        paint_at(&mut w, size, None, 5_000.0);
        assert_eq!(w.pill_shown, Some(to));
    }

    /// A press reports its index; a disabled item reports nothing.
    #[test]
    fn a_press_reports_its_index_and_a_disabled_item_does_not() {
        let (mut w, size) = laid_out(true, 0);
        let mut state = Picked::default();
        let at = w.item_rect(1).unwrap().center();
        dispatch(&mut w, size, &pointer(PointerPhase::Down, at), &mut state);
        dispatch(&mut w, size, &pointer(PointerPhase::Up, at), &mut state);
        assert_eq!(state.last, Some(1));

        let disabled = w.item_rect(3).unwrap().center();
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Down, disabled),
            &mut state,
        );
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Up, disabled),
            &mut state,
        );
        assert_eq!(state.count, 1, "the disabled item reported nothing");
    }

    /// A collapsed rail still hit-tests its items across the narrower row, so an
    /// icon rail stays pressable.
    #[test]
    fn a_collapsed_rail_is_still_pressable() {
        let (mut w, size) = laid_out(false, 0);
        let mut state = Picked::default();
        let at = w.item_rect(2).unwrap().center();
        assert!(at.x < SIDEBAR_WIDTH_ICON);
        dispatch(&mut w, size, &pointer(PointerPhase::Down, at), &mut state);
        dispatch(&mut w, size, &pointer(PointerPhase::Up, at), &mut state);
        assert_eq!(state.last, Some(2));
    }
}
