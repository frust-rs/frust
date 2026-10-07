//! Ports beUI's `cylinder-carousel` component.
//!
//! **Source:** `components/motion/cylinder-carousel.tsx`, beUI rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01. Registry
//! entry: slug `cylinder-carousel`, *"A carousel whose items line the inside of
//! a cylinder, receding into the center and growing toward the edges. Drag,
//! scroll or arrow-key to roll it, with a springy glide and snap. Reduced-motion
//! drops the glide."*
//!
//! | Upstream | Here |
//! |---|---|
//! | `itemSize = 200` / `visibleItems = 5` | [`DEFAULT_ITEM_SIZE`] / [`DEFAULT_VISIBLE_ITEMS`] |
//! | `variant = "concave" \| "convex"` | [`CylinderCarouselVariant`] |
//! | `minScale = 0.55` / `dragSpeed = 1.5` | [`DEFAULT_MIN_SCALE`] / [`DEFAULT_DRAG_SPEED`] |
//! | `THETA_EDGE` / `THETA_CLAMP` | [`THETA_EDGE`] / [`THETA_CLAMP`] |
//! | the per-ball `x`/`scale`/`y`/`visibility` transforms | [`CylinderCarouselPlacement`] |
//! | `GLIDE_SPRING`, `FLICK_MOMENTUM`, `MAX_FLICK_ITEMS` | the same three constants |
//! | `autoRotate` / `autoRotateSpeed = 0.4` | [`CylinderCarouselView::auto_rotate`] |
//!
//! # The 3D degradation, and why it is a small one
//!
//! Upstream is explicit that its cylinder is **already a hand-rolled
//! projection**, not a CSS 3D transform: its own comment describes "a single
//! perspective projection … horizontal position, size and height all share one
//! `1/(cosθ + k)` depth term", and every ball is then placed with a flat
//! `x`/`y`/`scale` on a `position: absolute` div. There is no `rotateY`, no
//! `perspective()`, no `preserve-3d` anywhere in the file.
//!
//! So the projection arithmetic ports **exactly** — [`CylinderCarouselGeometry`]
//! and [`CylinderCarouselPlacement`] carry upstream's own formulas and
//! constants, unrounded — and the only thing that becomes an approximation is
//! the *composite*: upstream's `scale` is a CSS transform on a rasterised
//! element, and here it is a scene [`Affine`] applied about each item's centre.
//! Those two differ only in resampling quality, so unlike
//! [`tilt_card`](super::tilt_card)'s genuinely lost perspective divide, this
//! degradation costs no geometry at all. The balls do not overlap, fade or
//! reorder in either version — upstream's own note, "the edge is the exit".
//!
//! # Degradation: the glide is not velocity-seeded
//!
//! Upstream hands the release velocity straight into its spring
//! (`animate(scroll, to, { type: "spring", ...GLIDE_SPRING, velocity })`), so
//! "the roll leaves the finger at finger speed". The catalog's spring
//! ([`Ramp::spring`]) solves a **unit displacement released from rest** — there
//! is no seam for an initial velocity, and `crate::press::SpringScalar` is
//! documented value-continuous rather than velocity-continuous.
//!
//! The flick's *reach* is kept exactly (`projected = scroll + velocity ·`
//! [`FLICK_MOMENTUM`], clamped to [`MAX_FLICK_ITEMS`], then snapped), so a hard
//! flick still travels as far as it does upstream and lands on the same item.
//! What is lost is the seamless hand-off at the moment of release: the roll
//! restarts from zero speed toward that further target instead of continuing at
//! finger speed. Recorded as a substrate finding — a velocity-seeded spring
//! belongs in [`crate::motion`], not in one component.
//!
//! # The true-3D wall, behind `gpu-effects`
//!
//! With the non-default `gpu-effects` feature on,
//! [`CylinderCarouselView::gpu_cylinder`] seats a **genuinely projected**
//! plate under every ball: a quad this crate renders through
//! [`crate::gpu_fx::cylinder`] into an offscreen target the engine composites,
//! turned into the wall at the same wall angle [`CylinderCarouselGeometry`]
//! resolves and seated in depth by the very scale that geometry computed
//! ([`Cylinder3d::scaled_face`](crate::gpu_fx::cylinder::Cylinder3d::scaled_face)).
//! So the plate lands on precisely the box the flat item occupies, and what
//! the 3D path adds is what an `Affine` cannot express: the keystone of a face
//! genuinely turned away from the viewer, and an occlusion order taken from
//! distance rather than from paint order.
//!
//! It is opt-in for one blunt reason, and the reason is the boundary the whole
//! substrate is under:
//!
//! > **A 3D face is a colour, a ramp or a caller-owned texture — never a
//! > widget subtree.** Each item's own content keeps compositing flat under
//! > the `Affine` above, exactly as it does without the feature. What the 3D
//! > path renders is the **wall the items line**, not the items.
//!
//! Two consequences a caller should expect:
//!
//! - **The stage grows a wall it did not have.** Upstream paints nothing
//!   behind a ball, so the plate is a deliberate departure rather than a port
//!   — it is what makes a turned wall visible at all, since a transparent one
//!   would project nothing.
//! - **The plates have square corners** and the stage's own `inset(0)` clip is
//!   what trims them at the edge, exactly as it trims the flat balls.
//!
//! The runtime fallback is the ordinary contract: acquisition answers `None`
//! before a shell's first surface, on every platform whose shell installs no
//! device, and under the substrate's kill switch — and the carousel then
//! paints precisely the 2D wall it always did, `gpu_cylinder` set or not.
//!
//! # Items are not pointer targets
//!
//! The stage owns the gesture outright, as it does upstream (`touch-none`,
//! `e.preventDefault()` on every `pointerdown`, `clip-path: inset(0)`), and each
//! item's painted position is a scene transform rather than its laid-out box —
//! so routing a pointer by that box would hit the wrong ball. Items are
//! therefore decorative here, the same call [`marquee`](super::marquee) makes
//! for the same reason. Broadcasts still reach every child so their pods stay
//! live.
//!
//! # Index reporting
//!
//! Upstream subscribes to its `scroll` motion value and reports every integer
//! crossing, including the ones a coasting glide makes on its own. A callback in
//! this tier can only be invoked from an event pass, so — exactly as
//! [`wheel_picker`](super::wheel_picker) does — **the landing index is reported
//! at release**, not once per coasting frame. A drag, a wheel notch and an arrow
//! key each report as they happen; a fling reports where it is going.

use std::rc::Rc;

use frust::authoring::{
    Action, Affine, AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, CursorIcon,
    ErasedArgCallback, EventCtx, EventResult, InputEvent, Key, KeyEvent, LayoutCtx, NamedKey,
    PaintCtx, PaintScene, PointerPhase, Role, ScrollDelta, SemanticsCtx, TickClass, View, ViewSeq,
    Widget, build_child, erase_callback_arg, rebuild_children, teardown_child, visit_children,
};
use frust::{FrameTime, SpringDescription, Theme};
use kurbo::{Point, Size};
#[cfg(feature = "gpu-effects")]
use kurbo::{Rect, Vec2};
#[cfg(feature = "gpu-effects")]
use peniko::Color;

#[cfg(feature = "gpu-effects")]
use crate::gpu_fx::card3d::{self, Card3d};
#[cfg(feature = "gpu-effects")]
use crate::gpu_fx::cylinder::{self, Cylinder3d, CylinderAxis};
#[cfg(feature = "gpu-effects")]
use crate::gpu_fx::quad3d::{Quad3d, QuadFace};
#[cfg(feature = "gpu-effects")]
use crate::gpu_fx::schedule::request_frame;
use crate::motion::Ramp;
use crate::press::{SpringScalar, inside, presses};
#[cfg(feature = "gpu-effects")]
use crate::style;

/// Upstream's `itemSize = 200` — the item box's cap, in logical px.
pub const DEFAULT_ITEM_SIZE: f64 = 200.0;

/// Upstream's `visibleItems = 5` — how many slots span the stage.
pub const DEFAULT_VISIBLE_ITEMS: usize = 5;

/// Upstream's `minScale = 0.55` — the smallest ball's scale.
pub const DEFAULT_MIN_SCALE: f64 = 0.55;

/// Upstream's `dragSpeed = 1.5` — items rolled per item-width dragged.
pub const DEFAULT_DRAG_SPEED: f64 = 1.5;

/// Upstream's `autoRotateSpeed = 0.4` — items per second when auto-rolling.
pub const DEFAULT_AUTO_ROTATE_SPEED: f64 = 0.4;

/// Upstream's `arc` default: `size * 0.35`.
pub const ARC_FRACTION: f64 = 0.35;

/// The resting row's diameters may take at most this fraction of the stage —
/// upstream's `(stageWidth * 0.65) / scaleSum`.
pub const FIT_FRACTION: f64 = 0.65;

/// The wall angle at which a ball's centre sits on the stage edge — upstream's
/// `THETA_EDGE = 72°`.
pub const THETA_EDGE: f64 = 72.0 * std::f64::consts::PI / 180.0;

/// The wall angle past which a ball is parked off-stage — upstream's
/// `THETA_CLAMP = 95°`.
pub const THETA_CLAMP: f64 = 95.0 * std::f64::consts::PI / 180.0;

/// The floor upstream clamps its camera term to (`Math.max(0.2, …)`), which is
/// what keeps the projection monotonic out to [`THETA_CLAMP`].
pub const MIN_CAMERA_TERM: f64 = 0.2;

/// The carousel's own glide spring — upstream's local `GLIDE_SPRING =
/// { stiffness: 40, damping: 20, mass: 3 }`, deliberately far softer than any
/// `lib/ease.ts` token so a flick keeps rolling and eases back.
pub const GLIDE_SPRING: SpringDescription = SpringDescription {
    mass: 3.0,
    stiffness: 40.0,
    damping: 20.0,
};

/// How far a flick keeps rolling — upstream's `FLICK_MOMENTUM = 0.45`, in
/// item-seconds.
pub const FLICK_MOMENTUM: f64 = 0.45;

/// The cap on a flick's projected travel — upstream's `MAX_FLICK_ITEMS = 6`.
pub const MAX_FLICK_ITEMS: f64 = 6.0;

/// The stage width assumed before anything has measured one — upstream's
/// `width || 800`.
pub const FALLBACK_STAGE_WIDTH: f64 = 800.0;

/// Logical px per wheel *line*, for a line-quantised scroll — the same
/// substitution [`wheel_picker`](super::wheel_picker) makes for `deltaMode === 1`.
pub const WHEEL_LINE_PX: f64 = 16.0;

/// How long the wheel must be idle before the roll settles, in milliseconds —
/// upstream's `setTimeout(…, 140)`.
pub const WHEEL_SETTLE_MS: f64 = 140.0;

/// The trailing window a release velocity is averaged over, in milliseconds.
///
/// Upstream differences its final two pointer samples. That is a single frame,
/// and a single noisy frame turns an even flick into a caught one, so this
/// averages over a short window instead — the same correction
/// [`wheel_picker`](super::wheel_picker) makes, for the same reason.
pub const VELOCITY_WINDOW_MS: f64 = 90.0;

/// How many pointer samples a drag keeps for the velocity estimate.
const VELOCITY_SAMPLES: usize = 8;

/// Alpha of one ball's plate on the true-3D wall
/// ([`CylinderCarouselView::gpu_cylinder`]).
///
/// Low enough to read as a surface the items sit on rather than as a card
/// around them — upstream has no plate at all, and this one exists to make the
/// wall's turn visible, not to reframe the content.
#[cfg(feature = "gpu-effects")]
pub const WALL_ALPHA: f32 = 0.06;

/// The label this component's GPU pass is diagnosed under.
#[cfg(feature = "gpu-effects")]
const FX_LABEL: &str = "cylinder-carousel";

/// Unthemed fallback ink for the wall's plates — the light table's
/// `--foreground`.
#[cfg(feature = "gpu-effects")]
const FALLBACK_WALL_INK: Color = crate::BEUI_LIGHT.foreground;

/// Which side of the cylinder the items line — upstream's `variant` prop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CylinderCarouselVariant {
    /// `"concave"` (upstream's default): the inside of the cylinder — the centre
    /// ball is smallest and dipped, growing toward the edges.
    #[default]
    Concave,
    /// `"convex"`: the outside — the centre ball is biggest and raised,
    /// shrinking toward the edges.
    Convex,
}

impl CylinderCarouselVariant {
    /// Both, in upstream's own order.
    pub const ALL: [CylinderCarouselVariant; 2] = [
        CylinderCarouselVariant::Concave,
        CylinderCarouselVariant::Convex,
    ];

    /// Whether this is the outside of the cylinder — upstream's `convex`.
    pub fn is_convex(self) -> bool {
        matches!(self, CylinderCarouselVariant::Convex)
    }
}

/// Where one item sits on the wall, in the stage's own space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CylinderCarouselPlacement {
    /// Horizontal offset from the stage centre, in logical px.
    pub x: f64,
    /// Vertical offset from the stage centre, in logical px.
    pub y: f64,
    /// The item's scale, `min_scale..=1`.
    pub scale: f64,
    /// Whether the item is far enough off-stage to stop painting — upstream's
    /// `visibility: hidden`.
    pub hidden: bool,
}

/// The stage's resolved projection — every constant upstream derives once per
/// measured width, in one place so the arithmetic is testable without a widget.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CylinderCarouselGeometry {
    /// The item box's side, in logical px — upstream's `size`.
    pub size: f64,
    /// Half the stage width.
    pub half_width: f64,
    /// The uniform slot width convex spacing uses — upstream's `gap`.
    pub gap: f64,
    /// The curve's depth between the centre ball and the edge balls, in px.
    pub arc: f64,
    /// The offset at which a ball's centre sits on the stage edge — upstream's
    /// `edgeOffset`.
    pub edge_offset: f64,
    /// Wall angle per item step, in radians — upstream's `alpha`.
    pub alpha: f64,
    /// The camera distance term — upstream's `k`.
    pub k: f64,
    /// The projection strength — upstream's `projection`.
    pub projection: f64,
    /// The smallest ball's scale.
    pub min_scale: f64,
    /// Which side of the cylinder is being lined.
    pub convex: bool,
}

impl CylinderCarouselGeometry {
    /// Resolve the projection for a measured stage, exactly as upstream does on
    /// every `ResizeObserver` callback.
    pub fn resolve(
        stage_width: f64,
        visible_items: usize,
        item_size: f64,
        min_scale: f64,
        variant: CylinderCarouselVariant,
        arc: Option<f64>,
    ) -> Self {
        let convex = variant.is_convex();
        let visible = visible_items.max(1);
        let stage_width = if stage_width > 0.0 {
            stage_width
        } else {
            FALLBACK_STAGE_WIDTH
        };
        let half_width = stage_width / 2.0;
        let edge_offset = (visible + 1) as f64 / 2.0;
        // Upstream's fit pass: the resting row's diameters may take at most
        // `FIT_FRACTION` of the stage, and `itemSize` only caps the result.
        let mut scale_sum = 0.0;
        for slot in 0..visible {
            let t = (slot as f64 - (visible as f64 - 1.0) / 2.0).abs() / edge_offset;
            scale_sum += if convex {
                1.0 - (1.0 - min_scale) * t
            } else {
                min_scale + (1.0 - min_scale) * t
            };
        }
        let size = if scale_sum > 0.0 {
            item_size.min(stage_width * FIT_FRACTION / scale_sum)
        } else {
            item_size
        };
        let gap = stage_width / (visible + 1) as f64;
        let arc = arc.unwrap_or(size * ARC_FRACTION);
        let alpha = THETA_EDGE / edge_offset;
        let k = MIN_CAMERA_TERM.max((min_scale - THETA_EDGE.cos()) / (1.0 - min_scale));
        let projection = (half_width * (THETA_EDGE.cos() + k)) / THETA_EDGE.sin();
        Self {
            size,
            half_width,
            gap,
            arc,
            edge_offset,
            alpha,
            k,
            projection,
            min_scale,
            convex,
        }
    }

    /// The nearest wrapped offset of item `index` at scroll position `scroll`,
    /// so a list loops around continuously — upstream's
    /// `o -= Math.round(o / count) * count`.
    pub fn wrapped_offset(index: usize, scroll: f64, count: usize) -> f64 {
        if count == 0 {
            return 0.0;
        }
        let mut offset = index as f64 - scroll;
        offset -= (offset / count as f64).round() * count as f64;
        offset
    }

    /// Place an item sitting `offset` slots from the stage centre.
    pub fn place(&self, offset: f64) -> CylinderCarouselPlacement {
        // Concave spacing follows the interior perspective (slow, tight centre);
        // convex pairs its big centre balls with uniform spacing, because the
        // interior projection would collapse them into each other.
        let x = if self.convex {
            offset * self.gap
        } else {
            let theta = (offset * self.alpha).clamp(-THETA_CLAMP, THETA_CLAMP);
            (self.projection * theta.sin()) / (theta.cos() + self.k)
        };
        // Linear in wall angle, not in depth: the depth curve is near-flat about
        // the centre, which made the middle three read as equal.
        let t = (offset.abs() / self.edge_offset).min(THETA_CLAMP / THETA_EDGE);
        let scale = if self.convex {
            1.0 - (1.0 - self.min_scale) * t
        } else {
            self.min_scale + (1.0 - self.min_scale) * t
        };
        // A parabola centred on the stage — a valley for concave, an arch for
        // convex — and deliberately unclamped, so a ball keeps following the
        // same curve as it crosses the edge and entries never pop.
        let normalised = if self.half_width > 0.0 {
            x / self.half_width
        } else {
            0.0
        };
        let valley = self.arc * (0.5 - normalised * normalised);
        let y = if self.convex { -valley } else { valley };
        CylinderCarouselPlacement {
            x,
            y,
            scale,
            hidden: x.abs() > self.half_width + self.size,
        }
    }
}

/// A view-held index-change callback (erased on build).
type OnIndexChange<State> = Rc<dyn Fn(&mut State, usize)>;

/// A declarative beUI cylinder carousel. See the [module docs](self).
///
/// # Example
///
/// ```
/// use frust::text;
/// use frust_beui::components::cylinder_carousel::{
///     CylinderCarouselVariant, cylinder_carousel,
/// };
///
/// let rail = cylinder_carousel::<(), _>((text("a"), text("b")))
///     .variant(CylinderCarouselVariant::Convex)
///     .snap(false);
/// ```
pub struct CylinderCarouselView<State: 'static> {
    items: Vec<AnyView<State>>,
    item_size: f64,
    visible_items: usize,
    variant: CylinderCarouselVariant,
    min_scale: f64,
    drag_speed: f64,
    arc: Option<f64>,
    snap: bool,
    auto_rotate: bool,
    auto_rotate_speed: f64,
    default_index: usize,
    height: Option<f64>,
    #[cfg(feature = "gpu-effects")]
    gpu_cylinder: bool,
    on_index_change: Option<OnIndexChange<State>>,
}

/// Line `items` on the inside of a cylinder, with upstream's own defaults.
pub fn cylinder_carousel<State: 'static, M>(
    items: impl ViewSeq<State, M>,
) -> CylinderCarouselView<State> {
    let mut erased = Vec::new();
    items.extend_views(&mut erased);
    CylinderCarouselView {
        items: erased,
        item_size: DEFAULT_ITEM_SIZE,
        visible_items: DEFAULT_VISIBLE_ITEMS,
        variant: CylinderCarouselVariant::default(),
        min_scale: DEFAULT_MIN_SCALE,
        drag_speed: DEFAULT_DRAG_SPEED,
        arc: None,
        snap: true,
        auto_rotate: false,
        auto_rotate_speed: DEFAULT_AUTO_ROTATE_SPEED,
        default_index: 0,
        height: None,
        #[cfg(feature = "gpu-effects")]
        gpu_cylinder: false,
        on_index_change: None,
    }
}

impl<State: 'static> CylinderCarouselView<State> {
    /// The item box's cap in logical px (`itemSize`, default
    /// [`DEFAULT_ITEM_SIZE`]).
    pub fn item_size(mut self, item_size: f64) -> Self {
        self.item_size = item_size.max(0.0);
        self
    }

    /// How many slots span the stage (`visibleItems`, default
    /// [`DEFAULT_VISIBLE_ITEMS`]).
    pub fn visible_items(mut self, visible_items: usize) -> Self {
        self.visible_items = visible_items.max(1);
        self
    }

    /// Which side of the cylinder to line (`variant`, default
    /// [`CylinderCarouselVariant::Concave`]).
    pub fn variant(mut self, variant: CylinderCarouselVariant) -> Self {
        self.variant = variant;
        self
    }

    /// The smallest ball's scale (`minScale`, default [`DEFAULT_MIN_SCALE`]),
    /// clamped below `1.0` so the projection stays monotonic.
    pub fn min_scale(mut self, min_scale: f64) -> Self {
        self.min_scale = min_scale.clamp(0.0, 0.99);
        self
    }

    /// Items rolled per item-width dragged (`dragSpeed`, default
    /// [`DEFAULT_DRAG_SPEED`]).
    pub fn drag_speed(mut self, drag_speed: f64) -> Self {
        self.drag_speed = drag_speed;
        self
    }

    /// The curve's depth in px (`arc`, default `size ·` [`ARC_FRACTION`]). Zero
    /// is a flat line.
    pub fn arc(mut self, arc: f64) -> Self {
        self.arc = Some(arc.max(0.0));
        self
    }

    /// Snap to the nearest item when the roll settles (`snap`, default `true`).
    pub fn snap(mut self, snap: bool) -> Self {
        self.snap = snap;
        self
    }

    /// Roll on its own until interacted with (`autoRotate`, default `false`).
    /// Suppressed under the theme's `reduce_motion`.
    pub fn auto_rotate(mut self, auto_rotate: bool) -> Self {
        self.auto_rotate = auto_rotate;
        self
    }

    /// Auto-roll speed in items per second (`autoRotateSpeed`, default
    /// [`DEFAULT_AUTO_ROTATE_SPEED`]).
    pub fn auto_rotate_speed(mut self, speed: f64) -> Self {
        self.auto_rotate_speed = speed;
        self
    }

    /// Which item the carousel opens on (`defaultIndex`, default `0`).
    pub fn default_index(mut self, index: usize) -> Self {
        self.default_index = index;
        self
    }

    /// The stage's height in px (`height`, default the resolved item size).
    pub fn height(mut self, height: f64) -> Self {
        self.height = Some(height.max(0.0));
        self
    }

    /// Line the wall with **genuinely projected** plates, behind the
    /// non-default `gpu-effects` feature — see the [module docs](self)' *The
    /// true-3D wall*.
    ///
    /// Off by default, and an enhancement either way: each item's content keeps
    /// compositing flat above the wall, and a build with no reachable GPU
    /// renders exactly the 2D carousel this control always did.
    #[cfg(feature = "gpu-effects")]
    pub fn gpu_cylinder(mut self, gpu_cylinder: bool) -> Self {
        self.gpu_cylinder = gpu_cylinder;
        self
    }

    /// Report the centred item as it changes (`onIndexChange`) — at release for
    /// a fling, see the [module docs](self).
    pub fn on_index_change<F: Fn(&mut State, usize) + 'static>(mut self, callback: F) -> Self {
        self.on_index_change = Some(Rc::new(callback));
        self
    }
}

/// A live pointer drag on the stage.
#[derive(Clone, Debug)]
struct CarouselDrag {
    /// The pointer x the gesture started at.
    anchor_x: f64,
    /// The scroll position it started from.
    anchor_scroll: f64,
    /// `(pointer x, frame)` samples, newest last.
    samples: Vec<(f64, FrameTime)>,
}

/// The retained widget for a [`CylinderCarouselView`].
pub struct CylinderCarouselWidget {
    items: Vec<ChildPod>,
    item_size: f64,
    visible_items: usize,
    variant: CylinderCarouselVariant,
    min_scale: f64,
    drag_speed: f64,
    arc: Option<f64>,
    snap: bool,
    auto_rotate: bool,
    auto_rotate_speed: f64,
    height: Option<f64>,
    /// The roll position in item units, continuous — upstream's `scroll`.
    scroll: SpringScalar,
    /// The projection resolved at the last layout.
    geometry: CylinderCarouselGeometry,
    /// The stage width the geometry was resolved for.
    stage_width: f64,
    /// A live drag, if one owns the stage.
    drag: Option<CarouselDrag>,
    /// When the last wheel notch landed, so the settle fires once it goes quiet.
    wheel_idle_since: Option<FrameTime>,
    /// The last painted frame — the clock an event pass borrows, since an
    /// `EventCtx` carries none.
    frame: FrameTime,
    /// The previous auto-rotate frame, so the roll advances by a real delta.
    auto_frame: Option<FrameTime>,
    /// Whether the pointer is over the stage, which pauses the auto-roll.
    hovered: bool,
    /// The index last reported, so a gesture crossing the same item twice
    /// reports it once.
    emitted: usize,
    /// Whether the caller asked for the true-3D wall.
    #[cfg(feature = "gpu-effects")]
    gpu_cylinder: bool,
    /// The 3D path's handle: acquired lazily on a paint, cleared when the wall
    /// stands down, released from `View::teardown`.
    #[cfg(feature = "gpu-effects")]
    fx: Card3d,
    on_index_change: Option<ErasedArgCallback<usize>>,
}

impl CylinderCarouselWidget {
    /// How many items the carousel holds.
    pub fn count(&self) -> usize {
        self.items.len()
    }

    /// The roll position in item units, continuous.
    pub fn scroll(&self) -> f64 {
        self.scroll.value()
    }

    /// The centred item — upstream's `((Math.round(v) % count) + count) % count`.
    pub fn index(&self) -> usize {
        let count = self.count();
        if count == 0 {
            return 0;
        }
        (self.scroll().round() as i64).rem_euclid(count as i64) as usize
    }

    /// The projection resolved at the last layout.
    pub fn geometry(&self) -> CylinderCarouselGeometry {
        self.geometry
    }

    /// Where item `index` sits right now.
    pub fn placement(&self, index: usize) -> CylinderCarouselPlacement {
        self.geometry
            .place(CylinderCarouselGeometry::wrapped_offset(
                index,
                self.scroll(),
                self.count(),
            ))
    }

    /// Whether a glide is in flight.
    pub fn is_gliding(&self) -> bool {
        self.scroll.is_animating()
    }

    /// Where a flick of `velocity` (items per second) coasts to — upstream's
    /// `settle`.
    pub fn flick_target(&self, velocity: f64) -> f64 {
        let projected =
            self.scroll() + (velocity * FLICK_MOMENTUM).clamp(-MAX_FLICK_ITEMS, MAX_FLICK_ITEMS);
        if self.snap {
            projected.round()
        } else {
            projected
        }
    }

    /// Glide toward `to`, or land on it outright under `reduce_motion`.
    fn glide_to(&mut self, to: f64, reduce: bool) {
        if reduce {
            self.scroll.jump_to(to);
        } else {
            self.scroll.set_target(to);
        }
    }

    /// Stop any glide where it stands — upstream's `stopGlide`.
    fn stop_glide(&mut self) {
        let at = self.scroll();
        self.scroll.jump_to(at);
    }

    /// The release velocity in items per second, averaged over
    /// [`VELOCITY_WINDOW_MS`] of samples.
    fn release_velocity(&self, drag: &CarouselDrag) -> f64 {
        let samples = &drag.samples;
        if samples.len() < 2 || self.geometry.gap <= 0.0 {
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
        let dt = latest.1.saturating_sub(reference.1).as_secs_f64();
        if dt <= 0.0 {
            return 0.0;
        }
        let px_per_second = (latest.0 - reference.0) / dt;
        // Upstream's `(-vpx * dragSpeed * 1000) / gap`: dragging right rolls the
        // wall left, so the sign inverts.
        -px_per_second * self.drag_speed / self.geometry.gap
    }

    /// Report the centred item, unless it is the one already reported.
    fn emit(&mut self, ctx: &mut EventCtx) {
        let index = self.index();
        if index == self.emitted || self.count() == 0 {
            return;
        }
        self.emitted = index;
        if let Some(callback) = &mut self.on_index_change {
            callback(ctx, index);
        }
    }
}

impl<State: 'static> View<State> for CylinderCarouselView<State> {
    type Element = CylinderCarouselWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> CylinderCarouselWidget {
        let geometry = CylinderCarouselGeometry::resolve(
            FALLBACK_STAGE_WIDTH,
            self.visible_items,
            self.item_size,
            self.min_scale,
            self.variant,
            self.arc,
        );
        CylinderCarouselWidget {
            items: self
                .items
                .iter()
                .map(|item| build_child(item, ctx))
                .collect(),
            item_size: self.item_size,
            visible_items: self.visible_items,
            variant: self.variant,
            min_scale: self.min_scale,
            drag_speed: self.drag_speed,
            arc: self.arc,
            snap: self.snap,
            auto_rotate: self.auto_rotate,
            auto_rotate_speed: self.auto_rotate_speed,
            height: self.height,
            scroll: SpringScalar::new(self.default_index as f64, Ramp::spring(GLIDE_SPRING)),
            geometry,
            stage_width: 0.0,
            drag: None,
            wheel_idle_since: None,
            frame: FrameTime::ZERO,
            auto_frame: None,
            hovered: false,
            emitted: self.default_index,
            #[cfg(feature = "gpu-effects")]
            gpu_cylinder: self.gpu_cylinder,
            #[cfg(feature = "gpu-effects")]
            fx: Card3d::new(FX_LABEL),
            on_index_change: self.on_index_change.as_ref().map(erase_callback_arg),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut CylinderCarouselWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Closures aren't comparable, so the adapter is reinstalled every pass.
        element.on_index_change = self.on_index_change.as_ref().map(erase_callback_arg);
        let mut flags = rebuild_children(
            &prev.items,
            &self.items,
            &mut element.items,
            ctx,
            |view| view,
            |_| None,
        );
        if prev.items.len() != self.items.len() {
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        let geometry_changed = element.item_size != self.item_size
            || element.visible_items != self.visible_items
            || element.variant != self.variant
            || element.min_scale != self.min_scale
            || element.arc != self.arc;
        if geometry_changed {
            element.item_size = self.item_size;
            element.visible_items = self.visible_items;
            element.variant = self.variant;
            element.min_scale = self.min_scale;
            element.arc = self.arc;
            element.geometry = CylinderCarouselGeometry::resolve(
                element.stage_width,
                self.visible_items,
                self.item_size,
                self.min_scale,
                self.variant,
                self.arc,
            );
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.drag_speed != self.drag_speed {
            element.drag_speed = self.drag_speed;
        }
        if element.snap != self.snap {
            element.snap = self.snap;
        }
        if element.auto_rotate != self.auto_rotate {
            element.auto_rotate = self.auto_rotate;
            element.auto_frame = None;
            flags |= ChangeFlags::PAINT;
        }
        if element.auto_rotate_speed != self.auto_rotate_speed {
            element.auto_rotate_speed = self.auto_rotate_speed;
        }
        if element.height != self.height {
            element.height = self.height;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        #[cfg(feature = "gpu-effects")]
        if element.gpu_cylinder != self.gpu_cylinder {
            element.gpu_cylinder = self.gpu_cylinder;
            if !self.gpu_cylinder {
                // Withdrawn mid-life: the claim is kept (it can be switched
                // back on) but the pass stops compositing a wall nothing is
                // asking for.
                element.fx.clear();
            }
            flags |= ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut CylinderCarouselWidget, ctx: &mut BuildCtx<'_>) {
        // The claim goes first: a handle dropped without this leaves its pass
        // called every frame forever.
        #[cfg(feature = "gpu-effects")]
        element.fx.release();
        for (view, pod) in self.items.iter().zip(element.items.iter_mut()) {
            teardown_child(view, pod, ctx);
        }
    }
}

#[cfg(feature = "gpu-effects")]
impl CylinderCarouselWidget {
    /// The wall this carousel's balls line, as the substrate describes it: a
    /// cylinder turning about a vertical axis, its faces on the outside for
    /// `convex` and the inside for `concave`.
    ///
    /// The radius is the one upstream's own edge rule implies — the radius at
    /// which a ball's centre at [`THETA_EDGE`] sits exactly on the stage's
    /// edge. Only the axis and the side are actually read off the drum here:
    /// the placement is upstream's hand-rolled projection and stays so (see the
    /// [module docs](self)), so the seat's own travel and depth are not the
    /// ones this component seats by.
    fn wall(&self) -> Cylinder3d {
        let radius = if THETA_EDGE.sin() > 0.0 {
            self.geometry.half_width / THETA_EDGE.sin()
        } else {
            self.geometry.half_width
        };
        Cylinder3d::new(radius, CylinderAxis::Vertical, self.geometry.convex)
    }

    /// Composite the projected wall, answering whether it took over.
    ///
    /// `false` — and therefore no wall at all, exactly as this stage has always
    /// painted — whenever the caller did not opt in, the GPU is unreachable,
    /// the stage has no usable box, or every ball is off-stage. Every one of
    /// those is an ordinary frame, not an error.
    fn paint_wall(
        &mut self,
        ctx: &mut PaintCtx,
        scene: &mut dyn PaintScene,
        origin: Point,
        size: Size,
    ) -> bool {
        if !self.gpu_cylinder {
            self.fx.clear();
            return false;
        }
        if !self.fx.acquire() {
            return false;
        }
        let ink = Theme::from_paint_ctx(ctx).map_or(FALLBACK_WALL_INK, |t| t.scheme().on_surface);
        let Some((dest, extent)) = card3d::target_for(origin, size) else {
            self.fx.clear();
            return false;
        };
        let faces = self.wall_faces(extent, ink);
        if faces.is_empty() {
            self.fx.clear();
            return false;
        }

        // See `Card3d::submit`'s doc for bind timing and why a `true` answer
        // asks for a frame.
        let first = self.fx.submit(extent, cylinder::drum_scene(faces));
        request_frame(ctx, card3d::cadence(first));
        let Some(id) = self.fx.scene_texture_id() else {
            return false;
        };
        scene.draw_scene_texture(id, dest);
        true
    }

    /// One plate per on-stage ball, placed where the flat ball is placed and
    /// seated in depth by the scale that placement already computed — so the
    /// plate projects onto precisely the box its item paints into, and the
    /// item's content lands on its own surface rather than beside it.
    ///
    /// The plates are submitted in item order, not depth order: the depth
    /// attachment is what resolves two that overlap, which is the whole point
    /// of the mode.
    fn wall_faces(&self, extent: (u32, u32), ink: Color) -> Vec<Quad3d> {
        let wall = self.wall();
        let count = self.count();
        let item = Size::new(self.geometry.size, self.geometry.size);
        let centre = Point::new(f64::from(extent.0) / 2.0, f64::from(extent.1) / 2.0);
        let face = QuadFace::Solid(style::with_alpha(ink, WALL_ALPHA));
        (0..count)
            .filter_map(|index| {
                let offset =
                    CylinderCarouselGeometry::wrapped_offset(index, self.scroll.value(), count);
                let placement = self.geometry.place(offset);
                // The stage's own exit rule decides what is on stage; the
                // substrate's horizon is the narrower second guard, since a
                // ball may be clamped past a quarter turn while its centre is
                // still inside the stage.
                let angle = (offset * self.geometry.alpha).clamp(-THETA_CLAMP, THETA_CLAMP);
                if placement.hidden || !wall.visible(angle) {
                    return None;
                }
                let seat = wall.seat(angle);
                let dest =
                    Rect::from_center_size(centre + Vec2::new(placement.x, placement.y), item);
                Some(wall.scaled_face(seat, dest, face.clone(), placement.scale))
            })
            .collect()
    }
}

#[cfg(not(feature = "gpu-effects"))]
impl CylinderCarouselWidget {
    /// Without the `gpu-effects` feature there is no wall to project: the
    /// stage is its 2D path and nothing else.
    fn paint_wall(
        &mut self,
        _ctx: &mut PaintCtx,
        _scene: &mut dyn PaintScene,
        _origin: Point,
        _size: Size,
    ) -> bool {
        false
    }
}

impl Widget for CylinderCarouselWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let stage_width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            FALLBACK_STAGE_WIDTH
        };
        self.stage_width = stage_width;
        self.geometry = CylinderCarouselGeometry::resolve(
            stage_width,
            self.visible_items,
            self.item_size,
            self.min_scale,
            self.variant,
            self.arc,
        );
        // Every ball is the same square box; the wall's projection is applied at
        // paint, so a roll never re-lays the stage out.
        let item = Size::new(self.geometry.size, self.geometry.size);
        let item_bc = BoxConstraints::tight(item);
        for pod in &mut self.items {
            pod.layout_child(ctx, &item_bc);
            pod.set_origin(Point::ORIGIN);
        }
        let height = self.height.unwrap_or(self.geometry.size);
        bc.constrain(Size::new(stage_width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let origin = ctx.origin();
        let size = ctx.size();
        let now = ctx.frame_time();
        self.frame = now;

        let mut owes_frame = false;
        if reduce {
            // "Reduced-motion drops the glide" — the registry's own words, and
            // upstream's `if (reduce) scroll.set(to)`: the roll *lands on its
            // target* rather than freezing where the finger left it. An event
            // pass sees no theme, so this is where that collapse happens.
            let target = self.scroll.target();
            self.scroll.jump_to(target);
            self.wheel_idle_since = None;
        } else {
            self.scroll.advance(now);
            owes_frame |= self.scroll.is_animating();
            // The wheel drives `scroll` continuously and snaps once it goes
            // quiet: a fresh glide per notch would stack animations that keep
            // interrupting each other, which reads as lag.
            if let Some(since) = self.wheel_idle_since {
                if now.saturating_sub(since).as_secs_f64() * 1000.0 >= WHEEL_SETTLE_MS {
                    self.wheel_idle_since = None;
                    let to = self.flick_target(0.0);
                    self.glide_to(to, false);
                    owes_frame = true;
                } else {
                    owes_frame = true;
                }
            }
        }

        // The auto-roll: a perpetual decorative loop, paused by a drag, a hover
        // or a glide, exactly as upstream's `requestAnimationFrame` tick is.
        let auto_rolling = self.auto_rotate
            && !reduce
            && self.count() > 0
            && self.drag.is_none()
            && !self.hovered
            && !self.scroll.is_animating();
        if auto_rolling {
            let previous = self.auto_frame.replace(now);
            if let Some(previous) = previous {
                let delta = now.saturating_sub(previous).as_secs_f64();
                let at = self.scroll() + self.auto_rotate_speed * delta;
                self.scroll.jump_to(at);
            }
        } else {
            self.auto_frame = None;
        }

        // `clip-path: inset(0)` — a ball leaving the stage disappears at the
        // edge rather than painting over a sibling.
        scene.push_clip(origin, size);

        // The wall the balls line, when the caller asked for the projected one:
        // inside the clip, so the overscanned target's margin is trimmed by the
        // same rule that trims a ball leaving the stage, and before the items,
        // which keep compositing flat on top of it.
        self.paint_wall(ctx, scene, origin, size);

        let centre = Point::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
        let item = self.geometry.size;
        let count = self.count();
        for index in 0..count {
            let placement = self
                .geometry
                .place(CylinderCarouselGeometry::wrapped_offset(
                    index,
                    self.scroll.value(),
                    count,
                ));
            if placement.hidden {
                continue;
            }
            // The pod paints its box at the stage origin; this maps that box's
            // centre onto the ball's projected centre and scales about it.
            let painted_centre = Point::new(origin.x + item / 2.0, origin.y + item / 2.0);
            let target = Point::new(centre.x + placement.x, centre.y + placement.y);
            scene.push_transform(
                Affine::translate(target.to_vec2())
                    * Affine::scale(placement.scale)
                    * Affine::translate(-painted_centre.to_vec2()),
            );
            self.items[index].paint_child(ctx, scene);
            scene.pop_transform();
        }
        scene.pop_clip();

        if auto_rolling {
            ctx.request_frame_class(TickClass::CosmeticLoop);
        } else if owes_frame {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            for pod in &mut self.items {
                pod.event_child(ctx, event);
            }
            return EventResult::Ignored;
        }
        if self.count() == 0 {
            return EventResult::Ignored;
        }
        let size = ctx.size();
        let frame = self.frame;
        match event {
            InputEvent::Key(key) => {
                let Some(step) = arrow_step(key) else {
                    return EventResult::Ignored;
                };
                self.stop_glide();
                self.wheel_idle_since = None;
                let to = self.scroll().round() + step;
                self.glide_to(to, false);
                // The landing item is known the moment the key is pressed.
                let landing = wrapped_index(to, self.count());
                if landing != self.emitted {
                    self.emitted = landing;
                    if let Some(callback) = &mut self.on_index_change {
                        callback(ctx, landing);
                    }
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            InputEvent::Scroll { position, delta } => {
                if !inside(*position, size) {
                    return EventResult::Ignored;
                }
                if self.geometry.gap <= 0.0 {
                    return EventResult::Ignored;
                }
                // Upstream takes whichever axis moved further, so a trackpad's
                // horizontal swipe rolls the wall too.
                let (dx, dy) = match delta {
                    ScrollDelta::Lines(x, y) => (x * WHEEL_LINE_PX, y * WHEEL_LINE_PX),
                    ScrollDelta::Pixels(x, y) => (*x, *y),
                };
                let px = if dx.abs() > dy.abs() { dx } else { dy };
                self.stop_glide();
                let at = self.scroll() + px / self.geometry.gap;
                self.scroll.jump_to(at);
                self.emit(ctx);
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
                    self.stop_glide();
                    self.wheel_idle_since = None;
                    self.drag = Some(CarouselDrag {
                        anchor_x: p.position.x,
                        anchor_scroll: self.scroll(),
                        samples: vec![(p.position.x, frame)],
                    });
                    ctx.capture_pointer();
                    ctx.request_focus();
                    ctx.set_cursor(CursorIcon::Grabbing);
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Move => {
                    let gap = self.geometry.gap;
                    let drag_speed = self.drag_speed;
                    let Some(drag) = &mut self.drag else {
                        let over = inside(p.position, size);
                        if over != self.hovered {
                            self.hovered = over;
                            // The auto-roll pauses under the pointer, so the
                            // change is worth a repaint.
                            ctx.request_redraw();
                        }
                        if over {
                            ctx.claim_hover();
                            ctx.set_cursor(CursorIcon::Grab);
                        }
                        return EventResult::Ignored;
                    };
                    drag.samples.push((p.position.x, frame));
                    if drag.samples.len() > VELOCITY_SAMPLES {
                        drag.samples.remove(0);
                    }
                    let at = if gap > 0.0 {
                        drag.anchor_scroll - ((p.position.x - drag.anchor_x) * drag_speed) / gap
                    } else {
                        drag.anchor_scroll
                    };
                    self.scroll.jump_to(at);
                    // The captured arm re-asks every move, which keeps
                    // `Grabbing` alive outside the stage's own bounds.
                    ctx.set_cursor(CursorIcon::Grabbing);
                    self.emit(ctx);
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Up | PointerPhase::Cancel => {
                    let Some(drag) = self.drag.take() else {
                        return EventResult::Ignored;
                    };
                    let velocity = if p.phase == PointerPhase::Up {
                        self.release_velocity(&drag)
                    } else {
                        0.0
                    };
                    let to = self.flick_target(velocity);
                    self.glide_to(to, false);
                    // The landing item is already known, so it is reported here
                    // rather than from the coasting frames — see the module docs.
                    let landing = wrapped_index(to, self.count());
                    if landing != self.emitted {
                        self.emitted = landing;
                        if let Some(callback) = &mut self.on_index_change {
                            callback(ctx, landing);
                        }
                    }
                    ctx.request_redraw();
                    EventResult::Handled
                }
            },
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // `role="application" aria-roledescription="carousel"` upstream, with a
        // focusable stage. The closest published role is a list box; which item
        // is centred is *not* marked selected, because each item publishes its
        // own node through `semantics_child` and this container has no seam to
        // annotate one of them after the fact.
        ctx.push_container(
            Role::ListBox,
            |node| {
                node.add_action(Action::Click);
            },
            |ctx| {
                for pod in &self.items {
                    pod.semantics_child(ctx);
                }
            },
        );
    }

    visit_children!(items);
}

/// The roll step an arrow key asks for — upstream's `ArrowRight`/`ArrowLeft`.
fn arrow_step(key: &KeyEvent) -> Option<f64> {
    match &key.key {
        Key::Named(NamedKey::ArrowRight) => Some(1.0),
        Key::Named(NamedKey::ArrowLeft) => Some(-1.0),
        _ => None,
    }
}

/// Fold a continuous roll position onto an item index.
fn wrapped_index(scroll: f64, count: usize) -> usize {
    if count == 0 {
        return 0;
    }
    (scroll.round() as i64).rem_euclid(count as i64) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{Modifiers, PointerButton, PointerEvent};
    use frust::{SizedBox, any};
    use frust_core::{BuildCtx, EventCtx, PaintCtx};
    use std::any::Any;

    /// The stage every test lays out.
    const STAGE: Size = Size::new(800.0, 200.0);
    /// How many items the test carousel holds.
    const COUNT: usize = 8;

    /// Records the transforms each painted ball was pushed under.
    #[derive(Default)]
    struct Recorder {
        transforms: Vec<Affine>,
        clips: usize,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: peniko::Color) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn push_transform(&mut self, transform: Affine) {
            self.transforms.push(transform);
        }
        fn push_clip(&mut self, _origin: Point, _size: Size) {
            self.clips += 1;
        }
    }

    #[derive(Default)]
    struct Landings {
        seen: Vec<usize>,
    }

    fn items() -> Vec<frust::AnyView<Landings>> {
        (0..COUNT)
            .map(|_| any(SizedBox::<Landings>(Some(120.0), Some(120.0))))
            .collect()
    }

    fn view() -> CylinderCarouselView<Landings> {
        cylinder_carousel(items()).on_index_change(|state: &mut Landings, index| {
            state.seen.push(index);
        })
    }

    fn laid_out(view: &CylinderCarouselView<Landings>) -> CylinderCarouselWidget {
        let mut next_id = 0u64;
        let mut widget = View::<Landings>::build(view, &mut BuildCtx::new(&mut next_id));
        let mut text_ctx = TextContext::new();
        let mut layout = LayoutCtx::with_text_context(&mut text_ctx as &mut dyn Any);
        widget.layout(&mut layout, &BoxConstraints::tight(STAGE));
        widget
    }

    fn painted(
        widget: &mut CylinderCarouselWidget,
        ms: u64,
        theme: Option<&Theme>,
    ) -> (Recorder, bool) {
        let mut ctx =
            PaintCtx::for_test(Point::ORIGIN, STAGE, FrameTime::from_nanos(ms * 1_000_000));
        if let Some(theme) = theme {
            ctx = ctx.with_theme(theme);
        }
        let mut recorder = Recorder::default();
        widget.paint(&mut ctx, &mut recorder);
        (recorder, ctx.needs_frame())
    }

    fn send(
        widget: &mut CylinderCarouselWidget,
        state: &mut Landings,
        phase: PointerPhase,
        x: f64,
    ) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, STAGE);
        widget.event(
            &mut ctx,
            &InputEvent::Pointer(PointerEvent {
                phase,
                position: Point::new(x, 100.0),
                button: PointerButton::Primary,
            }),
        )
    }

    fn wheel(widget: &mut CylinderCarouselWidget, state: &mut Landings, px: f64) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, STAGE);
        widget.event(
            &mut ctx,
            &InputEvent::Scroll {
                position: Point::new(400.0, 100.0),
                delta: ScrollDelta::Pixels(0.0, px),
            },
        )
    }

    fn arrow(
        widget: &mut CylinderCarouselWidget,
        state: &mut Landings,
        named: NamedKey,
    ) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, STAGE);
        widget.event(
            &mut ctx,
            &InputEvent::Key(KeyEvent {
                key: Key::Named(named),
                modifiers: Modifiers::default(),
                repeat: false,
            }),
        )
    }

    /// Settle the glide without painting, so a test can observe where a flick
    /// actually lands.
    fn settle(widget: &mut CylinderCarouselWidget) {
        for step in 0..200 {
            widget
                .scroll
                .advance(FrameTime::from_nanos((step * 25) * 1_000_000));
        }
    }

    fn reduced() -> Theme {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        theme
    }

    /// The concave wall: the centred ball is smallest and lowest, each step out
    /// is visibly bigger than the last, and the edge ball's centre lands on the
    /// stage edge at scale one — the three properties upstream's projection is
    /// solved for.
    #[test]
    fn the_concave_wall_grows_outward_to_a_full_size_edge_ball() {
        let geometry = CylinderCarouselGeometry::resolve(
            STAGE.width,
            DEFAULT_VISIBLE_ITEMS,
            DEFAULT_ITEM_SIZE,
            DEFAULT_MIN_SCALE,
            CylinderCarouselVariant::Concave,
            None,
        );

        let centre = geometry.place(0.0);
        assert_eq!(centre.x, 0.0, "the centred ball sits on the axis");
        assert!(
            (centre.scale - DEFAULT_MIN_SCALE).abs() < 1e-9,
            "the centre is the smallest ball: {}",
            centre.scale
        );

        // Every step outward is bigger and further than the last.
        let mut last = centre;
        for slot in 1..=3 {
            let out = geometry.place(slot as f64);
            assert!(out.x > last.x, "slot {slot} did not move outward");
            assert!(out.scale > last.scale, "slot {slot} did not grow");
            last = out;
        }

        // The ball one slot past the frame edge has its centre on the edge, at
        // full size — what `THETA_EDGE` and `projection` are solved for.
        let edge = geometry.place(geometry.edge_offset);
        assert!(
            (edge.x - geometry.half_width).abs() < 1e-6,
            "the edge ball missed the stage edge: {}",
            edge.x
        );
        assert!(
            (edge.scale - 1.0).abs() < 1e-9,
            "the edge ball is not full size"
        );

        // The wall is symmetric.
        let mirrored = geometry.place(-2.0);
        let out = geometry.place(2.0);
        assert!((mirrored.x + out.x).abs() < 1e-9);
        assert!((mirrored.scale - out.scale).abs() < 1e-9);
        assert!((mirrored.y - out.y).abs() < 1e-9);

        // The valley: the centre dips below the edges.
        assert!(centre.y > out.y, "the concave curve is not a valley");
    }

    /// The convex wall inverts both curves — biggest and raised at the centre —
    /// and spaces uniformly rather than by the interior projection.
    #[test]
    fn the_convex_wall_inverts_the_curve_and_spaces_uniformly() {
        let geometry = CylinderCarouselGeometry::resolve(
            STAGE.width,
            DEFAULT_VISIBLE_ITEMS,
            DEFAULT_ITEM_SIZE,
            DEFAULT_MIN_SCALE,
            CylinderCarouselVariant::Convex,
            None,
        );
        let centre = geometry.place(0.0);
        let out = geometry.place(2.0);
        assert!((centre.scale - 1.0).abs() < 1e-9, "the centre is full size");
        assert!(out.scale < centre.scale, "the wall did not shrink outward");
        assert!(centre.y < out.y, "the convex curve is not an arch");

        // Uniform spacing: every step is exactly one `gap`.
        for slot in 1..4 {
            let at = geometry.place(slot as f64);
            assert!(
                (at.x - slot as f64 * geometry.gap).abs() < 1e-9,
                "convex spacing is not uniform at {slot}"
            );
        }

        assert_eq!(CylinderCarouselVariant::ALL.len(), 2);
        assert!(CylinderCarouselVariant::Convex.is_convex());
        assert!(!CylinderCarouselVariant::Concave.is_convex());
    }

    /// A ball far enough off-stage stops painting, and the wrap keeps every item
    /// on the nearest side of the loop.
    #[test]
    fn far_items_are_culled_and_the_loop_wraps_both_ways() {
        let geometry = CylinderCarouselGeometry::resolve(
            STAGE.width,
            DEFAULT_VISIBLE_ITEMS,
            DEFAULT_ITEM_SIZE,
            DEFAULT_MIN_SCALE,
            CylinderCarouselVariant::Concave,
            None,
        );
        assert!(!geometry.place(0.0).hidden);
        assert!(
            geometry.place(20.0).hidden,
            "an off-stage ball still painted"
        );

        // The wrap: item 7 of 8 is one step *behind* item 0, not seven ahead.
        assert_eq!(CylinderCarouselGeometry::wrapped_offset(7, 0.0, 8), -1.0);
        assert_eq!(CylinderCarouselGeometry::wrapped_offset(1, 0.0, 8), 1.0);
        assert_eq!(CylinderCarouselGeometry::wrapped_offset(0, 7.0, 8), 1.0);
        // A degenerate list is inert rather than a division by zero.
        assert_eq!(CylinderCarouselGeometry::wrapped_offset(0, 3.0, 0), 0.0);
    }

    /// The fit pass caps the item box against the stage, and `itemSize` only
    /// caps the result.
    #[test]
    fn the_item_box_is_fitted_to_the_stage() {
        let roomy = CylinderCarouselGeometry::resolve(
            4000.0,
            DEFAULT_VISIBLE_ITEMS,
            DEFAULT_ITEM_SIZE,
            DEFAULT_MIN_SCALE,
            CylinderCarouselVariant::Concave,
            None,
        );
        assert_eq!(roomy.size, DEFAULT_ITEM_SIZE, "a wide stage takes the cap");

        let narrow = CylinderCarouselGeometry::resolve(
            320.0,
            DEFAULT_VISIBLE_ITEMS,
            DEFAULT_ITEM_SIZE,
            DEFAULT_MIN_SCALE,
            CylinderCarouselVariant::Concave,
            None,
        );
        assert!(
            narrow.size < DEFAULT_ITEM_SIZE,
            "a narrow stage did not shrink"
        );
        assert!((narrow.arc - narrow.size * ARC_FRACTION).abs() < 1e-9);
        assert!((narrow.gap - 320.0 / 6.0).abs() < 1e-9);

        // An explicit arc wins over the derived one, including a flat wall.
        let flat = CylinderCarouselGeometry::resolve(
            STAGE.width,
            DEFAULT_VISIBLE_ITEMS,
            DEFAULT_ITEM_SIZE,
            DEFAULT_MIN_SCALE,
            CylinderCarouselVariant::Concave,
            Some(0.0),
        );
        assert_eq!(flat.arc, 0.0);
        assert_eq!(flat.place(0.0).y, 0.0);
        assert_eq!(flat.place(2.0).y, 0.0);
    }

    /// A drag rolls the wall against the pointer at `dragSpeed`, and a flick
    /// coasts past where the finger stopped before snapping.
    #[test]
    fn a_drag_rolls_the_wall_and_a_flick_coasts_and_snaps() {
        let mut widget = laid_out(&view());
        let mut state = Landings::default();
        painted(&mut widget, 0, None);

        send(&mut widget, &mut state, PointerPhase::Down, 400.0);
        // Dragging right rolls the wall left: `scroll` decreases.
        send(&mut widget, &mut state, PointerPhase::Move, 500.0);
        let rolled = widget.scroll();
        let expected = -(100.0 * DEFAULT_DRAG_SPEED) / widget.geometry().gap;
        assert!((rolled - expected).abs() < 1e-9, "drag mapping: {rolled}");

        // A flick projects further than the finger reached, and snaps.
        let coast = widget.flick_target(4.0);
        assert!(
            coast > rolled,
            "a positive flick did not coast forward: {coast}"
        );
        assert_eq!(coast, coast.round(), "snap(true) did not land on an item");
        // …and its reach is capped.
        assert_eq!(
            widget.flick_target(1_000.0),
            (rolled + MAX_FLICK_ITEMS).round()
        );

        // Without snapping it lands wherever the projection put it.
        let loose = laid_out(&view().snap(false));
        assert_ne!(loose.flick_target(0.3), loose.flick_target(0.3).round());
    }

    /// A release glides toward the projected item and settles exactly on it.
    #[test]
    fn a_release_glides_onto_its_landing_item() {
        let mut widget = laid_out(&view());
        let mut state = Landings::default();
        painted(&mut widget, 0, None);

        send(&mut widget, &mut state, PointerPhase::Down, 600.0);
        send(&mut widget, &mut state, PointerPhase::Move, 300.0);
        send(&mut widget, &mut state, PointerPhase::Up, 300.0);
        assert!(widget.is_gliding(), "a release did not start a glide");
        let target = widget.scroll.target();
        assert_eq!(target, target.round(), "the glide is not aimed at an item");

        settle(&mut widget);
        assert!(!widget.is_gliding(), "the glide never settled");
        assert!((widget.scroll() - target).abs() < 1e-9);
        assert_eq!(widget.index(), wrapped_index(target, COUNT));

        // The landing index was reported at release, not once per coasting
        // frame.
        assert!(!state.seen.is_empty(), "no index was reported");
        assert_eq!(*state.seen.last().unwrap(), widget.index());
    }

    /// The wheel rolls continuously and snaps once it goes quiet, and an arrow
    /// key steps exactly one item.
    #[test]
    fn the_wheel_settles_and_an_arrow_key_steps_one_item() {
        let mut widget = laid_out(&view());
        let mut state = Landings::default();
        painted(&mut widget, 0, None);

        assert_eq!(wheel(&mut widget, &mut state, 300.0), EventResult::Handled);
        let rolled = widget.scroll();
        assert!(rolled > 0.0, "the wheel did not roll: {rolled}");
        assert!(!widget.is_gliding(), "a wheel notch started a glide");

        // Still inside the idle window: nothing snaps yet.
        painted(&mut widget, 50, None);
        assert!(!widget.is_gliding());
        // Past it, the roll snaps to the nearest item.
        painted(&mut widget, 300, None);
        assert!(widget.is_gliding(), "the wheel never settled");
        assert_eq!(widget.scroll.target(), rolled.round());

        // An arrow key steps one item from wherever the roll rests.
        let mut widget = laid_out(&view());
        painted(&mut widget, 0, None);
        assert_eq!(
            arrow(&mut widget, &mut state, NamedKey::ArrowRight),
            EventResult::Handled
        );
        assert_eq!(widget.scroll.target(), 1.0);
        arrow(&mut widget, &mut state, NamedKey::ArrowLeft);
        settle(&mut widget);
        arrow(&mut widget, &mut state, NamedKey::ArrowLeft);
        assert!(widget.scroll.target() < 1.0);

        // A key that is not an arrow is nobody's business here.
        assert_eq!(
            arrow(&mut widget, &mut state, NamedKey::Escape),
            EventResult::Ignored
        );
    }

    /// The auto-roll advances on a real frame delta, pauses under the pointer,
    /// and asks for a cosmetic cadence rather than a transition.
    #[test]
    fn the_auto_roll_advances_and_pauses_under_the_pointer() {
        let mut widget = laid_out(&view().auto_rotate(true));
        let mut state = Landings::default();

        // The first frame establishes the clock.
        let (_, needs_frame) = painted(&mut widget, 0, None);
        assert!(needs_frame);
        assert_eq!(widget.scroll(), 0.0);

        painted(&mut widget, 1_000, None);
        assert!(
            (widget.scroll() - DEFAULT_AUTO_ROTATE_SPEED).abs() < 1e-9,
            "a second of auto-roll moved {}",
            widget.scroll()
        );

        // A hover pauses it.
        send(&mut widget, &mut state, PointerPhase::Move, 400.0);
        let held = widget.scroll();
        painted(&mut widget, 2_000, None);
        assert_eq!(widget.scroll(), held, "the auto-roll ran under the pointer");

        // And a carousel that was never asked to roll does not.
        let mut still = laid_out(&view());
        painted(&mut still, 0, None);
        let (_, needs_frame) = painted(&mut still, 1_000, None);
        assert!(!needs_frame, "a resting carousel asked for a frame");
        assert_eq!(still.scroll(), 0.0);
    }

    /// Every visible ball is painted under its own transform, inside one clip,
    /// with the centred one placed on the stage's own centre.
    #[test]
    fn every_visible_ball_paints_under_its_own_transform() {
        let mut widget = laid_out(&view());
        let (recorder, _) = painted(&mut widget, 0, None);
        assert_eq!(recorder.clips, 1, "clip-path: inset(0)");
        assert!(!recorder.transforms.is_empty());
        assert!(
            recorder.transforms.len() <= COUNT,
            "more transforms than items"
        );

        // The centred ball's box lands centred on the stage.
        let item = widget.geometry().size;
        let painted_centre = Point::new(item / 2.0, item / 2.0);
        let placed = recorder.transforms[0] * painted_centre;
        assert!(
            (placed.x - STAGE.width / 2.0).abs() < 1e-6,
            "the centred ball is off-axis: {placed:?}"
        );
    }

    /// `reduce_motion` drops the glide — the registry's own words — so a release
    /// lands on its item at once and nothing is left in flight.
    #[test]
    fn reduce_motion_drops_the_glide() {
        let theme = reduced();
        let mut widget = laid_out(&view());
        let mut state = Landings::default();
        painted(&mut widget, 0, Some(&theme));

        send(&mut widget, &mut state, PointerPhase::Down, 600.0);
        send(&mut widget, &mut state, PointerPhase::Move, 300.0);
        send(&mut widget, &mut state, PointerPhase::Up, 300.0);
        let target = widget.scroll.target();

        let (_, needs_frame) = painted(&mut widget, 10, Some(&theme));
        assert!(!needs_frame, "reduced motion still animated the roll");
        assert_eq!(widget.scroll(), target, "the roll did not land at once");

        // Nor does it auto-roll.
        let mut spinning = laid_out(&view().auto_rotate(true));
        painted(&mut spinning, 0, Some(&theme));
        let (_, needs_frame) = painted(&mut spinning, 1_000, Some(&theme));
        assert!(!needs_frame);
        assert_eq!(spinning.scroll(), 0.0);
    }

    /// The fallback contract, from this control's side: no shell installs a
    /// device in a test process, so the projected wall never claims one and the
    /// stage paints *precisely* the transforms it paints with the mode off.
    /// This is the path every caller of `gpu_cylinder` gets on Android, on
    /// iOS, and on a desktop frame before the first surface exists.
    #[cfg(feature = "gpu-effects")]
    #[test]
    fn a_wall_without_a_device_paints_exactly_its_two_dimensional_path() {
        let mut flat = laid_out(&view());
        let mut projected = laid_out(&view().gpu_cylinder(true));
        let (flat_paint, _) = painted(&mut flat, 0, None);
        let (projected_paint, _) = painted(&mut projected, 0, None);

        assert!(
            !projected.fx.is_active(),
            "a test process has no shell device"
        );
        assert!(projected.gpu_cylinder, "the opt-in is still recorded");
        assert!(!flat.gpu_cylinder);
        assert_eq!(projected_paint.transforms, flat_paint.transforms);
        assert_eq!(projected_paint.clips, flat_paint.clips);
    }

    /// The claim that keeps the two paths from drifting: every plate lands on
    /// exactly the box its flat ball paints into — same centre, and a depth
    /// that projects it back to the same scale — while carrying the turn into
    /// the wall that the flat ball cannot.
    #[cfg(feature = "gpu-effects")]
    #[test]
    fn every_plate_lands_on_its_flat_balls_own_box() {
        let mut widget = laid_out(&view().gpu_cylinder(true));
        // Mid-roll, so no ball is dead centre.
        widget.scroll.jump_to(1.35);
        let (_, extent) = card3d::target_for(Point::ORIGIN, STAGE).expect("a stage with area");
        let faces = widget.wall_faces(extent, Color::WHITE);

        let on_stage: Vec<usize> = (0..widget.count())
            .filter(|index| {
                let offset = CylinderCarouselGeometry::wrapped_offset(
                    *index,
                    widget.scroll(),
                    widget.count(),
                );
                !widget.geometry.place(offset).hidden
            })
            .collect();
        assert_eq!(faces.len(), on_stage.len());

        let camera = f64::from(crate::gpu_fx::quad3d::CAMERA_DISTANCE);
        let centre = Point::new(f64::from(extent.0) / 2.0, f64::from(extent.1) / 2.0);
        for (face, index) in faces.iter().zip(&on_stage) {
            let offset =
                CylinderCarouselGeometry::wrapped_offset(*index, widget.scroll(), widget.count());
            let placement = widget.geometry.place(offset);
            assert!((face.dest.center().x - centre.x - placement.x).abs() < 1e-9);
            assert!((face.dest.center().y - centre.y - placement.y).abs() < 1e-9);
            assert!((face.dest.width() - widget.geometry.size).abs() < 1e-9);
            // The depth is the one that projects the plate back onto the ball's
            // own scale, which is what keeps the two the same size.
            let projected = camera / (camera - f64::from(face.depth_offset));
            assert!(
                (projected - placement.scale).abs() < 1e-5,
                "plate {index} projects at {projected} where its ball is scaled {}",
                placement.scale
            );
            assert!(
                face.pitch.abs() < 1e-9,
                "a vertical axis yaws, never pitches"
            );
            assert!(face.is_renderable());
        }

        // The wall is concave by default, so a ball right of centre turns its
        // right edge toward the viewer and one left of centre its left edge.
        let right = faces
            .iter()
            .max_by(|a, b| a.dest.center().x.total_cmp(&b.dest.center().x))
            .expect("a ball on stage");
        let left = faces
            .iter()
            .min_by(|a, b| a.dest.center().x.total_cmp(&b.dest.center().x))
            .expect("a ball on stage");
        assert!(right.yaw < 0.0 && left.yaw > 0.0);
    }

    /// Lining the outside of the wall inverts both the turn and the order the
    /// plates occlude in — the same drum seen from the other side.
    #[cfg(feature = "gpu-effects")]
    #[test]
    fn a_convex_wall_turns_and_stacks_the_other_way() {
        let (_, extent) = card3d::target_for(Point::ORIGIN, STAGE).expect("a stage with area");
        let faces_for = |variant: CylinderCarouselVariant| {
            let mut widget = laid_out(&view().variant(variant).gpu_cylinder(true));
            widget.scroll.jump_to(1.35);
            widget.wall_faces(extent, Color::WHITE)
        };
        let concave = faces_for(CylinderCarouselVariant::Concave);
        let convex = faces_for(CylinderCarouselVariant::Convex);

        // How far from the stage's centre line the nearest plate sits, against
        // how far the furthest-out one does: an outside wall is nearest in the
        // middle, an inside wall at its ends.
        let stage_centre = f64::from(extent.0) / 2.0;
        let from_centre = |face: &Quad3d| (face.dest.center().x - stage_centre).abs();
        let nearest = |faces: &[Quad3d]| {
            faces
                .iter()
                .max_by(|a, b| a.depth_offset.total_cmp(&b.depth_offset))
                .map(&from_centre)
                .expect("a ball on stage")
        };
        let widest = |faces: &[Quad3d]| faces.iter().map(&from_centre).fold(0.0_f64, f64::max);
        assert!(
            (nearest(&concave) - widest(&concave)).abs() < 1e-9,
            "the inside of the wall comes forward at its ends"
        );
        assert!(
            nearest(&convex) < widest(&convex),
            "the outside of the wall is nearest in the middle"
        );
        // ...and the turn mirrors with the side.
        let right = |faces: &[Quad3d]| {
            faces
                .iter()
                .max_by(|a, b| a.dest.center().x.total_cmp(&b.dest.center().x))
                .expect("a ball on stage")
                .yaw
        };
        assert!(right(&concave) < 0.0 && right(&convex) > 0.0);
    }

    /// The mode is a live opt-in: switching it off mid-life stops the wall
    /// without giving up the claim, and switching it back on resumes.
    #[cfg(feature = "gpu-effects")]
    #[test]
    fn the_projected_wall_switches_on_and_off_in_place() {
        let mut next_id = 0u64;
        let mut ctx = BuildCtx::new(&mut next_id);
        let off = view();
        let on = view().gpu_cylinder(true);
        let mut widget = View::<Landings>::build(&off, &mut ctx);
        assert!(!widget.gpu_cylinder);

        let flags = View::<Landings>::rebuild(&on, &off, &mut widget, &mut ctx);
        assert!(widget.gpu_cylinder);
        assert!(flags.contains(ChangeFlags::PAINT));

        let flags = View::<Landings>::rebuild(&off, &on, &mut widget, &mut ctx);
        assert!(!widget.gpu_cylinder);
        assert!(flags.contains(ChangeFlags::PAINT));

        View::<Landings>::teardown(&off, &mut widget, &mut ctx);
        assert!(!widget.fx.is_active());
    }

    /// An empty carousel is inert rather than a division by zero.
    #[test]
    fn an_empty_carousel_is_inert() {
        let mut widget = laid_out(&cylinder_carousel::<Landings, _>(
            Vec::<AnyView<Landings>>::new(),
        ));
        let mut state = Landings::default();
        assert_eq!(widget.count(), 0);
        assert_eq!(widget.index(), 0);
        assert_eq!(
            send(&mut widget, &mut state, PointerPhase::Down, 400.0),
            EventResult::Ignored
        );
        let (recorder, needs_frame) = painted(&mut widget, 0, None);
        assert!(!needs_frame);
        assert!(recorder.transforms.is_empty());
    }
}
