//! Layer 2: the retained [`Widget`] trait and its layout/paint contexts
//! (spec §6).
//!
//! Widgets are the long-lived counterpart to [`crate::view::View`]s. A view is
//! rebuilt every frame; the widget it produced persists in the arena and is
//! mutated in place. Widgets participate in two passes:
//!
//! * **layout** — receive [`BoxConstraints`] and return a chosen [`Size`].
//! * **paint** — emit draw commands into a scene.

use std::any::Any;

use forgekit_scene::{GlyphRun, SceneBuilder};
use kurbo::{Affine, BezPath, Point, Rect, Size};
use peniko::{Brush, Color};

use crate::anim::FrameTime;
use crate::event::{EventCtx, EventResult, ImeState, InputEvent};
use crate::layout::BoxConstraints;
use crate::semantics::SemanticsCtx;

/// The renderer-agnostic paint target a widget draws into.
///
/// This trait was introduced (task 02) as a **local stand-in** for
/// `forgekit_scene::SceneBuilder` while the scene crate was still a stub. Task
/// 08 reconciles the two *additively*: rather than churn the `Widget::paint`
/// signature (and every widget/test written against it), `SceneBuilder` now
/// [implements this trait](#impl-PaintScene-for-SceneBuilder), so widgets keep
/// painting through `&mut dyn PaintScene` while the shell hands them a real
/// `SceneBuilder` whose commands reach the GPU backend.
///
/// The original `fill_rect`/`draw_text` shape is retained for source
/// compatibility with existing recorder-style test scenes; real text rendering
/// goes through [`PaintScene::draw_glyph_run`], which carries shaped glyphs from
/// `forgekit-text`.
pub trait PaintScene {
    /// Emit a filled axis-aligned rectangle at `origin` with `size`, filled with
    /// the solid `color`.
    fn fill_rect(&mut self, origin: Point, size: Size, color: Color);

    /// Emit a filled axis-aligned rectangle with uniformly rounded corners.
    ///
    /// Defaulted to a no-op so pre-existing recorder scenes stay valid; the
    /// `SceneBuilder` implementation records a real rounded-rect command.
    fn fill_rounded_rect(&mut self, _origin: Point, _size: Size, _radius: f64, _color: Color) {}

    /// Stroke a straight line from `p0` to `p1` with the given `width` and solid
    /// `color`.
    ///
    /// Defaulted to a no-op so pre-existing recorder scenes stay valid; the
    /// `SceneBuilder` implementation records a real stroked-line command.
    fn stroke_line(&mut self, _p0: Point, _p1: Point, _width: f64, _color: Color) {}

    /// Push a rectangular clip (at `origin`/`size`) onto the backend clip stack;
    /// subsequent draws are clipped to it until the matching [`PaintScene::pop_clip`].
    ///
    /// Defaulted to a no-op so recorder scenes stay valid; the `SceneBuilder`
    /// implementation honors the clip by recording a push/pop command pair.
    fn push_clip(&mut self, _origin: Point, _size: Size) {}

    /// Pop the most recently pushed clip. Defaulted to a no-op; see
    /// [`PaintScene::push_clip`].
    fn pop_clip(&mut self) {}

    /// Emit a run of *unshaped* text anchored at `origin`.
    ///
    /// This records intent only — glyph shaping lives in `forgekit-text`
    /// (task 07). Real rendering uses [`PaintScene::draw_glyph_run`]; the
    /// `SceneBuilder` implementation treats this as a no-op.
    fn draw_text(&mut self, origin: Point, text: &str);

    /// Emit a run of already-shaped glyphs into the scene.
    ///
    /// Defaulted to a no-op so pre-existing recorder scenes (which predate the
    /// text pipeline) stay valid without modification; the `SceneBuilder`
    /// implementation overrides it to record a real glyph-run command.
    fn draw_glyph_run(&mut self, _run: GlyphRun) {}

    /// Draw an already-decoded image, scaled from its natural
    /// (`data.width`x`data.height`) size to fill the absolute `dest` rect.
    ///
    /// A single additive method (task 57, `forgekit-widgets::Image`) on this
    /// otherwise layer-2 trait — authorized because `Command::Image`'s
    /// `peniko::ImageData` payload has to reach the scene through the same
    /// `&mut dyn PaintScene` seam every other paint call uses. Defaulted to a
    /// no-op so pre-existing recorder scenes stay valid; the `SceneBuilder`
    /// implementation records a real image command.
    fn draw_image(&mut self, _data: &peniko::ImageData, _dest: Rect) {}

    /// Draw a gaussian-blurred rounded-rectangle elevation shadow (an
    /// approximation of a CSS `box-shadow`) at `origin`/`size`.
    ///
    /// Defaulted to a no-op so pre-existing recorder scenes stay valid; the
    /// `SceneBuilder` implementation records a real
    /// [`forgekit_scene::Command::BlurredRoundedRect`].
    fn draw_shadow(
        &mut self,
        _origin: Point,
        _size: Size,
        _radius: f64,
        _std_dev: f64,
        _color: Color,
    ) {
    }

    /// Emit a filled axis-aligned rectangle at `origin` with `size`, filled
    /// with an arbitrary `brush` (solid color or gradient).
    ///
    /// Default implementation delegates to [`PaintScene::fill_rect`] using
    /// the brush's solid color where possible (a `Brush::Solid` unwraps
    /// directly; a gradient brush falls back to transparent black, since a
    /// pre-existing recorder scene has no gradient concept to approximate
    /// it with) — so callers that only override `fill_rect` still see
    /// *something* painted rather than nothing. The `SceneBuilder`
    /// implementation records the brush faithfully via
    /// [`forgekit_scene::Command::RoundedRect`]'s zero-radius sibling
    /// (`FillRect`).
    fn fill_rect_brush(&mut self, origin: Point, size: Size, brush: &Brush) {
        let color = match brush {
            Brush::Solid(color) => *color,
            _ => Color::TRANSPARENT,
        };
        self.fill_rect(origin, size, color);
    }

    /// Emit a filled axis-aligned rectangle with uniformly rounded corners,
    /// filled with an arbitrary `brush` (solid color or gradient).
    ///
    /// Defaulted to a no-op so pre-existing recorder scenes stay valid; the
    /// `SceneBuilder` implementation records a real
    /// [`forgekit_scene::Command::RoundedRect`] carrying the brush.
    fn fill_rounded_rect_brush(
        &mut self,
        _origin: Point,
        _size: Size,
        _radius: f64,
        _brush: &Brush,
    ) {
    }

    /// Push a translucent layer (at `origin`/`size`) onto the backend layer
    /// stack; subsequent draws are composited at `alpha` until the matching
    /// [`PaintScene::pop_layer`].
    ///
    /// Defaulted to a no-op so pre-existing recorder scenes stay valid; the
    /// `SceneBuilder` implementation records a real
    /// [`forgekit_scene::Command::PushLayer`]/[`forgekit_scene::Command::PopLayer`]
    /// pair, nesting correctly with [`PaintScene::push_clip`]/[`PaintScene::pop_clip`].
    fn push_layer(&mut self, _origin: Point, _size: Size, _alpha: f32) {}

    /// Pop the most recently pushed layer. Defaulted to a no-op; see
    /// [`PaintScene::push_layer`].
    fn pop_layer(&mut self) {}

    /// Fill an arbitrary vector path (e.g. an arc — see
    /// [`forgekit_scene::arc_path`]) at `origin`, using the nonzero winding
    /// rule and `brush`.
    ///
    /// `path` is in the widget's local coordinate space; `origin` translates
    /// it into the parent's space, mirroring every other `PaintScene`
    /// method's origin convention (task 05, PLAN.md D2b). Defaulted to a
    /// no-op so pre-existing recorder scenes stay valid; the `SceneBuilder`
    /// implementation records a real [`forgekit_scene::Command::Path`].
    fn fill_path(&mut self, _origin: Point, _path: &BezPath, _brush: &Brush) {}

    /// Stroke an arbitrary vector path (e.g. an arc) at `origin` with `width`
    /// and round caps/joins, using `brush`.
    ///
    /// Defaulted to a no-op so pre-existing recorder scenes stay valid; see
    /// [`PaintScene::fill_path`].
    fn stroke_path(&mut self, _origin: Point, _path: &BezPath, _width: f64, _brush: &Brush) {}
}

/// Bridges the provisional [`PaintScene`] boundary onto the real
/// `forgekit_scene::SceneBuilder` (task 08 reconciliation).
///
/// Widgets paint through `&mut dyn PaintScene`; the desktop shell (task 08)
/// hands them a `SceneBuilder`, so filled rectangles and shaped glyph runs land
/// in the display list under the builder's current transform. Unshaped
/// [`PaintScene::draw_text`] is intentionally dropped here — text must be shaped
/// (by `forgekit-text`) into glyph runs before it can be drawn.
impl PaintScene for SceneBuilder<'_> {
    fn fill_rect(&mut self, origin: Point, size: Size, color: Color) {
        SceneBuilder::fill_rect(self, rect_at(origin, size), Brush::Solid(color));
    }

    fn fill_rounded_rect(&mut self, origin: Point, size: Size, radius: f64, color: Color) {
        SceneBuilder::fill_rounded_rect(self, rect_at(origin, size), radius, Brush::Solid(color));
    }

    fn stroke_line(&mut self, p0: Point, p1: Point, width: f64, color: Color) {
        SceneBuilder::stroke_line(self, p0, p1, width, Brush::Solid(color));
    }

    fn push_clip(&mut self, origin: Point, size: Size) {
        SceneBuilder::push_clip(self, rect_at(origin, size));
    }

    fn pop_clip(&mut self) {
        SceneBuilder::pop_clip(self);
    }

    fn draw_text(&mut self, _origin: Point, _text: &str) {
        // Unshaped text is not renderable; real text arrives as glyph runs.
    }

    fn draw_glyph_run(&mut self, run: GlyphRun) {
        SceneBuilder::draw_glyph_run(self, run);
    }

    fn draw_image(&mut self, data: &peniko::ImageData, dest: Rect) {
        SceneBuilder::draw_image(self, data, dest);
    }

    fn draw_shadow(&mut self, origin: Point, size: Size, radius: f64, std_dev: f64, color: Color) {
        SceneBuilder::draw_blurred_rounded_rect(
            self,
            rect_at(origin, size),
            radius,
            std_dev,
            color,
        );
    }

    fn fill_rect_brush(&mut self, origin: Point, size: Size, brush: &Brush) {
        SceneBuilder::fill_rect(self, rect_at(origin, size), brush.clone());
    }

    fn fill_rounded_rect_brush(&mut self, origin: Point, size: Size, radius: f64, brush: &Brush) {
        SceneBuilder::fill_rounded_rect(self, rect_at(origin, size), radius, brush.clone());
    }

    fn push_layer(&mut self, origin: Point, size: Size, alpha: f32) {
        SceneBuilder::push_layer(self, rect_at(origin, size), alpha);
    }

    fn pop_layer(&mut self) {
        SceneBuilder::pop_layer(self);
    }

    fn fill_path(&mut self, origin: Point, path: &BezPath, brush: &Brush) {
        SceneBuilder::fill_path(self, path_at(origin, path), brush.clone());
    }

    fn stroke_path(&mut self, origin: Point, path: &BezPath, width: f64, brush: &Brush) {
        SceneBuilder::stroke_path(self, path_at(origin, path), width, brush.clone());
    }
}

/// Build an origin/size pair into the `kurbo::Rect` the scene builder speaks.
fn rect_at(origin: Point, size: Size) -> Rect {
    Rect::new(
        origin.x,
        origin.y,
        origin.x + size.width,
        origin.y + size.height,
    )
}

/// Translates `path` (in the widget's local coordinate space) by `origin`,
/// mirroring [`rect_at`]'s origin/size convention for [`PaintScene::fill_path`]/
/// [`PaintScene::stroke_path`].
fn path_at(origin: Point, path: &BezPath) -> BezPath {
    Affine::translate((origin.x, origin.y)) * path.clone()
}

/// Context passed to [`Widget::layout`].
///
/// Beyond the (still-empty) container seam, it optionally carries the shared,
/// heavyweight text-shaping context the render root threads down for text
/// layout (spec §10.3), plus the app's active theme (spec §11 design tokens).
/// Both resources are **type-erased** (`&mut dyn Any` / `&dyn Any`) so
/// `forgekit-core` stays independent of `forgekit-text` (and thus of parley)
/// and of `forgekit-theme`; text widgets recover the shaping context with
/// [`LayoutCtx::text_context`] and themed widgets recover the theme with
/// [`LayoutCtx::theme_as`].
pub struct LayoutCtx<'a> {
    text_ctx: Option<&'a mut dyn Any>,
    /// The app's active theme, threaded down type-erased by the render root so
    /// this crate needs no `forgekit-theme` dependency. `None` in bare-core
    /// tests and pre-theme apps — a *supported* state (unlike the text context,
    /// whose absence at a text widget is a wiring bug), so [`LayoutCtx::theme_as`]
    /// returns `Option` rather than panicking.
    theme: Option<&'a dyn Any>,
}

impl<'a> LayoutCtx<'a> {
    /// Create a layout context with no shared resources.
    ///
    /// Used by leaf-only unit tests and by containers that never lay out text.
    pub fn new() -> LayoutCtx<'static> {
        LayoutCtx {
            text_ctx: None,
            theme: None,
        }
    }

    /// Create a layout context carrying the shared text-shaping context.
    ///
    /// The render root builds this so text widgets can shape their content
    /// during the layout pass; the concrete type is erased to keep this crate
    /// free of a `forgekit-text` dependency.
    pub fn with_text_context(text_ctx: &'a mut dyn Any) -> Self {
        LayoutCtx {
            text_ctx: Some(text_ctx),
            theme: None,
        }
    }

    /// Create a layout context carrying both an optional text-shaping context
    /// and an optional type-erased theme.
    ///
    /// The render root uses this to thread both resources it owns into the
    /// layout pass in one shot (see [`crate::app::RenderRoot::layout`]).
    pub fn with_resources(text_ctx: Option<&'a mut dyn Any>, theme: Option<&'a dyn Any>) -> Self {
        LayoutCtx { text_ctx, theme }
    }

    /// Attach the app's active theme, type-erased. Chainable builder used by the
    /// render root when it lends a stored theme into the layout pass.
    pub fn with_theme(mut self, theme: &'a dyn Any) -> Self {
        self.theme = Some(theme);
        self
    }

    /// Recover the shared text-shaping context as `&mut T`.
    ///
    /// Panics if no context was threaded into this pass, or if its concrete
    /// type differs from `T` — both are shell-wiring bugs, not runtime-data
    /// conditions.
    pub fn text_context<T: Any>(&mut self) -> &mut T {
        self.text_ctx
            .as_deref_mut()
            .expect("no text context threaded into this layout pass")
            .downcast_mut::<T>()
            .expect("threaded layout resource is not the expected text-context type")
    }

    /// Recover the threaded theme as `&T`, or `None` if no theme was threaded
    /// into this pass (a supported state — bare-core tests and pre-theme apps)
    /// or its concrete type differs from `T`.
    ///
    /// Mirrors [`LayoutCtx::text_context`] but returns `Option` rather than
    /// panicking, because a missing theme is a valid runtime state, not a
    /// wiring bug. Widgets that read `forgekit_theme::Theme` downcast through
    /// this (or the `Theme::from_layout_ctx` convenience wrapper).
    pub fn theme_as<T: Any>(&self) -> Option<&T> {
        self.theme?.downcast_ref::<T>()
    }
}

impl Default for LayoutCtx<'static> {
    fn default() -> Self {
        Self::new()
    }
}

/// Context passed to [`Widget::paint`].
///
/// Carries the widget's resolved geometry (as stored in its pod after layout) so
/// paint code can position itself in the parent coordinate space, plus the v1
/// animation-driver signal ([`PaintCtx::request_frame`]): a widget whose paint
/// advances animation state (e.g. a scroll fling) must call it so the shell keeps
/// scheduling frames even absent external input. The flag bubbles up through
/// [`ChildPod::paint_child`] and out of [`crate::app::RenderRoot::paint`] as a
/// [`PaintOutcome`], mirroring how [`EventCtx::request_redraw`] surfaces through
/// [`crate::event::EventOutcome`].
pub struct PaintCtx<'a> {
    origin: Point,
    size: Size,
    needs_frame: bool,
    ime_state: Option<ImeState>,
    /// Whether the widget being painted currently holds the focus path — seeded
    /// from its pod's recorded focus flag ([`ChildPod::is_focused`]) by
    /// [`ChildPod::paint_child`], and from [`crate::app::RenderRoot`]'s
    /// `focus_active` at the root. The paint-pass mirror of
    /// [`EventCtx::has_focus`]: an editable gates its focus chrome (accent
    /// border, caret, blink `request_frame`, IME republish) on it, so a
    /// container-routed blur — which clears the pod's focus path without calling
    /// the widget's `event()` — is finally observed here (see
    /// [`PaintCtx::has_focus`]).
    has_focus: bool,
    /// The shell-provided time for this frame, threaded from
    /// [`crate::app::RenderRoot::paint`] and seeded into each child by
    /// [`ChildPod::paint_child`]. Defaults to [`FrameTime::ZERO`] (the "no time
    /// available" fallback) for a paint context built without a clock (leaf unit
    /// tests, recorder scenes). A widget differences it against a stored earlier
    /// value to advance animation state — see [`PaintCtx::frame_time`].
    frame_time: FrameTime,
    /// The app's active theme, threaded down type-erased by the render root
    /// ([`crate::app::RenderRoot::paint`]) and seeded into each child by
    /// [`ChildPod::paint_child`], mirroring how `frame_time`/`has_focus` flow.
    /// `None` in bare-core tests and pre-theme apps — a supported state, so
    /// [`PaintCtx::theme_as`] returns `Option` rather than panicking.
    theme: Option<&'a dyn Any>,
}

impl<'a> PaintCtx<'a> {
    /// Create a paint context for a widget at `origin` with `size`.
    ///
    /// The frame time defaults to [`FrameTime::ZERO`] and no theme is threaded
    /// in; the render root seeds the real shell clock via
    /// [`PaintCtx::set_frame_time`] and the active theme via
    /// [`PaintCtx::set_theme`] before painting the root widget, and both flow to
    /// children through [`ChildPod::paint_child`].
    pub fn new(origin: Point, size: Size) -> Self {
        Self {
            origin,
            size,
            needs_frame: false,
            ime_state: None,
            has_focus: false,
            frame_time: FrameTime::ZERO,
            theme: None,
        }
    }

    /// Attach the app's active theme, type-erased. Chainable builder mirroring
    /// [`LayoutCtx::with_theme`] — used by widget unit tests that paint against a
    /// known theme; the render root threads it via [`PaintCtx::set_theme`].
    pub fn with_theme(mut self, theme: &'a dyn Any) -> Self {
        self.theme = Some(theme);
        self
    }

    /// Recover the threaded theme as `&T`, or `None` if no theme was threaded
    /// into this pass (a supported state — bare-core tests and pre-theme apps)
    /// or its concrete type differs from `T`.
    ///
    /// The paint-pass mirror of [`LayoutCtx::theme_as`]. Widgets that read
    /// `forgekit_theme::Theme` downcast through this (or the
    /// `Theme::from_paint_ctx` convenience wrapper).
    pub fn theme_as<T: Any>(&self) -> Option<&T> {
        self.theme?.downcast_ref::<T>()
    }

    /// Seed the type-erased theme lent by the render root. Called by
    /// [`crate::app::RenderRoot::paint`] at the root and by
    /// [`ChildPod::paint_child`] for each child, mirroring how `frame_time` is
    /// threaded. The `Option<&dyn Any>` is copied down unchanged so a nested
    /// widget observes the same theme instance without re-borrowing the parent
    /// context.
    pub(crate) fn set_theme(&mut self, theme: Option<&'a dyn Any>) {
        self.theme = theme;
    }

    /// The type-erased theme reference this context carries, for re-lending to a
    /// child context (copied, so it does not hold a borrow of `self`).
    pub(crate) fn theme_ref(&self) -> Option<&'a dyn Any> {
        self.theme
    }

    /// The widget's origin in its parent's coordinate space.
    pub fn origin(&self) -> Point {
        self.origin
    }

    /// The widget's resolved size.
    pub fn size(&self) -> Size {
        self.size
    }

    /// Whether the widget being painted holds the focus path.
    ///
    /// Threaded down from the widget's pod ([`ChildPod::is_focused`], seeded at
    /// the root from `RenderRoot::focus_active`), this is the *authoritative*
    /// focus signal during paint — a widget must prefer it over any focus flag
    /// it tracks internally. A container-routed blur clears the pod's focus path
    /// but never dispatches to the widget's `event()`, so a widget-internal flag
    /// can lag; reading `has_focus()` here (and self-correcting the internal
    /// flag against it) lets the widget converge one frame after the blur.
    /// Mirrors [`EventCtx::has_focus`].
    pub fn has_focus(&self) -> bool {
        self.has_focus
    }

    /// Seed whether the widget being painted holds focus. Called by
    /// [`ChildPod::paint_child`] (from the pod's recorded focus flag) and by
    /// [`crate::app::RenderRoot::paint`] (from `focus_active`) — the paint mirror
    /// of [`EventCtx::set_has_focus`].
    pub(crate) fn set_has_focus(&mut self, has_focus: bool) {
        self.has_focus = has_focus;
    }

    /// The shell-provided time for this frame (monotonic, arbitrary origin).
    ///
    /// This is the single shared clock the whole paint pass sees: seeded from the
    /// shell at the root ([`crate::app::RenderRoot::paint`]) and threaded
    /// unchanged into every child by [`ChildPod::paint_child`], so sibling and
    /// nested animations advance against one consistent timestamp. A widget may
    /// only *difference* it against an earlier `frame_time` it stored (via
    /// [`FrameTime::saturating_sub`] / [`crate::anim::AnimationController::advance`]),
    /// never interpret it absolutely — the origin varies per shell. Defaults to
    /// [`FrameTime::ZERO`] when no clock was threaded in (leaf unit tests).
    pub fn frame_time(&self) -> FrameTime {
        self.frame_time
    }

    /// Seed the shell-provided frame time. Called by
    /// [`crate::app::RenderRoot::paint`] at the root and by
    /// [`ChildPod::paint_child`] for each child, mirroring how `has_focus` is
    /// threaded.
    pub(crate) fn set_frame_time(&mut self, frame_time: FrameTime) {
        self.frame_time = frame_time;
    }

    /// Signal that this paint advanced animation state and needs to be
    /// re-invoked to continue, even with no intervening input event.
    ///
    /// The desktop shell honors this with a `window.request_redraw()` (its
    /// `ControlFlow::Wait` loop would otherwise idle); the mobile shells'
    /// continuous per-frame loops already schedule the next frame and can ignore
    /// it. Mirrors [`EventCtx::request_redraw`].
    pub fn request_frame(&mut self) {
        self.needs_frame = true;
    }

    /// Whether a continuation frame was requested during this (sub)paint.
    pub fn needs_frame(&self) -> bool {
        self.needs_frame
    }

    /// Publish the focused editable's current IME surface during paint.
    ///
    /// The event pass ([`EventCtx::publish_ime_state`]) refreshes the shell's
    /// IME view on every edit, but an *app-driven* controlled change — a
    /// submit clearing the field, applied by the next rebuild rather than by
    /// an event — never crosses the event pass, so the event-published value
    /// goes stale. A focused editable therefore also republishes here, in the
    /// paint that runs after every rebuild, so [`crate::app::RenderRoot::ime_state`]
    /// tracks the field's current text/caret regardless of what drove the
    /// change. Bubbles up through [`ChildPod::paint_child`], mirroring
    /// [`Self::request_frame`].
    pub fn publish_ime_state(&mut self, state: ImeState) {
        self.ime_state = Some(state);
    }

    /// Take the IME surface published during this (sub)paint, if any.
    pub fn take_ime_state(&mut self) -> Option<ImeState> {
        self.ime_state.take()
    }
}

/// The result of a whole [`crate::app::RenderRoot::paint`] pass.
///
/// `needs_frame` is whether any widget advanced animation state during paint and
/// asked (via [`PaintCtx::request_frame`]) to be re-invoked to continue. The
/// shell turns it into another scheduled frame — the desktop shell via
/// `window.request_redraw()`, the mobile shells implicitly through their
/// continuous loop. Mirrors [`crate::event::EventOutcome`]'s `needs_redraw`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PaintOutcome {
    /// Whether the shell should schedule another frame to continue an animation.
    pub needs_frame: bool,
}

/// A retained UI element living in the widget tree.
///
/// `Widget: Any` so the render root can downcast a boxed widget back to the
/// concrete element type its originating view produced (needed during rebuild).
/// Implementors get [`Widget::downcast_mut`] for free — no boilerplate method to
/// write.
pub trait Widget: Any {
    /// Choose a size within `bc` and return it. The chosen size must satisfy
    /// `bc` (callers may additionally clamp via [`BoxConstraints::constrain`]).
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size;

    /// Emit draw commands for this widget into `scene`.
    ///
    /// Paint **may** advance a widget's own animation state (e.g. a scroll
    /// fling integrated from a monotonic clock) as a v1 seam. A widget that does
    /// so must call [`PaintCtx::request_frame`] while the animation is still
    /// running so the shell re-invokes paint absent any external event —
    /// otherwise the desktop `ControlFlow::Wait` loop idles and the animation
    /// stalls. It must stop signalling once the animation reaches rest.
    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene);

    /// Handle an input event, optionally mutating application state through
    /// `ctx` and reporting whether it was consumed.
    ///
    /// Defaulted to [`EventResult::Ignored`] so non-interactive widgets (and
    /// every widget written before Phase 4A) are unaffected. Interactive widgets
    /// (Button/Checkbox/Slider, task 44) override this; containers forward to
    /// their [`ChildPod`] children via [`ChildPod::event_child`].
    fn event(&mut self, _ctx: &mut EventCtx, _event: &InputEvent) -> EventResult {
        EventResult::Ignored
    }

    /// Contribute this widget's accessibility node(s) into `ctx` (spec §9,
    /// phase-6c D1).
    ///
    /// Defaulted to a no-op so non-semantic widgets (and every widget written
    /// before this seam) are unaffected — exactly like [`Widget::event`]. A leaf
    /// widget overrides it to call [`SemanticsCtx::push_node`] with its role and
    /// state; a container overrides it to forward to each child via
    /// [`ChildPod::semantics_child`] (a transparent container contributes no node
    /// of its own, only recursion). The pass runs *after* layout, so
    /// [`SemanticsCtx::origin`]/[`SemanticsCtx::size`] carry valid absolute
    /// geometry.
    fn semantics(&self, _ctx: &mut SemanticsCtx) {}
}

impl dyn Widget {
    /// Attempt to downcast this trait object to a concrete widget type.
    ///
    /// Uses trait upcasting (`Widget: Any`, stable since Rust 1.86; the
    /// workspace MSRV is 1.88) — no `unsafe`.
    pub fn downcast_mut<W: Widget>(&mut self) -> Option<&mut W> {
        let any: &mut dyn Any = self;
        any.downcast_mut::<W>()
    }
}

/// A boxed widget is itself a [`Widget`], delegating every pass to its contents.
///
/// This blanket impl is what makes type-erased children work: a
/// [`crate::view::AnyView`]'s element is a `Box<dyn Widget>`, and
/// `View::Element` must implement `Widget` — so the box has to be a widget too.
/// It also lets a `Box<dyn Widget>` be stored inside a [`ChildPod`] like any
/// concrete widget. `Box<dyn Widget>` is `'static` (hence `Any`), so it satisfies
/// the `Widget: Any` bound and can be recovered by
/// [`downcast_mut`](dyn Widget::downcast_mut) during rebuild.
impl Widget for Box<dyn Widget> {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        (**self).layout(ctx, bc)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        (**self).paint(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        (**self).event(ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        (**self).semantics(ctx);
    }
}

/// A container's owned child: a boxed widget plus the layout geometry and
/// capture bookkeeping the container maintains for it.
///
/// Phase 4A containers own their children directly as `ChildPod`s (a `Vec` for
/// Flex/Stack, named fields for Padding/Align) rather than as arena nodes — the
/// [`WidgetTree`](crate::tree::WidgetTree) arena stays single-root. This is a
/// deliberate divergence from the masonry "everything in the arena" model,
/// recorded in the Phase 4A plan: it needs zero global-id plumbing and no
/// disjoint-borrow gymnastics for the features 4A ships. Arena-backed children
/// (for damage tracking / a11y global access) are deferred to a later phase.
///
/// `origin`/`size` are in the **container's** local coordinate space;
/// [`ChildPod::event_child`] translates events into the child's local space and
/// [`ChildPod::paint_child`] offsets the child's paint origin accordingly.
pub struct ChildPod {
    widget: Box<dyn Widget>,
    origin: Point,
    size: Size,
    /// Set when the child captured the pointer, so the container can route
    /// subsequent moves/releases straight to it (capture-by-recorded-path).
    /// Cleared by the container on `Up`/`Cancel` via [`ChildPod::set_active`].
    active: bool,
    /// Set when the child holds the focus path, so the container can route
    /// keyboard/IME events straight to it with no hit test (focus is the
    /// second recorded path, a mirror of `active`). Maintained by
    /// [`ChildPod::event_child`] on a `focus_requested`/`focus_released` bubble.
    focused: bool,
}

impl ChildPod {
    /// Wrap a freshly built child widget at the origin, with zero size until its
    /// first layout.
    pub fn new(widget: Box<dyn Widget>) -> Self {
        Self {
            widget,
            origin: Point::ZERO,
            size: Size::ZERO,
            active: false,
            focused: false,
        }
    }

    /// Shared access to the boxed child widget.
    pub fn widget(&self) -> &dyn Widget {
        &*self.widget
    }

    /// Mutable access to the boxed child widget (e.g. to downcast during a
    /// container's own rebuild).
    pub fn widget_mut(&mut self) -> &mut dyn Widget {
        &mut *self.widget
    }

    /// Replace the boxed child widget (used when a type-changing rebuild swaps
    /// the underlying widget).
    pub fn set_widget(&mut self, widget: Box<dyn Widget>) {
        self.widget = widget;
    }

    /// The child's origin in the container's coordinate space.
    pub fn origin(&self) -> Point {
        self.origin
    }

    /// The child's resolved size (valid after [`ChildPod::layout_child`]).
    pub fn size(&self) -> Size {
        self.size
    }

    /// Place the child at `origin` within the container's coordinate space.
    pub fn set_origin(&mut self, origin: Point) {
        self.origin = origin;
    }

    /// Whether this child currently holds the recorded active (captured) path.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Set (or clear) the recorded active path — the container clears this on
    /// `Up`/`Cancel` when capture auto-releases.
    pub fn set_active(&mut self, active: bool) {
        self.active = active;
    }

    /// Whether this child currently holds the recorded focus path — keyboard/IME
    /// events route straight to it (the focus mirror of [`ChildPod::is_active`]).
    pub fn is_focused(&self) -> bool {
        self.focused
    }

    /// Set (or clear) the recorded focus path. Containers clear it on
    /// blur-on-outside-tap and set it when a child requests focus; the flag is
    /// maintained automatically by [`ChildPod::event_child`] on a
    /// `focus_requested`/`focus_released` bubble.
    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    /// Lay the child out under `bc`, recording and returning its chosen size.
    pub fn layout_child(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self.widget.layout(ctx, bc);
        self.size = size;
        size
    }

    /// Paint the child, offsetting its paint origin by the container's origin
    /// (`ctx.origin()`) so the child draws at its absolute position.
    ///
    /// Bubbles the child's animation-continuation request ([`PaintCtx::needs_frame`])
    /// back into the parent `ctx`, mirroring [`ChildPod::event_child`]'s absorb of
    /// the child's redraw/capture flags — so a nested flinging widget keeps the
    /// whole tree's frames coming.
    pub fn paint_child(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let child_origin = ctx.origin() + self.origin.to_vec2();
        let mut child_ctx = PaintCtx::new(child_origin, self.size);
        // Thread the shared shell clock down unchanged so every widget in the
        // frame advances animations against one consistent timestamp.
        child_ctx.set_frame_time(ctx.frame_time());
        // Thread the app's active theme down unchanged (copied ref, so the child
        // context holds no borrow of the parent), mirroring the clock.
        child_ctx.set_theme(ctx.theme_ref());
        // Thread the pod's recorded focus path into paint (the mirror of how
        // `event_child` seeds the child `EventCtx`), so a focus-dependent widget
        // observes a container-routed blur that never reached its `event()`.
        // COMPOSED with the ancestor's paint focus: paint descends into every
        // child unconditionally (no focus-routing gate like the event pass), and
        // a container-routed blur only clears the focused pod at the
        // nearest-common-ancestor link — flags deeper in the blurred subtree
        // legitimately go stale. ANDing with `ctx.has_focus()` makes any cleared
        // link force `has_focus == false` for the whole subtree below it,
        // matching the effective gating focus-path routing gives events.
        child_ctx.set_has_focus(self.focused && ctx.has_focus());
        self.widget.paint(&mut child_ctx, scene);
        if child_ctx.needs_frame() {
            ctx.request_frame();
        }
        // Bubble a focused editable's republished IME surface up the paint path,
        // so `RenderRoot::paint` can refresh the shell-facing state after a
        // rebuild-driven controlled change (see `PaintCtx::publish_ime_state`).
        if let Some(ime) = child_ctx.take_ime_state() {
            ctx.publish_ime_state(ime);
        }
    }

    /// Collect the child's semantics, translating the current absolute origin
    /// into the child's space exactly like [`ChildPod::paint_child`] offsets its
    /// paint origin (`child_origin = ctx.origin() + self.origin`).
    ///
    /// A container's [`Widget::semantics`] calls this for each of its
    /// [`ChildPod`]s so their nodes attach under the container's node (or, for a
    /// transparent container, under whatever encloses it — see
    /// [`crate::semantics`]).
    pub fn semantics_child(&self, ctx: &mut SemanticsCtx) {
        ctx.descend(self.origin.to_vec2(), self.size, |ctx| {
            self.widget.semantics(ctx);
        });
    }

    /// Route an event into the child, translating its position into the child's
    /// local space and folding the child's redraw/capture flags back into `ctx`.
    ///
    /// If the child captured the pointer, this records the active path
    /// ([`ChildPod::is_active`]); the container clears it on `Up`/`Cancel`.
    ///
    /// Containers should not call this directly gated on an ad-hoc
    /// `contains()` check — that drops a captured gesture the instant it moves
    /// outside the child's bounds. Route through `forgekit-widgets`'
    /// `route_event`/`route_event_single` helpers instead, which check
    /// [`ChildPod::is_active`] first and forward unconditionally to a captured
    /// child.
    pub fn event_child(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let local = event.translated(-self.origin.to_vec2());
        let (captured, focus_req, focus_rel, redraw, ime, result) = {
            let mut child_ctx = ctx.child_ctx(self.origin, self.size, self.focused);
            let result = self.widget.event(&mut child_ctx, &local);
            (
                child_ctx.is_pointer_captured(),
                child_ctx.is_focus_requested(),
                child_ctx.is_focus_released(),
                child_ctx.needs_redraw(),
                child_ctx.take_ime_state(),
                result,
            )
        };
        if captured {
            self.active = true;
        }
        // Focus is the second recorded path, maintained exactly like `active`: a
        // `focus_requested` bubble records this child as the focused one; a
        // `focus_released` bubble drops it. A request wins over a release in the
        // rare case both fire in one dispatch (a re-focus supersedes a blur).
        if focus_rel {
            self.focused = false;
        }
        if focus_req {
            self.focused = true;
        }
        ctx.absorb_child(redraw, captured, focus_req, focus_rel, ime);
        result
    }

    /// Whether `point` (in the container's coordinate space) lies within this
    /// child's bounds — the container's hit test.
    ///
    /// This is only the *initial* hit test (deciding which child a fresh
    /// `Down`/first contact goes to). Once a child has captured the pointer
    /// ([`ChildPod::is_active`]), subsequent events must bypass this check and
    /// go straight to the captured child regardless of where the point now
    /// falls — see `forgekit-widgets`' `route_event`/`route_event_single`.
    pub fn contains(&self, point: Point) -> bool {
        point.x >= self.origin.x
            && point.x < self.origin.x + self.size.width
            && point.y >= self.origin.y
            && point.y < self.origin.y + self.size.height
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{PointerButton, PointerEvent, PointerPhase};

    /// A leaf widget that paints a filled box of a fixed intrinsic size.
    struct FixedBox {
        intrinsic: Size,
    }

    impl Widget for FixedBox {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(self.intrinsic)
        }

        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            scene.fill_rect(ctx.origin(), ctx.size(), Color::BLACK);
        }
    }

    /// A leaf widget that advances no state but signals it wants another frame
    /// on every paint — stands in for an animating widget (e.g. a fling).
    struct Animator;

    impl Widget for Animator {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }

        fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
            ctx.request_frame();
        }
    }

    /// A scene recorder used to assert paint output without any GPU dependency.
    #[derive(Default)]
    struct RecordingScene {
        rects: Vec<(Point, Size)>,
        texts: Vec<(Point, String)>,
    }

    impl PaintScene for RecordingScene {
        fn fill_rect(&mut self, origin: Point, size: Size, _color: Color) {
            self.rects.push((origin, size));
        }
        fn draw_text(&mut self, origin: Point, text: &str) {
            self.texts.push((origin, text.to_string()));
        }
    }

    /// A leaf widget that records the local position of the last event it saw,
    /// mutates a `u32` app state, and optionally captures the pointer on `Down`.
    struct Probe {
        last_pos: Option<Point>,
        capture_on_down: bool,
    }

    impl Widget for Probe {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            self.last_pos = Some(event.position());
            *ctx.state_mut::<u32>() += 1;
            ctx.request_redraw();
            if self.capture_on_down
                && matches!(
                    event,
                    InputEvent::Pointer(PointerEvent {
                        phase: PointerPhase::Down,
                        ..
                    })
                )
            {
                ctx.capture_pointer();
            }
            EventResult::Handled
        }
    }

    fn down(x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Down,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    /// A leaf that requests focus on `Down`, releases it on Escape, records
    /// whether it saw a `Key`/`Ime` event, and reports whether it had focus when
    /// the last event arrived.
    #[derive(Default)]
    struct FocusProbe {
        saw_key: bool,
        had_focus_on_key: bool,
    }

    impl Widget for FocusProbe {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &crate::event::InputEvent) -> EventResult {
            match event {
                InputEvent::Pointer(PointerEvent {
                    phase: PointerPhase::Down,
                    ..
                }) => {
                    ctx.request_focus();
                    EventResult::Handled
                }
                InputEvent::Key(_) | InputEvent::Ime(_) => {
                    self.saw_key = true;
                    self.had_focus_on_key = ctx.has_focus();
                    EventResult::Handled
                }
                _ => EventResult::Ignored,
            }
        }
    }

    fn key_enter() -> InputEvent {
        InputEvent::Key(crate::event::KeyEvent {
            key: crate::event::Key::Named(crate::event::NamedKey::Enter),
            modifiers: crate::event::Modifiers::default(),
            repeat: false,
        })
    }

    #[test]
    fn event_child_records_focus_on_request() {
        let mut pod = ChildPod::new(Box::new(FocusProbe::default()));
        pod.set_origin(Point::new(5.0, 5.0));
        assert!(!pod.is_focused());

        let mut count = 0u32;
        let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::new(50.0, 50.0));
        pod.event_child(&mut ctx, &down(6.0, 6.0));

        // The child requested focus → the pod records the focus path and it
        // bubbles up into the parent context.
        assert!(pod.is_focused());
        assert!(ctx.is_focus_requested());
    }

    #[test]
    fn event_child_threads_has_focus_into_child() {
        // A focused pod seeds `has_focus` on the child dispatch; a Key event
        // reaching a focused child sees `has_focus() == true`.
        let mut pod = ChildPod::new(Box::new(FocusProbe::default()));
        pod.set_focused(true);

        let mut count = 0u32;
        let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::new(10.0, 10.0));
        pod.event_child(&mut ctx, &key_enter());

        let probe = pod.widget_mut().downcast_mut::<FocusProbe>().unwrap();
        assert!(probe.saw_key);
        assert!(probe.had_focus_on_key);
    }

    #[test]
    fn widget_layout_respects_constraints() {
        let mut w = FixedBox {
            intrinsic: Size::new(1000.0, 1000.0),
        };
        let mut ctx = LayoutCtx::new();
        let bc = BoxConstraints::loose(Size::new(200.0, 100.0));
        assert_eq!(w.layout(&mut ctx, &bc), Size::new(200.0, 100.0));
    }

    #[test]
    fn widget_paint_emits_into_scene() {
        let mut w = FixedBox {
            intrinsic: Size::new(50.0, 20.0),
        };
        let mut scene = RecordingScene::default();
        let mut ctx = PaintCtx::new(Point::new(5.0, 7.0), Size::new(50.0, 20.0));
        w.paint(&mut ctx, &mut scene);
        assert_eq!(
            scene.rects,
            vec![(Point::new(5.0, 7.0), Size::new(50.0, 20.0))]
        );
    }

    #[test]
    fn downcast_recovers_concrete_widget() {
        let mut boxed: Box<dyn Widget> = Box::new(FixedBox {
            intrinsic: Size::new(3.0, 4.0),
        });
        let concrete = boxed.downcast_mut::<FixedBox>().expect("downcast");
        assert_eq!(concrete.intrinsic, Size::new(3.0, 4.0));
    }

    #[test]
    fn boxed_widget_delegates_every_pass() {
        // The `Box<dyn Widget>: Widget` blanket impl must forward layout/paint/event.
        let mut boxed: Box<dyn Widget> = Box::new(Probe {
            last_pos: None,
            capture_on_down: false,
        });
        let mut ctx = LayoutCtx::new();
        assert_eq!(
            boxed.layout(&mut ctx, &BoxConstraints::tight(Size::new(4.0, 5.0))),
            Size::new(4.0, 5.0)
        );
        let mut count = 0u32;
        let mut ectx = EventCtx::new(&mut count, Point::ZERO, Size::new(4.0, 5.0));
        assert_eq!(
            boxed.event(&mut ectx, &down(1.0, 1.0)),
            EventResult::Handled
        );
        assert_eq!(count, 1);
    }

    #[test]
    fn child_pod_layout_and_paint_offset_by_origin() {
        let mut pod = ChildPod::new(Box::new(FixedBox {
            intrinsic: Size::new(30.0, 10.0),
        }));
        pod.set_origin(Point::new(12.0, 8.0));

        let mut lctx = LayoutCtx::new();
        let size = pod.layout_child(&mut lctx, &BoxConstraints::loose(Size::new(100.0, 100.0)));
        assert_eq!(size, Size::new(30.0, 10.0));
        assert_eq!(pod.size(), Size::new(30.0, 10.0));

        // Paint under a container placed at (100, 200): child draws at the sum.
        let mut scene = RecordingScene::default();
        let mut pctx = PaintCtx::new(Point::new(100.0, 200.0), Size::new(300.0, 300.0));
        pod.paint_child(&mut pctx, &mut scene);
        assert_eq!(
            scene.rects,
            vec![(Point::new(112.0, 208.0), Size::new(30.0, 10.0))]
        );
    }

    #[test]
    fn child_pod_translates_event_into_child_space() {
        let mut pod = ChildPod::new(Box::new(Probe {
            last_pos: None,
            capture_on_down: false,
        }));
        pod.set_origin(Point::new(10.0, 20.0));

        let mut count = 0u32;
        let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::new(200.0, 200.0));
        // Event at container-space (25, 35) lands at child-local (15, 15).
        let result = pod.event_child(&mut ctx, &down(25.0, 35.0));
        assert_eq!(result, EventResult::Handled);
        assert_eq!(count, 1); // state mutated through the reborrowed context
        let probe = pod.widget_mut().downcast_mut::<Probe>().unwrap();
        assert_eq!(probe.last_pos, Some(Point::new(15.0, 15.0)));
    }

    #[test]
    fn child_pod_propagates_capture_flag() {
        let mut pod = ChildPod::new(Box::new(Probe {
            last_pos: None,
            capture_on_down: true,
        }));
        pod.set_origin(Point::new(5.0, 5.0));
        assert!(!pod.is_active());

        let mut count = 0u32;
        let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::new(50.0, 50.0));
        pod.event_child(&mut ctx, &down(6.0, 6.0));

        // The child captured → the pod records the active path and the flag
        // bubbles up to the parent context.
        assert!(pod.is_active());
        assert!(ctx.is_pointer_captured());
        assert!(ctx.needs_redraw());
    }

    #[test]
    fn child_pod_bubbles_needs_frame_from_child_paint() {
        // A non-animating child leaves the parent's frame flag clear.
        let mut still = ChildPod::new(Box::new(FixedBox {
            intrinsic: Size::new(10.0, 10.0),
        }));
        let mut lctx = LayoutCtx::new();
        still.layout_child(&mut lctx, &BoxConstraints::tight(Size::new(10.0, 10.0)));
        let mut scene = RecordingScene::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        still.paint_child(&mut pctx, &mut scene);
        assert!(!pctx.needs_frame());

        // An animating child bubbles its request into the parent context.
        let mut anim = ChildPod::new(Box::new(Animator));
        anim.layout_child(&mut lctx, &BoxConstraints::tight(Size::new(10.0, 10.0)));
        let mut pctx2 = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        assert!(!pctx2.needs_frame());
        anim.paint_child(&mut pctx2, &mut scene);
        assert!(pctx2.needs_frame());
    }

    #[test]
    fn child_pod_paint_seeds_has_focus_composed_with_ancestor() {
        use std::cell::Cell;
        use std::rc::Rc;

        // A probe recording what `ctx.has_focus()` it observed during paint.
        struct FocusProbe(Rc<Cell<Option<bool>>>);
        impl Widget for FocusProbe {
            fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
                bc.constrain(Size::new(10.0, 10.0))
            }
            fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
                self.0.set(Some(ctx.has_focus()));
            }
        }

        let seen = Rc::new(Cell::new(None));
        let mut pod = ChildPod::new(Box::new(FocusProbe(seen.clone())));
        let mut lctx = LayoutCtx::new();
        pod.layout_child(&mut lctx, &BoxConstraints::tight(Size::new(10.0, 10.0)));
        let mut scene = RecordingScene::default();

        // Focused pod under a focused ancestor: the child observes focus.
        pod.set_focused(true);
        let mut focused_parent = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        focused_parent.set_has_focus(true);
        pod.paint_child(&mut focused_parent, &mut scene);
        assert_eq!(seen.get(), Some(true));

        // Focused pod under a BLURRED ancestor (the stale-deep-flag case): the
        // cleared ancestor link must force `has_focus == false` below it.
        let mut blurred_parent = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        blurred_parent.set_has_focus(false);
        pod.paint_child(&mut blurred_parent, &mut scene);
        assert_eq!(seen.get(), Some(false));

        // Unfocused pod under a focused ancestor stays unfocused.
        pod.set_focused(false);
        let mut focused_parent2 = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        focused_parent2.set_has_focus(true);
        pod.paint_child(&mut focused_parent2, &mut scene);
        assert_eq!(seen.get(), Some(false));
    }

    #[test]
    fn child_pod_contains_uses_container_space_bounds() {
        let mut pod = ChildPod::new(Box::new(FixedBox {
            intrinsic: Size::ZERO,
        }));
        pod.set_origin(Point::new(10.0, 10.0));
        let mut lctx = LayoutCtx::new();
        pod.layout_child(&mut lctx, &BoxConstraints::tight(Size::new(20.0, 20.0)));

        assert!(pod.contains(Point::new(10.0, 10.0))); // top-left inclusive
        assert!(pod.contains(Point::new(29.9, 29.9)));
        assert!(!pod.contains(Point::new(30.0, 30.0))); // bottom-right exclusive
        assert!(!pod.contains(Point::new(9.9, 15.0)));
    }

    /// A scene recorder that overrides the task-08 shadow/layer additions, to
    /// prove they reach an implementor that opts in.
    #[derive(Default)]
    struct ShadowLayerRecordingScene {
        shadows: Vec<(Point, Size, f64, f64, Color)>,
        layers: Vec<(Point, Size, f32)>,
        pops: u32,
    }

    impl PaintScene for ShadowLayerRecordingScene {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}

        fn draw_shadow(
            &mut self,
            origin: Point,
            size: Size,
            radius: f64,
            std_dev: f64,
            color: Color,
        ) {
            self.shadows.push((origin, size, radius, std_dev, color));
        }

        fn push_layer(&mut self, origin: Point, size: Size, alpha: f32) {
            self.layers.push((origin, size, alpha));
        }

        fn pop_layer(&mut self) {
            self.pops += 1;
        }
    }

    #[test]
    fn draw_shadow_and_push_layer_are_no_ops_when_not_overridden() {
        // `RecordingScene` (defined above) does not override the task-08
        // additions — the trait's default no-op bodies must compile
        // unchanged and simply do nothing.
        let mut scene = RecordingScene::default();
        scene.draw_shadow(Point::ZERO, Size::new(10.0, 10.0), 4.0, 2.0, Color::BLACK);
        scene.push_layer(Point::ZERO, Size::new(10.0, 10.0), 0.5);
        scene.pop_layer();
        assert!(scene.rects.is_empty());
        assert!(scene.texts.is_empty());
    }

    #[test]
    fn fill_path_and_stroke_path_are_no_ops_when_not_overridden() {
        // `RecordingScene` does not override the task-05 path additions
        // either — the trait's default no-op bodies must compile unchanged.
        let mut scene = RecordingScene::default();
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((1.0, 0.0));
        scene.fill_path(Point::ZERO, &path, &Brush::Solid(Color::BLACK));
        scene.stroke_path(Point::ZERO, &path, 2.0, &Brush::Solid(Color::BLACK));
        assert!(scene.rects.is_empty());
        assert!(scene.texts.is_empty());
    }

    /// A scene recorder overriding the task-05 path additions, proving they
    /// reach an implementor that opts in.
    #[derive(Default)]
    struct PathRecordingScene {
        fills: Vec<(Point, BezPath)>,
        strokes: Vec<(Point, BezPath, f64)>,
    }

    impl PaintScene for PathRecordingScene {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}

        fn fill_path(&mut self, origin: Point, path: &BezPath, _brush: &Brush) {
            self.fills.push((origin, path.clone()));
        }

        fn stroke_path(&mut self, origin: Point, path: &BezPath, width: f64, _brush: &Brush) {
            self.strokes.push((origin, path.clone(), width));
        }
    }

    #[test]
    fn fill_path_and_stroke_path_reach_an_overriding_implementor() {
        let mut scene = PathRecordingScene::default();
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((5.0, 0.0));

        scene.fill_path(Point::new(1.0, 2.0), &path, &Brush::Solid(Color::BLACK));
        scene.stroke_path(
            Point::new(3.0, 4.0),
            &path,
            1.5,
            &Brush::Solid(Color::BLACK),
        );

        assert_eq!(scene.fills.len(), 1);
        assert_eq!(scene.fills[0].0, Point::new(1.0, 2.0));
        assert_eq!(scene.fills[0].1, path);

        assert_eq!(scene.strokes.len(), 1);
        assert_eq!(scene.strokes[0].0, Point::new(3.0, 4.0));
        assert_eq!(scene.strokes[0].1, path);
        assert_eq!(scene.strokes[0].2, 1.5);
    }

    #[test]
    fn path_at_translates_path_points_by_origin() {
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((5.0, 0.0));

        let translated = path_at(Point::new(10.0, 20.0), &path);
        let mut expected = BezPath::new();
        expected.move_to((10.0, 20.0));
        expected.line_to((15.0, 20.0));
        assert_eq!(translated, expected);
    }

    /// A dummy theme type, standing in for `forgekit_theme::Theme` — proving the
    /// type-erased theme slot works for *any* `'static` type, not just the real
    /// theme (`forgekit-core` never names it).
    #[derive(Debug, PartialEq)]
    struct TestTheme {
        accent: u32,
    }

    #[test]
    fn layout_ctx_theme_as_recovers_threaded_theme() {
        let theme = TestTheme { accent: 7 };
        let ctx = LayoutCtx::new().with_theme(&theme);
        assert_eq!(ctx.theme_as::<TestTheme>(), Some(&TestTheme { accent: 7 }));
    }

    #[test]
    fn layout_ctx_theme_as_is_none_without_a_theme() {
        let ctx = LayoutCtx::new();
        assert!(ctx.theme_as::<TestTheme>().is_none());
    }

    #[test]
    fn layout_ctx_theme_as_is_none_on_type_mismatch() {
        let theme = TestTheme { accent: 1 };
        let ctx = LayoutCtx::new().with_theme(&theme);
        // A downcast to the wrong type yields `None`, never a panic.
        assert!(ctx.theme_as::<u32>().is_none());
    }

    #[test]
    fn paint_ctx_theme_as_recovers_threaded_theme() {
        let theme = TestTheme { accent: 9 };
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(1.0, 1.0));
        ctx.set_theme(Some(&theme));
        assert_eq!(ctx.theme_as::<TestTheme>(), Some(&TestTheme { accent: 9 }));
    }

    #[test]
    fn paint_ctx_theme_as_is_none_without_a_theme() {
        let ctx = PaintCtx::new(Point::ZERO, Size::new(1.0, 1.0));
        assert!(ctx.theme_as::<TestTheme>().is_none());
    }

    #[test]
    fn child_pod_paint_threads_theme_into_child() {
        use std::cell::Cell;
        use std::rc::Rc;

        // A probe recording the accent it recovered from the paint context's
        // threaded theme (or `None` if no theme reached it).
        struct ThemeProbe(Rc<Cell<Option<u32>>>);
        impl Widget for ThemeProbe {
            fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
                bc.constrain(Size::new(10.0, 10.0))
            }
            fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
                self.0.set(ctx.theme_as::<TestTheme>().map(|t| t.accent));
            }
        }

        let seen = Rc::new(Cell::new(None));
        let mut pod = ChildPod::new(Box::new(ThemeProbe(seen.clone())));
        let mut lctx = LayoutCtx::new();
        pod.layout_child(&mut lctx, &BoxConstraints::tight(Size::new(10.0, 10.0)));
        let mut scene = RecordingScene::default();

        // A parent carrying a theme threads it into the child paint.
        let theme = TestTheme { accent: 42 };
        let mut parent = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        parent.set_theme(Some(&theme));
        pod.paint_child(&mut parent, &mut scene);
        assert_eq!(seen.get(), Some(42));

        // A parent with no theme leaves the child's accessor empty.
        let mut bare = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        pod.paint_child(&mut bare, &mut scene);
        assert_eq!(seen.get(), None);
    }

    #[test]
    fn draw_shadow_and_push_layer_reach_an_overriding_implementor() {
        let mut scene = ShadowLayerRecordingScene::default();
        let origin = Point::new(3.0, 4.0);
        let size = Size::new(20.0, 12.0);
        scene.draw_shadow(origin, size, 6.0, 3.0, Color::BLACK);
        scene.push_layer(origin, size, 0.25);
        scene.pop_layer();

        assert_eq!(scene.shadows, vec![(origin, size, 6.0, 3.0, Color::BLACK)]);
        assert_eq!(scene.layers, vec![(origin, size, 0.25)]);
        assert_eq!(scene.pops, 1);
    }
}
