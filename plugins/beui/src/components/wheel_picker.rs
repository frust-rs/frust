//! Ports beUI's **Wheel Picker** — the iOS-style drum that coasts on a flick
//! and settles onto a notch.
//!
//! Source: `components/motion/wheel-picker.tsx` (beUI v2, rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01), registry
//! slug `wheel-picker`: *"iOS-style picker wheel: a 3D drum on native momentum
//! scroll that snaps to the nearest notch, with wheel, drag and keyboard
//! control."*
//!
//! | class / constant | here |
//! |---|---|
//! | `visibleCount = 5`, `itemHeight = 36` | [`WheelPicker::visible_count`], [`WheelPicker::item_height`] |
//! | `angle = 90 / cutoff`, `r = itemHeight / tan(angle)` | [`Drum::resolve`] |
//! | `height = 2r·sin(rowsEachSide·angle) + itemHeight` | [`Drum::height`] |
//! | `DECELERATION = 0.00042` rows/ms² | [`DECELERATION`] |
//! | `MAX_VELOCITY = 0.18` rows/ms | [`MAX_VELOCITY`] |
//! | `VELOCITY_WINDOW = 90` ms | [`VELOCITY_WINDOW_MS`] |
//! | `WHEEL_SENS = 0.012` rows/px, `WHEEL_SETTLE = 110` ms | [`WHEEL_SENSITIVITY`], [`WHEEL_SETTLE_MS`] |
//! | `easeOutCubic`, `easeOutBack` with `BACK = 1.35` | [`Ease`] |
//! | `rounded-2xl border border-border bg-card` | [`style::RADIUS_2XL`], `outline_variant`, `surface_container` |
//! | centre band `bg-foreground/[0.04] rounded-md` | [`BAND_ALPHA`], [`style::RADIUS_MD`] |
//! | `focus-visible:ring-2 ring-foreground/20` | [`style::FOCUS_RING_WIDTH`] at [`RING_ALPHA`] |
//! | `pointer-events-none opacity-50` | [`style::DISABLED_OPACITY`] |
//!
//! # The 3D drum, as a 2D approximation — the port's headline degradation
//!
//! Upstream is a real CSS 3D scene: a `perspective: 1000px` container, a
//! `transform-style: preserve-3d` list, and one `rotateX(…) translateZ(r)` per
//! row. This scene's paint vocabulary has no perspective projection and no
//! per-vertex transform, so each row is placed with a **2D affine derived from
//! that same cylinder**:
//!
//! - **Vertical offset** `−r·sin ψ`, the row's true height on the drum;
//! - **Uniform scale** `P / (P + r·(1 − cos ψ))` — the same perspective divide
//!   the browser applies, evaluated at the row's centre ([`PERSPECTIVE`]);
//! - **Extra vertical squash** `|cos ψ|`, which is what `rotateX` does to a
//!   row's own height;
//! - **Horizon cull** at `|i − scroll| > cutoff`, exactly upstream's
//!   `visibility: hidden` rule.
//!
//! What that loses: the projection is evaluated once per row rather than per
//! pixel, so a row is not *keystoned* (its top and bottom edges scale
//! identically instead of converging), and glyphs inside a row do not fan. At
//! the row heights this control uses the difference is sub-pixel near the
//! centre and a fraction of a pixel at the horizon, which is why the
//! approximation is worth taking; it is still an approximation, not the same
//! render.
//!
//! # Other degradations
//!
//! - **The edge fade is per row, not per pixel.** Upstream masks the container
//!   with `linear-gradient(to bottom, transparent, #000 22%, #000 78%,
//!   transparent)`; there is no gradient mask here, so [`mask_alpha`] evaluates
//!   that ramp at each row's centre and composites the whole row at it.
//! - **No tick sound.** `sound` drives `lib/tick-sound.ts`'s WebAudio player;
//!   frust has no audio seam, so the prop is not ported.
//! - **One reduced-motion rendering, not two.** Upstream swaps the drum for a
//!   scroll-snap list of buttons under `useReducedMotion`. This port keeps one
//!   control and collapses its *animation* instead: a fling, a wheel settle and
//!   a key step all land immediately. Direct manipulation is not motion, so the
//!   drag itself is untouched.
//! - **A fling reports its landing value at release**, not once per coasting
//!   frame. The callback needs `&mut State`, which only an event pass carries —
//!   and the landing notch is already known when the pointer lifts, so the
//!   glide that follows is purely visual.
//! - **Velocity is sampled per painted frame**, since an `EventCtx` carries no
//!   clock; upstream averages pointer samples over [`VELOCITY_WINDOW_MS`], and
//!   so does this, over the frames those moves landed on.

use std::rc::Rc;

use frust::authoring::text::{FontWeight, TextStyle};
use frust::authoring::{
    Action, Affine, BoxConstraints, Brush, BuildCtx, ChangeFlags, Color, CursorIcon, EventCtx,
    EventResult, InputEvent, Key, LayoutCtx, NamedKey, PaintCtx, PaintScene, Point, PointerPhase,
    Rect, Role, RoundedRect, ScrollDelta, SemanticsCtx, Shape, Size, View, Widget,
    erase_callback_arg,
};
use frust::{FrameTime, Theme};

use crate::press::{inside_inclusive as inside, presses};
use crate::style;
use crate::text::Label as ShapedText;
use crate::tokens::sans_family;

/// Width used when the incoming constraints are horizontally unbounded — the
/// drum has no intrinsic width upstream either.
pub const UNBOUNDED_WIDTH: f64 = 200.0;

/// Rows visible through the window, before rounding down to an odd count
/// (`visibleCount = 5`).
pub const DEFAULT_VISIBLE_COUNT: usize = 5;
/// Row height, in logical px (`itemHeight = 36`).
pub const DEFAULT_ITEM_HEIGHT: f64 = 36.0;

/// The CSS `perspective` the container declares, in logical px
/// (`style={{ …, perspective: 1000 }}`).
pub const PERSPECTIVE: f64 = 1000.0;

/// How fast a flick bleeds off, in rows per millisecond squared
/// (`DECELERATION = 0.00042`) — lower is freer.
pub const DECELERATION: f64 = 0.000_42;
/// The cap on a hard fling, in rows per millisecond (`MAX_VELOCITY = 0.18`).
pub const MAX_VELOCITY: f64 = 0.18;
/// How much recent drag a release velocity is averaged over, in milliseconds
/// (`VELOCITY_WINDOW = 90`) — a single noisy frame otherwise makes an even
/// flick feel like it caught or slipped.
pub const VELOCITY_WINDOW_MS: f64 = 90.0;
/// Rows per pixel of wheel delta (`WHEEL_SENS = 0.012`).
pub const WHEEL_SENSITIVITY: f64 = 0.012;
/// How long the wheel must be idle before the drum snaps, in milliseconds
/// (`WHEEL_SETTLE = 110`).
pub const WHEEL_SETTLE_MS: f64 = 110.0;
/// Logical px per wheel *line*, for a line-quantised scroll — the value
/// upstream substitutes for `deltaMode === 1`.
pub const WHEEL_LINE_HEIGHT: f64 = 16.0;

/// How far past its target [`Ease::OutBack`] drifts (`BACK = 1.35`).
pub const BACK: f64 = 1.35;

/// The wheel settle's glide duration, in milliseconds (`glide(…, 240,
/// easeOutBack)`).
pub const WHEEL_SNAP_MS: f64 = 240.0;
/// A key step's glide duration, in milliseconds (`glide(…, 300, easeOutBack)`).
pub const KEY_STEP_MS: f64 = 300.0;
/// The glide a confirmed value change takes, in milliseconds
/// (`glide(target, 260)`).
pub const VALUE_SYNC_MS: f64 = 260.0;
/// The rubber-band return after a drag ends past an end, in milliseconds
/// (`glide(clamp(round(from), …), 260)`).
pub const REBOUND_MS: f64 = 260.0;
/// A fling's shortest settle, in milliseconds.
pub const FLING_MIN_MS: f64 = 280.0;
/// A fling's longest settle, in milliseconds.
pub const FLING_MAX_MS: f64 = 1_700.0;

/// How much of a drag past an end the drum keeps (`next *= 0.3`).
pub const RUBBER_BAND: f64 = 0.3;

/// Alpha of the centre band's wash (`bg-foreground/[0.04]`).
pub const BAND_ALPHA: f32 = 0.04;
/// Alpha of the focus ring (`ring-foreground/20`).
pub const RING_ALPHA: f32 = 0.2;
/// Where the container's mask reaches full opacity, as a fraction of its height
/// (`linear-gradient(to bottom, transparent, #000 22%, …)`).
pub const MASK_STOP: f64 = 0.22;

/// Unthemed fallback ink — the light table's `--foreground`.
const FALLBACK_FOREGROUND: Color = crate::BEUI_LIGHT.foreground;
/// Unthemed fallback dimmed ink — the light table's `--muted-foreground`.
const FALLBACK_MUTED_FOREGROUND: Color = crate::BEUI_LIGHT.muted_foreground;
/// Unthemed fallback panel — the light table's `--card`.
const FALLBACK_CARD: Color = crate::BEUI_LIGHT.card;
/// Unthemed fallback hairline — the light table's `--border`.
const FALLBACK_BORDER: Color = crate::BEUI_LIGHT.border;

/// The two easings the drum glides on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ease {
    /// `easeOutCubic = 1 − (1 − p)³` — the plain settle.
    OutCubic,
    /// `easeOutBack` with [`BACK`] — overshoots a few percent then resolves, the
    /// little spring bounce as a row snaps home.
    OutBack,
}

impl Ease {
    /// This easing at `p` in `0..=1`.
    pub fn at(self, p: f64) -> f64 {
        let p = p.clamp(0.0, 1.0);
        match self {
            Ease::OutCubic => 1.0 - (1.0 - p).powi(3),
            Ease::OutBack => 1.0 + (BACK + 1.0) * (p - 1.0).powi(3) + BACK * (p - 1.0).powi(2),
        }
    }
}

/// The cylinder the rows are seated on, resolved from the two authored props.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drum {
    /// Degrees of drum per row (`angle = 90 / cutoff`).
    pub item_angle: f64,
    /// The drum's radius, in logical px (`r = itemHeight / tan(angle)`).
    pub radius: f64,
    /// Rows past this distance from the centre sit behind the horizon and are
    /// dropped (`hideBeyond = cutoff`).
    pub hide_beyond: f64,
    /// The window's height, in logical px.
    pub height: f64,
    /// The row height it was resolved for.
    pub item_height: f64,
}

impl Drum {
    /// Resolve the geometry for `visible_count` rows of `item_height`.
    ///
    /// Upstream's own derivation: half the visible rows sit on each side, the
    /// cutoff is one row past them, and the drum turns 90° over that cutoff —
    /// so the row at the cutoff is edge-on to the viewer and invisible.
    pub fn resolve(visible_count: usize, item_height: f64) -> Self {
        let item_height = item_height.max(1.0);
        let rows_each_side = (visible_count / 2).max(1) as f64;
        let cutoff = rows_each_side + 1.0;
        let item_angle = 90.0 / cutoff;
        let radius = item_height / item_angle.to_radians().tan();
        let height =
            (2.0 * radius * (rows_each_side * item_angle).to_radians().sin() + item_height).round();
        Self {
            item_angle,
            radius,
            hide_beyond: cutoff,
            height,
            item_height,
        }
    }

    /// The row's angle on the drum at scroll position `scroll`, in radians.
    ///
    /// Zero when the row is facing the viewer, negative below the centre —
    /// upstream's `rotateX(angle·scroll)` on the drum composed with each row's
    /// own `rotateX(−angle·i)`.
    pub fn angle(self, index: f64, scroll: f64) -> f64 {
        (self.item_angle * (scroll - index)).to_radians()
    }

    /// How a row `index` is placed at scroll position `scroll`: its vertical
    /// offset from the window's centre, and the two scale factors the 2D
    /// approximation stands in for the perspective projection with.
    pub fn place(self, index: f64, scroll: f64) -> RowPlacement {
        let angle = self.angle(index, scroll);
        // The row's height on the drum, and how far behind the front plane it
        // has travelled.
        let offset = -self.radius * angle.sin();
        let depth = self.radius * (1.0 - angle.cos());
        let scale = PERSPECTIVE / (PERSPECTIVE + depth);
        RowPlacement {
            offset,
            scale_x: scale,
            scale_y: scale * angle.cos().abs(),
            hidden: (index - scroll).abs() > self.hide_beyond,
        }
    }
}

/// Where one row of the drum lands, and how it is squashed getting there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RowPlacement {
    /// Vertical offset from the window's centre, in logical px.
    pub offset: f64,
    /// Horizontal scale — the perspective divide.
    pub scale_x: f64,
    /// Vertical scale — the perspective divide *and* the `rotateX`
    /// foreshortening.
    pub scale_y: f64,
    /// Whether the row is past the horizon and should not be painted.
    pub hidden: bool,
}

/// The container mask's alpha at a row centred `y` px into a window of
/// `height` — `linear-gradient(to bottom, transparent, #000 22%, #000 78%,
/// transparent)`, evaluated once per row (see the [module docs](self)).
pub fn mask_alpha(y: f64, height: f64) -> f64 {
    if height <= 0.0 {
        return 1.0;
    }
    let t = y / height;
    let top = (t / MASK_STOP).clamp(0.0, 1.0);
    let bottom = ((1.0 - t) / MASK_STOP).clamp(0.0, 1.0);
    top.min(bottom)
}

/// One row of the wheel: the value it reports and the text it shows.
///
/// Upstream's `WheelPickerOption` is `string | { label, value }`; the two `From`
/// impls below are that union, so a caller can hand this a list of plain
/// strings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WheelPickerOption {
    /// What the picker reports when this row is selected.
    pub value: String,
    /// What the row shows.
    pub label: String,
}

impl WheelPickerOption {
    /// A row whose reported value and shown label differ.
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
        }
    }
}

impl From<&str> for WheelPickerOption {
    fn from(text: &str) -> Self {
        Self::new(text, text)
    }
}

impl From<String> for WheelPickerOption {
    fn from(text: String) -> Self {
        Self::new(text.clone(), text)
    }
}

/// A glide from one scroll position to another on a fixed duration.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Glide {
    from: f64,
    to: f64,
    duration_ms: f64,
    ease: Ease,
    /// The frame it started on; `None` until `paint` stamps one.
    start: Option<FrameTime>,
}

/// A live drag: where it was grabbed, and the recent samples a release velocity
/// is averaged over.
struct Drag {
    /// The pointer y the gesture started at.
    anchor_y: f64,
    /// The scroll position it started from.
    anchor_scroll: f64,
    /// `(pointer y, frame)` samples, newest last.
    samples: Vec<(f64, FrameTime)>,
}

/// A view-held, typed change callback (erased on build).
type OnValueChange<State> = Rc<dyn Fn(&mut State, String)>;

/// A declarative beUI wheel picker. See the [module docs](self).
pub struct WheelPicker<State: 'static> {
    options: Vec<WheelPickerOption>,
    value: String,
    visible_count: usize,
    item_height: f64,
    disabled: bool,
    label: Option<String>,
    on_value_change: OnValueChange<State>,
}

/// Create a wheel picker over `options`, showing `value` selected, reporting
/// the newly selected value through `on_value_change` — a **controlled**
/// component (see the [module docs](self)).
pub fn wheel_picker<State: 'static, F: Fn(&mut State, String) + 'static>(
    options: Vec<WheelPickerOption>,
    value: impl Into<String>,
    on_value_change: F,
) -> WheelPicker<State> {
    WheelPicker {
        options,
        value: value.into(),
        visible_count: DEFAULT_VISIBLE_COUNT,
        item_height: DEFAULT_ITEM_HEIGHT,
        disabled: false,
        label: None,
        on_value_change: Rc::new(on_value_change),
    }
}

impl<State: 'static> WheelPicker<State> {
    /// Set how many rows show through the window (`visibleCount`, default 5).
    /// More rows flatten the curve.
    pub fn visible_count(mut self, visible_count: usize) -> Self {
        self.visible_count = visible_count.max(1);
        self
    }

    /// Set the row height, in logical px (`itemHeight`, default 36).
    pub fn item_height(mut self, item_height: f64) -> Self {
        self.item_height = item_height.max(1.0);
        self
    }

    /// Disable the control: 50% opacity and fully inert
    /// (`pointer-events-none opacity-50`).
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Name the control for assistive tech (`aria-label`).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// The index of `value` in the option list, or `0` when it names none —
    /// upstream's `indexOf`.
    fn index_of(&self) -> f64 {
        self.options
            .iter()
            .position(|option| option.value == self.value)
            .unwrap_or(0) as f64
    }
}

/// The resolved wheel palette.
struct WheelColors {
    /// `text-foreground` — the crisp centre row, the band wash and the ring.
    ink: Color,
    /// `text-muted-foreground` — the drum's dimmed rows.
    dim: Color,
    /// `bg-card` — the container.
    card: Color,
    /// `border-border` — the container's hairline.
    border: Color,
}

/// Resolve the palette, falling back to the vendored light table with no theme
/// threaded.
fn resolve_colors(theme: Option<&Theme>) -> WheelColors {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            WheelColors {
                ink: scheme.primary,
                dim: scheme.on_surface_variant,
                card: scheme.surface_container,
                border: scheme.outline_variant,
            }
        }
        None => WheelColors {
            ink: FALLBACK_FOREGROUND,
            dim: FALLBACK_MUTED_FOREGROUND,
            card: FALLBACK_CARD,
            border: FALLBACK_BORDER,
        },
    }
}

/// A row label's style: `font-medium` at the theme's body size.
fn row_style(color: Color) -> TextStyle {
    TextStyle {
        family: sans_family(),
        weight: FontWeight::MEDIUM,
        size: style::TEXT_BASE as f32,
        color,
        ..TextStyle::default()
    }
}

impl<State: 'static> View<State> for WheelPicker<State> {
    type Element = WheelPickerWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> WheelPickerWidget {
        let index = self.index_of();
        WheelPickerWidget {
            labels: self
                .options
                .iter()
                .map(|option| {
                    (
                        ShapedText::new(option.label.clone()),
                        ShapedText::new(option.label.clone()),
                    )
                })
                .collect(),
            options: self.options.clone(),
            value: self.value.clone(),
            emitted: self.value.clone(),
            disabled: self.disabled,
            label: self.label.clone(),
            drum: Drum::resolve(self.visible_count, self.item_height),
            visible_count: self.visible_count,
            item_height: self.item_height,
            scroll: index,
            glide: None,
            drag: None,
            wheel_idle_since: None,
            frame: FrameTime::ZERO,
            size: Size::ZERO,
            on_value_change: erase_callback_arg(&self.on_value_change),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut WheelPickerWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Closures aren't comparable, so the adapter is reinstalled every pass.
        element.on_value_change = erase_callback_arg(&self.on_value_change);
        let mut flags = ChangeFlags::NONE;

        if prev.options != self.options {
            element.options = self.options.clone();
            element.labels = self
                .options
                .iter()
                .map(|option| {
                    (
                        ShapedText::new(option.label.clone()),
                        ShapedText::new(option.label.clone()),
                    )
                })
                .collect();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.visible_count != self.visible_count || prev.item_height != self.item_height {
            element.visible_count = self.visible_count;
            element.item_height = self.item_height;
            element.drum = Drum::resolve(self.visible_count, self.item_height);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.value != self.value {
            // The app is the source of truth: adopt the confirmed value and,
            // unless a gesture owns the drum, glide onto it.
            element.value = self.value.clone();
            element.emitted = self.value.clone();
            let target = self.index_of();
            if element.drag.is_none() && (element.scroll.round() - target).abs() >= 0.001 {
                element.glide = Some(Glide {
                    from: element.scroll,
                    to: target,
                    duration_ms: VALUE_SYNC_MS,
                    ease: Ease::OutCubic,
                    start: None,
                });
            }
            flags |= ChangeFlags::PAINT;
        }
        if prev.disabled != self.disabled {
            element.disabled = self.disabled;
            if self.disabled {
                // A control disabled mid-gesture keeps no armed state behind.
                element.drag = None;
                element.wheel_idle_since = None;
            }
            flags |= ChangeFlags::PAINT;
        }
        if prev.label != self.label {
            element.label = self.label.clone();
            // Semantics-only, but `PAINT` is what bumps the root's semantics
            // dirty gate and there is no narrower flag.
            flags |= ChangeFlags::PAINT;
        }
        flags
    }
}

/// The retained widget for a [`WheelPicker`].
pub struct WheelPickerWidget {
    options: Vec<WheelPickerOption>,
    /// One `(dimmed, crisp)` shaped run per option — the drum paints the first,
    /// the centre band the second.
    labels: Vec<(ShapedText, ShapedText)>,
    /// The app-confirmed value (source of truth; adopted on `rebuild`).
    value: String,
    /// The value last reported, so a gesture that crosses the same row twice
    /// reports it once.
    emitted: String,
    disabled: bool,
    label: Option<String>,
    drum: Drum,
    visible_count: usize,
    item_height: f64,
    /// The drum's position, measured in rows — a float index, and the one
    /// source of truth for both painted layers.
    scroll: f64,
    glide: Option<Glide>,
    drag: Option<Drag>,
    /// When the last wheel notch landed, so the settle can fire once the wheel
    /// goes quiet.
    wheel_idle_since: Option<FrameTime>,
    /// The last painted frame — the clock an event pass borrows, since an
    /// `EventCtx` carries none.
    frame: FrameTime,
    size: Size,
    on_value_change: frust::authoring::ErasedArgCallback<String>,
}

impl WheelPickerWidget {
    /// The index of the last option.
    fn last(&self) -> f64 {
        (self.options.len().max(1) - 1) as f64
    }

    /// Clamp a float index into the option list.
    fn clamp_index(&self, index: f64) -> f64 {
        index.clamp(0.0, self.last())
    }

    /// Report the value at `index`, unless it is the one already reported —
    /// upstream's `emit`.
    fn emit(&mut self, ctx: &mut EventCtx, index: f64) {
        let at = self.clamp_index(index).round() as usize;
        let Some(option) = self.options.get(at) else {
            return;
        };
        if option.value == self.emitted {
            return;
        }
        self.emitted = option.value.clone();
        let value = option.value.clone();
        (self.on_value_change)(ctx, value);
    }

    /// Start a glide from wherever the drum is to an integer detent.
    fn glide_to(&mut self, to: f64, duration_ms: f64, ease: Ease) {
        self.glide = Some(Glide {
            from: self.scroll,
            to,
            duration_ms,
            ease,
            start: None,
        });
    }

    /// Where a flick of `velocity` (rows per millisecond) coasts to, and how
    /// long the settle takes — upstream's `fling`.
    fn fling_target(&self, velocity: f64) -> (f64, f64) {
        let from = self.scroll;
        if from < 0.0 || from > self.last() {
            // Rubber-band back rather than coasting away from the ends.
            return (self.clamp_index(from.round()), REBOUND_MS);
        }
        let coast = ((velocity * velocity) / (2.0 * DECELERATION)) * velocity.signum();
        let to = self.clamp_index((from + coast).round());
        let duration = ((to - from).abs().sqrt() * 300.0 + 240.0).clamp(FLING_MIN_MS, FLING_MAX_MS);
        (to, duration)
    }

    /// The release velocity, in rows per millisecond, averaged over the last
    /// [`VELOCITY_WINDOW_MS`] of samples rather than the final two — a single
    /// noisy frame otherwise makes an even flick feel like it caught.
    fn release_velocity(&self, drag: &Drag) -> f64 {
        let samples = &drag.samples;
        if samples.len() < 2 {
            return 0.0;
        }
        let latest = samples[samples.len() - 1];
        let mut reference = samples[0];
        for sample in samples {
            if latest.1.saturating_sub(sample.1).as_secs_f64() * 1000.0 <= VELOCITY_WINDOW_MS {
                reference = *sample;
                break;
            }
        }
        let dt = latest.1.saturating_sub(reference.1).as_secs_f64() * 1000.0;
        if dt <= 0.0 {
            return 0.0;
        }
        ((reference.0 - latest.0) / self.item_height / dt).clamp(-MAX_VELOCITY, MAX_VELOCITY)
    }

    /// Advance any running glide to `now`, returning whether one is still in
    /// flight.
    fn advance(&mut self, now: FrameTime) -> bool {
        let Some(mut glide) = self.glide else {
            return false;
        };
        let start = *glide.start.get_or_insert(now);
        self.glide = Some(glide);
        let distance = glide.to - glide.from;
        if distance == 0.0 || glide.duration_ms <= 0.0 {
            self.scroll = glide.to;
            self.glide = None;
            return false;
        }
        let elapsed = now.saturating_sub(start).as_secs_f64() * 1000.0;
        if elapsed >= glide.duration_ms {
            self.scroll = glide.to;
            self.glide = None;
            return false;
        }
        self.scroll = glide.from + distance * glide.ease.at(elapsed / glide.duration_ms);
        true
    }

    /// Land any running glide immediately — the `reduce_motion` path.
    fn settle_now(&mut self) {
        if let Some(glide) = self.glide.take() {
            self.scroll = glide.to;
        }
    }

    /// The centre band's box, in local coordinates.
    fn band(&self, size: Size) -> Rect {
        Rect::from_origin_size(
            Point::new(0.0, (size.height - self.item_height) / 2.0),
            Size::new(size.width, self.item_height),
        )
    }

    /// Paint the drum's rows. `crisp` picks the centre band's copy — the very
    /// same rows, driven by the identical placement so the two register
    /// exactly, with no parallax ghost as a row crosses the window.
    fn paint_rows(
        &self,
        scene: &mut dyn PaintScene,
        origin: Point,
        size: Size,
        crisp: bool,
        disabled_alpha: f32,
    ) {
        let centre_y = origin.y + size.height / 2.0;
        for (index, (dim, ink)) in self.labels.iter().enumerate() {
            let placement = self.drum.place(index as f64, self.scroll);
            if placement.hidden {
                continue;
            }
            let row_centre = centre_y + placement.offset;
            let alpha = mask_alpha(row_centre - origin.y, size.height) * f64::from(disabled_alpha);
            if alpha <= 0.0 {
                continue;
            }
            let run = if crisp { ink } else { dim };
            let text = run.size();
            let at = Point::new(
                origin.x + (size.width - text.width) / 2.0,
                row_centre - text.height / 2.0,
            );
            let pivot = Point::new(origin.x + size.width / 2.0, row_centre);
            scene.push_layer(
                Point::new(origin.x, row_centre - self.item_height),
                Size::new(size.width, self.item_height * 2.0),
                alpha as f32,
            );
            scene.push_transform(
                Affine::translate(pivot.to_vec2())
                    * Affine::scale_non_uniform(placement.scale_x, placement.scale_y)
                    * Affine::translate(-pivot.to_vec2()),
            );
            run.paint(at, scene);
            scene.pop_transform();
            scene.pop_layer();
        }
    }
}

impl Widget for WheelPickerWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let colors = resolve_colors(theme);
        let dim = row_style(colors.dim);
        let ink = row_style(colors.ink);
        for (dimmed, crisp) in &mut self.labels {
            dimmed.layout(ctx, &dim);
            crisp.layout(ctx, &ink);
        }

        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            UNBOUNDED_WIDTH
        };
        self.size = bc.constrain(Size::new(width, self.drum.height));
        self.size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let colors = resolve_colors(theme);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let focused = ctx.has_focus();
        let now = ctx.frame_time();
        self.frame = now;

        let mut owes_frame = false;
        if reduce {
            self.settle_now();
            self.wheel_idle_since = None;
        } else {
            owes_frame |= self.advance(now);
            // The wheel drives `scroll` continuously and snaps once it goes
            // quiet: a fresh eased step per notch would stack overlapping
            // animations that keep interrupting each other, which reads as lag.
            if let Some(since) = self.wheel_idle_since {
                if now.saturating_sub(since).as_secs_f64() * 1000.0 >= WHEEL_SETTLE_MS {
                    self.wheel_idle_since = None;
                    let to = self.clamp_index(self.scroll.round());
                    self.glide_to(to, WHEEL_SNAP_MS, Ease::OutBack);
                    owes_frame = true;
                } else {
                    owes_frame = true;
                }
            }
        }

        let origin = ctx.origin();
        let size = ctx.size();
        let opacity = if self.disabled {
            style::DISABLED_OPACITY
        } else {
            1.0
        };
        let tint =
            |color: Color| style::disabled_tint(color, self.disabled, style::DISABLED_OPACITY);

        scene.fill_rounded_rect(origin, size, style::RADIUS_2XL, tint(colors.card));
        scene.push_clip_rounded(origin, size, style::RADIUS_2XL);

        // The curved drum of dimmed rows.
        self.paint_rows(scene, origin, size, false, opacity);

        // The centre band: the very same drum, clipped to one row and drawn
        // crisp over its own faint wash.
        let band = self.band(size);
        let band_origin = origin + band.origin().to_vec2();
        scene.fill_rounded_rect(
            band_origin,
            band.size(),
            style::RADIUS_MD,
            tint(style::with_alpha(colors.ink, BAND_ALPHA)),
        );
        scene.push_clip_rounded(band_origin, band.size(), style::RADIUS_MD);
        self.paint_rows(scene, origin, size, true, opacity);
        scene.pop_clip();

        scene.pop_clip();

        let inset = style::BORDER_WIDTH / 2.0;
        let outline = RoundedRect::from_rect(
            Rect::from_origin_size(Point::ORIGIN, size).inset(-inset),
            (style::RADIUS_2XL - inset).max(0.0),
        );
        scene.stroke_path(
            origin,
            &Shape::to_path(&outline, style::PATH_TOLERANCE),
            style::BORDER_WIDTH,
            &Brush::Solid(tint(colors.border)),
        );

        if focused {
            let out = style::FOCUS_RING_WIDTH / 2.0;
            let ring = RoundedRect::from_rect(
                Rect::from_origin_size(Point::ORIGIN, size).inset(out),
                style::RADIUS_2XL + out,
            );
            scene.stroke_path(
                origin,
                &Shape::to_path(&ring, style::PATH_TOLERANCE),
                style::FOCUS_RING_WIDTH,
                &Brush::Solid(tint(style::with_alpha(colors.ink, RING_ALPHA))),
            );
        }

        // Paint-only animation (the window never resizes), so a bare frame
        // request is the right one — never `request_layout`.
        if owes_frame {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // `pointer-events-none` on a disabled picker, and `tabIndex={-1}`, so
        // it takes neither pointer nor key.
        if self.disabled {
            return EventResult::Ignored;
        }
        let size = ctx.size();
        let frame = self.frame;
        match event {
            InputEvent::Key(key) => {
                let at = self.scroll.round();
                let to = match &key.key {
                    Key::Named(NamedKey::ArrowUp) => at - 1.0,
                    Key::Named(NamedKey::ArrowDown) => at + 1.0,
                    Key::Named(NamedKey::Home) => 0.0,
                    Key::Named(NamedKey::End) => self.last(),
                    _ => return EventResult::Ignored,
                };
                let to = self.clamp_index(to);
                self.wheel_idle_since = None;
                self.glide_to(to, KEY_STEP_MS, Ease::OutBack);
                self.emit(ctx, to);
                ctx.request_redraw();
                EventResult::Handled
            }
            InputEvent::Scroll { position, delta } => {
                if !inside(*position, size) {
                    return EventResult::Ignored;
                }
                let pixels = match delta {
                    ScrollDelta::Lines(_, y) => y * WHEEL_LINE_HEIGHT,
                    ScrollDelta::Pixels(_, y) => *y,
                };
                self.glide = None;
                self.scroll = self.clamp_index(self.scroll + pixels * WHEEL_SENSITIVITY);
                let landed = self.scroll.round();
                self.emit(ctx, landed);
                // Re-arm the idle timer: the snap fires once the wheel stops.
                self.wheel_idle_since = Some(frame);
                ctx.request_focus();
                ctx.request_redraw();
                EventResult::Handled
            }
            InputEvent::Pointer(p) => match p.phase {
                PointerPhase::Down => {
                    if !presses(p) || !inside(p.position, size) {
                        return EventResult::Ignored;
                    }
                    self.glide = None;
                    self.wheel_idle_since = None;
                    self.drag = Some(Drag {
                        anchor_y: p.position.y,
                        anchor_scroll: self.scroll,
                        samples: vec![(p.position.y, frame)],
                    });
                    ctx.capture_pointer();
                    ctx.request_focus();
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Move => {
                    let Some(drag) = &mut self.drag else {
                        if inside(p.position, size) {
                            ctx.claim_hover();
                            ctx.set_cursor(CursorIcon::Grab);
                        }
                        return EventResult::Ignored;
                    };
                    drag.samples.push((p.position.y, frame));
                    if drag.samples.len() > 8 {
                        drag.samples.remove(0);
                    }
                    let mut next =
                        drag.anchor_scroll + (drag.anchor_y - p.position.y) / self.item_height;
                    // Rubber-band past either end rather than stopping dead.
                    let last = self.last();
                    if next < 0.0 {
                        next *= RUBBER_BAND;
                    } else if next > last {
                        next = last + (next - last) * RUBBER_BAND;
                    }
                    self.scroll = next;
                    // The captured arm re-asks every move, which keeps
                    // `Grabbing` alive outside the widget's own bounds.
                    ctx.set_cursor(CursorIcon::Grabbing);
                    self.emit(ctx, next);
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Up => {
                    let Some(drag) = self.drag.take() else {
                        return EventResult::Ignored;
                    };
                    let velocity = self.release_velocity(&drag);
                    let (to, duration) = self.fling_target(velocity);
                    self.glide_to(to, duration, Ease::OutBack);
                    // The landing notch is already known, so it is reported
                    // here rather than from the coasting frames — see the
                    // module docs.
                    self.emit(ctx, to);
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Cancel => {
                    if self.drag.take().is_none() {
                        return EventResult::Ignored;
                    }
                    // A cancel returns to the app-confirmed row rather than
                    // committing wherever the finger was taken.
                    let to = self.clamp_index(
                        self.options
                            .iter()
                            .position(|option| option.value == self.value)
                            .unwrap_or(0) as f64,
                    );
                    self.glide_to(to, REBOUND_MS, Ease::OutCubic);
                    ctx.request_redraw();
                    EventResult::Handled
                }
            },
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let selected = self.clamp_index(self.scroll.round()) as usize;
        ctx.push_container(
            Role::ListBox,
            |node| {
                if let Some(label) = &self.label {
                    node.set_label(label.as_str());
                }
                if self.disabled {
                    node.set_disabled();
                }
            },
            |ctx| {
                for (index, option) in self.options.iter().enumerate() {
                    ctx.push_node(Role::ListBoxOption, |node| {
                        node.set_label(option.label.as_str());
                        node.set_selected(index == selected);
                        if self.disabled {
                            node.set_disabled();
                        } else {
                            node.add_action(Action::Click);
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
        BezPath, EventOutcome, KeyEvent, Modifiers, PointerButton, PointerEvent, SemanticsUpdate,
        scene::GlyphRun,
    };
    use frust_core::RenderRoot;
    use std::any::Any;

    /// Records the fills, clips, strokes, layer alphas, transform pushes and
    /// glyph origins this widget emits.
    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<(Rect, f64, Color)>,
        clips: Vec<(Point, Size)>,
        layers: Vec<f32>,
        transforms: Vec<Affine>,
        glyphs: Vec<Point>,
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
        fn push_clip_rounded(&mut self, origin: Point, size: Size, _radius: f64) {
            self.clips.push((origin, size));
        }
        fn push_layer(&mut self, _o: Point, _s: Size, alpha: f32) {
            self.layers.push(alpha);
        }
        fn push_transform(&mut self, transform: Affine) {
            self.transforms.push(transform);
        }
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            let t = run.transform.translation();
            self.glyphs.push(Point::new(t.x, t.y));
        }
    }

    #[derive(Default)]
    struct Picked {
        last: Option<String>,
        count: u32,
    }

    const WIDTH: f64 = 160.0;

    fn ft_ms(millis: f64) -> FrameTime {
        FrameTime::from_nanos((millis * 1_000_000.0) as u64)
    }

    fn options(count: usize) -> Vec<WheelPickerOption> {
        (0..count)
            .map(|index| WheelPickerOption::from(format!("{index:02}")))
            .collect()
    }

    fn view(value: &str, count: usize) -> WheelPicker<Picked> {
        wheel_picker::<Picked, _>(options(count), value, |s: &mut Picked, v: String| {
            s.last = Some(v);
            s.count += 1;
        })
        .label("minutes")
    }

    fn laid_out(view: &WheelPicker<Picked>) -> (WheelPickerWidget, Size) {
        let mut counter = 0u64;
        let mut w = View::<Picked>::build(view, &mut BuildCtx::new(&mut counter));
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let size = w.layout(
            &mut ctx,
            &BoxConstraints::new(Size::ZERO, Size::new(WIDTH, 600.0)),
        );
        (w, size)
    }

    fn paint_at(
        w: &mut WheelPickerWidget,
        size: Size,
        theme: Option<&Theme>,
        millis: f64,
    ) -> (Recorder, bool) {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(millis));
        if let Some(t) = theme {
            ctx = ctx.with_theme(t);
        }
        w.paint(&mut ctx, &mut rec);
        (rec, ctx.needs_frame())
    }

    fn paint(w: &mut WheelPickerWidget, size: Size) -> Recorder {
        paint_at(w, size, None, 0.0).0
    }

    fn pointer(phase: PointerPhase, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(WIDTH / 2.0, y),
            button: PointerButton::Primary,
        })
    }

    fn key(named: NamedKey) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Named(named),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    fn wheel(size: Size, pixels: f64) -> InputEvent {
        InputEvent::Scroll {
            position: Point::new(WIDTH / 2.0, size.height / 2.0),
            delta: ScrollDelta::Pixels(0.0, pixels),
        }
    }

    fn dispatch(
        w: &mut WheelPickerWidget,
        size: Size,
        state: &mut Picked,
        event: &InputEvent,
    ) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, size);
        w.event(&mut ctx, event)
    }

    // ---- Geometry ---------------------------------------------------------

    #[test]
    fn the_drum_resolves_the_authored_cylinder() {
        let drum = Drum::resolve(DEFAULT_VISIBLE_COUNT, DEFAULT_ITEM_HEIGHT);
        // 5 visible → 2 rows each side → a cutoff of 3 → 30° per row.
        assert_eq!(drum.item_angle, 30.0);
        assert_eq!(drum.hide_beyond, 3.0);
        assert!((drum.radius - DEFAULT_ITEM_HEIGHT / 30.0_f64.to_radians().tan()).abs() < 1e-9);
        let expected =
            (2.0 * drum.radius * 60.0_f64.to_radians().sin() + DEFAULT_ITEM_HEIGHT).round();
        assert_eq!(drum.height, expected);
        // More visible rows flatten the curve: a smaller angle per row.
        assert!(Drum::resolve(9, DEFAULT_ITEM_HEIGHT).item_angle < drum.item_angle);
        // A degenerate count still leaves one row each side.
        assert_eq!(Drum::resolve(0, DEFAULT_ITEM_HEIGHT).hide_beyond, 2.0);
    }

    #[test]
    fn the_selected_row_faces_the_viewer_and_its_neighbours_recede() {
        let drum = Drum::resolve(5, DEFAULT_ITEM_HEIGHT);
        let centre = drum.place(3.0, 3.0);
        assert!(centre.offset.abs() < 1e-9, "dead centre");
        assert!((centre.scale_x - 1.0).abs() < 1e-9, "and unprojected");
        assert!((centre.scale_y - 1.0).abs() < 1e-9);
        assert!(!centre.hidden);

        // The row below sits below the centre, smaller, and squashed further
        // vertically than horizontally — that is the `rotateX`.
        let below = drum.place(4.0, 3.0);
        assert!(below.offset > 0.0, "below the centre");
        assert!(below.scale_x < 1.0 && below.scale_y < below.scale_x);
        // ...and the row above mirrors it exactly.
        let above = drum.place(2.0, 3.0);
        assert!((above.offset + below.offset).abs() < 1e-9);
        assert!((above.scale_x - below.scale_x).abs() < 1e-9);

        // Past the horizon, a row is dropped rather than painted edge-on.
        assert!(drum.place(7.0, 3.0).hidden);
        assert!(!drum.place(6.0, 3.0).hidden);
    }

    #[test]
    fn the_mask_ramp_fades_the_top_and_bottom_bands() {
        let height = 100.0;
        assert_eq!(mask_alpha(0.0, height), 0.0);
        assert_eq!(mask_alpha(height, height), 0.0);
        assert_eq!(mask_alpha(height / 2.0, height), 1.0);
        assert_eq!(mask_alpha(MASK_STOP * height, height), 1.0);
        assert!((mask_alpha(MASK_STOP * height / 2.0, height) - 0.5).abs() < 1e-9);
        // A degenerate window does not divide by zero.
        assert_eq!(mask_alpha(0.0, 0.0), 1.0);
    }

    #[test]
    fn the_two_easings_are_anchored_and_only_one_overshoots() {
        for ease in [Ease::OutCubic, Ease::OutBack] {
            assert_eq!(ease.at(0.0), 0.0);
            assert!((ease.at(1.0) - 1.0).abs() < 1e-9);
            // Out of range clamps rather than running away.
            assert_eq!(ease.at(-1.0), 0.0);
            assert!((ease.at(4.0) - 1.0).abs() < 1e-9);
        }
        let peak = (0..=100)
            .map(|step| Ease::OutBack.at(f64::from(step) / 100.0))
            .fold(0.0_f64, f64::max);
        assert!(peak > 1.0, "the back easing drifts past its target: {peak}");
        let cubic = (0..=100)
            .map(|step| Ease::OutCubic.at(f64::from(step) / 100.0))
            .fold(0.0_f64, f64::max);
        assert!(cubic <= 1.0 + 1e-9, "the plain settle never does");
    }

    // ---- Layout / paint ---------------------------------------------------

    #[test]
    fn the_window_is_the_drums_own_height_and_carries_a_centre_band() {
        let (mut w, size) = laid_out(&view("03", 12));
        assert_eq!(size, Size::new(WIDTH, w.drum.height));
        let rec = paint(&mut w, size);

        // The card, then the band, then the crisp copy's own rows.
        assert_eq!(rec.rrects[0].1, size);
        assert_eq!(rec.rrects[0].2, style::RADIUS_2XL, "rounded-2xl");
        let band = rec
            .rrects
            .iter()
            .find(|(_, s, ..)| s.height == DEFAULT_ITEM_HEIGHT)
            .expect("the centre band");
        assert_eq!(band.2, style::RADIUS_MD);
        assert_eq!(band.3.components[3], BAND_ALPHA);
        assert!(
            (band.0.y + DEFAULT_ITEM_HEIGHT / 2.0 - size.height / 2.0).abs() < 1e-9,
            "and it is centred"
        );
        // The container is clipped, and so is the band.
        assert_eq!(rec.clips.len(), 2);
        assert_eq!(rec.strokes.len(), 1, "the hairline border, and no ring");
    }

    #[test]
    fn only_the_rows_inside_the_horizon_are_painted_twice() {
        let (mut w, size) = laid_out(&view("06", 40));
        let rec = paint(&mut w, size);
        // Every painted row is composited through its own layer, once for the
        // dimmed drum and once for the crisp band copy.
        let visible = rec.layers.len();
        assert!(visible > 0);
        assert_eq!(visible % 2, 0, "the same rows, twice");
        // Never more than the horizon allows on each side of the centre.
        let per_pass = visible / 2;
        assert!(
            per_pass <= (2.0 * w.drum.hide_beyond) as usize + 1,
            "{per_pass} rows painted past a cutoff of {}",
            w.drum.hide_beyond
        );
        assert_eq!(rec.glyphs.len(), visible, "one run per painted row");
    }

    #[test]
    fn the_row_under_the_needle_is_the_only_unscaled_one() {
        let (mut w, size) = laid_out(&view("05", 20));
        let rec = paint(&mut w, size);
        let scales: Vec<f64> = rec.transforms.iter().map(|t| t.as_coeffs()[0]).collect();
        let unscaled = scales.iter().filter(|s| (**s - 1.0).abs() < 1e-9).count();
        assert_eq!(unscaled, 2, "the selected row, in both passes");
        assert!(
            scales.iter().all(|s| *s <= 1.0 + 1e-9),
            "a perspective divide only ever shrinks"
        );
    }

    #[test]
    fn disabled_paint_halves_every_painted_alpha() {
        let theme = crate::theme();
        let (mut enabled, size) = laid_out(&view("03", 12));
        let (mut disabled, _) = laid_out(&view("03", 12).disabled(true));
        let on = paint_at(&mut enabled, size, Some(&theme), 0.0).0;
        let off = paint_at(&mut disabled, size, Some(&theme), 0.0).0;
        assert!(
            (off.rrects[0].3.components[3]
                - on.rrects[0].3.components[3] * style::DISABLED_OPACITY)
                .abs()
                < 1e-6
        );
        // The rows dim through their own layer alpha, not their colour.
        assert!((off.layers[0] - on.layers[0] * style::DISABLED_OPACITY).abs() < 1e-6);
    }

    // ---- Drag, fling and snap ---------------------------------------------

    #[test]
    fn dragging_reports_the_row_under_the_window_as_it_passes() {
        let (mut w, size) = laid_out(&view("00", 20));
        let mut state = Picked::default();
        paint(&mut w, size);
        dispatch(
            &mut w,
            size,
            &mut state,
            &pointer(PointerPhase::Down, 100.0),
        );
        assert!(w.drag.is_some());

        // Drag up by exactly three rows.
        dispatch(
            &mut w,
            size,
            &mut state,
            &pointer(PointerPhase::Move, 100.0 - 3.0 * DEFAULT_ITEM_HEIGHT),
        );
        assert!((w.scroll - 3.0).abs() < 1e-9);
        assert_eq!(state.last.as_deref(), Some("03"));
        assert_eq!(w.value, "00", "the app owns the value");
    }

    #[test]
    fn a_release_snaps_to_a_whole_row_and_reports_it_once() {
        let (mut w, size) = laid_out(&view("00", 20));
        let mut state = Picked::default();
        paint(&mut w, size);
        dispatch(
            &mut w,
            size,
            &mut state,
            &pointer(PointerPhase::Down, 100.0),
        );
        // Land between two rows.
        dispatch(
            &mut w,
            size,
            &mut state,
            &pointer(PointerPhase::Move, 100.0 - 2.4 * DEFAULT_ITEM_HEIGHT),
        );
        assert!((w.scroll - 2.4).abs() < 1e-9);
        dispatch(
            &mut w,
            size,
            &mut state,
            &pointer(PointerPhase::Up, 100.0 - 2.4 * DEFAULT_ITEM_HEIGHT),
        );

        let glide = w.glide.expect("a settle glide");
        assert_eq!(glide.to, glide.to.round(), "onto a whole notch");
        assert_eq!(glide.to, 2.0, "the nearest one, with no velocity to carry");
        assert_eq!(glide.ease, Ease::OutBack, "with the little bounce");
        assert_eq!(state.last.as_deref(), Some("02"));

        // The glide plays out and lands exactly on the notch.
        paint_at(&mut w, size, None, 0.0);
        let (_, owes) = paint_at(&mut w, size, None, 10.0);
        assert!(owes, "a running glide owes the next frame");
        let (_, owes) = paint_at(&mut w, size, None, FLING_MAX_MS + 10.0);
        assert!(!owes);
        assert_eq!(w.scroll, 2.0);
        assert!(w.glide.is_none());
    }

    #[test]
    fn a_flick_coasts_past_where_the_finger_stopped() {
        let (mut w, size) = laid_out(&view("00", 40));
        let mut state = Picked::default();
        // Frames advance while the gesture runs, which is the clock the
        // velocity estimate borrows.
        let grab = size.height - 10.0;
        paint_at(&mut w, size, None, 0.0);
        dispatch(&mut w, size, &mut state, &pointer(PointerPhase::Down, grab));
        for frame in 1..=4 {
            paint_at(&mut w, size, None, f64::from(frame) * 16.0);
            dispatch(
                &mut w,
                size,
                &mut state,
                &pointer(
                    PointerPhase::Move,
                    grab - f64::from(frame) * DEFAULT_ITEM_HEIGHT,
                ),
            );
        }
        let stopped = w.scroll;
        dispatch(
            &mut w,
            size,
            &mut state,
            &pointer(PointerPhase::Up, grab - 4.0 * DEFAULT_ITEM_HEIGHT),
        );
        let glide = w.glide.expect("a fling");
        assert!(
            glide.to > stopped,
            "a flick carries past {stopped} to {}",
            glide.to
        );
        assert!(glide.duration_ms >= FLING_MIN_MS && glide.duration_ms <= FLING_MAX_MS);
        assert_eq!(
            state.last.as_deref(),
            Some(w.options[glide.to as usize].value.as_str())
        );
    }

    #[test]
    fn a_drag_past_an_end_rubber_bands_and_settles_back_inside() {
        let (mut w, size) = laid_out(&view("00", 6));
        let mut state = Picked::default();
        paint(&mut w, size);
        dispatch(
            &mut w,
            size,
            &mut state,
            &pointer(PointerPhase::Down, 100.0),
        );
        // Pull four rows above the first one.
        dispatch(
            &mut w,
            size,
            &mut state,
            &pointer(PointerPhase::Move, 100.0 + 4.0 * DEFAULT_ITEM_HEIGHT),
        );
        assert!(
            w.scroll < 0.0 && w.scroll >= -4.0 * RUBBER_BAND,
            "only {RUBBER_BAND} of the over-drag is kept: {}",
            w.scroll
        );
        assert_eq!(state.count, 0, "and nothing past the first row is reported");

        dispatch(
            &mut w,
            size,
            &mut state,
            &pointer(PointerPhase::Up, 100.0 + 4.0 * DEFAULT_ITEM_HEIGHT),
        );
        assert_eq!(w.glide.expect("a rebound").to, 0.0);
    }

    #[test]
    fn a_cancel_returns_to_the_app_confirmed_row() {
        let (mut w, size) = laid_out(&view("04", 20));
        let mut state = Picked::default();
        paint(&mut w, size);
        assert_eq!(w.scroll, 4.0);
        dispatch(
            &mut w,
            size,
            &mut state,
            &pointer(PointerPhase::Down, 100.0),
        );
        dispatch(
            &mut w,
            size,
            &mut state,
            &pointer(PointerPhase::Move, 100.0 - 3.0 * DEFAULT_ITEM_HEIGHT),
        );
        dispatch(
            &mut w,
            size,
            &mut state,
            &pointer(PointerPhase::Cancel, 0.0),
        );
        assert_eq!(w.glide.expect("a rebound").to, 4.0);
        assert!(w.drag.is_none());
    }

    // ---- Wheel and keys ---------------------------------------------------

    #[test]
    fn the_wheel_drives_the_drum_and_snaps_once_it_goes_quiet() {
        let (mut w, size) = laid_out(&view("00", 20));
        let mut state = Picked::default();
        paint_at(&mut w, size, None, 0.0);
        // Two rows' worth of pixels, at 0.012 rows per px.
        let pixels = 2.0 / WHEEL_SENSITIVITY;
        dispatch(&mut w, size, &mut state, &wheel(size, pixels));
        assert!((w.scroll - 2.0).abs() < 1e-9);
        assert_eq!(state.last.as_deref(), Some("02"));
        assert!(w.wheel_idle_since.is_some(), "the settle timer is armed");

        // Still inside the idle window: nothing has snapped, but a frame is owed.
        let (_, owes) = paint_at(&mut w, size, None, WHEEL_SETTLE_MS / 2.0);
        assert!(owes);
        assert!(w.glide.is_none());

        // Past it: the snap glide starts.
        paint_at(&mut w, size, None, WHEEL_SETTLE_MS + 1.0);
        assert_eq!(w.glide.expect("the settle").to, 2.0);
        assert!(w.wheel_idle_since.is_none());
    }

    #[test]
    fn a_line_quantised_wheel_is_measured_in_pixels_per_line() {
        let (mut w, size) = laid_out(&view("00", 20));
        let mut state = Picked::default();
        paint(&mut w, size);
        let lines = InputEvent::Scroll {
            position: Point::new(WIDTH / 2.0, size.height / 2.0),
            delta: ScrollDelta::Lines(0.0, 5.0),
        };
        dispatch(&mut w, size, &mut state, &lines);
        assert!((w.scroll - 5.0 * WHEEL_LINE_HEIGHT * WHEEL_SENSITIVITY).abs() < 1e-9);
    }

    #[test]
    fn the_arrow_and_end_keys_step_the_drum() {
        let (mut w, size) = laid_out(&view("05", 20));
        let mut state = Picked::default();
        paint(&mut w, size);
        dispatch(&mut w, size, &mut state, &key(NamedKey::ArrowDown));
        assert_eq!(w.glide.expect("a step").to, 6.0);
        assert_eq!(state.last.as_deref(), Some("06"));

        dispatch(&mut w, size, &mut state, &key(NamedKey::Home));
        assert_eq!(w.glide.expect("a step").to, 0.0);
        assert_eq!(state.last.as_deref(), Some("00"));

        dispatch(&mut w, size, &mut state, &key(NamedKey::End));
        assert_eq!(w.glide.expect("a step").to, 19.0);
        assert_eq!(state.last.as_deref(), Some("19"));

        // An unrelated key is not a step.
        assert_eq!(
            dispatch(&mut w, size, &mut state, &key(NamedKey::Tab)),
            EventResult::Ignored
        );
    }

    #[test]
    fn a_step_at_an_end_stays_inside_the_list() {
        let (mut w, size) = laid_out(&view("00", 6));
        let mut state = Picked::default();
        paint(&mut w, size);
        dispatch(&mut w, size, &mut state, &key(NamedKey::ArrowUp));
        assert_eq!(w.glide.expect("a step").to, 0.0);
        assert_eq!(
            state.count, 0,
            "the value it already holds is not re-reported"
        );
    }

    #[test]
    fn a_disabled_picker_takes_no_pointer_wheel_or_key() {
        let (mut w, size) = laid_out(&view("03", 20).disabled(true));
        let mut state = Picked::default();
        for event in [
            pointer(PointerPhase::Down, 100.0),
            wheel(size, 200.0),
            key(NamedKey::ArrowDown),
        ] {
            assert_eq!(
                dispatch(&mut w, size, &mut state, &event),
                EventResult::Ignored
            );
        }
        assert_eq!(state.count, 0);
        assert!(w.drag.is_none() && w.glide.is_none());
    }

    // ---- Controlled reconcile and reduced motion --------------------------

    #[test]
    fn a_confirmed_value_glides_the_drum_onto_its_row() {
        let mut counter = 0u64;
        let prev = view("00", 20);
        let mut w = View::<Picked>::build(&prev, &mut BuildCtx::new(&mut counter));
        let next = view("07", 20);
        View::<Picked>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        let glide = w.glide.expect("a sync glide");
        assert_eq!(glide.to, 7.0);
        assert_eq!(glide.duration_ms, VALUE_SYNC_MS);
        assert_eq!(glide.ease, Ease::OutCubic, "no bounce on a value sync");
        assert_eq!(w.emitted, "07", "and it will not be echoed back");

        // A value the drum already sits on does not restart anything.
        let same = view("07", 20);
        w.scroll = 7.0;
        w.glide = None;
        View::<Picked>::rebuild(&same, &next, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(w.glide.is_none());
    }

    #[test]
    fn an_unknown_value_parks_the_drum_on_the_first_row() {
        let (w, _) = laid_out(&view("nope", 20));
        assert_eq!(w.scroll, 0.0);
    }

    #[test]
    fn reduce_motion_lands_every_glide_at_once_and_owes_no_frame() {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        let (mut w, size) = laid_out(&view("00", 20));
        let mut state = Picked::default();
        dispatch(&mut w, size, &mut state, &key(NamedKey::ArrowDown));
        assert!(w.glide.is_some());
        let (_, owes) = paint_at(&mut w, size, Some(&theme), 0.0);
        assert!(!owes, "reduce_motion asks for no animation frame");
        assert_eq!(w.scroll, 1.0, "landed at once");
        assert!(w.glide.is_none());
    }

    // ---- Root-driven: focus ring, cursor, semantics ------------------------

    struct Harness {
        root: RenderRoot<Picked, WheelPicker<Picked>>,
        state: Picked,
        tcx: TextContext,
    }

    impl Harness {
        fn new(disabled: bool) -> Self {
            let mut h = Harness {
                root: RenderRoot::new(),
                state: Picked::default(),
                tcx: TextContext::new(),
            };
            h.root.set_theme(Box::new(crate::theme()));
            let mut logic = move |_s: &mut Picked| view("03", 12).disabled(disabled);
            h.root.rebuild(&mut logic, &mut h.state);
            h.root
                .layout_with_text(Size::new(WIDTH, 400.0), &mut h.tcx as &mut dyn Any);
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
    fn focus_paints_the_ring_and_a_move_asks_for_grab() {
        let mut h = Harness::new(false);
        assert_eq!(h.paint().strokes.len(), 1, "the border only, at rest");

        h.dispatch(&pointer(PointerPhase::Move, 40.0));
        assert_eq!(h.root.cursor(), CursorIcon::Grab);

        h.dispatch(&pointer(PointerPhase::Down, 40.0));
        assert!(h.root.is_focus_active());
        let rec = h.paint();
        assert_eq!(rec.strokes.len(), 2, "border + ring");
        let (bbox, width, color) = rec.strokes[1];
        assert_eq!(width, style::FOCUS_RING_WIDTH);
        assert_eq!(color.components[3], RING_ALPHA);
        assert!(bbox.x0 < 0.0, "the ring sits outside the box");

        h.dispatch(&pointer(PointerPhase::Move, 60.0));
        assert_eq!(h.root.cursor(), CursorIcon::Grabbing);
    }

    #[test]
    fn semantics_reports_a_listbox_of_options_with_the_centre_one_selected() {
        let h = Harness::new(false);
        let update = h.semantics();
        let (_, listbox) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::ListBox)
            .expect("a Role::ListBox node");
        assert_eq!(listbox.label(), Some("minutes"));

        let rows: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::ListBoxOption)
            .collect();
        assert_eq!(rows.len(), 12, "one node per option");
        let selected: Vec<_> = rows
            .iter()
            .filter(|(_, n)| n.is_selected().unwrap_or(false))
            .collect();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].1.label(), Some("03"));
        assert!(rows[0].1.supports_action(Action::Click));
    }

    #[test]
    fn a_disabled_picker_reports_itself_disabled() {
        let h = Harness::new(true);
        let update = h.semantics();
        let (_, listbox) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::ListBox)
            .expect("a Role::ListBox node");
        assert!(listbox.is_disabled());
    }

    #[test]
    fn the_option_union_accepts_plain_strings_and_labelled_pairs() {
        assert_eq!(
            WheelPickerOption::from("07"),
            WheelPickerOption::new("07", "07")
        );
        assert_eq!(
            WheelPickerOption::from(String::from("07")),
            WheelPickerOption::new("07", "07")
        );
        let labelled = WheelPickerOption::new("07", "7 minutes");
        assert_eq!(labelled.value, "07");
        assert_eq!(labelled.label, "7 minutes");
    }
}
