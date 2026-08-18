//! Ports shadcn/ui's **ToggleGroup** (`ToggleGroup` + `ToggleGroupItem`) from
//! `tmp/ui/apps/v4/registry/new-york-v4/ui/toggle-group.tsx` (rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved 2026-08-17).
//!
//! The items are `toggleVariants(...)`-styled, which is why the two axes are
//! [`crate::ToggleVariant`]/[`crate::ToggleSize`] — the same enums `toggle` owns,
//! carried here as group-level props exactly as the source carries them through
//! its React context.
//!
//! | class | here |
//! |---|---|
//! | root `flex w-fit items-center gap-[--spacing(var(--gap))] rounded-md` | a shrink-wrapped row at [`ToggleGroupView::spacing`] |
//! | root `data-[spacing=default]:data-[variant=outline]:shadow-xs` | one group-level `shadow-xs` when flush + outline |
//! | item `px-3` | [`TOGGLE_GROUP_PADDING_X`] (overriding the toggle's own `px-*`) |
//! | item `data-[spacing=0]:rounded-none first:rounded-l-md last:rounded-r-md` | flush items, clipped to the group's rounded box |
//! | item `data-[spacing=0]:data-[variant=outline]:border-l-0 first:border-l` | one outer border plus 1px dividers |
//! | item `data-[state=on]:bg-accent` + `hover:bg-muted`/`hover:bg-accent` | per-item fills, pressed winning over hover |
//! | `focus-visible:border-ring` + `ring-[3px] ring-ring/50` | the focused item's ring |
//!
//! # Flush items are a clip, not four corner radii
//!
//! `first:rounded-l-md last:rounded-r-md` asks for per-corner rounding, which
//! `PaintScene` has no primitive for. The port paints the same picture the other
//! way round: item fills go down as plain rectangles inside a rounded clip the
//! size of the whole group, so the outer corners round and the inner edges stay
//! square. The outline variant's shared borders likewise become **one** rounded
//! outer stroke plus a 1px divider between neighbours — which is exactly what
//! `border-l-0`/`first:border-l` renders.
//!
//! With a non-zero [`ToggleGroupView::spacing`] the source stops collapsing the
//! items (`data-[spacing=0]` no longer matches), so each item paints its own
//! `rounded-md` box, its own border and its own shadow, and the group paints none.
//!
//! # Selection: one mode enum, one callback shape
//!
//! [`ToggleGroupMode::Single`] and [`ToggleGroupMode::Multiple`] both report
//! through the same `on_value_change(state, Vec<String>)` — the **whole** new
//! selection, so an app never has to reconstruct it. Single mode reports a vec of
//! 0 or 1 values (pressing the selected item clears it, which is Radix's
//! deselect behavior); multiple mode reports the full set. The group is
//! **controlled**: it never writes its own `selected`.
//!
//! # Roving focus moves, it does not select
//!
//! Unlike `radio_group` (whose arrows select as they move, per WAI-ARIA's radio
//! pattern), a toggle group's arrows only move the roving focus — Radix wraps the
//! items in a `RovingFocusGroup` and leaves activation to `Space`/`Enter`. That
//! difference is deliberate here too.
//!
//! # Hover claims, and where the container rule lands
//!
//! The group paints its own items (no `ChildPod` per item — the shipped precedent
//! for a fixed item strip), so the "container claims hover *after* routing" rule
//! has nothing to order: there is no child to route to, and the group's own claim
//! from its uncaptured `Move` arm is the only claim in the pass. It still runs the
//! full claim/latch/self-correct contract, because the item chrome under the
//! pointer depends on it.

use std::rc::Rc;

use frust::Theme;
use frust::authoring::{
    Action, BoxConstraints, Brush, BuildCtx, ChangeFlags, Color, EventCtx, EventResult, InputEvent,
    Key, KeyEvent, LayoutCtx, NamedKey, PaintCtx, PaintScene, Point, PointerPhase, Rect, Role,
    RoundedRect, SemanticsCtx, Shape, Size, Toggled, View, Widget, erase_callback_arg,
    text::{FontWeight, TextStyle},
};

use crate::components::toggle::{ToggleSize, ToggleVariant};
use crate::hit::presses;
use crate::style::{self, PATH_TOLERANCE, precedence_fill, precedence_ink};
use crate::text::{LabelRun, SHAPING_INK};
use crate::tokens::ShadcnTokens;

/// Horizontal padding per item, in logical px (`px-3` — the group's own override
/// of whatever `px-*` the size axis would give a standalone toggle).
pub const TOGGLE_GROUP_PADDING_X: f64 = 12.0;

/// Unthemed fallback `--foreground` (resting ink).
const FALLBACK_FOREGROUND: Color = Color::from_rgb8(0x0A, 0x0A, 0x0A);
/// Unthemed fallback `--muted`.
const FALLBACK_MUTED: Color = Color::from_rgb8(0xF5, 0xF5, 0xF5);
/// Unthemed fallback `--muted-foreground`.
const FALLBACK_MUTED_FOREGROUND: Color = Color::from_rgb8(0x73, 0x73, 0x73);
/// Unthemed fallback `--accent`.
const FALLBACK_ACCENT: Color = Color::from_rgb8(0xF5, 0xF5, 0xF5);
/// Unthemed fallback `--accent-foreground`.
const FALLBACK_ACCENT_FOREGROUND: Color = Color::from_rgb8(0x17, 0x17, 0x17);
/// Unthemed fallback `--input` (the outline variant's border).
const FALLBACK_INPUT: Color = Color::from_rgb8(0xE5, 0xE5, 0xE5);

/// How many values a toggle group may hold at once (`type="single" | "multiple"`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToggleGroupMode {
    /// At most one value; pressing the selected item clears the selection.
    #[default]
    Single,
    /// Any number of values; each press toggles that item's membership.
    Multiple,
}

/// One item in a [`ToggleGroupView`]: the value it contributes plus its label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToggleGroupItem {
    value: String,
    label: String,
    disabled: bool,
}

/// Create an item labelled `label` that contributes `value` to the selection.
pub fn toggle_group_item(value: impl Into<String>, label: impl Into<String>) -> ToggleGroupItem {
    ToggleGroupItem {
        value: value.into(),
        label: label.into(),
        disabled: false,
    }
}

impl ToggleGroupItem {
    /// Disable this one item (50% opacity, inert, skipped by arrow traversal).
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// The value this item contributes.
    pub fn value(&self) -> &str {
        &self.value
    }

    /// This item's label.
    pub fn label(&self) -> &str {
        &self.label
    }
}

/// A view-held, typed selection callback (erased on build).
type OnValueChange<State> = Rc<dyn Fn(&mut State, Vec<String>)>;

/// A declarative shadcn toggle group. See the [module docs](self).
pub struct ToggleGroupView<State: 'static> {
    items: Vec<ToggleGroupItem>,
    selected: Vec<String>,
    mode: ToggleGroupMode,
    variant: ToggleVariant,
    size: ToggleSize,
    spacing: f64,
    disabled: bool,
    on_value_change: OnValueChange<State>,
}

/// Create a toggle group over `items` whose pressed items are those whose values
/// appear in `selected`, reporting the whole new selection through
/// `on_value_change` — a **controlled** component (see the [module docs](self)).
pub fn toggle_group<State: 'static, F: Fn(&mut State, Vec<String>) + 'static>(
    items: Vec<ToggleGroupItem>,
    selected: Vec<String>,
    on_value_change: F,
) -> ToggleGroupView<State> {
    ToggleGroupView {
        items,
        selected,
        mode: ToggleGroupMode::default(),
        variant: ToggleVariant::default(),
        size: ToggleSize::default(),
        spacing: 0.0,
        disabled: false,
        on_value_change: Rc::new(on_value_change),
    }
}

impl<State: 'static> ToggleGroupView<State> {
    /// Pick the selection mode (`type="single" | "multiple"`).
    pub fn mode(mut self, mode: ToggleGroupMode) -> Self {
        self.mode = mode;
        self
    }

    /// Pick the `variant` every item inherits (the source's context prop).
    pub fn variant(mut self, variant: ToggleVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Pick the `size` every item inherits (the source's context prop).
    pub fn size(mut self, size: ToggleSize) -> Self {
        self.size = size;
        self
    }

    /// Set the inter-item gap in Tailwind spacing steps (`spacing`, default `0`).
    ///
    /// `0` is the flush, button-group look; anything else un-collapses the items
    /// so each paints its own rounded box, border and shadow.
    pub fn spacing(mut self, steps: f64) -> Self {
        self.spacing = steps.max(0.0);
        self
    }

    /// Disable the whole group.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// The resolved item palette (identical in shape to `toggle`'s, since the items
/// are `toggleVariants`-styled).
struct GroupColors {
    on_fill: Color,
    on_ink: Color,
    hover_fill: Color,
    hover_ink: Color,
    ink: Color,
    border: Color,
}

/// Resolve the palette for `variant`, falling back to the `neutral` preset's
/// light values with no theme threaded.
fn resolve_colors(theme: Option<&Theme>, variant: ToggleVariant) -> GroupColors {
    let (accent, accent_ink, muted, muted_ink, ink, border) = match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            (
                scheme.primary_container,
                scheme.on_primary_container,
                scheme.surface_container_highest,
                scheme.on_surface_variant,
                scheme.on_surface,
                scheme.outline_variant,
            )
        }
        None => (
            FALLBACK_ACCENT,
            FALLBACK_ACCENT_FOREGROUND,
            FALLBACK_MUTED,
            FALLBACK_MUTED_FOREGROUND,
            FALLBACK_FOREGROUND,
            FALLBACK_INPUT,
        ),
    };
    let (hover_fill, hover_ink) = match variant {
        ToggleVariant::Default => (muted, muted_ink),
        ToggleVariant::Outline => (accent, accent_ink),
    };
    GroupColors {
        on_fill: accent,
        on_ink: accent_ink,
        hover_fill,
        hover_ink,
        ink,
        border,
    }
}

/// The label style: the theme's (Inter) family at `text-sm`/`font-medium`, or the
/// bundled sans stack unthemed.
fn label_style(theme: Option<&Theme>) -> TextStyle {
    let family = theme.map_or_else(crate::tokens::sans_family, |t| {
        t.type_scale.label_large.family.clone()
    });
    TextStyle {
        family,
        weight: FontWeight::MEDIUM,
        ..TextStyle::new(style::TEXT_SM as f32, SHAPING_INK)
    }
}

/// Whether `key` activates the focused item.
fn is_activation_key(key: &KeyEvent) -> bool {
    match &key.key {
        Key::Named(NamedKey::Enter) => true,
        Key::Character(text) => text == " ",
        _ => false,
    }
}

/// Which way an arrow key moves the roving focus, or `None`.
fn arrow_step(key: &KeyEvent) -> Option<isize> {
    match &key.key {
        Key::Named(NamedKey::ArrowRight | NamedKey::ArrowDown) => Some(1),
        Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowUp) => Some(-1),
        _ => None,
    }
}

/// One retained item: its value and label run plus the geometry layout resolved
/// for it.
struct ItemEntry {
    /// The value this item contributes to the selection.
    value: String,
    label: String,
    run: LabelRun,
    disabled: bool,
    /// Local x of the item's left edge (filled at layout).
    x: f64,
    /// The item's width incl. padding (filled at layout).
    width: f64,
}

impl<State: 'static> View<State> for ToggleGroupView<State> {
    type Element = ToggleGroupWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ToggleGroupWidget {
        ToggleGroupWidget {
            items: self.items.iter().map(entry).collect(),
            selected: self.selected.clone(),
            mode: self.mode,
            variant: self.variant,
            size: self.size,
            spacing: self.spacing,
            disabled: self.disabled,
            focused_item: 0,
            hovered: None,
            captured: None,
            on_value_change: erase_callback_arg(&self.on_value_change),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ToggleGroupWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_value_change = erase_callback_arg(&self.on_value_change);
        let mut flags = ChangeFlags::NONE;
        if prev.items != self.items {
            element.items = self.items.iter().map(entry).collect();
            element.captured = None;
            element.hovered = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.selected != self.selected {
            // The app is the source of truth: adopt the confirmed selection.
            element.selected = self.selected.clone();
            flags |= ChangeFlags::PAINT;
        }
        if prev.mode != self.mode {
            element.mode = self.mode;
        }
        if prev.variant != self.variant {
            element.variant = self.variant;
            flags |= ChangeFlags::PAINT;
        }
        if prev.size != self.size || prev.spacing != self.spacing {
            element.size = self.size;
            element.spacing = self.spacing;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.disabled != self.disabled {
            element.disabled = self.disabled;
            if self.disabled {
                element.captured = None;
                element.hovered = None;
            }
            flags |= ChangeFlags::PAINT;
        }
        element.focused_item = element
            .focused_item
            .min(element.items.len().saturating_sub(1));
        flags
    }
}

/// Build a retained entry from a declarative item.
fn entry(item: &ToggleGroupItem) -> ItemEntry {
    ItemEntry {
        value: item.value.clone(),
        label: item.label.clone(),
        run: LabelRun::new(item.label.clone()),
        disabled: item.disabled,
        x: 0.0,
        width: 0.0,
    }
}

/// The retained widget for a [`ToggleGroupView`].
pub struct ToggleGroupWidget {
    items: Vec<ItemEntry>,
    /// The app-confirmed selection (source of truth; adopted on `rebuild`).
    selected: Vec<String>,
    mode: ToggleGroupMode,
    variant: ToggleVariant,
    size: ToggleSize,
    spacing: f64,
    disabled: bool,
    /// The item the roving focus sits on (arrow keys move it; it never selects).
    focused_item: usize,
    /// The latched hovered item, self-corrected from `PaintCtx::is_hovered()`
    /// every paint.
    hovered: Option<usize>,
    /// The item a `Down` armed, cleared on `Up`/`Cancel`.
    captured: Option<usize>,
    on_value_change: frust::authoring::ErasedArgCallback<Vec<String>>,
}

impl ToggleGroupWidget {
    /// The inter-item gap in logical px.
    fn gap(&self) -> f64 {
        style::spacing(self.spacing)
    }

    /// Whether the items are painted flush (the source's `data-[spacing=0]`).
    fn flush(&self) -> bool {
        self.spacing == 0.0
    }

    /// Whether item `index` can be interacted with.
    fn enabled(&self, index: usize) -> bool {
        !self.disabled && self.items.get(index).is_some_and(|item| !item.disabled)
    }

    /// Whether item `index` is currently pressed.
    fn is_on(&self, index: usize, values: &[String]) -> bool {
        self.items
            .get(index)
            .is_some_and(|item| values.iter().any(|v| v == &item.value))
    }

    /// The item under a widget-local `pos`, if any.
    fn hit_item(&self, pos: Point, height: f64) -> Option<usize> {
        self.items.iter().position(|item| {
            Rect::from_origin_size(Point::new(item.x, 0.0), Size::new(item.width, height))
                .contains(pos)
        })
    }

    /// The next enabled item `step` places from `from`, wrapping.
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

    /// The selection pressing item `index` would produce, under the group's mode.
    fn next_selection(&self, index: usize) -> Vec<String> {
        let Some(item) = self.items.get(index) else {
            return self.selected.clone();
        };
        let value = item.value.clone();
        match self.mode {
            ToggleGroupMode::Single => {
                if self.selected.iter().any(|v| v == &value) {
                    // Radix's single-mode deselect: pressing the pressed item
                    // clears the group.
                    Vec::new()
                } else {
                    vec![value]
                }
            }
            ToggleGroupMode::Multiple => {
                let mut next: Vec<String> = self
                    .selected
                    .iter()
                    .filter(|v| *v != &value)
                    .cloned()
                    .collect();
                if next.len() == self.selected.len() {
                    next.push(value);
                }
                next
            }
        }
    }

    /// Report the selection pressing item `index` produces (never assigning it).
    fn request_press(&mut self, ctx: &mut EventCtx, index: usize) {
        let next = self.next_selection(index);
        (self.on_value_change)(ctx, next);
    }

    /// Set the latched hovered item, reporting whether it changed.
    fn set_hovered(&mut self, hovered: Option<usize>) -> bool {
        let changed = self.hovered != hovered;
        self.hovered = hovered;
        changed
    }
}

impl Widget for ToggleGroupWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let style = label_style(Theme::from_layout_ctx(ctx));
        let height = self.size.height();
        let gap = self.gap();
        let mut x = 0.0;
        for item in &mut self.items {
            let label = item.run.layout(ctx, &style);
            // `px-3` per item, floored at the size's own `min-w-*` (= its height),
            // so an icon-only item stays square.
            let width = (label.width + TOGGLE_GROUP_PADDING_X * 2.0).max(height);
            item.x = x;
            item.width = width;
            x += width + gap;
        }
        let total = if self.items.is_empty() { 0.0 } else { x - gap };
        // `w-fit`: the group is exactly as wide as its items.
        bc.constrain(Size::new(total, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // `PaintCtx::is_hovered` is authoritative for *whether* the pointer is on
        // this group's path; which item it is over is the latch's business, so a
        // group that is not hovered at all drops the latch.
        if !ctx.is_hovered() || self.disabled {
            self.hovered = None;
        }

        let theme = Theme::from_paint_ctx(ctx);
        let colors = resolve_colors(theme, self.variant);
        let radius = ShadcnTokens::resolve_radius(None, theme).md;
        let group_focused = ctx.has_focus();
        let origin = ctx.origin();
        let size = ctx.size();
        let outlined = self.variant == ToggleVariant::Outline;
        let flush = self.flush();
        let selected = self.selected.clone();

        // Flush + outline puts one `shadow-xs` under the whole group; spaced items
        // each carry their own.
        if outlined && flush {
            style::draw_shadow(scene, origin, size, radius, style::SHADOW_XS, theme);
        }

        // Flush item fills are square rects inside a group-sized rounded clip —
        // the port's translation of `first:rounded-l-md last:rounded-r-md`.
        if flush {
            scene.push_clip_rounded(origin, size, radius);
        }
        for (index, item) in self.items.iter().enumerate() {
            let dimmed = !self.enabled(index);
            let tint = |color: Color| style::disabled_tint(color, dimmed);
            let item_origin = Point::new(origin.x + item.x, origin.y);
            let item_size = Size::new(item.width, size.height);
            let on = self.is_on(index, &selected);
            let hovered = self.hovered == Some(index) && !dimmed;

            if outlined && !flush {
                style::draw_shadow(
                    scene,
                    item_origin,
                    item_size,
                    radius,
                    style::SHADOW_XS,
                    theme,
                );
            }
            // Pressed wins over hover (`toggle`'s documented precedence).
            let fill = precedence_fill(on, hovered, colors.on_fill, colors.hover_fill);
            if let Some(fill) = fill {
                if flush {
                    // Square inside the clip: the group's corners do the rounding.
                    scene.fill_rect(item_origin, item_size, tint(fill));
                } else {
                    scene.fill_rounded_rect(item_origin, item_size, radius, tint(fill));
                }
            }
        }
        if flush {
            scene.pop_clip();
        }

        // Borders: flush + outline is one rounded outer stroke plus a divider per
        // seam; spaced items each stroke their own box.
        if outlined {
            let inset = style::BORDER_WIDTH / 2.0;
            let border = |focused: bool| style::focus_border(colors.border, focused, theme);
            if flush {
                let outline = RoundedRect::from_rect(
                    Rect::from_origin_size(Point::ORIGIN, size).inset(-inset),
                    (radius - inset).max(0.0),
                );
                scene.stroke_path(
                    origin,
                    &Shape::to_path(&outline, PATH_TOLERANCE),
                    style::BORDER_WIDTH,
                    &Brush::Solid(style::disabled_tint(border(false), self.disabled)),
                );
                for item in self.items.iter().skip(1) {
                    scene.stroke_line(
                        Point::new(origin.x + item.x, origin.y),
                        Point::new(origin.x + item.x, origin.y + size.height),
                        style::BORDER_WIDTH,
                        style::disabled_tint(colors.border, self.disabled),
                    );
                }
            } else {
                for (index, item) in self.items.iter().enumerate() {
                    let dimmed = !self.enabled(index);
                    let outline = RoundedRect::from_rect(
                        Rect::from_origin_size(Point::ORIGIN, Size::new(item.width, size.height))
                            .inset(-inset),
                        (radius - inset).max(0.0),
                    );
                    scene.stroke_path(
                        Point::new(origin.x + item.x, origin.y),
                        &Shape::to_path(&outline, PATH_TOLERANCE),
                        style::BORDER_WIDTH,
                        &Brush::Solid(style::disabled_tint(border(false), dimmed)),
                    );
                }
            }
        }

        // Labels, then the focused item's ring on top.
        for (index, item) in self.items.iter().enumerate() {
            let dimmed = !self.enabled(index);
            let on = self.is_on(index, &selected);
            let hovered = self.hovered == Some(index) && !dimmed;
            let ink = precedence_ink(on, hovered, colors.on_ink, colors.hover_ink, colors.ink);
            let label = item.run.size();
            let label_origin = Point::new(
                origin.x + item.x + (item.width - label.width) / 2.0,
                origin.y + (size.height - label.height) / 2.0,
            );
            item.run
                .paint(label_origin, style::disabled_tint(ink, dimmed), scene);
        }

        if group_focused && let Some(item) = self.items.get(self.focused_item) {
            style::draw_focus_ring(
                scene,
                Point::new(origin.x + item.x, origin.y),
                Size::new(item.width, size.height),
                radius,
                style::ring_color(None, theme),
            );
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // `disabled:pointer-events-none` (inherited from `toggleVariants`): a
        // disabled group is not a pointer target at all.
        if self.disabled {
            return EventResult::Ignored;
        }
        let size = ctx.size();
        match event {
            InputEvent::Key(key) => {
                if let Some(step) = arrow_step(key) {
                    // Roving focus only — a toggle group's arrows never select.
                    let Some(next) = self.step_enabled(self.focused_item, step) else {
                        return EventResult::Ignored;
                    };
                    self.focused_item = next;
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                if is_activation_key(key) && self.enabled(self.focused_item) {
                    let index = self.focused_item;
                    self.request_press(ctx, index);
                    return EventResult::Handled;
                }
                EventResult::Ignored
            }
            InputEvent::Pointer(p) => match p.phase {
                PointerPhase::Down => {
                    if !presses(p) {
                        return EventResult::Ignored;
                    }
                    let Some(index) = self.hit_item(p.position, size.height) else {
                        return EventResult::Ignored;
                    };
                    if !self.enabled(index) {
                        return EventResult::Ignored;
                    }
                    self.captured = Some(index);
                    self.focused_item = index;
                    ctx.capture_pointer();
                    ctx.request_focus();
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Move => {
                    if self.captured.is_none() {
                        let over = self
                            .hit_item(p.position, size.height)
                            .filter(|index| self.enabled(*index));
                        if over.is_some() {
                            // Claim on every qualifying move; the claim is per-pass.
                            ctx.claim_hover();
                            ctx.set_cursor(style::ACTIVE_CURSOR);
                        }
                        if self.set_hovered(over) {
                            // The gated redraw is the frame the hover fill needs.
                            ctx.request_redraw();
                        }
                        return EventResult::Ignored;
                    }
                    // Captured: re-ask so the shape survives a drag off the item.
                    ctx.set_cursor(style::ACTIVE_CURSOR);
                    EventResult::Handled
                }
                PointerPhase::Up => {
                    let Some(armed) = self.captured.take() else {
                        return EventResult::Ignored;
                    };
                    let released_on = self.hit_item(p.position, size.height);
                    if released_on == Some(armed) {
                        self.request_press(ctx, armed);
                    }
                    EventResult::Handled
                }
                PointerPhase::Cancel => {
                    if self.captured.take().is_none() {
                        return EventResult::Ignored;
                    }
                    // Internal flags only.
                    EventResult::Handled
                }
            },
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let selected = self.selected.clone();
        ctx.push_container(
            Role::Group,
            |node| {
                if self.disabled {
                    node.set_disabled();
                }
            },
            |ctx| {
                for (index, item) in self.items.iter().enumerate() {
                    // ARIA's toggle button: `role="button"` + `aria-pressed`,
                    // which accesskit spells as a Button node carrying `Toggled`.
                    ctx.push_node(Role::Button, |node| {
                        node.set_label(item.label.as_str());
                        node.set_toggled(Toggled::from(self.is_on(index, &selected)));
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
    use frust::authoring::text::TextContext;
    use frust::authoring::{
        BezPath, EventOutcome, Modifiers, PointerButton, PointerEvent, SemanticsUpdate,
    };
    use frust::{CursorIcon, FrameTime};
    use std::any::Any;

    /// Records everything this widget paints, including the clip stack depth at
    /// each fill (so "square fills inside a rounded clip" is assertable).
    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color, usize)>,
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<(Rect, f64, Color)>,
        lines: Vec<(Point, Point, f64, Color)>,
        shadows: Vec<(Point, Size, f64, f64, Color)>,
        rounded_clips: Vec<(Point, Size, f64)>,
        clip_depth: usize,
        glyph_inks: Vec<Color>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.rects.push((o, s, c, self.clip_depth));
        }
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
        fn stroke_line(&mut self, p0: Point, p1: Point, width: f64, color: Color) {
            self.lines.push((p0, p1, width, color));
        }
        fn draw_shadow(&mut self, o: Point, s: Size, radius: f64, std_dev: f64, color: Color) {
            self.shadows.push((o, s, radius, std_dev, color));
        }
        fn push_clip_rounded(&mut self, origin: Point, size: Size, radius: f64) {
            self.rounded_clips.push((origin, size, radius));
            self.clip_depth += 1;
        }
        fn pop_clip(&mut self) {
            self.clip_depth = self.clip_depth.saturating_sub(1);
        }
        fn draw_glyph_run(&mut self, run: frust::authoring::scene::GlyphRun) {
            if let Brush::Solid(color) = run.brush {
                self.glyph_inks.push(color);
            }
        }
    }

    #[derive(Default)]
    struct Selection {
        last: Option<Vec<String>>,
        count: u32,
    }

    fn items() -> Vec<ToggleGroupItem> {
        vec![
            toggle_group_item("bold", "B"),
            toggle_group_item("italic", "I"),
            toggle_group_item("underline", "U").disabled(true),
        ]
    }

    fn view(selected: &[&str], mode: ToggleGroupMode) -> ToggleGroupView<Selection> {
        toggle_group::<Selection, _>(
            items(),
            selected.iter().map(|v| v.to_string()).collect(),
            |s: &mut Selection, v: Vec<String>| {
                s.last = Some(v);
                s.count += 1;
            },
        )
        .mode(mode)
    }

    /// Build + lay out a group, returning it with its resolved size.
    fn laid_out(
        view: ToggleGroupView<Selection>,
        theme: Option<&Theme>,
    ) -> (ToggleGroupWidget, Size) {
        let mut counter = 0u64;
        let mut w = View::<Selection>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut tcx = TextContext::new();
        let mut ctx =
            LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), theme.map(|t| t as &dyn Any));
        let size = w.layout(
            &mut ctx,
            &BoxConstraints::new(Size::ZERO, Size::new(600.0, 200.0)),
        );
        (w, size)
    }

    fn paint(w: &mut ToggleGroupWidget, size: Size, theme: Option<&Theme>) -> Recorder {
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
        w: &mut ToggleGroupWidget,
        state: &mut Selection,
        size: Size,
        event: &InputEvent,
    ) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, size);
        w.event(&mut ctx, event)
    }

    /// The x-centre of item `index`.
    fn item_x(w: &ToggleGroupWidget, index: usize) -> f64 {
        let item = &w.items[index];
        item.x + item.width / 2.0
    }

    // ---- Layout / paint ---------------------------------------------------

    #[test]
    fn layout_lays_the_items_flush_by_default_and_gapped_when_spaced() {
        let (flush, size) = laid_out(view(&[], ToggleGroupMode::Single), None);
        assert_eq!(size.height, style::HEIGHT_DEFAULT);
        assert_eq!(flush.gap(), 0.0);
        assert!(flush.flush());
        assert_eq!(
            flush.items[1].x, flush.items[0].width,
            "no gap between items"
        );
        let total: f64 = flush.items.iter().map(|i| i.width).sum();
        assert!((size.width - total).abs() < 1e-9, "`w-fit`");
        // Every item clears `min-w-9`.
        assert!(flush.items.iter().all(|i| i.width >= style::HEIGHT_DEFAULT));

        let (spaced, spaced_size) = laid_out(view(&[], ToggleGroupMode::Single).spacing(1.0), None);
        assert_eq!(spaced.gap(), style::SPACING_UNIT);
        assert!(!spaced.flush());
        assert_eq!(
            spaced.items[1].x,
            spaced.items[0].width + style::SPACING_UNIT
        );
        assert!(spaced_size.width > size.width);
    }

    #[test]
    fn flush_item_fills_are_square_rects_inside_one_rounded_group_clip() {
        let theme = crate::theme();
        let (mut w, size) = laid_out(view(&["bold"], ToggleGroupMode::Single), Some(&theme));
        let rec = paint(&mut w, size, Some(&theme));
        assert_eq!(rec.rounded_clips.len(), 1, "one group-sized rounded clip");
        let (clip_origin, clip_size, clip_radius) = rec.rounded_clips[0];
        assert_eq!((clip_origin, clip_size), (Point::ZERO, size));
        assert_eq!(clip_radius, crate::ShadcnRadius::shadcn().md);

        assert_eq!(rec.rects.len(), 1, "only the pressed item fills");
        let (origin, fill_size, color, depth) = rec.rects[0];
        assert_eq!(origin, Point::ZERO);
        assert_eq!(fill_size, Size::new(w.items[0].width, size.height));
        assert_eq!(color, theme.scheme().primary_container, "`bg-accent`");
        assert_eq!(depth, 1, "painted inside the clip");
        assert!(rec.rrects.is_empty(), "no per-item rounded box when flush");
        assert_eq!(rec.glyph_inks[0], theme.scheme().on_primary_container);
        assert_eq!(rec.glyph_inks[1], theme.scheme().on_surface, "resting ink");
    }

    #[test]
    fn a_flush_outline_group_paints_one_border_one_shadow_and_a_divider_per_seam() {
        let theme = crate::theme();
        let (mut w, size) = laid_out(
            view(&[], ToggleGroupMode::Single).variant(ToggleVariant::Outline),
            Some(&theme),
        );
        let rec = paint(&mut w, size, Some(&theme));
        assert_eq!(rec.shadows.len(), 1, "one group-level `shadow-xs`");
        assert_eq!(rec.shadows[0].1, size);
        assert_eq!(rec.strokes.len(), 1, "one outer border");
        assert_eq!(rec.strokes[0].2, theme.scheme().outline_variant);
        assert_eq!(rec.lines.len(), 2, "a divider between each pair");
        assert_eq!(rec.lines[0].0.x, w.items[1].x);
        assert_eq!(rec.lines[1].0.x, w.items[2].x);
    }

    #[test]
    fn a_spaced_outline_group_gives_every_item_its_own_box() {
        let theme = crate::theme();
        let (mut w, size) = laid_out(
            view(&[], ToggleGroupMode::Single)
                .variant(ToggleVariant::Outline)
                .spacing(2.0),
            Some(&theme),
        );
        let rec = paint(&mut w, size, Some(&theme));
        assert!(rec.rounded_clips.is_empty(), "no group clip when spaced");
        assert_eq!(rec.shadows.len(), 3, "one `shadow-xs` per item");
        assert_eq!(rec.strokes.len(), 3, "one border per item");
        assert!(rec.lines.is_empty(), "no dividers when spaced");
        // The disabled item's border is halved.
        assert_eq!(
            rec.strokes[2].2.components[3],
            rec.strokes[0].2.components[3] * style::DISABLED_OPACITY
        );
    }

    #[test]
    fn a_spaced_group_fills_a_pressed_item_as_its_own_rounded_box() {
        let (mut w, size) = laid_out(
            view(&["italic"], ToggleGroupMode::Single).spacing(2.0),
            None,
        );
        let rec = paint(&mut w, size, None);
        assert!(rec.rects.is_empty());
        assert_eq!(rec.rrects.len(), 1);
        assert_eq!(rec.rrects[0].0.x, w.items[1].x);
        assert_eq!(rec.rrects[0].3, FALLBACK_ACCENT);
    }

    // ---- Selection --------------------------------------------------------

    #[test]
    fn single_mode_replaces_the_selection_and_deselects_on_a_second_press() {
        let (mut w, size) = laid_out(view(&["bold"], ToggleGroupMode::Single), None);
        let mut state = Selection::default();
        let x = item_x(&w, 1);
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, x, 18.0),
        );
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Up, x, 18.0),
        );
        assert_eq!(state.last.as_deref(), Some(&["italic".to_string()][..]));
        assert_eq!(w.selected, vec!["bold".to_string()], "the app owns it");

        // Pressing the already-selected item clears the group.
        let x = item_x(&w, 0);
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, x, 18.0),
        );
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Up, x, 18.0),
        );
        assert_eq!(state.last.as_deref(), Some(&[][..]));
    }

    #[test]
    fn multiple_mode_adds_and_removes_membership() {
        let (mut w, size) = laid_out(view(&["bold"], ToggleGroupMode::Multiple), None);
        let mut state = Selection::default();
        let x = item_x(&w, 1);
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, x, 18.0),
        );
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Up, x, 18.0),
        );
        assert_eq!(
            state.last.clone().unwrap(),
            vec!["bold".to_string(), "italic".to_string()]
        );

        let x = item_x(&w, 0);
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, x, 18.0),
        );
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Up, x, 18.0),
        );
        assert_eq!(state.last.clone().unwrap(), Vec::<String>::new());
    }

    #[test]
    fn a_release_on_another_item_or_a_cancel_never_fires() {
        let (mut w, size) = laid_out(view(&[], ToggleGroupMode::Single), None);
        let mut state = Selection::default();
        let (first, second) = (item_x(&w, 0), item_x(&w, 1));
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, first, 18.0),
        );
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Up, second, 18.0),
        );
        assert_eq!(state.count, 0);

        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, first, 18.0),
        );
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Cancel, first, 18.0),
        );
        assert_eq!(state.count, 0);
        assert_eq!(w.captured, None);
    }

    #[test]
    fn a_disabled_item_and_a_disabled_group_are_both_inert() {
        let (mut w, size) = laid_out(view(&[], ToggleGroupMode::Single), None);
        let mut state = Selection::default();
        let third = item_x(&w, 2);
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, third, 18.0),
        );
        assert_eq!(w.captured, None, "the third item is disabled");

        let (mut disabled, size) =
            laid_out(view(&[], ToggleGroupMode::Single).disabled(true), None);
        for event in [
            pointer(PointerPhase::Down, 10.0, 18.0),
            pointer(PointerPhase::Move, 10.0, 18.0),
            key(Key::Character(" ".into())),
        ] {
            assert_eq!(
                dispatch(&mut disabled, &mut state, size, &event),
                EventResult::Ignored
            );
        }
        assert_eq!(state.count, 0);
    }

    // ---- Keyboard ---------------------------------------------------------

    #[test]
    fn arrow_keys_move_the_roving_focus_without_selecting() {
        let (mut w, size) = laid_out(view(&[], ToggleGroupMode::Single), None);
        let mut state = Selection::default();
        assert_eq!(w.focused_item, 0);
        dispatch(
            &mut w,
            &mut state,
            size,
            &key(Key::Named(NamedKey::ArrowRight)),
        );
        assert_eq!(w.focused_item, 1);
        assert_eq!(state.count, 0, "moving focus is not selecting");
        // Wraps past the disabled third item back to the first.
        dispatch(
            &mut w,
            &mut state,
            size,
            &key(Key::Named(NamedKey::ArrowRight)),
        );
        assert_eq!(w.focused_item, 0);
        dispatch(
            &mut w,
            &mut state,
            size,
            &key(Key::Named(NamedKey::ArrowLeft)),
        );
        assert_eq!(w.focused_item, 1);
    }

    #[test]
    fn space_presses_the_focused_item() {
        let (mut w, size) = laid_out(view(&[], ToggleGroupMode::Multiple), None);
        let mut state = Selection::default();
        w.focused_item = 1;
        dispatch(&mut w, &mut state, size, &key(Key::Character(" ".into())));
        assert_eq!(state.last.clone().unwrap(), vec!["italic".to_string()]);
        assert_eq!(
            dispatch(&mut w, &mut state, size, &key(Key::Named(NamedKey::Tab))),
            EventResult::Ignored
        );
        assert_eq!(state.count, 1);
    }

    // ---- Hover ------------------------------------------------------------

    #[test]
    fn the_hover_latch_tracks_the_item_under_the_pointer() {
        let (mut w, size) = laid_out(view(&[], ToggleGroupMode::Single), None);
        let mut state = Selection::default();

        let state_any: &mut dyn Any = &mut state;
        let mut enter = EventCtx::new(state_any, Point::ZERO, size);
        w.event(
            &mut enter,
            &pointer(PointerPhase::Move, item_x(&w, 0), 18.0),
        );
        assert_eq!(w.hovered, Some(0));
        assert!(enter.needs_redraw());

        let state_any: &mut dyn Any = &mut state;
        let mut across = EventCtx::new(state_any, Point::ZERO, size);
        w.event(
            &mut across,
            &pointer(PointerPhase::Move, item_x(&w, 1), 18.0),
        );
        assert_eq!(w.hovered, Some(1));
        assert!(across.needs_redraw(), "the link moved between items");

        let state_any: &mut dyn Any = &mut state;
        let mut disabled_item = EventCtx::new(state_any, Point::ZERO, size);
        w.event(
            &mut disabled_item,
            &pointer(PointerPhase::Move, item_x(&w, 2), 18.0),
        );
        assert_eq!(w.hovered, None, "a disabled item takes no hover");
    }

    #[test]
    fn paint_drops_a_stale_hover_latch() {
        let (mut w, size) = laid_out(view(&[], ToggleGroupMode::Single), None);
        w.hovered = Some(1);
        // A bare `PaintCtx` reports no hover link at all.
        let _ = paint(&mut w, size, None);
        assert_eq!(w.hovered, None);
    }

    #[test]
    fn rebuild_adopts_the_confirmed_selection_and_clamps_the_roving_focus() {
        let mut counter = 0u64;
        let prev = view(&[], ToggleGroupMode::Single);
        let mut w = View::<Selection>::build(&prev, &mut BuildCtx::new(&mut counter));
        w.focused_item = 2;
        let next = view(&["italic"], ToggleGroupMode::Single);
        let flags =
            View::<Selection>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert_eq!(w.selected, vec!["italic".to_string()]);
        assert!(flags.needs_paint());

        let shorter = toggle_group::<Selection, _>(
            vec![toggle_group_item("bold", "B")],
            Vec::new(),
            |_s: &mut Selection, _v: Vec<String>| {},
        );
        View::<Selection>::rebuild(&shorter, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert_eq!(w.focused_item, 0);
    }

    // ---- Root-driven: focus ring, cursor, semantics ------------------------

    struct Harness {
        root: frust_core::RenderRoot<Selection, ToggleGroupView<Selection>>,
        state: Selection,
        tcx: TextContext,
    }

    impl Harness {
        fn new(disabled: bool) -> Self {
            let mut h = Harness {
                root: frust_core::RenderRoot::new(),
                state: Selection::default(),
                tcx: TextContext::new(),
            };
            h.root.set_theme(Box::new(crate::theme()));
            let mut logic = move |_s: &mut Selection| {
                view(&["bold"], ToggleGroupMode::Single).disabled(disabled)
            };
            h.root.rebuild(&mut logic, &mut h.state);
            h.root
                .layout_with_text(Size::new(400.0, 200.0), &mut h.tcx as &mut dyn Any);
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
    fn hovering_an_item_paints_its_muted_fill_on_the_frame_the_latch_asks_for() {
        let mut h = Harness::new(false);
        assert_eq!(h.paint().rects.len(), 1, "only the pressed item at rest");

        // The second item sits past the first, which is at least `min-w-9` wide.
        let gain = h.dispatch(&pointer(
            PointerPhase::Move,
            style::HEIGHT_DEFAULT + 4.0,
            18.0,
        ));
        assert!(gain.needs_redraw);
        let rec = h.paint();
        assert_eq!(rec.rects.len(), 2, "pressed + hovered");
        assert_eq!(
            rec.rects[1].2,
            crate::theme().scheme().surface_container_highest,
            "`hover:bg-muted`"
        );
    }

    #[test]
    fn the_focus_ring_lands_on_the_pressed_item_and_the_cursor_resolves() {
        let mut h = Harness::new(false);
        assert!(h.paint().strokes.is_empty(), "default variant, no borders");

        h.dispatch(&pointer(
            PointerPhase::Down,
            style::HEIGHT_DEFAULT + 4.0,
            18.0,
        ));
        assert!(h.root.is_focus_active());
        let rec = h.paint();
        assert_eq!(rec.strokes.len(), 1, "just the focus ring");
        let (bbox, width, color) = rec.strokes[0];
        assert_eq!(width, style::FOCUS_RING_WIDTH);
        assert_eq!(color.components[3], style::FOCUS_RING_OPACITY);
        assert!(bbox.x0 > 0.0, "the ring is around the second item");

        h.dispatch(&pointer(PointerPhase::Move, 10.0, 18.0));
        assert_eq!(h.root.cursor(), style::ACTIVE_CURSOR);

        let mut disabled = Harness::new(true);
        disabled.dispatch(&pointer(PointerPhase::Move, 10.0, 18.0));
        assert_eq!(
            disabled.root.cursor(),
            CursorIcon::Default,
            "`pointer-events-none` asks for no shape"
        );
    }

    #[test]
    fn semantics_yields_a_group_of_toggle_buttons() {
        let h = Harness::new(false);
        let update = h.semantics();
        let group = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Group)
            .expect("a Role::Group node");
        let buttons: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::Button)
            .collect();
        assert_eq!(buttons.len(), 3);
        assert_eq!(group.1.children().len(), 3);
        assert_eq!(buttons[0].1.label(), Some("B"));
        assert_eq!(buttons[0].1.toggled(), Some(Toggled::True));
        assert_eq!(buttons[1].1.toggled(), Some(Toggled::False));
        assert!(buttons[2].1.is_disabled());
    }
}
