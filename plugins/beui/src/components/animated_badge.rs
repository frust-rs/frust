//! Ports beUI's `animated-badge` component.
//!
//! **Source:** `components/motion/animated-badge.tsx`, beUI rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01.
//!
//! # What the component actually is
//!
//! A **status** badge, not a count badge: a pill carrying one of six
//! [`AnimatedBadgeStatus`] tones, an optional status icon, and a label — and the
//! motion is a *roll on change*. When the status changes the icon rolls up out of
//! its slot as the new one rolls in from below; when the label changes the label
//! does the same. `loading` additionally spins its icon and pulses the pill.
//!
//! | class | here |
//! |---|---|
//! | `rounded-full border font-medium tabular-nums` | [`crate::style::RADIUS_CONTROL`] + [`crate::style::BORDER_WIDTH`] |
//! | `h-6 gap-1.5 px-2 text-[11px]` | [`AnimatedBadgeSize::Sm`] |
//! | `h-8 gap-2 px-3 text-xs` | [`AnimatedBadgeSize::Md`] |
//! | `border-*/30 bg-*/10` | [`BORDER_ALPHA`] / [`FILL_ALPHA`] |
//! | `bg-current opacity-10`, `scale: [0.94, 1.08, 0.94]` | the pulse |
//! | `overflow-hidden` on both slots | the roll clips |
//!
//! # Degradations against upstream
//!
//! - **No blur on either roll.** Both `ICON_ROLL_VARIANTS` and
//!   `TEXT_ROLL_VARIANTS` animate `filter: blur(6px) → blur(0px)`; frust's scene
//!   has no blur primitive, so the roll keeps its translation, opacity and (for
//!   the icon) its scale, and drops the blur.
//! - **No icon rotation on the roll.** The icon variant also tweens
//!   `rotate: -8deg → 0`; that is dropped with the blur rather than half-ported,
//!   since the roll reads as a roll without it. The `loading` spin is a separate
//!   animation and *is* ported.
//! - **The roll springs are substituted.** Upstream authors per-component
//!   springs (`{stiffness: 210, damping: 24, mass: 0.85}` for the travel,
//!   `{250, 24, 0.75}` for the scale); neither is one of
//!   [`crate::tokens::motion`]'s six, so both rolls run on
//!   [`SPRING_SWAP`](crate::tokens::motion::SPRING_SWAP) — which is the catalog's
//!   own "content swapping inside a control" spring, i.e. exactly this.
//! - **The status hues are beUI's own.** Upstream hardcodes Tailwind's
//!   `emerald-500`/`amber-500` for success/warning; this port reads
//!   [`BeuiTokens`]' authored `--success`/`--warning` instead, because the
//!   catalog resolves colour from tokens only.
//! - **No layout spring on the pill.** Upstream wraps the badge in
//!   `motion.span layout`, so a width change glides. Here the pill takes its new
//!   width immediately and only its *contents* roll; a layout-animating
//!   container is a framework capability this catalog does not have.
//! - **Icons are drawn, not Lucide glyphs.** There is no bundled icon set, so
//!   each status draws the shape its Lucide counterpart resolves to — the same
//!   route `frust_shadcn::spinner` takes.

use std::f64::consts::{PI, TAU};
use std::time::Duration;

use frust::authoring::text::{FontWeight, TextStyle};
use frust::authoring::{
    BezPath, BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, Role,
    SemanticsCtx, Shape, TickClass, View, Widget,
};
use frust::{FrameTime, Theme};
use kurbo::{Arc, Point, Size, Vec2};
use peniko::{Brush, Color};

use crate::motion::Ramp;
use crate::style::{
    BORDER_WIDTH, PATH_TOLERANCE, RADIUS_CONTROL, TEXT_XS, resolve_radius, scale_alpha, spacing,
    with_alpha,
};
use crate::text::LabelRun;
use crate::tokens::motion::SPRING_SWAP;
use crate::tokens::{BEUI_LIGHT, BeuiTokens};

/// Alpha a status hue's border is painted at — `border-*/30`.
pub const BORDER_ALPHA: f32 = 0.30;

/// Alpha a status hue's fill is painted at — `bg-*/10`.
pub const FILL_ALPHA: f32 = 0.10;

/// How far a rolling label or icon travels, as a fraction of its slot —
/// `y: "85%"` on the text roll, `"80%"` on the icon roll; the port uses the
/// text value for both, since the two slots are the same height here.
const ROLL_TRAVEL: f64 = 0.85;

/// The alpha an entering part starts at — `opacity: 0.76`.
const ROLL_ENTER_ALPHA: f64 = 0.76;

/// How long the `loading` icon takes to turn once — `duration: 1, ease:
/// "linear"`.
const SPIN_PERIOD: Duration = Duration::from_millis(1000);

/// One full pulse — `duration: 1.6`.
const PULSE_PERIOD: Duration = Duration::from_millis(1600);

/// The pulse's scale range — `scale: [0.94, 1.08, 0.94]`.
const PULSE_SCALE: (f64, f64) = (0.94, 1.08);

/// The pulse's opacity range — `opacity: [0.08, 0.16, 0.08]`.
const PULSE_ALPHA: (f32, f32) = (0.08, 0.16);

/// The drawn sweep of the `loading` arc — three quarters of a turn, the shape a
/// `LoaderCircle` glyph resolves to.
const SPINNER_SWEEP: f64 = PI * 1.5;

/// Unthemed fallback ink (beUI light `--foreground`).
const FALLBACK_INK: Color = BEUI_LIGHT.muted_foreground;
/// Unthemed fallback surface (beUI light `--card`).
const FALLBACK_SURFACE: Color = BEUI_LIGHT.card;
/// Unthemed fallback hairline (beUI light `--border`).
const FALLBACK_BORDER: Color = BEUI_LIGHT.border;

/// The badge's tone — upstream's `AnimatedBadgeStatus`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AnimatedBadgeStatus {
    /// `neutral`: the card surface with a plain hairline and dimmed ink.
    #[default]
    Neutral,
    /// `info`: the primary hue.
    Info,
    /// `success`: beUI's `--success`.
    Success,
    /// `warning`: beUI's `--warning`.
    Warning,
    /// `danger`: the destructive hue.
    Danger,
    /// `loading`: the primary hue, with a spinning icon and a pulse.
    Loading,
}

impl AnimatedBadgeStatus {
    /// Every tone, in upstream's own declaration order.
    pub const ALL: [AnimatedBadgeStatus; 6] = [
        AnimatedBadgeStatus::Neutral,
        AnimatedBadgeStatus::Info,
        AnimatedBadgeStatus::Success,
        AnimatedBadgeStatus::Warning,
        AnimatedBadgeStatus::Danger,
        AnimatedBadgeStatus::Loading,
    ];

    /// Whether this tone pulses by default — upstream's
    /// `pulse = status === "loading"`.
    pub fn pulses_by_default(self) -> bool {
        self == AnimatedBadgeStatus::Loading
    }
}

/// The badge's size — upstream's `AnimatedBadgeSize`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AnimatedBadgeSize {
    /// `sm`: `h-6 gap-1.5 px-2 text-[11px]`.
    Sm,
    /// `md`: `h-8 gap-2 px-3 text-xs`.
    #[default]
    Md,
}

impl AnimatedBadgeSize {
    /// Both rungs, small first.
    pub const ALL: [AnimatedBadgeSize; 2] = [AnimatedBadgeSize::Sm, AnimatedBadgeSize::Md];

    /// The pill's height (`h-*`).
    pub fn height(self) -> f64 {
        match self {
            AnimatedBadgeSize::Sm => spacing(6.0),
            AnimatedBadgeSize::Md => spacing(8.0),
        }
    }

    /// Horizontal padding (`px-*`).
    pub fn padding_x(self) -> f64 {
        match self {
            AnimatedBadgeSize::Sm => spacing(2.0),
            AnimatedBadgeSize::Md => spacing(3.0),
        }
    }

    /// The gap between the icon and the label (`gap-*`).
    pub fn gap(self) -> f64 {
        match self {
            AnimatedBadgeSize::Sm => spacing(1.5),
            AnimatedBadgeSize::Md => spacing(2.0),
        }
    }

    /// The label's type size (`text-*`).
    pub fn text_size(self) -> f64 {
        match self {
            // `text-[11px]` — an arbitrary value upstream, not a ladder step.
            AnimatedBadgeSize::Sm => 11.0,
            AnimatedBadgeSize::Md => TEXT_XS,
        }
    }

    /// The icon's edge (`h-3 w-3` / `h-3.5 w-3.5`).
    pub fn icon_size(self) -> f64 {
        match self {
            AnimatedBadgeSize::Sm => spacing(3.0),
            AnimatedBadgeSize::Md => spacing(3.5),
        }
    }
}

/// A declarative beUI status badge. See the [module docs](self).
///
/// # Example
///
/// ```
/// use frust_beui::components::animated_badge::{AnimatedBadgeStatus, animated_badge};
///
/// let deploying = animated_badge::<()>("Deploying").status(AnimatedBadgeStatus::Loading);
/// ```
pub struct AnimatedBadgeView<State: 'static> {
    label: String,
    status: AnimatedBadgeStatus,
    size: AnimatedBadgeSize,
    show_icon: bool,
    pulse: Option<bool>,
    _state: std::marker::PhantomData<fn(&mut State)>,
}

/// Create a badge carrying `label` in the
/// [`Neutral`](AnimatedBadgeStatus::Neutral) tone.
pub fn animated_badge<State: 'static>(label: impl Into<String>) -> AnimatedBadgeView<State> {
    AnimatedBadgeView {
        label: label.into(),
        status: AnimatedBadgeStatus::default(),
        size: AnimatedBadgeSize::default(),
        show_icon: true,
        pulse: None,
        _state: std::marker::PhantomData,
    }
}

impl<State: 'static> AnimatedBadgeView<State> {
    /// Carry `status` instead of [`AnimatedBadgeStatus::Neutral`].
    pub fn status(mut self, status: AnimatedBadgeStatus) -> Self {
        self.status = status;
        self
    }

    /// Use `size` instead of [`AnimatedBadgeSize::Md`].
    pub fn size(mut self, size: AnimatedBadgeSize) -> Self {
        self.size = size;
        self
    }

    /// Show or hide the status icon (default shown — upstream's
    /// `showIcon = true`).
    pub fn show_icon(mut self, show_icon: bool) -> Self {
        self.show_icon = show_icon;
        self
    }

    /// Force the pulse on or off. Unset, it follows
    /// [`AnimatedBadgeStatus::pulses_by_default`].
    pub fn pulse(mut self, pulse: bool) -> Self {
        self.pulse = Some(pulse);
        self
    }

    /// Whether this badge will pulse.
    pub fn pulses(&self) -> bool {
        self.pulse
            .unwrap_or_else(|| self.status.pulses_by_default())
    }
}

/// The retained widget for an [`AnimatedBadgeView`].
pub struct AnimatedBadgeWidget {
    label: LabelRun,
    /// The label being rolled out, kept alive for the length of its exit.
    outgoing: Option<LabelRun>,
    status: AnimatedBadgeStatus,
    /// The status being rolled out, for the icon slot's own exit.
    outgoing_status: Option<AnimatedBadgeStatus>,
    size: AnimatedBadgeSize,
    show_icon: bool,
    pulse: Option<bool>,
    /// When the label roll started, latched on its first paint.
    label_roll: Option<FrameTime>,
    /// When the icon roll started.
    icon_roll: Option<FrameTime>,
    /// The clock the perpetual parts (spin, pulse) are timed against.
    started: Option<FrameTime>,
}

/// The resolved fill, border and ink for one status.
#[derive(Clone, Copy, Debug, PartialEq)]
struct BadgeColors {
    fill: Color,
    border: Color,
    ink: Color,
}

impl AnimatedBadgeWidget {
    /// The label this badge carries.
    pub fn label(&self) -> &str {
        self.label.content()
    }

    /// Its tone.
    pub fn status(&self) -> AnimatedBadgeStatus {
        self.status
    }

    /// Whether it pulses.
    pub fn pulses(&self) -> bool {
        self.pulse
            .unwrap_or_else(|| self.status.pulses_by_default())
    }

    /// The style the label is shaped with — `font-medium` at the size rung.
    fn label_style(&self, theme: Option<&Theme>) -> TextStyle {
        let family = theme.map_or_else(crate::tokens::sans_family, |t| {
            t.type_scale.label_small.family.clone()
        });
        TextStyle {
            family,
            weight: FontWeight::MEDIUM,
            // Shaped with an ink the paint pass overrides; keeping it constant
            // keeps the shape cache off the colour.
            ..TextStyle::new(self.size.text_size() as f32, Color::BLACK)
        }
    }

    /// The fill, border and ink `status` paints with.
    fn colors(&self, status: AnimatedBadgeStatus, theme: Option<&Theme>) -> BadgeColors {
        let tokens = BeuiTokens::resolve(theme);
        let scheme = theme.map(Theme::scheme);
        let hue =
            |pick: fn(&frust::ColorScheme) -> Color, fallback: Color| scheme.map_or(fallback, pick);
        match status {
            AnimatedBadgeStatus::Neutral => BadgeColors {
                fill: hue(|s| s.surface_container, FALLBACK_SURFACE),
                border: hue(|s| s.outline_variant, FALLBACK_BORDER),
                ink: hue(|s| s.on_surface_variant, FALLBACK_INK),
            },
            AnimatedBadgeStatus::Info | AnimatedBadgeStatus::Loading => {
                let hue = hue(|s| s.primary, BEUI_LIGHT.primary);
                BadgeColors {
                    fill: with_alpha(hue, FILL_ALPHA),
                    border: with_alpha(hue, BORDER_ALPHA),
                    ink: hue,
                }
            }
            AnimatedBadgeStatus::Success => BadgeColors {
                fill: with_alpha(tokens.success, FILL_ALPHA),
                border: with_alpha(tokens.success, BORDER_ALPHA),
                ink: tokens.success,
            },
            AnimatedBadgeStatus::Warning => BadgeColors {
                fill: with_alpha(tokens.warning, FILL_ALPHA),
                border: with_alpha(tokens.warning, BORDER_ALPHA),
                ink: tokens.warning,
            },
            AnimatedBadgeStatus::Danger => {
                let hue = hue(|s| s.error, BEUI_LIGHT.danger);
                BadgeColors {
                    fill: with_alpha(hue, FILL_ALPHA),
                    border: with_alpha(hue, BORDER_ALPHA),
                    ink: hue,
                }
            }
        }
    }
}

impl<State: 'static> View<State> for AnimatedBadgeView<State> {
    type Element = AnimatedBadgeWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> AnimatedBadgeWidget {
        AnimatedBadgeWidget {
            label: LabelRun::new(self.label.clone()),
            outgoing: None,
            status: self.status,
            outgoing_status: None,
            size: self.size,
            show_icon: self.show_icon,
            pulse: self.pulse,
            label_roll: None,
            icon_roll: None,
            started: None,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AnimatedBadgeWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if element.label.content() != self.label {
            // The old label is kept alive for the length of its exit — the
            // catalog's own presence rule, applied to one text slot.
            element.outgoing = Some(std::mem::replace(
                &mut element.label,
                LabelRun::new(self.label.clone()),
            ));
            element.label_roll = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.status != self.status {
            element.outgoing_status = Some(element.status);
            element.status = self.status;
            element.icon_roll = None;
            flags |= ChangeFlags::PAINT;
        }
        if prev.size != self.size {
            element.size = self.size;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.show_icon != self.show_icon {
            element.show_icon = self.show_icon;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.pulse != self.pulse {
            element.pulse = self.pulse;
            flags |= ChangeFlags::PAINT;
        }
        flags
    }
}

impl Widget for AnimatedBadgeWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let style = self.label_style(Theme::from_layout_ctx(ctx));
        let label = self.label.layout(ctx, &style);
        if let Some(outgoing) = &mut self.outgoing {
            // Shaped so it can be painted mid-exit; it never widens the pill.
            outgoing.layout(ctx, &style);
        }
        let mut width = self.size.padding_x() * 2.0 + label.width;
        if self.show_icon {
            width += self.size.icon_size() + self.size.gap();
        }
        bc.constrain(Size::new(width, self.size.height()))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let colors = self.colors(self.status, theme);
        let now = ctx.frame_time();
        let started = *self.started.get_or_insert(now);
        let elapsed = now.saturating_sub(started);
        let origin = ctx.origin();
        let size = ctx.size();
        let radius = resolve_radius(RADIUS_CONTROL, size.width, size.height);

        let pulsing = self.pulses() && !reduce;
        if pulsing {
            // `absolute inset-0 rounded-full bg-current opacity-10`, scaling.
            let wave = ping_pong(cycle(elapsed, PULSE_PERIOD));
            let scale = PULSE_SCALE.0 + (PULSE_SCALE.1 - PULSE_SCALE.0) * wave;
            let alpha = PULSE_ALPHA.0 + (PULSE_ALPHA.1 - PULSE_ALPHA.0) * wave as f32;
            let grown = Size::new(size.width * scale, size.height * scale);
            scene.fill_rounded_rect(
                Point::new(
                    origin.x - (grown.width - size.width) / 2.0,
                    origin.y - (grown.height - size.height) / 2.0,
                ),
                grown,
                resolve_radius(RADIUS_CONTROL, grown.width, grown.height),
                with_alpha(colors.ink, alpha),
            );
        }

        scene.fill_rounded_rect(origin, size, radius, colors.fill);
        let inset = BORDER_WIDTH / 2.0;
        let outline = kurbo::RoundedRect::from_rect(
            kurbo::Rect::from_origin_size(Point::ORIGIN, size).inset(-inset),
            (radius - inset).max(0.0),
        );
        scene.stroke_path(
            origin,
            &Shape::to_path(&outline, PATH_TOLERANCE),
            BORDER_WIDTH,
            &Brush::Solid(colors.border),
        );

        // A roll is in flight exactly while an outgoing part is still held, and
        // the clock it is timed from is latched on the first paint that sees it
        // — so a badge never plays a roll on mount (upstream's
        // `AnimatePresence initial={false}`).
        let icon_progress = self.advance_roll(now, reduce, Roll::Icon);
        let label_progress = self.advance_roll(now, reduce, Roll::Label);

        let mut x = origin.x + self.size.padding_x();
        if self.show_icon {
            let icon = self.size.icon_size();
            let slot = Point::new(x, origin.y + (size.height - icon) / 2.0);
            let slot_size = Size::new(icon, icon);
            // `overflow-hidden` on the icon slot: a rolling glyph disappears at
            // the slot's edge rather than over the pill's border.
            scene.push_clip(slot, slot_size);
            if let Some(progress) = icon_progress {
                if let Some(previous) = self.outgoing_status {
                    let (offset, alpha, scale) = exit_stage(progress);
                    self.paint_icon(
                        previous,
                        slot,
                        icon * scale,
                        offset,
                        alpha,
                        elapsed,
                        reduce,
                        theme,
                        scene,
                    );
                }
                let (offset, alpha, scale) = enter_stage(progress);
                self.paint_icon(
                    self.status,
                    slot,
                    icon * scale,
                    offset,
                    alpha,
                    elapsed,
                    reduce,
                    theme,
                    scene,
                );
            } else {
                self.paint_icon(
                    self.status,
                    slot,
                    icon,
                    0.0,
                    1.0,
                    elapsed,
                    reduce,
                    theme,
                    scene,
                );
            }
            scene.pop_clip();
            x += icon + self.size.gap();
        }

        let label = self.label.size();
        let slot = Point::new(x, origin.y + (size.height - label.height) / 2.0);
        scene.push_clip(
            Point::new(slot.x, origin.y),
            Size::new(
                (size.width - (slot.x - origin.x) - self.size.padding_x()).max(0.0),
                size.height,
            ),
        );
        if let Some(progress) = label_progress {
            if let Some(outgoing) = &self.outgoing {
                let (offset, alpha, _) = exit_stage(progress);
                outgoing.paint(
                    Point::new(slot.x, slot.y + offset * label.height),
                    scale_alpha(colors.ink, alpha as f32),
                    scene,
                );
            }
            let (offset, alpha, _) = enter_stage(progress);
            self.label.paint(
                Point::new(slot.x, slot.y + offset * label.height),
                scale_alpha(colors.ink, alpha as f32),
                scene,
            );
        } else {
            self.label.paint(slot, colors.ink, scene);
        }
        scene.pop_clip();

        if reduce {
            return;
        }
        if label_progress.is_some() || icon_progress.is_some() {
            // A roll has a visible endpoint.
            ctx.request_frame();
        } else if pulsing || self.status == AnimatedBadgeStatus::Loading {
            // The pulse and the spin are perpetual decorative loops.
            ctx.request_frame_class(TickClass::CosmeticLoop);
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // A status badge announces its own text; the icon is `aria-hidden`
        // upstream and contributes nothing here either.
        ctx.push_node(Role::Status, |node| {
            node.set_label(self.label.content());
        });
    }
}

impl AnimatedBadgeWidget {
    /// Step one roll and report its progress, or `None` when nothing is
    /// rolling.
    ///
    /// A roll is in flight exactly while its outgoing part is still held; the
    /// clock is latched on the first paint that sees one, and both are dropped
    /// the frame the spring settles — or immediately under reduced motion,
    /// which has no roll to play.
    fn advance_roll(&mut self, now: FrameTime, reduce: bool, roll: Roll) -> Option<f64> {
        let pending = match roll {
            Roll::Icon => self.outgoing_status.is_some(),
            Roll::Label => self.outgoing.is_some(),
        };
        let settle = |widget: &mut Self| match roll {
            Roll::Icon => {
                widget.outgoing_status = None;
                widget.icon_roll = None;
            }
            Roll::Label => {
                widget.outgoing = None;
                widget.label_roll = None;
            }
        };
        if !pending || reduce {
            settle(self);
            return None;
        }
        let slot = match roll {
            Roll::Icon => &mut self.icon_roll,
            Roll::Label => &mut self.label_roll,
        };
        let started = *slot.get_or_insert(now);
        let elapsed = now.saturating_sub(started);
        let ramp = Ramp::spring(SPRING_SWAP);
        if ramp.is_settled(elapsed) {
            settle(self);
            return None;
        }
        Some(ramp.progress_clamped(elapsed))
    }

    /// Draw one status icon into a `edge`-square slot at `origin`, translated by
    /// `offset` slot-heights and composited at `alpha`.
    #[allow(clippy::too_many_arguments)]
    fn paint_icon(
        &self,
        status: AnimatedBadgeStatus,
        origin: Point,
        edge: f64,
        offset: f64,
        alpha: f64,
        elapsed: Duration,
        reduce: bool,
        theme: Option<&Theme>,
        scene: &mut dyn PaintScene,
    ) {
        if edge <= 0.0 {
            return;
        }
        let colors = self.colors(status, theme);
        let ink = scale_alpha(colors.ink, alpha as f32);
        let slot = self.size.icon_size();
        let at = Point::new(
            origin.x + (slot - edge) / 2.0,
            origin.y + (slot - edge) / 2.0 + offset * slot,
        );
        // Lucide strokes at 2px on a 24px canvas.
        let stroke = (edge / 24.0 * 2.0).max(1.0);
        let centre = Point::new(at.x + edge / 2.0, at.y + edge / 2.0);
        let radius = (edge - stroke) / 2.0;
        let brush = Brush::Solid(ink);

        match status {
            AnimatedBadgeStatus::Neutral => {
                let ring = Arc::new(centre, Vec2::new(radius, radius), 0.0, TAU, 0.0);
                scene.stroke_path(Point::ORIGIN, &ring.to_path(PATH_TOLERANCE), stroke, &brush);
            }
            AnimatedBadgeStatus::Info => {
                let ring = Arc::new(centre, Vec2::new(radius, radius), 0.0, TAU, 0.0);
                scene.stroke_path(Point::ORIGIN, &ring.to_path(PATH_TOLERANCE), stroke, &brush);
                // The `i`: a stem, and the dot above it.
                let mut stem = BezPath::new();
                stem.move_to(Point::new(centre.x, centre.y - edge * 0.04));
                stem.line_to(Point::new(centre.x, centre.y + edge * 0.24));
                scene.stroke_path(Point::ORIGIN, &stem, stroke, &brush);
                let dot = edge * 0.14;
                scene.fill_rounded_rect(
                    Point::new(centre.x - dot / 2.0, centre.y - edge * 0.26 - dot / 2.0),
                    Size::new(dot, dot),
                    dot / 2.0,
                    ink,
                );
            }
            AnimatedBadgeStatus::Success => {
                let mut tick = BezPath::new();
                tick.move_to(Point::new(at.x + edge * 0.2, at.y + edge * 0.52));
                tick.line_to(Point::new(at.x + edge * 0.42, at.y + edge * 0.74));
                tick.line_to(Point::new(at.x + edge * 0.8, at.y + edge * 0.28));
                scene.stroke_path(Point::ORIGIN, &tick, stroke, &brush);
            }
            AnimatedBadgeStatus::Warning => {
                let mut triangle = BezPath::new();
                triangle.move_to(Point::new(centre.x, at.y + edge * 0.14));
                triangle.line_to(Point::new(at.x + edge * 0.9, at.y + edge * 0.82));
                triangle.line_to(Point::new(at.x + edge * 0.1, at.y + edge * 0.82));
                triangle.close_path();
                scene.stroke_path(Point::ORIGIN, &triangle, stroke, &brush);
                let mut bang = BezPath::new();
                bang.move_to(Point::new(centre.x, at.y + edge * 0.4));
                bang.line_to(Point::new(centre.x, at.y + edge * 0.62));
                scene.stroke_path(Point::ORIGIN, &bang, stroke, &brush);
            }
            AnimatedBadgeStatus::Danger => {
                let inset = edge * 0.22;
                let mut cross = BezPath::new();
                cross.move_to(Point::new(at.x + inset, at.y + inset));
                cross.line_to(Point::new(at.x + edge - inset, at.y + edge - inset));
                cross.move_to(Point::new(at.x + edge - inset, at.y + inset));
                cross.line_to(Point::new(at.x + inset, at.y + edge - inset));
                scene.stroke_path(Point::ORIGIN, &cross, stroke, &brush);
            }
            AnimatedBadgeStatus::Loading => {
                let angle = if reduce {
                    0.0
                } else {
                    cycle(elapsed, SPIN_PERIOD) * TAU
                };
                let arc = Arc::new(centre, Vec2::new(radius, radius), angle, SPINNER_SWEEP, 0.0);
                scene.stroke_path(Point::ORIGIN, &arc.to_path(PATH_TOLERANCE), stroke, &brush);
            }
        }
    }
}

/// Which of the badge's two rolling slots a helper is acting on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Roll {
    /// The status icon, rolled by a status change.
    Icon,
    /// The label, rolled by a text change.
    Label,
}

/// The entering part's staging at `progress`: its offset in slot-heights, its
/// alpha and its scale — `initial { y: 85%, opacity: 0.76, scale: 0.92 }` to
/// rest.
fn enter_stage(progress: f64) -> (f64, f64, f64) {
    let p = progress.clamp(0.0, 1.0);
    (
        (1.0 - p) * ROLL_TRAVEL,
        ROLL_ENTER_ALPHA + (1.0 - ROLL_ENTER_ALPHA) * p,
        0.92 + 0.08 * p,
    )
}

/// The leaving part's staging at `progress` — `exit { y: -85%, opacity: 0.5 }`,
/// carried the rest of the way to invisible so nothing is stranded on screen.
fn exit_stage(progress: f64) -> (f64, f64, f64) {
    let p = progress.clamp(0.0, 1.0);
    (-p * ROLL_TRAVEL, 1.0 - p, 1.0 - 0.04 * p)
}

/// `elapsed` folded into `[0, 1)` of a `period`-long loop.
fn cycle(elapsed: Duration, period: Duration) -> f64 {
    if period.is_zero() {
        return 0.0;
    }
    let turns = elapsed.as_secs_f64() / period.as_secs_f64();
    turns - turns.floor()
}

/// A `0 → 1 → 0` triangle over one loop — the shape upstream's
/// `[a, b, a]` keyframe sets produce.
fn ping_pong(phase: f64) -> f64 {
    1.0 - (phase * 2.0 - 1.0).abs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::text::TextContext;
    use frust_core::{BuildCtx, PaintCtx};
    use std::any::Any;

    /// The box every paint test lays its badge into.
    const BOX: Size = Size::new(120.0, 32.0);

    /// Records the pill fills, the stroked icon paths and the alpha every label
    /// run was drawn at — the roll's whole observable surface.
    #[derive(Default)]
    struct Recorder {
        rrects: Vec<Color>,
        strokes: usize,
        label_alphas: Vec<f32>,
        clips: usize,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn fill_rounded_rect(&mut self, _o: Point, _s: Size, _r: f64, color: Color) {
            self.rrects.push(color);
        }
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, _w: f64, _brush: &Brush) {
            self.strokes += 1;
        }
        fn push_clip(&mut self, _origin: Point, _size: Size) {
            self.clips += 1;
        }
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            if let Brush::Solid(color) = run.brush {
                self.label_alphas.push(color.components[3]);
            }
        }
    }

    fn laid_out(view: &AnimatedBadgeView<()>) -> AnimatedBadgeWidget {
        let mut next_id = 0u64;
        let mut widget = View::<()>::build(view, &mut BuildCtx::new(&mut next_id));
        relayout(&mut widget);
        widget
    }

    fn relayout(widget: &mut AnimatedBadgeWidget) {
        let mut text_ctx = TextContext::new();
        let mut layout = LayoutCtx::with_text_context(&mut text_ctx as &mut dyn Any);
        widget.layout(&mut layout, &BoxConstraints::loose(BOX));
    }

    fn painted(
        widget: &mut AnimatedBadgeWidget,
        ms: u64,
        theme: Option<&Theme>,
    ) -> (Recorder, bool, Option<TickClass>) {
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, BOX, FrameTime::from_nanos(ms * 1_000_000));
        if let Some(theme) = theme {
            ctx = ctx.with_theme(theme);
        }
        let mut recorder = Recorder::default();
        widget.paint(&mut ctx, &mut recorder);
        (recorder, ctx.needs_frame(), ctx.frame_class())
    }

    fn rebuilt(
        prev: &AnimatedBadgeView<()>,
        next: &AnimatedBadgeView<()>,
        widget: &mut AnimatedBadgeWidget,
    ) {
        let mut next_id = 0u64;
        View::<()>::rebuild(next, prev, widget, &mut BuildCtx::new(&mut next_id));
        relayout(widget);
    }

    fn reduced() -> Theme {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        theme
    }

    /// Every tone and every size upstream ships is constructible and paints.
    #[test]
    fn every_status_and_size_is_constructible() {
        assert_eq!(AnimatedBadgeStatus::ALL.len(), 6);
        assert_eq!(AnimatedBadgeSize::ALL.len(), 2);
        for status in AnimatedBadgeStatus::ALL {
            for size in AnimatedBadgeSize::ALL {
                let mut widget = laid_out(&animated_badge::<()>("Ready").status(status).size(size));
                assert_eq!(widget.status(), status);
                let (recorder, _, _) = painted(&mut widget, 0, None);
                assert!(!recorder.rrects.is_empty(), "{status:?}/{size:?} pill");
                assert!(recorder.strokes > 0, "{status:?}/{size:?} border and icon");
            }
        }
    }

    /// The size rungs carry the class-list metrics verbatim.
    #[test]
    fn the_size_rungs_carry_the_source_metrics() {
        assert_eq!(AnimatedBadgeSize::Sm.height(), 24.0, "h-6");
        assert_eq!(AnimatedBadgeSize::Md.height(), 32.0, "h-8");
        assert_eq!(AnimatedBadgeSize::Sm.padding_x(), 8.0, "px-2");
        assert_eq!(AnimatedBadgeSize::Md.padding_x(), 12.0, "px-3");
        assert_eq!(AnimatedBadgeSize::Sm.gap(), 6.0, "gap-1.5");
        assert_eq!(AnimatedBadgeSize::Md.gap(), 8.0, "gap-2");
        assert_eq!(AnimatedBadgeSize::Sm.text_size(), 11.0);
        assert_eq!(AnimatedBadgeSize::Md.text_size(), TEXT_XS);
        assert_eq!(AnimatedBadgeSize::Sm.icon_size(), 12.0, "size-3");
        assert_eq!(AnimatedBadgeSize::Md.icon_size(), 14.0, "size-3.5");
        assert_eq!(AnimatedBadgeSize::default(), AnimatedBadgeSize::Md);
    }

    /// Hiding the icon narrows the pill by exactly the icon and its gap.
    #[test]
    fn hiding_the_icon_reclaims_its_slot() {
        let mut with_icon = laid_out(&animated_badge::<()>("Ready"));
        let mut without = laid_out(&animated_badge::<()>("Ready").show_icon(false));
        let mut text_ctx = TextContext::new();
        let mut layout = LayoutCtx::with_text_context(&mut text_ctx as &mut dyn Any);
        let wide = with_icon.layout(&mut layout, &BoxConstraints::loose(BOX));
        let narrow = without.layout(&mut layout, &BoxConstraints::loose(BOX));
        let size = AnimatedBadgeSize::default();
        assert!(
            (wide.width - narrow.width - size.icon_size() - size.gap()).abs() < 1e-9,
            "{wide:?} vs {narrow:?}"
        );
        assert_eq!(wide.height, size.height());
    }

    /// Only `loading` pulses by default, and the prop overrides either way.
    #[test]
    fn the_pulse_follows_loading_unless_overridden() {
        for status in AnimatedBadgeStatus::ALL {
            let expected = status == AnimatedBadgeStatus::Loading;
            assert_eq!(status.pulses_by_default(), expected, "{status:?}");
            assert_eq!(
                laid_out(&animated_badge::<()>("x").status(status)).pulses(),
                expected
            );
        }
        assert!(laid_out(&animated_badge::<()>("x").pulse(true)).pulses());
        assert!(
            !laid_out(
                &animated_badge::<()>("x")
                    .status(AnimatedBadgeStatus::Loading)
                    .pulse(false)
            )
            .pulses()
        );
    }

    /// A pulsing badge paints an extra washed pill behind the real one, and a
    /// still one does not.
    #[test]
    fn the_pulse_paints_a_wash_behind_the_pill() {
        let mut still = laid_out(&animated_badge::<()>("Ready"));
        let (plain, _, _) = painted(&mut still, 0, None);

        let mut pulsing = laid_out(&animated_badge::<()>("Ready").pulse(true));
        let (recorder, _, class) = painted(&mut pulsing, 0, None);
        assert_eq!(recorder.rrects.len(), plain.rrects.len() + 1);
        assert!(
            recorder.rrects[0].components[3] <= PULSE_ALPHA.1,
            "the wash is faint"
        );
        assert_eq!(class, Some(TickClass::CosmeticLoop), "a perpetual loop");
    }

    /// A badge does not animate on mount — upstream's `initial={false}`.
    #[test]
    fn a_badge_does_not_roll_on_mount() {
        let mut widget = laid_out(&animated_badge::<()>("Ready"));
        let (recorder, needs_frame, class) = painted(&mut widget, 0, None);
        assert!(!needs_frame, "mounting is not a roll");
        assert_eq!(class, None);
        assert_eq!(
            recorder.label_alphas.len(),
            1,
            "one label on screen, not two"
        );
    }

    /// A label change rolls the old text out as the new one rolls in, and the
    /// pair collapses back to one once the spring settles.
    #[test]
    fn a_label_change_rolls_the_old_text_out() {
        let first = animated_badge::<()>("Queued");
        let mut widget = laid_out(&first);
        painted(&mut widget, 0, None);

        let second = animated_badge::<()>("Running");
        rebuilt(&first, &second, &mut widget);
        let (mid, needs_frame, _) = painted(&mut widget, 10, None);
        assert!(needs_frame, "a roll has a visible endpoint");
        assert_eq!(mid.label_alphas.len(), 2, "both labels on screen");
        assert!(mid.clips >= 2, "each slot clips its own roll");

        // Well past the spring's settle time only the new label remains.
        let (settled, needs_frame, _) = painted(&mut widget, 5_000, None);
        assert!(!needs_frame);
        assert_eq!(settled.label_alphas.len(), 1);
        assert_eq!(widget.label(), "Running");
    }

    /// A status change rolls the icon the same way, and the badge takes the new
    /// tone's colours.
    #[test]
    fn a_status_change_rolls_the_icon() {
        let first = animated_badge::<()>("Ready");
        let mut widget = laid_out(&first);
        let (before, _, _) = painted(&mut widget, 0, None);

        let second = animated_badge::<()>("Ready").status(AnimatedBadgeStatus::Success);
        rebuilt(&first, &second, &mut widget);
        let (mid, needs_frame, _) = painted(&mut widget, 10, None);
        assert!(needs_frame);
        assert!(
            mid.strokes > before.strokes,
            "two icons are drawn mid-roll: {} vs {}",
            mid.strokes,
            before.strokes
        );

        let (_, needs_frame, _) = painted(&mut widget, 5_000, None);
        assert!(!needs_frame);
        assert_eq!(widget.status(), AnimatedBadgeStatus::Success);
    }

    /// `reduce_motion` shows the settled badge outright — no roll, no pulse, no
    /// frames — even with a change in flight.
    #[test]
    fn reduce_motion_drops_the_roll_and_the_pulse() {
        let theme = reduced();
        let first = animated_badge::<()>("Queued").status(AnimatedBadgeStatus::Loading);
        let mut widget = laid_out(&first);
        let second = animated_badge::<()>("Running").status(AnimatedBadgeStatus::Success);
        rebuilt(&first, &second, &mut widget);

        let (recorder, needs_frame, class) = painted(&mut widget, 0, Some(&theme));
        assert!(!needs_frame);
        assert_eq!(class, None);
        assert_eq!(recorder.label_alphas.len(), 1, "no outgoing label");
        assert_eq!(recorder.rrects.len(), 1, "no pulse wash");
    }

    /// The status hues come from the token tables, at the class list's own
    /// alphas — not from upstream's hardcoded Tailwind literals.
    #[test]
    fn the_status_hues_resolve_from_the_token_tables() {
        let theme = crate::theme();
        let widget = laid_out(&animated_badge::<()>("x"));
        let tokens = BeuiTokens::beui();

        let success = widget.colors(AnimatedBadgeStatus::Success, Some(&theme));
        assert_eq!(success.ink, tokens.success);
        assert_eq!(success.fill, with_alpha(tokens.success, FILL_ALPHA));
        assert_eq!(success.border, with_alpha(tokens.success, BORDER_ALPHA));

        let warning = widget.colors(AnimatedBadgeStatus::Warning, Some(&theme));
        assert_eq!(warning.ink, tokens.warning);

        let danger = widget.colors(AnimatedBadgeStatus::Danger, Some(&theme));
        assert_eq!(danger.ink, theme.scheme().error);

        // Neutral is the only tone painting an opaque surface rather than a
        // wash of its own hue.
        let neutral = widget.colors(AnimatedBadgeStatus::Neutral, Some(&theme));
        assert_eq!(neutral.fill, theme.scheme().surface_container);
        assert_eq!(neutral.ink, theme.scheme().on_surface_variant);

        // Unthemed, every tone still resolves rather than painting nothing.
        for status in AnimatedBadgeStatus::ALL {
            let colors = widget.colors(status, None);
            assert!(colors.ink.components[3] > 0.0, "{status:?}");
        }
    }

    /// The roll's two halves are anchored: an entering part arrives at rest, a
    /// leaving one departs to invisible, and they travel opposite ways.
    #[test]
    fn the_roll_stages_are_anchored_at_both_ends() {
        let (offset, alpha, scale) = enter_stage(0.0);
        assert_eq!(offset, ROLL_TRAVEL, "enters from below its slot");
        assert_eq!(alpha, ROLL_ENTER_ALPHA);
        assert!(scale < 1.0);

        let (offset, alpha, scale) = enter_stage(1.0);
        assert_eq!(offset, 0.0);
        assert_eq!(alpha, 1.0);
        assert_eq!(scale, 1.0);

        let (offset, alpha, _) = exit_stage(0.0);
        assert_eq!((offset, alpha), (0.0, 1.0));
        let (offset, alpha, _) = exit_stage(1.0);
        assert_eq!(offset, -ROLL_TRAVEL, "leaves upward — the opposite way");
        assert_eq!(alpha, 0.0, "nothing is stranded on screen");
    }

    /// The pulse wave is a triangle over its period, so the badge breathes
    /// symmetrically rather than snapping back.
    #[test]
    fn the_pulse_wave_is_a_triangle() {
        assert_eq!(ping_pong(0.0), 0.0);
        assert!((ping_pong(0.5) - 1.0).abs() < 1e-9);
        assert!(ping_pong(1.0).abs() < 1e-9);
        assert!((ping_pong(0.25) - ping_pong(0.75)).abs() < 1e-9);
        assert_eq!(cycle(PULSE_PERIOD, PULSE_PERIOD), 0.0, "the loop wraps");
        assert_eq!(cycle(Duration::from_millis(1), Duration::ZERO), 0.0);
    }
}
