//! Retained-page boundaries: [`repaint_boundary(child)`](repaint_boundary) and
//! the boundary state its adopters (the navigator's pages, the pattern
//! switcher's children, a [`.cached(true)`](crate::motion::AnimatedOpacityView::cached)
//! `AnimatedOpacity`) share.
//!
//! # What a boundary paints
//!
//! A boundary paints its child's body into a page the renderer keeps across
//! frames (a `PushRetained`/`PopRetained` bracket keyed by the boundary's own
//! [`RetainedKey`]) and, on a later frame where nothing under the child changed
//! and the renderer's resident-key ledger still holds that page, composites the
//! page (`DrawRetained`) instead of painting the child again. Per frame:
//!
//! - **Draw the page** when the child's subtree is clean
//!   ([`PaintCtx::subtree_clean`]), the boundary recorded its page under the
//!   linear part (scale/rotation/skew) the scene is at now, the body's last
//!   paint used no paint-time side channel ([`PaintCtx::paint_publications`]),
//!   the ledger reports the key resident at the extent and scale the page
//!   would be drawn at ([`PaintCtx::retained_available`]), and the ledger
//!   proves the page holds the boundary's latest recording (see *Which
//!   recording the page holds* below). The child is not painted; its pod stays
//!   clean.
//! - **Record the page** otherwise, when recording can pay off: the body is
//!   bracketed and painted.
//! - **Paint the body directly** (a `push_layer` when the composite alpha is
//!   below 1) when retained layers are off or no ledger was threaded
//!   ([`PaintCtx::retained_enabled`] — the `FRUST_NO_RETAINED_LAYERS` kill
//!   switch), or the caller says the body cannot be recorded this frame.
//!
//! # Composite parameters
//!
//! The bracket's `scale` is always `1.0`: a page is served only at the linear
//! part it was rasterized at, so asking for one at another scale would record
//! a new page every frame and gain nothing. A boundary composites at
//! [`alpha`](RepaintBoundaryView::alpha) and at its own origin; a page drawn at
//! another origin than it was recorded at is moved through a translation
//! transform, which the renderer rounds to whole device pixels (a fractional
//! slide sits up to half a pixel off the inline frame). A container whose
//! transition step has a scale component paints that frame inline instead.
//!
//! # Staleness
//!
//! "Clean" means unchanged since the body was last *painted*, not since it was
//! last *recorded*, so every paint of the body outside the bracket while it was
//! dirty forgets the recorded page: the next draw must follow a fresh record.
//! An adopter therefore paints its child only through its boundary
//! ([`RetainedBoundary::paint`] or [`RetainedBoundary::paint_plain`]).
//!
//! # Which recording the page holds
//!
//! A resident page is not necessarily the boundary's latest recording: the
//! scene carrying that recording can be dropped on its way to the renderer
//! (the render split's scene slot is latest-wins), leaving the previous page
//! resident under the same key. The ledger's
//! [`frame`](frust_core::RetainedLedger::frame) — the paint serial
//! ([`PaintCtx::paint_serial`]) of the last scene the renderer committed — is
//! what tells them apart, and a boundary keeps a **run**: the serials of the
//! consecutive paints, starting at `serial`, that each recorded the current
//! body at the same origin, size and linear part. Every scene in a run carries
//! the same recording, so a ledger whose frame falls inside the run proves the
//! page holds it — the page is then *confirmed* and may be drawn until the body
//! changes. A recording at another origin starts a new run: a body the parent
//! moved between two recordings must not be vouched for by a ledger that only
//! describes the earlier one.
//!
//! Until then the page is drawn only when the ledger covers the run's start
//! **and** the boundary recorded on the immediately previous paint (so no
//! scene after the run, which might have committed without the recording,
//! can be the one the ledger describes). Otherwise it records again: on the
//! next paint after a dirty one, a clean body continues the run, so a ledger
//! that lags the paints by a frame or two still lands inside it — the
//! boundary never records forever behind a renderer that is merely late.
//! A context no render root numbered (serial `0`, a widget unit test) skips
//! the serial check.
//!
//! See `docs/WIDGETS_ARCHITECTURE.md` for where boundaries sit in the widget set
//! and the renderer contract in `frust-engine`'s `cache::retained` module docs.

use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent, LayoutCtx,
    PaintCtx, PaintScene, RetainedKey, SemanticsCtx, View, Widget,
};
use kurbo::{Affine, Point, Rect, Size};

/// How far apart two linear parts may be and still be read as one — the
/// renderer's own tolerance for serving a page (`RetainedPlacement::LINEAR_EPSILON`
/// in `frust-engine`), tighter than the ledger's scale tolerance, so a page is
/// never drawn under a linear part the renderer would refuse.
const LINEAR_EPSILON: f64 = 1e-6;

/// How a [`RetainedBoundary`] painted its body this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BoundaryPaint {
    /// Composited the retained page; the body was not painted.
    Retained,
    /// Painted the body inside a `PushRetained`/`PopRetained` bracket.
    Recorded,
    /// Painted the body with no bracket.
    Direct,
}

/// What a boundary remembers of the page it last recorded.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Recorded {
    /// The scene's linear part (`[a, b, c, d]`) the bracket was recorded under.
    linear: [f64; 4],
    /// The bracket rectangle's origin (the body's paint origin).
    origin: Point,
    /// The bracket rectangle's size.
    size: Size,
    /// The paint serial the run of recordings of this body started at — the
    /// first of the consecutive paints that each recorded it at this origin,
    /// size and linear part (see the [module docs](self)).
    serial: u64,
    /// The paint serial of the latest recording in the run.
    last: u64,
    /// Whether a ledger has proven that the renderer holds this recording.
    confirmed: bool,
}

impl Recorded {
    /// Whether the page may still be vouched for at paint `serial`: confirmed,
    /// recorded on the immediately previous paint, or painted with no serial
    /// at all.
    fn usable_at(&self, serial: u64) -> bool {
        self.confirmed || serial == 0 || self.last.wrapping_add(1) == serial
    }
}

/// The per-owner state of one retained-page boundary: its key, the placement
/// of the page it last recorded, and whether its body uses a paint-time side
/// channel. One per navigator page, per switcher child, per cached
/// `AnimatedOpacity`, per [`RepaintBoundaryWidget`]. See the
/// [module docs](self).
#[derive(Debug)]
pub(crate) struct RetainedBoundary {
    key: RetainedKey,
    recorded: Option<Recorded>,
    publishes: bool,
}

impl RetainedBoundary {
    /// A boundary with a freshly minted, process-unique key.
    pub(crate) fn new() -> Self {
        Self::with_key(RetainedKey::mint())
    }

    /// A boundary drawing on `key` (an owner that reserved a key range).
    pub(crate) fn with_key(key: RetainedKey) -> Self {
        Self {
            key,
            recorded: None,
            publishes: false,
        }
    }

    /// The key this boundary records and draws its page under.
    pub(crate) fn key(&self) -> RetainedKey {
        self.key
    }

    /// Paint `pod`'s body through the boundary at `origin`/`size` (absolute
    /// paint coordinates — the body's own paint origin) composited at `alpha`
    /// (clamped to `[0, 1]`): draw the retained page, record it, or paint the
    /// body directly — see the [module docs](self). `may_record` is the
    /// caller's word that the body sits wholly inside every clip around it this
    /// frame (a renderer never retains a clipped body, so bracketing one is
    /// wasted work); a scroll ancestor's visible rect is checked here as well.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint(
        &mut self,
        pod: &mut ChildPod,
        ctx: &mut PaintCtx,
        scene: &mut dyn PaintScene,
        origin: Point,
        size: Size,
        alpha: f32,
        may_record: bool,
    ) -> BoundaryPaint {
        let alpha = alpha.clamp(0.0, 1.0);
        let transform = scene.current_transform().unwrap_or(Affine::IDENTITY);
        let finite = transform.as_coeffs().iter().all(|c| c.is_finite());
        if !ctx.retained_enabled() || !finite {
            self.paint_direct(pod, ctx, scene, origin, size, alpha);
            return BoundaryPaint::Direct;
        }
        let [a, b, c, d, _, _] = transform.as_coeffs();
        let linear = [a, b, c, d];
        let serial = ctx.paint_serial();

        if let Some(recorded) = self.recorded
            && self.can_draw(pod, ctx, size, linear, recorded)
        {
            // Drawing proves nothing new once confirmed; the first draw is
            // the proof (see the module docs).
            self.recorded = Some(Recorded {
                confirmed: true,
                ..recorded
            });
            // The page sits where the body was recorded; move it to where the
            // body paints now. The renderer reads a composite's position from
            // its transform alone, so the move is a translation, never a new
            // rectangle origin.
            let shift = origin - recorded.origin;
            let moved = shift.hypot2() > 0.0;
            if moved {
                scene.push_transform(Affine::translate(shift));
            }
            scene.draw_retained(self.key.get(), recorded.origin, size, alpha, 1.0);
            if moved {
                scene.pop_transform();
            }
            return BoundaryPaint::Retained;
        }

        let visible = ctx
            .visible_rect()
            .is_none_or(|v| rect_contains(v, Rect::from_origin_size(origin, size)));
        if may_record && visible {
            // A clean body recorded again on the paint right after its last
            // recording, under the same linear part, origin and size, continues
            // that run: the two scenes carry the same page. A recording at
            // another origin starts a new run, so a ledger can only confirm the
            // page recorded where the body now paints. Size is compared too:
            // `can_draw` checks the drawn size against the ledger's extent, but
            // a run must never mix two recordings of different sizes.
            let run_start = self
                .recorded
                .filter(|r| {
                    !r.confirmed
                        && serial != 0
                        && r.last.wrapping_add(1) == serial
                        && r.origin == origin
                        && r.size == size
                        && same_linear(r.linear, linear)
                        && ctx.subtree_clean(pod)
                })
                .map_or(serial, |r| r.serial);
            scene.push_retained(self.key.get(), origin, size, alpha, 1.0);
            self.paint_body(pod, ctx, scene);
            scene.pop_retained();
            self.recorded = Some(Recorded {
                linear,
                origin,
                size,
                serial: run_start,
                last: serial,
                confirmed: false,
            });
            return BoundaryPaint::Recorded;
        }

        self.paint_direct(pod, ctx, scene, origin, size, alpha);
        BoundaryPaint::Direct
    }

    /// Paint `pod`'s body with no bracket and no composite of its own — for a
    /// caller that wraps the paint itself (a scale step, an alpha-zero
    /// `DiscardScene` redirect, an owner at rest). Forgets the recorded page
    /// when the body was dirty, since painting it moves it past that page.
    pub(crate) fn paint_plain(
        &mut self,
        pod: &mut ChildPod,
        ctx: &mut PaintCtx,
        scene: &mut dyn PaintScene,
    ) {
        if !ctx.subtree_clean(pod) {
            self.recorded = None;
        }
        self.paint_body(pod, ctx, scene);
    }

    /// Paint the body directly under a `push_layer` when `alpha < 1`.
    fn paint_direct(
        &mut self,
        pod: &mut ChildPod,
        ctx: &mut PaintCtx,
        scene: &mut dyn PaintScene,
        origin: Point,
        size: Size,
        alpha: f32,
    ) {
        let layered = alpha < 1.0;
        if layered {
            scene.push_layer(origin, size, alpha);
        }
        self.paint_plain(pod, ctx, scene);
        if layered {
            scene.pop_layer();
        }
    }

    /// Paint the body, noting whether it used a paint-time side channel.
    fn paint_body(&mut self, pod: &mut ChildPod, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let before = ctx.paint_publications();
        pod.paint_child(ctx, scene);
        self.publishes = ctx.paint_publications() != before;
    }

    /// Whether the page recorded at `recorded` can stand in for the body this
    /// frame, drawn under `linear` at `size`.
    fn can_draw(
        &self,
        pod: &ChildPod,
        ctx: &PaintCtx,
        size: Size,
        linear: [f64; 4],
        recorded: Recorded,
    ) -> bool {
        if self.publishes
            || !ctx.subtree_clean(pod)
            || !recorded.usable_at(ctx.paint_serial())
            || !same_linear(recorded.linear, linear)
        {
            return false;
        }
        // The extent and scale the renderer reports for a page drawn under
        // `linear` (its `RetainedPlacement::extent`/`scale`).
        let [a, b, c, d] = linear;
        let extent = Size::new(size.width * a.hypot(b), size.height * c.hypot(d));
        let scale = (a * d - b * c).abs().sqrt() as f32;
        ctx.retained_available(self.key, extent, scale, recorded.serial)
    }
}

/// Whether two linear parts are one, to [`LINEAR_EPSILON`].
fn same_linear(a: [f64; 4], b: [f64; 4]) -> bool {
    a.iter()
        .zip(b.iter())
        .all(|(x, y)| (x - y).abs() <= LINEAR_EPSILON)
}

/// Whether `inner` lies within `outer`, to a hair of floating-point slack.
fn rect_contains(outer: Rect, inner: Rect) -> bool {
    const SLACK: f64 = 1e-6;
    inner.x0 >= outer.x0 - SLACK
        && inner.y0 >= outer.y0 - SLACK
        && inner.x1 <= outer.x1 + SLACK
        && inner.y1 <= outer.y1 + SLACK
}

/// Whether a body at `origin`/`size` lies wholly inside `clip_origin`/
/// `clip_size` — what a container that clips its children asks before letting
/// a boundary record one.
pub(crate) fn within(origin: Point, size: Size, clip_origin: Point, clip_size: Size) -> bool {
    rect_contains(
        Rect::from_origin_size(clip_origin, clip_size),
        Rect::from_origin_size(origin, size),
    )
}

// --- RepaintBoundary ---------------------------------------------------------

/// A declarative retained-page boundary. See the [module docs](self).
pub struct RepaintBoundaryView<V> {
    child: V,
    alpha: f32,
}

/// Paint `child` through a retained-page boundary: its body is recorded once
/// and composited from the renderer's cached page on every later frame where
/// nothing under it changed — see the [module docs](self). Layout, hit-testing
/// and semantics are the child's own, unchanged.
///
/// Worth it for a body that is expensive to paint and often unchanged while
/// something around it animates or moves (a scrolled card, a panel under a
/// fade). Never nest two boundaries over the same body.
pub fn repaint_boundary<V>(child: V) -> RepaintBoundaryView<V> {
    RepaintBoundaryView { child, alpha: 1.0 }
}

impl<V> RepaintBoundaryView<V> {
    /// Composite the body at `alpha` (clamped to `[0, 1]`; default `1.0`). A
    /// fade over an unchanged body composites the retained page at no paint
    /// cost.
    pub fn alpha(mut self, alpha: f32) -> Self {
        self.alpha = alpha.clamp(0.0, 1.0);
        self
    }
}

/// The retained widget for a [`RepaintBoundaryView`].
pub struct RepaintBoundaryWidget {
    child: ChildPod,
    alpha: f32,
    boundary: RetainedBoundary,
}

impl RepaintBoundaryWidget {
    /// The key this boundary records and draws its page under.
    pub fn retained_key(&self) -> RetainedKey {
        self.boundary.key()
    }
}

impl<State: 'static, V: View<State>> View<State> for RepaintBoundaryView<V> {
    type Element = RepaintBoundaryWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> RepaintBoundaryWidget {
        RepaintBoundaryWidget {
            child: crate::authoring::build_child(&self.child, ctx),
            alpha: self.alpha,
            boundary: RetainedBoundary::new(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut RepaintBoundaryWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.alpha != self.alpha {
            element.alpha = self.alpha;
            flags |= ChangeFlags::PAINT;
        }
        flags |= crate::authoring::rebuild_child(&prev.child, &self.child, &mut element.child, ctx);
        flags
    }

    fn teardown(&self, element: &mut RepaintBoundaryWidget, ctx: &mut BuildCtx<'_>) {
        crate::authoring::teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for RepaintBoundaryWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // Transparent wrapper: exactly the child's size.
        let size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ZERO);
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let origin = ctx.origin();
        let size = ctx.size();
        self.boundary
            .paint(&mut self.child, ctx, scene, origin, size, self.alpha, true);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        crate::authoring::route_event_single(&mut self.child, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.child.semantics_child(ctx);
    }

    crate::authoring::visit_children!(child);
}
