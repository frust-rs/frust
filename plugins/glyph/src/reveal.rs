//! [`glyph_reveal`]/[`GlyphRevealView`]: the tree-expansion vocabulary — a
//! controlled container that animates its own height between nothing and its
//! children's natural extent, cascades those children in as it opens, and
//! optionally rules a dashed guide line down its leading edge. [`glyph_chevron`]
//! is its companion leaf: the small rotating disclosure marker a row pairs with
//! a reveal (public on its own, so any row can reuse it).
//!
//! This module contributes the *reveal*, never the trigger: the header row, its
//! press target, and the `expanded` flag all belong to the composing app. For a
//! packaged header-plus-body disclosure panel with its own chrome, see
//! [`mod@crate::accordion`].
//!
//! # Controlled, and settled on first build
//!
//! [`GlyphRevealView::expanded`] is app-owned; this widget never flips it. A
//! node that is **already expanded on its first build renders settled** — full
//! height, no height tween, no cascade — so a resting pre-expanded tree draws
//! itself in one frame. Only a post-mount flip animates, in either direction.
//!
//! # Height reveal, and why it requests layout
//!
//! Children are always laid out at their full natural height and stay laid out;
//! the reveal is a `0..1` fraction applied twice — the height this widget
//! reports is `fraction · natural`, and paint clips the children to that same
//! band. The fraction is an implicit tween on the paint clock (a fresh
//! `AnimationController` per flip, `motion.durations.base` = 220ms on the
//! `motion.easing.spatial` curve), so `paint` must ask for a **relayout** — not
//! a bare frame — while it runs, or the mobile intra-frame layout skip freezes
//! the reported height mid-flight. `paint` also compares the fraction against
//! the previous pass's and requests one more relayout whenever they differ,
//! which covers the driver's landing frame (`advance` reports `false` on the
//! very pass that snaps onto the target) and the `reduce_motion` snap with the
//! same rule. [`mod@crate::accordion`]'s module docs carry the long form; the driver
//! here is a private port of the same shape.
//!
//! # The child cascade
//!
//! While *expanding*, child `i` reveals on [`GlyphStagger::glyph`]'s timing (90ms
//! per-item delay, 150ms `effects`-curve reveal each) driven by its own
//! controller sized to `GlyphStagger::total_duration(n)` — decoupled from the
//! height tween, so the literal per-item millisecond spacing stays meaningful
//! whatever the height animation is doing. Each child fades on that progress and
//! slides in from [`GLYPH_REVEAL_SLIDE_DP`] to the right of its resting spot. A
//! child whose window has not opened yet paints nothing at all — and is not
//! hit-testable either: pointer/scroll input reaches only the children the last
//! paint actually revealed, so a row's blank space cannot accept a tap before it
//! is there to be tapped (broadcasts and focus-routed events still reach every
//! child, as they do while collapsed). A settled child paints with no
//! layer/transform wrapper.
//!
//! **A child-list change mid-cascade resumes, it does not restart.** The timeline
//! is sized off the child count, so a new list has to re-arm it — but from the
//! elapsed position it already held. Each item's window is absolute on that
//! elapsed clock (item `i` opens at `i · 90ms`), so every already-revealed child
//! keeps exactly the progress it had while the longer timeline's tail carries the
//! new ones in. The cascade is paced by *index*, not identity: appending lands
//! the new rows on that tail, while inserting ahead of a revealed one hands it
//! the next index's later window and re-fades it — this container holds no child
//! keys to pace against, so a mid-cascade insert is an append or nothing.
//!
//! **Collapsing does not cascade out**: children stay fully opaque and are simply
//! swallowed by the shrinking clip, so a collapse reads as one motion rather than
//! `n` fades. [`GlyphRevealView::stagger`]`(false)` drops the cascade entirely.
//!
//! # The guide
//!
//! [`RevealGuide`] rules a dashed 1px vertical line at `indent` and insets every
//! child by `indent + pad`, over the revealed extent only. The ink is the
//! `outline` role, or `primary` on an accented guide. Like
//! [`mod@crate::empty_state`]'s dashed border it is a run of short `stroke_line`
//! segments ([`GLYPH_REVEAL_DASH_LEN`] on, [`GLYPH_REVEAL_DASH_GAP`] off) rather
//! than one dashed stroke: the run's phase is anchored to the container's top
//! edge, so the dashes stay put as the revealed extent grows instead of
//! re-flowing each frame.
//!
//! # Scheduling what happens after a reveal
//!
//! There is no `on_reveal_settled` callback: a settle is observed in `paint`,
//! which has no app-state seam to fire one through (this crate's callbacks run
//! from `event`). An app that wants to chain an auto-expansion instead schedules
//! it off the same numbers this widget uses —
//! `GlyphStagger::glyph().total_duration(n)` milliseconds for the cascade, and
//! `Theme::motion.durations.base` for the height tween; the two overlap, so the
//! whole reveal is settled by `max(base, total_duration(n))`.
//!
//! # reduce_motion
//!
//! The height snaps to its target with a single relayout, and the cascade
//! collapses to one synchronized fade — [`GlyphStagger::item_progress`]'s own
//! reduced arm, so every child tracks the shared progress with no per-item delay.

use std::time::Duration;

use frust::Theme;
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, PointerPhase, SemanticsCtx, View, Widget, build_child,
    rebuild_children, route_event, teardown_child, visit_children,
};
use frust::{AnimationController, Curve, FrameTime, Tween};
use kurbo::{Affine, Point, Size, Vec2};
use peniko::Color;

use crate::motion::GlyphStagger;

/// Default inter-child gap, in logical px — Glyph's 6px spacing step (the same
/// 6px the design system's `.dropdown{padding:6px}` panel gutter uses; the Glyph
/// token set carries no spacing scale on `Theme`, so this is a named constant).
pub const GLYPH_REVEAL_GAP: f64 = 6.0;

/// How far right of its resting spot a cascading child starts, in logical px
/// (dp). Shorter than the 16dp a whole-screen `crate::motion::GlyphSlide`
/// travels: this is an in-place list-item settle, not a page change.
pub const GLYPH_REVEAL_SLIDE_DP: f64 = 10.0;

/// Guide-line stroke width, in logical px.
const GUIDE_WIDTH: f64 = 1.0;
/// Guide dash on-length, in logical px (matching [`mod@crate::empty_state`]'s).
pub const GLYPH_REVEAL_DASH_LEN: f64 = 6.0;
/// Guide dash off-length (gap), in logical px.
pub const GLYPH_REVEAL_DASH_GAP: f64 = 4.0;

/// Reveal-animation fallback duration when no theme is threaded.
const FALLBACK_DURATION: Duration = Duration::from_millis(220);

/// The chevron's square stroke box, in logical px — sized to the Glyph
/// `caption/11` type role so the marker sits optically level with a row's
/// small text.
pub const GLYPH_CHEVRON_BOX: f64 = 11.0;
/// The chevron's arm length, in logical px — 3.5 against
/// [`mod@crate::accordion`]'s 4.5, deliberately, because the two marks are boxed
/// differently: the accordion draws its chevron free-hand at a computed center
/// in a tall padded header row, with no declared box to stay inside, while this
/// one is a laid-out leaf whose entire footprint is [`GLYPH_CHEVRON_BOX`]. All
/// reaches below are from the box center against its 5.5px half-extent. At rest
/// the mark reaches `arm + CHEVRON_WIDTH / 2` = 4.25, a 1.25px optical margin,
/// where 4.5 would reach 5.25 and leave 0.25. Rotating, the arms' outer ends
/// sweep `arm · √2`, peaking at 45°: 5.7 here — 0.2 past the box for the few
/// frames it lasts — against 7.11 for a 4.5 arm, 1.6px into whatever a row laid
/// out beside it. The two constants are a pair — move one and re-check the other.
const CHEVRON_ARM: f64 = 3.5;
/// The chevron stroke width, in logical px.
const CHEVRON_WIDTH: f64 = 1.5;

// ---- Unthemed fallback constants (Glyph **dark** values) ---------------

const GUIDE_BORDER: Color = Color::from_rgb8(0x3e, 0x3f, 0x44); // outline
const ACCENT: Color = Color::from_rgb8(0xff, 0xb6, 0x27); // THE accent
const CHEVRON_DIM: Color = Color::from_rgb8(0xa3, 0x9c, 0x88); // fg-muted

/// An implicit reveal driver: a `0..1` fraction tweened from its current value
/// to a new target whenever the target changes, over a per-flip-fresh
/// [`AnimationController`]. A degenerate zero-duration controller keeps
/// [`value`](Self::value) reading the initial fraction until the first real
/// flip, which is what makes an already-expanded first build render settled.
///
/// A private port of [`mod@crate::accordion`]'s identical driver — the two catalog
/// modules keep their own copies rather than sharing one, since neither owns a
/// motion seam the other could depend on.
struct RevealDriver {
    target: f64,
    driving_target: f64,
    tween: Tween<f64>,
    driver: AnimationController,
}

impl RevealDriver {
    fn new(initial: f64) -> Self {
        Self {
            target: initial,
            driving_target: initial,
            tween: Tween::new(initial, initial),
            driver: AnimationController::new(Duration::ZERO),
        }
    }

    fn value(&self) -> f64 {
        self.tween.lerp(self.driver.value_clamped())
    }

    fn set_target(&mut self, target: f64) {
        self.target = target;
    }

    /// Snap immediately to the target (reduce-motion path).
    fn snap(&mut self) {
        self.tween = Tween::new(self.target, self.target);
        self.driving_target = self.target;
        self.driver = AnimationController::new(Duration::ZERO);
    }

    /// Advance one frame; launches a fresh driver on a pending retarget.
    fn advance(&mut self, now: FrameTime, dur: Duration, curve: Curve) -> bool {
        if self.target != self.driving_target {
            let from = self.value();
            self.tween = Tween::new(from, self.target);
            self.driving_target = self.target;
            self.driver = AnimationController::new(dur).with_curve(curve);
            self.driver.forward();
        }
        self.driver.advance(now)
    }
}

/// The dashed indent rule a [`GlyphRevealView`] can draw down its leading edge.
///
/// `indent` is where the line sits (logical px from the container's leading
/// edge); `pad` is the extra space between the line and the children, so every
/// child is inset by `indent + pad`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RevealGuide {
    /// Distance from the container's leading edge to the rule, in logical px.
    pub indent: f64,
    /// Space between the rule and the children, in logical px.
    pub pad: f64,
    /// Draw the rule in the accent ink (`primary`) instead of `outline`.
    pub accent: bool,
}

impl RevealGuide {
    /// A guide at `indent` with `pad` before the children, in the `outline` ink.
    pub const fn new(indent: f64, pad: f64) -> Self {
        Self {
            indent,
            pad,
            accent: false,
        }
    }

    /// Draw the rule in the accent ink (the chainable form of the
    /// [`accent`](Self::accent) field).
    pub fn accented(mut self, accent: bool) -> Self {
        self.accent = accent;
        self
    }
}

/// A declarative reveal container. See the [module docs](self).
pub struct GlyphRevealView<State: 'static> {
    children: Vec<AnyView<State>>,
    expanded: bool,
    stagger: bool,
    gap: f64,
    guide: Option<RevealGuide>,
}

/// Create a reveal container over `children`. Controlled: pass the current
/// [`.expanded(bool)`](GlyphRevealView::expanded); the container animates
/// whenever that value flips after the first build.
pub fn glyph_reveal<State: 'static>(
    children: impl IntoIterator<Item = impl View<State>>,
) -> GlyphRevealView<State> {
    GlyphRevealView {
        children: children.into_iter().map(AnyView::new).collect(),
        expanded: false,
        stagger: true,
        gap: GLYPH_REVEAL_GAP,
        guide: None,
    }
}

impl<State: 'static> GlyphRevealView<State> {
    /// Set the (app-owned) expanded state. Flipping it after the first build
    /// animates; an already-`true` first build renders settled.
    pub fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = expanded;
        self
    }

    /// Cascade the children in while expanding (default `true`). `false` reveals
    /// them all at once under the growing clip.
    pub fn stagger(mut self, stagger: bool) -> Self {
        self.stagger = stagger;
        self
    }

    /// Set the inter-child gap, in logical px (default [`GLYPH_REVEAL_GAP`]).
    pub fn gap(mut self, gap: f64) -> Self {
        self.gap = gap;
        self
    }

    /// Rule a dashed guide line down the leading edge and inset the children
    /// behind it. See [`RevealGuide`].
    pub fn guide(mut self, guide: RevealGuide) -> Self {
        self.guide = Some(guide);
        self
    }
}

/// The retained widget for a [`GlyphRevealView`]. See the [module docs](self).
pub struct GlyphRevealWidget {
    pods: Vec<ChildPod>,
    expanded: bool,
    stagger: bool,
    gap: f64,
    guide: Option<RevealGuide>,
    /// The height tween.
    reveal: RevealDriver,
    /// The latest reveal fraction, updated in paint and read by layout.
    reveal_value: f64,
    /// The children's full natural extent, including gaps (set in layout).
    content_height: f64,
    /// The cascade's shared `0..1` timeline, sized to
    /// `GlyphStagger::total_duration(n)` and armed once per expansion (re-armed,
    /// from where it stood, by a mid-cascade child-list change).
    cascade: AnimationController,
    /// Where on the shared timeline [`Self::cascade`] was armed: `0.0` for a
    /// fresh expansion, or the already-elapsed fraction a re-arm resumed from.
    /// The controller itself only spans what was left, so the timeline position
    /// is the two composed (see [`GlyphRevealWidget::arm_cascade`]).
    cascade_from: f64,
    /// The cascade's latest shared progress.
    cascade_value: f64,
    /// Whether the cascade controller is still running.
    cascading: bool,
    /// How many leading children the last paint revealed — the ones pointer
    /// input may reach while [`Self::cascading`] (`item_progress` is
    /// non-increasing in `i`, so the revealed set is always a prefix).
    /// `usize::MAX` until a paint records one, which reads as "no gate".
    /// Consulted only through [`GlyphRevealWidget::input_reach`], the one
    /// spot `event` and `semantics` both read it from.
    cascade_frontier: usize,
}

impl<State: 'static> View<State> for GlyphRevealView<State> {
    type Element = GlyphRevealWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> GlyphRevealWidget {
        // Settled on first build: an already-expanded node starts fully
        // revealed and fully cascaded, with no driver running (see the module
        // docs).
        let settled = if self.expanded { 1.0 } else { 0.0 };
        GlyphRevealWidget {
            pods: self
                .children
                .iter()
                .map(|child| build_child(child, ctx))
                .collect(),
            expanded: self.expanded,
            stagger: self.stagger,
            gap: self.gap,
            guide: self.guide,
            reveal: RevealDriver::new(settled),
            reveal_value: settled,
            content_height: 0.0,
            cascade: AnimationController::new(Duration::ZERO),
            cascade_from: settled,
            cascade_value: settled,
            cascading: false,
            cascade_frontier: usize::MAX,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut GlyphRevealWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let before = element.pods.len();
        let mut flags = rebuild_children(
            &prev.children,
            &self.children,
            &mut element.pods,
            ctx,
            |view| view,
            |_| None,
        );
        if element.expanded != self.expanded {
            element.expanded = self.expanded;
            element
                .reveal
                .set_target(if self.expanded { 1.0 } else { 0.0 });
            if self.expanded {
                element.arm_cascade(0.0);
            } else {
                // Collapsing never cascades out: the children stay opaque under
                // the shrinking clip.
                element.settle_cascade();
            }
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else if element.cascading && element.pods.len() != before {
            // The timeline is sized off the child count, so a mid-cascade list
            // change re-arms it rather than pacing against a stale length — but
            // from where the cascade already stood, never from zero. Re-arming
            // at zero paints no children at all on the next pass and then
            // re-cascades rows that were already in; resuming on the elapsed
            // clock leaves every revealed child exactly as revealed as it was
            // (see the module docs).
            let elapsed = element.cascade_elapsed_ms(before);
            element.arm_cascade(elapsed);
        }
        if element.stagger != self.stagger {
            element.stagger = self.stagger;
            flags |= ChangeFlags::PAINT;
        }
        if element.gap != self.gap || element.guide != self.guide {
            element.gap = self.gap;
            element.guide = self.guide;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut GlyphRevealWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.children.iter().zip(element.pods.iter_mut()) {
            teardown_child(view, pod, ctx);
        }
    }
}

impl GlyphRevealWidget {
    /// Arm the cascade over the current child count with `elapsed_ms` of its
    /// timeline already behind it — `0.0` starts from nothing (a fresh
    /// expansion), anything else resumes an in-flight cascade over a new child
    /// count.
    ///
    /// The controller spans only what is *left* (`total - elapsed`) and
    /// [`Self::cascade_from`] carries the rest, which keeps the composed value
    /// advancing at exactly one timeline-millisecond per millisecond either way.
    fn arm_cascade(&mut self, elapsed_ms: f64) {
        let total_ms = GlyphStagger::glyph().total_duration(self.pods.len());
        let elapsed = elapsed_ms.clamp(0.0, total_ms);
        let remaining = (total_ms - elapsed).max(0.0);
        let mut c =
            AnimationController::new(Duration::from_millis(remaining.round().max(0.0) as u64))
                .with_curve(Curve::Linear);
        c.forward();
        self.cascade = c;
        self.cascade_from = if total_ms > 0.0 {
            elapsed / total_ms
        } else {
            1.0
        };
        self.cascade_value = self.cascade_from;
        self.cascading = true;
    }

    /// Where the cascade currently stands in milliseconds, read against a
    /// timeline sized for `n` children — the position [`Self::arm_cascade`]
    /// resumes from when the child list changes under it.
    fn cascade_elapsed_ms(&self, n: usize) -> f64 {
        self.cascade_value * GlyphStagger::glyph().total_duration(n)
    }

    /// Drop the cascade and leave every child fully revealed.
    fn settle_cascade(&mut self) {
        self.cascade = AnimationController::new(Duration::ZERO);
        self.cascade_from = 1.0;
        self.cascade_value = 1.0;
        self.cascading = false;
    }

    /// How far in the children sit: the guide's `indent + pad`, or nothing.
    fn inset(&self) -> f64 {
        self.guide.map_or(0.0, |g| g.indent + g.pad)
    }

    /// The number of leading children pointer/scroll input can currently
    /// reach — and, by the input-parity carve-out `docs/CODE_STANDARDS.md`'s
    /// Semantics Conventions grants a container that gates input, exactly how
    /// many [`semantics`](Widget::semantics) below may forward. This is the
    /// one shared reach both methods read, so the two cannot drift (the same
    /// shape the navigator's R23 `input_routed_pages` takes): an
    /// accesskit-synthesized `Click` (`RenderRoot::perform_accessibility_action`)
    /// routes through this same gated `event`, so a child beyond this reach
    /// could not be activated by one either — exposing it as an actionable
    /// semantics node would read as a silent no-op.
    ///
    /// While [`Self::cascading`], the reach is the frontier the last paint
    /// revealed ([`Self::cascade_frontier`]); settled, collapsed, or
    /// `stagger(false)` reach every child — `event`'s own
    /// broadcast/focus-routed/already-active-pod escape hatches (none of
    /// which have a semantics analogue, so they stay local to `event`) layer
    /// on top of this, never narrow it.
    fn input_reach(&self) -> usize {
        if self.cascading {
            self.cascade_frontier.min(self.pods.len())
        } else {
            self.pods.len()
        }
    }
}

/// Resolve `(guide_border, accent)` inks.
fn resolve_inks(theme: Option<&Theme>) -> (Color, Color) {
    match theme {
        Some(theme) => {
            let s = theme.scheme();
            (s.outline, s.primary)
        }
        None => (GUIDE_BORDER, ACCENT),
    }
}

/// Resolve the reveal `(duration, curve)` — the theme's "220ms spatial" timing.
fn resolve_timing(theme: Option<&Theme>) -> (Duration, Curve) {
    theme
        .map(|t| {
            (
                Duration::from_secs_f64(t.motion.durations.base / 1000.0),
                t.motion.easing.spatial,
            )
        })
        .unwrap_or((FALLBACK_DURATION, Curve::EaseInOut))
}

/// Stroke a dashed vertical rule at `x` from `y0` down to `y1` (see the module
/// docs' guide section).
fn stroke_dashed_vline(scene: &mut dyn PaintScene, x: f64, y0: f64, y1: f64, color: Color) {
    let step = GLYPH_REVEAL_DASH_LEN + GLYPH_REVEAL_DASH_GAP;
    let mut y = y0;
    while y < y1 {
        let end = (y + GLYPH_REVEAL_DASH_LEN).min(y1);
        scene.stroke_line(Point::new(x, y), Point::new(x, end), GUIDE_WIDTH, color);
        y += step;
    }
}

impl Widget for GlyphRevealWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let bounded = bc.max().width.is_finite();
        let inset = self.inset();
        let child_max_w = if bounded {
            (bc.max().width - inset).max(0.0)
        } else {
            f64::INFINITY
        };
        let child_bc = BoxConstraints::new(Size::ZERO, Size::new(child_max_w, f64::INFINITY));

        // Children are always laid out at their full natural height (see the
        // module docs) — the reveal only scales what this widget reports.
        let mut y = 0.0;
        let mut natural_w: f64 = 0.0;
        for (i, pod) in self.pods.iter_mut().enumerate() {
            if i > 0 {
                y += self.gap;
            }
            let size = pod.layout_child(ctx, &child_bc);
            pod.set_origin(Point::new(inset, y));
            y += size.height;
            natural_w = natural_w.max(size.width);
        }
        self.content_height = y;

        let width = if bounded {
            bc.max().width
        } else {
            natural_w + inset
        };
        bc.constrain(Size::new(width, self.content_height * self.reveal_value))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Every theme read happens up front: the `&Theme` borrows the context,
        // and the relayout requests plus the child paints below need it mutably.
        let theme = Theme::from_paint_ctx(ctx);
        let reduce_motion = theme.map(|t| t.motion.reduce_motion).unwrap_or(false);
        let (dur, curve) = resolve_timing(theme);
        let (border, accent) = resolve_inks(theme);

        // Advance the height tween. The revealed height is computed in `layout`
        // from `reveal_value`, so an animating pass must request a *relayout*,
        // and the before/after comparison covers both the driver's landing frame
        // and the reduce-motion snap (see the module docs).
        let before = self.reveal_value;
        if reduce_motion {
            self.reveal.snap();
        } else if self.reveal.advance(ctx.frame_time(), dur, curve) {
            ctx.request_layout();
        }
        self.reveal_value = self.reveal.value().clamp(0.0, 1.0);
        if self.reveal_value != before {
            ctx.request_layout();
        }

        // Advance the cascade — a decoupled, paint-only timeline. The
        // controller spans what was left when it was armed, so the shared
        // progress is that reading composed with where it started.
        if self.cascading {
            if self.cascade.advance(ctx.frame_time()) {
                ctx.request_frame();
            } else {
                self.cascading = false;
            }
            self.cascade_value =
                self.cascade_from + (1.0 - self.cascade_from) * self.cascade.value_clamped();
        }

        let revealed = self.reveal_value * self.content_height;
        if revealed <= 0.0 {
            return;
        }

        let o = ctx.origin();
        let size = ctx.size();
        scene.push_clip(o, Size::new(size.width, revealed));

        if let Some(guide) = self.guide {
            let ink = if guide.accent { accent } else { border };
            stroke_dashed_vline(scene, o.x + guide.indent, o.y, o.y + revealed, ink);
        }

        // Cascade only while expanding: a collapse keeps every child opaque and
        // lets the shrinking clip swallow them.
        let cascading_in = self.stagger && self.expanded;
        let spec = GlyphStagger::glyph();
        let n = self.pods.len();
        // How far down the list the cascade has actually revealed — recorded for
        // `event`, so hit-testing can't reach a child that painted nothing.
        let mut frontier = 0usize;
        for (i, pod) in self.pods.iter_mut().enumerate() {
            let progress = if cascading_in {
                spec.item_progress(self.cascade_value, i, n, reduce_motion)
            } else {
                1.0
            };
            if progress <= 0.0 {
                continue;
            }
            frontier = i + 1;
            let dx = GLYPH_REVEAL_SLIDE_DP * (1.0 - progress);
            let settled = progress >= 1.0;
            let child_origin = o + pod.origin().to_vec2() + Vec2::new(dx, 0.0);
            if !settled {
                scene.push_layer(child_origin, pod.size(), progress as f32);
                scene.push_transform(Affine::translate((dx, 0.0)));
            }
            pod.paint_child(ctx, scene);
            if !settled {
                scene.pop_transform();
                scene.pop_layer();
            }
        }
        // Off the cascade every child is revealed, so the frontier gates
        // nothing; a collapsed container returns above and keeps the last one.
        self.cascade_frontier = frontier;

        scene.pop_clip();
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // Only the cascade window gates anything. Off it — settled, collapsed,
        // or `stagger(false)` — every child routes exactly as `route_event`
        // alone would; so do broadcasts, focus-routed events (both reach a
        // collapsed subtree already, matching `semantics` below), and a child
        // already holding the pointer capture, which was visible when it
        // claimed and must not lose the rest of its gesture.
        let frontier = self.input_reach();
        if frontier == self.pods.len()
            || event.is_broadcast()
            || event.is_focus_routed()
            || self.pods.iter().any(|p| p.is_active())
        {
            return route_event(&mut self.pods, ctx, event);
        }
        // Pointer/scroll input reaches only what the last paint revealed: a
        // child whose cascade window has not opened paints nothing, so routing
        // a tap into its rect would have blank space accept it.
        let result = route_event(&mut self.pods[..frontier], ctx, event);
        // Blur-on-outside-tap still covers the whole container, not just the
        // routed prefix: a `Down` that re-claimed nothing must drop a hidden
        // child's stale focus link exactly as ungated routing would (mirroring
        // `route_event`'s own rule over the children it never saw).
        if matches!(event, InputEvent::Pointer(p) if matches!(p.phase, PointerPhase::Down)) {
            for pod in &mut self.pods[frontier..] {
                if pod.is_focused() {
                    pod.set_focused(false);
                }
            }
        }
        result
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // Mirrors `event`'s own cascade gate through the shared
        // `input_reach` — the one carve-out `docs/CODE_STANDARDS.md`'s
        // Semantics Conventions allows a container that gates input: while
        // cascading, a child beyond the frontier cannot be tapped, and an AT
        // client's synthesized `Click` can't reach it either
        // (`RenderRoot::perform_accessibility_action` routes it through this
        // same gated `event`), so forwarding it here would expose a silent
        // no-op as actionable. Settled, collapsed, or `stagger(false)`
        // containers reach — and forward — every child: the silent-drop
        // rule (hidden state is the platform's concern) still covers that
        // case, unlike the mid-cascade one this fixes.
        for pod in &self.pods[..self.input_reach()] {
            pod.semantics_child(ctx);
        }
    }

    visit_children!(pods);
}

// ---- Chevron ------------------------------------------------------------

/// A declarative rotating chevron. See [`glyph_chevron`].
pub struct GlyphChevronView {
    expanded: bool,
}

/// Create a disclosure chevron: points right (`>`) while collapsed, rotates 90°
/// to point down as `expanded` flips, on the same 220ms spatial timing the
/// reveal container's height uses. Dim ink collapsed, accent ink expanded, with
/// the tint riding the same fraction as the rotation.
///
/// Like [`glyph_reveal`] it is controlled and settles on its first build: a
/// chevron built `expanded` starts pointing down rather than spinning into it.
pub fn glyph_chevron(expanded: bool) -> GlyphChevronView {
    GlyphChevronView { expanded }
}

/// The retained widget for a [`GlyphChevronView`].
pub struct GlyphChevronWidget {
    reveal: RevealDriver,
    /// The latest rotation fraction (`0` = right, `1` = down).
    reveal_value: f64,
}

impl<State: 'static> View<State> for GlyphChevronView {
    type Element = GlyphChevronWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> GlyphChevronWidget {
        let settled = if self.expanded { 1.0 } else { 0.0 };
        GlyphChevronWidget {
            reveal: RevealDriver::new(settled),
            reveal_value: settled,
        }
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut GlyphChevronWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let target = if self.expanded { 1.0 } else { 0.0 };
        if element.reveal.target != target {
            element.reveal.set_target(target);
            return ChangeFlags::PAINT;
        }
        ChangeFlags::NONE
    }
}

impl Widget for GlyphChevronWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(GLYPH_CHEVRON_BOX, GLYPH_CHEVRON_BOX))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let reduce_motion = theme.map(|t| t.motion.reduce_motion).unwrap_or(false);
        let (dur, curve) = resolve_timing(theme);
        let (dim, accent) = match theme {
            Some(theme) => {
                let s = theme.scheme();
                (s.on_surface_variant, s.primary)
            }
            None => (CHEVRON_DIM, ACCENT),
        };

        // A chevron is fixed-size, so its rotation is paint-only: an animating
        // pass asks for another frame, never a relayout.
        if reduce_motion {
            self.reveal.snap();
        } else if self.reveal.advance(ctx.frame_time(), dur, curve) {
            ctx.request_frame();
        }
        self.reveal_value = self.reveal.value().clamp(0.0, 1.0);

        let size = ctx.size();
        let center = Point::new(
            ctx.origin().x + size.width / 2.0,
            ctx.origin().y + size.height / 2.0,
        );
        let angle = self.reveal_value * std::f64::consts::FRAC_PI_2;
        draw_chevron(
            scene,
            center,
            angle,
            mix_ink(dim, accent, self.reveal_value),
        );
    }
}

/// Draw a chevron centered at `center`, rotated `angle` radians clockwise (0 =
/// pointing right `>`, `FRAC_PI_2` = pointing down `v`).
fn draw_chevron(scene: &mut dyn PaintScene, center: Point, angle: f64, color: Color) {
    let (s, c) = angle.sin_cos();
    let rot = |dx: f64, dy: f64| Point::new(center.x + dx * c - dy * s, center.y + dx * s + dy * c);
    let top = rot(-CHEVRON_ARM, -CHEVRON_ARM);
    let tip = rot(CHEVRON_ARM, 0.0);
    let bottom = rot(-CHEVRON_ARM, CHEVRON_ARM);
    scene.stroke_line(top, tip, CHEVRON_WIDTH, color);
    scene.stroke_line(tip, bottom, CHEVRON_WIDTH, color);
}

/// Blend `from` toward `to` by `t`, component-wise in the colors' own space (a
/// plain sRGB mix — adequate for a small monochrome marker's tint, not a
/// perceptual interpolation).
///
/// Both endpoints are returned verbatim rather than computed, so a settled
/// chevron paints the resolved role's exact color instead of a float-rounded
/// neighbour of it.
fn mix_ink(from: Color, to: Color, t: f64) -> Color {
    let t = t.clamp(0.0, 1.0) as f32;
    if t <= 0.0 {
        return from;
    }
    if t >= 1.0 {
        return to;
    }
    let (a, b) = (from.components, to.components);
    Color::new([
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3] + (b[3] - a[3]) * t,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::{PointerButton, PointerEvent, Role, any};
    use std::any::Any;

    const WINDOW: Size = Size::new(300.0, 400.0);
    /// The three test children's heights; with the default 6px gap the natural
    /// extent is `20 + 6 + 30 + 6 + 40`.
    const HEIGHTS: [f64; 3] = [20.0, 30.0, 40.0];
    const NATURAL: f64 = 20.0 + GLYPH_REVEAL_GAP + 30.0 + GLYPH_REVEAL_GAP + 40.0;

    #[derive(Debug, Clone, PartialEq)]
    enum Op {
        Clip(Point, Size),
        PopClip,
        Layer(Point, Size, f32),
        PopLayer,
        Transform(Affine),
        PopTransform,
        Rect(Point, Size),
        Line(Point, Point, Color),
    }

    #[derive(Default)]
    struct Recorder {
        ops: Vec<Op>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, origin: Point, size: Size, _color: Color) {
            self.ops.push(Op::Rect(origin, size));
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn push_clip(&mut self, origin: Point, size: Size) {
            self.ops.push(Op::Clip(origin, size));
        }
        fn pop_clip(&mut self) {
            self.ops.push(Op::PopClip);
        }
        fn push_layer(&mut self, origin: Point, size: Size, alpha: f32) {
            self.ops.push(Op::Layer(origin, size, alpha));
        }
        fn pop_layer(&mut self) {
            self.ops.push(Op::PopLayer);
        }
        fn push_transform(&mut self, transform: Affine) {
            self.ops.push(Op::Transform(transform));
        }
        fn pop_transform(&mut self) {
            self.ops.push(Op::PopTransform);
        }
        fn stroke_line(&mut self, p0: Point, p1: Point, _width: f64, color: Color) {
            self.ops.push(Op::Line(p0, p1, color));
        }
    }

    impl Recorder {
        fn clips(&self) -> Vec<(Point, Size)> {
            self.ops
                .iter()
                .filter_map(|op| match op {
                    Op::Clip(o, s) => Some((*o, *s)),
                    _ => None,
                })
                .collect()
        }

        fn layer_alphas(&self) -> Vec<f32> {
            self.ops
                .iter()
                .filter_map(|op| match op {
                    Op::Layer(_, _, a) => Some(*a),
                    _ => None,
                })
                .collect()
        }

        fn translations(&self) -> Vec<f64> {
            self.ops
                .iter()
                .filter_map(|op| match op {
                    Op::Transform(t) => Some(t.translation().x),
                    _ => None,
                })
                .collect()
        }

        fn child_rects(&self) -> Vec<(Point, Size)> {
            self.ops
                .iter()
                .filter_map(|op| match op {
                    Op::Rect(o, s) => Some((*o, *s)),
                    _ => None,
                })
                .collect()
        }

        fn lines(&self) -> Vec<(Point, Point, Color)> {
            self.ops
                .iter()
                .filter_map(|op| match op {
                    Op::Line(a, b, c) => Some((*a, *b, *c)),
                    _ => None,
                })
                .collect()
        }

        /// Each painted child's reveal progress, in child order: the alpha of the
        /// fade layer wrapping it, or `1.0` for a settled child that painted
        /// bare. A child whose cascade window has not opened paints nothing and
        /// is simply absent, so the vec's length is the revealed frontier.
        fn child_progress(&self) -> Vec<f64> {
            let mut out = Vec::new();
            let mut wrapping = None;
            for op in &self.ops {
                match op {
                    Op::Layer(_, _, alpha) => wrapping = Some(*alpha),
                    Op::Rect(..) => out.push(wrapping.take().map_or(1.0, f64::from)),
                    _ => {}
                }
            }
            out
        }
    }

    /// A fixed-size leaf that paints one rect — a deterministic stand-in for a
    /// real child view.
    struct Block(Size);

    /// The retained half of [`Block`].
    struct BlockWidget(Size);

    impl<S: 'static> View<S> for Block {
        type Element = BlockWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> BlockWidget {
            BlockWidget(self.0)
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut BlockWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            element.0 = self.0;
            ChangeFlags::NONE
        }
    }

    impl Widget for BlockWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(self.0)
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            scene.fill_rect(ctx.origin(), ctx.size(), Color::BLACK);
        }
        /// Consumes any pointer event that reaches it, the way a real row's
        /// press target would — so a container test can read "did this child get
        /// the tap?" straight off the routed result.
        fn event(&mut self, _ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            match event {
                InputEvent::Pointer(_) => EventResult::Handled,
                _ => EventResult::Ignored,
            }
        }
        /// One node per forwarded child, so a semantics-parity test can count
        /// exactly how many of `GlyphRevealWidget`'s children `semantics`
        /// actually reached.
        fn semantics(&self, ctx: &mut SemanticsCtx) {
            ctx.push_node(Role::GenericContainer, |_| {});
        }
    }

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn view_of(expanded: bool, heights: &[f64]) -> GlyphRevealView<()> {
        glyph_reveal(heights.iter().map(|h| any(Block(Size::new(120.0, *h))))).expanded(expanded)
    }

    fn view(expanded: bool) -> GlyphRevealView<()> {
        view_of(expanded, &HEIGHTS)
    }

    /// Dispatch a pointer `Down` at `at` (container-local) and report whether the
    /// container routed it to a child.
    fn tap(w: &mut GlyphRevealWidget, at: Point) -> EventResult {
        let mut state = ();
        let sa: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(sa, Point::ZERO, WINDOW);
        w.event(
            &mut ctx,
            &InputEvent::Pointer(PointerEvent {
                phase: PointerPhase::Down,
                position: at,
                button: PointerButton::Primary,
            }),
        )
    }

    fn build(v: &GlyphRevealView<()>) -> GlyphRevealWidget {
        let mut counter = 0u64;
        View::<()>::build(v, &mut BuildCtx::new(&mut counter))
    }

    fn rebuild(prev: &GlyphRevealView<()>, next: &GlyphRevealView<()>, w: &mut GlyphRevealWidget) {
        let mut counter = 0u64;
        <GlyphRevealView<()> as View<()>>::rebuild(next, prev, w, &mut BuildCtx::new(&mut counter));
    }

    fn layout(w: &mut GlyphRevealWidget) -> Size {
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::loose(WINDOW))
    }

    /// Paint at `ms` against `size`, returning the recorded scene and whether the
    /// pass asked for a relayout.
    fn paint(
        w: &mut GlyphRevealWidget,
        size: Size,
        ms: f64,
        theme: Option<&Theme>,
    ) -> (Recorder, bool) {
        let mut rec = Recorder::default();
        let mut ctx = match theme {
            Some(t) => PaintCtx::for_test(Point::ZERO, size, ft_ms(ms)).with_theme(t as &dyn Any),
            None => PaintCtx::for_test(Point::ZERO, size, ft_ms(ms)),
        };
        w.paint(&mut ctx, &mut rec);
        let needs_layout = ctx.needs_layout();
        (rec, needs_layout)
    }

    /// Flip a collapsed widget open and run one frame per 16ms until the height
    /// tween stops asking for relayouts, re-running layout each time exactly as a
    /// shell would. Returns the last reported size.
    fn drive_open(w: &mut GlyphRevealWidget, theme: Option<&Theme>) -> Size {
        let mut size = layout(w);
        let mut t = 0.0;
        loop {
            let (_, needs_layout) = paint(w, size, t, theme);
            if !needs_layout {
                break;
            }
            size = layout(w);
            t += 16.0;
            assert!(t < 2000.0, "the reveal should settle well under 2s");
        }
        size
    }

    // ---- Height reveal ---------------------------------------------------

    #[test]
    fn an_already_expanded_first_build_renders_settled_with_no_animation() {
        let mut w = build(&view(true));
        let size = layout(&mut w);
        assert_eq!(
            size.height, NATURAL,
            "full natural extent on the first pass"
        );
        let (rec, needs_layout) = paint(&mut w, size, 0.0, None);
        assert!(
            !needs_layout,
            "a settled first build never asks for another layout"
        );
        assert_eq!(rec.clips(), vec![(Point::ZERO, Size::new(300.0, NATURAL))]);
        assert!(
            rec.layer_alphas().is_empty(),
            "settled children paint with no layer at all"
        );
        assert!(rec.translations().is_empty(), "and no slide transform");
        assert_eq!(rec.child_rects().len(), 3, "every child painted");
    }

    #[test]
    fn a_collapsed_container_reports_no_height_and_paints_nothing() {
        let mut w = build(&view(false));
        let size = layout(&mut w);
        assert_eq!(size.height, 0.0);
        assert!(
            w.content_height > 0.0,
            "children are still measured at full height"
        );
        let (rec, needs_layout) = paint(&mut w, size, 0.0, None);
        assert!(!needs_layout);
        assert!(rec.ops.is_empty(), "nothing at all is painted");
    }

    #[test]
    fn expanding_animates_the_height_and_asks_for_a_relayout_each_frame() {
        let (closed, open) = (view(false), view(true));
        let mut w = build(&closed);
        layout(&mut w);
        rebuild(&closed, &open, &mut w);

        let mut size = layout(&mut w);
        let (_, seed) = paint(&mut w, size, 0.0, None);
        assert!(seed, "the seeding frame asks for a relayout");
        size = layout(&mut w);

        // Mid-flight: strictly between nothing and the natural extent.
        let (rec, animating) = paint(&mut w, size, 60.0, None);
        assert!(animating, "still animating at 60ms of a 220ms tween");
        let revealed = rec.clips()[0].1.height;
        assert!(
            revealed > 0.0 && revealed < NATURAL,
            "mid-reveal height: {revealed}"
        );

        // Settling lands exactly on the target, and stops asking.
        let settled = drive_open(&mut w, None);
        assert_eq!(settled.height, NATURAL);
        let (rec, needs_layout) = paint(&mut w, settled, 1000.0, None);
        assert!(!needs_layout, "settled: no further layout requests");
        assert_eq!(rec.clips()[0].1.height, NATURAL);
    }

    #[test]
    fn collapsing_reverses_the_tween_and_lands_on_zero() {
        let (closed, open) = (view(false), view(true));
        let mut w = build(&open);
        let size = layout(&mut w);
        assert_eq!(size.height, NATURAL);

        rebuild(&open, &closed, &mut w);
        let mut size = layout(&mut w);
        let (_, seed) = paint(&mut w, size, 0.0, None);
        assert!(seed);
        size = layout(&mut w);
        let (rec, animating) = paint(&mut w, size, 60.0, None);
        assert!(animating);
        let revealed = rec.clips()[0].1.height;
        assert!(
            revealed > 0.0 && revealed < NATURAL,
            "mid-collapse height: {revealed}"
        );
        // Collapsing children stay opaque under the shrinking clip.
        assert!(rec.layer_alphas().is_empty());

        let mut t = 76.0;
        loop {
            let (_, needs_layout) = paint(&mut w, size, t, None);
            size = layout(&mut w);
            if !needs_layout {
                break;
            }
            t += 16.0;
            assert!(t < 2000.0);
        }
        assert_eq!(size.height, 0.0, "a collapse lands exactly on zero");
    }

    #[test]
    fn nothing_paints_below_the_revealed_height_mid_animation() {
        let (closed, open) = (view(false), view(true));
        let mut w = build(&closed);
        layout(&mut w);
        rebuild(&closed, &open, &mut w);
        let mut size = layout(&mut w);
        paint(&mut w, size, 0.0, None);
        size = layout(&mut w);
        let (rec, _) = paint(&mut w, size, 100.0, None);

        // The clip is pushed before anything else and popped last, and its band
        // is exactly the revealed extent.
        assert!(matches!(rec.ops.first(), Some(Op::Clip(..))));
        assert_eq!(rec.ops.last(), Some(&Op::PopClip));
        let revealed = rec.clips()[0].1.height;
        assert!(revealed < NATURAL);
        // Every child that painted at all started inside the band.
        for (origin, _) in rec.child_rects() {
            assert!(
                origin.y < revealed,
                "child origin {origin:?} inside the band"
            );
        }
    }

    // ---- Cascade ---------------------------------------------------------

    #[test]
    fn children_cascade_in_ninety_milliseconds_apart() {
        let spec = GlyphStagger::glyph();
        // 3 items: 2 * 90ms of delay plus one 150ms reveal.
        assert_eq!(spec.total_duration(3), 330.0);

        let (closed, open) = (view(false), view(true));
        let mut w = build(&closed);
        layout(&mut w);
        rebuild(&closed, &open, &mut w);
        let mut size = layout(&mut w);
        paint(&mut w, size, 0.0, None); // seeds both clocks
        size = layout(&mut w);

        // Just before the second child's 90ms window opens, only the first is in.
        let (rec, _) = paint(&mut w, size, 85.0, None);
        assert_eq!(rec.child_rects().len(), 1, "only child 0 has opened");
        // Just after, the second is in too — and behind the first.
        let (rec, _) = paint(&mut w, size, 95.0, None);
        let alphas = rec.layer_alphas();
        assert_eq!(rec.child_rects().len(), 2, "child 1 has now opened");
        assert!(
            alphas[0] > alphas[1] && alphas[1] > 0.0,
            "the cascade trails: {alphas:?}"
        );
        // The third child's window (180ms) has not opened, so it paints nothing.
        assert_eq!(alphas.len(), 2);

        // Each unsettled child slides in from the right, shrinking as it reveals.
        let dx = rec.translations();
        assert_eq!(dx.len(), 2);
        // The recorded alpha is the f32 the layer carries, so the slide it
        // implies matches within f32 precision, not exactly.
        assert!((dx[0] - GLYPH_REVEAL_SLIDE_DP * (1.0 - alphas[0] as f64)).abs() < 1e-5);
        assert!(dx[0] > 0.0 && dx[0] < dx[1] && dx[1] <= GLYPH_REVEAL_SLIDE_DP);

        // Past the whole timeline every child is settled: no layers, no slide.
        let (rec, _) = paint(&mut w, size, 400.0, None);
        assert_eq!(rec.child_rects().len(), 3);
        assert!(rec.layer_alphas().is_empty());
        assert!(rec.translations().is_empty());
    }

    #[test]
    fn stagger_off_reveals_every_child_at_once() {
        let closed = view(false).stagger(false);
        let open = view(true).stagger(false);
        let mut w = build(&closed);
        layout(&mut w);
        rebuild(&closed, &open, &mut w);
        let mut size = layout(&mut w);
        paint(&mut w, size, 0.0, None);
        size = layout(&mut w);
        let (rec, _) = paint(&mut w, size, 16.0, None);
        assert_eq!(rec.child_rects().len(), 3, "all three from the first frame");
        assert!(rec.layer_alphas().is_empty(), "no per-child fade");
    }

    #[test]
    fn reduce_motion_snaps_the_height_and_reveals_every_child_together() {
        let mut theme = crate::baseline();
        theme.motion.reduce_motion = true;
        let (closed, open) = (view(false), view(true));
        let mut w = build(&closed);
        layout(&mut w);
        rebuild(&closed, &open, &mut w);
        let size = layout(&mut w);

        // The snap moves a layout-relevant value, so it must request exactly one
        // relayout — and the relayout a shell then runs is already at target.
        let (_, needs_layout) = paint(&mut w, size, 0.0, Some(&theme));
        assert_eq!(w.reveal_value, 1.0, "the height snapped, it did not tween");
        assert!(needs_layout);
        let snapped = layout(&mut w);
        assert_eq!(snapped.height, NATURAL);
        let (rec, needs_layout) = paint(&mut w, snapped, 16.0, Some(&theme));
        assert!(!needs_layout, "a snap asks for exactly one relayout");
        assert_eq!(rec.clips()[0].1.height, NATURAL);

        // The cascade collapses: every child shares one progress, no per-item
        // delay, so all three are on screen together at the same alpha.
        let (rec, _) = paint(&mut w, snapped, 100.0, Some(&theme));
        let alphas = rec.layer_alphas();
        assert_eq!(alphas.len(), 3, "every child fades from the same frame");
        assert!(
            alphas.iter().all(|a| (a - alphas[0]).abs() < 1e-6),
            "simultaneous, not staggered: {alphas:?}"
        );
    }

    #[test]
    fn a_mid_cascade_child_arrival_resumes_the_timeline_instead_of_restarting_it() {
        let (closed, three) = (view(false), view(true));
        let mut w = build(&closed);
        layout(&mut w);
        rebuild(&closed, &three, &mut w);
        let mut size = layout(&mut w);
        paint(&mut w, size, 0.0, None); // seeds both clocks
        size = layout(&mut w);

        // Half way down the three-child, 330ms timeline: child 0 is fully in,
        // child 1 is fading, child 2's 180ms window has not opened.
        let (rec, _) = paint(&mut w, size, 165.0, None);
        let before = rec.child_progress();
        assert_eq!(before.len(), 2, "child 2 is not in yet: {before:?}");
        assert_eq!(before[0], 1.0, "child 0 has settled: {before:?}");
        assert!(before[1] > 0.0 && before[1] < 1.0, "mid-fade: {before:?}");

        // A fourth child arrives mid-cascade, lengthening the timeline to 420ms.
        let four = view_of(true, &[20.0, 30.0, 40.0, 50.0]);
        rebuild(&three, &four, &mut w);
        size = layout(&mut w);

        // The very next frame paints exactly what the last one did: the arrival
        // must not blank the container for a frame, nor re-fade rows already in.
        let (rec, _) = paint(&mut w, size, 165.0, None);
        let resumed = rec.child_progress();
        assert_eq!(
            resumed.len(),
            before.len(),
            "no blank frame, no extra reveal: {before:?} -> {resumed:?}"
        );
        for (i, (now, was)) in resumed.iter().zip(&before).enumerate() {
            assert!(
                now >= was,
                "child {i} went backwards: {before:?} -> {resumed:?}"
            );
        }

        // And the clock carries on at one timeline-ms per ms on the new length:
        // child 2's 180ms window opens, child 3's 270ms one has not.
        let (rec, _) = paint(&mut w, size, 250.0, None);
        let mid = rec.child_progress();
        assert_eq!(mid.len(), 3, "child 2 opened, child 3 has not: {mid:?}");
        for (i, (now, was)) in mid.iter().zip(&resumed).enumerate() {
            assert!(
                now >= was,
                "child {i} went backwards: {resumed:?} -> {mid:?}"
            );
        }

        // The newcomer joins the tail of the cascade rather than snapping in.
        let (rec, _) = paint(&mut w, size, 300.0, None);
        let tail = rec.child_progress();
        assert_eq!(tail.len(), 4, "child 3 has opened: {tail:?}");
        assert!(tail[3] > 0.0 && tail[3] < 1.0, "and it fades: {tail:?}");

        // Past the four-child timeline every row is settled.
        let (rec, _) = paint(&mut w, size, 500.0, None);
        assert_eq!(rec.child_progress(), vec![1.0; 4]);
    }

    #[test]
    fn a_child_the_cascade_has_not_revealed_yet_is_not_hit_testable() {
        let (closed, open) = (view(false), view(true));
        let mut w = build(&closed);
        layout(&mut w);
        rebuild(&closed, &open, &mut w);
        let mut size = layout(&mut w);
        paint(&mut w, size, 0.0, None);
        size = layout(&mut w);

        // Child 2 sits at y 62..102 and its 180ms window has not opened at 165ms.
        let (rec, _) = paint(&mut w, size, 165.0, None);
        assert_eq!(rec.child_progress().len(), 2, "child 2 painted nothing");
        let in_child_2 = Point::new(60.0, 80.0);
        assert_eq!(
            tap(&mut w, in_child_2),
            EventResult::Ignored,
            "blank space must not accept a tap before the row is there"
        );
        // The rows the cascade *has* revealed take theirs normally.
        assert_eq!(tap(&mut w, Point::new(60.0, 10.0)), EventResult::Handled);

        // Once the cascade passes its window, the same tap lands.
        let (rec, _) = paint(&mut w, size, 400.0, None);
        assert_eq!(rec.child_progress(), vec![1.0; 3], "all three settled");
        assert_eq!(tap(&mut w, in_child_2), EventResult::Handled);
    }

    #[test]
    fn a_collapsed_container_still_routes_to_its_children() {
        // The cascade window is the only thing gated: collapsed routing keeps
        // the silent-drop rule `semantics` follows, and `stagger(false)` never
        // hides a row from input at all.
        let mut w = build(&view(false));
        layout(&mut w);
        assert_eq!(tap(&mut w, Point::new(60.0, 10.0)), EventResult::Handled);

        let closed = view(false).stagger(false);
        let open = view(true).stagger(false);
        let mut w = build(&closed);
        layout(&mut w);
        rebuild(&closed, &open, &mut w);
        let mut size = layout(&mut w);
        paint(&mut w, size, 0.0, None);
        size = layout(&mut w);
        paint(&mut w, size, 16.0, None);
        assert_eq!(tap(&mut w, Point::new(60.0, 80.0)), EventResult::Handled);
    }

    // ---- Guide -----------------------------------------------------------

    #[test]
    fn the_guide_rules_a_dashed_line_at_its_indent_and_insets_the_children() {
        let guide = RevealGuide::new(12.0, 8.0);
        let mut w = build(&view(true).guide(guide));
        let size = layout(&mut w);
        for pod in &w.pods {
            assert_eq!(pod.origin().x, 20.0, "children inset by indent + pad");
        }
        let (rec, _) = paint(&mut w, size, 0.0, None);
        let lines = rec.lines();
        assert!(lines.len() > 1, "a dash run, not one solid stroke");
        for (p0, p1, color) in &lines {
            assert_eq!(p0.x, 12.0, "the rule sits at the indent");
            assert_eq!(p1.x, 12.0);
            assert!(p1.y <= NATURAL, "and never past the revealed extent");
            assert_eq!(*color, GUIDE_BORDER, "border ink unless accented");
        }
        // Dash geometry: a 6-on/4-off run down the revealed extent.
        assert!((lines[0].1.y - lines[0].0.y - GLYPH_REVEAL_DASH_LEN).abs() < 1e-9);
        assert!(
            (lines[1].0.y - lines[0].0.y - (GLYPH_REVEAL_DASH_LEN + GLYPH_REVEAL_DASH_GAP)).abs()
                < 1e-9
        );
    }

    #[test]
    fn an_accent_guide_uses_the_accent_ink_and_stops_at_the_revealed_extent() {
        let guide = RevealGuide::new(12.0, 8.0).accented(true);
        let (closed, open) = (view(false).guide(guide), view(true).guide(guide));
        let mut w = build(&open);
        let size = layout(&mut w);
        let (rec, _) = paint(&mut w, size, 0.0, None);
        assert!(rec.lines().iter().all(|(_, _, c)| *c == ACCENT));

        // Mid-collapse the rule shortens with the band.
        rebuild(&open, &closed, &mut w);
        let mut size = layout(&mut w);
        paint(&mut w, size, 0.0, None);
        size = layout(&mut w);
        let (rec, _) = paint(&mut w, size, 100.0, None);
        let revealed = rec.clips()[0].1.height;
        assert!(revealed < NATURAL);
        assert!(rec.lines().iter().all(|(_, p1, _)| p1.y <= revealed));
    }

    #[test]
    fn a_themed_guide_resolves_the_outline_and_primary_roles() {
        let theme = crate::baseline();
        let mut w = build(&view(true).guide(RevealGuide::new(10.0, 6.0)));
        let size = layout(&mut w);
        let (rec, _) = paint(&mut w, size, 0.0, Some(&theme));
        assert!(
            rec.lines()
                .iter()
                .all(|(_, _, c)| *c == theme.scheme().outline)
        );

        let mut w = build(&view(true).guide(RevealGuide::new(10.0, 6.0).accented(true)));
        let size = layout(&mut w);
        let (rec, _) = paint(&mut w, size, 0.0, Some(&theme));
        assert!(
            rec.lines()
                .iter()
                .all(|(_, _, c)| *c == theme.scheme().primary)
        );
    }

    // ---- Plumbing --------------------------------------------------------

    #[test]
    fn every_child_is_published_to_the_visitor_and_to_semantics() {
        let w = build(&view(false));
        let mut seen = 0usize;
        Widget::visit_children(&w, &mut |_pod| seen += 1);
        assert_eq!(seen, 3, "collapsed or not, every child pod is visited");

        // Settled (collapsed, no cascade in flight) still forwards every
        // child to `semantics` too — unconditionally, same as the visitor
        // above. This is the *only* state that still does: mid-cascade,
        // `semantics` narrows to the frontier `event` can reach (see
        // `semantics_forwarding_matches_the_event_reachable_prefix_mid_cascade`
        // below). `SemanticsCtx`'s constructor is crate-private to
        // `frust-core`, so this drives a real `RenderRoot` pass rather than
        // the context itself (the `crate::toggle` precedent).
        fn logic(_s: &mut ()) -> GlyphRevealView<()> {
            view(false)
        }
        let mut root: frust_core::RenderRoot<(), GlyphRevealView<()>> =
            frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        root.layout(WINDOW);
        let update = root.semantics();
        let forwarded = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::GenericContainer)
            .count();
        assert_eq!(
            forwarded, 3,
            "settled: every child forwarded to semantics, unconditionally"
        );
    }

    #[test]
    fn semantics_forwarding_matches_the_event_reachable_prefix_mid_cascade() {
        // Same construction and timing as
        // `a_child_the_cascade_has_not_revealed_yet_is_not_hit_testable`: at
        // 165ms child 2's window has not opened, so `event` reaches only
        // children 0 and 1. `semantics` must forward exactly that same
        // prefix — the parity `GlyphRevealWidget::input_reach` fixes.
        fn logic(expanded: &mut bool) -> GlyphRevealView<bool> {
            glyph_reveal::<bool>(HEIGHTS.iter().map(|h| any(Block(Size::new(120.0, *h)))))
                .expanded(*expanded)
        }

        let mut root: frust_core::RenderRoot<bool, GlyphRevealView<bool>> =
            frust_core::RenderRoot::new();
        let mut expanded = false;
        root.rebuild(&mut logic, &mut expanded);
        root.layout(WINDOW);

        expanded = true;
        root.rebuild(&mut logic, &mut expanded);
        root.layout(WINDOW);
        let mut rec = Recorder::default();
        root.paint(&mut rec, ft_ms(0.0));
        root.layout(WINDOW);
        let mut rec = Recorder::default();
        root.paint(&mut rec, ft_ms(165.0));
        assert_eq!(
            rec.child_progress().len(),
            2,
            "sanity: same frontier as the widget-level hit-test test"
        );

        let update = root.semantics();
        let forwarded = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::GenericContainer)
            .count();
        assert_eq!(
            forwarded, 2,
            "semantics forwards only the frontier the last paint revealed, exactly like event"
        );
    }

    #[test]
    fn a_rebuilt_child_list_relays_out_and_re_measures() {
        let two = glyph_reveal::<()>(vec![
            any(Block(Size::new(120.0, 20.0))),
            any(Block(Size::new(120.0, 30.0))),
        ])
        .expanded(true);
        let three = view(true);
        let mut w = build(&two);
        assert_eq!(layout(&mut w).height, 20.0 + GLYPH_REVEAL_GAP + 30.0);
        rebuild(&two, &three, &mut w);
        assert_eq!(layout(&mut w).height, NATURAL);
        assert_eq!(w.pods.len(), 3);
    }

    #[test]
    fn a_custom_gap_changes_the_natural_extent() {
        let mut w = build(&view(true).gap(0.0));
        assert_eq!(layout(&mut w).height, HEIGHTS.iter().sum::<f64>());
    }

    // ---- Chevron ---------------------------------------------------------

    fn chevron_build(expanded: bool) -> GlyphChevronWidget {
        let mut counter = 0u64;
        View::<()>::build(&glyph_chevron(expanded), &mut BuildCtx::new(&mut counter))
    }

    fn chevron_paint(
        w: &mut GlyphChevronWidget,
        ms: f64,
        theme: Option<&Theme>,
    ) -> (Recorder, bool) {
        let size = Size::new(GLYPH_CHEVRON_BOX, GLYPH_CHEVRON_BOX);
        let mut rec = Recorder::default();
        let mut ctx = match theme {
            Some(t) => PaintCtx::for_test(Point::ZERO, size, ft_ms(ms)).with_theme(t as &dyn Any),
            None => PaintCtx::for_test(Point::ZERO, size, ft_ms(ms)),
        };
        w.paint(&mut ctx, &mut rec);
        let needs_frame = ctx.needs_frame();
        (rec, needs_frame)
    }

    /// The chevron's tip point (the shared end of its two arms).
    fn tip(rec: &Recorder) -> Point {
        rec.lines()[0].1
    }

    #[test]
    fn a_settled_chevron_points_right_when_collapsed_and_down_when_expanded() {
        let center = Point::new(GLYPH_CHEVRON_BOX / 2.0, GLYPH_CHEVRON_BOX / 2.0);

        let mut collapsed = chevron_build(false);
        let (rec, _) = chevron_paint(&mut collapsed, 0.0, None);
        let t = tip(&rec);
        assert!(
            (t.x - (center.x + CHEVRON_ARM)).abs() < 1e-9,
            "points right"
        );
        assert!((t.y - center.y).abs() < 1e-9);
        assert_eq!(rec.lines()[0].2, CHEVRON_DIM, "dim ink while collapsed");

        // Built expanded: settled at 90°, never spun into place.
        let mut expanded = chevron_build(true);
        let (rec, needs_frame) = chevron_paint(&mut expanded, 0.0, None);
        let t = tip(&rec);
        assert!((t.x - center.x).abs() < 1e-9, "points down");
        assert!((t.y - (center.y + CHEVRON_ARM)).abs() < 1e-9);
        assert_eq!(rec.lines()[0].2, ACCENT, "accent ink while expanded");
        assert!(!needs_frame, "a settled chevron asks for no frames");
    }

    #[test]
    fn a_toggled_chevron_rotates_between_the_two_settled_orientations() {
        let center = Point::new(GLYPH_CHEVRON_BOX / 2.0, GLYPH_CHEVRON_BOX / 2.0);
        let mut w = chevron_build(false);
        let mut counter = 0u64;
        <GlyphChevronView as View<()>>::rebuild(
            &glyph_chevron(true),
            &glyph_chevron(false),
            &mut w,
            &mut BuildCtx::new(&mut counter),
        );

        let (_, seeding) = chevron_paint(&mut w, 0.0, None);
        assert!(seeding, "an animating chevron asks for the next frame");
        let (rec, animating) = chevron_paint(&mut w, 60.0, None);
        assert!(animating);
        let t = tip(&rec);
        assert!(
            t.x < center.x + CHEVRON_ARM && t.y > center.y,
            "mid-rotation tip: {t:?}"
        );

        // Settles exactly on 90°, and stops asking for frames.
        let mut ms = 76.0;
        loop {
            let (rec, more) = chevron_paint(&mut w, ms, None);
            if !more {
                let t = tip(&rec);
                assert!((t.x - center.x).abs() < 1e-9);
                assert!((t.y - (center.y + CHEVRON_ARM)).abs() < 1e-9);
                break;
            }
            ms += 16.0;
            assert!(ms < 2000.0, "the rotation should settle well under 2s");
        }
    }

    #[test]
    fn a_themed_chevron_resolves_the_dim_and_accent_roles() {
        let theme = crate::baseline();
        let mut collapsed = chevron_build(false);
        let (rec, _) = chevron_paint(&mut collapsed, 0.0, Some(&theme));
        assert_eq!(rec.lines()[0].2, theme.scheme().on_surface_variant);
        let mut expanded = chevron_build(true);
        let (rec, _) = chevron_paint(&mut expanded, 0.0, Some(&theme));
        assert_eq!(rec.lines()[0].2, theme.scheme().primary);
    }

    #[test]
    fn reduce_motion_snaps_the_chevron_straight_to_its_target() {
        let mut theme = crate::baseline();
        theme.motion.reduce_motion = true;
        let mut w = chevron_build(false);
        let mut counter = 0u64;
        <GlyphChevronView as View<()>>::rebuild(
            &glyph_chevron(true),
            &glyph_chevron(false),
            &mut w,
            &mut BuildCtx::new(&mut counter),
        );
        let (rec, needs_frame) = chevron_paint(&mut w, 0.0, Some(&theme));
        let center = Point::new(GLYPH_CHEVRON_BOX / 2.0, GLYPH_CHEVRON_BOX / 2.0);
        let t = tip(&rec);
        assert!((t.x - center.x).abs() < 1e-9, "already pointing down");
        assert!(!needs_frame, "a snap asks for no further frames");
    }

    #[test]
    fn the_chevron_arm_is_sized_to_the_box_it_rotates_inside() {
        let half = GLYPH_CHEVRON_BOX / 2.0;
        let center = Point::new(half, half);
        let stroke = CHEVRON_WIDTH / 2.0;

        // At rest — the two orientations a reader actually sees — the whole mark
        // sits inside the box with an optical margin.
        for &angle in &[0.0, std::f64::consts::FRAC_PI_2] {
            let mut rec = Recorder::default();
            draw_chevron(&mut rec, center, angle, CHEVRON_DIM);
            assert!(
                reach(&rec, center) + stroke < half - 1.0,
                "a settled chevron clears the {GLYPH_CHEVRON_BOX}px box"
            );
        }

        // Rotating, the arms' outer ends sweep `arm · √2` and peak at 45°. This
        // is what keeps the arm at 3.5 rather than `accordion`'s 4.5, whose
        // chevron is drawn free-hand in a header row with no box to overrun —
        // see `CHEVRON_ARM`.
        let peak = (0..=90)
            .map(|deg| {
                let mut rec = Recorder::default();
                draw_chevron(&mut rec, center, (deg as f64).to_radians(), CHEVRON_DIM);
                reach(&rec, center)
            })
            .fold(0.0_f64, f64::max)
            + stroke;
        assert!(
            peak < half + 0.25,
            "the 45\u{b0} peak stays on the box edge: {peak} against {half}"
        );
    }

    /// The furthest any stroked endpoint sits from `center`, along either axis —
    /// the mark's reach against its box's half-extent.
    fn reach(rec: &Recorder, center: Point) -> f64 {
        rec.lines()
            .iter()
            .flat_map(|(p0, p1, _)| [*p0, *p1])
            .fold(0.0_f64, |acc, p| {
                acc.max((p.x - center.x).abs()).max((p.y - center.y).abs())
            })
    }

    #[test]
    fn the_chevron_occupies_its_token_sized_box() {
        let mut w = chevron_build(false);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(100.0, 100.0)));
        assert_eq!(size, Size::new(GLYPH_CHEVRON_BOX, GLYPH_CHEVRON_BOX));
    }
}
