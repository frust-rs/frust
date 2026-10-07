//! `accordion`: the disclosure list — bordered items whose triggers reveal an
//! animated content panel.
//!
//! Source: `apps/v4/registry/new-york-v4/ui/accordion.tsx` (shadcn/ui v4, rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved 2026-08-17): an item is
//! `border-b last:border-b-0`; its trigger is `flex flex-1 items-start
//! justify-between gap-4 rounded-md py-4 text-left text-sm font-medium
//! hover:underline focus-visible:border-ring focus-visible:ring-[3px]
//! focus-visible:ring-ring/50 disabled:pointer-events-none disabled:opacity-50`
//! with a trailing `ChevronDownIcon` (`size-4 text-muted-foreground
//! transition-transform duration-200`, `rotate-180` while open); its content is
//! `overflow-hidden text-sm` around a `pt-0 pb-4` body, animated by the
//! `accordion-down`/`accordion-up` keyframes.
//!
//! # Controlled, in both modes
//!
//! The open set is a prop, never widget state: activating a trigger computes the
//! *requested* next set — replacing it in [`AccordionMode::Single`], toggling one
//! entry in [`AccordionMode::Multiple`] — and reports it through
//! `on_open_change`. An app that rejects the change keeps its own set, and the
//! next rebuild reinstates it.
//!
//! # Motion
//!
//! Each item owns an [`AnimationController`] over the reveal (200ms, the
//! `duration-200` the source's own chevron transition names, eased out). The
//! reveal changes the item's *height*, so a running animation asks for
//! `request_layout` rather than a bare `request_frame`
//! (`docs/WIDGETS_CODE_STANDARDS.md`'s animation rule — a paint-only request
//! leaves the mobile intra-frame layout skip showing an unresized panel). Under
//! `Theme.motion.reduce_motion` the reveal collapses to a jump: the controller is
//! stopped and the panel goes straight to its target, with no frames requested.
//!
//! # Focus and keyboard
//!
//! The widget takes focus for itself and remembers which trigger the pointer last
//! pressed; while it holds the focus path, Space and Enter toggle that trigger and
//! the focus ring is painted around it. Focus in this framework is per-*pod*, and
//! an accordion's triggers are regions of one widget rather than pods of their
//! own — so a per-trigger focus pod would mean a widget per trigger, which is a
//! shape this port deliberately does not take (the panel geometry has to be
//! resolved across items).

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, ErasedArgCallback, EventCtx,
    EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, PointerPhase, Role,
    SemanticsCtx, Size, ThemeTextColor, ThemeTextType, View, Widget, any, build_child,
    erase_callback_arg, rebuild_child, route_event, teardown_child, text::FontWeight,
    visit_children,
};
use frust::{AnimationController, Curve, TextView, Theme, text};

use crate::components::input::FALLBACK;
use crate::components::native_select::{activates, draw_chevron};
use crate::hit::{inside, presses};
use crate::style;
use crate::tokens::ShadcnTokens;

/// Trigger row padding: `py-4`.
const TRIGGER_PAD_Y: f64 = 16.0;
/// Gap between a trigger's label and its chevron: `gap-4`.
const TRIGGER_GAP: f64 = 16.0;
/// Content bottom padding: `pb-4` (`pt-0`, so the panel starts flush).
const CONTENT_PAD_BOTTOM: f64 = 16.0;
/// The chevron's own nudge: `translate-y-0.5`.
const CHEVRON_NUDGE_Y: f64 = 2.0;
/// Reveal duration: the `duration-200` the source's transition names.
const REVEAL_MS: u64 = 200;
/// Progress difference below which a reveal counts as settled.
const PROGRESS_EPSILON: f64 = 1e-4;

/// How many items an accordion keeps open — upstream's `type` prop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AccordionMode {
    /// `type="single"`: opening one closes the rest.
    #[default]
    Single,
    /// `type="multiple"`: each item toggles independently.
    Multiple,
}

/// A view-held, typed open-set callback (erased on build).
type OnOpenChange<State> = Rc<dyn Fn(&mut State, Vec<usize>)>;

/// One accordion item: a trigger label and the content it reveals.
pub struct AccordionItem<State: 'static> {
    title: String,
    content: AnyView<State>,
    disabled: bool,
}

/// Build an accordion item from its trigger `title` and its `content` view.
pub fn accordion_item<State: 'static, V: View<State>>(
    title: impl Into<String>,
    content: V,
) -> AccordionItem<State> {
    AccordionItem {
        title: title.into(),
        content: any(content),
        disabled: false,
    }
}

impl<State: 'static> AccordionItem<State> {
    /// Disable the item: inert, dimmed, [`style::DISABLED_CURSOR`].
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// A declarative accordion. See the [module docs](self).
pub struct AccordionView<State: 'static> {
    items: Vec<AccordionItem<State>>,
    open: Vec<usize>,
    mode: AccordionMode,
    on_open_change: OnOpenChange<State>,
}

/// Build a controlled accordion over `items`, with `open` holding the indices of
/// the currently open ones and every change reported through
/// `on_open_change(state, next_open)`.
pub fn accordion<State: 'static, I, O, F>(
    items: I,
    open: O,
    on_open_change: F,
) -> AccordionView<State>
where
    I: IntoIterator<Item = AccordionItem<State>>,
    O: IntoIterator<Item = usize>,
    F: Fn(&mut State, Vec<usize>) + 'static,
{
    let mut open: Vec<usize> = open.into_iter().collect();
    open.sort_unstable();
    open.dedup();
    AccordionView {
        items: items.into_iter().collect(),
        open,
        mode: AccordionMode::default(),
        on_open_change: Rc::new(on_open_change),
    }
}

impl<State: 'static> AccordionView<State> {
    /// Set the mode (default [`AccordionMode::Single`]).
    pub fn mode(mut self, mode: AccordionMode) -> Self {
        self.mode = mode;
        self
    }

    /// The trigger label view for item `index`: `text-sm font-medium`.
    // erasure: keep built into a ChildPod: build_child/rebuild_child/teardown_child take &AnyView
    fn title_view(&self, index: usize) -> AnyView<State> {
        any(trigger_text(&self.items[index].title))
    }
}

/// A trigger label: `text-sm font-medium` in `foreground`.
fn trigger_text(title: &str) -> TextView {
    text(title.to_string())
        .size(style::TEXT_SM as f32)
        .themed_family(ThemeTextType::LabelLarge)
        .weight(FontWeight::MEDIUM)
        .themed_role(ThemeTextColor::OnSurface)
}

/// The retained widget for an [`AccordionView`].
pub struct AccordionWidget {
    /// Every pod, flat and in paint order — `[title, content]` per item — so the
    /// whole accordion routes through [`route_event`] rather than a hand-rolled
    /// dispatch. `rows` carries the per-item state and geometry.
    pods: Vec<ChildPod>,
    rows: Vec<RowState>,
    mode: AccordionMode,
    /// The latched hovered trigger (self-corrected from `PaintCtx::is_hovered`).
    hovered: Option<usize>,
    pressed: Option<usize>,
    /// The trigger the keyboard acts on while this widget holds focus.
    focused: Option<usize>,
    captured: bool,
    on_open_change: ErasedArgCallback<Vec<usize>>,
}

/// Per-item state and geometry — the reveal animation plus what `layout`
/// resolved. The pods live in [`AccordionWidget::pods`], two per item.
///
/// # Why the controller is a *normalized* driver
///
/// [`AnimationController`] always starts at `0.0` and offers no way to seed a
/// value, so driving `progress` with it directly would make an item that is
/// *already open* on its first frame animate up from nothing. Instead the
/// controller runs a plain `0 → 1` ramp and the reveal interpolates
/// `from → target` across it: an item built open simply starts at `progress ==
/// 1.0` with an idle controller, and a reveal interrupted mid-flight restarts
/// from wherever it had reached.
struct RowState {
    disabled: bool,
    open: bool,
    /// The progress the running reveal started from.
    from: f64,
    /// The progress it is heading to (`1.0` open, `0.0` closed).
    target: f64,
    /// The reveal progress `layout` last used, updated in `paint`.
    progress: f64,
    anim: AnimationController,
    trigger_height: f64,
    top: f64,
    content_height: f64,
}

impl RowState {
    fn new(open: bool, disabled: bool) -> Self {
        let settled = if open { 1.0 } else { 0.0 };
        RowState {
            disabled,
            open,
            from: settled,
            target: settled,
            progress: settled,
            anim: reveal_controller(),
            trigger_height: 0.0,
            top: 0.0,
            content_height: 0.0,
        }
    }

    /// Start a reveal from the current progress toward `target`.
    fn start_reveal(&mut self, target: f64) {
        self.from = self.progress;
        self.target = target;
        self.anim = reveal_controller();
        self.anim.forward();
    }

    /// The progress this frame, given `now` and whether motion is reduced.
    /// Returns `(progress, still_animating)`.
    fn advance(&mut self, now: frust::FrameTime, reduce_motion: bool) -> (f64, bool) {
        if reduce_motion {
            // `reduce_motion` collapses the reveal to a jump — and stops asking for
            // frames.
            if self.anim.is_animating() {
                self.anim.stop();
            }
            return (self.target, false);
        }
        if !self.anim.is_animating() {
            return (self.progress, false);
        }
        let animating = self.anim.advance(now);
        let ramp = self.anim.value_clamped();
        (self.from + (self.target - self.from) * ramp, animating)
    }
}

/// A fresh `0 → 1` reveal ramp at the source's own `duration-200`, eased out.
fn reveal_controller() -> AnimationController {
    AnimationController::new(Duration::from_millis(REVEAL_MS)).with_curve(Curve::EaseOut)
}

impl<State: 'static> View<State> for AccordionView<State> {
    type Element = AccordionWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> AccordionWidget {
        let mut pods = Vec::new();
        let mut rows = Vec::new();
        for (index, item) in self.items.iter().enumerate() {
            pods.push(build_child(&self.title_view(index), ctx));
            pods.push(build_child(&item.content, ctx));
            // A freshly built open item starts revealed rather than animating in
            // (see [`RowState`]'s note on the normalized driver).
            rows.push(RowState::new(self.open.contains(&index), item.disabled));
        }
        AccordionWidget {
            pods,
            rows,
            mode: self.mode,
            hovered: None,
            pressed: None,
            focused: None,
            captured: false,
            on_open_change: erase_callback_arg(&self.on_open_change),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AccordionWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if self.items.len() != prev.items.len() {
            // The item set changed length: tear it down against the previous views
            // and rebuild (item identity here is positional).
            for (index, chunk) in element.pods.chunks_mut(2).enumerate() {
                teardown_child(&prev.title_view(index), &mut chunk[0], ctx);
                teardown_child(&prev.items[index].content, &mut chunk[1], ctx);
            }
            let rebuilt = View::<State>::build(self, ctx);
            *element = rebuilt;
            return ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        for index in 0..self.items.len() {
            flags |= rebuild_child(
                &prev.title_view(index),
                &self.title_view(index),
                &mut element.pods[index * 2],
                ctx,
            );
            flags |= rebuild_child(
                &prev.items[index].content,
                &self.items[index].content,
                &mut element.pods[index * 2 + 1],
                ctx,
            );
            let row = &mut element.rows[index];
            row.disabled = self.items[index].disabled;
            let open = self.open.contains(&index);
            if row.open != open {
                // The app confirmed (or rejected) the request: start the reveal
                // toward whatever it decided.
                row.open = open;
                row.start_reveal(if open { 1.0 } else { 0.0 });
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
        }
        if element.mode != self.mode {
            element.mode = self.mode;
        }
        element.on_open_change = erase_callback_arg(&self.on_open_change);
        flags
    }

    fn teardown(&self, element: &mut AccordionWidget, ctx: &mut BuildCtx<'_>) {
        for (index, chunk) in element.pods.chunks_mut(2).enumerate() {
            teardown_child(&self.title_view(index), &mut chunk[0], ctx);
            teardown_child(&self.items[index].content, &mut chunk[1], ctx);
        }
    }
}

impl AccordionWidget {
    /// The item whose *trigger row* is at widget-local `pos`.
    fn trigger_at(&self, pos: Point, size: Size) -> Option<usize> {
        if !inside(pos, size) {
            return None;
        }
        self.rows.iter().enumerate().find_map(|(index, row)| {
            (pos.y >= row.top && pos.y < row.top + row.trigger_height).then_some(index)
        })
    }

    /// The open set this widget would request if item `index` were activated.
    fn next_open(&self, index: usize) -> Vec<usize> {
        let open = |i: usize| self.rows[i].open;
        match self.mode {
            AccordionMode::Single => {
                if open(index) {
                    Vec::new()
                } else {
                    vec![index]
                }
            }
            AccordionMode::Multiple => {
                let mut next: Vec<usize> = (0..self.rows.len())
                    .filter(|i| if *i == index { !open(*i) } else { open(*i) })
                    .collect();
                next.sort_unstable();
                next
            }
        }
    }

    /// Report the open set item `index`'s activation asks for. The widget never
    /// changes its own `open` flags — the next rebuild does, from the app.
    fn toggle(&mut self, index: usize, ctx: &mut EventCtx) {
        if self.rows.get(index).is_none_or(|row| row.disabled) {
            return;
        }
        let next = self.next_open(index);
        (self.on_open_change)(ctx, next);
    }
}

impl Widget for AccordionWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        let label_width = (width - style::ICON_SIZE - TRIGGER_GAP).max(0.0);
        let mut y = 0.0;
        for index in 0..self.rows.len() {
            // The trigger row: `py-4` around the label, with the chevron beside it.
            let label = self.pods[index * 2].layout_child(
                ctx,
                &BoxConstraints::new(Size::ZERO, Size::new(label_width, f64::INFINITY)),
            );
            let trigger_height = label.height.max(style::ICON_SIZE) + 2.0 * TRIGGER_PAD_Y;
            self.pods[index * 2].set_origin(Point::new(0.0, y + TRIGGER_PAD_Y));

            // The content panel: laid out at full height and revealed by clipping,
            // which is what `overflow-hidden` plus a height animation does.
            let content = self.pods[index * 2 + 1].layout_child(
                ctx,
                &BoxConstraints::new(Size::ZERO, Size::new(width, f64::INFINITY)),
            );
            self.pods[index * 2 + 1].set_origin(Point::new(0.0, y + trigger_height));

            let row = &mut self.rows[index];
            row.top = y;
            row.trigger_height = trigger_height;
            row.content_height = content.height + CONTENT_PAD_BOTTOM;
            y += trigger_height + row.content_height * row.progress;
        }
        bc.constrain(Size::new(width, y))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Authoritative hover read: a pointer that left sends this widget nothing.
        if !ctx.is_hovered() {
            self.hovered = None;
        }
        let (origin, size) = (ctx.origin(), ctx.size());
        let now = ctx.frame_time();
        // Every theme read happens here, in one scope: the `&Theme` borrows the
        // context immutably, and everything below it (`request_layout`, the child
        // paints) needs it mutably.
        let (reduce_motion, border, muted, ink, radius, ring) = {
            let theme = Theme::from_paint_ctx(ctx);
            (
                theme.is_some_and(|t| t.motion.reduce_motion),
                theme.map_or(FALLBACK.border, |t| t.scheme().outline),
                theme.map_or(FALLBACK.muted_foreground, |t| t.scheme().on_surface_variant),
                theme.map_or(FALLBACK.foreground, |t| t.scheme().on_surface),
                ShadcnTokens::resolve_radius(None, theme).md,
                style::ring_color(None, theme),
            )
        };
        let focused_item = ctx.has_focus().then_some(self.focused).flatten();
        let last = self.rows.len().saturating_sub(1);

        // Advance every reveal first: the geometry `layout` used is a frame old, so
        // a progress change has to ask for a relayout, not just a repaint.
        let mut needs_layout = false;
        for row in &mut self.rows {
            let (next, animating) = row.advance(now, reduce_motion);
            if animating {
                needs_layout = true;
            }
            if (next - row.progress).abs() > PROGRESS_EPSILON {
                row.progress = next;
                needs_layout = true;
            }
        }
        if needs_layout {
            // A layout-affecting animation: `request_layout` implies a frame, and a
            // bare `request_frame` would leave the panel unresized on mobile.
            ctx.request_layout();
        }

        for index in 0..self.rows.len() {
            let (top, trigger_height, disabled, progress, content_height) = {
                let row = &self.rows[index];
                (
                    row.top,
                    row.trigger_height,
                    row.disabled,
                    row.progress,
                    row.content_height,
                )
            };
            let trigger_origin = Point::new(origin.x, origin.y + top);
            let trigger_size = Size::new(size.width, trigger_height);

            if focused_item == Some(index) && !disabled {
                style::draw_focus_ring(scene, trigger_origin, trigger_size, radius, ring);
            }

            // `[&[data-state=open]>svg]:rotate-180`, interpolated by the reveal.
            draw_chevron(
                scene,
                Point::new(
                    origin.x + size.width - style::ICON_SIZE / 2.0,
                    origin.y + top + trigger_height / 2.0 + CHEVRON_NUDGE_Y,
                ),
                style::ICON_SIZE,
                std::f64::consts::PI * progress,
                style::disabled_tint(muted, disabled),
            );

            // `hover:underline` under the trigger label.
            if self.hovered == Some(index) && !disabled {
                let label = &self.pods[index * 2];
                let label_origin = origin + label.origin().to_vec2();
                scene.fill_rect(
                    Point::new(label_origin.x, label_origin.y + label.size().height),
                    Size::new(label.size().width, style::BORDER_WIDTH),
                    ink,
                );
            }

            // `border-b last:border-b-0`.
            if index != last {
                let rule_y = origin.y + top + trigger_height + content_height * progress
                    - style::BORDER_WIDTH;
                scene.fill_rect(
                    Point::new(origin.x, rule_y),
                    Size::new(size.width, style::BORDER_WIDTH),
                    border,
                );
            }

            // The label, dimmed with the whole item when disabled.
            if disabled {
                scene.push_layer(trigger_origin, trigger_size, style::DISABLED_OPACITY);
                self.pods[index * 2].paint_child(ctx, scene);
                scene.pop_layer();
            } else {
                self.pods[index * 2].paint_child(ctx, scene);
            }

            // The content, clipped to the revealed band (`overflow-hidden`).
            let revealed = content_height * progress;
            if revealed > 0.0 {
                scene.push_clip(
                    Point::new(origin.x, origin.y + top + trigger_height),
                    Size::new(size.width, revealed),
                );
                self.pods[index * 2 + 1].paint_child(ctx, scene);
                scene.pop_clip();
            }
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // Content first: a control inside an open panel owns its own events, and
        // the trigger's hover claim must come *after* the routing.
        let routed = route_event(&mut self.pods, ctx, event);
        if event.is_broadcast() {
            return routed;
        }
        if routed == EventResult::Handled {
            return routed;
        }
        if let InputEvent::Key(key) = event {
            let Some(index) = self.focused else {
                return EventResult::Ignored;
            };
            if !activates(&key.key) || self.rows[index].disabled {
                return EventResult::Ignored;
            }
            self.toggle(index, ctx);
            ctx.request_redraw();
            return EventResult::Handled;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        let size = ctx.size();
        match p.phase {
            PointerPhase::Move => {
                if !self.captured {
                    let over = self.trigger_at(p.position, size);
                    let enabled = over.filter(|index| !self.rows[*index].disabled);
                    if over.is_some() {
                        // The container claims as a fallback, after routing.
                        ctx.claim_hover();
                        ctx.set_cursor(if enabled.is_some() {
                            style::ACTIVE_CURSOR
                        } else {
                            style::DISABLED_CURSOR
                        });
                    }
                    if self.hovered != enabled {
                        self.hovered = enabled;
                        ctx.request_redraw();
                    }
                    return EventResult::Ignored;
                }
                ctx.set_cursor(style::ACTIVE_CURSOR);
                let armed = self
                    .trigger_at(p.position, size)
                    .filter(|index| self.pressed == Some(*index));
                if self.pressed != armed {
                    self.pressed = armed;
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            PointerPhase::Down => {
                if !presses(p) {
                    return EventResult::Ignored;
                }
                let Some(index) = self
                    .trigger_at(p.position, size)
                    .filter(|index| !self.rows[*index].disabled)
                else {
                    return EventResult::Ignored;
                };
                self.pressed = Some(index);
                self.focused = Some(index);
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
                let armed = self.pressed.take();
                if let Some(index) = armed
                    && self.trigger_at(p.position, size) == Some(index)
                {
                    self.toggle(index, ctx);
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                // Flags and a redraw only: a Cancel arm never touches app state.
                self.captured = false;
                self.pressed = None;
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::Group,
            |_node| {},
            |ctx| {
                for (index, row) in self.rows.iter().enumerate() {
                    let title = &self.pods[index * 2];
                    let content = &self.pods[index * 2 + 1];
                    ctx.push_container(
                        Role::Button,
                        |node| {
                            node.set_expanded(row.open);
                            if row.disabled {
                                node.set_disabled();
                            } else {
                                node.add_action(Action::Click);
                            }
                        },
                        |ctx| title.semantics_child(ctx),
                    );
                    // A collapsed panel is not reachable, and offering a screen
                    // reader a control the user cannot get to is worse than
                    // omitting it — the same input-parity rule the navigator
                    // follows.
                    if row.open {
                        content.semantics_child(ctx);
                    }
                }
            },
        );
    }

    visit_children!(pods);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{
        Brush, Color, Key, KeyEvent, Modifiers, PointerButton, PointerEvent, Rect, Shape,
    };
    use frust::{Brightness, CursorIcon, FrameTime};
    use frust_core::RenderRoot;
    use std::any::Any;

    /// A fixed-size content leaf, generic over the app state (the shared
    /// `test_support::leaf` is `View<()>` only, and an accordion's content is a
    /// view over the app's own state).
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

    /// The content leaf every test hands its items.
    fn block() -> Block {
        Block(Size::new(100.0, CONTENT_H))
    }

    /// `FrameTime` `ms` past the epoch.
    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        clips: Vec<(Point, Size)>,
        layers: Vec<f32>,
        strokes: Vec<(Rect, f64, Color)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, origin: Point, size: Size, color: Color) {
            self.rects.push((origin, size, color));
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn push_clip(&mut self, origin: Point, size: Size) {
            self.clips.push((origin, size));
        }
        fn pop_clip(&mut self) {}
        fn push_layer(&mut self, _origin: Point, _size: Size, alpha: f32) {
            self.layers.push(alpha);
        }
        fn pop_layer(&mut self) {}
        fn stroke_path(
            &mut self,
            origin: Point,
            path: &frust::authoring::BezPath,
            width: f64,
            brush: &Brush,
        ) {
            let color = match brush {
                Brush::Solid(c) => *c,
                _ => Color::TRANSPARENT,
            };
            self.strokes
                .push((path.bounding_box() + origin.to_vec2(), width, color));
        }
    }

    impl Recorder {
        /// The 1px rules of `color`, by y origin.
        fn rules(&self, color: Color) -> Vec<f64> {
            self.rects
                .iter()
                .filter(|(_, size, c)| size.height == style::BORDER_WIDTH && *c == color)
                .map(|(origin, _, _)| origin.y)
                .collect()
        }

        fn rings(&self) -> Vec<Rect> {
            self.strokes
                .iter()
                .filter(|(_, w, _)| *w == style::FOCUS_RING_WIDTH)
                .map(|(bbox, _, _)| *bbox)
                .collect()
        }

        /// The chevron strokes' bounding boxes, in paint order.
        fn chevrons(&self) -> Vec<Rect> {
            self.strokes
                .iter()
                .filter(|(_, w, _)| *w != style::FOCUS_RING_WIDTH)
                .map(|(bbox, _, _)| *bbox)
                .collect()
        }
    }

    const WINDOW: Size = Size::new(320.0, 400.0);
    /// The content leaf's height, so the revealed band is predictable.
    const CONTENT_H: f64 = 40.0;

    #[derive(Default, Clone)]
    struct AppState {
        open: Vec<usize>,
        requests: Vec<Vec<usize>>,
    }

    struct Harness {
        root: RenderRoot<AppState, AccordionView<AppState>>,
        state: AppState,
        tcx: TextContext,
        mode: AccordionMode,
        disabled: Option<usize>,
        reduce_motion: bool,
    }

    impl Harness {
        fn new(mode: AccordionMode) -> Self {
            let mut h = Harness {
                root: RenderRoot::new(),
                state: AppState::default(),
                tcx: TextContext::new(),
                mode,
                disabled: None,
                reduce_motion: false,
            };
            h.theme();
            h
        }

        fn with_disabled(mode: AccordionMode, disabled: usize) -> Self {
            let mut h = Harness::new(mode);
            h.disabled = Some(disabled);
            h.pass();
            h
        }

        fn reduced(mode: AccordionMode) -> Self {
            let mut h = Harness::new(mode);
            h.reduce_motion = true;
            h.theme();
            h
        }

        fn theme(&mut self) {
            let mut theme = crate::theme().with_brightness(Brightness::Light);
            theme.motion.reduce_motion = self.reduce_motion;
            self.root.set_theme(Box::new(theme));
            self.pass();
        }

        fn pass(&mut self) -> Size {
            let (mode, disabled) = (self.mode, self.disabled);
            let mut logic = move |state: &mut AppState| {
                let items = (0..2).map(|index| {
                    accordion_item::<AppState, _>(format!("Item {index}"), block())
                        .disabled(disabled == Some(index))
                });
                accordion(items, state.open.clone(), |s: &mut AppState, next| {
                    s.requests.push(next.clone());
                    s.open = next;
                })
                .mode(mode)
            };
            self.root.rebuild(&mut logic, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any)
        }

        /// Paint at `ms` past the epoch, then re-lay out (a reveal is
        /// layout-affecting, so the geometry follows the paint).
        fn frame(&mut self, ms: f64) -> Recorder {
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, ft_ms(ms));
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
            rec
        }

        /// Run the reveal to rest, so the live geometry matches a freshly built
        /// widget's (whose open items start fully revealed).
        fn settle(&mut self) {
            self.frame(0.0);
            self.frame(REVEAL_MS as f64 * 4.0);
        }

        fn pointer(&mut self, phase: PointerPhase, y: f64) -> bool {
            self.root
                .event(
                    &mut self.state,
                    &InputEvent::Pointer(PointerEvent {
                        phase,
                        position: Point::new(40.0, y),
                        button: PointerButton::Primary,
                    }),
                )
                .needs_redraw
        }

        fn key(&mut self, key: Key) {
            self.root.event(
                &mut self.state,
                &InputEvent::Key(KeyEvent {
                    key,
                    modifiers: Modifiers::default(),
                    repeat: false,
                }),
            );
        }
    }

    fn build<S: 'static>(view: &AccordionView<S>) -> AccordionWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut AccordionWidget, size: Size) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(size))
    }

    /// A y inside the *second* trigger row when `open` is settled — measured off a
    /// standalone layout of the same view (whose open items start revealed, which
    /// is why the harness settles its animation first).
    fn second_trigger_y(open: Vec<usize>) -> f64 {
        let mut w = build(&two_items(open));
        layout(&mut w, WINDOW);
        w.rows[1].top + 10.0
    }

    fn two_items(open: Vec<usize>) -> AccordionView<AppState> {
        accordion(
            (0..2).map(|index| accordion_item::<AppState, _>(format!("Item {index}"), block())),
            open,
            |_s: &mut AppState, _next| {},
        )
    }

    #[test]
    fn a_closed_accordion_is_just_its_trigger_rows() {
        let mut w = build(&two_items(Vec::new()));
        let size = layout(&mut w, WINDOW);
        let trigger = w.rows[0].trigger_height;
        assert!(trigger >= 2.0 * TRIGGER_PAD_Y + style::ICON_SIZE, "py-4");
        assert_eq!(size.height, trigger * 2.0, "no content revealed");
        assert_eq!(w.rows[1].top, trigger);
        // The label is inset by the trigger padding and leaves the chevron room.
        assert_eq!(w.pods[0].origin().y, TRIGGER_PAD_Y);
        assert!(w.pods[0].size().width <= WINDOW.width - style::ICON_SIZE - TRIGGER_GAP);
    }

    #[test]
    fn an_open_item_reserves_its_content_plus_pb_4() {
        let mut w = build(&two_items(vec![0]));
        let size = layout(&mut w, WINDOW);
        let trigger = w.rows[0].trigger_height;
        assert_eq!(w.rows[0].content_height, CONTENT_H + CONTENT_PAD_BOTTOM);
        assert_eq!(size.height, trigger * 2.0 + CONTENT_H + CONTENT_PAD_BOTTOM);
        // The panel starts flush under its trigger (`pt-0`).
        assert_eq!(w.pods[1].origin().y, trigger);
        assert_eq!(w.rows[1].top, trigger + CONTENT_H + CONTENT_PAD_BOTTOM);
    }

    #[test]
    fn activating_a_trigger_requests_the_next_open_set_never_self_mutating() {
        let mut h = Harness::new(AccordionMode::Single);
        let trigger_y = 10.0;
        h.pointer(PointerPhase::Down, trigger_y);
        assert!(h.state.requests.is_empty(), "never on down");
        h.pointer(PointerPhase::Up, trigger_y);
        assert_eq!(h.state.requests, vec![vec![0]]);
        assert_eq!(h.state.open, vec![0], "the app applied its own set");

        // Single mode: opening the second closes the first.
        h.pass();
        h.settle();
        let second_y = second_trigger_y(vec![0]);
        h.pointer(PointerPhase::Down, second_y);
        h.pointer(PointerPhase::Up, second_y);
        assert_eq!(h.state.requests.last().unwrap(), &vec![1]);

        // ...and re-activating an open item closes it.
        h.pass();
        h.pointer(PointerPhase::Down, trigger_y);
        h.pointer(PointerPhase::Up, trigger_y);
        assert_eq!(h.state.requests.last().unwrap(), &vec![0]);
    }

    #[test]
    fn multiple_mode_toggles_one_entry_at_a_time() {
        let mut h = Harness::new(AccordionMode::Multiple);
        h.pointer(PointerPhase::Down, 10.0);
        h.pointer(PointerPhase::Up, 10.0);
        assert_eq!(h.state.open, vec![0]);
        h.pass();
        h.settle();
        let second_y = second_trigger_y(vec![0]);
        h.pointer(PointerPhase::Down, second_y);
        h.pointer(PointerPhase::Up, second_y);
        assert_eq!(h.state.open, vec![0, 1], "both stay open");
        h.pass();
        h.pointer(PointerPhase::Down, 10.0);
        h.pointer(PointerPhase::Up, 10.0);
        assert_eq!(h.state.open, vec![1], "the first toggled off alone");
    }

    #[test]
    fn the_reveal_animates_over_the_source_duration_and_settles() {
        let mut h = Harness::new(AccordionMode::Single);
        h.pointer(PointerPhase::Down, 10.0);
        h.pointer(PointerPhase::Up, 10.0);
        h.pass();
        // The first paint seeds the animation clock (zero delta), so the panel is
        // still closed; halfway through it is partly revealed; past the duration it
        // is fully open and stops asking for frames.
        h.frame(0.0);
        let mid = h.frame(REVEAL_MS as f64 / 2.0);
        let mid_clip = mid
            .clips
            .first()
            .copied()
            .expect("a partially revealed panel is clipped");
        assert!(mid_clip.1.height > 0.0);
        assert!(mid_clip.1.height < CONTENT_H + CONTENT_PAD_BOTTOM);

        let done = h.frame(REVEAL_MS as f64 * 2.0);
        let clip = done.clips.first().copied().expect("clip");
        assert!((clip.1.height - (CONTENT_H + CONTENT_PAD_BOTTOM)).abs() < 1e-6);
    }

    #[test]
    fn reduce_motion_jumps_the_reveal_and_requests_no_frames() {
        let mut h = Harness::reduced(AccordionMode::Single);
        h.pointer(PointerPhase::Down, 10.0);
        h.pointer(PointerPhase::Up, 10.0);
        h.pass();
        // The very first paint is already fully revealed — no intermediate frame.
        let rec = h.frame(0.0);
        let clip = rec.clips.first().copied().expect("clip");
        assert!((clip.1.height - (CONTENT_H + CONTENT_PAD_BOTTOM)).abs() < 1e-6);
    }

    #[test]
    fn hovering_a_trigger_underlines_its_label_and_the_latch_clears_on_leave() {
        let mut h = Harness::new(AccordionMode::Single);
        let ink = crate::theme()
            .with_brightness(Brightness::Light)
            .scheme()
            .on_surface;
        assert!(h.frame(0.0).rules(ink).is_empty(), "no underline at rest");

        assert!(h.pointer(PointerPhase::Move, 10.0), "entry repaints");
        assert_eq!(h.frame(0.0).rules(ink).len(), 1, "hover:underline");
        assert!(!h.pointer(PointerPhase::Move, 12.0), "unchanged latch");

        h.pointer(PointerPhase::Move, WINDOW.height - 1.0);
        assert!(h.frame(0.0).rules(ink).is_empty());
        assert_eq!(h.root.cursor(), CursorIcon::Default);
    }

    #[test]
    fn a_focused_trigger_rings_and_toggles_on_space_or_enter() {
        let mut h = Harness::new(AccordionMode::Multiple);
        assert!(h.frame(0.0).rings().is_empty());
        h.pointer(PointerPhase::Down, 10.0);
        h.pointer(PointerPhase::Up, 10.0);
        h.pass();
        let rings = h.frame(0.0).rings();
        assert_eq!(rings.len(), 1, "the pressed trigger holds focus");
        assert!(rings[0].y0 < 0.0, "the ring sits outside the trigger row");

        let before = h.state.requests.len();
        h.key(Key::Character(" ".to_string()));
        h.key(Key::Named(frust::authoring::NamedKey::Enter));
        assert_eq!(h.state.requests.len(), before + 2, "Space and Enter toggle");
        h.key(Key::Named(frust::authoring::NamedKey::Tab));
        assert_eq!(h.state.requests.len(), before + 2);
    }

    #[test]
    fn the_chevron_rotates_with_the_reveal() {
        // Closed: the chevron's bounding box is wider than it is tall (a "v").
        let mut w = build(&two_items(Vec::new()));
        let size = layout(&mut w, WINDOW);
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, size, FrameTime::ZERO);
        w.paint(&mut ctx, &mut rec);
        let closed = rec.chevrons()[0];
        assert!(closed.width() > closed.height());

        // Open (progress 1.0 after the build's own `animate_to`): rotated 180°, so
        // the box has the same shape but the glyph's own tip has flipped — assert on
        // the *rotation* through the mid-point of the stroke path instead.
        let mut w = build(&two_items(vec![0]));
        let size = layout(&mut w, WINDOW);
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, size, FrameTime::ZERO);
        w.paint(&mut ctx, &mut rec);
        let open = rec.chevrons()[0];
        assert!((open.width() - closed.width()).abs() < 1e-6);
        // A 180° rotation moves the glyph's centre of mass above its box centre.
        assert!(open.center().y < closed.center().y + 1e-6);
    }

    #[test]
    fn a_disabled_item_is_inert_dimmed_and_asks_not_allowed() {
        let mut h = Harness::with_disabled(AccordionMode::Single, 0);
        h.pointer(PointerPhase::Move, 10.0);
        assert_eq!(h.root.cursor(), CursorIcon::NotAllowed);
        h.pointer(PointerPhase::Down, 10.0);
        h.pointer(PointerPhase::Up, 10.0);
        assert!(
            h.state.requests.is_empty(),
            "a disabled trigger never fires"
        );
        let rec = h.frame(0.0);
        assert!(
            rec.layers.contains(&style::DISABLED_OPACITY),
            "disabled:opacity-50 over the trigger"
        );
    }

    #[test]
    fn the_last_item_drops_its_bottom_rule() {
        let mut w = build(&two_items(Vec::new()));
        let size = layout(&mut w, WINDOW);
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, size, FrameTime::ZERO);
        w.paint(&mut ctx, &mut rec);
        let rules = rec.rules(FALLBACK.border);
        assert_eq!(rules.len(), 1, "border-b last:border-b-0, got {rules:?}");
        assert_eq!(
            rules[0],
            w.rows[0].trigger_height - style::BORDER_WIDTH,
            "the rule sits at the first item's bottom edge"
        );
    }

    #[test]
    fn visit_children_publishes_both_pods_per_item() {
        let w = build(&two_items(vec![0]));
        let mut seen = 0usize;
        Widget::visit_children(&w, &mut |_pod| seen += 1);
        assert_eq!(seen, 4, "a label and a panel per item");
    }

    #[test]
    fn semantics_reports_expanded_triggers_and_omits_collapsed_panels() {
        let mut h = Harness::new(AccordionMode::Single);
        h.state.open = vec![0];
        h.pass();
        let update = h.root.semantics();
        let triggers: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::Button)
            .collect();
        assert_eq!(triggers.len(), 2);
        assert_eq!(
            triggers
                .iter()
                .filter(|(_, n)| n.is_expanded() == Some(true))
                .count(),
            1,
            "exactly one open trigger"
        );
        assert!(
            triggers
                .iter()
                .all(|(_, n)| n.supports_action(Action::Click))
        );
    }

    // ---- Typeface: the trigger labels follow the live theme --------------

    /// Two closed items over text-free content, so every painted glyph run is a
    /// trigger label.
    #[cfg(feature = "bundled-fonts")]
    fn two_closed_items(_: &mut ()) -> AccordionView<()> {
        accordion(
            [
                accordion_item("Shipping", Block(Size::new(10.0, 10.0))),
                accordion_item("Returns", Block(Size::new(10.0, 10.0))),
            ],
            Vec::new(),
            |_: &mut (), _| {},
        )
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_trigger_labels_paint_in_the_theme_face() {
        crate::text::typeface_probe::assert_paints_in_the_theme_face(
            "the accordion's trigger labels",
            two_closed_items,
        );
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_trigger_labels_follow_a_live_theme_swap() {
        crate::text::typeface_probe::assert_follows_a_live_theme_swap(
            "the accordion's trigger labels",
            two_closed_items,
        );
    }
}
