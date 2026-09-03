//! Ports beUI's `action-swap` component and its three registry examples —
//! `components/motion/action-swap.tsx` plus `action-swap-{blur,cascade,roll}.tsx`
//! (beUI rev `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01).
//!
//! One button that cycles through a list of labelled actions, animating the
//! hand-over with one of [`ActionSwapTransition`]'s four treatments. Upstream's
//! three example files are thin wrappers that pin `animation` — they are this
//! enum's three named arms here, not three separate components.
//!
//! # There is no blur in [`ActionSwapTransition::Blur`]
//!
//! Every one of upstream's swap treatments leans on a CSS `filter: blur(…)`:
//! `blur(8px)` on the blur swap, `blur(3px)` on the roll and the cascade.
//! frust's scene has no blur primitive at all, so **no arm of this port blurs
//! anything**. [`Blur`](ActionSwapTransition::Blur) keeps the rest of the
//! treatment — the opacity fade and the `scale: 0.94 → 1` pop — which is what
//! makes it read differently from the plain
//! [`Fade`](ActionSwapTransition::Fade); the roll and the cascade keep their
//! travel. The motion is therefore softer-edged upstream than here, and this is
//! the port's single largest visual departure.
//!
//! # `Fade` is this port's name for upstream's implicit fourth
//!
//! The source's `animation` prop takes `blur | roll | cascade` and defaults to
//! `blur`. This enum's `#[default]` is [`Blur`](ActionSwapTransition::Blur) for
//! that reason, which leaves the plain opacity crossfade — the treatment a
//! reader expects a "default" arm to name — needing a name of its own. It is
//! [`Fade`](ActionSwapTransition::Fade): the same swap with neither the scale
//! pop nor the travel, and the arm to reach for when the motion should be as
//! quiet as possible.
//!
//! # Departures from the source
//!
//! - **No blur**, as above.
//! - **No icon slot.** Upstream's `ActionSwapIcon` swaps an arbitrary React
//!   node beside (or instead of) the label. This catalog has no icon vocabulary
//!   and the crate is facade-only, so the port is text-only: an item is an id
//!   and a label.
//! - **The button reserves its widest item** rather than morphing its width as
//!   the label changes, the same trade the stateful button in
//!   [`button`](super::button) makes and for the same reason (a width morph is a
//!   relayout per frame).
//! - **Cycling is controlled.** Upstream keeps an internal `value` when none is
//!   passed; a frust control never mutates the value it was handed, so a press
//!   reports the id it wants through
//!   [`on_change`](ActionSwapView::on_change) and paints whatever the next
//!   rebuild feeds back.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, Affine, BoxConstraints, BuildCtx, ChangeFlags, Color, ErasedArgCallback, EventCtx,
    EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, PointerPhase, Role,
    SemanticsCtx, Size, TypedArgCallback, Vec2, View, Widget, erase_callback_arg,
};
use frust::{FrameTime, Theme};

use crate::motion::{CharCells, Ramp, Stagger};
use crate::style::{
    ACTIVE_CURSOR, DISABLED_OPACITY, HEIGHT_LG, HEIGHT_MD, HEIGHT_SM, PRESS_SCALE_CSS,
    RADIUS_CONTROL, TEXT_BASE, TEXT_SM, TEXT_XS, disabled_tint, resolve_radius, spacing,
};
use crate::tokens::motion::{EASE_IN_OUT, EASE_OUT, SPRING_PRESS, SPRING_SWAP};

use super::button::ButtonTone;
use crate::press::{SpringScalar, inside, presses, stroke_outline};
use crate::text::{LabelRun, label_style};

/// The blur swap's timing: `BLUR_TRANSITION = { duration: 0.2, ease: "easeInOut" }`,
/// resolved against beUI's own symmetric curve rather than the CSS keyword the
/// source names (`lib/ease.ts`'s stated position is that the defaults are weak).
const BLUR_RUN: Duration = Duration::from_millis(200);

/// The roll's exit: `ROLL_EXIT_TRANSITION = { duration: 0.14, ease: EASE_OUT }`.
const ROLL_EXIT: Duration = Duration::from_millis(140);

/// The cascade's per-cell exit: `{ duration: 0.16, ease: EASE_OUT }`, with each
/// cell's delay halved on the way out.
const CASCADE_EXIT: Duration = Duration::from_millis(160);
const CASCADE_EXIT_DELAY_FACTOR: f64 = 0.5;

/// The cascade's per-cell delay: `CASCADE_STAGGER = 0.025`.
const CASCADE_STEP: Duration = Duration::from_millis(25);

/// How far a rolling label travels, as a fraction of its own height:
/// `y: "90%"` on the roll, `"105%"` on the cascade's letters.
const ROLL_TRAVEL: f64 = 0.9;
const CASCADE_TRAVEL: f64 = 1.05;

/// The scale a blur-swapped label pops from: `scale: 0.94`.
const BLUR_SCALE: f64 = 0.94;

/// Which treatment a swap plays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ActionSwapTransition {
    /// Opacity alone — see the [module docs](self) for why this arm exists.
    Fade,
    /// Opacity plus a `0.94 → 1` scale pop. Upstream's default, and this enum's.
    /// **Paints no blur** — the source's `blur(8px)` has no frust equivalent.
    #[default]
    Blur,
    /// The label's graphemes roll in one at a time, left to right, the leaving
    /// string's letters dropping away as the arriving ones land.
    Cascade,
    /// The whole label rolls up out of its slot as the next one rolls in.
    Roll,
}

impl ActionSwapTransition {
    /// The ramp a label enters on.
    fn enter(self) -> Ramp {
        match self {
            ActionSwapTransition::Fade | ActionSwapTransition::Blur => {
                Ramp::eased(BLUR_RUN, EASE_IN_OUT)
            }
            ActionSwapTransition::Cascade | ActionSwapTransition::Roll => Ramp::spring(SPRING_SWAP),
        }
    }

    /// The ramp a label leaves on.
    fn exit(self) -> Ramp {
        match self {
            ActionSwapTransition::Fade | ActionSwapTransition::Blur => {
                Ramp::eased(BLUR_RUN, EASE_IN_OUT)
            }
            ActionSwapTransition::Cascade => Ramp::eased(CASCADE_EXIT, EASE_OUT),
            ActionSwapTransition::Roll => Ramp::eased(ROLL_EXIT, EASE_OUT),
        }
    }

    /// Whether the treatment animates per grapheme rather than per label.
    fn is_per_cell(self) -> bool {
        matches!(self, ActionSwapTransition::Cascade)
    }
}

/// The action-swap button's `size` prop.
///
/// Deliberately its own ladder rather than [`ButtonSize`](super::button::ButtonSize):
/// upstream gives this control tighter horizontal padding than the plain button
/// (`px-4` against `px-5` at the default size) and makes its icon size a circle
/// rather than the squared-off one the button uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ActionSwapSize {
    /// `h-8 gap-1.5 rounded-full px-3 text-xs`.
    Sm,
    /// `h-10 gap-2 rounded-full px-4 text-sm`.
    #[default]
    Md,
    /// `h-12 gap-2.5 rounded-full px-5 text-base`.
    Lg,
    /// `h-10 w-10 rounded-full` — a circle.
    Icon,
}

impl ActionSwapSize {
    fn height(self) -> f64 {
        match self {
            ActionSwapSize::Sm => HEIGHT_SM,
            ActionSwapSize::Md | ActionSwapSize::Icon => HEIGHT_MD,
            ActionSwapSize::Lg => HEIGHT_LG,
        }
    }

    fn pad_x(self) -> f64 {
        match self {
            ActionSwapSize::Sm => spacing(3.0),
            ActionSwapSize::Md => spacing(4.0),
            ActionSwapSize::Lg => spacing(5.0),
            ActionSwapSize::Icon => 0.0,
        }
    }

    fn font_size(self) -> f64 {
        match self {
            ActionSwapSize::Sm => TEXT_XS,
            ActionSwapSize::Md | ActionSwapSize::Icon => TEXT_SM,
            ActionSwapSize::Lg => TEXT_BASE,
        }
    }
}

/// One state the button cycles through: a stable id and the label it shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionSwapItem {
    /// The id reported through [`on_change`](ActionSwapView::on_change) and
    /// matched against [`value`](ActionSwapView::value).
    pub id: String,
    /// The text the button shows in this state.
    pub label: String,
}

impl ActionSwapItem {
    /// One item.
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
        }
    }
}

/// A declarative action-swap button.
pub struct ActionSwapView<State: 'static> {
    items: Vec<ActionSwapItem>,
    value: Option<String>,
    transition: ActionSwapTransition,
    tone: ButtonTone,
    size: ActionSwapSize,
    disabled: bool,
    on_change: TypedArgCallback<State, String>,
}

/// A button that shows one of `items` and reports the next one on every press.
///
/// Controlled: the shown item is whichever [`value`](ActionSwapView::value)
/// names (the first item until one is set), and a press reports the id it wants
/// through [`on_change`](ActionSwapView::on_change) rather than advancing on its
/// own.
pub fn action_swap<State: 'static>(items: Vec<ActionSwapItem>) -> ActionSwapView<State> {
    ActionSwapView {
        items,
        value: None,
        transition: ActionSwapTransition::default(),
        tone: ButtonTone::Secondary,
        size: ActionSwapSize::default(),
        disabled: false,
        on_change: Rc::new(|_, _| {}),
    }
}

impl<State: 'static> ActionSwapView<State> {
    /// Show the item with this id (default: the first item).
    pub fn value(mut self, value: impl Into<String>) -> Self {
        self.value = Some(value.into());
        self
    }

    /// Select the swap treatment (default [`ActionSwapTransition::Blur`]).
    pub fn transition(mut self, transition: ActionSwapTransition) -> Self {
        self.transition = transition;
        self
    }

    /// Select the fill treatment. Upstream's own default for this control is
    /// `secondary`, not the plain button's `primary`.
    pub fn tone(mut self, tone: ButtonTone) -> Self {
        self.tone = tone;
        self
    }

    /// Select the size (default [`ActionSwapSize::Md`]).
    pub fn size(mut self, size: ActionSwapSize) -> Self {
        self.size = size;
        self
    }

    /// Disable the button.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Handle a press: the argument is the id of the item the button is asking
    /// to move to.
    pub fn on_change(mut self, handler: impl Fn(&mut State, String) + 'static) -> Self {
        self.on_change = Rc::new(handler);
        self
    }

    /// The index the view's `value` selects, or `0`.
    fn index(&self) -> usize {
        self.value
            .as_deref()
            .and_then(|value| self.items.iter().position(|item| item.id == value))
            .unwrap_or(0)
    }
}

/// One item's shaped runs: the whole label, plus its per-grapheme cells for the
/// cascade.
struct ItemRuns {
    whole: LabelRun,
    cells: Vec<LabelRun>,
    /// Each cell's x offset from the label's left edge, and the summed width the
    /// cascade lays the cells out to.
    offsets: Vec<f64>,
    cells_width: f64,
}

impl ItemRuns {
    fn new(label: &str) -> Self {
        let split = CharCells::split(label);
        Self {
            whole: LabelRun::new(label),
            cells: split
                .cells()
                .iter()
                .map(|cell| LabelRun::new(cell.text()))
                .collect(),
            offsets: Vec::new(),
            cells_width: 0.0,
        }
    }
}

/// The retained widget for an [`ActionSwapView`].
pub struct ActionSwapWidget {
    runs: Vec<ItemRuns>,
    labels: Vec<String>,
    ids: Vec<String>,
    current: usize,
    /// The item being swapped away from, and the frame the swap started at.
    previous: Option<usize>,
    swap_started: Option<FrameTime>,
    transition: ActionSwapTransition,
    tone: ButtonTone,
    size: ActionSwapSize,
    disabled: bool,
    on_change: ErasedArgCallback<String>,
    hovered: bool,
    pressed: bool,
    captured: bool,
    scale: SpringScalar,
    /// The widest item's box, reserved once at layout.
    content: Size,
}

impl ActionSwapWidget {
    /// The id the next press would ask for — the following item, wrapping.
    fn next_id(&self) -> Option<&str> {
        if self.labels.is_empty() {
            return None;
        }
        let next = (self.current + 1) % self.labels.len();
        Some(self.ids_at(next))
    }

    /// The id of the item at `index`. Ids ride on the widget so a press can name
    /// one without the view being reachable.
    fn ids_at(&self, index: usize) -> &str {
        &self.ids[index]
    }
}

impl<State: 'static> View<State> for ActionSwapView<State> {
    type Element = ActionSwapWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ActionSwapWidget {
        ActionSwapWidget {
            runs: self
                .items
                .iter()
                .map(|item| ItemRuns::new(&item.label))
                .collect(),
            labels: self.items.iter().map(|item| item.label.clone()).collect(),
            ids: self.items.iter().map(|item| item.id.clone()).collect(),
            current: self.index(),
            previous: None,
            swap_started: None,
            transition: self.transition,
            tone: self.tone,
            size: self.size,
            disabled: self.disabled,
            on_change: erase_callback_arg(&self.on_change),
            hovered: false,
            pressed: false,
            captured: false,
            scale: SpringScalar::new(1.0, Ramp::spring(SPRING_PRESS)),
            content: Size::ZERO,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ActionSwapWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_change = erase_callback_arg(&self.on_change);
        let mut flags = ChangeFlags::NONE;

        if prev.items != self.items {
            element.runs = self
                .items
                .iter()
                .map(|item| ItemRuns::new(&item.label))
                .collect();
            element.labels = self.items.iter().map(|item| item.label.clone()).collect();
            element.ids = self.items.iter().map(|item| item.id.clone()).collect();
            element.previous = None;
            element.swap_started = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        let index = self.index().min(element.runs.len().saturating_sub(1));
        if index != element.current {
            // The swap is staged here and timed from the frame it first paints.
            element.previous = Some(element.current);
            element.swap_started = None;
            element.current = index;
            flags |= ChangeFlags::PAINT;
        }
        if prev.transition != self.transition {
            element.transition = self.transition;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.tone != self.tone {
            element.tone = self.tone;
            flags |= ChangeFlags::PAINT;
        }
        if prev.size != self.size {
            element.size = self.size;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.disabled != self.disabled {
            element.disabled = self.disabled;
            flags |= ChangeFlags::PAINT;
            if self.disabled {
                element.pressed = false;
                element.captured = false;
                element.hovered = false;
            }
        }
        flags
    }
}

impl Widget for ActionSwapWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let style = label_style(self.size.font_size());
        let per_cell = self.transition.is_per_cell();
        let mut content = Size::ZERO;
        for run in &mut self.runs {
            let whole = run.whole.layout(ctx, &style);
            content.height = content.height.max(whole.height);
            if per_cell {
                // The cascade lays its own cells out, so the label it reserves
                // is the summed cell width — measuring the whole string instead
                // can come out narrower (kerning) and clip the last glyph.
                run.offsets.clear();
                let mut x = 0.0;
                for cell in &mut run.cells {
                    let size = cell.layout(ctx, &style);
                    run.offsets.push(x);
                    x += size.width;
                    content.height = content.height.max(size.height);
                }
                run.cells_width = x;
                content.width = content.width.max(x);
            } else {
                content.width = content.width.max(whole.width);
            }
        }
        self.content = content;

        let height = self.size.height();
        if self.size == ActionSwapSize::Icon {
            return bc.constrain(Size::new(height, height));
        }
        bc.constrain(Size::new(content.width + self.size.pad_x() * 2.0, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let tone = self.tone.resolve(theme);
        let now = ctx.frame_time();
        let size = ctx.size();
        let origin = ctx.origin();
        let dim = self.disabled;
        if !dim && !self.captured {
            self.hovered = ctx.is_hovered();
        }

        let scale = if reduce {
            self.scale.jump_to(1.0);
            1.0
        } else {
            self.scale.set_target(if self.pressed && !dim {
                PRESS_SCALE_CSS
            } else {
                1.0
            });
            self.scale.advance(now)
        };
        let centre = origin + Vec2::new(size.width / 2.0, size.height / 2.0);
        if scale != 1.0 {
            scene.push_transform(
                Affine::translate(centre.to_vec2())
                    * Affine::scale(scale)
                    * Affine::translate(-centre.to_vec2()),
            );
        }

        let radius = resolve_radius(RADIUS_CONTROL, size.width, size.height);
        let active = !dim && (self.hovered || self.pressed);
        let (fill, ink) = if active {
            tone.active
        } else {
            (tone.fill, tone.ink)
        };
        let fill = disabled_tint(fill, dim, DISABLED_OPACITY);
        if fill.components[3] > 0.0 {
            scene.fill_rounded_rect(origin, size, radius, fill);
        }
        if let Some(border) = tone.border {
            stroke_outline(
                scene,
                origin,
                size,
                radius,
                disabled_tint(border, dim, DISABLED_OPACITY),
            );
        }
        let ink = disabled_tint(ink, dim, DISABLED_OPACITY);

        // `overflow-hidden`: a rolling label leaves through the control's own
        // edge rather than over whatever is beside it.
        scene.push_clip_rounded(origin, size, radius);
        self.paint_swap(scene, now, reduce, ink, origin, size);
        scene.pop_clip();

        if scale != 1.0 {
            scene.pop_transform();
        }

        if !reduce && (self.scale.is_animating() || self.previous.is_some()) {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if self.disabled {
            return EventResult::Ignored;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        let size = ctx.size();
        match p.phase {
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
            PointerPhase::Move => {
                let over = inside(p.position, size);
                if self.captured {
                    self.pressed = over;
                    ctx.set_cursor(ACTIVE_CURSOR);
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
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
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                if inside(p.position, size)
                    && let Some(next) = self.next_id().map(str::to_string)
                {
                    // Controlled: the next value is reported, never applied here.
                    (self.on_change)(ctx, next);
                }
                self.pressed = false;
                self.captured = false;
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.pressed = false;
                self.captured = false;
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::Button, |node| {
            if let Some(label) = self.labels.get(self.current) {
                node.set_label(label.as_str());
            }
            if self.disabled {
                node.set_disabled();
            } else {
                node.add_action(Action::Click);
            }
        });
    }
}

impl ActionSwapWidget {
    /// Paint the arriving label over the leaving one, staged by the selected
    /// treatment.
    fn paint_swap(
        &mut self,
        scene: &mut dyn PaintScene,
        now: FrameTime,
        reduce: bool,
        ink: Color,
        origin: Point,
        size: Size,
    ) {
        if self.runs.is_empty() {
            return;
        }
        let elapsed = self.previous.map(|_| {
            let started = *self.swap_started.get_or_insert(now);
            now.saturating_sub(started)
        });
        let enter = self.transition.enter();
        let exit = self.transition.exit();

        // A settled (or collapsed) swap paints the arriving label alone.
        let swapping = match (elapsed, reduce) {
            (Some(elapsed), false) => {
                let done = elapsed >= self.swap_total(&enter, &exit);
                if done {
                    self.previous = None;
                    self.swap_started = None;
                    None
                } else {
                    Some(elapsed)
                }
            }
            _ => {
                self.previous = None;
                self.swap_started = None;
                None
            }
        };

        let transition = self.transition;
        let content = self.content;
        let leaving = swapping.and(self.previous);
        let settled = swapping.is_none();
        let elapsed = swapping.unwrap_or(Duration::ZERO);

        if let Some(index) = leaving {
            paint_item(
                &self.runs[index],
                Staging {
                    transition,
                    stage: Stage::Leaving,
                    progress: exit.progress_clamped(elapsed),
                    elapsed,
                    ramp: exit,
                    settled: false,
                },
                scene,
                ink,
                origin,
                size,
                content,
            );
        }
        paint_item(
            &self.runs[self.current],
            Staging {
                transition,
                stage: Stage::Arriving,
                progress: if settled {
                    1.0
                } else {
                    enter.progress_clamped(elapsed)
                },
                elapsed,
                ramp: enter,
                settled,
            },
            scene,
            ink,
            origin,
            size,
            content,
        );
    }

    /// How long the whole hand-over takes: the slower of the two ramps, plus
    /// the last cell's own delay when the treatment staggers.
    fn swap_total(&self, enter: &Ramp, exit: &Ramp) -> Duration {
        let longest = enter.settle().max(exit.settle());
        if !self.transition.is_per_cell() {
            return longest;
        }
        let cells = self
            .runs
            .iter()
            .map(|run| run.cells.len())
            .max()
            .unwrap_or(0);
        longest + CASCADE_STEP.mul_f64(cells.saturating_sub(1) as f64)
    }
}

/// Which side of a swap a label is on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Arriving,
    Leaving,
}

/// Where one side of a swap is in its own run.
#[derive(Clone, Copy)]
struct Staging {
    transition: ActionSwapTransition,
    stage: Stage,
    /// Progress through [`ramp`](Staging::ramp), for a whole-label treatment.
    progress: f64,
    /// Time since the swap started, which a per-cell treatment staggers from.
    elapsed: Duration,
    ramp: Ramp,
    /// Whether there is no swap in flight at all, so the label paints at rest.
    settled: bool,
}

/// Paint one item's label, staged by `staging`.
fn paint_item(
    run: &ItemRuns,
    staging: Staging,
    scene: &mut dyn PaintScene,
    ink: Color,
    origin: Point,
    size: Size,
    content: Size,
) {
    let Staging {
        transition,
        stage,
        progress,
        elapsed,
        ramp,
        settled,
    } = staging;
    let width = if transition.is_per_cell() {
        run.cells_width
    } else {
        run.whole.size().width
    };
    let height = run.whole.size().height.max(content.height);
    let left = origin.x + (size.width - width) / 2.0;
    let top = origin.y + (size.height - height) / 2.0;

    if transition.is_per_cell() {
        let count = run.cells.len();
        if count == 0 {
            return;
        }
        let stagger = match stage {
            Stage::Arriving => Stagger::new(CASCADE_STEP, ramp),
            // The leaving string's cells go at half the arriving stagger, so
            // its tail lingers briefly.
            Stage::Leaving => Stagger::new(CASCADE_STEP.mul_f64(CASCADE_EXIT_DELAY_FACTOR), ramp),
        };
        for (index, cell) in run.cells.iter().enumerate() {
            let cell_progress = if settled {
                1.0
            } else {
                stagger.progress_clamped(elapsed, index, count)
            };
            let (alpha, travel) = stage_values(stage, cell_progress, CASCADE_TRAVEL);
            let cell_size = cell.size();
            let at = Point::new(
                left + run.offsets.get(index).copied().unwrap_or(0.0),
                top + travel * height,
            );
            if alpha <= 0.0 {
                continue;
            }
            scene.push_layer(at, cell_size, alpha as f32);
            cell.paint(at, ink, scene);
            scene.pop_layer();
        }
        return;
    }

    let (alpha, travel) = match transition {
        ActionSwapTransition::Roll => stage_values(stage, progress, ROLL_TRAVEL),
        _ => {
            let alpha = match stage {
                Stage::Arriving => progress,
                Stage::Leaving => 1.0 - progress,
            };
            (alpha, 0.0)
        }
    };
    if alpha <= 0.0 {
        return;
    }
    let at = Point::new(left, top + travel * height);
    let label_size = run.whole.size();

    // The blur treatment's surviving half: the `0.94 → 1` scale pop.
    let scale = if transition == ActionSwapTransition::Blur {
        BLUR_SCALE + (1.0 - BLUR_SCALE) * alpha
    } else {
        1.0
    };
    let centre = at + Vec2::new(label_size.width / 2.0, label_size.height / 2.0);
    if scale != 1.0 {
        scene.push_transform(
            Affine::translate(centre.to_vec2())
                * Affine::scale(scale)
                * Affine::translate(-centre.to_vec2()),
        );
    }
    scene.push_layer(at, label_size, alpha as f32);
    run.whole.paint(at, ink, scene);
    scene.pop_layer();
    if scale != 1.0 {
        scene.pop_transform();
    }
}

/// The opacity and the vertical travel one side of a swap is at, given its own
/// `progress` and the treatment's `travel` (as a fraction of the label's
/// height).
///
/// An arriving label rises from `+travel` to rest; a leaving one continues up to
/// `-travel`. Both are the source's own directions.
fn stage_values(stage: Stage, progress: f64, travel: f64) -> (f64, f64) {
    match stage {
        Stage::Arriving => (progress, (1.0 - progress) * travel),
        Stage::Leaving => (1.0 - progress, -progress * travel),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::text::TextContext;
    use frust::authoring::{BezPath, Brush, PointerButton, PointerEvent};
    use std::any::Any;

    #[derive(Default)]
    struct Log {
        asked: Vec<String>,
    }

    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<Color>,
        inks: Vec<Color>,
        alphas: Vec<f32>,
        layers: Vec<(Point, Size, f32)>,
        transforms: usize,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, _w: f64, brush: &Brush) {
            if let Brush::Solid(color) = brush {
                self.strokes.push(*color);
            }
        }
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            if let Brush::Solid(color) = run.brush {
                self.inks.push(color);
            }
        }
        fn push_layer(&mut self, origin: Point, size: Size, alpha: f32) {
            self.alphas.push(alpha);
            self.layers.push((origin, size, alpha));
        }
        fn push_transform(&mut self, _t: Affine) {
            self.transforms += 1;
        }
    }

    fn items() -> Vec<ActionSwapItem> {
        vec![
            ActionSwapItem::new("copy", "Copy"),
            ActionSwapItem::new("copied", "Copied"),
        ]
    }

    fn view(transition: ActionSwapTransition) -> ActionSwapView<Log> {
        action_swap::<Log>(items())
            .transition(transition)
            .on_change(|s: &mut Log, id| s.asked.push(id))
    }

    fn build(view: &ActionSwapView<Log>) -> ActionSwapWidget {
        let mut next_id = 0u64;
        View::<Log>::build(view, &mut BuildCtx::new(&mut next_id))
    }

    fn rebuild(prev: &ActionSwapView<Log>, next: &ActionSwapView<Log>, w: &mut ActionSwapWidget) {
        let mut next_id = 0u64;
        View::<Log>::rebuild(next, prev, w, &mut BuildCtx::new(&mut next_id));
    }

    fn lay_out(w: &mut ActionSwapWidget) -> Size {
        let mut text_ctx = TextContext::new();
        let mut ctx = LayoutCtx::with_resources(Some(&mut text_ctx as &mut dyn Any), None);
        w.layout(&mut ctx, &BoxConstraints::loose(Size::new(400.0, 200.0)))
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn dispatch(w: &mut ActionSwapWidget, state: &mut Log, size: Size, event: &InputEvent) {
        let any_state: &mut dyn Any = state;
        let mut ctx = EventCtx::new(any_state, Point::ZERO, size);
        w.event(&mut ctx, event);
    }

    fn paint_at(w: &mut ActionSwapWidget, size: Size, theme: &Theme, ms: u64) -> (Recorder, bool) {
        let mut ctx =
            PaintCtx::for_test(Point::ORIGIN, size, FrameTime::from_nanos(ms * 1_000_000))
                .with_theme(theme);
        let mut rec = Recorder::default();
        w.paint(&mut ctx, &mut rec);
        (rec, ctx.needs_frame())
    }

    fn theme() -> Theme {
        crate::theme()
    }

    fn reduced_theme() -> Theme {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        theme
    }

    /// Every registry variant constructs, lays out at its ladder height and
    /// paints — the acceptance check for the slug's four treatments.
    #[test]
    fn every_transition_and_size_builds_lays_out_and_paints() {
        for transition in [
            ActionSwapTransition::Fade,
            ActionSwapTransition::Blur,
            ActionSwapTransition::Cascade,
            ActionSwapTransition::Roll,
        ] {
            for (size, height) in [
                (ActionSwapSize::Sm, HEIGHT_SM),
                (ActionSwapSize::Md, HEIGHT_MD),
                (ActionSwapSize::Lg, HEIGHT_LG),
                (ActionSwapSize::Icon, HEIGHT_MD),
            ] {
                let v = view(transition).size(size);
                let mut w = build(&v);
                let laid = lay_out(&mut w);
                assert_eq!(laid.height, height, "{transition:?}/{size:?}");
                if size == ActionSwapSize::Icon {
                    assert_eq!(laid.width, height, "the icon size is a circle");
                }
                let (rec, _) = paint_at(&mut w, laid, &theme(), 0);
                assert!(!rec.inks.is_empty(), "{transition:?}/{size:?} painted text");
            }
        }
    }

    /// The box is the widest item's, so cycling never resizes the control.
    #[test]
    fn the_button_reserves_its_widest_item() {
        let v = view(ActionSwapTransition::Blur);
        let mut w = build(&v);
        let first = lay_out(&mut w);

        let second = action_swap::<Log>(items())
            .transition(ActionSwapTransition::Blur)
            .value("copied");
        rebuild(&v, &second, &mut w);
        assert_eq!(lay_out(&mut w), first);
    }

    /// Controlled cycling: a press asks for the *next* id and changes nothing.
    #[test]
    fn a_press_asks_for_the_next_item_and_wraps() {
        let v = view(ActionSwapTransition::Blur);
        let mut w = build(&v);
        let size = lay_out(&mut w);
        let mut state = Log::default();

        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Down, 5.0, 5.0));
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Up, 5.0, 5.0));
        assert_eq!(state.asked, vec!["copied".to_string()]);
        assert_eq!(w.current, 0, "the widget waits for the rebuild");

        // Confirm the move, then press again: it wraps back to the first.
        let second = view(ActionSwapTransition::Blur).value("copied");
        rebuild(&v, &second, &mut w);
        assert_eq!(w.current, 1);
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Down, 5.0, 5.0));
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Up, 5.0, 5.0));
        assert_eq!(state.asked, vec!["copied".to_string(), "copy".to_string()]);
    }

    #[test]
    fn a_release_outside_and_a_cancel_both_report_nothing() {
        let v = view(ActionSwapTransition::Blur);
        let mut w = build(&v);
        let size = lay_out(&mut w);
        let mut state = Log::default();

        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Down, 5.0, 5.0));
        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Up, size.width + 30.0, 5.0),
        );
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Down, 5.0, 5.0));
        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Cancel, 5.0, 5.0),
        );
        assert!(state.asked.is_empty());
    }

    #[test]
    fn a_disabled_button_takes_no_input() {
        let v = view(ActionSwapTransition::Blur).disabled(true);
        let mut w = build(&v);
        let size = lay_out(&mut w);
        let mut state = Log::default();
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Down, 5.0, 5.0));
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Up, 5.0, 5.0));
        assert!(state.asked.is_empty());
        assert!(!w.captured);
    }

    /// A staged swap composites both labels, and retires itself once both ramps
    /// have run.
    #[test]
    fn a_swap_shows_both_labels_and_then_settles() {
        let first = view(ActionSwapTransition::Roll);
        let mut w = build(&first);
        let size = lay_out(&mut w);
        let (rec, more) = paint_at(&mut w, size, &theme(), 0);
        assert_eq!(rec.alphas.len(), 1, "one label at rest");
        assert!(!more, "and nothing owed");

        let second = view(ActionSwapTransition::Roll).value("copied");
        rebuild(&first, &second, &mut w);
        // The frame the swap starts on: the leaving label is still fully
        // opaque, and the arriving one is not yet visible enough to composite.
        let (rec, more) = paint_at(&mut w, size, &theme(), 0);
        assert_eq!(rec.alphas.len(), 1);
        assert!((rec.alphas[0] - 1.0).abs() < 1e-6);
        assert!(more);

        // Mid-swap both are on screen, the leaving one fading out under the
        // arriving one.
        let (rec, _) = paint_at(&mut w, size, &theme(), 60);
        assert_eq!(rec.alphas.len(), 2, "both labels mid-swap");
        assert!(
            rec.alphas[0] > 0.0 && rec.alphas[0] < 1.0,
            "{:?}",
            rec.alphas
        );
        assert!(rec.alphas[1] > 0.0 && rec.alphas[1] < 1.0);

        let (rec, more) = paint_at(&mut w, size, &theme(), 3_000);
        assert_eq!(rec.alphas.len(), 1);
        assert!(!more);
        assert!(w.previous.is_none());
    }

    /// The roll moves its labels vertically; the fade does not.
    #[test]
    fn the_roll_travels_and_the_fade_stays_put() {
        for (transition, travels) in [
            (ActionSwapTransition::Roll, true),
            (ActionSwapTransition::Fade, false),
        ] {
            let first = view(transition);
            let mut w = build(&first);
            let size = lay_out(&mut w);
            let resting = paint_at(&mut w, size, &theme(), 0).0.layers[0].0.y;

            let second = view(transition).value("copied");
            rebuild(&first, &second, &mut w);
            paint_at(&mut w, size, &theme(), 0);
            let mid = paint_at(&mut w, size, &theme(), 40).0;
            let arriving = mid.layers[1].0.y;
            assert_eq!(
                (arriving - resting).abs() > 1.0,
                travels,
                "{transition:?} travel"
            );
        }
    }

    /// The blur treatment keeps its scale pop — the half of it frust can paint.
    /// The fade, which is the same swap without it, pushes no transform.
    #[test]
    fn only_the_blur_treatment_scales_its_labels() {
        for (transition, scales) in [
            (ActionSwapTransition::Blur, true),
            (ActionSwapTransition::Fade, false),
        ] {
            let first = view(transition);
            let mut w = build(&first);
            let size = lay_out(&mut w);
            let second = view(transition).value("copied");
            rebuild(&first, &second, &mut w);
            paint_at(&mut w, size, &theme(), 0);
            let mid = paint_at(&mut w, size, &theme(), 60).0;
            assert_eq!(mid.transforms > 0, scales, "{transition:?}");
        }
    }

    /// The cascade animates per grapheme: one composited layer per letter of
    /// each label, not one per label.
    #[test]
    fn the_cascade_composites_one_layer_per_letter() {
        let first = view(ActionSwapTransition::Cascade);
        let mut w = build(&first);
        let size = lay_out(&mut w);
        let (rec, _) = paint_at(&mut w, size, &theme(), 0);
        assert_eq!(rec.alphas.len(), "Copy".len(), "four letters at rest");

        let second = view(ActionSwapTransition::Cascade).value("copied");
        rebuild(&first, &second, &mut w);
        paint_at(&mut w, size, &theme(), 0);
        let mid = paint_at(&mut w, size, &theme(), 30).0;
        // Both strings' letters, minus whichever have faded fully out or not
        // yet begun to appear.
        assert!(!mid.alphas.is_empty());
        assert!(mid.alphas.len() <= "Copy".len() + "Copied".len());
    }

    /// The cascade's cells run left to right: an earlier letter is further
    /// along than a later one while the run is in flight.
    #[test]
    fn the_cascade_runs_left_to_right() {
        let first = view(ActionSwapTransition::Cascade);
        let mut w = build(&first);
        let size = lay_out(&mut w);
        let second = view(ActionSwapTransition::Cascade).value("copied");
        rebuild(&first, &second, &mut w);
        paint_at(&mut w, size, &theme(), 0);
        let mid = paint_at(&mut w, size, &theme(), 60).0;
        // The arriving cells are the trailing run of layers; each one is at
        // least as opaque as the one after it.
        let arriving: Vec<f32> = mid.layers.iter().rev().take(3).map(|l| l.2).collect();
        assert!(
            arriving.windows(2).all(|w| w[0] <= w[1] + 1e-6),
            "later letters trail earlier ones: {arriving:?}"
        );
    }

    /// Reduced motion drops the swap entirely: one label, no frames owed.
    #[test]
    fn reduced_motion_swaps_without_animating() {
        for transition in [
            ActionSwapTransition::Fade,
            ActionSwapTransition::Blur,
            ActionSwapTransition::Cascade,
            ActionSwapTransition::Roll,
        ] {
            let first = view(transition);
            let mut w = build(&first);
            let size = lay_out(&mut w);
            let second = view(transition).value("copied");
            rebuild(&first, &second, &mut w);
            let (rec, more) = paint_at(&mut w, size, &reduced_theme(), 0);
            assert!(!more, "{transition:?} owes no frames");
            assert!(w.previous.is_none(), "{transition:?} retired the swap");
            assert!(rec.alphas.iter().all(|a| (*a - 1.0).abs() < 1e-6));
        }
    }

    /// An empty item list is inert rather than a panic: nothing to show, nothing
    /// to ask for.
    #[test]
    fn an_empty_item_list_paints_and_reports_nothing() {
        let v = action_swap::<Log>(Vec::new());
        let mut w = build(&v);
        let size = lay_out(&mut w);
        let mut state = Log::default();
        assert!(w.next_id().is_none());
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Down, 2.0, 2.0));
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Up, 2.0, 2.0));
        assert!(state.asked.is_empty());
        let (rec, _) = paint_at(&mut w, size, &theme(), 0);
        assert!(rec.inks.is_empty());
    }

    /// The tone ladder is the button's, reused rather than re-derived.
    #[test]
    fn the_default_tone_is_the_source_s_secondary() {
        let v = action_swap::<Log>(items());
        let mut w = build(&v);
        let size = lay_out(&mut w);
        let (rec, _) = paint_at(&mut w, size, &theme(), 0);
        assert_eq!(rec.rrects[0].3, theme().scheme().surface_container);
        assert!(!rec.strokes.is_empty(), "secondary is bordered");
        assert_eq!(w.tone, ButtonTone::Secondary);
    }
}
