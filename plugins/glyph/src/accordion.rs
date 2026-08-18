//! [`accordion`]/[`AccordionView`]: a disclosure panel — a pressable header row
//! (title + a chevron that rotates 90° as it opens) over a height-animated body
//! child view (the Glyph design system's `.accordion`). **Controlled**:
//! the caller owns the `open: bool` and receives `on_toggle` on a header
//! press; the widget never mutates its own open state (the same never-self-
//! mutating contract `crate::material::navbar` uses).
//!
//! # Content-size-independent height reveal
//!
//! The body child is **always laid out at its full natural height** and stays
//! laid out throughout; the reveal is a `0..1` fraction of that measured height
//! applied two ways — the widget's reported height is `header + fraction ·
//! body_height` (layout), and the body is drawn under a clip of that same
//! revealed band (paint). So the animation never depends on the child's content
//! or re-measures it mid-flight.
//!
//! The reveal is driven by an implicit tween ([`RevealDriver`]): a fresh
//! `0..1` [`AnimationController`] launched from the current value on each
//! toggle, over the theme's `motion.durations.base` (220ms) with the
//! `motion.easing.spatial` curve — the design system's "220ms spatial" timing.
//! Because the animation clock lives on [`PaintCtx`] (not [`LayoutCtx`]), the
//! reveal is advanced during paint and the freshest value is read by the next
//! layout pass — but the *height* this widget reports is computed from that
//! value in `layout`, so a paint-only `request_frame` is not enough: on the
//! mobile intra-frame layout skip (`docs/ARCHITECTURE.md`'s Frame gate),
//! layout never re-runs just because paint asked for another frame, and the
//! revealed height would freeze until something else dirtied the tree. While
//! the reveal driver is still animating, `paint` instead calls
//! `PaintCtx::request_layout` (which implies `request_frame`), forcing the
//! next frame's layout to re-run and pick up the freshly-advanced value —
//! paint requests layout, the next frame relayouts, and the height tracks the
//! animation on every platform, gated or not. (`reduce_motion` snaps straight
//! to the target and requests neither.)
//!
//! # Token resolution
//!
//! - **container** = `surface_container` fill + `outline` border,
//!   `shape.medium` corner; header/body split by an `outline_variant` hairline.
//! - **title** = `on_surface`; **chevron** = `on_surface_variant`.
//!
//! Unthemed, each falls back to the literal Glyph **dark** constant.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::text::{
    FontFamily, FontWeight, GenericSlot, TextContext, TextLayout, TextStyle,
};
use frust::authoring::{Action, Role};
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, PointerPhase, SemanticsCtx, View, Widget, any,
};
use frust::{AnimationController, Curve, FrameTime, Tween};
use frust::{ShapeScale, Theme};
use kurbo::{Point, Rect, RoundedRect, Shape, Size};
use peniko::{Brush, Color};

use crate::press::presses;

/// Corner-rounding tolerance for the container border stroke.
const PATH_TOLERANCE: f64 = 0.1;
/// Header horizontal padding, in logical px.
const HEADER_PAD_X: f64 = 16.0;
/// Header vertical padding, in logical px.
const HEADER_PAD_Y: f64 = 14.0;
/// Body padding on all four edges, in logical px.
const BODY_PAD: f64 = 16.0;
/// Container border width, in logical px.
const BORDER_WIDTH: f64 = 1.0;
/// Header/body divider (hairline) width, in logical px.
const DIVIDER_WIDTH: f64 = 1.0;
/// The chevron's arm length, in logical px.
const CHEVRON_ARM: f64 = 4.5;
/// The chevron stroke width, in logical px.
const CHEVRON_WIDTH: f64 = 1.5;
/// Title font size, in logical px.
const TITLE_SIZE: f32 = 13.5;
/// Reveal-animation fallback duration when no theme is threaded.
const FALLBACK_DURATION: Duration = Duration::from_millis(220);

/// Unthemed corner-radius fallback — Glyph `--radius-md` (10px).
const RADIUS_FALLBACK: f64 = 10.0;

// ---- Unthemed fallback constants (Glyph **dark** values) ---------------

const BG: Color = Color::from_rgb8(0x16, 0x1a, 0x23); // bg-surface
const BORDER: Color = Color::from_rgb8(0x3e, 0x3f, 0x44); // outline
const DIVIDER: Color = Color::from_rgb8(0x2a, 0x2d, 0x33); // outline-variant
const TITLE_FG: Color = Color::from_rgb8(0xf2, 0xea, 0xd9); // fg
const CHEVRON_FG: Color = Color::from_rgb8(0xa3, 0x9c, 0x88); // fg-muted

/// An implicit reveal driver: a `0..1` fraction tweened from its current value
/// to a new target whenever the target changes, over a per-toggle-fresh
/// [`AnimationController`]. Settles exactly on the target (see the module
/// docs). A degenerate zero-duration controller keeps `value()` reading the
/// initial fraction until the first real toggle.
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

/// A view-held, typed toggle callback (erased on build).
type OnToggle<State> = Rc<dyn Fn(&mut State)>;

/// A declarative disclosure accordion. See the [module docs](self).
pub struct AccordionView<State: 'static> {
    title: String,
    open: bool,
    body: AnyView<State>,
    on_toggle: Option<OnToggle<State>>,
}

/// Create an accordion with a header `title` over `body`. Controlled: pass the
/// current [`.open(bool)`](AccordionView::open) and handle
/// [`.on_toggle(..)`](AccordionView::on_toggle).
pub fn accordion<State: 'static, V: View<State>>(
    title: impl Into<String>,
    body: V,
) -> AccordionView<State> {
    AccordionView {
        title: title.into(),
        open: false,
        body: any(body),
        on_toggle: None,
    }
}

impl<State: 'static> AccordionView<State> {
    /// Set the (app-owned) open state.
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Fire `on_toggle` on a header press (release inside the header). The
    /// widget never flips `open` itself — the app feeds the new value back in.
    pub fn on_toggle<F: Fn(&mut State) + 'static>(mut self, on_toggle: F) -> Self {
        self.on_toggle = Some(Rc::new(on_toggle));
        self
    }
}

/// A minimal retained text run — see [`super::badge`]'s `GlyphLabel`.
struct GlyphLabel {
    content: String,
    layout: Option<TextLayout>,
    laid_out_style: Option<TextStyle>,
    laid_out_max_width: Option<f32>,
}

impl GlyphLabel {
    fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            layout: None,
            laid_out_style: None,
            laid_out_max_width: None,
        }
    }

    fn set_content(&mut self, content: impl Into<String>) {
        let content = content.into();
        if self.content != content {
            self.content = content;
            self.layout = None;
        }
    }

    fn layout(&mut self, ctx: &mut LayoutCtx, style: &TextStyle, max_width: Option<f32>) -> Size {
        if let Some(cached) = &self.layout
            && self.laid_out_max_width == max_width
            && self.laid_out_style.as_ref() == Some(style)
        {
            return cached.size();
        }
        let text_ctx = ctx.text_context::<TextContext>();
        let laid = text_ctx.layout(&self.content, style, max_width);
        let size = laid.size();
        self.layout = Some(laid);
        self.laid_out_style = Some(style.clone());
        self.laid_out_max_width = max_width;
        size
    }

    fn paint(&self, origin: Point, scene: &mut dyn PaintScene) {
        if let Some(layout) = &self.layout {
            for run in layout.to_scene_runs(origin) {
                scene.draw_glyph_run(run);
            }
        }
    }
}

/// The retained widget for an [`AccordionView`].
pub struct AccordionWidget {
    title: GlyphLabel,
    title_text: String,
    title_size: Size,
    body: ChildPod,
    open: bool,
    on_toggle: Option<frust::authoring::ErasedCallback>,
    reveal: RevealDriver,
    /// The latest reveal fraction, updated in paint and read by layout.
    reveal_value: f64,
    /// The measured header height (set in layout).
    header_height: f64,
    /// The measured full body height (set in layout).
    body_full_height: f64,
    /// Header press bookkeeping (fire-on-up-inside the header).
    pressed: bool,
    captured: bool,
}

impl<State: 'static> View<State> for AccordionView<State> {
    type Element = AccordionWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> AccordionWidget {
        AccordionWidget {
            title: GlyphLabel::new(self.title.clone()),
            title_text: self.title.clone(),
            title_size: Size::ZERO,
            body: frust::authoring::build_child(&self.body, ctx),
            open: self.open,
            on_toggle: self
                .on_toggle
                .as_ref()
                .map(frust::authoring::erase_callback),
            reveal: RevealDriver::new(if self.open { 1.0 } else { 0.0 }),
            reveal_value: if self.open { 1.0 } else { 0.0 },
            header_height: 0.0,
            body_full_height: 0.0,
            pressed: false,
            captured: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AccordionWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.title != self.title {
            element.title.set_content(self.title.clone());
            element.title_text = self.title.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.open != self.open {
            element.open = self.open;
            element.reveal.set_target(if self.open { 1.0 } else { 0.0 });
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        element.on_toggle = self
            .on_toggle
            .as_ref()
            .map(frust::authoring::erase_callback);
        flags |= frust::authoring::rebuild_child(&prev.body, &self.body, &mut element.body, ctx);
        flags
    }

    fn teardown(&self, element: &mut AccordionWidget, ctx: &mut BuildCtx<'_>) {
        frust::authoring::teardown_child(&self.body, &mut element.body, ctx);
    }
}

fn title_style(color: Color) -> TextStyle {
    TextStyle {
        family: FontFamily::stack_with_generic(["IBM Plex Mono"], GenericSlot::Monospace),
        weight: FontWeight::MEDIUM,
        ..TextStyle::new(TITLE_SIZE, color)
    }
}

/// Resolve `(bg, border, divider, title, chevron)`.
fn resolve_colors(theme: Option<&Theme>) -> (Color, Color, Color, Color, Color) {
    match theme {
        Some(theme) => {
            let s = theme.scheme();
            (
                s.surface_container,
                s.outline,
                s.outline_variant,
                s.on_surface,
                s.on_surface_variant,
            )
        }
        None => (BG, BORDER, DIVIDER, TITLE_FG, CHEVRON_FG),
    }
}

fn resolve_radius(theme: Option<&Theme>, size: Size) -> f64 {
    match theme {
        Some(theme) => ShapeScale::resolve(theme.shape.medium, size.width, size.height),
        None => RADIUS_FALLBACK,
    }
}

impl Widget for AccordionWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let (_, _, _, title_c, _) = resolve_colors(theme);

        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            320.0
        };

        // Header height from the title run.
        let title_max = (width - HEADER_PAD_X * 2.0 - CHEVRON_ARM * 2.0 - 12.0).max(20.0) as f32;
        self.title_size = self
            .title
            .layout(ctx, &title_style(title_c), Some(title_max));
        self.header_height = self.title_size.height + HEADER_PAD_Y * 2.0;

        // The body is always laid out at its full natural height.
        let body_max_w = (width - BODY_PAD * 2.0).max(0.0);
        let body_bc = BoxConstraints::new(Size::ZERO, Size::new(body_max_w, f64::INFINITY));
        let body_size = self.body.layout_child(ctx, &body_bc);
        self.body_full_height = body_size.height + BODY_PAD * 2.0;
        self.body
            .set_origin(Point::new(BODY_PAD, self.header_height + BODY_PAD));

        let revealed = self.reveal_value * self.body_full_height;
        let height = self.header_height + revealed;
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let (bg, border, divider, _title_c, chevron_c) = resolve_colors(theme);
        let radius = resolve_radius(theme, ctx.size());
        let reduce_motion = theme.map(|t| t.motion.reduce_motion).unwrap_or(false);
        let (dur, curve) = theme
            .map(|t| {
                (
                    Duration::from_secs_f64(t.motion.durations.base / 1000.0),
                    t.motion.easing.spatial,
                )
            })
            .unwrap_or((FALLBACK_DURATION, Curve::EaseInOut));

        // Advance the reveal (clock lives here). reduce_motion snaps. While
        // still animating, request layout (not just another frame) — the
        // revealed height is computed in `layout` from `reveal_value`, so the
        // mobile intra-frame layout skip needs an explicit relayout request
        // to keep the height tracking the animation (see the module docs'
        // "Content-size-independent height reveal" section).
        if reduce_motion {
            self.reveal.snap();
        } else if self.reveal.advance(ctx.frame_time(), dur, curve) {
            ctx.request_layout();
        }
        self.reveal_value = self.reveal.value().clamp(0.0, 1.0);

        let o = ctx.origin();
        let size = ctx.size();

        // Container fill + body clip (only the revealed band shows the body).
        scene.fill_rounded_rect(o, size, radius, bg);

        let revealed = self.reveal_value * self.body_full_height;
        if revealed > 0.5 {
            scene.push_clip(
                Point::new(o.x, o.y + self.header_height),
                Size::new(size.width, revealed),
            );
            self.body.paint_child(ctx, scene);
            scene.pop_clip();

            // Divider between header and body.
            scene.stroke_line(
                Point::new(o.x, o.y + self.header_height),
                Point::new(o.x + size.width, o.y + self.header_height),
                DIVIDER_WIDTH,
                divider,
            );
        }

        // Header title.
        let title_y = o.y + (self.header_height - self.title_size.height) / 2.0;
        self.title
            .paint(Point::new(o.x + HEADER_PAD_X, title_y), scene);

        // Chevron, rotated by reveal · 90° (points right when closed, down when
        // open).
        let cx = o.x + size.width - HEADER_PAD_X - CHEVRON_ARM;
        let cy = o.y + self.header_height / 2.0;
        let angle = self.reveal_value * std::f64::consts::FRAC_PI_2;
        draw_chevron(scene, Point::new(cx, cy), angle, chevron_c);

        // Container border on top.
        let rr = RoundedRect::from_rect(Rect::from_origin_size(Point::ORIGIN, size), radius);
        let path = rr.to_path(PATH_TOLERANCE);
        scene.stroke_path(o, &path, BORDER_WIDTH, &Brush::Solid(border));
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // Only the header row is the toggle target; a pointer below it (in the
        // revealed body) routes to the body child instead.
        let InputEvent::Pointer(p) = event else {
            // Focus-routed events go to the body if it holds focus.
            return frust::authoring::route_event_single(&mut self.body, ctx, event);
        };
        let in_header = p.position.y < self.header_height;
        match p.phase {
            PointerPhase::Down => {
                // Only a primary press works the header toggle; a secondary
                // press falls through to the body like an out-of-header one.
                if presses(p) && in_header && self.on_toggle.is_some() {
                    self.pressed = true;
                    self.captured = true;
                    ctx.capture_pointer();
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                frust::authoring::route_event_single(&mut self.body, ctx, event)
            }
            PointerPhase::Move => {
                if !self.captured {
                    return frust::authoring::route_event_single(&mut self.body, ctx, event);
                }
                let inside = in_header;
                if inside != self.pressed {
                    self.pressed = inside;
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.captured {
                    return frust::authoring::route_event_single(&mut self.body, ctx, event);
                }
                self.captured = false;
                let fired = in_header;
                self.pressed = false;
                if fired && let Some(cb) = self.on_toggle.as_mut() {
                    cb(ctx);
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if self.captured {
                    self.captured = false;
                    self.pressed = false;
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                frust::authoring::route_event_single(&mut self.body, ctx, event)
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::Button,
            |node| {
                node.set_label(self.title_text.as_str());
                node.set_expanded(self.open);
                node.add_action(Action::Click);
            },
            |ctx| {
                // The body's semantics is forwarded regardless of reveal state
                // (a collapsed section still contributes its subtree — the
                // silent-drop rule; hidden state is the platform's concern).
                self.body.semantics_child(ctx);
            },
        );
    }

    frust::authoring::visit_children!(body);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::Any;

    fn ft_secs(s: f64) -> FrameTime {
        FrameTime::from_nanos((s * 1_000_000_000.0) as u64)
    }

    #[derive(Default)]
    struct Recorder {
        clips: Vec<(Point, Size)>,
        lines: Vec<(Point, Point, Color)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, _o: Point, _s: Size, _r: f64, _c: Color) {}
        fn push_clip(&mut self, origin: Point, size: Size) {
            self.clips.push((origin, size));
        }
        fn pop_clip(&mut self) {}
        fn stroke_line(&mut self, p0: Point, p1: Point, _w: f64, c: Color) {
            self.lines.push((p0, p1, c));
        }
        fn stroke_path(&mut self, _o: Point, _p: &kurbo::BezPath, _w: f64, _b: &Brush) {}
        fn draw_glyph_run(&mut self, _run: frust::authoring::scene::GlyphRun) {}
    }

    fn build<S: 'static>(view: &AccordionView<S>) -> AccordionWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut AccordionWidget, theme: Option<&Theme>) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = match theme {
            Some(t) => {
                LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), Some(t as &dyn Any))
            }
            None => LayoutCtx::with_text_context(&mut tcx as &mut dyn Any),
        };
        w.layout(
            &mut lctx,
            &BoxConstraints::new(Size::new(320.0, 0.0), Size::new(320.0, f64::INFINITY)),
        )
    }

    fn paint_at(w: &mut AccordionWidget, size: Size, theme: Option<&Theme>) -> Recorder {
        let mut rec = Recorder::default();
        let mut ctx = match theme {
            Some(th) => PaintCtx::new(Point::ZERO, size).with_theme(th),
            None => PaintCtx::new(Point::ZERO, size),
        };
        w.paint(&mut ctx, &mut rec);
        rec
    }

    fn body() -> frust::SizedBoxView<()> {
        frust::SizedBox::<()>(Some(200.0), Some(100.0))
    }

    #[test]
    fn closed_reports_only_header_height() {
        let view: AccordionView<()> = accordion("Details", body()).open(false);
        let mut w = build(&view);
        let size = layout(&mut w, None);
        assert_eq!(size.height, w.header_height);
        assert!(w.body_full_height > 0.0);
    }

    #[test]
    fn open_reports_header_plus_full_body() {
        let view: AccordionView<()> = accordion("Details", body()).open(true);
        let mut w = build(&view);
        let size = layout(&mut w, None);
        assert!((size.height - (w.header_height + w.body_full_height)).abs() < 1e-6);
    }

    #[test]
    fn reveal_driver_lands_between_endpoints_mid_animation() {
        // The reveal driver interpolates strictly between 0 and 1 partway
        // through a 220ms open animation.
        let mut d = RevealDriver::new(0.0);
        assert_eq!(d.value(), 0.0);
        d.set_target(1.0);
        let dur = Duration::from_millis(220);
        // A monotonic curve keeps the mid-duration read cleanly between the
        // endpoints (the Glyph spatial bezier overshoots and can reach ~1 by
        // mid-flight — the driver mechanism, not the curve, is under test here).
        let curve = Curve::Linear;
        d.advance(ft_secs(0.0), dur, curve); // seed the clock
        let still = d.advance(ft_secs(0.11), dur, curve); // half-way
        let mid = d.value();
        assert!(mid > 0.0 && mid < 1.0, "reveal mid-animation: {mid}");
        assert!(still, "still animating at the halfway point");
        // Completing the duration settles exactly on the target.
        d.advance(ft_secs(0.5), dur, curve);
        assert_eq!(d.value(), 1.0);
    }

    #[test]
    fn height_is_between_closed_and_open_at_mid_reveal() {
        let view: AccordionView<()> = accordion("Details", body()).open(true);
        let mut w = build(&view);
        layout(&mut w, None); // measures header + full body at reveal 1.0
        let header = w.header_height;
        let full = header + w.body_full_height;

        // Force a mid reveal and re-measure.
        w.reveal_value = 0.5;
        let size = layout(&mut w, None);
        assert!(
            size.height > header && size.height < full,
            "height {} between {} and {}",
            size.height,
            header,
            full
        );
        assert!((size.height - (header + 0.5 * w.body_full_height)).abs() < 1e-6);
    }

    #[test]
    fn chevron_rotates_toward_down_as_it_opens() {
        // Closed: chevron points right (tip at max x).
        let closed: AccordionView<()> = accordion("Details", body()).open(false);
        let mut w = build(&closed);
        layout(&mut w, None);
        let hh = w.header_height;
        let rec_closed = paint_at(&mut w, Size::new(320.0, hh), None);
        let cn = rec_closed.lines.len();
        let closed_tip = rec_closed.lines[cn - 1].0; // tip point of the second arm
        // Open fully: tip rotates 90° toward pointing down (x decreases).
        let open: AccordionView<()> = accordion("Details", body()).open(true);
        let mut w2 = build(&open);
        let size2 = layout(&mut w2, None);
        let rec_open = paint_at(&mut w2, size2, None);
        let on = rec_open.lines.len();
        let open_tip = rec_open.lines[on - 1].0;
        assert!(open_tip.x < closed_tip.x);
    }

    #[test]
    fn controlled_toggle_never_self_mutates_open() {
        #[derive(Default)]
        struct S {
            toggles: u32,
        }
        let view: AccordionView<S> =
            accordion("Details", frust::SizedBox::<S>(Some(200.0), Some(80.0)))
                .open(false)
                .on_toggle(|s: &mut S| s.toggles += 1);
        let mut w = build(&view);
        layout_generic(&mut w);

        let ev = |phase, y: f64| {
            InputEvent::Pointer(frust_core::PointerEvent {
                phase,
                position: Point::new(20.0, y),
                button: frust_core::PointerButton::Primary,
            })
        };
        let mut state = S::default();
        let dispatch = |w: &mut AccordionWidget, s: &mut S, e: &InputEvent| {
            let sa: &mut dyn Any = s;
            let mut ctx = EventCtx::new(sa, Point::ZERO, Size::new(320.0, 400.0));
            w.event(&mut ctx, e)
        };
        let hy = w.header_height / 2.0;
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, hy));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, hy));
        assert_eq!(state.toggles, 1);
        // The widget's own `open` is unchanged — controlled contract.
        assert!(!w.open);
    }

    fn layout_generic(w: &mut AccordionWidget) {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(
            &mut lctx,
            &BoxConstraints::new(Size::new(320.0, 0.0), Size::new(320.0, f64::INFINITY)),
        );
    }

    #[test]
    fn up_outside_header_does_not_toggle() {
        #[derive(Default)]
        struct S {
            toggles: u32,
        }
        let view: AccordionView<S> =
            accordion("Details", frust::SizedBox::<S>(Some(200.0), Some(80.0)))
                .open(true)
                .on_toggle(|s: &mut S| s.toggles += 1);
        let mut w = build(&view);
        layout_generic(&mut w);
        let ev = |phase, y: f64| {
            InputEvent::Pointer(frust_core::PointerEvent {
                phase,
                position: Point::new(20.0, y),
                button: frust_core::PointerButton::Primary,
            })
        };
        let mut state = S::default();
        let dispatch = |w: &mut AccordionWidget, s: &mut S, e: &InputEvent| {
            let sa: &mut dyn Any = s;
            let mut ctx = EventCtx::new(sa, Point::ZERO, Size::new(320.0, 400.0));
            w.event(&mut ctx, e)
        };
        let hy = w.header_height / 2.0;
        let below = w.header_height + 20.0;
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, hy));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, below));
        assert_eq!(state.toggles, 0);
    }

    #[test]
    fn reduce_motion_snaps_to_target() {
        let mut theme = crate::baseline();
        theme.motion.reduce_motion = true;
        let closed: AccordionView<()> = accordion("Details", body()).open(false);
        let mut w = build(&closed);
        layout(&mut w, Some(&theme));
        let open: AccordionView<()> = accordion("Details", body()).open(true);
        let mut counter = 0u64;
        <AccordionView<()> as View<()>>::rebuild(
            &open,
            &closed,
            &mut w,
            &mut BuildCtx::new(&mut counter),
        );
        // One paint under reduce_motion snaps reveal straight to 1.0.
        let mut ctx =
            PaintCtx::new(Point::ZERO, Size::new(320.0, w.header_height)).with_theme(&theme);
        let mut rec = Recorder::default();
        w.paint(&mut ctx, &mut rec);
        assert_eq!(w.reveal_value, 1.0);
        assert!(!ctx.needs_frame(), "snapped: no further frames requested");
    }

    #[test]
    fn paint_requests_layout_while_reveal_is_animating() {
        // After `set_target` flips open (via a
        // rebuild), each paint during the reveal animation must report
        // `PaintOutcome::needs_layout == true`; once settled, false. Driven
        // through `RenderRoot` (not a bare `PaintCtx`) since only
        // `RenderRoot::paint` accepts an explicit `FrameTime` from outside
        // `frust-core`.
        let mut state = ();
        let mut root: frust_core::RenderRoot<(), AccordionView<()>> = frust_core::RenderRoot::new();
        root.rebuild(
            &mut |_s: &mut ()| accordion("Details", body()).open(false),
            &mut state,
        );
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(320.0, 400.0), &mut tcx as &mut dyn Any);
        let mut scene = Recorder::default();
        let settled_closed = root.paint(&mut scene, FrameTime::ZERO);
        assert!(
            !settled_closed.needs_layout,
            "already-settled closed state requests no layout"
        );

        // Flip open — the reveal driver now has a pending retarget.
        root.rebuild(
            &mut |_s: &mut ()| accordion("Details", body()).open(true),
            &mut state,
        );
        root.layout_with_text(Size::new(320.0, 400.0), &mut tcx as &mut dyn Any);

        let mut t_ns = 0u64;
        let mut saw_animating_frame = false;
        loop {
            let mut scene = Recorder::default();
            let outcome = root.paint(&mut scene, FrameTime::from_nanos(t_ns));
            if outcome.needs_layout {
                saw_animating_frame = true;
                assert!(outcome.needs_frame, "request_layout implies needs_frame");
                // Re-run layout for the next frame, as a real shell would on
                // seeing `needs_layout` in the pending change flags.
                root.layout_with_text(Size::new(320.0, 400.0), &mut tcx as &mut dyn Any);
            } else {
                break;
            }
            t_ns += 16_000_000; // ~16ms per simulated frame
            assert!(
                t_ns < 2_000_000_000,
                "reveal animation should settle well under 2s of simulated frames"
            );
        }
        assert!(
            saw_animating_frame,
            "at least one paint during the reveal reported needs_layout"
        );

        // One more settled paint reports no further layout request.
        let mut scene = Recorder::default();
        let settled_open = root.paint(&mut scene, FrameTime::from_nanos(t_ns));
        assert!(
            !settled_open.needs_layout,
            "settled: no more layout requests"
        );
    }

    #[test]
    fn multi_instance_toggle_only_animates_the_toggled_instance() {
        // Three accordions in a Column — toggling
        // only the middle one must animate only its own body clip/height,
        // leaving the other two closed and clip-free throughout. Regression
        // guard for the "content on wrong instance" symptom now that
        // relayout tracks the reveal animation.
        #[derive(Default)]
        struct S {
            open: [bool; 3],
        }

        fn body_s() -> frust::SizedBoxView<S> {
            frust::SizedBox::<S>(Some(200.0), Some(100.0))
        }

        fn view(s: &mut S) -> AnyView<S> {
            any(frust::Column(vec![
                any(accordion("A", body_s())
                    .open(s.open[0])
                    .on_toggle(|s: &mut S| s.open[0] = !s.open[0])),
                any(accordion("B", body_s())
                    .open(s.open[1])
                    .on_toggle(|s: &mut S| s.open[1] = !s.open[1])),
                any(accordion("C", body_s())
                    .open(s.open[2])
                    .on_toggle(|s: &mut S| s.open[2] = !s.open[2])),
            ]))
        }

        let mut state = S::default();
        let mut root: frust_core::RenderRoot<S, AnyView<S>> = frust_core::RenderRoot::new();
        root.rebuild(&mut view, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(320.0, 2000.0), &mut tcx as &mut dyn Any);
        let mut scene = Recorder::default();
        root.paint(&mut scene, FrameTime::ZERO);
        assert!(scene.clips.is_empty(), "all three closed: no body clip yet");

        // Toggle only the middle instance open.
        state.open[1] = true;
        root.rebuild(&mut view, &mut state);
        root.layout_with_text(Size::new(320.0, 2000.0), &mut tcx as &mut dyn Any);

        let mut t_ns = 0u64;
        let mut saw_mid_clip = false;
        loop {
            let mut scene = Recorder::default();
            let outcome = root.paint(&mut scene, FrameTime::from_nanos(t_ns));
            // Only the toggled (middle) instance ever reveals a body clip —
            // A and C stay closed and clip-free on every frame.
            assert!(
                scene.clips.len() <= 1,
                "only the toggled instance ever reveals a body clip, got {}",
                scene.clips.len()
            );
            if let Some(&(_, size)) = scene.clips.first() {
                saw_mid_clip = true;
                assert!(size.height > 0.0);
            }
            if !outcome.needs_layout {
                break;
            }
            root.layout_with_text(Size::new(320.0, 2000.0), &mut tcx as &mut dyn Any);
            t_ns += 16_000_000;
            assert!(
                t_ns < 2_000_000_000,
                "reveal animation should settle well under 2s of simulated frames"
            );
        }
        assert!(
            saw_mid_clip,
            "the toggled instance revealed a body clip during animation"
        );
    }

    #[test]
    fn semantics_is_a_button_with_expanded_state_forwarding_body() {
        fn logic(_s: &mut ()) -> AccordionView<()> {
            accordion("Details", frust::text("body content")).open(true)
        }
        let mut root: frust_core::RenderRoot<(), AccordionView<()>> = frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(320.0, 400.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Button)
            .expect("accordion contributes a Role::Button node");
        assert_eq!(node.label(), Some("Details"));
        assert!(!node.children().is_empty(), "the body is a semantics child");
    }
}
