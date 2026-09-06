//! Ports beUI's `expandable-control` component —
//! `components/motion/expandable-control.tsx` (beUI rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01), both of
//! the two controls it exports:
//!
//! - [`expandable_button`] — an icon-only pill that widens to reveal its label.
//! - [`expandable_chip`] — a label pill that widens to reveal a trailing action.
//!
//! Both are "layout continuity" surfaces: the control keeps its identity while
//! its footprint changes, so the reveal is a **width morph**, not a swap.
//!
//! # The morph runs through layout, one frame behind
//!
//! The animated quantity here is the control's own width, which `layout`
//! decides and `paint` cannot. The spring is therefore advanced during paint —
//! the only pass with a clock — and the widget calls `request_layout` while it
//! runs, so the next layout reads the value the last paint produced. The morph
//! is one frame behind the ramp it is driven by; nothing else about it differs
//! from a paint-only animation, and the alternative (a clock in `layout`) is not
//! something the framework offers.
//!
//! # Expansion is controlled, like every other value in the framework
//!
//! Upstream supports both a controlled `expanded` prop and an uncontrolled
//! `defaultExpanded`. frust's own convention is that a control never mutates the
//! value it is given — it reports the requested one and waits for the rebuild
//! that confirms it — so only the controlled form is ported: pass `expanded` and
//! handle `on_expanded_change`. A caller wanting the uncontrolled behaviour
//! keeps the flag in its own component state, which is a two-line `build`.
//!
//! # Departures from the source
//!
//! - **The morph spring is [`SPRING_LAYOUT`], not upstream's inline one.**
//!   `expandable-control.tsx` authors a `CONTINUITY_SPRING` of its own
//!   (stiffness 220, damping 17, mass 0.85) that appears in no token table;
//!   `SPRING_LAYOUT` (360/32/0.6) is the catalog's published shared-layout
//!   glide, which is what this morph *is*. The port is therefore slightly
//!   quicker and slightly tighter than the source.
//! - **No blur.** Upstream's label fades through `blur(4px)`; frust has no blur
//!   primitive, so the reveal is opacity alone.
//! - **The icon slots take a short string, not an arbitrary node.** Upstream
//!   takes a React node (a Lucide glyph). This catalog has no icon vocabulary
//!   and the crate is facade-only, so the slot renders a caller-supplied short
//!   string — a symbol, an emoji, or an icon-font codepoint — centred in the
//!   slot the source reserves.

use std::rc::Rc;
use std::time::Duration;

use frust::Theme;
use frust::authoring::{
    Action, Affine, BoxConstraints, BuildCtx, ChangeFlags, Color, ErasedArgCallback, EventCtx,
    EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, PointerPhase, Role,
    SemanticsCtx, Size, TypedArgCallback, Vec2, View, Widget, erase_callback_arg,
};

use crate::motion::Ramp;
use crate::style::{
    ACTIVE_CURSOR, DISABLED_OPACITY, HEIGHT_INPUT, HEIGHT_MD, HOVER_WASH_ALPHA, RADIUS_CONTROL,
    TEXT_SM, disabled_tint, resolve_radius, with_alpha,
};
use crate::tokens::color_scheme_light;
use crate::tokens::motion::{EASE_OUT, SPRING_LAYOUT, SPRING_PRESS};

use crate::press::{SpringScalar, inside, presses, stroke_outline};
use crate::text::{LabelRun, label_style};

/// The expandable button's height: `h-11`, which is also its collapsed width
/// (`min-w-11`).
const BUTTON_HEIGHT: f64 = HEIGHT_INPUT;

/// The chip's height: `h-10`.
const CHIP_HEIGHT: f64 = HEIGHT_MD;

/// The button's inner padding: `p-1`.
const BUTTON_PAD: f64 = 4.0;

/// The button's icon slot: `size-9`.
const ICON_SLOT: f64 = 36.0;

/// The gutter after the revealed label: `pr-3`.
const LABEL_GUTTER: f64 = 12.0;

/// The chip's leading gutter: `pl-3`.
const CHIP_LEAD: f64 = 12.0;

/// The chip's trailing gutter, collapsed (`pr-3`) and expanded (`pr-1`).
const CHIP_TAIL: f64 = 12.0;
const CHIP_TAIL_EXPANDED: f64 = 4.0;

/// The chip's action slot: `w-8`.
const CHIP_ACTION_SLOT: f64 = 32.0;

/// The scale a pressed control shrinks to: `whileTap={{ scale: 0.97 }}`.
const PRESS_SCALE: f64 = 0.97;

/// How long the revealed content fades in and out. Upstream ties the label's
/// opacity to the same continuity spring the width uses; a short eased fade is
/// the port's equivalent, and it is what keeps a half-open control from showing
/// a clipped word at full strength.
const REVEAL_FADE: Duration = Duration::from_millis(180);

/// The two controls' resolved roles — a bordered, transparent pill either way.
#[derive(Clone, Copy)]
struct ControlPaint {
    border: Color,
    ink: Color,
    muted_ink: Color,
    wash: Color,
}

impl ControlPaint {
    fn resolve(theme: Option<&Theme>) -> Self {
        let scheme = theme.map(Theme::scheme);
        macro_rules! role {
            ($role:ident) => {
                scheme.map_or(color_scheme_light().$role, |s| s.$role)
            };
        }
        Self {
            border: role!(outline_variant),
            ink: role!(on_surface),
            muted_ink: role!(on_surface_variant),
            wash: with_alpha(role!(primary), HOVER_WASH_ALPHA),
        }
    }
}

// =============================================================================
// Expandable button
// =============================================================================

/// A declarative icon pill that widens to reveal its label.
pub struct ExpandableButtonView<State: 'static> {
    icon: String,
    label: String,
    expanded: bool,
    disabled: bool,
    on_expanded_change: TypedArgCallback<State, bool>,
}

/// An icon-only pill showing `icon`, which widens to reveal `label` while
/// `expanded`.
///
/// Controlled: a press reports the flag it wants through `on_expanded_change`
/// and paints whatever the next rebuild feeds back through
/// [`expanded`](ExpandableButtonView::expanded).
pub fn expandable_button<State: 'static>(
    icon: impl Into<String>,
    label: impl Into<String>,
    on_expanded_change: impl Fn(&mut State, bool) + 'static,
) -> ExpandableButtonView<State> {
    ExpandableButtonView {
        icon: icon.into(),
        label: label.into(),
        expanded: false,
        disabled: false,
        on_expanded_change: Rc::new(on_expanded_change),
    }
}

impl<State: 'static> ExpandableButtonView<State> {
    /// Whether the label is revealed (default `false`).
    pub fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = expanded;
        self
    }

    /// Disable the control: no toggle, no claims, everything dimmed.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// The retained widget for an [`ExpandableButtonView`].
pub struct ExpandableButtonWidget {
    icon: LabelRun,
    label: LabelRun,
    expanded: bool,
    disabled: bool,
    on_expanded_change: ErasedArgCallback<bool>,
    hovered: bool,
    pressed: bool,
    captured: bool,
    /// The control's own width, on `SPRING_LAYOUT` — see the [module docs](self)
    /// for why it is advanced at paint and read at layout.
    width: SpringScalar,
    reveal: SpringScalar,
    scale: SpringScalar,
    icon_size: Size,
    label_size: Size,
}

impl ExpandableButtonWidget {
    /// The width this control settles at in each state.
    fn width_for(&self, expanded: bool) -> f64 {
        let collapsed = BUTTON_PAD * 2.0 + ICON_SLOT;
        if expanded {
            collapsed + self.label_size.width + LABEL_GUTTER
        } else {
            collapsed
        }
    }
}

impl<State: 'static> View<State> for ExpandableButtonView<State> {
    type Element = ExpandableButtonWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ExpandableButtonWidget {
        ExpandableButtonWidget {
            icon: LabelRun::new(self.icon.clone()),
            label: LabelRun::new(self.label.clone()),
            expanded: self.expanded,
            disabled: self.disabled,
            on_expanded_change: erase_callback_arg(&self.on_expanded_change),
            hovered: false,
            pressed: false,
            captured: false,
            width: SpringScalar::new(f64::NAN, Ramp::spring(SPRING_LAYOUT)),
            reveal: SpringScalar::new(
                if self.expanded { 1.0 } else { 0.0 },
                Ramp::eased(REVEAL_FADE, EASE_OUT),
            ),
            scale: SpringScalar::new(1.0, Ramp::spring(SPRING_PRESS)),
            icon_size: Size::ZERO,
            label_size: Size::ZERO,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ExpandableButtonWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_expanded_change = erase_callback_arg(&self.on_expanded_change);
        let mut flags = ChangeFlags::NONE;
        if prev.icon != self.icon {
            element.icon.set_content(self.icon.clone());
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.label != self.label {
            element.label.set_content(self.label.clone());
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.expanded != self.expanded {
            element.expanded = self.expanded;
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

impl Widget for ExpandableButtonWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let style = label_style(TEXT_SM);
        self.icon_size = self.icon.layout(ctx, &style);
        self.label_size = self.label.layout(ctx, &style);
        // The first layout lands on the resting width rather than animating out
        // of an arbitrary starting value.
        if self.width.value().is_nan() {
            self.width.jump_to(self.width_for(self.expanded));
        }
        bc.constrain(Size::new(self.width.value(), BUTTON_HEIGHT))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let paint = ControlPaint::resolve(theme);
        let now = ctx.frame_time();
        let size = ctx.size();
        let origin = ctx.origin();
        let dim = self.disabled;
        if !dim && !self.captured {
            self.hovered = ctx.is_hovered();
        }

        let target_width = self.width_for(self.expanded);
        let target_reveal = if self.expanded { 1.0 } else { 0.0 };
        let width_before = self.width.value();
        if reduce {
            self.width.jump_to(target_width);
            self.reveal.jump_to(target_reveal);
            self.scale.jump_to(1.0);
        } else {
            self.width.set_target(target_width);
            self.width.advance(now);
            self.reveal.set_target(target_reveal);
            self.reveal.advance(now);
            self.scale.set_target(if self.pressed && !dim {
                PRESS_SCALE
            } else {
                1.0
            });
            self.scale.advance(now);
        }
        let reveal = self.reveal.value();
        let scale = self.scale.value();

        let centre = origin + Vec2::new(size.width / 2.0, size.height / 2.0);
        if scale != 1.0 {
            scene.push_transform(
                Affine::translate(centre.to_vec2())
                    * Affine::scale(scale)
                    * Affine::translate(-centre.to_vec2()),
            );
        }

        let radius = resolve_radius(RADIUS_CONTROL, size.width, size.height);
        if self.hovered && !dim {
            scene.fill_rounded_rect(origin, size, radius, paint.wash);
        }
        stroke_outline(
            scene,
            origin,
            size,
            radius,
            disabled_tint(paint.border, dim, DISABLED_OPACITY),
        );

        let ink = disabled_tint(paint.ink, dim, DISABLED_OPACITY);
        let icon_at = Point::new(
            origin.x + BUTTON_PAD + (ICON_SLOT - self.icon_size.width) / 2.0,
            origin.y + (size.height - self.icon_size.height) / 2.0,
        );
        self.icon.paint(icon_at, ink, scene);

        // The label is clipped to whatever width the morph has reached, so a
        // half-open control shows a half-word rather than overflowing its pill.
        if reveal > 0.0 {
            let label_at = Point::new(
                origin.x + BUTTON_PAD + ICON_SLOT,
                origin.y + (size.height - self.label_size.height) / 2.0,
            );
            scene.push_clip_rounded(origin, size, radius);
            scene.push_layer(label_at, self.label_size, reveal.clamp(0.0, 1.0) as f32);
            self.label.paint(label_at, ink, scene);
            scene.pop_layer();
            scene.pop_clip();
        }

        if scale != 1.0 {
            scene.pop_transform();
        }

        // A layout-affecting animation still owes a relayout on the frame it
        // lands: `SpringScalar::advance` clears `is_animating` the instant it
        // settles (inside the same call that produces the final value), and
        // `jump_to` under `reduce_motion` never sets it at all — so the flag
        // alone misses both the settle frame and the reduced-motion snap.
        // Comparing against the pre-advance value catches whichever of those
        // moved the width this frame.
        if self.width.is_animating() || self.width.value() != width_before {
            ctx.request_layout();
        } else if self.reveal.is_animating() || self.scale.is_animating() {
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
                if inside(p.position, size) {
                    // Controlled: report the flag wanted, never flip it here.
                    (self.on_expanded_change)(ctx, !self.expanded);
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
            node.set_label(self.label.content());
            node.set_expanded(self.expanded);
            if self.disabled {
                node.set_disabled();
            } else {
                node.add_action(Action::Click);
            }
        });
    }
}

// =============================================================================
// Expandable chip
// =============================================================================

/// A declarative label pill that widens to reveal a trailing action.
pub struct ExpandableChipView<State: 'static> {
    label: String,
    action_icon: String,
    action_label: String,
    expanded: bool,
    disabled: bool,
    collapse_on_action: bool,
    on_expanded_change: TypedArgCallback<State, bool>,
    on_action: TypedArgCallback<State, bool>,
}

/// A chip labelled `label` whose trailing action — drawn as `action_icon` and
/// named `action_label` for a screen reader — is revealed while `expanded`.
///
/// Controlled, like [`expandable_button`]: pressing the label reports the flag
/// it wants through [`on_expanded_change`](ExpandableChipView::on_expanded_change).
pub fn expandable_chip<State: 'static>(
    label: impl Into<String>,
    action_icon: impl Into<String>,
    action_label: impl Into<String>,
) -> ExpandableChipView<State> {
    ExpandableChipView {
        label: label.into(),
        action_icon: action_icon.into(),
        action_label: action_label.into(),
        expanded: false,
        disabled: false,
        collapse_on_action: true,
        on_expanded_change: Rc::new(|_, _| {}),
        on_action: Rc::new(|_, _| {}),
    }
}

impl<State: 'static> ExpandableChipView<State> {
    /// Whether the action is revealed (default `false`).
    pub fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = expanded;
        self
    }

    /// Handle a press on the label: the argument is the expansion the control
    /// is asking for.
    pub fn on_expanded_change(mut self, handler: impl Fn(&mut State, bool) + 'static) -> Self {
        self.on_expanded_change = Rc::new(handler);
        self
    }

    /// Handle a press on the revealed action. The argument is always `true` —
    /// the callback takes one so it shares
    /// [`ErasedArgCallback`](frust::authoring::ErasedArgCallback) with the
    /// expansion handler.
    pub fn on_action(mut self, handler: impl Fn(&mut State, bool) + 'static) -> Self {
        self.on_action = Rc::new(handler);
        self
    }

    /// Whether firing the action also asks for a collapse (default `true`,
    /// upstream's `collapseOnAction`).
    pub fn collapse_on_action(mut self, collapse: bool) -> Self {
        self.collapse_on_action = collapse;
        self
    }

    /// Disable the chip: neither region responds, everything dims.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// Which of the chip's two regions a pointer is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChipRegion {
    /// The label — pressing it toggles the expansion.
    Trigger,
    /// The revealed action — pressing it fires the action.
    Action,
}

/// The retained widget for an [`ExpandableChipView`].
pub struct ExpandableChipWidget {
    label: LabelRun,
    action_icon: LabelRun,
    action_label: String,
    expanded: bool,
    disabled: bool,
    collapse_on_action: bool,
    on_expanded_change: ErasedArgCallback<bool>,
    on_action: ErasedArgCallback<bool>,
    hovered: bool,
    /// The region the live press started in, if any.
    pressed: Option<ChipRegion>,
    captured: bool,
    width: SpringScalar,
    reveal: SpringScalar,
    scale: SpringScalar,
    label_size: Size,
    icon_size: Size,
}

impl ExpandableChipWidget {
    /// The width the chip settles at in each state.
    fn width_for(&self, expanded: bool) -> f64 {
        let base = CHIP_LEAD + self.label_size.width;
        if expanded {
            base + CHIP_TAIL_EXPANDED + CHIP_ACTION_SLOT
        } else {
            base + CHIP_TAIL
        }
    }

    /// Where the trigger region ends, in widget-local px.
    fn trigger_width(&self) -> f64 {
        CHIP_LEAD + self.label_size.width + CHIP_TAIL_EXPANDED
    }

    /// Which region `x` falls in — `None` when the action is not revealed and
    /// the pointer is past the label.
    fn region_at(&self, x: f64) -> ChipRegion {
        if self.expanded && x >= self.trigger_width() {
            ChipRegion::Action
        } else {
            ChipRegion::Trigger
        }
    }
}

impl<State: 'static> View<State> for ExpandableChipView<State> {
    type Element = ExpandableChipWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ExpandableChipWidget {
        ExpandableChipWidget {
            label: LabelRun::new(self.label.clone()),
            action_icon: LabelRun::new(self.action_icon.clone()),
            action_label: self.action_label.clone(),
            expanded: self.expanded,
            disabled: self.disabled,
            collapse_on_action: self.collapse_on_action,
            on_expanded_change: erase_callback_arg(&self.on_expanded_change),
            on_action: erase_callback_arg(&self.on_action),
            hovered: false,
            pressed: None,
            captured: false,
            width: SpringScalar::new(f64::NAN, Ramp::spring(SPRING_LAYOUT)),
            reveal: SpringScalar::new(
                if self.expanded { 1.0 } else { 0.0 },
                Ramp::eased(REVEAL_FADE, EASE_OUT),
            ),
            scale: SpringScalar::new(1.0, Ramp::spring(SPRING_PRESS)),
            label_size: Size::ZERO,
            icon_size: Size::ZERO,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ExpandableChipWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_expanded_change = erase_callback_arg(&self.on_expanded_change);
        element.on_action = erase_callback_arg(&self.on_action);
        element.action_label = self.action_label.clone();
        element.collapse_on_action = self.collapse_on_action;
        let mut flags = ChangeFlags::NONE;
        if prev.label != self.label {
            element.label.set_content(self.label.clone());
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.action_icon != self.action_icon {
            element.action_icon.set_content(self.action_icon.clone());
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.expanded != self.expanded {
            element.expanded = self.expanded;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.disabled != self.disabled {
            element.disabled = self.disabled;
            flags |= ChangeFlags::PAINT;
            if self.disabled {
                element.pressed = None;
                element.captured = false;
                element.hovered = false;
            }
        }
        flags
    }
}

impl Widget for ExpandableChipWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let style = label_style(TEXT_SM);
        self.label_size = self.label.layout(ctx, &style);
        self.icon_size = self.action_icon.layout(ctx, &style);
        if self.width.value().is_nan() {
            self.width.jump_to(self.width_for(self.expanded));
        }
        bc.constrain(Size::new(self.width.value(), CHIP_HEIGHT))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let paint = ControlPaint::resolve(theme);
        let now = ctx.frame_time();
        let size = ctx.size();
        let origin = ctx.origin();
        let dim = self.disabled;
        if !dim && !self.captured {
            self.hovered = ctx.is_hovered();
        }

        let target_width = self.width_for(self.expanded);
        let target_reveal = if self.expanded { 1.0 } else { 0.0 };
        let width_before = self.width.value();
        if reduce {
            self.width.jump_to(target_width);
            self.reveal.jump_to(target_reveal);
            self.scale.jump_to(1.0);
        } else {
            self.width.set_target(target_width);
            self.width.advance(now);
            self.reveal.set_target(target_reveal);
            self.reveal.advance(now);
            self.scale.set_target(if self.pressed.is_some() && !dim {
                PRESS_SCALE
            } else {
                1.0
            });
            self.scale.advance(now);
        }
        let reveal = self.reveal.value();
        let scale = self.scale.value();

        let centre = origin + Vec2::new(size.width / 2.0, size.height / 2.0);
        if scale != 1.0 {
            scene.push_transform(
                Affine::translate(centre.to_vec2())
                    * Affine::scale(scale)
                    * Affine::translate(-centre.to_vec2()),
            );
        }

        let radius = resolve_radius(RADIUS_CONTROL, size.width, size.height);
        if self.hovered && !dim {
            scene.fill_rounded_rect(origin, size, radius, paint.wash);
        }
        stroke_outline(
            scene,
            origin,
            size,
            radius,
            disabled_tint(paint.border, dim, DISABLED_OPACITY),
        );

        let ink = disabled_tint(paint.ink, dim, DISABLED_OPACITY);
        let label_at = Point::new(
            origin.x + CHIP_LEAD,
            origin.y + (size.height - self.label_size.height) / 2.0,
        );
        self.label.paint(label_at, ink, scene);

        if reveal > 0.0 {
            // `hover:text-foreground` on the action, muted otherwise.
            let action_ink = if self.pressed == Some(ChipRegion::Action) {
                ink
            } else {
                disabled_tint(paint.muted_ink, dim, DISABLED_OPACITY)
            };
            let slot_left = origin.x + self.trigger_width();
            let icon_at = Point::new(
                slot_left + (CHIP_ACTION_SLOT - self.icon_size.width) / 2.0,
                origin.y + (size.height - self.icon_size.height) / 2.0,
            );
            scene.push_clip_rounded(origin, size, radius);
            scene.push_layer(icon_at, self.icon_size, reveal.clamp(0.0, 1.0) as f32);
            self.action_icon.paint(icon_at, action_ink, scene);
            scene.pop_layer();
            scene.pop_clip();
        }

        if scale != 1.0 {
            scene.pop_transform();
        }

        // See the button's paint for why the flag alone cannot catch the
        // landing frame or the reduced-motion snap.
        if self.width.is_animating() || self.width.value() != width_before {
            ctx.request_layout();
        } else if self.reveal.is_animating() || self.scale.is_animating() {
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
                self.pressed = Some(self.region_at(p.position.x));
                self.captured = true;
                ctx.capture_pointer();
                ctx.request_focus();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                let over = inside(p.position, size);
                if self.captured {
                    // A press that wanders off the control drops its region, so
                    // a release outside fires nothing.
                    self.pressed = over.then(|| self.region_at(p.position.x));
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
                let region = self.pressed.take();
                self.captured = false;
                if inside(p.position, size) {
                    match region {
                        Some(ChipRegion::Trigger) => {
                            (self.on_expanded_change)(ctx, !self.expanded);
                        }
                        Some(ChipRegion::Action) => {
                            (self.on_action)(ctx, true);
                            if self.collapse_on_action {
                                (self.on_expanded_change)(ctx, false);
                            }
                        }
                        None => {}
                    }
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.pressed = None;
                self.captured = false;
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // Two activatable regions in one box, so the chip contributes a group
        // with a node for each rather than one node a screen reader could only
        // reach half of.
        ctx.push_container(
            Role::Group,
            |node| node.set_label(self.label.content()),
            |ctx| {
                ctx.push_node(Role::Button, |node| {
                    node.set_label(self.label.content());
                    node.set_expanded(self.expanded);
                    node.add_action(Action::Click);
                });
                if self.expanded {
                    ctx.push_node(Role::Button, |node| {
                        node.set_label(self.action_label.as_str());
                        node.add_action(Action::Click);
                    });
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::spacing;
    use frust::FrameTime;
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::text::TextContext;
    use frust::authoring::{BezPath, Brush, PointerButton, PointerEvent};
    use std::any::Any;

    #[derive(Default, PartialEq, Debug)]
    struct Log {
        expanded: Vec<bool>,
        actions: u32,
    }

    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<Color>,
        inks: Vec<Color>,
        alphas: Vec<f32>,
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
        fn push_layer(&mut self, _o: Point, _s: Size, alpha: f32) {
            self.alphas.push(alpha);
        }
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn dispatch<W: Widget>(w: &mut W, state: &mut Log, size: Size, event: &InputEvent) {
        let any_state: &mut dyn Any = state;
        let mut ctx = EventCtx::new(any_state, Point::ZERO, size);
        w.event(&mut ctx, event);
    }

    fn lay_out<W: Widget>(w: &mut W) -> Size {
        let mut text_ctx = TextContext::new();
        let mut ctx = LayoutCtx::with_resources(Some(&mut text_ctx as &mut dyn Any), None);
        w.layout(&mut ctx, &BoxConstraints::loose(Size::new(400.0, 200.0)))
    }

    fn paint_at<W: Widget>(
        w: &mut W,
        size: Size,
        theme: &Theme,
        ms: u64,
    ) -> (Recorder, bool, bool) {
        let mut ctx =
            PaintCtx::for_test(Point::ORIGIN, size, FrameTime::from_nanos(ms * 1_000_000))
                .with_theme(theme);
        let mut rec = Recorder::default();
        w.paint(&mut ctx, &mut rec);
        (rec, ctx.needs_frame(), ctx.needs_layout())
    }

    fn theme() -> Theme {
        crate::theme()
    }

    fn reduced_theme() -> Theme {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        theme
    }

    fn button(view: &ExpandableButtonView<Log>) -> ExpandableButtonWidget {
        let mut next_id = 0u64;
        View::<Log>::build(view, &mut BuildCtx::new(&mut next_id))
    }

    fn chip(view: &ExpandableChipView<Log>) -> ExpandableChipWidget {
        let mut next_id = 0u64;
        View::<Log>::build(view, &mut BuildCtx::new(&mut next_id))
    }

    fn rebuild_button(
        prev: &ExpandableButtonView<Log>,
        next: &ExpandableButtonView<Log>,
        w: &mut ExpandableButtonWidget,
    ) {
        let mut next_id = 0u64;
        View::<Log>::rebuild(next, prev, w, &mut BuildCtx::new(&mut next_id));
    }

    fn rebuild_chip(
        prev: &ExpandableChipView<Log>,
        next: &ExpandableChipView<Log>,
        w: &mut ExpandableChipWidget,
    ) {
        let mut next_id = 0u64;
        View::<Log>::rebuild(next, prev, w, &mut BuildCtx::new(&mut next_id));
    }

    // ---- Expandable button --------------------------------------------------

    /// Collapsed it is the square pill `min-w-11` describes; expanded it is that
    /// plus the label and its gutter.
    #[test]
    fn the_button_morphs_between_a_square_pill_and_its_labelled_width() {
        let collapsed = expandable_button::<Log>("+", "Add item", |_, _| {});
        let mut w = button(&collapsed);
        let size = lay_out(&mut w);
        assert_eq!(size, Size::new(BUTTON_HEIGHT, BUTTON_HEIGHT));

        let expanded = expandable_button::<Log>("+", "Add item", |_, _| {}).expanded(true);
        let mut open = button(&expanded);
        let open_size = lay_out(&mut open);
        assert_eq!(open_size.height, BUTTON_HEIGHT);
        assert!(open_size.width > size.width);
        assert_eq!(
            open_size.width,
            BUTTON_PAD * 2.0 + ICON_SLOT + open.label_size.width + LABEL_GUTTER
        );
    }

    /// The morph runs through layout: the paint that advances the spring asks
    /// for a relayout, and the next layout is the one that is wider. The
    /// landing frame — where the spring settles inside `advance` and its
    /// `is_animating` flag drops in that same call — still owes a relayout,
    /// because the width value itself is what moved.
    #[test]
    fn the_width_morph_drives_a_relayout_until_it_settles() {
        let collapsed = expandable_button::<Log>("+", "Add item", |_, _| {});
        let mut w = button(&collapsed);
        let narrow = lay_out(&mut w);

        let expanded = expandable_button::<Log>("+", "Add item", |_, _| {}).expanded(true);
        rebuild_button(&collapsed, &expanded, &mut w);

        let (_, _, relayout) = paint_at(&mut w, narrow, &theme(), 0);
        assert!(relayout, "the morph owes a relayout");
        let mid = lay_out(&mut w);
        assert_eq!(mid.width, narrow.width, "the first frame only latches");

        paint_at(&mut w, mid, &theme(), 100);
        let opening = lay_out(&mut w);
        assert!(opening.width > narrow.width, "and then it opens");

        // The landing frame: the ramp settles inside this very `advance`, so
        // `is_animating` is already false by the time paint reads it — the
        // relayout has to come from the value having moved this frame.
        let (_, _, relayout) = paint_at(&mut w, opening, &theme(), 5_000);
        assert!(relayout, "the landing frame still owes a relayout");
        let settled = lay_out(&mut w);
        assert_eq!(settled.width, w.width_for(true));

        // Now it has actually settled: nothing more is owed.
        let (_, _, relayout) = paint_at(&mut w, settled, &theme(), 5_100);
        assert!(!relayout, "and stops once settled");
    }

    /// Reduced motion lands on the settled width on the first paint. The snap
    /// still moves the width in one shot, so it owes exactly the one
    /// relayout that reveals it (which itself implies the frame) — nothing
    /// further once settled.
    #[test]
    fn reduced_motion_lands_the_morph_immediately() {
        let collapsed = expandable_button::<Log>("+", "Add item", |_, _| {});
        let mut w = button(&collapsed);
        let narrow = lay_out(&mut w);
        let expanded = expandable_button::<Log>("+", "Add item", |_, _| {}).expanded(true);
        rebuild_button(&collapsed, &expanded, &mut w);

        let (_, more, relayout) = paint_at(&mut w, narrow, &reduced_theme(), 0);
        assert!(more && relayout, "the snap owes its one relayout");
        let settled = lay_out(&mut w);
        assert_eq!(settled.width, w.width_for(true));

        let (_, more, relayout) = paint_at(&mut w, settled, &reduced_theme(), 100);
        assert!(!more && !relayout, "and nothing more once settled");
    }

    /// Controlled: a press reports the flag it wants and changes nothing on its
    /// own.
    #[test]
    fn the_button_reports_the_expansion_it_wants_without_self_mutating() {
        let view = expandable_button::<Log>("+", "Add item", |s: &mut Log, next| {
            s.expanded.push(next);
        });
        let mut w = button(&view);
        let size = lay_out(&mut w);
        let mut state = Log::default();

        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Down, 10.0, 20.0),
        );
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Up, 10.0, 20.0));
        assert_eq!(state.expanded, vec![true]);
        assert!(!w.expanded, "the widget waits for the rebuild");

        // A release outside reports nothing.
        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Down, 10.0, 20.0),
        );
        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Up, size.width + 20.0, 20.0),
        );
        assert_eq!(state.expanded, vec![true]);
    }

    #[test]
    fn a_disabled_button_neither_reports_nor_presses() {
        let view = expandable_button::<Log>("+", "Add item", |s: &mut Log, next| {
            s.expanded.push(next);
        })
        .disabled(true);
        let mut w = button(&view);
        let size = lay_out(&mut w);
        let mut state = Log::default();
        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Down, 10.0, 20.0),
        );
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Up, 10.0, 20.0));
        assert!(state.expanded.is_empty());
        assert!(!w.captured);
    }

    /// The label only paints once it is being revealed, and the icon always
    /// does.
    #[test]
    fn the_label_appears_only_while_the_control_is_open() {
        let collapsed = expandable_button::<Log>("+", "Add item", |_, _| {});
        let mut w = button(&collapsed);
        let size = lay_out(&mut w);
        let (rec, _, _) = paint_at(&mut w, size, &theme(), 0);
        assert!(rec.alphas.is_empty(), "nothing revealed");
        assert!(!rec.inks.is_empty(), "the icon still paints");
        assert!(!rec.strokes.is_empty(), "and the pill is bordered");

        let expanded = expandable_button::<Log>("+", "Add item", |_, _| {}).expanded(true);
        let mut open = button(&expanded);
        let open_size = lay_out(&mut open);
        let (rec, _, _) = paint_at(&mut open, open_size, &theme(), 0);
        assert_eq!(rec.alphas, vec![1.0], "the label is composited at full");
    }

    // ---- Expandable chip ----------------------------------------------------

    #[test]
    fn the_chip_widens_by_its_action_slot_when_open() {
        let collapsed = expandable_chip::<Log>("Draft", "x", "Discard draft");
        let mut w = chip(&collapsed);
        let closed = lay_out(&mut w);
        assert_eq!(closed.height, CHIP_HEIGHT);
        assert_eq!(closed.width, CHIP_LEAD + w.label_size.width + CHIP_TAIL);

        let expanded = expandable_chip::<Log>("Draft", "x", "Discard draft").expanded(true);
        let mut open = chip(&expanded);
        let open_size = lay_out(&mut open);
        assert_eq!(
            open_size.width,
            CHIP_LEAD + open.label_size.width + CHIP_TAIL_EXPANDED + CHIP_ACTION_SLOT
        );
    }

    /// The two regions do different things, and the action region only exists
    /// while the chip is open.
    #[test]
    fn the_label_toggles_and_the_revealed_action_fires() {
        let view = expandable_chip::<Log>("Draft", "x", "Discard draft")
            .expanded(true)
            .on_expanded_change(|s: &mut Log, next| s.expanded.push(next))
            .on_action(|s: &mut Log, _| s.actions += 1);
        let mut w = chip(&view);
        let size = lay_out(&mut w);
        let mut state = Log::default();

        // The label half toggles.
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Down, 6.0, 20.0));
        assert_eq!(w.pressed, Some(ChipRegion::Trigger));
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Up, 6.0, 20.0));
        assert_eq!(state.expanded, vec![false]);
        assert_eq!(state.actions, 0);

        // The action half fires, and asks for the collapse upstream's
        // `collapseOnAction` performs.
        let action_x = w.trigger_width() + CHIP_ACTION_SLOT / 2.0;
        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Down, action_x, 20.0),
        );
        assert_eq!(w.pressed, Some(ChipRegion::Action));
        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Up, action_x, 20.0),
        );
        assert_eq!(state.actions, 1);
        assert_eq!(state.expanded, vec![false, false]);
    }

    /// With the chip closed there is no action region at all: the same
    /// coordinate is part of the trigger.
    #[test]
    fn a_closed_chip_has_no_action_region() {
        let view = expandable_chip::<Log>("Draft", "x", "Discard draft")
            .on_expanded_change(|s: &mut Log, next| s.expanded.push(next))
            .on_action(|s: &mut Log, _| s.actions += 1);
        let mut w = chip(&view);
        let size = lay_out(&mut w);
        let mut state = Log::default();

        let far = size.width - 2.0;
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Down, far, 20.0));
        assert_eq!(w.pressed, Some(ChipRegion::Trigger));
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Up, far, 20.0));
        assert_eq!(state.actions, 0);
        assert_eq!(state.expanded, vec![true]);
    }

    /// `collapse_on_action(false)` fires the action and leaves the chip open.
    #[test]
    fn an_action_can_be_told_not_to_collapse() {
        let view = expandable_chip::<Log>("Draft", "x", "Discard draft")
            .expanded(true)
            .collapse_on_action(false)
            .on_expanded_change(|s: &mut Log, next| s.expanded.push(next))
            .on_action(|s: &mut Log, _| s.actions += 1);
        let mut w = chip(&view);
        let size = lay_out(&mut w);
        let mut state = Log::default();
        let action_x = w.trigger_width() + CHIP_ACTION_SLOT / 2.0;
        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Down, action_x, 20.0),
        );
        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Up, action_x, 20.0),
        );
        assert_eq!(state.actions, 1);
        assert!(state.expanded.is_empty());
    }

    /// A press that wanders off the chip fires nothing on release.
    #[test]
    fn a_press_dragged_off_the_chip_fires_nothing() {
        let view = expandable_chip::<Log>("Draft", "x", "Discard draft")
            .expanded(true)
            .on_expanded_change(|s: &mut Log, next| s.expanded.push(next))
            .on_action(|s: &mut Log, _| s.actions += 1);
        let mut w = chip(&view);
        let size = lay_out(&mut w);
        let mut state = Log::default();
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Down, 6.0, 20.0));
        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Move, size.width + 40.0, 20.0),
        );
        assert_eq!(w.pressed, None);
        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Up, size.width + 40.0, 20.0),
        );
        assert!(state.expanded.is_empty());
        assert_eq!(state.actions, 0);
    }

    /// The action glyph is composited only while the chip is revealing it.
    #[test]
    fn the_action_glyph_appears_only_while_the_chip_is_open() {
        let collapsed = expandable_chip::<Log>("Draft", "x", "Discard draft");
        let mut w = chip(&collapsed);
        let size = lay_out(&mut w);
        let (rec, _, _) = paint_at(&mut w, size, &theme(), 0);
        assert!(rec.alphas.is_empty());

        let expanded = expandable_chip::<Log>("Draft", "x", "Discard draft").expanded(true);
        rebuild_chip(&collapsed, &expanded, &mut w);
        // The first frame latches the ramp's start; the second is the one that
        // has something to composite.
        paint_at(&mut w, size, &theme(), 0);
        let (rec, _, _) = paint_at(&mut w, size, &theme(), 60);
        assert_eq!(rec.alphas.len(), 1, "the glyph fades in with the reveal");
        assert!(
            rec.alphas[0] > 0.0 && rec.alphas[0] < 1.0,
            "mid-fade: {:?}",
            rec.alphas
        );

        let (rec, _, _) = paint_at(&mut w, size, &theme(), 5_000);
        assert_eq!(rec.alphas, vec![1.0], "and settles fully revealed");
    }

    /// The chip publishes both of its activatable regions, and only the ones a
    /// pointer can actually reach.
    #[test]
    fn the_chip_publishes_a_node_per_reachable_region() {
        let closed = expandable_chip::<Log>("Draft", "x", "Discard draft");
        let mut w = chip(&closed);
        assert_eq!(w.region_at(1_000.0), ChipRegion::Trigger);

        let open = expandable_chip::<Log>("Draft", "x", "Discard draft").expanded(true);
        rebuild_chip(&closed, &open, &mut w);
        lay_out(&mut w);
        assert_eq!(w.region_at(w.trigger_width() + 1.0), ChipRegion::Action);
        assert_eq!(w.region_at(w.trigger_width() - 1.0), ChipRegion::Trigger);
    }

    /// The spacing ladder is where these metrics come from, not a local guess.
    #[test]
    fn the_metrics_are_the_shared_ladders_own() {
        assert_eq!(BUTTON_PAD, spacing(1.0), "p-1");
        assert_eq!(ICON_SLOT, spacing(9.0), "size-9");
        assert_eq!(LABEL_GUTTER, spacing(3.0), "pr-3");
        assert_eq!(CHIP_ACTION_SLOT, spacing(8.0), "w-8");
        assert_eq!(BUTTON_HEIGHT, HEIGHT_INPUT, "h-11");
        assert_eq!(CHIP_HEIGHT, HEIGHT_MD, "h-10");
    }
}
