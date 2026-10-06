//! Ports beUI's `expanding-arrow-button` registry slug — the three
//! call-to-action buttons it installs together:
//! `components/motion/expanding-arrow-button.tsx`,
//! `components/motion/hold-action-button.tsx` and
//! `components/motion/slide-action-button.tsx` (beUI rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01).
//!
//! - [`expanding_arrow_button`] — an accent tile that expands into a trail of
//!   dotted chevrons on hover or focus.
//! - [`hold_action_button`] — hold to complete, with a fill that rises (or
//!   sweeps) behind the label; release early to cancel.
//! - [`slide_action_button`] — drag the thumb past a threshold to confirm;
//!   release short of it and the thumb springs back.
//!
//! # Colors come from the token tables, not the source's literals
//!
//! These three are the catalog's most hardcoded components upstream:
//! `bg-neutral-950` with a `lime-300` accent, `bg-sky-400` for the hold fill.
//! None of those are beUI tokens — they are Tailwind palette entries picked per
//! component — so a token-driven port cannot transcribe them. The mapping used
//! here, and the departure it represents:
//!
//! - the near-black CTA track becomes `inverse_surface` with `inverse_on_surface`
//!   ink, which is what a beUI theme's inverted surface pair *is*;
//! - the bright accent tile and the hold fill become the accent pair
//!   (`primary_container`/`on_primary_container`), beUI's one saturated fill.
//!
//! So the shapes, metrics and motion are the source's; two hues are not.
//!
//! # Departures from the source
//!
//! - **The hold button's liquid edge is not ported.** Upstream rides a looping
//!   wave SVG on the leading edge of its fill; this paints the fill's edge
//!   straight. The fill, its direction, its timing and its cancel are ported.
//! - **[`HoldActionButtonView::on_complete`] fires on the next event pass, not
//!   the instant the fill lands.** The fill is advanced during paint, and a
//!   paint pass carries no `EventCtx` to call an app callback through — the same
//!   deferral [`Presence`](crate::motion::Presence) documents. The completion is
//!   latched at paint (the fill and the completion label land immediately) and
//!   drained on the next pointer event the widget sees, which for a real hold
//!   gesture is the release. The framework's own `Housekeeping` flush seam,
//!   which would close the gap, is not on the public facade this catalog is
//!   restricted to.
//! - **Keyboard activation is not ported.** Upstream's hold and slide buttons
//!   both accept `Enter`/`Space`; these are pointer-driven only.
//! - **No blur or shadow filters**, matching the rest of the port.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, Affine, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, Color, ErasedCallback,
    EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, PointerEvent,
    PointerPhase, Role, SemanticsCtx, Size, Vec2, View, Widget, erase_callback,
};
use frust::{FrameTime, Theme};

use crate::motion::{Ramp, Stagger};
use crate::style::{
    ACTIVE_CURSOR, DISABLED_OPACITY, PRESS_SCALE_CSS, TEXT_SM, disabled_tint, resolve_radius,
    with_alpha,
};
use crate::tokens::color_scheme_light;
use crate::tokens::motion::{EASE_OUT, SPRING_LAYOUT, SPRING_PRESS};

use crate::press::{SpringScalar, inside, presses, stroke_outline};
use crate::text::{LabelRun, ThemeTextType, label_style};

// ---- Shared CTA metrics ----------------------------------------------------

/// The CTA height all three buttons share: `h-16`.
const CTA_HEIGHT: f64 = 64.0;

/// Their minimum width: `min-w-72` on the first two, a fixed `w-72` on the
/// slider.
const CTA_MIN_WIDTH: f64 = 288.0;

/// Their corner radius: `rounded-[22px]`.
const CTA_RADIUS: f64 = 22.0;

/// `text-lg` — the CTA label size. One rung above
/// [`TEXT_BASE`](crate::style::TEXT_BASE), and used by nothing else in the
/// catalog, so it stays here rather than in the shared ladder.
const CTA_TEXT: f64 = 18.0;

/// How long a CTA's own label cross-fade takes: `duration: 0.12`.
const LABEL_FADE: Duration = Duration::from_millis(120);

/// The type-scale role every CTA label takes its family from at layout — a
/// control label, as on [`button`](super::button).
const LABEL_ROLE: ThemeTextType = ThemeTextType::LabelLarge;

// ---- Expanding arrow -------------------------------------------------------

/// The padding around the accent tile: `p-1.5` / `inset-y-1.5 left-1.5`.
const ARROW_PAD: f64 = 6.0;

/// The accent tile's collapsed width: `width: active ? "calc(100% - 12px)" : 52`.
const ARROW_TILE_COLLAPSED: f64 = 52.0;

/// The accent tile's radius: `borderRadius: 16`.
const ARROW_TILE_RADIUS: f64 = 16.0;

/// The label's own gutters: `ml-[76px] mr-5`.
const ARROW_LABEL_LEFT: f64 = 76.0;
const ARROW_LABEL_RIGHT: f64 = 20.0;

/// How far the label slides while it fades out: `translateX(6px)`.
const ARROW_LABEL_SHIFT: f64 = 6.0;

/// The chevron glyph's box: `h-7 w-5`, drawn from a `0 0 20 28` viewBox at 1:1.
const CHEVRON: Size = Size::new(20.0, 28.0);

/// The chevron's five dots and their radius, in viewBox units.
const CHEVRON_DOTS: [(f64, f64); 5] = [
    (4.0, 4.0),
    (10.0, 9.0),
    (16.0, 14.0),
    (10.0, 19.0),
    (4.0, 24.0),
];
const CHEVRON_DOT_RADIUS: f64 = 2.0;

/// The trail's five opacities: `ARROW_OPACITY`.
const ARROW_TRAIL_ALPHA: [f32; 5] = [1.0, 0.78, 0.54, 0.32, 0.16];

/// The trail's inner padding: `px-3`.
const ARROW_TRAIL_PAD: f64 = 12.0;

/// One trail chevron's fade-in: `{ duration: 0.18, delay: 0.04 + index * 0.025 }`.
const ARROW_TRAIL_STEP: Duration = Duration::from_millis(25);
const ARROW_TRAIL_RUN: Duration = Duration::from_millis(180);
const ARROW_TRAIL_LEAD: Duration = Duration::from_millis(40);

/// How far a trail chevron slides in: `translateX(-6px)` → `0`.
const ARROW_TRAIL_SHIFT: f64 = 6.0;

/// The collapsed chevron's own fade: `{ duration: 0.1 }`.
const ARROW_GLYPH_FADE: Duration = Duration::from_millis(100);

// ---- Hold action -----------------------------------------------------------

/// The default hold length: `holdDuration = 1600`.
pub const HOLD_DURATION: Duration = Duration::from_millis(1600);

/// How fast a cancelled fill retracts: `{ duration: 0.24, ease: EASE_OUT }`.
const HOLD_RETRACT: Duration = Duration::from_millis(240);

/// The hold button's horizontal padding: `px-8`.
const HOLD_PAD_X: f64 = 32.0;

/// The scale a held button shrinks to: `whileTap={{ scale: 0.98 }}`.
const HOLD_PRESS_SCALE: f64 = 0.98;

// ---- Slide action ----------------------------------------------------------

/// The fraction of the travel a release must be past to confirm:
/// `threshold = 0.82`.
pub const SLIDE_THRESHOLD: f64 = 0.82;

/// How long a confirmed slider waits before resetting: `resetDelay = 1200`.
pub const SLIDE_RESET_DELAY: Duration = Duration::from_millis(1200);

/// The track's inner padding: `p-1`.
const SLIDE_PAD: f64 = 4.0;

/// The thumb: `size-14`, `rounded-[18px]`.
const SLIDE_THUMB: f64 = 56.0;
const SLIDE_THUMB_RADIUS: f64 = 18.0;

/// The slack `maxDistance` keeps at the far end:
/// `track.clientWidth - thumb.clientWidth - 8`.
const SLIDE_END_SLACK: f64 = 8.0;

/// Where the track label has finished fading: `[0, 0.35, 0.65] → [1, 0.75, 0]`.
const SLIDE_LABEL_FADED: f64 = 0.65;

/// The scale a dragged thumb shrinks to: `whileTap={{ scale: 0.94 }}`.
const SLIDE_PRESS_SCALE: f64 = 0.94;

/// The alpha the slider's own track and ring are washed at: `bg-primary/10`,
/// `ring-primary/10`.
const SLIDE_TRACK_ALPHA: f32 = 0.1;

/// The thumb glyph's three keyframes — `iconPath`'s chevron, half-turned tick
/// and finished tick, as three points each in a `0 0 24 24` box. The painted
/// glyph interpolates between them by drag progress, which is exactly what
/// `useTransform` does with the three `d` strings.
const SLIDE_GLYPH: [[(f64, f64); 3]; 3] = [
    [(8.0, 5.0), (15.0, 12.0), (8.0, 19.0)],
    [(7.0, 8.0), (12.0, 14.0), (17.0, 10.0)],
    [(5.0, 12.0), (10.0, 17.0), (19.0, 7.0)],
];

/// The glyph's own box, and the stroke it is drawn with: `size-5`,
/// `strokeWidth="2"`.
const SLIDE_GLYPH_BOX: f64 = 20.0;
const SLIDE_GLYPH_STROKE: f64 = 2.0;

// =============================================================================
// Expanding arrow button
// =============================================================================

/// The expanding-arrow CTA's resolved roles.
#[derive(Clone, Copy)]
struct ArrowPaint {
    track: Color,
    ink: Color,
    tile: Color,
    tile_ink: Color,
}

impl ArrowPaint {
    fn resolve(theme: Option<&Theme>) -> Self {
        let scheme = theme.map(Theme::scheme);
        macro_rules! role {
            ($role:ident) => {
                scheme.map_or(color_scheme_light().$role, |s| s.$role)
            };
        }
        Self {
            track: role!(inverse_surface),
            ink: role!(inverse_on_surface),
            tile: role!(primary_container),
            tile_ink: role!(on_primary_container),
        }
    }
}

/// A declarative expanding-arrow CTA.
pub struct ExpandingArrowButtonView<State: 'static> {
    label: String,
    disabled: bool,
    on_press: Rc<dyn Fn(&mut State)>,
}

/// A call-to-action button whose accent tile expands into a dotted-arrow trail
/// while it is hovered or focused, running `on_press` on release inside.
pub fn expanding_arrow_button<State: 'static>(
    label: impl Into<String>,
    on_press: impl Fn(&mut State) + 'static,
) -> ExpandingArrowButtonView<State> {
    ExpandingArrowButtonView {
        label: label.into(),
        disabled: false,
        on_press: Rc::new(on_press),
    }
}

impl<State: 'static> ExpandingArrowButtonView<State> {
    /// Disable the button: no callback, no claims, everything dimmed.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// The retained widget for an [`ExpandingArrowButtonView`].
pub struct ExpandingArrowButtonWidget {
    label: LabelRun,
    disabled: bool,
    on_press: ErasedCallback,
    hovered: bool,
    pressed: bool,
    captured: bool,
    /// The tile's width, on `SPRING_LAYOUT`.
    tile: SpringScalar,
    /// The press scale, on `SPRING_PRESS`.
    scale: SpringScalar,
    /// The resting chevron's opacity and the label's, each on its own short
    /// eased ramp. Both start settled at 1, so a button's first paint shows its
    /// resting state rather than fading into it.
    glyph_alpha: SpringScalar,
    label_alpha: SpringScalar,
    /// Whether the trail is currently expanded, and when that changed — the
    /// clock the per-chevron stagger is timed from.
    active: bool,
    changed_at: Option<FrameTime>,
    label_size: Size,
}

impl<State: 'static> View<State> for ExpandingArrowButtonView<State> {
    type Element = ExpandingArrowButtonWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ExpandingArrowButtonWidget {
        ExpandingArrowButtonWidget {
            label: LabelRun::new(self.label.clone()),
            disabled: self.disabled,
            on_press: erase_callback(&self.on_press),
            hovered: false,
            pressed: false,
            captured: false,
            tile: SpringScalar::new(ARROW_TILE_COLLAPSED, Ramp::spring(SPRING_LAYOUT)),
            scale: SpringScalar::new(1.0, Ramp::spring(SPRING_PRESS)),
            glyph_alpha: SpringScalar::new(1.0, Ramp::eased(ARROW_GLYPH_FADE, EASE_OUT)),
            label_alpha: SpringScalar::new(1.0, Ramp::eased(LABEL_FADE, EASE_OUT)),
            active: false,
            changed_at: None,
            label_size: Size::ZERO,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ExpandingArrowButtonWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_press = erase_callback(&self.on_press);
        let mut flags = ChangeFlags::NONE;
        if prev.label != self.label {
            element.label.set_content(self.label.clone());
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

impl Widget for ExpandingArrowButtonWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        self.label_size = self
            .label
            .layout_themed(ctx, &label_style(CTA_TEXT), LABEL_ROLE);
        let width =
            (self.label_size.width + ARROW_LABEL_LEFT + ARROW_LABEL_RIGHT).max(CTA_MIN_WIDTH);
        bc.constrain(Size::new(width, CTA_HEIGHT))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let paint = ArrowPaint::resolve(theme);
        let now = ctx.frame_time();
        let size = ctx.size();
        let origin = ctx.origin();
        let focused = ctx.has_focus();
        if !self.disabled {
            self.hovered = ctx.is_hovered();
        }

        // Upstream's `active`: hovered (on a hover-capable pointer) or focused,
        // and never while disabled.
        let active = !self.disabled && (self.hovered || focused);
        if active != self.active {
            self.active = active;
            self.changed_at = None;
        }
        let elapsed = {
            let started = *self.changed_at.get_or_insert(now);
            now.saturating_sub(started)
        };

        let dim = self.disabled;
        let track = disabled_tint(paint.track, dim, DISABLED_OPACITY);
        let radius = resolve_radius(CTA_RADIUS, size.width, size.height);

        let target_scale = if !dim && self.pressed {
            PRESS_SCALE_CSS
        } else {
            1.0
        };
        let scale = if reduce {
            self.scale.jump_to(1.0);
            1.0
        } else {
            self.scale.set_target(target_scale);
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

        scene.fill_rounded_rect(origin, size, radius, track);

        // The accent tile: 52px collapsed, the full inner width expanded.
        let expanded = (size.width - ARROW_PAD * 2.0).max(ARROW_TILE_COLLAPSED);
        let target_width = if active {
            expanded
        } else {
            ARROW_TILE_COLLAPSED
        };
        let tile_width = if reduce {
            self.tile.jump_to(target_width);
            target_width
        } else {
            self.tile.set_target(target_width);
            self.tile.advance(now)
        };
        let tile_origin = Point::new(origin.x + ARROW_PAD, origin.y + ARROW_PAD);
        let tile_size = Size::new(tile_width, (size.height - ARROW_PAD * 2.0).max(0.0));
        scene.fill_rounded_rect(
            tile_origin,
            tile_size,
            ARROW_TILE_RADIUS,
            disabled_tint(paint.tile, dim, DISABLED_OPACITY),
        );

        // The chevrons, clipped to the tile so the trail cannot spill while the
        // tile is still opening.
        scene.push_clip_rounded(tile_origin, tile_size, ARROW_TILE_RADIUS);
        let glyph_ink = disabled_tint(paint.tile_ink, dim, DISABLED_OPACITY);
        // `opacity: active ? 0 : 1`, on its own short ramp.
        let resting_alpha = if active { 0.0 } else { 1.0 };
        let glyph_alpha = if reduce {
            self.glyph_alpha.jump_to(resting_alpha);
            resting_alpha
        } else {
            self.glyph_alpha.set_target(resting_alpha);
            self.glyph_alpha.advance(now)
        };
        if active {
            let trail = if reduce {
                Stagger::eased(ARROW_TRAIL_STEP, ARROW_TRAIL_RUN, EASE_OUT).collapsed()
            } else {
                Stagger::eased(ARROW_TRAIL_STEP, ARROW_TRAIL_RUN, EASE_OUT)
            };
            let since_lead = elapsed.saturating_sub(ARROW_TRAIL_LEAD);
            let count = ARROW_TRAIL_ALPHA.len();
            let slot = (tile_size.width - ARROW_TRAIL_PAD * 2.0).max(0.0) / count as f64;
            for (index, alpha) in ARROW_TRAIL_ALPHA.iter().enumerate() {
                let progress = trail.progress_clamped(since_lead, index, count);
                let x = tile_origin.x + ARROW_TRAIL_PAD + slot * (index as f64 + 0.5)
                    - CHEVRON.width / 2.0
                    - (1.0 - progress) * ARROW_TRAIL_SHIFT;
                let at = Point::new(x, tile_origin.y + (tile_size.height - CHEVRON.height) / 2.0);
                paint_chevron(scene, at, with_alpha(glyph_ink, *alpha * progress as f32));
            }
        } else {
            // The single resting chevron, centred in the collapsed tile.
            let at = Point::new(
                tile_origin.x + (tile_size.width - CHEVRON.width) / 2.0,
                tile_origin.y + (tile_size.height - CHEVRON.height) / 2.0,
            );
            paint_chevron(scene, at, with_alpha(glyph_ink, glyph_alpha as f32));
        }
        scene.pop_clip();

        // The label fades out and slides right as the tile takes its place.
        let label_alpha = if reduce {
            self.label_alpha.jump_to(resting_alpha);
            resting_alpha
        } else {
            self.label_alpha.set_target(resting_alpha);
            self.label_alpha.advance(now)
        };
        let hidden = 1.0 - label_alpha;
        let label_at = Point::new(
            origin.x + ARROW_LABEL_LEFT + hidden * ARROW_LABEL_SHIFT,
            origin.y + (size.height - self.label_size.height) / 2.0,
        );
        scene.push_layer(label_at, self.label_size, label_alpha as f32);
        self.label.paint(
            label_at,
            disabled_tint(paint.ink, dim, DISABLED_OPACITY),
            scene,
        );
        scene.pop_layer();

        if scale != 1.0 {
            scene.pop_transform();
        }

        let trailing =
            active && elapsed < ARROW_TRAIL_LEAD + ARROW_TRAIL_RUN + ARROW_TRAIL_STEP * 5;
        let running = self.tile.is_animating()
            || self.scale.is_animating()
            || self.glyph_alpha.is_animating()
            || self.label_alpha.is_animating();
        if !reduce && (running || trailing) {
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
        press_machine(
            p,
            ctx.size(),
            &mut self.pressed,
            &mut self.captured,
            &mut self.hovered,
            ctx,
            &mut self.on_press,
        )
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::Button, |node| {
            node.set_label(self.label.content());
            if self.disabled {
                node.set_disabled();
            } else {
                node.add_action(Action::Click);
            }
        });
    }
}

/// Paint one dotted chevron, its top-left at `at`, in `ink`.
fn paint_chevron(scene: &mut dyn PaintScene, at: Point, ink: Color) {
    if ink.components[3] <= 0.0 {
        return;
    }
    let diameter = CHEVRON_DOT_RADIUS * 2.0;
    for (x, y) in CHEVRON_DOTS {
        scene.fill_rounded_rect(
            Point::new(at.x + x - CHEVRON_DOT_RADIUS, at.y + y - CHEVRON_DOT_RADIUS),
            Size::new(diameter, diameter),
            CHEVRON_DOT_RADIUS,
            ink,
        );
    }
}

/// The catalog's plain press machine: capture on a primary down inside, track
/// the pressed visual on move, fire on an up that lands inside, clear on cancel.
///
/// Shared by the two CTA buttons whose whole surface is the control; the slider
/// drives its own, because its gesture is a drag rather than a press.
fn press_machine(
    p: &PointerEvent,
    size: Size,
    pressed: &mut bool,
    captured: &mut bool,
    hovered: &mut bool,
    ctx: &mut EventCtx,
    on_press: &mut ErasedCallback,
) -> EventResult {
    match p.phase {
        PointerPhase::Down => {
            if !presses(p) || !inside(p.position, size) {
                return EventResult::Ignored;
            }
            *pressed = true;
            *captured = true;
            ctx.capture_pointer();
            ctx.request_focus();
            ctx.request_redraw();
            EventResult::Handled
        }
        PointerPhase::Move => {
            let over = inside(p.position, size);
            if *captured {
                *pressed = over;
                ctx.set_cursor(ACTIVE_CURSOR);
                ctx.request_redraw();
                return EventResult::Handled;
            }
            if over {
                ctx.claim_hover();
                ctx.set_cursor(ACTIVE_CURSOR);
            }
            if *hovered != over {
                *hovered = over;
                ctx.request_redraw();
            }
            EventResult::Ignored
        }
        PointerPhase::Up => {
            if !*captured {
                return EventResult::Ignored;
            }
            if inside(p.position, size) {
                on_press(ctx);
            }
            *pressed = false;
            *captured = false;
            ctx.request_redraw();
            EventResult::Handled
        }
        PointerPhase::Cancel => {
            if !*captured {
                return EventResult::Ignored;
            }
            *pressed = false;
            *captured = false;
            ctx.request_redraw();
            EventResult::Handled
        }
    }
}

// =============================================================================
// Hold action button
// =============================================================================

/// Which way a [`hold_action_button`]'s fill travels — upstream's `type` prop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HoldActionDirection {
    /// `translateY(115%)` → `0`: the fill rises from below.
    #[default]
    Vertical,
    /// `translateX(-100%)` → `0`: the fill sweeps in from the left.
    Horizontal,
}

/// Where a hold is in its lifecycle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum HoldPhase {
    /// Nothing held; the fill is retracting or already gone.
    #[default]
    Idle,
    /// Held, the fill filling.
    Holding,
    /// The fill landed. The completion is latched for the next event pass.
    Completed,
}

/// The hold CTA's resolved roles.
#[derive(Clone, Copy)]
struct HoldPaint {
    track: Color,
    ink: Color,
    fill: Color,
}

impl HoldPaint {
    fn resolve(theme: Option<&Theme>) -> Self {
        let scheme = theme.map(Theme::scheme);
        macro_rules! role {
            ($role:ident) => {
                scheme.map_or(color_scheme_light().$role, |s| s.$role)
            };
        }
        Self {
            track: role!(primary),
            ink: role!(on_primary),
            fill: role!(primary_container),
        }
    }
}

/// A declarative hold-to-confirm CTA.
pub struct HoldActionButtonView<State: 'static> {
    label: String,
    holding_label: String,
    complete_label: String,
    direction: HoldActionDirection,
    hold_duration: Duration,
    disabled: bool,
    on_complete: Rc<dyn Fn(&mut State)>,
}

/// A call-to-action button that runs `on_complete` once its fill has been held
/// all the way, and cancels silently if the pointer leaves or lifts first.
pub fn hold_action_button<State: 'static>(
    label: impl Into<String>,
    on_complete: impl Fn(&mut State) + 'static,
) -> HoldActionButtonView<State> {
    HoldActionButtonView {
        label: label.into(),
        // The source's own prop defaults.
        holding_label: "Keep holding".to_string(),
        complete_label: "Done".to_string(),
        direction: HoldActionDirection::default(),
        hold_duration: HOLD_DURATION,
        disabled: false,
        on_complete: Rc::new(on_complete),
    }
}

impl<State: 'static> HoldActionButtonView<State> {
    /// The label shown while the hold is in progress (default `"Keep holding"`).
    pub fn holding_label(mut self, label: impl Into<String>) -> Self {
        self.holding_label = label.into();
        self
    }

    /// The label shown once the hold completes (default `"Done"`).
    pub fn complete_label(mut self, label: impl Into<String>) -> Self {
        self.complete_label = label.into();
        self
    }

    /// Which way the fill travels (default [`HoldActionDirection::Vertical`]).
    pub fn direction(mut self, direction: HoldActionDirection) -> Self {
        self.direction = direction;
        self
    }

    /// How long the hold takes (default [`HOLD_DURATION`]).
    pub fn hold_duration(mut self, duration: Duration) -> Self {
        self.hold_duration = duration;
        self
    }

    /// Disable the button.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// The retained widget for a [`HoldActionButtonView`].
pub struct HoldActionButtonWidget {
    /// The idle, holding and completed labels, in [`HoldPhase`] order.
    labels: [LabelRun; 3],
    direction: HoldActionDirection,
    hold_duration: Duration,
    disabled: bool,
    on_complete: ErasedCallback,

    phase: HoldPhase,
    captured: bool,
    /// The frame the current hold started at, and the fill's progress when it
    /// was last cancelled (so a retraction starts where the fill had reached).
    hold_started: Option<FrameTime>,
    fill: SpringScalar,
    scale: SpringScalar,
    /// The completion the next event pass drains — see the [module docs](self).
    completion_pending: bool,
    label_sizes: [Size; 3],
}

impl HoldActionButtonWidget {
    /// The label the current phase shows.
    fn label(&self) -> &LabelRun {
        &self.labels[self.phase as usize]
    }

    /// Stop holding, leaving the fill to retract. Never fires the callback.
    fn cancel(&mut self) {
        self.phase = HoldPhase::Idle;
        self.hold_started = None;
    }
}

impl<State: 'static> View<State> for HoldActionButtonView<State> {
    type Element = HoldActionButtonWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> HoldActionButtonWidget {
        HoldActionButtonWidget {
            labels: [
                LabelRun::new(self.label.clone()),
                LabelRun::new(self.holding_label.clone()),
                LabelRun::new(self.complete_label.clone()),
            ],
            direction: self.direction,
            hold_duration: self.hold_duration,
            disabled: self.disabled,
            on_complete: erase_callback(&self.on_complete),
            phase: HoldPhase::Idle,
            captured: false,
            hold_started: None,
            fill: SpringScalar::new(0.0, Ramp::eased(HOLD_RETRACT, EASE_OUT)),
            scale: SpringScalar::new(1.0, Ramp::spring(SPRING_PRESS)),
            completion_pending: false,
            label_sizes: [Size::ZERO; 3],
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut HoldActionButtonWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_complete = erase_callback(&self.on_complete);
        let mut flags = ChangeFlags::NONE;
        for (index, (was, now)) in [
            (&prev.label, &self.label),
            (&prev.holding_label, &self.holding_label),
            (&prev.complete_label, &self.complete_label),
        ]
        .into_iter()
        .enumerate()
        {
            if was != now {
                element.labels[index].set_content(now.clone());
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
        }
        if prev.direction != self.direction {
            element.direction = self.direction;
            flags |= ChangeFlags::PAINT;
        }
        if prev.hold_duration != self.hold_duration {
            element.hold_duration = self.hold_duration;
            flags |= ChangeFlags::PAINT;
        }
        if prev.disabled != self.disabled {
            element.disabled = self.disabled;
            flags |= ChangeFlags::PAINT;
            if self.disabled {
                element.captured = false;
                element.cancel();
            }
        }
        flags
    }
}

impl Widget for HoldActionButtonWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let style = label_style(CTA_TEXT);
        let mut widest: f64 = 0.0;
        for (index, label) in self.labels.iter_mut().enumerate() {
            let size = label.layout_themed(ctx, &style, LABEL_ROLE);
            self.label_sizes[index] = size;
            widest = widest.max(size.width);
        }
        let width = (widest + HOLD_PAD_X * 2.0).max(CTA_MIN_WIDTH);
        bc.constrain(Size::new(width, CTA_HEIGHT))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let paint = HoldPaint::resolve(theme);
        let now = ctx.frame_time();
        let size = ctx.size();
        let origin = ctx.origin();
        let dim = self.disabled;

        // The fill: linear while held (upstream times it `ease: "linear"`),
        // eased on the way back out.
        let progress = match self.phase {
            HoldPhase::Holding => {
                let started = *self.hold_started.get_or_insert(now);
                let elapsed = now.saturating_sub(started);
                let progress = if self.hold_duration.is_zero() {
                    1.0
                } else {
                    (elapsed.as_secs_f64() / self.hold_duration.as_secs_f64()).clamp(0.0, 1.0)
                };
                self.fill.jump_to(progress);
                if progress >= 1.0 {
                    // Latched here and drained on the next event pass, which is
                    // the only pass that can reach app state.
                    self.phase = HoldPhase::Completed;
                    self.completion_pending = true;
                }
                progress
            }
            HoldPhase::Completed => {
                self.fill.jump_to(1.0);
                1.0
            }
            HoldPhase::Idle => {
                if reduce {
                    self.fill.jump_to(0.0);
                    0.0
                } else {
                    self.fill.set_target(0.0);
                    self.fill.advance(now)
                }
            }
        };

        let target_scale = if !dim && self.phase == HoldPhase::Holding {
            HOLD_PRESS_SCALE
        } else {
            1.0
        };
        let scale = if reduce {
            self.scale.jump_to(1.0);
            1.0
        } else {
            self.scale.set_target(target_scale);
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

        let radius = resolve_radius(CTA_RADIUS, size.width, size.height);
        scene.fill_rounded_rect(
            origin,
            size,
            radius,
            disabled_tint(paint.track, dim, DISABLED_OPACITY),
        );

        // The fill, clipped to the track's own rounded box — the clip upstream
        // needs a `clip-path` for, since the fill is composited separately.
        if progress > 0.0 {
            scene.push_clip_rounded(origin, size, radius);
            let fill_color = disabled_tint(paint.fill, dim, DISABLED_OPACITY);
            match self.direction {
                HoldActionDirection::Vertical => {
                    let height = size.height * progress;
                    scene.fill_rect(
                        Point::new(origin.x, origin.y + size.height - height),
                        Size::new(size.width, height),
                        fill_color,
                    );
                }
                HoldActionDirection::Horizontal => {
                    scene.fill_rect(
                        origin,
                        Size::new(size.width * progress, size.height),
                        fill_color,
                    );
                }
            }
            scene.pop_clip();
        }

        // One label per phase, cross-fading in place.
        let ink = disabled_tint(paint.ink, dim, DISABLED_OPACITY);
        let label = self.label();
        let label_size = self.label_sizes[self.phase as usize];
        let at = Point::new(
            origin.x + (size.width - label_size.width) / 2.0,
            origin.y + (size.height - label_size.height) / 2.0,
        );
        label.paint(at, ink, scene);

        if scale != 1.0 {
            scene.pop_transform();
        }

        let running = self.phase == HoldPhase::Holding || self.fill.is_animating();
        if running || (!reduce && self.scale.is_animating()) {
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
        // The deferred completion, drained before the phase arms below so a
        // release that follows a completed hold still reports it. Never on a
        // `Cancel`, which may not reach app state at all.
        if self.completion_pending && p.phase != PointerPhase::Cancel {
            self.completion_pending = false;
            (self.on_complete)(ctx);
            ctx.request_redraw();
        }
        let size = ctx.size();
        match p.phase {
            PointerPhase::Down => {
                if !presses(p) || !inside(p.position, size) {
                    return EventResult::Ignored;
                }
                self.captured = true;
                self.phase = HoldPhase::Holding;
                self.hold_started = None;
                ctx.capture_pointer();
                ctx.request_focus();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if !self.captured {
                    if inside(p.position, size) {
                        ctx.claim_hover();
                        ctx.set_cursor(ACTIVE_CURSOR);
                    }
                    return EventResult::Ignored;
                }
                ctx.set_cursor(ACTIVE_CURSOR);
                // Leaving the button abandons the hold, wherever the pointer
                // then goes — the same measured leave upstream performs, since a
                // captured pointer announces no boundary.
                if !inside(p.position, size) && self.phase == HoldPhase::Holding {
                    self.cancel();
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.captured = false;
                self.cancel();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.captured = false;
                // A `Cancel` arm may only clear its own flags — the hold's
                // completion latch is state-bearing, so it is dropped rather
                // than reported here.
                self.completion_pending = false;
                self.cancel();
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::Button, |node| {
            node.set_label(self.labels[HoldPhase::Idle as usize].content());
            if self.disabled {
                node.set_disabled();
            } else {
                node.add_action(Action::Click);
            }
        });
    }
}

// =============================================================================
// Slide action button
// =============================================================================

/// The slider's resolved roles.
#[derive(Clone, Copy)]
struct SlidePaint {
    track: Color,
    fill: Color,
    ink: Color,
    on_fill: Color,
    thumb: Color,
    thumb_ink: Color,
    done_thumb: Color,
    done_thumb_ink: Color,
}

impl SlidePaint {
    fn resolve(theme: Option<&Theme>) -> Self {
        let scheme = theme.map(Theme::scheme);
        macro_rules! role {
            ($role:ident) => {
                scheme.map_or(color_scheme_light().$role, |s| s.$role)
            };
        }
        Self {
            track: with_alpha(role!(primary), SLIDE_TRACK_ALPHA),
            fill: role!(primary),
            ink: role!(on_surface),
            on_fill: role!(on_primary),
            thumb: role!(primary),
            thumb_ink: role!(on_primary),
            done_thumb: role!(surface),
            done_thumb_ink: role!(on_surface),
        }
    }
}

/// A declarative drag-to-confirm slider.
pub struct SlideActionButtonView<State: 'static> {
    label: String,
    complete_label: String,
    threshold: f64,
    reset_delay: Duration,
    on_complete: Rc<dyn Fn(&mut State)>,
}

/// A slider whose thumb must be dragged past
/// [`SLIDE_THRESHOLD`] of its travel to run `on_complete`; a release short of
/// that springs the thumb back and reports nothing.
pub fn slide_action_button<State: 'static>(
    label: impl Into<String>,
    on_complete: impl Fn(&mut State) + 'static,
) -> SlideActionButtonView<State> {
    SlideActionButtonView {
        label: label.into(),
        complete_label: "Complete".to_string(),
        threshold: SLIDE_THRESHOLD,
        reset_delay: SLIDE_RESET_DELAY,
        on_complete: Rc::new(on_complete),
    }
}

impl<State: 'static> SlideActionButtonView<State> {
    /// The label shown after a confirmed slide (default `"Complete"`).
    pub fn complete_label(mut self, label: impl Into<String>) -> Self {
        self.complete_label = label.into();
        self
    }

    /// The fraction of the travel a release must be past to confirm (default
    /// [`SLIDE_THRESHOLD`]). Clamped into `0.0..=1.0`.
    pub fn threshold(mut self, threshold: f64) -> Self {
        self.threshold = threshold.clamp(0.0, 1.0);
        self
    }

    /// How long a confirmed slider rests before returning to its start (default
    /// [`SLIDE_RESET_DELAY`]).
    pub fn reset_delay(mut self, delay: Duration) -> Self {
        self.reset_delay = delay;
        self
    }
}

/// The retained widget for a [`SlideActionButtonView`].
pub struct SlideActionButtonWidget {
    label: LabelRun,
    complete_label: LabelRun,
    threshold: f64,
    reset_delay: Duration,
    on_complete: ErasedCallback,

    /// The thumb's travel in logical px, and the spring that returns it.
    offset: SpringScalar,
    scale: SpringScalar,
    dragging: bool,
    /// Where inside the thumb the drag began, so the thumb does not jump to the
    /// pointer on the first move.
    grab: f64,
    completed: bool,
    completed_at: Option<FrameTime>,
    label_size: Size,
    complete_size: Size,
    /// The travel the last layout allowed, so the event pass can clamp without
    /// re-deriving it.
    travel: f64,
}

impl SlideActionButtonWidget {
    /// The travel available inside a `width`-wide track:
    /// `track - thumb - 8`, floored at zero.
    fn travel_for(width: f64) -> f64 {
        (width - SLIDE_THUMB - SLIDE_END_SLACK).max(0.0)
    }

    /// How far along its travel the thumb is, in `0..=1`.
    fn progress(&self) -> f64 {
        if self.travel <= 0.0 {
            return 0.0;
        }
        (self.offset.value() / self.travel).clamp(0.0, 1.0)
    }
}

impl<State: 'static> View<State> for SlideActionButtonView<State> {
    type Element = SlideActionButtonWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> SlideActionButtonWidget {
        SlideActionButtonWidget {
            label: LabelRun::new(self.label.clone()),
            complete_label: LabelRun::new(self.complete_label.clone()),
            threshold: self.threshold,
            reset_delay: self.reset_delay,
            on_complete: erase_callback(&self.on_complete),
            offset: SpringScalar::new(0.0, Ramp::spring(SPRING_LAYOUT)),
            scale: SpringScalar::new(1.0, Ramp::spring(SPRING_PRESS)),
            dragging: false,
            grab: 0.0,
            completed: false,
            completed_at: None,
            label_size: Size::ZERO,
            complete_size: Size::ZERO,
            travel: 0.0,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SlideActionButtonWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_complete = erase_callback(&self.on_complete);
        let mut flags = ChangeFlags::NONE;
        if prev.label != self.label {
            element.label.set_content(self.label.clone());
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.complete_label != self.complete_label {
            element
                .complete_label
                .set_content(self.complete_label.clone());
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        element.threshold = self.threshold;
        element.reset_delay = self.reset_delay;
        flags
    }
}

impl Widget for SlideActionButtonWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let style = label_style(TEXT_SM);
        self.label_size = self.label.layout_themed(ctx, &style, LABEL_ROLE);
        self.complete_size = self.complete_label.layout_themed(ctx, &style, LABEL_ROLE);
        let size = bc.constrain(Size::new(CTA_MIN_WIDTH, CTA_HEIGHT));
        self.travel = Self::travel_for(size.width);
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let paint = SlidePaint::resolve(theme);
        let now = ctx.frame_time();
        let size = ctx.size();
        let origin = ctx.origin();
        self.travel = Self::travel_for(size.width);

        // A confirmed slider returns to its start once the delay is up. No
        // callback is owed for that, so it happens here rather than being
        // latched for an event pass.
        if self.completed {
            let since = *self.completed_at.get_or_insert(now);
            if now.saturating_sub(since) >= self.reset_delay {
                self.completed = false;
                self.completed_at = None;
                if reduce {
                    self.offset.jump_to(0.0);
                } else {
                    self.offset.set_target(0.0);
                }
            }
        }
        let offset = if reduce && !self.dragging {
            self.offset.jump_to(self.offset.target());
            self.offset.value()
        } else {
            self.offset.advance(now)
        };
        let progress = self.progress();

        let radius = resolve_radius(CTA_RADIUS, size.width, size.height);
        scene.fill_rounded_rect(origin, size, radius, paint.track);
        stroke_outline(scene, origin, size, radius, paint.track);

        // The fill grows with the drag (`scaleX(fillProgress)` from the left).
        if progress > 0.0 {
            scene.push_clip_rounded(origin, size, radius);
            scene.fill_rect(
                origin,
                Size::new(size.width * progress, size.height),
                paint.fill,
            );
            scene.pop_clip();
        }

        // The instruction label fades out over the first two thirds of the
        // travel; the completion label replaces it once confirmed.
        let (label, label_size, ink, alpha) = if self.completed {
            (
                &self.complete_label,
                self.complete_size,
                paint.on_fill,
                1.0f64,
            )
        } else {
            let fade = (1.0 - progress / SLIDE_LABEL_FADED).clamp(0.0, 1.0);
            (&self.label, self.label_size, paint.ink, fade)
        };
        let label_at = Point::new(
            origin.x + (size.width - label_size.width) / 2.0,
            origin.y + (size.height - label_size.height) / 2.0,
        );
        scene.push_layer(label_at, label_size, alpha as f32);
        label.paint(label_at, ink, scene);
        scene.pop_layer();

        // The thumb.
        let target_scale = if self.dragging && !self.completed {
            SLIDE_PRESS_SCALE
        } else {
            1.0
        };
        let scale = if reduce {
            self.scale.jump_to(1.0);
            1.0
        } else {
            self.scale.set_target(target_scale);
            self.scale.advance(now)
        };
        let thumb_origin = Point::new(
            origin.x + SLIDE_PAD + offset,
            origin.y + (size.height - SLIDE_THUMB) / 2.0,
        );
        let thumb_size = Size::new(SLIDE_THUMB, SLIDE_THUMB);
        let thumb_centre = thumb_origin + Vec2::new(SLIDE_THUMB / 2.0, SLIDE_THUMB / 2.0);
        if scale != 1.0 {
            scene.push_transform(
                Affine::translate(thumb_centre.to_vec2())
                    * Affine::scale(scale)
                    * Affine::translate(-thumb_centre.to_vec2()),
            );
        }
        let (thumb_fill, thumb_ink) = if self.completed {
            (paint.done_thumb, paint.done_thumb_ink)
        } else {
            (paint.thumb, paint.thumb_ink)
        };
        scene.fill_rounded_rect(thumb_origin, thumb_size, SLIDE_THUMB_RADIUS, thumb_fill);
        paint_slide_glyph(scene, thumb_centre, progress, thumb_ink);
        if scale != 1.0 {
            scene.pop_transform();
        }

        if self.offset.is_animating() || self.scale.is_animating() || self.completed {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        let size = ctx.size();
        self.travel = Self::travel_for(size.width);
        let thumb_left = SLIDE_PAD + self.offset.value();
        let on_thumb = p.position.x >= thumb_left
            && p.position.x < thumb_left + SLIDE_THUMB
            && inside(p.position, size);

        match p.phase {
            PointerPhase::Down => {
                if !presses(p) || !on_thumb || self.completed {
                    return EventResult::Ignored;
                }
                self.dragging = true;
                self.grab = p.position.x - thumb_left;
                ctx.capture_pointer();
                ctx.request_focus();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if !self.dragging {
                    if on_thumb {
                        ctx.claim_hover();
                        ctx.set_cursor(ACTIVE_CURSOR);
                    }
                    return EventResult::Ignored;
                }
                ctx.set_cursor(ACTIVE_CURSOR);
                let wanted = (p.position.x - self.grab - SLIDE_PAD).clamp(0.0, self.travel);
                // A live drag tracks the pointer exactly; the spring is what
                // returns or completes it.
                self.offset.jump_to(wanted);
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.dragging {
                    return EventResult::Ignored;
                }
                self.dragging = false;
                if self.travel > 0.0 && self.offset.value() >= self.travel * self.threshold {
                    self.completed = true;
                    self.completed_at = None;
                    self.offset.set_target(self.travel);
                    (self.on_complete)(ctx);
                } else {
                    self.offset.set_target(0.0);
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.dragging {
                    return EventResult::Ignored;
                }
                self.dragging = false;
                self.offset.set_target(0.0);
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::Button, |node| {
            node.set_label(self.label.content());
            node.add_action(Action::Click);
        });
    }
}

/// Paint the thumb's morphing glyph: the three-point polyline interpolated
/// between `SLIDE_GLYPH`'s keyframes by `progress`.
fn paint_slide_glyph(scene: &mut dyn PaintScene, centre: Point, progress: f64, ink: Color) {
    let t = progress.clamp(0.0, 1.0);
    let (a, b, blend) = if t < 0.5 {
        (SLIDE_GLYPH[0], SLIDE_GLYPH[1], t * 2.0)
    } else {
        (SLIDE_GLYPH[1], SLIDE_GLYPH[2], (t - 0.5) * 2.0)
    };
    // The keyframes are authored in a 24-unit box drawn at `size-5`.
    let unit = SLIDE_GLYPH_BOX / 24.0;
    let mut path = BezPath::new();
    for (index, ((ax, ay), (bx, by))) in a.iter().zip(b.iter()).enumerate() {
        let x = centre.x + (ax + (bx - ax) * blend - 12.0) * unit;
        let y = centre.y + (ay + (by - ay) * blend - 12.0) * unit;
        if index == 0 {
            path.move_to(Point::new(x, y));
        } else {
            path.line_to(Point::new(x, y));
        }
    }
    scene.stroke_path(Point::ORIGIN, &path, SLIDE_GLYPH_STROKE, &Brush::Solid(ink));
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::PointerButton;
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::text::TextContext;
    use std::any::Any;

    #[derive(Default)]
    struct Fired {
        count: u32,
    }

    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        rects: Vec<(Point, Size, Color)>,
        strokes: Vec<Color>,
        inks: Vec<Color>,
        alphas: Vec<f32>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, o: Point, s: Size, color: Color) {
            self.rects.push((o, s, color));
        }
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

    fn dispatch<W: Widget>(w: &mut W, state: &mut Fired, size: Size, event: &InputEvent) {
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
        theme: Option<&Theme>,
        ms: u64,
    ) -> (Recorder, bool) {
        let mut ctx =
            PaintCtx::for_test(Point::ORIGIN, size, FrameTime::from_nanos(ms * 1_000_000));
        if let Some(theme) = theme {
            ctx = ctx.with_theme(theme);
        }
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

    // ---- Expanding arrow ----------------------------------------------------

    fn arrow(view: &ExpandingArrowButtonView<Fired>) -> ExpandingArrowButtonWidget {
        let mut next_id = 0u64;
        View::<Fired>::build(view, &mut BuildCtx::new(&mut next_id))
    }

    #[test]
    fn the_arrow_button_takes_the_cta_metrics_and_its_label_widens_it() {
        let view = expanding_arrow_button::<Fired>("Go", |_| {});
        let mut w = arrow(&view);
        let size = lay_out(&mut w);
        assert_eq!(size, Size::new(CTA_MIN_WIDTH, CTA_HEIGHT));

        let long = expanding_arrow_button::<Fired>(
            "A call to action long enough to outgrow the minimum",
            |_| {},
        );
        let mut wide = arrow(&long);
        let wide_size = lay_out(&mut wide);
        assert!(wide_size.width > CTA_MIN_WIDTH);
        assert_eq!(wide_size.height, CTA_HEIGHT);
    }

    /// At rest the tile is the collapsed 52px block with one chevron in it.
    #[test]
    fn the_resting_tile_is_collapsed_and_shows_one_chevron() {
        let view = expanding_arrow_button::<Fired>("Go", |_| {});
        let mut w = arrow(&view);
        let size = lay_out(&mut w);
        let (rec, _) = paint_at(&mut w, size, Some(&theme()), 0);
        // Track, tile, then the chevron's five dots.
        assert_eq!(rec.rrects.len(), 2 + CHEVRON_DOTS.len());
        assert_eq!(rec.rrects[1].1.width, ARROW_TILE_COLLAPSED);
        assert_eq!(rec.rrects[1].1.height, CTA_HEIGHT - ARROW_PAD * 2.0);
    }

    /// Hovering expands the tile into the five-chevron trail and opens it to
    /// the button's full inner width.
    ///
    /// Driven through a real `RenderRoot`, because the hover link this widget
    /// self-corrects from (`PaintCtx::is_hovered`) is seeded by the root and
    /// reads `false` on any synthetic context.
    #[test]
    fn hovering_expands_the_tile_into_the_five_chevron_trail() {
        let mut root: frust_core::RenderRoot<Fired, ExpandingArrowButtonView<Fired>> =
            frust_core::RenderRoot::new();
        root.set_theme(Box::new(theme()));
        let mut state = Fired::default();
        let mut text_ctx = TextContext::new();
        let mut app = |_s: &mut Fired| expanding_arrow_button::<Fired>("Go", |_| {});
        root.rebuild(&mut app, &mut state);
        let size = root.layout_with_text(Size::new(400.0, 200.0), &mut text_ctx as &mut dyn Any);

        let mut rec = Recorder::default();
        root.paint(&mut rec, FrameTime::ZERO);
        assert_eq!(
            rec.rrects.len(),
            2 + CHEVRON_DOTS.len(),
            "one chevron while collapsed"
        );

        root.event(&mut state, &ev(PointerPhase::Move, 20.0, 32.0));
        // One frame to latch the change, then well into the stagger.
        let mut latch = Recorder::default();
        root.paint(&mut latch, FrameTime::ZERO);
        let mut rec = Recorder::default();
        root.paint(&mut rec, FrameTime::from_nanos(600_000_000));

        assert_eq!(
            rec.rrects.len() - 2,
            CHEVRON_DOTS.len() * ARROW_TRAIL_ALPHA.len(),
            "five chevrons of five dots each"
        );
        assert!(
            rec.rrects[1].1.width > ARROW_TILE_COLLAPSED * 2.0,
            "the tile opened: {:?}",
            rec.rrects[1].1
        );
        assert!(rec.rrects[1].1.width <= size.width - ARROW_PAD * 2.0);
    }

    #[test]
    fn the_arrow_button_fires_on_an_up_inside_and_never_on_a_cancel() {
        let view = expanding_arrow_button::<Fired>("Go", |s: &mut Fired| s.count += 1);
        let mut w = arrow(&view);
        let size = lay_out(&mut w);
        let mut state = Fired::default();

        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Down, 10.0, 10.0),
        );
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Up, 10.0, 10.0));
        assert_eq!(state.count, 1);

        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Down, 10.0, 10.0),
        );
        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Cancel, 10.0, 10.0),
        );
        assert_eq!(state.count, 1);
    }

    #[test]
    fn a_disabled_arrow_button_neither_fires_nor_expands() {
        let view =
            expanding_arrow_button::<Fired>("Go", |s: &mut Fired| s.count += 1).disabled(true);
        let mut w = arrow(&view);
        let size = lay_out(&mut w);
        let mut state = Fired::default();
        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Down, 10.0, 10.0),
        );
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Up, 10.0, 10.0));
        assert_eq!(state.count, 0);
        paint_at(&mut w, size, Some(&theme()), 0);
        assert!(!w.active);
    }

    // ---- Hold action --------------------------------------------------------

    fn hold(view: &HoldActionButtonView<Fired>) -> HoldActionButtonWidget {
        let mut next_id = 0u64;
        View::<Fired>::build(view, &mut BuildCtx::new(&mut next_id))
    }

    /// The whole hold machine: press starts it, the fill runs linearly, the
    /// completion latches at the top and is reported on the release.
    #[test]
    fn a_completed_hold_reports_itself_exactly_once() {
        let view = hold_action_button::<Fired>("Delete", |s: &mut Fired| s.count += 1);
        let mut w = hold(&view);
        let size = lay_out(&mut w);
        let mut state = Fired::default();

        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Down, 20.0, 20.0),
        );
        assert_eq!(w.phase, HoldPhase::Holding);

        let (rec, more) = paint_at(&mut w, size, Some(&theme()), 0);
        assert!(more, "a running hold owes frames");
        assert!(rec.rects.is_empty(), "nothing filled on the first frame");

        // Half way: the fill covers half the track's height.
        let half = HOLD_DURATION.as_millis() as u64 / 2;
        let (rec, _) = paint_at(&mut w, size, Some(&theme()), half);
        assert_eq!(rec.rects.len(), 1);
        assert!((rec.rects[0].1.height - CTA_HEIGHT / 2.0).abs() < 1.0);
        assert_eq!(state.count, 0, "not yet");

        // At the top the completion latches, and the label is the complete one.
        paint_at(
            &mut w,
            size,
            Some(&theme()),
            HOLD_DURATION.as_millis() as u64,
        );
        assert_eq!(w.phase, HoldPhase::Completed);
        assert!(w.completion_pending);
        assert_eq!(w.label().content(), "Done");

        // The release drains it — once.
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Up, 20.0, 20.0));
        assert_eq!(state.count, 1);
        assert!(!w.completion_pending);
        assert_eq!(w.phase, HoldPhase::Idle);
        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Move, 20.0, 20.0),
        );
        assert_eq!(state.count, 1, "and never again");
    }

    /// The acceptance criterion: releasing early cancels, silently.
    #[test]
    fn an_early_release_cancels_without_reporting() {
        let view = hold_action_button::<Fired>("Delete", |s: &mut Fired| s.count += 1);
        let mut w = hold(&view);
        let size = lay_out(&mut w);
        let mut state = Fired::default();

        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Down, 20.0, 20.0),
        );
        paint_at(&mut w, size, Some(&theme()), 0);
        paint_at(&mut w, size, Some(&theme()), 200);
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Up, 20.0, 20.0));
        assert_eq!(state.count, 0);
        assert_eq!(w.phase, HoldPhase::Idle);
        assert!(!w.completion_pending);

        // ...and the fill retracts rather than snapping away.
        let (_, more) = paint_at(&mut w, size, Some(&theme()), 210);
        assert!(more, "the retraction is animated");
    }

    /// Sliding off the button abandons the hold, because a captured pointer
    /// announces no boundary crossing of its own.
    #[test]
    fn moving_off_the_button_abandons_the_hold() {
        let view = hold_action_button::<Fired>("Delete", |s: &mut Fired| s.count += 1);
        let mut w = hold(&view);
        let size = lay_out(&mut w);
        let mut state = Fired::default();

        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Down, 20.0, 20.0),
        );
        paint_at(&mut w, size, Some(&theme()), 0);
        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Move, size.width + 50.0, 20.0),
        );
        assert_eq!(w.phase, HoldPhase::Idle);
        paint_at(
            &mut w,
            size,
            Some(&theme()),
            HOLD_DURATION.as_millis() as u64,
        );
        assert_eq!(state.count, 0);
    }

    /// A cancelled gesture drops the latch rather than reporting from an arm
    /// that may not touch state.
    #[test]
    fn a_cancel_drops_a_pending_completion() {
        let view = hold_action_button::<Fired>("Delete", |s: &mut Fired| s.count += 1);
        let mut w = hold(&view);
        let size = lay_out(&mut w);
        let mut state = Fired::default();

        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Down, 20.0, 20.0),
        );
        paint_at(&mut w, size, Some(&theme()), 0);
        paint_at(
            &mut w,
            size,
            Some(&theme()),
            HOLD_DURATION.as_millis() as u64,
        );
        assert!(w.completion_pending);
        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Cancel, 20.0, 20.0),
        );
        assert_eq!(state.count, 0);
        assert!(!w.completion_pending);
    }

    /// The horizontal direction fills from the left rather than from below.
    #[test]
    fn the_horizontal_direction_fills_across_instead_of_up() {
        let view = hold_action_button::<Fired>("Delete", |_| {})
            .direction(HoldActionDirection::Horizontal);
        let mut w = hold(&view);
        let size = lay_out(&mut w);
        let mut state = Fired::default();
        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Down, 20.0, 20.0),
        );
        paint_at(&mut w, size, Some(&theme()), 0);
        let (rec, _) = paint_at(&mut w, size, Some(&theme()), 800);
        assert_eq!(rec.rects[0].1.height, CTA_HEIGHT, "full height");
        assert!(rec.rects[0].1.width < size.width, "part width");
    }

    /// A zero-length hold completes on the frame it starts rather than dividing
    /// by zero.
    #[test]
    fn a_zero_length_hold_completes_immediately() {
        let view = hold_action_button::<Fired>("Delete", |s: &mut Fired| s.count += 1)
            .hold_duration(Duration::ZERO);
        let mut w = hold(&view);
        let size = lay_out(&mut w);
        let mut state = Fired::default();
        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Down, 20.0, 20.0),
        );
        paint_at(&mut w, size, Some(&theme()), 0);
        assert_eq!(w.phase, HoldPhase::Completed);
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Up, 20.0, 20.0));
        assert_eq!(state.count, 1);
    }

    // ---- Slide action -------------------------------------------------------

    fn slide(view: &SlideActionButtonView<Fired>) -> SlideActionButtonWidget {
        let mut next_id = 0u64;
        View::<Fired>::build(view, &mut BuildCtx::new(&mut next_id))
    }

    /// The acceptance criterion: a release past the threshold confirms, and one
    /// short of it springs back reporting nothing.
    #[test]
    fn the_slider_confirms_only_past_its_threshold() {
        let view = slide_action_button::<Fired>("Slide to pay", |s: &mut Fired| s.count += 1);
        let mut w = slide(&view);
        let size = lay_out(&mut w);
        let mut state = Fired::default();
        let travel = SlideActionButtonWidget::travel_for(size.width);
        let mid = SLIDE_THUMB / 2.0;

        // Short of the threshold: back to the start, nothing reported.
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Down, mid, 32.0));
        assert!(w.dragging);
        let short = travel * (SLIDE_THRESHOLD - 0.1);
        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Move, mid + short, 32.0),
        );
        assert!((w.offset.value() - short).abs() < 1.0);
        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Up, mid + short, 32.0),
        );
        assert_eq!(state.count, 0);
        assert_eq!(w.offset.target(), 0.0, "it springs back");
        assert!(!w.completed);
        // Let the return spring settle, so the thumb is back under the grab
        // point the next gesture starts from. The first frame after a retarget
        // only latches the ramp's start; the second is the one that runs it.
        paint_at(&mut w, size, Some(&theme()), 5_000);
        paint_at(&mut w, size, Some(&theme()), 6_000);
        assert_eq!(w.offset.value(), 0.0);

        // Past it: confirmed, once.
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Down, mid, 32.0));
        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Move, mid + travel * 2.0, 32.0),
        );
        assert_eq!(w.offset.value(), travel, "clamped to the far end");
        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Up, mid + travel, 32.0),
        );
        assert_eq!(state.count, 1);
        assert!(w.completed);
    }

    /// The drag only starts on the thumb — a press on the bare track is not a
    /// grab.
    #[test]
    fn a_press_off_the_thumb_does_not_start_a_drag() {
        let view = slide_action_button::<Fired>("Slide", |_| {});
        let mut w = slide(&view);
        let size = lay_out(&mut w);
        let mut state = Fired::default();
        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Down, size.width - 10.0, 32.0),
        );
        assert!(!w.dragging);
    }

    /// A confirmed slider returns to its start once the reset delay is up, with
    /// no second callback.
    #[test]
    fn a_confirmed_slider_resets_itself_after_the_delay() {
        let view = slide_action_button::<Fired>("Slide", |s: &mut Fired| s.count += 1)
            .reset_delay(Duration::from_millis(300));
        let mut w = slide(&view);
        let size = lay_out(&mut w);
        let mut state = Fired::default();
        let travel = SlideActionButtonWidget::travel_for(size.width);
        let mid = SLIDE_THUMB / 2.0;

        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Down, mid, 32.0));
        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Move, mid + travel, 32.0),
        );
        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Up, mid + travel, 32.0),
        );
        assert!(w.completed);

        paint_at(&mut w, size, Some(&theme()), 0);
        assert!(w.completed, "still resting at the far end");
        paint_at(&mut w, size, Some(&theme()), 400);
        assert!(!w.completed);
        assert_eq!(w.offset.target(), 0.0);
        assert_eq!(state.count, 1, "the reset reports nothing of its own");
    }

    /// The track fill and the label fade both track the drag.
    #[test]
    fn the_fill_grows_and_the_label_fades_with_the_drag() {
        let view = slide_action_button::<Fired>("Slide", |_| {});
        let mut w = slide(&view);
        let size = lay_out(&mut w);
        let mut state = Fired::default();
        let travel = SlideActionButtonWidget::travel_for(size.width);
        let mid = SLIDE_THUMB / 2.0;

        let (rec, _) = paint_at(&mut w, size, Some(&theme()), 0);
        assert!(rec.rects.is_empty(), "no fill at the start");
        assert_eq!(rec.alphas[0], 1.0, "the label is fully legible");

        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Down, mid, 32.0));
        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Move, mid + travel, 32.0),
        );
        let (rec, _) = paint_at(&mut w, size, Some(&theme()), 10);
        assert!(rec.rects[0].1.width > size.width * 0.5);
        assert_eq!(rec.alphas[0], 0.0, "the label is gone by two thirds");
    }

    /// The thumb glyph morphs from the chevron to the tick as the drag runs.
    #[test]
    fn the_thumb_glyph_morphs_across_the_drag() {
        let start = glyph_points(0.0);
        let middle = glyph_points(0.5);
        let end = glyph_points(1.0);
        assert_eq!(start.len(), 3);
        assert_ne!(start, middle);
        assert_ne!(middle, end);
        // The finished glyph is a tick: its last point is the highest of the
        // three, which the resting chevron's is not.
        assert!(end[2].1 < end[1].1);
        assert!(start[2].1 > start[1].1);
    }

    /// The glyph's three interpolated points at `progress`, in widget space
    /// around an origin-centred thumb.
    fn glyph_points(progress: f64) -> Vec<(f64, f64)> {
        #[derive(Default)]
        struct Points(Vec<(f64, f64)>);
        impl PaintScene for Points {
            fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
            fn draw_text(&mut self, _o: Point, _t: &str) {}
            fn stroke_path(&mut self, _o: Point, path: &BezPath, _w: f64, _b: &Brush) {
                for element in path.elements() {
                    match element {
                        frust::kurbo::PathEl::MoveTo(p) | frust::kurbo::PathEl::LineTo(p) => {
                            self.0.push((p.x, p.y));
                        }
                        _ => {}
                    }
                }
            }
        }
        let mut points = Points::default();
        paint_slide_glyph(&mut points, Point::ORIGIN, progress, Color::BLACK);
        points.0
    }

    /// Reduced motion collapses every one of the three: no frames owed once the
    /// gesture itself is over.
    #[test]
    fn reduced_motion_settles_all_three_without_owing_frames() {
        let reduced = reduced_theme();

        let mut arrow_widget = arrow(&expanding_arrow_button::<Fired>("Go", |_| {}));
        let size = lay_out(&mut arrow_widget);
        paint_at(&mut arrow_widget, size, Some(&reduced), 0);
        let (_, more) = paint_at(&mut arrow_widget, size, Some(&reduced), 5_000);
        assert!(!more, "the arrow rests");

        let mut hold_widget = hold(&hold_action_button::<Fired>("Delete", |_| {}));
        let size = lay_out(&mut hold_widget);
        let (_, more) = paint_at(&mut hold_widget, size, Some(&reduced), 0);
        assert!(!more, "an unheld button rests");

        let mut slide_widget = slide(&slide_action_button::<Fired>("Slide", |_| {}));
        let size = lay_out(&mut slide_widget);
        let (_, more) = paint_at(&mut slide_widget, size, Some(&reduced), 0);
        assert!(!more, "an undragged slider rests");
    }

    // ---- Typeface: the three CTAs' labels follow the live theme -------------

    use crate::text::typeface_probe::{
        Face, Probe, assert_all, assert_control, assert_follows_a_live_family_swap,
        assert_follows_a_live_family_swap_on, assert_paints_only_in_geist,
    };

    const PROBE_WINDOW: Size = Size::new(400.0, 300.0);

    /// All three CTAs at rest, each painting its idle label.
    fn probe_view(_: &mut ()) -> frust::FlexView<()> {
        frust::column()
            .child(expanding_arrow_button::<()>("Continue", |_| {}))
            .child(hold_action_button::<()>("Delete", |_| {}))
            .child(slide_action_button::<()>("Slide", |_| {}))
    }

    #[test]
    fn idle_labels_paint_in_geist_under_the_beui_theme() {
        assert_paints_only_in_geist("the CTA labels", probe_view, PROBE_WINDOW);
    }

    #[test]
    fn idle_labels_follow_a_live_theme_family_swap() {
        assert_follows_a_live_family_swap("the CTA labels", probe_view, PROBE_WINDOW);
    }

    /// A slider dragged the whole way and released, so it paints its
    /// completion label — held there by a reset delay no probe frame reaches.
    fn completed_slider() -> Probe<frust::FlexView<()>, impl FnMut(&mut ()) -> frust::FlexView<()>>
    {
        let logic = |_: &mut ()| {
            frust::column().child(
                slide_action_button::<()>("Slide", |_| {})
                    .complete_label("Complete")
                    .reset_delay(Duration::from_secs(3_600)),
            )
        };
        let mut probe = Probe::new(logic, PROBE_WINDOW, crate::theme());
        probe.frame();
        let y = CTA_HEIGHT / 2.0;
        probe.event(&ev(PointerPhase::Down, SLIDE_PAD + SLIDE_THUMB / 2.0, y));
        probe.event(&ev(PointerPhase::Move, CTA_MIN_WIDTH - 1.0, y));
        probe.event(&ev(PointerPhase::Up, CTA_MIN_WIDTH - 1.0, y));
        probe
    }

    #[test]
    fn the_completion_label_paints_in_geist_under_the_beui_theme() {
        assert_control("the slider's completion label", PROBE_WINDOW);
        assert_all(
            "the slider's completion label",
            "under the beUI theme",
            &completed_slider().frame(),
            Face::Geist,
        );
    }

    #[test]
    fn the_completion_label_follows_a_live_theme_family_swap() {
        assert_follows_a_live_family_swap_on(
            "the slider's completion label",
            &mut completed_slider(),
        );
    }
}
