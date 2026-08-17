//! Ports shadcn/ui's **Slider** (including its multi-thumb form) from
//! `tmp/ui/apps/v4/registry/new-york-v4/ui/slider.tsx` (rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved 2026-08-17).
//!
//! | class | here |
//! |---|---|
//! | track `h-1.5 rounded-full bg-muted` | [`SLIDER_TRACK_THICKNESS`] over `surface_container_highest` |
//! | range `bg-primary` | the filled span, `primary` |
//! | thumb `size-4 rounded-full border border-primary bg-white shadow-sm` | [`SLIDER_THUMB_SIZE`], a `primary` ring around **literal white** |
//! | thumb `hover:ring-4` / `focus-visible:ring-4` + `ring-ring/50` | [`SLIDER_THUMB_RING_WIDTH`] at [`style::FOCUS_RING_OPACITY`] |
//! | root `data-[disabled]:opacity-50`, thumb `disabled:pointer-events-none` | [`style::disabled_tint`] + a fully inert control |
//!
//! Authored on `frust::authoring` from scratch rather than wrapping
//! `frust::slider`: the baseline widget is Material/Cupertino-shaped (its own
//! track/knob metrics and state layer), and re-tinting it could not produce
//! shadcn's hairline track, bordered white thumb or 4px hover ring.
//!
//! # Two deliberate deviations
//!
//! - **The ring is 4px, not the catalog's 3px.** The thumb's class list overrides
//!   the ring width (`ring-4`), so it paints its own ring rather than calling
//!   [`style::draw_focus_ring`] — same color and opacity, one pixel wider. It is
//!   also painted on **hover**, which is the one place in the catalog where hover
//!   chrome and focus chrome are the same treatment.
//! - **Horizontal only.** The source models a vertical orientation
//!   (`data-[orientation=vertical]`); this port covers the horizontal one and
//!   leaves the vertical geometry unported.
//!
//! # Controlled, including mid-drag
//!
//! `values` is a prop. A press, a drag and an arrow key all *report* the whole new
//! value list through `on_value_change`; nothing is written locally, so a drag
//! only moves on screen once the app feeds the new values back down. Reports are
//! suppressed when the computed list equals the confirmed one, so holding a
//! thumb still costs nothing.
//!
//! # Cursor: `Grab` uncaptured, `Grabbing` from the captured arm
//!
//! The uncaptured `Move` arm asks for [`CursorIcon::Grab`] while the pointer is
//! over a thumb; the **captured** arm asks for [`CursorIcon::Grabbing`] on every
//! move, which is what keeps the shape while the pointer is dragged outside the
//! widget's own bounds (a captured pass routes only to the capturer). Neither is
//! ever requested from `Down`/`Up` — no other pass resolves the cursor.
//!
//! # Thumbs cannot cross
//!
//! A dragged thumb is clamped between its neighbours (Radix's own behavior), so a
//! two-thumb range never inverts.

use std::rc::Rc;

use frust::Theme;
use frust::authoring::{
    Action, BoxConstraints, Brush, BuildCtx, ChangeFlags, Color, CursorIcon, EventCtx, EventResult,
    InputEvent, Key, LayoutCtx, NamedKey, PaintCtx, PaintScene, Point, PointerPhase, Rect, Role,
    RoundedRect, SemanticsCtx, Shape, Size, View, Widget, erase_callback_arg,
};

use crate::hit::inside;
use crate::style::{self, PATH_TOLERANCE};

/// Thumb edge, in logical px (`size-4`).
pub const SLIDER_THUMB_SIZE: f64 = 16.0;

/// Track thickness, in logical px (`h-1.5`).
pub const SLIDER_TRACK_THICKNESS: f64 = 6.0;

/// The thumb's hover/focus ring width, in logical px (`ring-4`) — wider than the
/// catalog's [`style::FOCUS_RING_WIDTH`], which is why this component paints its
/// own ring.
pub const SLIDER_THUMB_RING_WIDTH: f64 = 4.0;

/// The width a slider takes when its constraints are unbounded, in logical px.
///
/// The source is `w-full`, which has no intrinsic width to fall back on; a
/// horizontally-unconstrained parent (a `Row` measuring loosely) would otherwise
/// give an infinite one. A port decision, not a source value.
const UNCONSTRAINED_WIDTH: f64 = 200.0;

/// Unthemed fallback track — the `neutral` preset's light `--muted`.
const FALLBACK_MUTED: Color = Color::from_rgb8(0xF5, 0xF5, 0xF5);
/// Unthemed fallback range/thumb border — light `--primary`.
const FALLBACK_PRIMARY: Color = Color::from_rgb8(0x17, 0x17, 0x17);

/// A view-held, typed change callback (erased on build).
type OnValueChange<State> = Rc<dyn Fn(&mut State, Vec<f64>)>;

/// A declarative shadcn slider. See the [module docs](self).
pub struct SliderView<State: 'static> {
    values: Vec<f64>,
    min: f64,
    max: f64,
    step: f64,
    disabled: bool,
    label: Option<String>,
    on_value_change: OnValueChange<State>,
}

/// Create a slider over `values` (one entry per thumb) that reports the whole new
/// list through `on_value_change` — a **controlled** component (see the
/// [module docs](self)).
///
/// Defaults match the source's own: `min = 0`, `max = 100`, `step = 1`.
pub fn slider<State: 'static, F: Fn(&mut State, Vec<f64>) + 'static>(
    values: Vec<f64>,
    on_value_change: F,
) -> SliderView<State> {
    SliderView {
        values,
        min: 0.0,
        max: 100.0,
        step: 1.0,
        disabled: false,
        label: None,
        on_value_change: Rc::new(on_value_change),
    }
}

impl<State: 'static> SliderView<State> {
    /// Set the value range (`min`/`max`). An empty or inverted range is clamped to
    /// `min` by the value math, never panicked on.
    pub fn range(mut self, min: f64, max: f64) -> Self {
        self.min = min;
        self.max = max;
        self
    }

    /// Set the quantization step (`step`); a non-positive step means "no
    /// quantization".
    pub fn step(mut self, step: f64) -> Self {
        self.step = step;
        self
    }

    /// Disable the control: 50% opacity and fully inert
    /// (`data-[disabled]:opacity-50` + `disabled:pointer-events-none`).
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Name the control for assistive tech (a slider paints no text of its own).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

/// The resolved slider palette.
struct SliderColors {
    /// The track (`bg-muted`).
    track: Color,
    /// The filled range and the thumb's border (`bg-primary`/`border-primary`).
    primary: Color,
    /// The thumb's fill — `bg-white`, a literal Tailwind color rather than a
    /// token, so it does **not** follow the theme's `background` role.
    thumb: Color,
}

/// Resolve the palette, falling back to the `neutral` preset's light values with
/// no theme threaded.
fn resolve_colors(theme: Option<&Theme>) -> SliderColors {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            SliderColors {
                track: scheme.surface_container_highest,
                primary: scheme.primary,
                thumb: Color::WHITE,
            }
        }
        None => SliderColors {
            track: FALLBACK_MUTED,
            primary: FALLBACK_PRIMARY,
            thumb: Color::WHITE,
        },
    }
}

impl<State: 'static> View<State> for SliderView<State> {
    type Element = SliderWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> SliderWidget {
        SliderWidget {
            values: self.values.clone(),
            min: self.min,
            max: self.max,
            step: self.step,
            disabled: self.disabled,
            label: self.label.clone(),
            width: 0.0,
            focused_thumb: 0,
            hovered: None,
            captured: None,
            on_value_change: erase_callback_arg(&self.on_value_change),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SliderWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_value_change = erase_callback_arg(&self.on_value_change);
        let mut flags = ChangeFlags::NONE;
        if prev.values != self.values {
            // The app is the source of truth: adopt the confirmed values (this is
            // what actually moves a thumb during a drag).
            element.values = self.values.clone();
            flags |= ChangeFlags::PAINT;
        }
        if prev.min != self.min || prev.max != self.max || prev.step != self.step {
            element.min = self.min;
            element.max = self.max;
            element.step = self.step;
            flags |= ChangeFlags::PAINT;
        }
        if prev.disabled != self.disabled {
            element.disabled = self.disabled;
            if self.disabled {
                element.captured = None;
                element.hovered = None;
            }
            flags |= ChangeFlags::PAINT;
        }
        if prev.label != self.label {
            element.label = self.label.clone();
            flags |= ChangeFlags::PAINT;
        }
        element.focused_thumb = element
            .focused_thumb
            .min(element.values.len().saturating_sub(1));
        flags
    }
}

/// The retained widget for a [`SliderView`].
pub struct SliderWidget {
    /// The app-confirmed values, one per thumb (source of truth; adopted on
    /// `rebuild`).
    values: Vec<f64>,
    min: f64,
    max: f64,
    step: f64,
    disabled: bool,
    label: Option<String>,
    /// The width layout resolved, retained because the event pass needs it to map
    /// a pointer position back to a value (`EventCtx` carries geometry, but the
    /// thumb math wants the same number layout used).
    width: f64,
    /// The thumb arrow keys move (a slider is one tab stop, like the roving focus
    /// of a radio group).
    focused_thumb: usize,
    /// The latched hovered thumb, self-corrected from `PaintCtx::is_hovered()`
    /// every paint.
    hovered: Option<usize>,
    /// The thumb a `Down` armed, cleared on `Up`/`Cancel`.
    captured: Option<usize>,
    on_value_change: frust::authoring::ErasedArgCallback<Vec<f64>>,
}

impl SliderWidget {
    /// The travel span of a thumb *centre*, in local x: the thumb stays inside the
    /// widget's own bounds at both extremes (Radix's in-bounds offset).
    fn travel(&self) -> (f64, f64) {
        let radius = SLIDER_THUMB_SIZE / 2.0;
        (radius, (self.width - radius).max(radius))
    }

    /// `value` as a `0.0..=1.0` fraction of the range (`0.0` for an empty range).
    fn fraction(&self, value: f64) -> f64 {
        let span = self.max - self.min;
        if span <= 0.0 {
            0.0
        } else {
            ((value - self.min) / span).clamp(0.0, 1.0)
        }
    }

    /// The local x of the centre of the thumb holding `value`.
    fn center_x(&self, value: f64) -> f64 {
        let (start, end) = self.travel();
        start + (end - start) * self.fraction(value)
    }

    /// The value a local x maps to, quantized to `step` and clamped to the range.
    fn value_at(&self, x: f64) -> f64 {
        let (start, end) = self.travel();
        let span = end - start;
        let fraction = if span <= 0.0 {
            0.0
        } else {
            ((x - start) / span).clamp(0.0, 1.0)
        };
        let raw = self.min + (self.max - self.min) * fraction;
        self.quantize(raw)
    }

    /// Snap `value` to the nearest step from `min` and clamp it to the range.
    fn quantize(&self, value: f64) -> f64 {
        let clamped = value.clamp(self.min.min(self.max), self.max.max(self.min));
        if self.step <= 0.0 {
            return clamped;
        }
        let steps = ((clamped - self.min) / self.step).round();
        (self.min + steps * self.step).clamp(self.min.min(self.max), self.max.max(self.min))
    }

    /// The thumb whose centre is nearest `x` (the one a press grabs).
    fn nearest_thumb(&self, x: f64) -> Option<usize> {
        self.values
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                let da = (self.center_x(**a) - x).abs();
                let db = (self.center_x(**b) - x).abs();
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(index, _)| index)
    }

    /// The thumb under `pos`, if the pointer is over one (its full 16px box).
    fn hit_thumb(&self, pos: Point, height: f64) -> Option<usize> {
        (0..self.values.len()).find(|index| {
            let value = self.values[*index];
            let rect = Rect::from_center_size(
                Point::new(self.center_x(value), height / 2.0),
                Size::new(SLIDER_THUMB_SIZE, SLIDER_THUMB_SIZE),
            );
            rect.contains(pos)
        })
    }

    /// The value list with thumb `index` moved to `value`, clamped between its
    /// neighbours so thumbs cannot cross.
    fn with_thumb(&self, index: usize, value: f64) -> Vec<f64> {
        let mut next = self.values.clone();
        if index >= next.len() {
            return next;
        }
        let lower = index
            .checked_sub(1)
            .and_then(|i| next.get(i).copied())
            .unwrap_or(self.min.min(self.max));
        let upper = next
            .get(index + 1)
            .copied()
            .unwrap_or(self.max.max(self.min));
        next[index] = value.clamp(lower.min(upper), upper.max(lower));
        next
    }

    /// Report `next` unless it equals the confirmed list (so a held thumb, or a
    /// key at the end of the range, costs nothing).
    fn report(&mut self, ctx: &mut EventCtx, next: Vec<f64>) -> bool {
        if next == self.values {
            return false;
        }
        (self.on_value_change)(ctx, next);
        true
    }

    /// Move thumb `index` to the value under local `x` and report it.
    fn drag_to(&mut self, ctx: &mut EventCtx, index: usize, x: f64) -> bool {
        let value = self.value_at(x);
        let next = self.with_thumb(index, value);
        self.report(ctx, next)
    }

    /// Nudge the focused thumb by `steps` steps (an arrow key) and report it.
    fn nudge(&mut self, ctx: &mut EventCtx, steps: f64) -> bool {
        let index = self.focused_thumb;
        let Some(current) = self.values.get(index).copied() else {
            return false;
        };
        let increment = if self.step > 0.0 { self.step } else { 1.0 };
        let value = self.quantize(current + increment * steps);
        let next = self.with_thumb(index, value);
        self.report(ctx, next)
    }

    /// Set the latched hovered thumb, reporting whether it changed.
    fn set_hovered(&mut self, hovered: Option<usize>) -> bool {
        let changed = self.hovered != hovered;
        self.hovered = hovered;
        changed
    }

    /// The `(start, end)` local x of the filled range: from the track start to the
    /// only thumb, or between the outermost thumbs of a multi-thumb slider.
    fn range_span(&self) -> Option<(f64, f64)> {
        match self.values.len() {
            0 => None,
            1 => Some((0.0, self.center_x(self.values[0]))),
            _ => {
                let centers: Vec<f64> = self.values.iter().map(|v| self.center_x(*v)).collect();
                let low = centers.iter().copied().fold(f64::INFINITY, f64::min);
                let high = centers.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                Some((low, high))
            }
        }
    }
}

impl Widget for SliderWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // `w-full`: take the width offered, or the documented fallback when the
        // parent offers no bound at all.
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            UNCONSTRAINED_WIDTH
        };
        // `items-center` over a 16px thumb and a 6px track: the thumb is the
        // tallest child, so it sets the row height.
        let size = bc.constrain(Size::new(width, SLIDER_THUMB_SIZE));
        self.width = size.width;
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // `PaintCtx::is_hovered` is authoritative for whether the pointer is on
        // this widget's path at all.
        if !ctx.is_hovered() || self.disabled {
            self.hovered = None;
        }

        let theme = Theme::from_paint_ctx(ctx);
        let colors = resolve_colors(theme);
        let ring = style::with_alpha(style::ring_color(None, theme), style::FOCUS_RING_OPACITY);
        let focused = ctx.has_focus();
        let origin = ctx.origin();
        let size = ctx.size();
        let tint = |color: Color| style::disabled_tint(color, self.disabled);

        // Track: full width, vertically centred, pill-rounded.
        let track_y = origin.y + (size.height - SLIDER_TRACK_THICKNESS) / 2.0;
        scene.fill_rounded_rect(
            Point::new(origin.x, track_y),
            Size::new(size.width, SLIDER_TRACK_THICKNESS),
            SLIDER_TRACK_THICKNESS / 2.0,
            tint(colors.track),
        );

        // Range: the filled span, same thickness, same rounding.
        if let Some((start, end)) = self.range_span() {
            let width = (end - start).max(0.0);
            if width > 0.0 {
                scene.fill_rounded_rect(
                    Point::new(origin.x + start, track_y),
                    Size::new(width, SLIDER_TRACK_THICKNESS),
                    SLIDER_TRACK_THICKNESS / 2.0,
                    tint(colors.primary),
                );
            }
        }

        // Thumbs: a white circle with a `primary` hairline over `shadow-sm`, plus
        // the 4px ring while hovered, dragged or focused.
        let radius = SLIDER_THUMB_SIZE / 2.0;
        let thumb_size = Size::new(SLIDER_THUMB_SIZE, SLIDER_THUMB_SIZE);
        for (index, value) in self.values.iter().enumerate() {
            let center = Point::new(
                origin.x + self.center_x(*value),
                origin.y + size.height / 2.0,
            );
            let thumb_origin = Point::new(center.x - radius, center.y - radius);
            let ringed = !self.disabled
                && (self.hovered == Some(index)
                    || self.captured == Some(index)
                    || (focused && index == self.focused_thumb));

            if ringed {
                // Painted before the thumb so the thumb's own fill stays crisp on
                // top of it; the ring is centred on the thumb's edge, so it
                // occupies the band just outside it.
                let half = SLIDER_THUMB_RING_WIDTH / 2.0;
                let outline = RoundedRect::new(
                    -half,
                    -half,
                    SLIDER_THUMB_SIZE + half,
                    SLIDER_THUMB_SIZE + half,
                    radius + half,
                );
                scene.stroke_path(
                    thumb_origin,
                    &Shape::to_path(&outline, PATH_TOLERANCE),
                    SLIDER_THUMB_RING_WIDTH,
                    &Brush::Solid(ring),
                );
            }

            style::draw_shadow(
                scene,
                thumb_origin,
                thumb_size,
                radius,
                style::SHADOW_SM,
                theme,
            );
            scene.fill_rounded_rect(thumb_origin, thumb_size, radius, tint(colors.thumb));
            let inset = style::BORDER_WIDTH / 2.0;
            let border = RoundedRect::from_rect(
                Rect::from_origin_size(Point::ORIGIN, thumb_size).inset(-inset),
                radius + inset,
            );
            scene.stroke_path(
                thumb_origin,
                &Shape::to_path(&border, PATH_TOLERANCE),
                style::BORDER_WIDTH,
                &Brush::Solid(tint(colors.primary)),
            );
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // `disabled:pointer-events-none` on the thumb: a disabled slider is not a
        // pointer target and takes no keys.
        if self.disabled {
            return EventResult::Ignored;
        }
        let size = ctx.size();
        match event {
            InputEvent::Key(key) => {
                let steps = match &key.key {
                    Key::Named(NamedKey::ArrowRight | NamedKey::ArrowUp) => 1.0,
                    Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowDown) => -1.0,
                    Key::Named(NamedKey::Home) => {
                        let next = self.with_thumb(self.focused_thumb, self.min);
                        self.report(ctx, next);
                        return EventResult::Handled;
                    }
                    Key::Named(NamedKey::End) => {
                        let next = self.with_thumb(self.focused_thumb, self.max);
                        self.report(ctx, next);
                        return EventResult::Handled;
                    }
                    _ => return EventResult::Ignored,
                };
                self.nudge(ctx, steps);
                EventResult::Handled
            }
            InputEvent::Pointer(p) => match p.phase {
                PointerPhase::Down => {
                    if !inside(p.position, size) {
                        return EventResult::Ignored;
                    }
                    // A press anywhere on the row grabs the nearest thumb and jumps
                    // it to the pressed position (Radix's own behavior).
                    let Some(index) = self.nearest_thumb(p.position.x) else {
                        return EventResult::Ignored;
                    };
                    self.captured = Some(index);
                    self.focused_thumb = index;
                    ctx.capture_pointer();
                    ctx.request_focus();
                    // The thumb's ring appears on focus even if the value did not
                    // change, so the redraw is unconditional.
                    ctx.request_redraw();
                    self.drag_to(ctx, index, p.position.x);
                    EventResult::Handled
                }
                PointerPhase::Move => {
                    let Some(index) = self.captured else {
                        // The hover/cursor pass: `Grab` over a thumb, and the
                        // latched thumb drives its 4px ring.
                        let over = self.hit_thumb(p.position, size.height);
                        if over.is_some() {
                            ctx.claim_hover();
                            ctx.set_cursor(CursorIcon::Grab);
                        } else if inside(p.position, size) {
                            // Still the slider's row: claim so an ancestor's chrome
                            // does not win, but ask for no thumb-specific shape.
                            ctx.claim_hover();
                        }
                        if self.set_hovered(over) {
                            ctx.request_redraw();
                        }
                        return EventResult::Ignored;
                    };
                    // The captured arm re-asks every move, which is what keeps
                    // `Grabbing` alive while the pointer is dragged outside the
                    // widget's bounds.
                    ctx.set_cursor(CursorIcon::Grabbing);
                    self.drag_to(ctx, index, p.position.x);
                    EventResult::Handled
                }
                PointerPhase::Up => {
                    let Some(index) = self.captured.take() else {
                        return EventResult::Ignored;
                    };
                    // A release commits wherever the pointer ended, inside the
                    // bounds or not — the value is already clamped to the range.
                    self.drag_to(ctx, index, p.position.x);
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Cancel => {
                    if self.captured.take().is_none() {
                        return EventResult::Ignored;
                    }
                    // Internal flags only — a cancel keeps whatever value the app
                    // last confirmed.
                    ctx.request_redraw();
                    EventResult::Handled
                }
            },
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // One thumb is one `Role::Slider` node; a multi-thumb slider wraps its
        // thumbs in a group, since accesskit has no multi-value slider.
        let push_thumb = |ctx: &mut SemanticsCtx, index: usize, value: f64| {
            ctx.push_node(Role::Slider, |node| {
                if let Some(label) = &self.label {
                    node.set_label(label.as_str());
                }
                node.set_numeric_value(value);
                node.set_min_numeric_value(self.min);
                node.set_max_numeric_value(self.max);
                if self.step > 0.0 {
                    node.set_numeric_value_step(self.step);
                }
                if self.disabled {
                    node.set_disabled();
                } else {
                    node.add_action(Action::Increment);
                    node.add_action(Action::Decrement);
                    node.add_action(Action::SetValue);
                }
                let _ = index;
            });
        };
        match self.values.len() {
            1 => push_thumb(ctx, 0, self.values[0]),
            _ => {
                ctx.push_container(
                    Role::Group,
                    |node| {
                        if let Some(label) = &self.label {
                            node.set_label(label.as_str());
                        }
                        if self.disabled {
                            node.set_disabled();
                        }
                    },
                    |ctx| {
                        for (index, value) in self.values.iter().enumerate() {
                            push_thumb(ctx, index, *value);
                        }
                    },
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::FrameTime;
    use frust::authoring::{
        BezPath, EventOutcome, KeyEvent, Modifiers, PointerButton, PointerEvent, SemanticsUpdate,
    };
    use std::any::Any;

    const WIDTH: f64 = 216.0;

    /// Records the fills, stroked paths and shadows this widget emits.
    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<(Rect, f64, Color)>,
        shadows: Vec<(Point, Size, f64, f64, Color)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn stroke_path(&mut self, origin: Point, path: &BezPath, width: f64, brush: &Brush) {
            let bbox = path.bounding_box() + origin.to_vec2();
            let color = match brush {
                Brush::Solid(c) => *c,
                _ => Color::TRANSPARENT,
            };
            self.strokes.push((bbox, width, color));
        }
        fn draw_shadow(&mut self, o: Point, s: Size, radius: f64, std_dev: f64, color: Color) {
            self.shadows.push((o, s, radius, std_dev, color));
        }
    }

    #[derive(Default)]
    struct Values {
        last: Option<Vec<f64>>,
        count: u32,
    }

    fn view(values: Vec<f64>) -> SliderView<Values> {
        slider::<Values, _>(values, |s: &mut Values, v: Vec<f64>| {
            s.last = Some(v);
            s.count += 1;
        })
    }

    /// Build + lay out a slider at [`WIDTH`], returning it with its size.
    fn laid_out(view: SliderView<Values>) -> (SliderWidget, Size) {
        let mut counter = 0u64;
        let mut w = View::<Values>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut ctx = LayoutCtx::new();
        let size = w.layout(
            &mut ctx,
            &BoxConstraints::new(Size::ZERO, Size::new(WIDTH, 100.0)),
        );
        (w, size)
    }

    fn paint(w: &mut SliderWidget, size: Size, theme: Option<&Theme>) -> Recorder {
        let mut rec = Recorder::default();
        let mut ctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, size).with_theme(t),
            None => PaintCtx::new(Point::ZERO, size),
        };
        w.paint(&mut ctx, &mut rec);
        rec
    }

    fn pointer(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn key(key: Key) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key,
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    fn dispatch(
        w: &mut SliderWidget,
        state: &mut Values,
        size: Size,
        event: &InputEvent,
    ) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, size);
        w.event(&mut ctx, event)
    }

    // ---- Geometry / value math --------------------------------------------

    #[test]
    fn layout_takes_the_offered_width_and_the_thumbs_height() {
        let (w, size) = laid_out(view(vec![50.0]));
        assert_eq!(size, Size::new(WIDTH, SLIDER_THUMB_SIZE));
        assert_eq!(w.width, WIDTH);

        // Unbounded width falls back to the documented constant.
        let mut counter = 0u64;
        let mut unbounded =
            View::<Values>::build(&view(vec![0.0]), &mut BuildCtx::new(&mut counter));
        let mut ctx = LayoutCtx::new();
        let size = unbounded.layout(
            &mut ctx,
            &BoxConstraints::new(Size::ZERO, Size::new(f64::INFINITY, 100.0)),
        );
        assert_eq!(size.width, UNCONSTRAINED_WIDTH);
    }

    #[test]
    fn the_thumb_centre_stays_inside_the_bounds_at_both_extremes() {
        let (w, _) = laid_out(view(vec![0.0]));
        let radius = SLIDER_THUMB_SIZE / 2.0;
        assert_eq!(w.center_x(0.0), radius);
        assert_eq!(w.center_x(100.0), WIDTH - radius);
        assert_eq!(w.center_x(50.0), WIDTH / 2.0);
        // ...and a value outside the range is clamped, not extrapolated.
        assert_eq!(w.center_x(-20.0), radius);
        assert_eq!(w.center_x(400.0), WIDTH - radius);
    }

    #[test]
    fn a_position_maps_back_to_a_step_quantized_value() {
        let (w, _) = laid_out(view(vec![0.0]).step(10.0));
        assert_eq!(w.value_at(w.center_x(50.0)), 50.0);
        // A hair off a step still lands on it.
        assert_eq!(w.value_at(w.center_x(50.0) + 1.0), 50.0);
        assert_eq!(w.value_at(-500.0), 0.0, "clamped to min");
        assert_eq!(w.value_at(9_999.0), 100.0, "clamped to max");

        // A non-positive step means no quantization.
        let (free, _) = laid_out(view(vec![0.0]).step(0.0));
        let x = free.center_x(33.3);
        assert!((free.value_at(x) - 33.3).abs() < 1e-6);
    }

    #[test]
    fn a_custom_range_and_an_empty_range_both_behave() {
        let (w, _) = laid_out(view(vec![5.0]).range(-10.0, 10.0).step(5.0));
        assert_eq!(w.value_at(w.center_x(-10.0)), -10.0);
        assert_eq!(w.value_at(w.center_x(0.0)), 0.0);
        assert_eq!(w.quantize(7.4), 5.0);

        let (empty, _) = laid_out(view(vec![3.0]).range(3.0, 3.0));
        assert_eq!(empty.fraction(3.0), 0.0);
        assert_eq!(empty.value_at(100.0), 3.0);
    }

    // ---- Paint ------------------------------------------------------------

    #[test]
    fn a_single_thumb_slider_paints_track_range_and_thumb() {
        let theme = crate::theme();
        let scheme = theme.scheme();
        let (mut w, size) = laid_out(view(vec![50.0]));
        let rec = paint(&mut w, size, Some(&theme));

        assert_eq!(rec.rrects.len(), 3, "track, range, thumb");
        let (track_origin, track_size, track_radius, track) = rec.rrects[0];
        assert_eq!(
            track_origin.y,
            (SLIDER_THUMB_SIZE - SLIDER_TRACK_THICKNESS) / 2.0
        );
        assert_eq!(track_size, Size::new(WIDTH, SLIDER_TRACK_THICKNESS));
        assert_eq!(track_radius, SLIDER_TRACK_THICKNESS / 2.0);
        assert_eq!(track, scheme.surface_container_highest, "`bg-muted`");

        let (range_origin, range_size, _, range) = rec.rrects[1];
        assert_eq!(range_origin.x, 0.0, "the range starts at the track's edge");
        assert_eq!(range_size.width, w.center_x(50.0));
        assert_eq!(range, scheme.primary);

        let (thumb_origin, thumb_size, thumb_radius, thumb) = rec.rrects[2];
        assert_eq!(thumb_size, Size::new(SLIDER_THUMB_SIZE, SLIDER_THUMB_SIZE));
        assert_eq!(thumb_radius, SLIDER_THUMB_SIZE / 2.0);
        assert_eq!(thumb, Color::WHITE, "`bg-white`, not the background role");
        assert_eq!(thumb_origin.x, w.center_x(50.0) - SLIDER_THUMB_SIZE / 2.0);

        // `shadow-sm` under the thumb, and a `primary` hairline around it.
        assert_eq!(rec.shadows.len(), 1);
        assert_eq!(rec.shadows[0].3, style::SHADOW_SM.std_dev);
        assert_eq!(rec.strokes.len(), 1);
        assert_eq!(rec.strokes[0].1, style::BORDER_WIDTH);
        assert_eq!(rec.strokes[0].2, scheme.primary);
    }

    #[test]
    fn a_two_thumb_slider_fills_the_span_between_them() {
        let (mut w, size) = laid_out(view(vec![25.0, 75.0]));
        let rec = paint(&mut w, size, None);
        assert_eq!(rec.rrects.len(), 4, "track, range, two thumbs");
        let (range_origin, range_size, _, _) = rec.rrects[1];
        assert_eq!(range_origin.x, w.center_x(25.0));
        assert_eq!(range_size.width, w.center_x(75.0) - w.center_x(25.0));
        assert_eq!(rec.shadows.len(), 2, "one shadow per thumb");
    }

    #[test]
    fn a_hovered_or_dragged_thumb_paints_the_4px_ring() {
        let theme = crate::theme();
        let (mut w, size) = laid_out(view(vec![50.0]));
        assert_eq!(
            paint(&mut w, size, Some(&theme)).strokes.len(),
            1,
            "border only"
        );

        w.captured = Some(0);
        let rec = paint(&mut w, size, Some(&theme));
        assert_eq!(rec.strokes.len(), 2, "ring + border");
        let (bbox, width, color) = rec.strokes[0];
        assert_eq!(
            width, SLIDER_THUMB_RING_WIDTH,
            "`ring-4`, not the catalog's 3px"
        );
        let ring = style::ring_color(None, Some(&theme));
        assert_eq!(color.components[..3], ring.components[..3]);
        assert_eq!(
            color.components[3],
            style::FOCUS_RING_OPACITY,
            "`ring-ring/50`"
        );
        // The ring sits outside the thumb's own box.
        let thumb_left = w.center_x(50.0) - SLIDER_THUMB_SIZE / 2.0;
        assert!(bbox.x0 < thumb_left);
    }

    #[test]
    fn disabled_paint_halves_every_painted_alpha_and_drops_the_ring() {
        let theme = crate::theme();
        let mut counter = 0u64;
        let disabled_view = view(vec![50.0]).disabled(true);
        let mut w = View::<Values>::build(&disabled_view, &mut BuildCtx::new(&mut counter));
        let mut lctx = LayoutCtx::new();
        let size = w.layout(
            &mut lctx,
            &BoxConstraints::new(Size::ZERO, Size::new(WIDTH, 100.0)),
        );
        w.hovered = Some(0);
        let rec = paint(&mut w, size, Some(&theme));
        assert_eq!(
            rec.rrects[0].3.components[3],
            style::DISABLED_OPACITY,
            "`data-[disabled]:opacity-50`"
        );
        assert_eq!(rec.strokes.len(), 1, "no ring on a disabled thumb");
        assert_eq!(w.hovered, None, "and no latched hover either");
    }

    // ---- Interaction ------------------------------------------------------

    #[test]
    fn a_press_grabs_the_nearest_thumb_and_jumps_it_to_the_pressed_position() {
        let (mut w, size) = laid_out(view(vec![10.0, 90.0]));
        let mut state = Values::default();
        let x = w.center_x(60.0);

        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, x, 8.0),
        );
        assert_eq!(w.captured, Some(1), "60 is nearer the 90 thumb");
        assert_eq!(w.focused_thumb, 1);
        assert_eq!(state.last.clone().unwrap(), vec![10.0, 60.0]);
        assert_eq!(w.values, vec![10.0, 90.0], "the app owns the values");
    }

    #[test]
    fn a_captured_drag_reports_every_change_and_nothing_when_still() {
        let (mut w, size) = laid_out(view(vec![50.0]));
        let mut state = Values::default();
        let start = w.center_x(50.0);
        let seventy = w.center_x(70.0);
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, start, 8.0),
        );
        assert_eq!(
            state.count, 0,
            "pressing the thumb's own position is a no-op"
        );

        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Move, seventy, 8.0),
        );
        assert_eq!(state.last.clone().unwrap(), vec![70.0]);

        // Once the app confirms the reported value, holding the thumb still
        // reports nothing more (the suppression is against the *confirmed* list —
        // an app that never feeds a value back keeps hearing the same request,
        // which is the controlled contract, not a bug).
        let mut counter = 0u64;
        View::<Values>::rebuild(
            &view(vec![70.0]),
            &view(vec![50.0]),
            &mut w,
            &mut BuildCtx::new(&mut counter),
        );
        let reports = state.count;
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Move, seventy, 8.0),
        );
        assert_eq!(state.count, reports);

        // A drag past the end clamps to `max`.
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Move, WIDTH + 500.0, 8.0),
        );
        assert_eq!(state.last.clone().unwrap(), vec![100.0]);
    }

    #[test]
    fn an_uncaptured_move_never_changes_the_value() {
        let (mut w, size) = laid_out(view(vec![50.0]));
        let mut state = Values::default();
        let ten = w.center_x(10.0);
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Move, ten, 8.0),
        );
        assert_eq!(state.count, 0);
    }

    #[test]
    fn thumbs_cannot_cross_their_neighbours() {
        let (mut w, size) = laid_out(view(vec![40.0, 60.0]));
        let mut state = Values::default();
        let (forty, ninety_five) = (w.center_x(40.0), w.center_x(95.0));
        // Grab the lower thumb and drag it well past the upper one.
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, forty, 8.0),
        );
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Move, ninety_five, 8.0),
        );
        assert_eq!(
            state.last.clone().unwrap(),
            vec![60.0, 60.0],
            "clamped at 60"
        );
    }

    #[test]
    fn cancel_disarms_without_reporting_and_up_commits() {
        let (mut w, size) = laid_out(view(vec![50.0]));
        let mut state = Values::default();
        let (fifty, eighty) = (w.center_x(50.0), w.center_x(80.0));
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, fifty, 8.0),
        );
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Cancel, eighty, 8.0),
        );
        assert_eq!(state.count, 0, "a cancel reports nothing");
        assert_eq!(w.captured, None);

        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, fifty, 8.0),
        );
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Up, eighty, 8.0),
        );
        assert_eq!(
            state.last.clone().unwrap(),
            vec![80.0],
            "the release commits"
        );
        assert_eq!(w.captured, None);
    }

    #[test]
    fn arrow_keys_nudge_by_one_step_and_home_end_jump_to_the_bounds() {
        let (mut w, size) = laid_out(view(vec![50.0]).step(5.0));
        let mut state = Values::default();
        dispatch(
            &mut w,
            &mut state,
            size,
            &key(Key::Named(NamedKey::ArrowRight)),
        );
        assert_eq!(state.last.clone().unwrap(), vec![55.0]);
        dispatch(
            &mut w,
            &mut state,
            size,
            &key(Key::Named(NamedKey::ArrowLeft)),
        );
        assert_eq!(state.last.clone().unwrap(), vec![45.0]);
        dispatch(
            &mut w,
            &mut state,
            size,
            &key(Key::Named(NamedKey::ArrowUp)),
        );
        assert_eq!(state.last.clone().unwrap(), vec![55.0]);
        dispatch(&mut w, &mut state, size, &key(Key::Named(NamedKey::Home)));
        assert_eq!(state.last.clone().unwrap(), vec![0.0]);
        dispatch(&mut w, &mut state, size, &key(Key::Named(NamedKey::End)));
        assert_eq!(state.last.clone().unwrap(), vec![100.0]);
        assert_eq!(
            dispatch(&mut w, &mut state, size, &key(Key::Named(NamedKey::Tab))),
            EventResult::Ignored
        );
        assert_eq!(w.values, vec![50.0], "no key ever wrote the value");
    }

    #[test]
    fn an_arrow_key_at_the_end_of_the_range_reports_nothing() {
        let (mut w, size) = laid_out(view(vec![100.0]));
        let mut state = Values::default();
        dispatch(
            &mut w,
            &mut state,
            size,
            &key(Key::Named(NamedKey::ArrowRight)),
        );
        assert_eq!(state.count, 0);
    }

    #[test]
    fn a_disabled_slider_is_inert_to_pointer_and_key() {
        let mut counter = 0u64;
        let disabled = view(vec![50.0]).disabled(true);
        let mut w = View::<Values>::build(&disabled, &mut BuildCtx::new(&mut counter));
        let mut lctx = LayoutCtx::new();
        let size = w.layout(
            &mut lctx,
            &BoxConstraints::new(Size::ZERO, Size::new(WIDTH, 100.0)),
        );
        let mut state = Values::default();
        for event in [
            pointer(PointerPhase::Down, WIDTH / 2.0, 8.0),
            pointer(PointerPhase::Move, WIDTH / 2.0, 8.0),
            key(Key::Named(NamedKey::ArrowRight)),
        ] {
            assert_eq!(
                dispatch(&mut w, &mut state, size, &event),
                EventResult::Ignored
            );
        }
        assert_eq!(state.count, 0);
    }

    #[test]
    fn the_hover_latch_tracks_the_thumb_and_repaints_on_change() {
        let (mut w, size) = laid_out(view(vec![50.0]));
        let mut state = Values::default();

        let center = w.center_x(50.0);
        let state_any: &mut dyn Any = &mut state;
        let mut enter = EventCtx::new(state_any, Point::ZERO, size);
        w.event(&mut enter, &pointer(PointerPhase::Move, center, 8.0));
        assert_eq!(w.hovered, Some(0));
        assert!(enter.needs_redraw());

        let state_any: &mut dyn Any = &mut state;
        let mut off_thumb = EventCtx::new(state_any, Point::ZERO, size);
        w.event(&mut off_thumb, &pointer(PointerPhase::Move, 2.0, 8.0));
        assert_eq!(w.hovered, None, "the bare track is not a thumb");
        assert!(off_thumb.needs_redraw());
    }

    #[test]
    fn rebuild_adopts_the_confirmed_values_and_clamps_the_focused_thumb() {
        let mut counter = 0u64;
        let prev = view(vec![10.0, 20.0]);
        let mut w = View::<Values>::build(&prev, &mut BuildCtx::new(&mut counter));
        w.focused_thumb = 1;
        let next = view(vec![30.0]);
        let flags = View::<Values>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert_eq!(w.values, vec![30.0]);
        assert_eq!(w.focused_thumb, 0);
        assert!(flags.needs_paint());
    }

    // ---- Root-driven: cursor, focus ring, semantics -------------------------

    struct Harness {
        root: frust_core::RenderRoot<Values, SliderView<Values>>,
        state: Values,
    }

    impl Harness {
        fn new(values: Vec<f64>) -> Self {
            let mut h = Harness {
                root: frust_core::RenderRoot::new(),
                state: Values::default(),
            };
            h.root.set_theme(Box::new(crate::theme()));
            let mut logic = move |_s: &mut Values| view(values.clone()).label("volume");
            h.root.rebuild(&mut logic, &mut h.state);
            h.root.layout(Size::new(WIDTH, 100.0));
            h
        }

        fn dispatch(&mut self, event: &InputEvent) -> EventOutcome {
            self.root.event(&mut self.state, event)
        }

        fn paint(&mut self) -> Recorder {
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, FrameTime::ZERO);
            rec
        }

        fn semantics(&self) -> SemanticsUpdate {
            self.root.semantics()
        }
    }

    #[test]
    fn the_cursor_is_grab_over_a_thumb_and_grabbing_while_dragging() {
        let mut h = Harness::new(vec![50.0]);
        let center = WIDTH / 2.0;
        h.dispatch(&pointer(PointerPhase::Move, center, 8.0));
        assert_eq!(h.root.cursor(), CursorIcon::Grab);

        // Over the bare track: no thumb shape.
        h.dispatch(&pointer(PointerPhase::Move, 2.0, 8.0));
        assert_eq!(h.root.cursor(), CursorIcon::Default);

        // A captured drag keeps `Grabbing` even outside the widget's bounds.
        h.dispatch(&pointer(PointerPhase::Down, center, 8.0));
        h.dispatch(&pointer(PointerPhase::Move, WIDTH + 400.0, 400.0));
        assert_eq!(h.root.cursor(), CursorIcon::Grabbing);
    }

    #[test]
    fn focus_paints_the_thumbs_ring_after_a_press() {
        let mut h = Harness::new(vec![50.0]);
        assert_eq!(h.paint().strokes.len(), 1, "hairline only at rest");
        h.dispatch(&pointer(PointerPhase::Down, WIDTH / 2.0, 8.0));
        assert!(h.root.is_focus_active());
        // Release so the capture-driven ring is not what is being observed.
        h.dispatch(&pointer(PointerPhase::Up, WIDTH / 2.0, 8.0));
        let rec = h.paint();
        assert_eq!(rec.strokes.len(), 2, "focus ring + hairline");
        assert_eq!(rec.strokes[0].1, SLIDER_THUMB_RING_WIDTH);
    }

    #[test]
    fn semantics_reports_one_slider_node_per_thumb_with_its_numeric_range() {
        let single = Harness::new(vec![42.0]);
        let update = single.semantics();
        let sliders: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::Slider)
            .collect();
        assert_eq!(sliders.len(), 1);
        let node = &sliders[0].1;
        assert_eq!(node.label(), Some("volume"));
        assert_eq!(node.numeric_value(), Some(42.0));
        assert_eq!(node.min_numeric_value(), Some(0.0));
        assert_eq!(node.max_numeric_value(), Some(100.0));
        assert_eq!(node.numeric_value_step(), Some(1.0));
        assert!(node.supports_action(Action::Increment));
        assert!(node.supports_action(Action::Decrement));
        assert!(
            update.nodes.iter().all(|(_, n)| n.role() != Role::Group),
            "a single-thumb slider needs no wrapper group"
        );

        let multi = Harness::new(vec![20.0, 80.0]);
        let update = multi.semantics();
        assert_eq!(
            update
                .nodes
                .iter()
                .filter(|(_, n)| n.role() == Role::Slider)
                .count(),
            2
        );
        let group = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Group)
            .expect("a multi-thumb slider wraps its thumbs");
        assert_eq!(group.1.children().len(), 2);
    }
}
