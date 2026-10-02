//! Ports beUI's `bounce-sidebar` component.
//!
//! **Source:** `components/motion/bounce-sidebar.tsx` of the beUI monorepo, rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01.
//!
//! | upstream | here |
//! |---|---|
//! | list `flex-col gap-1.5 pl-6` | [`BOUNCE_ITEM_GAP`], [`BOUNCE_LIST_PADDING_LEFT`] |
//! | dot `absolute left-3 h-1.5 w-1.5 rounded-full bg-primary` | [`BOUNCE_DOT_LEFT`], [`BOUNCE_DOT_SIZE`] |
//! | item `min-h-9 gap-2 rounded-lg px-2 text-sm font-medium` | [`BOUNCE_ITEM_HEIGHT`], [`BOUNCE_ITEM_INNER_GAP`], [`BOUNCE_ITEM_PADDING_X`] |
//! | active `text-foreground`, else `text-muted-foreground hover:text-foreground` | the resolved item inks |
//! | `disabled` `opacity-40` | [`BOUNCE_DISABLED_OPACITY`] |
//! | `BOUNCE_SPRING` (280 / 18 / 0.3) | [`BOUNCE_SPRING`] |
//! | `quadraticBezier(0, controlX, 0, p)` / `(startY, controlY, destY, p)` | the arc [`BounceSidebarWidget`] traces |
//!
//! # Premise correction: the bounce is the *indicator's*, not the items'
//!
//! The porting card describes this slug as "the bouncier variant" of
//! [`animated_sidebar`](crate::components::animated_sidebar) with an
//! "item hover bounce". It is neither. Upstream's `bounce-sidebar.tsx` is a
//! **different component**: a plain, never-collapsing nav list whose only motion
//! is a 6px dot that travels between the selected rows, and whose items carry no
//! motion at all — hover is a `transition-colors` recolour and there is no
//! `whileHover`, no `whileTap` and no scale anywhere in the file. The port keeps
//! that: no item bounce is invented here.
//!
//! # The arc, transcribed
//!
//! The dot does not travel in a straight line. Upstream animates one `0 → 1`
//! progress and reads two quadratic Béziers off it, so the dot bows *out to the
//! left* and back while it falls:
//!
//! - `travel` is the absolute distance between the two dot positions.
//! - `long_jump = clamp((travel − 48) / 120, 0, 1)` — how much of a long jump
//!   this is. It softens the spring as the jump grows (stiffness `− 60·p`,
//!   damping `+ p`, mass `+ 0.15·p`), so a long fall takes visibly longer than
//!   a short one instead of whipping.
//! - the sideways control point is `−min(40, max(8, travel · 0.25))`: always at
//!   least an 8px bow, never more than 40.
//! - the vertical control point is `dest + (midpoint − dest) · long_jump`, which
//!   is the destination itself for a short hop (a straight fall) and bends
//!   toward the midpoint for a long one.
//!
//! Both curves are evaluated at the spring's **raw** progress, so the spring's
//! own overshoot past `1` carries into the arc — which is where the landing
//! bounce comes from — and the dot is snapped exactly onto its destination when
//! the ramp settles.
//!
//! # Degradations against the web original
//!
//! - **Selection is an index, not an id.** Upstream keys items by `id` and
//!   resolves a requested id back to an index for every measurement it makes;
//!   the list here is positional to begin with, so the index is the whole state.
//! - **No `ResizeObserver` snap.** Upstream re-snaps the dot whenever the list
//!   resizes. Here the dot's rest position is *derived* from the item geometry
//!   at every paint rather than cached, so a resize needs no observer: only a
//!   travel that is already in flight keeps its (now slightly stale) endpoints,
//!   for the few frames it has left.
//! - **No `href` items.** Upstream renders an `<a>` when given one; frust has no
//!   navigation primitive at this tier, so every entry reports through
//!   `on_select` and a host routes.
//! - **No focus ring.** The whole list is one widget, so there is no per-item
//!   focus target to ring.

use std::rc::Rc;

use frust::authoring::{
    Action, AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, Color, EventCtx, EventResult,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, PointerPhase, Rect, Role, SemanticsCtx,
    Size, View, Widget, any, build_child, erase_callback_arg, rebuild_children, route_event,
    teardown_child,
    text::{FontWeight, TextStyle},
    visit_children,
};
use frust::{ChildKey, FrameTime, SpringDescription, Theme};

use crate::motion::Ramp;
use crate::press::presses;
use crate::style;
use crate::text::{LabelRun, ThemeTextType, themed_style};

/// The travelling dot's diameter, in logical px (`DOT_SIZE` / `h-1.5 w-1.5`).
pub const BOUNCE_DOT_SIZE: f64 = 6.0;

/// The dot's resting x inside the list, in logical px (`left-3`).
pub const BOUNCE_DOT_LEFT: f64 = 12.0;

/// The list's left padding, in logical px (`pl-6`).
pub const BOUNCE_LIST_PADDING_LEFT: f64 = 24.0;

/// One row's height, in logical px (`min-h-9`).
pub const BOUNCE_ITEM_HEIGHT: f64 = 36.0;

/// The gap between rows, in logical px (`gap-1.5`).
pub const BOUNCE_ITEM_GAP: f64 = 6.0;

/// A row's horizontal padding, in logical px (`px-2`).
pub const BOUNCE_ITEM_PADDING_X: f64 = 8.0;

/// The gap between a row's icon and its label, in logical px (`gap-2`).
pub const BOUNCE_ITEM_INNER_GAP: f64 = 8.0;

/// A row's icon box, in logical px.
pub const BOUNCE_ICON_SIZE: f64 = 16.0;

/// A disabled row's opacity (`opacity-40`) — lower than the catalog's usual
/// [`style::DISABLED_OPACITY`], which is upstream's own choice here.
pub const BOUNCE_DISABLED_OPACITY: f32 = 0.40;

/// Travel below which a jump is entirely "short", in logical px — the numerator
/// offset in `(travel − 48) / 120`.
pub const BOUNCE_SHORT_JUMP: f64 = 48.0;

/// The travel range over which a jump grades from short to fully long, in
/// logical px — the denominator in `(travel − 48) / 120`.
pub const BOUNCE_JUMP_RANGE: f64 = 120.0;

/// The smallest sideways bow of the arc, in logical px (`max(8, …)`).
pub const BOUNCE_BOW_MIN: f64 = 8.0;

/// The largest sideways bow of the arc, in logical px (`min(40, …)`).
pub const BOUNCE_BOW_MAX: f64 = 40.0;

/// How much of the travel the sideways bow is, before clamping (`travel * 0.25`).
pub const BOUNCE_BOW_FRACTION: f64 = 0.25;

/// The dot's base spring — upstream's own `BOUNCE_SPRING`, "a compact, lightly
/// underdamped spring [that] gives the dot a quick landing without turning the
/// sidebar into a playful toy".
pub const BOUNCE_SPRING: SpringDescription = SpringDescription {
    mass: 0.3,
    stiffness: 280.0,
    damping: 18.0,
};

/// A quadratic Bézier at `progress` — upstream's own `quadraticBezier` helper,
/// transcribed.
fn quadratic_bezier(start: f64, control: f64, end: f64, progress: f64) -> f64 {
    let remaining = 1.0 - progress;
    remaining * remaining * start + 2.0 * remaining * progress * control + progress * progress * end
}

/// How long a jump of `travel` logical px is, in `[0, 1]` — upstream's
/// `longJumpProgress`.
fn long_jump(travel: f64) -> f64 {
    ((travel - BOUNCE_SHORT_JUMP) / BOUNCE_JUMP_RANGE).clamp(0.0, 1.0)
}

/// The spring a jump of `travel` is animated with: [`BOUNCE_SPRING`] softened
/// in proportion to [`long_jump`], exactly as upstream's spread does.
fn jump_spring(travel: f64) -> SpringDescription {
    let p = long_jump(travel);
    SpringDescription {
        mass: BOUNCE_SPRING.mass + 0.15 * p,
        stiffness: BOUNCE_SPRING.stiffness - 60.0 * p,
        damping: BOUNCE_SPRING.damping + p,
    }
}

/// One nav row: an icon view and its label.
pub struct BounceSidebarItem<State: 'static> {
    icon: AnyView<State>,
    label: String,
    disabled: bool,
}

/// Create a nav row rendering `icon` beside `label`.
///
/// The icon is required rather than optional (upstream's is optional) so the
/// entry list and the child-pod list stay index-for-index aligned; pass an empty
/// leaf for a row that genuinely has none.
pub fn bounce_sidebar_item<State: 'static, V: View<State>>(
    icon: V,
    label: impl Into<String>,
) -> BounceSidebarItem<State> {
    BounceSidebarItem {
        icon: any(icon),
        label: label.into(),
        disabled: false,
    }
}

impl<State: 'static> BounceSidebarItem<State> {
    /// Make this row inert: dimmed to [`BOUNCE_DISABLED_OPACITY`] and
    /// unpressable.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// A view-held selection callback, erased on build.
type OnSelect<State> = Rc<dyn Fn(&mut State, usize)>;

/// A declarative beUI bounce sidebar. See the [module docs](self).
pub struct BounceSidebarView<State: 'static> {
    selected: usize,
    items: Vec<BounceSidebarItem<State>>,
    on_select: OnSelect<State>,
}

/// Create a nav list whose dot rests beside item `selected`, reporting a
/// requested index through `on_select` — a **controlled** component.
pub fn bounce_sidebar<State: 'static, F: Fn(&mut State, usize) + 'static>(
    selected: usize,
    items: Vec<BounceSidebarItem<State>>,
    on_select: F,
) -> BounceSidebarView<State> {
    BounceSidebarView {
        selected,
        items,
        on_select: Rc::new(on_select),
    }
}

/// The resolved list palette.
struct BounceColors {
    /// The travelling dot (`bg-primary`).
    dot: Color,
    /// The selected row's ink (`text-foreground`).
    active_ink: Color,
    /// An unselected row's ink (`text-muted-foreground`).
    ink: Color,
}

/// Resolve the palette, falling back to the vendored **light** table unthemed.
fn resolve_colors(theme: Option<&Theme>) -> BounceColors {
    match theme {
        Some(theme) => {
            let s = theme.scheme();
            BounceColors {
                dot: s.primary,
                active_ink: s.on_surface,
                ink: s.on_surface_variant,
            }
        }
        None => {
            let p = crate::BEUI_LIGHT;
            BounceColors {
                dot: p.primary,
                active_ink: p.foreground,
                ink: p.muted_foreground,
            }
        }
    }
}

/// The row label style (`text-sm font-medium`), in the theme's `label_large`
/// family.
fn label_style(theme: Option<&Theme>) -> TextStyle {
    let style = TextStyle {
        weight: FontWeight::MEDIUM,
        ..TextStyle::new(style::TEXT_SM as f32, Color::BLACK)
    };
    themed_style(style, ThemeTextType::LabelLarge, theme)
}

/// One retained row.
struct Entry {
    label: LabelRun,
    name: String,
    disabled: bool,
}

/// A dot travel in flight: where it left, where it is going, and the arc it
/// takes between them.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Travel {
    start_y: f64,
    dest_y: f64,
    control_x: f64,
    control_y: f64,
    spring: SpringDescription,
    started: Option<FrameTime>,
}

impl Travel {
    /// Plan the arc from `start_y` to `dest_y`, exactly as upstream's
    /// `positionIndicator` does.
    fn plan(start_y: f64, dest_y: f64) -> Self {
        let travel = (dest_y - start_y).abs();
        let p = long_jump(travel);
        let midpoint_y = (start_y + dest_y) / 2.0;
        Travel {
            start_y,
            dest_y,
            control_x: -(travel * BOUNCE_BOW_FRACTION).clamp(BOUNCE_BOW_MIN, BOUNCE_BOW_MAX),
            control_y: dest_y + (midpoint_y - dest_y) * p,
            spring: jump_spring(travel),
            started: None,
        }
    }

    /// The dot's offset from its resting x/y at `progress`.
    fn at(&self, progress: f64) -> (f64, f64) {
        (
            quadratic_bezier(0.0, self.control_x, 0.0, progress),
            quadratic_bezier(self.start_y, self.control_y, self.dest_y, progress),
        )
    }
}

impl<State: 'static> View<State> for BounceSidebarView<State> {
    type Element = BounceSidebarWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> BounceSidebarWidget {
        BounceSidebarWidget {
            entries: self
                .items
                .iter()
                .map(|item| Entry {
                    label: LabelRun::new(item.label.clone()),
                    name: item.label.clone(),
                    disabled: item.disabled,
                })
                .collect(),
            icons: self
                .items
                .iter()
                .map(|item| build_child(&item.icon, ctx))
                .collect(),
            selected: self.selected,
            travel: None,
            dot: None,
            width: 0.0,
            hovered: None,
            captured: None,
            on_select: erase_callback_arg(&self.on_select),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut BounceSidebarWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_select = erase_callback_arg(&self.on_select);
        let mut flags = ChangeFlags::NONE;

        if prev.items.len() != self.items.len() {
            element.entries = self
                .items
                .iter()
                .map(|item| Entry {
                    label: LabelRun::new(item.label.clone()),
                    name: item.label.clone(),
                    disabled: item.disabled,
                })
                .collect();
            element.captured = None;
            element.hovered = None;
            element.travel = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else {
            for (entry, item) in element.entries.iter_mut().zip(self.items.iter()) {
                if entry.name != item.label {
                    entry.label = LabelRun::new(item.label.clone());
                    entry.name = item.label.clone();
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
                if entry.disabled != item.disabled {
                    entry.disabled = item.disabled;
                    flags |= ChangeFlags::PAINT;
                }
            }
        }

        if prev.selected != self.selected {
            // Leave from where the dot actually is, so a change mid-arc is one
            // continuous flight rather than a jump back to the old row.
            let start_y = element.dot.map_or_else(|| element.rest_y(), |dot| dot.1);
            element.selected = self.selected;
            element.travel = Some(Travel::plan(start_y, element.rest_y()));
            flags |= ChangeFlags::PAINT;
        }

        flags |= rebuild_children(
            &prev.items,
            &self.items,
            &mut element.icons,
            ctx,
            |item: &BounceSidebarItem<State>| &item.icon,
            |_| None::<ChildKey>,
        );
        flags
    }

    fn teardown(&self, element: &mut BounceSidebarWidget, ctx: &mut BuildCtx<'_>) {
        for (item, pod) in self.items.iter().zip(element.icons.iter_mut()) {
            teardown_child(&item.icon, pod, ctx);
        }
    }
}

/// The retained widget for a [`BounceSidebarView`].
pub struct BounceSidebarWidget {
    entries: Vec<Entry>,
    icons: Vec<ChildPod>,
    /// The app-confirmed selected index (source of truth, adopted on `rebuild`).
    selected: usize,
    /// The arc in flight, if any.
    travel: Option<Travel>,
    /// The dot's last painted position, in logical px.
    dot: Option<(f64, f64)>,
    /// The widest row layout resolved, used for the reported width.
    width: f64,
    /// The latched hovered row, self-corrected from `PaintCtx::is_hovered`.
    hovered: Option<usize>,
    /// The row a `Down` armed.
    captured: Option<usize>,
    on_select: frust::authoring::ErasedArgCallback<usize>,
}

impl BounceSidebarWidget {
    /// Whether row `index` can be pressed.
    fn enabled(&self, index: usize) -> bool {
        self.entries.get(index).is_some_and(|e| !e.disabled)
    }

    /// Row `index`'s box.
    fn item_rect(&self, index: usize) -> Option<Rect> {
        if index >= self.entries.len() {
            return None;
        }
        let y = index as f64 * (BOUNCE_ITEM_HEIGHT + BOUNCE_ITEM_GAP);
        Some(Rect::from_origin_size(
            Point::new(BOUNCE_LIST_PADDING_LEFT, y),
            Size::new(
                (self.width - BOUNCE_LIST_PADDING_LEFT).max(0.0),
                BOUNCE_ITEM_HEIGHT,
            ),
        ))
    }

    /// Where the dot rests beside the selected row —
    /// `offsetTop + (offsetHeight − DOT_SIZE) / 2`.
    fn rest_y(&self) -> f64 {
        let index = self.selected.min(self.entries.len().saturating_sub(1));
        let top = index as f64 * (BOUNCE_ITEM_HEIGHT + BOUNCE_ITEM_GAP);
        top + (BOUNCE_ITEM_HEIGHT - BOUNCE_DOT_SIZE) / 2.0
    }

    /// The row under a widget-local `pos`, if any.
    fn hit_item(&self, pos: Point) -> Option<usize> {
        (0..self.entries.len())
            .find(|index| self.item_rect(*index).is_some_and(|r| r.contains(pos)))
    }

    /// Advance the dot to `now`, returning its position and whether the arc is
    /// still in flight.
    fn advance_dot(&mut self, now: FrameTime, reduce_motion: bool) -> ((f64, f64), bool) {
        let rest = (0.0, self.rest_y());
        if self.entries.is_empty() {
            self.dot = None;
            return (rest, false);
        }
        let Some(travel) = self.travel.as_mut().filter(|_| !reduce_motion) else {
            self.travel = None;
            self.dot = Some(rest);
            return (rest, false);
        };
        let ramp = Ramp::spring(travel.spring);
        let started = *travel.started.get_or_insert(now);
        let elapsed = now.saturating_sub(started);
        if ramp.is_settled(elapsed) {
            self.travel = None;
            self.dot = Some(rest);
            return (rest, false);
        }
        // Raw progress: the spring's overshoot past 1 is what lands the dot with
        // a bounce rather than a stop.
        let at = travel.at(ramp.progress(elapsed));
        self.dot = Some(at);
        (at, true)
    }
}

impl Widget for BounceSidebarWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let text_style = label_style(theme);
        let mut content: f64 = 0.0;
        for entry in &mut self.entries {
            let label = entry.label.layout(ctx, &text_style);
            content = content.max(label.width);
        }
        self.width = (BOUNCE_LIST_PADDING_LEFT
            + BOUNCE_ITEM_PADDING_X * 2.0
            + BOUNCE_ICON_SIZE
            + BOUNCE_ITEM_INNER_GAP
            + content)
            .min(bc.max().width);

        let icon_bc =
            BoxConstraints::new(Size::ZERO, Size::new(BOUNCE_ICON_SIZE, BOUNCE_ICON_SIZE));
        for index in 0..self.entries.len() {
            let rect = self.item_rect(index);
            let Some(pod) = self.icons.get_mut(index) else {
                continue;
            };
            let size = pod.layout_child(ctx, &icon_bc);
            if let Some(rect) = rect {
                pod.set_origin(Point::new(
                    rect.x0 + BOUNCE_ITEM_PADDING_X + (BOUNCE_ICON_SIZE - size.width) / 2.0,
                    rect.y0 + (BOUNCE_ITEM_HEIGHT - size.height) / 2.0,
                ));
            }
        }

        let count = self.entries.len();
        let height =
            count as f64 * BOUNCE_ITEM_HEIGHT + count.saturating_sub(1) as f64 * BOUNCE_ITEM_GAP;
        bc.constrain(Size::new(self.width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if !ctx.is_hovered() {
            self.hovered = None;
        }

        let theme = Theme::from_paint_ctx(ctx);
        let colors = resolve_colors(theme);
        let reduce_motion = theme.is_some_and(|t| t.motion.reduce_motion);
        let origin = ctx.origin();
        let ((dot_x, dot_y), flying) = self.advance_dot(ctx.frame_time(), reduce_motion);

        // Rows: label ink follows selection, then hover, then the dimmed token.
        for (index, entry) in self.entries.iter().enumerate() {
            let Some(rect) = self.item_rect(index) else {
                continue;
            };
            let ink = if index == self.selected || (self.hovered == Some(index) && !entry.disabled)
            {
                colors.active_ink
            } else {
                colors.ink
            };
            let ink = style::disabled_tint(ink, entry.disabled, BOUNCE_DISABLED_OPACITY);
            let label = entry.label.size();
            entry.label.paint(
                Point::new(
                    origin.x
                        + rect.x0
                        + BOUNCE_ITEM_PADDING_X
                        + BOUNCE_ICON_SIZE
                        + BOUNCE_ITEM_INNER_GAP,
                    origin.y + rect.y0 + (BOUNCE_ITEM_HEIGHT - label.height) / 2.0,
                ),
                ink,
                scene,
            );
        }

        for pod in &mut self.icons {
            pod.paint_child(ctx, scene);
        }

        // The dot, over the rows, bowing left out of its rest column while it
        // travels.
        scene.fill_rounded_rect(
            Point::new(origin.x + BOUNCE_DOT_LEFT + dot_x, origin.y + dot_y),
            Size::new(BOUNCE_DOT_SIZE, BOUNCE_DOT_SIZE),
            BOUNCE_DOT_SIZE / 2.0,
            colors.dot,
        );

        // The arc moves within an already-measured box, so a bare frame request
        // is enough.
        if flying {
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
        let selected = self.selected;
        ctx.push_container(
            Role::Navigation,
            |_| {},
            |ctx| {
                for (index, entry) in self.entries.iter().enumerate() {
                    ctx.push_node(Role::Button, |node| {
                        node.set_label(entry.name.as_str());
                        node.set_selected(index == selected);
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
        rrects: Vec<(Point, Size, f64, Color)>,
        inks: Vec<Color>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
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

    fn items() -> Vec<BounceSidebarItem<Picked>> {
        vec![
            bounce_sidebar_item(text("O"), "Overview"),
            bounce_sidebar_item(text("R"), "Reports"),
            bounce_sidebar_item(text("T"), "Team"),
            bounce_sidebar_item(text("B"), "Billing"),
            bounce_sidebar_item(text("X"), "Archived").disabled(true),
        ]
    }

    fn view(selected: usize) -> BounceSidebarView<Picked> {
        bounce_sidebar::<Picked, _>(selected, items(), |s: &mut Picked, i: usize| {
            s.last = Some(i);
            s.count += 1;
        })
    }

    fn build(selected: usize) -> BounceSidebarWidget {
        let mut counter = 0u64;
        View::<Picked>::build(&view(selected), &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut BounceSidebarWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(
            &mut ctx,
            &BoxConstraints::new(Size::ZERO, Size::new(400.0, 600.0)),
        )
    }

    fn laid_out(selected: usize) -> (BounceSidebarWidget, Size) {
        let mut w = build(selected);
        let size = layout(&mut w);
        (w, size)
    }

    fn paint_at(
        w: &mut BounceSidebarWidget,
        size: Size,
        theme: Option<&Theme>,
        ms: f64,
    ) -> (Recorder, bool) {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(ms));
        if let Some(t) = theme {
            ctx = ctx.with_theme(t);
        }
        w.paint(&mut ctx, &mut rec);
        (rec, ctx.needs_frame())
    }

    fn select(w: &mut BounceSidebarWidget, from: usize, to: usize) {
        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        View::<Picked>::rebuild(&view(to), &view(from), w, &mut ctx);
    }

    fn pointer(phase: PointerPhase, position: Point) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position,
            button: PointerButton::Primary,
        })
    }

    fn dispatch(w: &mut BounceSidebarWidget, size: Size, event: &InputEvent, state: &mut Picked) {
        let mut ctx = EventCtx::new(state as &mut dyn Any, Point::ZERO, size);
        w.event(&mut ctx, event);
    }

    /// The Bézier helper is upstream's: anchored at both ends and pulled toward
    /// the control point in between.
    #[test]
    fn the_quadratic_bezier_is_anchored_and_pulled() {
        assert_eq!(quadratic_bezier(10.0, 90.0, 50.0, 0.0), 10.0);
        assert_eq!(quadratic_bezier(10.0, 90.0, 50.0, 1.0), 50.0);
        let half = quadratic_bezier(0.0, 100.0, 0.0, 0.5);
        assert_eq!(half, 50.0, "a symmetric bow peaks at half the control");
        // A control point above the straight line pulls the midpoint above it,
        // one below pulls it under — which is what bends the dot's fall.
        assert!(quadratic_bezier(0.0, 80.0, 100.0, 0.5) > 50.0);
        assert!(quadratic_bezier(0.0, 20.0, 100.0, 0.5) < 50.0);
    }

    /// The long-jump grading and the arc parameters it drives are upstream's
    /// numbers exactly: short hops are ungraded, the bow is clamped both ways,
    /// and a long jump softens the spring.
    #[test]
    fn a_long_jump_softens_the_spring_and_bends_the_arc() {
        assert_eq!(long_jump(0.0), 0.0);
        assert_eq!(long_jump(BOUNCE_SHORT_JUMP), 0.0);
        assert_eq!(long_jump(BOUNCE_SHORT_JUMP + BOUNCE_JUMP_RANGE), 1.0);
        assert_eq!(long_jump(1_000.0), 1.0, "clamped, never past one");

        assert_eq!(jump_spring(0.0), BOUNCE_SPRING, "a short hop is unmodified");
        let long = jump_spring(500.0);
        assert!(long.stiffness < BOUNCE_SPRING.stiffness);
        assert!(long.damping > BOUNCE_SPRING.damping);
        assert!(long.mass > BOUNCE_SPRING.mass);

        // The bow: at least 8px, at most 40, and always to the left.
        assert_eq!(Travel::plan(0.0, 4.0).control_x, -BOUNCE_BOW_MIN);
        assert_eq!(Travel::plan(0.0, 1_000.0).control_x, -BOUNCE_BOW_MAX);
        assert_eq!(Travel::plan(0.0, 100.0).control_x, -25.0);

        // The vertical control point is the destination for a short hop and
        // bends toward the midpoint for a long one.
        assert_eq!(Travel::plan(0.0, 40.0).control_y, 40.0);
        let long = Travel::plan(0.0, 400.0);
        assert_eq!(long.control_y, 200.0, "fully bent to the midpoint");
    }

    /// The dot rests beside the selected row, centred on it.
    #[test]
    fn the_dot_rests_centred_beside_the_selected_row() {
        for selected in 0..5 {
            let (w, _) = laid_out(selected);
            let row = w.item_rect(selected).unwrap();
            assert_eq!(
                w.rest_y(),
                row.y0 + (BOUNCE_ITEM_HEIGHT - BOUNCE_DOT_SIZE) / 2.0
            );
        }
    }

    /// A selection change flies the dot along the arc: it bows out to the left
    /// mid-flight, travels down, and lands exactly on the new rest position.
    #[test]
    fn the_dot_bows_left_while_it_travels_and_lands_on_the_new_row() {
        let (mut w, size) = laid_out(0);
        paint_at(&mut w, size, None, 0.0);
        let from = w.rest_y();
        select(&mut w, 0, 3);
        let to = w.rest_y();
        assert!(to > from);

        let (_, needs_frame) = paint_at(&mut w, size, None, 100.0);
        assert!(needs_frame, "a flying dot owes frames");
        assert_eq!(w.dot, Some((0.0, from)), "it leaves from the old row");

        let mut bowed = false;
        let mut previous = from;
        for step in 1..=10 {
            paint_at(&mut w, size, None, 100.0 + step as f64 * 15.0);
            let (x, y) = w.dot.unwrap();
            bowed |= x < -1.0;
            assert!(y >= previous - 1e-9, "the dot climbed back up: {y}");
            previous = y;
        }
        assert!(bowed, "the arc never bowed out to the left");

        let (_, still) = paint_at(&mut w, size, None, 5_000.0);
        assert!(!still, "a settled dot owes no frame");
        assert_eq!(w.dot, Some((0.0, to)));
    }

    /// A second change mid-flight leaves from where the dot is, so the two
    /// travels read as one continuous flight.
    #[test]
    fn a_change_mid_flight_leaves_from_where_the_dot_is() {
        let (mut w, size) = laid_out(0);
        paint_at(&mut w, size, None, 0.0);
        select(&mut w, 0, 3);
        paint_at(&mut w, size, None, 100.0);
        paint_at(&mut w, size, None, 140.0);
        let (_, mid_y) = w.dot.unwrap();

        select(&mut w, 3, 1);
        assert_eq!(w.travel.unwrap().start_y, mid_y);
        assert_eq!(w.travel.unwrap().dest_y, w.rest_y());
    }

    /// `reduce_motion` snaps the dot onto its row with no arc and no frames.
    #[test]
    fn reduce_motion_snaps_the_dot() {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        let (mut w, size) = laid_out(0);
        paint_at(&mut w, size, Some(&theme), 0.0);
        select(&mut w, 0, 3);
        let (rec, needs_frame) = paint_at(&mut w, size, Some(&theme), 100.0);
        assert!(!needs_frame);
        assert_eq!(w.dot, Some((0.0, w.rest_y())));
        assert!(
            rec.rrects.iter().any(|(o, s, _, c)| o.x == BOUNCE_DOT_LEFT
                && s.width == BOUNCE_DOT_SIZE
                && *c == crate::theme().scheme().primary),
            "the dot is painted at its rest column"
        );
    }

    /// A press reports its index; a disabled row reports nothing.
    #[test]
    fn a_press_reports_its_index_and_a_disabled_row_does_not() {
        let (mut w, size) = laid_out(0);
        let mut state = Picked::default();
        let at = w.item_rect(2).unwrap().center();
        dispatch(&mut w, size, &pointer(PointerPhase::Down, at), &mut state);
        dispatch(&mut w, size, &pointer(PointerPhase::Up, at), &mut state);
        assert_eq!(state.last, Some(2));
        assert_eq!(w.selected, 0, "the widget never writes its own selection");

        let disabled = w.item_rect(4).unwrap().center();
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
        assert_eq!(state.count, 1);
    }

    /// The selected row inks `foreground` and an unselected one
    /// `muted-foreground`, the recolour upstream's `transition-colors` makes.
    #[test]
    fn the_selected_row_takes_the_full_ink() {
        let p = crate::BEUI_LIGHT;
        let (mut w, size) = laid_out(1);
        let (rec, _) = paint_at(&mut w, size, None, 0.0);
        assert_eq!(rec.inks[0], p.muted_foreground);
        assert_eq!(rec.inks[1], p.foreground);
        assert_eq!(
            rec.inks[4],
            style::disabled_tint(p.muted_foreground, true, BOUNCE_DISABLED_OPACITY),
            "the disabled row is dimmed to upstream's own 40%"
        );
    }

    // ---- Typeface: the row labels follow the live theme ---------------------

    use crate::text::typeface_probe::{
        assert_follows_a_live_family_swap, assert_paints_only_in_geist,
    };

    const PROBE_WINDOW: Size = Size::new(400.0, 600.0);

    /// Two rows with glyph-free icons, so every painted run is a label.
    fn probe_view(_: &mut ()) -> BounceSidebarView<()> {
        let icon = || frust::SizedBox::<()>(Some(16.0), Some(16.0));
        bounce_sidebar::<(), _>(
            0,
            vec![
                bounce_sidebar_item(icon(), "Overview"),
                bounce_sidebar_item(icon(), "Reports"),
            ],
            |_: &mut (), _| {},
        )
    }

    #[test]
    fn row_labels_paint_in_geist_under_the_beui_theme() {
        assert_paints_only_in_geist("the sidebar's rows", probe_view, PROBE_WINDOW);
    }

    #[test]
    fn row_labels_follow_a_live_theme_family_swap() {
        assert_follows_a_live_family_swap("the sidebar's rows", probe_view, PROBE_WINDOW);
    }
}
