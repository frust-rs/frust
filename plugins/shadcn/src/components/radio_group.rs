//! Ports shadcn/ui's **RadioGroup** (`RadioGroup` + `RadioGroupItem`) from
//! `tmp/ui/apps/v4/registry/new-york-v4/ui/radio-group.tsx` (rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved 2026-08-17).
//!
//! | class | here |
//! |---|---|
//! | `grid gap-3` (root) | a single vertical column at [`RADIO_GAP`] |
//! | `aspect-square size-4 rounded-full` (item) | a [`RADIO_SIZE`] circle |
//! | `border border-input` | [`style::BORDER_WIDTH`] over `outline_variant` |
//! | `shadow-xs` | [`style::SHADOW_XS`] |
//! | `dark:bg-input/30` | [`DARK_FILL_ALPHA`] over `outline_variant`, dark only |
//! | indicator `CircleIcon size-2 fill-primary` | a [`RADIO_DOT_SIZE`] `primary` dot |
//! | `focus-visible:border-ring` + `ring-[3px] ring-ring/50` | [`style::focus_border`] + [`style::draw_focus_ring`] |
//! | `disabled:opacity-50` + `disabled:cursor-not-allowed` | [`style::disabled_tint`] + [`style::DISABLED_CURSOR`] |
//!
//! **The checked border stays `border-input`.** The source authors no
//! `data-[state=checked]:border-primary` on the item (unlike `checkbox`): the
//! only checked-state change is the `primary` dot appearing inside an unchanged
//! ring.
//!
//! # One widget, N items — and what that means for the hover rule
//!
//! The group paints its own items rather than owning a `ChildPod` per item (the
//! shipped precedent for a fixed small item strip — `frust_glyph::tabs`,
//! `frust_glyph::segmented_control`). Item geometry, hit testing, roving focus
//! and arrow-key traversal then all live in one place, which is what makes
//! group-level keyboard semantics expressible at all.
//!
//! It also means the "a container claims hover *after* routing" rule
//! (`docs/CODE_STANDARDS.md`'s Interaction Semantics) has nothing to order here:
//! there is no child to route to, so the group's own claim from its uncaptured
//! `Move` arm is the only claim in the pass. Like `checkbox`, the source authors
//! no `hover:` variant, so the claim exists for the *path* (an enclosing row or
//! card reads hovered) and for the cursor, not for chrome of its own.
//!
//! # Roving focus, the way Radix does it
//!
//! A radio group is one tab stop: focus lands on the group and one item inside it
//! is the "current" one. That item index is retained
//! ([`RadioGroupWidget::focused_item`]) — seeded from the selected value, moved by
//! a `Down` on an item or by an arrow key — and the focus ring paints on it while
//! `PaintCtx::has_focus()` holds.
//!
//! Arrow keys **select as they move** (WAI-ARIA's radio-group pattern, which is
//! Radix's behavior too): `ArrowDown`/`ArrowRight` report the next enabled
//! value, `ArrowUp`/`ArrowLeft` the previous, wrapping at both ends, skipping
//! disabled items. `Space`/`Enter` report the currently-focused item. Every one
//! of those is a *report* — the group never writes its own `value`.

use std::rc::Rc;

use frust::authoring::{
    Action, BoxConstraints, Brush, BuildCtx, ChangeFlags, Color, CursorIcon, EventCtx, EventResult,
    InputEvent, Key, KeyEvent, LayoutCtx, NamedKey, PaintCtx, PaintScene, Point, PointerPhase,
    Rect, Role, RoundedRect, SemanticsCtx, Shape, Size, View, Widget, erase_callback_arg,
};
use frust::{Brightness, Theme};

use crate::hit::presses;
use crate::style::{self, PATH_TOLERANCE};

/// Item edge, in logical px (`size-4`).
pub const RADIO_SIZE: f64 = 16.0;

/// Vertical gap between items, in logical px (`gap-3` on the grid root).
pub const RADIO_GAP: f64 = 12.0;

/// Indicator dot edge, in logical px (`size-2` on the `CircleIcon`).
pub const RADIO_DOT_SIZE: f64 = 8.0;

/// Alpha of the item fill in dark mode (`dark:bg-input/30`); light mode paints
/// no fill at all.
const DARK_FILL_ALPHA: f32 = 0.30;

/// Unthemed fallback ring token — the `neutral` preset's light `--input`.
const FALLBACK_INPUT: Color = Color::from_rgb8(0xE5, 0xE5, 0xE5);
/// Unthemed fallback dot — the `neutral` preset's light `--primary`.
const FALLBACK_PRIMARY: Color = Color::from_rgb8(0x17, 0x17, 0x17);

/// One option in a [`RadioGroupView`]: the value it reports plus its own
/// accessible name and enabled state.
///
/// shadcn pairs each `RadioGroupItem` with a sibling `<Label>` rather than
/// nesting text in it, and so does this port — [`RadioGroupItem::label`] names
/// the item for assistive tech, while the visible label is composed alongside by
/// the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RadioGroupItem {
    value: String,
    label: Option<String>,
    disabled: bool,
}

/// Create an item reporting `value` when chosen.
pub fn radio_group_item(value: impl Into<String>) -> RadioGroupItem {
    RadioGroupItem {
        value: value.into(),
        label: None,
        disabled: false,
    }
}

impl RadioGroupItem {
    /// Name this item for assistive tech (the item paints no text of its own).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Disable this one item: 50% opacity, inert, not-allowed cursor, and
    /// skipped by arrow-key traversal.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// The value this item reports.
    pub fn value(&self) -> &str {
        &self.value
    }
}

/// A view-held, typed selection callback (erased on build).
type OnValueChange<State> = Rc<dyn Fn(&mut State, String)>;

/// A declarative shadcn radio group. See the [module docs](self).
pub struct RadioGroupView<State: 'static> {
    value: String,
    items: Vec<RadioGroupItem>,
    disabled: bool,
    on_value_change: OnValueChange<State>,
}

/// Create a radio group whose selected item is the one whose value equals
/// `value` (none, if no item matches), reporting a chosen item's value through
/// `on_value_change` — a **controlled** component: the group never writes its own
/// `value`, the app does, and the next `rebuild` feeds it back down.
pub fn radio_group<State: 'static, F: Fn(&mut State, String) + 'static>(
    value: impl Into<String>,
    items: Vec<RadioGroupItem>,
    on_value_change: F,
) -> RadioGroupView<State> {
    RadioGroupView {
        value: value.into(),
        items,
        disabled: false,
        on_value_change: Rc::new(on_value_change),
    }
}

impl<State: 'static> RadioGroupView<State> {
    /// Disable the whole group (every item goes inert at 50% opacity).
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// The resolved item palette.
struct RadioColors {
    /// The item ring (`border-input`, checked or not).
    ring: Color,
    /// The indicator dot (`fill-primary`).
    dot: Color,
    /// `Some(input/30)` in dark mode, `None` in light (no `bg-*` class).
    fill: Option<Color>,
}

/// Resolve the palette, falling back to the `neutral` preset's light values with
/// no theme threaded.
fn resolve_colors(theme: Option<&Theme>) -> RadioColors {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            RadioColors {
                ring: scheme.outline_variant,
                dot: scheme.primary,
                fill: (theme.brightness == Brightness::Dark)
                    .then(|| style::scale_alpha(scheme.outline_variant, DARK_FILL_ALPHA)),
            }
        }
        None => RadioColors {
            ring: FALLBACK_INPUT,
            dot: FALLBACK_PRIMARY,
            fill: None,
        },
    }
}

/// Whether `key` activates the focused item: `Space` (arriving as typed text —
/// there is no `NamedKey::Space`) or `Enter`.
fn is_activation_key(key: &KeyEvent) -> bool {
    match &key.key {
        Key::Named(NamedKey::Enter) => true,
        Key::Character(text) => text == " ",
        _ => false,
    }
}

/// Which way an arrow key moves the roving focus, or `None` for any other key.
///
/// Both axes move: the source's grid is vertical, but Radix accepts either pair
/// and so does this port.
fn arrow_step(key: &KeyEvent) -> Option<isize> {
    match &key.key {
        Key::Named(NamedKey::ArrowDown | NamedKey::ArrowRight) => Some(1),
        Key::Named(NamedKey::ArrowUp | NamedKey::ArrowLeft) => Some(-1),
        _ => None,
    }
}

impl<State: 'static> View<State> for RadioGroupView<State> {
    type Element = RadioGroupWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> RadioGroupWidget {
        let focused_item = selected_index(&self.items, &self.value).unwrap_or(0);
        RadioGroupWidget {
            value: self.value.clone(),
            items: self.items.clone(),
            disabled: self.disabled,
            focused_item,
            captured: None,
            on_value_change: erase_callback_arg(&self.on_value_change),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut RadioGroupWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_value_change = erase_callback_arg(&self.on_value_change);
        let mut flags = ChangeFlags::NONE;
        if prev.items != self.items {
            element.items = self.items.clone();
            element.captured = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.value != self.value {
            // The app is the source of truth: adopt the confirmed value, and let
            // the roving focus follow it (that is where a keyboard user is).
            element.value = self.value.clone();
            if let Some(index) = selected_index(&element.items, &element.value) {
                element.focused_item = index;
            }
            flags |= ChangeFlags::PAINT;
        }
        if prev.disabled != self.disabled {
            element.disabled = self.disabled;
            if self.disabled {
                element.captured = None;
            }
            flags |= ChangeFlags::PAINT;
        }
        element.focused_item = element
            .focused_item
            .min(element.items.len().saturating_sub(1));
        flags
    }
}

/// The index of the item whose value equals `value`, if any.
fn selected_index(items: &[RadioGroupItem], value: &str) -> Option<usize> {
    items.iter().position(|item| item.value == value)
}

/// The retained widget for a [`RadioGroupView`].
pub struct RadioGroupWidget {
    /// The app-confirmed selected value (source of truth; adopted on `rebuild`).
    value: String,
    items: Vec<RadioGroupItem>,
    disabled: bool,
    /// The item the roving focus sits on — the one that paints the focus ring
    /// while the group holds focus (see the [module docs](self)).
    focused_item: usize,
    /// The item a `Down` armed, cleared on `Up`/`Cancel`. Nothing paints
    /// differently while armed (the source authors no `:active` rule).
    captured: Option<usize>,
    on_value_change: frust::authoring::ErasedArgCallback<String>,
}

impl RadioGroupWidget {
    /// Whether item `index` can be interacted with.
    fn enabled(&self, index: usize) -> bool {
        !self.disabled && self.items.get(index).is_some_and(|item| !item.disabled)
    }

    /// The local-space box of item `index`.
    fn item_rect(index: usize) -> Rect {
        Rect::from_origin_size(
            Point::new(0.0, index as f64 * (RADIO_SIZE + RADIO_GAP)),
            Size::new(RADIO_SIZE, RADIO_SIZE),
        )
    }

    /// The item under a widget-local `pos`, if any.
    fn hit_item(&self, pos: Point) -> Option<usize> {
        (0..self.items.len()).find(|index| Self::item_rect(*index).contains(pos))
    }

    /// Report item `index`'s value (never assigning it to `self.value`).
    fn request_value(&mut self, ctx: &mut EventCtx, index: usize) {
        if let Some(item) = self.items.get(index) {
            let value = item.value.clone();
            (self.on_value_change)(ctx, value);
        }
    }

    /// The next enabled item `step` places from `from`, wrapping, or `None` when
    /// no item is enabled.
    fn step_enabled(&self, from: usize, step: isize) -> Option<usize> {
        let len = self.items.len();
        if len == 0 {
            return None;
        }
        let mut index = from;
        for _ in 0..len {
            index = ((index as isize + step).rem_euclid(len as isize)) as usize;
            if self.enabled(index) {
                return Some(index);
            }
        }
        None
    }

    /// The cursor for item `index` (or for the group's disabled state).
    fn cursor(&self, index: usize) -> CursorIcon {
        if self.enabled(index) {
            style::ACTIVE_CURSOR
        } else {
            style::DISABLED_CURSOR
        }
    }
}

impl Widget for RadioGroupWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let count = self.items.len();
        let height = if count == 0 {
            0.0
        } else {
            count as f64 * RADIO_SIZE + (count - 1) as f64 * RADIO_GAP
        };
        // Shrink-wrapped: the items are `size-4 shrink-0`, and the app composes
        // each visible label beside the group rather than inside it.
        bc.constrain(Size::new(RADIO_SIZE, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let colors = resolve_colors(theme);
        let group_focused = ctx.has_focus();
        let ring_color = style::ring_color(None, theme);
        let selected = selected_index(&self.items, &self.value);
        let origin = ctx.origin();
        let item_size = Size::new(RADIO_SIZE, RADIO_SIZE);
        // A circle is a rounded rect at half its edge — no arc path needed, and it
        // keeps every fill on one `PaintScene` call.
        let circle_radius = RADIO_SIZE / 2.0;

        for index in 0..self.items.len() {
            let dimmed = !self.enabled(index);
            let tint = |color: Color| style::disabled_tint(color, dimmed);
            let rect = Self::item_rect(index);
            let item_origin = Point::new(origin.x + rect.x0, origin.y + rect.y0);
            let focused = group_focused && index == self.focused_item;

            style::draw_shadow(
                scene,
                item_origin,
                item_size,
                circle_radius,
                style::SHADOW_XS,
                theme,
            );
            if let Some(fill) = colors.fill {
                scene.fill_rounded_rect(item_origin, item_size, circle_radius, tint(fill));
            }

            let inset = style::BORDER_WIDTH / 2.0;
            let outline = RoundedRect::from_rect(
                Rect::from_origin_size(Point::ORIGIN, item_size).inset(-inset),
                circle_radius + inset,
            );
            scene.stroke_path(
                item_origin,
                &Shape::to_path(&outline, PATH_TOLERANCE),
                style::BORDER_WIDTH,
                &Brush::Solid(tint(style::focus_border(colors.ring, focused, theme))),
            );

            if selected == Some(index) {
                let dot_inset = (RADIO_SIZE - RADIO_DOT_SIZE) / 2.0;
                scene.fill_rounded_rect(
                    Point::new(item_origin.x + dot_inset, item_origin.y + dot_inset),
                    Size::new(RADIO_DOT_SIZE, RADIO_DOT_SIZE),
                    RADIO_DOT_SIZE / 2.0,
                    tint(colors.dot),
                );
            }

            if focused {
                style::draw_focus_ring(scene, item_origin, item_size, circle_radius, ring_color);
            }
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        match event {
            InputEvent::Key(key) => {
                if self.disabled {
                    return EventResult::Ignored;
                }
                if let Some(step) = arrow_step(key) {
                    // Arrow keys select as they move (the WAI-ARIA radio-group
                    // pattern): the roving focus is internal state and moves now,
                    // the selection is only *reported*.
                    let Some(next) = self.step_enabled(self.focused_item, step) else {
                        return EventResult::Ignored;
                    };
                    self.focused_item = next;
                    ctx.request_redraw();
                    self.request_value(ctx, next);
                    return EventResult::Handled;
                }
                if is_activation_key(key) && self.enabled(self.focused_item) {
                    self.request_value(ctx, self.focused_item);
                    return EventResult::Handled;
                }
                EventResult::Ignored
            }
            InputEvent::Pointer(p) => match p.phase {
                PointerPhase::Down => {
                    if !presses(p) {
                        return EventResult::Ignored;
                    }
                    let Some(index) = self.hit_item(p.position) else {
                        return EventResult::Ignored;
                    };
                    if !self.enabled(index) {
                        return EventResult::Ignored;
                    }
                    self.captured = Some(index);
                    self.focused_item = index;
                    ctx.capture_pointer();
                    ctx.request_focus();
                    // The focus ring moving to this item is what earns the frame.
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Move => {
                    let Some(armed) = self.captured else {
                        // The hover/cursor pass: claim on every qualifying move
                        // and re-ask for the cursor, both being per-pass requests.
                        if let Some(index) = self.hit_item(p.position) {
                            ctx.claim_hover();
                            ctx.set_cursor(self.cursor(index));
                        }
                        return EventResult::Ignored;
                    };
                    // Captured: re-ask so the shape survives a drag outside the
                    // armed item.
                    ctx.set_cursor(self.cursor(armed));
                    EventResult::Handled
                }
                PointerPhase::Up => {
                    let Some(armed) = self.captured.take() else {
                        return EventResult::Ignored;
                    };
                    if Self::item_rect(armed).contains(p.position) {
                        self.request_value(ctx, armed);
                    }
                    EventResult::Handled
                }
                PointerPhase::Cancel => {
                    if self.captured.take().is_none() {
                        return EventResult::Ignored;
                    }
                    // Internal flags only — never state, never the callback.
                    EventResult::Handled
                }
            },
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let selected = selected_index(&self.items, &self.value);
        ctx.push_container(
            Role::RadioGroup,
            |node| {
                if self.disabled {
                    node.set_disabled();
                }
            },
            |ctx| {
                for (index, item) in self.items.iter().enumerate() {
                    ctx.push_node(Role::RadioButton, |node| {
                        if let Some(label) = &item.label {
                            node.set_label(label.as_str());
                        }
                        node.set_selected(selected == Some(index));
                        if self.enabled(index) {
                            node.add_action(Action::Click);
                        } else {
                            node.set_disabled();
                        }
                    });
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::FrameTime;
    use frust::authoring::{
        BezPath, EventOutcome, Modifiers, PointerButton, PointerEvent, SemanticsUpdate,
    };
    use std::any::Any;

    /// Records the fills, stroked paths and shadows the group emits, in paint
    /// order (per item: shadow, optional fill, ring, optional dot, optional focus
    /// ring).
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
    struct Picked {
        last: Option<String>,
        count: u32,
    }

    fn items() -> Vec<RadioGroupItem> {
        vec![
            radio_group_item("default").label("Default"),
            radio_group_item("comfortable").label("Comfortable"),
            radio_group_item("compact").label("Compact").disabled(true),
        ]
    }

    fn view(value: &str) -> RadioGroupView<Picked> {
        radio_group::<Picked, _>(value, items(), |s: &mut Picked, v: String| {
            s.last = Some(v);
            s.count += 1;
        })
    }

    fn widget(value: &str) -> RadioGroupWidget {
        let mut counter = 0u64;
        View::<Picked>::build(&view(value), &mut BuildCtx::new(&mut counter))
    }

    fn group_size(count: usize) -> Size {
        Size::new(
            RADIO_SIZE,
            count as f64 * RADIO_SIZE + (count - 1) as f64 * RADIO_GAP,
        )
    }

    fn paint(w: &mut RadioGroupWidget, theme: Option<&Theme>) -> Recorder {
        let mut rec = Recorder::default();
        let size = group_size(w.items.len());
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

    fn dispatch(w: &mut RadioGroupWidget, state: &mut Picked, event: &InputEvent) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, group_size(w.items.len()));
        w.event(&mut ctx, event)
    }

    /// The y-centre of item `index`.
    fn item_y(index: usize) -> f64 {
        index as f64 * (RADIO_SIZE + RADIO_GAP) + RADIO_SIZE / 2.0
    }

    // ---- Layout / paint ---------------------------------------------------

    #[test]
    fn layout_stacks_the_items_at_the_grid_gap() {
        let mut w = widget("default");
        let mut ctx = LayoutCtx::new();
        let size = w.layout(
            &mut ctx,
            &BoxConstraints::new(Size::ZERO, Size::new(400.0, 400.0)),
        );
        assert_eq!(size, group_size(3));
        assert_eq!(RadioGroupWidget::item_rect(1).y0, RADIO_SIZE + RADIO_GAP);
    }

    #[test]
    fn every_item_paints_a_ring_and_only_the_selected_one_a_dot() {
        let theme = crate::theme();
        let scheme = theme.scheme();
        let mut w = widget("comfortable");
        let rec = paint(&mut w, Some(&theme));

        assert_eq!(rec.strokes.len(), 3, "one ring per item, no focus ring");
        for (_, width, color) in &rec.strokes[..2] {
            assert_eq!(*width, style::BORDER_WIDTH);
            // The checked item's ring is `border-input` too — the source authors
            // no checked border override.
            assert_eq!(*color, scheme.outline_variant);
        }
        assert_eq!(rec.shadows.len(), 3, "`shadow-xs` per item");

        // Light mode paints no item fill, so the only rounded rect is the dot.
        assert_eq!(rec.rrects.len(), 1);
        let (origin, size, radius, color) = rec.rrects[0];
        assert_eq!(color, scheme.primary);
        assert_eq!(size, Size::new(RADIO_DOT_SIZE, RADIO_DOT_SIZE));
        assert_eq!(radius, RADIO_DOT_SIZE / 2.0);
        let dot_inset = (RADIO_SIZE - RADIO_DOT_SIZE) / 2.0;
        assert_eq!(
            origin,
            Point::new(dot_inset, RADIO_SIZE + RADIO_GAP + dot_inset),
            "the dot is centred in the second item"
        );
    }

    #[test]
    fn dark_mode_paints_the_input_wash_behind_every_item() {
        let theme = crate::theme().with_brightness(Brightness::Dark);
        let mut w = widget("");
        let rec = paint(&mut w, Some(&theme));
        assert_eq!(rec.rrects.len(), 3, "`dark:bg-input/30`, no dot selected");
        let input = theme.scheme().outline_variant;
        assert!(
            (rec.rrects[0].3.components[3] - input.components[3] * DARK_FILL_ALPHA).abs() < 1e-6
        );
    }

    #[test]
    fn a_disabled_item_paints_at_half_alpha_while_its_siblings_do_not() {
        let theme = crate::theme();
        let mut w = widget("default");
        let rec = paint(&mut w, Some(&theme));
        let enabled_alpha = rec.strokes[0].2.components[3];
        let disabled_alpha = rec.strokes[2].2.components[3];
        assert_eq!(disabled_alpha, enabled_alpha * style::DISABLED_OPACITY);
    }

    // ---- Interaction ------------------------------------------------------

    #[test]
    fn up_inside_reports_that_items_value_without_self_mutating() {
        let mut w = widget("default");
        let mut state = Picked::default();
        let y = item_y(1);
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, 8.0, y));
        assert_eq!(w.captured, Some(1));
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Up, 8.0, y));
        assert_eq!(state.last.as_deref(), Some("comfortable"));
        assert_eq!(w.value, "default", "the app owns `value`");
        assert_eq!(w.captured, None);
    }

    #[test]
    fn a_release_that_drifted_off_the_armed_item_never_fires() {
        let mut w = widget("default");
        let mut state = Picked::default();
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Down, 8.0, item_y(0)),
        );
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Up, 8.0, item_y(1)),
        );
        assert_eq!(state.count, 0);

        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Down, 8.0, item_y(0)),
        );
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Cancel, 8.0, item_y(0)),
        );
        assert_eq!(state.count, 0);
        assert_eq!(w.captured, None);
    }

    #[test]
    fn a_disabled_item_takes_no_press_and_the_gap_between_items_is_dead() {
        let mut w = widget("default");
        let mut state = Picked::default();
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Down, 8.0, item_y(2)),
        );
        assert_eq!(w.captured, None, "the third item is disabled");

        // The `gap-3` band between items belongs to nobody.
        let gap_y = RADIO_SIZE + RADIO_GAP / 2.0;
        assert_eq!(
            dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, 8.0, gap_y)),
            EventResult::Ignored
        );
        assert_eq!(state.count, 0);
    }

    #[test]
    fn arrow_keys_move_the_roving_focus_and_report_as_they_go() {
        let mut w = widget("default");
        let mut state = Picked::default();
        assert_eq!(w.focused_item, 0);

        dispatch(&mut w, &mut state, &key(Key::Named(NamedKey::ArrowDown)));
        assert_eq!(w.focused_item, 1);
        assert_eq!(state.last.as_deref(), Some("comfortable"));

        // The third item is disabled, so `next` wraps past it to the first.
        dispatch(&mut w, &mut state, &key(Key::Named(NamedKey::ArrowDown)));
        assert_eq!(w.focused_item, 0);
        assert_eq!(state.last.as_deref(), Some("default"));

        // ...and back up, wrapping the other way (still skipping the disabled).
        dispatch(&mut w, &mut state, &key(Key::Named(NamedKey::ArrowUp)));
        assert_eq!(w.focused_item, 1);
        // Left/right are accepted as well.
        dispatch(&mut w, &mut state, &key(Key::Named(NamedKey::ArrowRight)));
        assert_eq!(w.focused_item, 0);
        assert_eq!(w.value, "default", "no arrow key ever wrote `value`");
    }

    #[test]
    fn space_reports_the_focused_item_and_an_unrelated_key_is_ignored() {
        let mut w = widget("default");
        let mut state = Picked::default();
        w.focused_item = 1;
        dispatch(&mut w, &mut state, &key(Key::Character(" ".into())));
        assert_eq!(state.last.as_deref(), Some("comfortable"));
        dispatch(&mut w, &mut state, &key(Key::Named(NamedKey::Enter)));
        assert_eq!(state.count, 2);

        assert_eq!(
            dispatch(&mut w, &mut state, &key(Key::Named(NamedKey::Tab))),
            EventResult::Ignored
        );
        assert_eq!(state.count, 2);
    }

    #[test]
    fn a_disabled_group_is_inert_to_pointer_and_key() {
        let mut counter = 0u64;
        let disabled = view("default").disabled(true);
        let mut w = View::<Picked>::build(&disabled, &mut BuildCtx::new(&mut counter));
        let mut state = Picked::default();
        assert_eq!(
            dispatch(
                &mut w,
                &mut state,
                &pointer(PointerPhase::Down, 8.0, item_y(0))
            ),
            EventResult::Ignored
        );
        assert_eq!(
            dispatch(&mut w, &mut state, &key(Key::Named(NamedKey::ArrowDown))),
            EventResult::Ignored
        );
        assert_eq!(state.count, 0);
    }

    #[test]
    fn a_hover_move_claims_without_arming_or_repainting() {
        let mut w = widget("default");
        let mut state = Picked::default();
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, group_size(3));
        let result = w.event(&mut ctx, &pointer(PointerPhase::Move, 8.0, item_y(0)));
        assert_eq!(result, EventResult::Ignored);
        assert_eq!(w.captured, None);
        assert!(!ctx.needs_redraw(), "no hover chrome, so no frame is owed");
    }

    #[test]
    fn rebuild_adopts_the_confirmed_value_and_moves_the_roving_focus_to_it() {
        let mut counter = 0u64;
        let prev = view("default");
        let mut w = View::<Picked>::build(&prev, &mut BuildCtx::new(&mut counter));
        let next = view("comfortable");
        let flags = View::<Picked>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert_eq!(w.value, "comfortable");
        assert_eq!(w.focused_item, 1);
        assert!(flags.needs_paint());
    }

    #[test]
    fn a_shorter_item_list_clamps_the_roving_focus() {
        let mut counter = 0u64;
        let prev = view("default");
        let mut w = View::<Picked>::build(&prev, &mut BuildCtx::new(&mut counter));
        w.focused_item = 2;
        let next = radio_group::<Picked, _>(
            "default",
            vec![radio_group_item("default")],
            |_s: &mut Picked, _v: String| {},
        );
        View::<Picked>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert_eq!(w.focused_item, 0);
    }

    // ---- Root-driven: focus ring, cursor, semantics ------------------------

    /// One radio group under a real `RenderRoot` — focus, hover and the cursor
    /// are all recorded by the root's event pass, so nothing else can exercise
    /// them.
    struct Harness {
        root: frust_core::RenderRoot<Picked, RadioGroupView<Picked>>,
        state: Picked,
    }

    impl Harness {
        fn new() -> Self {
            let mut h = Harness {
                root: frust_core::RenderRoot::new(),
                state: Picked::default(),
            };
            h.root.set_theme(Box::new(crate::theme()));
            let mut logic = |_s: &mut Picked| view("default");
            h.root.rebuild(&mut logic, &mut h.state);
            h.root.layout(Size::new(200.0, 200.0));
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
    fn the_focus_ring_lands_on_the_pressed_item_only() {
        let mut h = Harness::new();
        assert_eq!(h.paint().strokes.len(), 3, "three rings, no focus ring");

        h.dispatch(&pointer(PointerPhase::Down, 8.0, item_y(1)));
        assert!(h.root.is_focus_active());
        let rec = h.paint();
        // Paint runs item by item, so the focused item's ring lands between its
        // own border stroke and the next item's: [i0 border, i1 border, i1 ring,
        // i2 border].
        assert_eq!(rec.strokes.len(), 4, "three borders + one focus ring");
        let ring = style::ring_color(None, Some(&crate::theme()));
        // The second item's border swapped to the ring color...
        assert_eq!(rec.strokes[1].2, ring);
        assert_ne!(rec.strokes[0].2, ring, "its siblings did not");
        // ...and its 3px ring sits outside that item's own box.
        let (bbox, width, color) = rec.strokes[2];
        assert_eq!(width, style::FOCUS_RING_WIDTH);
        assert_eq!(color.components[3], style::FOCUS_RING_OPACITY);
        let item_top = RADIO_SIZE + RADIO_GAP;
        assert!(bbox.y0 < item_top && bbox.y1 > item_top + RADIO_SIZE);
    }

    #[test]
    fn a_move_resolves_pointer_over_an_item_and_not_allowed_over_a_disabled_one() {
        let mut h = Harness::new();
        h.dispatch(&pointer(PointerPhase::Move, 8.0, item_y(0)));
        assert_eq!(h.root.cursor(), style::ACTIVE_CURSOR);
        h.dispatch(&pointer(PointerPhase::Move, 8.0, item_y(2)));
        assert_eq!(h.root.cursor(), style::DISABLED_CURSOR);
        // Over the gap, nobody asks.
        h.dispatch(&pointer(
            PointerPhase::Move,
            8.0,
            RADIO_SIZE + RADIO_GAP / 2.0,
        ));
        assert_eq!(h.root.cursor(), CursorIcon::Default);
    }

    #[test]
    fn semantics_yields_a_radio_group_of_radio_buttons_with_one_selected() {
        let h = Harness::new();
        let update = h.semantics();
        let group = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::RadioGroup)
            .expect("a Role::RadioGroup node");
        let buttons: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::RadioButton)
            .collect();
        assert_eq!(buttons.len(), 3);
        assert_eq!(group.1.children().len(), 3);
        assert_eq!(buttons[0].1.label(), Some("Default"));
        assert_eq!(buttons[0].1.is_selected(), Some(true));
        assert_eq!(buttons[1].1.is_selected(), Some(false));
        assert!(buttons[1].1.supports_action(Action::Click));
        assert!(buttons[2].1.is_disabled(), "the third item is disabled");
    }
}
