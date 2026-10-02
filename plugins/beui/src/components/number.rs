//! Ports beUI's `number` component family (several upstream variants).
//!
//! **Source:** beUI rev `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved
//! 2026-09-01. The registry entry `number` names
//! `components/motion/animated-number.tsx` plus one `extraFile`,
//! `components/motion/number-ticker.tsx`, and lists both as examples. They are
//! the same component here, split by [`NumberMode`]:
//!
//! | upstream example | file | mode |
//! |---|---|---|
//! | `ticker` | `number-ticker.tsx` | [`NumberMode::Roll`] |
//! | `animated` | `animated-number.tsx` | [`NumberMode::CountUp`] |
//!
//! # One slot template, two motions
//!
//! Both upstream components render **tabular figures** — a fixed grid of digit
//! cells that does not reflow as the value changes — so this port shapes the ten
//! digits (and each literal the format inserts: a group separator, a sign, a
//! prefix, a suffix) **once**, and every frame afterwards is a choice of which
//! pre-shaped glyph to draw where. Nothing re-shapes while a number animates,
//! which is what lets both motions run entirely in `paint`.
//!
//! - [`Roll`](NumberMode::Roll) is upstream's slot machine: each digit cell holds
//!   a column of `0..9` clipped to one cell height, and the column slides so the
//!   wanted digit lands in the window. The slide is
//!   [`SPRING_SWAP`](crate::tokens::motion::SPRING_SWAP) — a content swap, which
//!   is exactly what a digit changing is.
//! - [`CountUp`](NumberMode::CountUp) is upstream's `animate(from, to)`: the
//!   *value* eases along [`EASE_OUT`](crate::tokens::motion::EASE_OUT) and the
//!   digits are drawn snapped, right-aligned into the template, with the slots
//!   the running value has not reached yet left blank.
//!
//! # Retargeting never jumps
//!
//! A value change mid-flight re-aims each column **from wherever it currently
//! is** rather than from the digit it was last asked for, so a number updated
//! twice in quick succession slides once, continuously. Same for the count-up's
//! value ramp. This is the `AnimatedOpacity`/`AnimatedScale` retarget semantic,
//! applied to a digit column.
//!
//! # Degradations against upstream
//!
//! - **No blur.** `NumberTicker`'s optional `blur` prop animates
//!   `filter: blur(10px) → blur(0px)` across a roll; frust's scene has no blur
//!   primitive, so the prop is not ported rather than being silently ignored.
//! - **Grouping is ASCII, not locale-aware.** Upstream's `locale` prop calls
//!   `Number.toLocaleString()`; there is no locale-aware number formatter behind
//!   the frust facade, so [`NumberView::group`] inserts an ASCII comma every
//!   three digits — the en-US result, stated as a choice rather than presented
//!   as localization.
//! - **No `startOnView` gate.** Both upstream components arm on an
//!   `IntersectionObserver`; the catalog's viewport substrate
//!   ([`crate::motion::ScrollFx`]) is a scroll-progress feed rather than a
//!   visibility oracle, so the animation is armed from the widget's first paint.
//!   A caller that wants viewport arming rebuilds with the value when it wants
//!   it.

use std::time::Duration;

use frust::authoring::text::{TextContext, TextLayout, TextStyle};
use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, Role, SemanticsCtx,
    View, Widget,
};
use frust::{FrameTime, Theme};
use kurbo::{Point, Size};
use peniko::{Brush, Color};

use crate::motion::Ramp;
use crate::style::TEXT_BASE;
use crate::text::{ThemeTextType, themed_style};
use crate::tokens::BEUI_LIGHT;
use crate::tokens::motion::{EASE_OUT, SPRING_SWAP};

/// How long a count-up takes — `duration = 1.2` seconds
/// (`animated-number.tsx`).
const COUNT_UP_DURATION: Duration = Duration::from_millis(1200);

/// The gap between consecutive digit slots on the entrance roll —
/// `stagger = 0.04` (`number-ticker.tsx`).
const ROLL_STAGGER: Duration = Duration::from_millis(40);

/// How many digits a column holds.
const DIGITS: usize = 10;

/// The digit cell's height as a multiple of the tallest shaped digit —
/// `DIGIT_HEIGHT_EM = 1.1` (`number-ticker.tsx`), which is what gives the slot
/// window a little air above and below the glyph.
const CELL_HEIGHT_FACTOR: f64 = 1.1;

/// How many digits a group separator falls between — `toLocaleString`'s
/// thousands grouping.
const GROUP_SIZE: usize = 3;

/// The character [`NumberView::group`] inserts. See the [module docs](self) on
/// why this is ASCII rather than locale-resolved.
const GROUP_SEPARATOR: char = ',';

/// Unthemed fallback ink (beUI light `--foreground`).
const FALLBACK_INK: Color = BEUI_LIGHT.foreground;

/// Which of the two upstream number motions a [`NumberView`] plays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NumberMode {
    /// `number-ticker`: per-digit vertical roll — the old digit leaves its slot
    /// as the new one arrives.
    #[default]
    Roll,
    /// `animated-number`: the value itself counts up, digits drawn snapped.
    CountUp,
}

impl NumberMode {
    /// Both modes, in the order upstream's registry entry lists its examples.
    pub const ALL: [NumberMode; 2] = [NumberMode::Roll, NumberMode::CountUp];
}

/// One position of the rendered number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NumberSlot {
    /// A digit cell: a rolling column, or a snapped glyph.
    Digit,
    /// A fixed character the format inserted — a separator, a sign, a prefix or
    /// suffix character.
    Literal(char),
}

/// A declarative beUI animated number. See the [module docs](self).
///
/// # Example
///
/// ```
/// use frust_beui::components::number::{NumberMode, number};
///
/// let downloads = number::<()>(128_400.0).group(true).mode(NumberMode::CountUp);
/// ```
pub struct NumberView<State: 'static> {
    value: f64,
    mode: NumberMode,
    pad: usize,
    group: bool,
    prefix: String,
    suffix: String,
    size: f32,
    color: Option<Color>,
    duration: Option<Duration>,
    stagger: Duration,
    _state: std::marker::PhantomData<fn(&mut State)>,
}

/// Create an animated number showing `value`, rolling its digits
/// ([`NumberMode::Roll`]).
pub fn number<State: 'static>(value: f64) -> NumberView<State> {
    NumberView {
        value,
        mode: NumberMode::default(),
        pad: 0,
        group: false,
        prefix: String::new(),
        suffix: String::new(),
        size: TEXT_BASE as f32,
        color: None,
        duration: None,
        stagger: ROLL_STAGGER,
        _state: std::marker::PhantomData,
    }
}

impl<State: 'static> NumberView<State> {
    /// Play `mode` instead of [`NumberMode::Roll`].
    pub fn mode(mut self, mode: NumberMode) -> Self {
        self.mode = mode;
        self
    }

    /// Left-pad the digits to at least `digits` wide with zeros — upstream's
    /// `pad` prop.
    pub fn pad(mut self, digits: usize) -> Self {
        self.pad = digits;
        self
    }

    /// Insert a group separator every three digits — upstream's `locale` prop,
    /// with the caveat in the [module docs](self).
    pub fn group(mut self, group: bool) -> Self {
        self.group = group;
        self
    }

    /// Text placed before the digits (a currency mark).
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// Text placed after the digits (a unit).
    pub fn suffix(mut self, suffix: impl Into<String>) -> Self {
        self.suffix = suffix.into();
        self
    }

    /// Set the type size, in logical px (default [`TEXT_BASE`]).
    pub fn size(mut self, size: f64) -> Self {
        self.size = size.max(0.0) as f32;
        self
    }

    /// Paint in `color` instead of the theme's resolved ink.
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    /// Override the motion's own duration: the count-up's ramp, or the roll's
    /// per-digit slide (which is otherwise a spring and has no authored
    /// duration).
    pub fn duration(mut self, duration: Duration) -> Self {
        self.duration = Some(duration);
        self
    }

    /// Override the gap between consecutive digits on the entrance roll
    /// (default [`ROLL_STAGGER`]).
    pub fn stagger(mut self, stagger: Duration) -> Self {
        self.stagger = stagger;
        self
    }

    /// The formatted text this view will render.
    pub fn text(&self) -> String {
        format_value(self.value, self.pad, self.group)
    }
}

/// One digit column's animation state.
///
/// Timed against the widget's single run clock rather than one of its own: every
/// column is re-aimed together, so a second clock could only ever disagree.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Column {
    /// Where the column sat when it was last re-aimed, in digit units.
    from: f64,
    /// Where it is heading, in digit units.
    to: f64,
    /// How long after the run's start this column begins moving — the entrance
    /// stagger, zero once the entrance has played.
    delay: Duration,
}

impl Column {
    /// Where the column sits at `elapsed` into its slide, in digit units.
    fn position(&self, ramp: Ramp, elapsed: Duration) -> f64 {
        let progress = match elapsed.checked_sub(self.delay) {
            Some(after) => ramp.progress(after),
            None => 0.0,
        };
        self.from + (self.to - self.from) * progress
    }

    /// Whether this column has finished its slide by `elapsed`.
    fn is_settled(&self, ramp: Ramp, elapsed: Duration) -> bool {
        self.from == self.to || elapsed >= self.delay + ramp.settle()
    }
}

/// The retained widget for a [`NumberView`].
pub struct NumberWidget {
    value: f64,
    mode: NumberMode,
    pad: usize,
    group: bool,
    prefix: String,
    suffix: String,
    size: f32,
    color: Option<Color>,
    duration: Option<Duration>,
    stagger: Duration,
    /// The fixed slot grid, derived from the formatted target.
    slots: Vec<NumberSlot>,
    /// One animation state per digit slot, in slot order.
    columns: Vec<Column>,
    /// The count-up's own ramp endpoints, in the same retarget-from-here shape
    /// a column uses.
    count_from: f64,
    count_to: f64,
    /// The single clock every column and the count-up ramp are timed against,
    /// latched on the first paint after a re-aim.
    started: Option<FrameTime>,
    /// Set by `rebuild` when a new value arrived: the re-aim needs a clock, and
    /// only `paint` has one.
    pending: bool,
    /// The ten shaped digits, and one shaped run per literal slot.
    digits: Vec<TextLayout>,
    literals: Vec<Option<TextLayout>>,
    shaped: Option<(Vec<NumberSlot>, TextStyle)>,
    /// The digit cell's width and the column's slide pitch, both measured from
    /// the shaped digits.
    cell: Size,
}

impl NumberWidget {
    /// The value this widget is showing (or heading toward).
    pub fn value(&self) -> f64 {
        self.value
    }

    /// The formatted target, without prefix or suffix.
    pub fn text(&self) -> String {
        format_value(self.value, self.pad, self.group)
    }

    /// The whole announced string — what a screen reader is handed.
    pub fn readable(&self) -> String {
        format!("{}{}{}", self.prefix, self.text(), self.suffix)
    }

    /// How many digit cells the grid holds.
    pub fn digit_count(&self) -> usize {
        self.columns.len()
    }

    /// Each digit column's position at `elapsed`, in digit units — the roll's
    /// whole observable state.
    pub fn positions(&self, elapsed: Duration) -> Vec<f64> {
        let ramp = self.ramp();
        self.columns
            .iter()
            .map(|column| column.position(ramp, elapsed))
            .collect()
    }

    /// The ramp a digit column slides on: the caller's duration override on
    /// [`EASE_OUT`], else [`SPRING_SWAP`].
    fn ramp(&self) -> Ramp {
        match self.duration {
            Some(duration) => Ramp::eased(duration, EASE_OUT),
            None => Ramp::spring(SPRING_SWAP),
        }
    }

    /// Rebuild the slot grid from the current value and format, re-aiming every
    /// digit column that survives and resting any that is new.
    ///
    /// `elapsed` is how far the *outgoing* run had got, so a surviving column is
    /// re-aimed from where it actually is; `None` is a first stage with nothing
    /// to preserve. `staggered` asks for the entrance's per-slot delay (only the
    /// first run wants it — a live value change rolls every digit at once, which
    /// is upstream's `entered` rule).
    fn restage(&mut self, elapsed: Option<Duration>, staggered: bool) {
        let text = format_value(self.value, self.pad, self.group);
        let slots = slot_template(&self.prefix, &text, &self.suffix);
        let digits: Vec<f64> = text
            .chars()
            .filter_map(|c| c.to_digit(10))
            .map(f64::from)
            .collect();

        let ramp = self.ramp();
        let previous = std::mem::take(&mut self.columns);
        // Keyed by place value — position from the *right* — so a number growing
        // a digit keeps the ones, tens and hundreds already on screen rolling
        // from where they are (`number-ticker.tsx`'s own keying rule).
        let mut columns: Vec<Column> = Vec::with_capacity(digits.len());
        for (index, digit) in digits.iter().enumerate() {
            let from_right = digits.len() - 1 - index;
            let existing = previous
                .len()
                .checked_sub(from_right + 1)
                .and_then(|i| previous.get(i));
            let column = match (existing, elapsed) {
                (Some(existing), Some(elapsed)) => Column {
                    from: existing.position(ramp, elapsed),
                    to: *digit,
                    delay: Duration::ZERO,
                },
                // A brand-new column enters from zero, upstream's `armed ? digit
                // : 0` starting point.
                _ => Column {
                    from: 0.0,
                    to: *digit,
                    delay: if staggered {
                        self.stagger * index as u32
                    } else {
                        Duration::ZERO
                    },
                },
            };
            columns.push(column);
        }

        if self.slots != slots {
            // A different grid needs different literal runs.
            self.shaped = None;
        }
        self.slots = slots;
        self.columns = columns;
    }

    /// The style the digits and literals are shaped with, in the theme's
    /// `body_medium` family. `layout` keys the shaped digits on it, so a theme
    /// swap that changes the family reshapes them.
    fn run_style(&self, theme: Option<&Theme>, ink: Color) -> TextStyle {
        themed_style(
            TextStyle::new(self.size, ink),
            ThemeTextType::BodyMedium,
            theme,
        )
    }

    /// The resolved ink: the explicit override, else the theme's `on_surface`,
    /// else [`FALLBACK_INK`].
    fn ink(&self, theme: Option<&Theme>) -> Color {
        self.color
            .unwrap_or_else(|| theme.map_or(FALLBACK_INK, |t| t.scheme().on_surface))
    }

    /// The count-up's displayed value at `elapsed`.
    fn counted(&self, elapsed: Duration) -> f64 {
        let duration = self.duration.unwrap_or(COUNT_UP_DURATION);
        if duration.is_zero() {
            return self.count_to;
        }
        let progress = EASE_OUT.transform(elapsed.as_secs_f64() / duration.as_secs_f64());
        self.count_from + (self.count_to - self.count_from) * progress
    }
}

impl<State: 'static> View<State> for NumberView<State> {
    type Element = NumberWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> NumberWidget {
        let mut widget = NumberWidget {
            value: self.value,
            mode: self.mode,
            pad: self.pad,
            group: self.group,
            prefix: self.prefix.clone(),
            suffix: self.suffix.clone(),
            size: self.size,
            color: self.color,
            duration: self.duration,
            stagger: self.stagger,
            slots: Vec::new(),
            columns: Vec::new(),
            count_from: 0.0,
            count_to: self.value,
            started: None,
            pending: false,
            digits: Vec::new(),
            literals: Vec::new(),
            shaped: None,
            cell: Size::ZERO,
        };
        widget.restage(None, true);
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut NumberWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        let format_changed = prev.pad != self.pad
            || prev.group != self.group
            || prev.prefix != self.prefix
            || prev.suffix != self.suffix;
        if format_changed {
            element.pad = self.pad;
            element.group = self.group;
            element.prefix = self.prefix.clone();
            element.suffix = self.suffix.clone();
        }
        if prev.mode != self.mode {
            element.mode = self.mode;
            element.pending = true;
            flags |= ChangeFlags::PAINT;
        }
        if prev.size != self.size {
            element.size = self.size;
            element.shaped = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.color != self.color {
            element.color = self.color;
            flags |= ChangeFlags::PAINT;
        }
        if prev.duration != self.duration {
            element.duration = self.duration;
            flags |= ChangeFlags::PAINT;
        }
        if prev.stagger != self.stagger {
            element.stagger = self.stagger;
            flags |= ChangeFlags::PAINT;
        }
        // A value change is the whole point: re-aim from wherever the columns
        // currently are. The clock is not available here, so the re-aim is
        // deferred to the next paint, which has one.
        if prev.value != self.value || format_changed {
            element.value = self.value;
            element.pending = true;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }
}

impl Widget for NumberWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let style = self.run_style(theme, self.ink(theme));
        if self.shaped.as_ref() != Some(&(self.slots.clone(), style.clone())) {
            let text_ctx = ctx.text_context::<TextContext>();
            self.digits = (0..DIGITS)
                .map(|digit| text_ctx.layout(&digit.to_string(), &style, None))
                .collect();
            self.literals = self
                .slots
                .iter()
                .map(|slot| match slot {
                    NumberSlot::Digit => None,
                    NumberSlot::Literal(c) => Some(text_ctx.layout(&c.to_string(), &style, None)),
                })
                .collect();
            self.shaped = Some((self.slots.clone(), style));
            let widest = self
                .digits
                .iter()
                .map(|run| run.size().width)
                .fold(0.0_f64, f64::max);
            let tallest = self
                .digits
                .iter()
                .map(|run| run.size().height)
                .fold(0.0_f64, f64::max);
            self.cell = Size::new(widest, tallest * CELL_HEIGHT_FACTOR);
        }

        let width: f64 = self
            .slots
            .iter()
            .zip(self.literals.iter())
            .map(|(slot, literal)| match slot {
                NumberSlot::Digit => self.cell.width,
                NumberSlot::Literal(_) => literal.as_ref().map_or(0.0, |run| run.size().width),
            })
            .sum();
        bc.constrain(Size::new(width, self.cell.height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let ink = self.ink(theme);
        let now = ctx.frame_time();

        // A pending re-aim (a rebuild saw a new value but had no clock) resolves
        // here, against the frame's own timestamp.
        if self.pending {
            self.pending = false;
            let outgoing = self.started.map(|started| now.saturating_sub(started));
            let held = match (outgoing, self.mode) {
                (Some(elapsed), NumberMode::CountUp) => self.counted(elapsed),
                _ => self.count_to,
            };
            self.restage(outgoing, false);
            self.count_from = held;
            self.count_to = self.value;
            self.started = Some(now);
        }
        let started = *self.started.get_or_insert(now);
        let elapsed = now.saturating_sub(started);

        let origin = ctx.origin();
        let ramp = self.ramp();
        let brush = Brush::Solid(ink);

        // Which digit each slot shows when it is not rolling: the count-up's
        // running value, right-aligned into the grid.
        let snapped = if self.mode == NumberMode::CountUp && !reduce {
            Some(format_value(self.counted(elapsed), self.pad, self.group))
        } else {
            None
        };
        let visible: Vec<Option<f64>> = match &snapped {
            Some(text) => align_right(&self.slots, text),
            None => self.columns.iter().map(|c| Some(c.to)).collect(),
        };

        let mut x = origin.x;
        let mut column_index = 0usize;
        for (index, slot) in self.slots.iter().enumerate() {
            match slot {
                NumberSlot::Literal(_) => {
                    if let Some(run) = self.literals.get(index).and_then(Option::as_ref) {
                        // A separator whose left neighbour has not been reached
                        // yet is not drawn — otherwise a count-up shows a
                        // stranded comma floating ahead of its digits. A prefix
                        // character (nothing to its left at all) always draws.
                        let carries = column_index == 0
                            || visible.get(column_index - 1).copied().flatten().is_some();
                        if carries {
                            let y = origin.y + (self.cell.height - run.size().height) / 2.0;
                            crate::text::paint_glyph_run(
                                Some(run),
                                Point::new(x, y),
                                &brush,
                                scene,
                            );
                        }
                        x += run.size().width;
                    }
                }
                NumberSlot::Digit => {
                    let position = match (reduce, self.mode) {
                        // Reduced motion snaps to the target and animates
                        // nothing — upstream's own `duration: 0` branch.
                        (true, _) => self.columns.get(column_index).map(|c| c.to),
                        (false, NumberMode::Roll) => self
                            .columns
                            .get(column_index)
                            .map(|c| c.position(ramp, elapsed)),
                        (false, NumberMode::CountUp) => {
                            visible.get(column_index).copied().flatten()
                        }
                    };
                    if let Some(position) = position {
                        draw_column(
                            &self.digits,
                            Point::new(x, origin.y),
                            self.cell,
                            position,
                            &brush,
                            scene,
                        );
                    }
                    x += self.cell.width;
                    column_index += 1;
                }
            }
        }

        if reduce {
            return;
        }
        let running = match self.mode {
            NumberMode::Roll => !self
                .columns
                .iter()
                .all(|column| column.is_settled(ramp, elapsed)),
            NumberMode::CountUp => elapsed < self.duration.unwrap_or(COUNT_UP_DURATION),
        };
        if running {
            // A run with a visible endpoint, never a decorative loop.
            ctx.request_frame();
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // The resolved value, never the mid-roll sample — upstream publishes the
        // same thing through an `sr-only` span.
        ctx.push_node(Role::Label, |node| {
            node.set_value(self.readable().as_str());
        });
    }
}

/// Format `value` the way both upstream components do: rounded to an integer,
/// left-padded to `pad` digits, optionally grouped.
fn format_value(value: f64, pad: usize, group: bool) -> String {
    let rounded = if value.is_finite() {
        value.round()
    } else {
        0.0
    };
    let negative = rounded < 0.0;
    let mut digits = format!("{:.0}", rounded.abs());
    if digits.len() < pad {
        digits = format!("{}{}", "0".repeat(pad - digits.len()), digits);
    }
    if group {
        digits = group_digits(&digits);
    }
    if negative {
        format!("-{digits}")
    } else {
        digits
    }
}

/// Insert [`GROUP_SEPARATOR`] every [`GROUP_SIZE`] digits, counting from the
/// right.
fn group_digits(digits: &str) -> String {
    let count = digits.len();
    let mut out = String::with_capacity(count + count / GROUP_SIZE);
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (count - index).is_multiple_of(GROUP_SIZE) {
            out.push(GROUP_SEPARATOR);
        }
        out.push(character);
    }
    out
}

/// The fixed slot grid for a formatted number with its prefix and suffix.
fn slot_template(prefix: &str, text: &str, suffix: &str) -> Vec<NumberSlot> {
    prefix
        .chars()
        .map(NumberSlot::Literal)
        .chain(text.chars().map(|c| {
            if c.is_ascii_digit() {
                NumberSlot::Digit
            } else {
                NumberSlot::Literal(c)
            }
        }))
        .chain(suffix.chars().map(NumberSlot::Literal))
        .collect()
}

/// Right-align `text`'s digits into `slots`, reporting one entry per digit slot
/// in slot order: the digit that slot shows, or `None` when the running value is
/// not wide enough to reach it.
///
/// Walked from the right so a narrower value lands under the wider grid's low
/// places — the same place-value alignment the roll's own keying uses.
fn align_right(slots: &[NumberSlot], text: &str) -> Vec<Option<f64>> {
    let digits: Vec<f64> = text
        .chars()
        .filter_map(|c| c.to_digit(10))
        .map(f64::from)
        .collect();
    let cells = slots
        .iter()
        .filter(|slot| matches!(slot, NumberSlot::Digit))
        .count();
    (0..cells)
        .map(|index| {
            let from_right = cells - 1 - index;
            digits
                .len()
                .checked_sub(from_right + 1)
                .and_then(|i| digits.get(i).copied())
        })
        .collect()
}

/// Draw one digit column: the ten pre-shaped digits stacked a cell apart and
/// slid so `position` sits in the window, clipped to the cell.
fn draw_column(
    digits: &[TextLayout],
    origin: Point,
    cell: Size,
    position: f64,
    brush: &Brush,
    scene: &mut dyn PaintScene,
) {
    if digits.is_empty() || cell.height <= 0.0 {
        return;
    }
    scene.push_clip(origin, cell);
    for (digit, run) in digits.iter().enumerate() {
        let offset = (digit as f64 - position) * cell.height;
        // Only the one or two digits actually inside the window are worth
        // emitting; the rest are clipped away entirely.
        if offset <= -cell.height || offset >= cell.height {
            continue;
        }
        let glyph = run.size();
        let at = Point::new(
            origin.x + (cell.width - glyph.width) / 2.0,
            origin.y + offset + (cell.height - glyph.height) / 2.0,
        );
        crate::text::paint_glyph_run(Some(run), at, brush, scene);
    }
    scene.pop_clip();
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::scene::GlyphRun;
    use frust_core::{BuildCtx, PaintCtx};
    use std::any::Any;

    /// The box every paint test lays its number into.
    const BOX: Size = Size::new(240.0, 40.0);

    /// Records how many glyph runs were drawn, and every clip pushed — the roll
    /// window is a clip, so its presence is the observable part of the slot.
    #[derive(Default)]
    struct Recorder {
        runs: usize,
        clips: Vec<(Point, Size)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn draw_glyph_run(&mut self, _run: GlyphRun) {
            self.runs += 1;
        }
        fn push_clip(&mut self, origin: Point, size: Size) {
            self.clips.push((origin, size));
        }
    }

    fn laid_out(view: &NumberView<()>) -> NumberWidget {
        let mut next_id = 0u64;
        let mut widget = View::<()>::build(view, &mut BuildCtx::new(&mut next_id));
        relayout(&mut widget);
        widget
    }

    fn relayout(widget: &mut NumberWidget) {
        let mut text_ctx = TextContext::new();
        let mut layout = LayoutCtx::with_text_context(&mut text_ctx as &mut dyn Any);
        widget.layout(&mut layout, &BoxConstraints::loose(BOX));
    }

    fn painted(widget: &mut NumberWidget, ms: u64, theme: Option<&Theme>) -> (Recorder, bool) {
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, BOX, FrameTime::from_nanos(ms * 1_000_000));
        if let Some(theme) = theme {
            ctx = ctx.with_theme(theme);
        }
        let mut recorder = Recorder::default();
        widget.paint(&mut ctx, &mut recorder);
        (recorder, ctx.needs_frame())
    }

    fn rebuilt(prev: &NumberView<()>, next: &NumberView<()>, widget: &mut NumberWidget) {
        let mut next_id = 0u64;
        View::<()>::rebuild(next, prev, widget, &mut BuildCtx::new(&mut next_id));
        relayout(widget);
    }

    fn reduced() -> Theme {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        theme
    }

    /// Both upstream examples are constructible through the one component.
    #[test]
    fn both_registry_modes_are_constructible() {
        assert_eq!(NumberMode::ALL.len(), 2);
        for mode in NumberMode::ALL {
            let widget = laid_out(&number::<()>(1234.0).mode(mode));
            assert_eq!(widget.value(), 1234.0);
            assert_eq!(widget.digit_count(), 4);
        }
    }

    /// The formatter is upstream's: rounded, optionally zero-padded, optionally
    /// grouped, with the sign kept outside the padding.
    #[test]
    fn the_formatter_rounds_pads_and_groups() {
        assert_eq!(format_value(7.0, 0, false), "7");
        assert_eq!(format_value(7.6, 0, false), "8", "rounded, not truncated");
        assert_eq!(format_value(7.0, 4, false), "0007", "pad");
        assert_eq!(format_value(1234.0, 0, true), "1,234");
        assert_eq!(format_value(1_234_567.0, 0, true), "1,234,567");
        assert_eq!(format_value(100.0, 0, true), "100", "no leading separator");
        assert_eq!(format_value(-42.0, 0, false), "-42");
        assert_eq!(format_value(-1234.0, 0, true), "-1,234");
        // A non-finite value has no digits to show rather than a `NaN` string.
        assert_eq!(format_value(f64::NAN, 0, false), "0");
    }

    /// The slot grid separates rolling digit cells from the fixed characters
    /// around them.
    #[test]
    fn the_slot_grid_splits_digits_from_literals() {
        let slots = slot_template("$", "1,234", "/s");
        assert_eq!(
            slots,
            vec![
                NumberSlot::Literal('$'),
                NumberSlot::Digit,
                NumberSlot::Literal(','),
                NumberSlot::Digit,
                NumberSlot::Digit,
                NumberSlot::Digit,
                NumberSlot::Literal('/'),
                NumberSlot::Literal('s'),
            ]
        );
    }

    /// A narrower running value lands on the grid's low places, leaving the
    /// high ones blank — the alignment that keeps a count-up from jittering.
    #[test]
    fn a_running_value_right_aligns_into_the_grid() {
        let slots = slot_template("", "1234", "");
        assert_eq!(
            align_right(&slots, "12"),
            vec![None, None, Some(1.0), Some(2.0)]
        );
        assert_eq!(
            align_right(&slots, "1234"),
            vec![Some(1.0), Some(2.0), Some(3.0), Some(4.0)]
        );
        assert_eq!(align_right(&slots, "0"), vec![None, None, None, Some(0.0)]);
    }

    /// Every digit column enters from zero and lands exactly on its digit —
    /// the slot-machine reveal.
    #[test]
    fn the_roll_enters_from_zero_and_lands_on_the_value() {
        let mut widget = laid_out(&number::<()>(507.0));
        painted(&mut widget, 0, None);
        assert_eq!(widget.positions(Duration::ZERO), vec![0.0, 0.0, 0.0]);

        let settled = widget.positions(Duration::from_secs(5));
        assert_eq!(settled, vec![5.0, 0.0, 7.0]);
    }

    /// The entrance is staggered left to right: at one instant an earlier digit
    /// is further along than a later one, and the last has not moved.
    #[test]
    fn the_entrance_staggers_across_the_digits() {
        let mut widget = laid_out(&number::<()>(999.0).stagger(Duration::from_millis(120)));
        painted(&mut widget, 0, None);
        let at = widget.positions(Duration::from_millis(130));
        assert!(at[0] > at[1], "the leading digit leads: {at:?}");
        assert_eq!(at[2], 0.0, "the last digit's slot has not opened");
    }

    /// A value change mid-roll re-aims from where the column *is*, so nothing
    /// jumps — the retarget semantic the acceptance criteria name.
    #[test]
    fn retargeting_mid_roll_never_jumps() {
        let first = number::<()>(5.0).stagger(Duration::ZERO);
        let mut widget = laid_out(&first);
        painted(&mut widget, 0, None);
        let mid = Duration::from_millis(40);
        let before = widget.positions(mid)[0];
        assert!(
            before > 0.0 && before < 5.0,
            "the column is genuinely mid-flight: {before}"
        );

        let second = number::<()>(9.0).stagger(Duration::ZERO);
        rebuilt(&first, &second, &mut widget);
        // Painting at the same instant the sample was taken re-aims the column
        // from exactly there.
        painted(&mut widget, 40, None);
        let after = widget.positions(Duration::ZERO)[0];
        assert!(
            (after - before).abs() < 1e-9,
            "the column jumped from {before} to {after}"
        );
        // ...and it now heads for the new digit rather than the old one.
        assert_eq!(widget.positions(Duration::from_secs(5))[0], 9.0);
    }

    /// A number that grows a digit keeps the places already on screen — the
    /// ones column does not restart because a thousands column appeared.
    #[test]
    fn a_growing_number_keeps_its_low_places() {
        let first = number::<()>(9.0).stagger(Duration::ZERO);
        let mut widget = laid_out(&first);
        painted(&mut widget, 0, None);
        painted(&mut widget, 5_000, None);
        assert_eq!(widget.positions(Duration::from_secs(5)), vec![9.0]);

        let second = number::<()>(19.0).stagger(Duration::ZERO);
        rebuilt(&first, &second, &mut widget);
        // The re-aim resolves against this paint's clock, which becomes the new
        // run's zero.
        painted(&mut widget, 5_000, None);
        let start = widget.positions(Duration::ZERO);
        assert_eq!(start.len(), 2);
        assert_eq!(start[0], 0.0, "the new tens column enters from zero");
        assert_eq!(start[1], 9.0, "the ones column stayed where it was");
        assert_eq!(
            widget.positions(Duration::from_secs(5)),
            vec![1.0, 9.0],
            "and both land on the new value"
        );
    }

    /// The roll asks for frames while any column is moving and stops the moment
    /// they have all landed.
    #[test]
    fn the_roll_requests_frames_only_while_it_moves() {
        let mut widget = laid_out(&number::<()>(1234.0));
        let (recorder, needs_frame) = painted(&mut widget, 0, None);
        assert!(needs_frame);
        assert_eq!(recorder.clips.len(), 4, "one roll window per digit");
        assert!(recorder.runs > 0, "digits painted");

        let (_, needs_frame) = painted(&mut widget, 10_000, None);
        assert!(!needs_frame);
    }

    /// The count-up runs for its own duration and then stops.
    #[test]
    fn the_count_up_runs_for_its_duration() {
        let mut widget = laid_out(
            &number::<()>(100.0)
                .mode(NumberMode::CountUp)
                .duration(Duration::from_millis(500)),
        );
        let (_, needs_frame) = painted(&mut widget, 0, None);
        assert!(needs_frame);
        let (_, needs_frame) = painted(&mut widget, 400, None);
        assert!(needs_frame);
        let (_, needs_frame) = painted(&mut widget, 900, None);
        assert!(!needs_frame);
    }

    /// The counted value eases from its previous reading to the target and
    /// never overshoots it.
    #[test]
    fn the_counted_value_eases_to_the_target() {
        let mut widget = laid_out(
            &number::<()>(100.0)
                .mode(NumberMode::CountUp)
                .duration(Duration::from_millis(500)),
        );
        painted(&mut widget, 0, None);
        assert_eq!(widget.counted(Duration::ZERO), 0.0);
        let mid = widget.counted(Duration::from_millis(250));
        assert!(mid > 0.0 && mid < 100.0, "mid-count reading: {mid}");
        assert_eq!(widget.counted(Duration::from_millis(500)), 100.0);
        assert_eq!(widget.counted(Duration::from_secs(5)), 100.0);
    }

    /// `reduce_motion` shows the settled number and asks for nothing — the same
    /// collapse both upstream components make.
    #[test]
    fn reduce_motion_shows_the_settled_number() {
        let theme = reduced();
        for mode in NumberMode::ALL {
            let mut widget = laid_out(&number::<()>(407.0).mode(mode));
            let (recorder, needs_frame) = painted(&mut widget, 0, Some(&theme));
            assert!(!needs_frame, "{mode:?} still asked for a frame");
            assert!(recorder.runs > 0, "{mode:?} painted nothing");
            assert_eq!(widget.positions(Duration::ZERO).len(), 3);
        }
    }

    /// The announced string is the resolved value with its prefix and suffix,
    /// never the digits mid-roll.
    #[test]
    fn the_announced_string_is_the_resolved_value() {
        let widget = laid_out(
            &number::<()>(1234.0)
                .group(true)
                .prefix("$")
                .suffix(" saved"),
        );
        assert_eq!(widget.text(), "1,234");
        assert_eq!(widget.readable(), "$1,234 saved");
        // The separator is a literal, so it is not one of the rolling cells.
        assert_eq!(widget.digit_count(), 4);
    }

    /// A padded number rolls every reserved cell, leading zeros included.
    #[test]
    fn padding_reserves_real_rolling_cells() {
        let mut widget = laid_out(&number::<()>(7.0).pad(4));
        painted(&mut widget, 0, None);
        assert_eq!(widget.digit_count(), 4);
        assert_eq!(
            widget.positions(Duration::from_secs(5)),
            vec![0.0, 0.0, 0.0, 7.0]
        );
    }

    // ---- Typeface: digits and literals follow the live theme ---------------

    use crate::text::typeface_probe::{
        assert_follows_a_live_family_swap, assert_paints_only_in_geist,
    };

    /// A grouped, prefixed roll, so both the digit columns and the literal
    /// runs paint.
    fn probe_view(_: &mut ()) -> NumberView<()> {
        number::<()>(1234.0).group(true).prefix("$")
    }

    #[test]
    fn digits_and_literals_paint_in_geist_under_the_beui_theme() {
        assert_paints_only_in_geist("the number's digits", probe_view, BOX);
    }

    #[test]
    fn digits_and_literals_follow_a_live_theme_family_swap() {
        assert_follows_a_live_family_swap("the number's digits", probe_view, BOX);
    }
}
