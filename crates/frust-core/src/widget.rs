//! Layer 2: the retained [`Widget`] trait and its layout/paint contexts.
//!
//! Widgets are the long-lived counterpart to [`crate::view::View`]s. A view is
//! rebuilt every frame; the widget it produced persists in the arena and is
//! mutated in place. Widgets participate in two passes:
//!
//! * **layout** — receive [`BoxConstraints`] and return a chosen [`Size`].
//! * **paint** — emit draw commands into a scene.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use frust_scene::{GlyphRun, SceneBuilder, ShaderProgram};
use kurbo::{Affine, BezPath, Point, Rect, Size};
use peniko::{Brush, Color};

use crate::anim::FrameTime;
use crate::event::{EventCtx, EventResult, ImeState, InputEvent};
use crate::insets::WindowInsets;
use crate::layout::BoxConstraints;
use crate::semantics::SemanticsCtx;

/// The renderer-agnostic paint target a widget draws into.
///
/// This trait was introduced as a **local stand-in** for
/// `frust_scene::SceneBuilder` while the scene crate was still a stub, and the
/// two were later reconciled *additively*: rather than churn the `Widget::paint`
/// signature (and every widget/test written against it), `SceneBuilder` now
/// [implements this trait](#impl-PaintScene-for-SceneBuilder), so widgets keep
/// painting through `&mut dyn PaintScene` while the shell hands them a real
/// `SceneBuilder` whose commands reach the GPU backend.
///
/// The original `fill_rect`/`draw_text` shape is retained for source
/// compatibility with existing recorder-style test scenes; real text rendering
/// goes through [`PaintScene::draw_glyph_run`], which carries shaped glyphs from
/// `frust-text`.
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

    /// Push a clip with uniformly rounded corners (at `origin`/`size`, corner
    /// `radius`) onto the backend clip stack; subsequent draws are clipped to
    /// the rounded shape until the matching [`PaintScene::pop_clip`] — the same
    /// pop [`PaintScene::push_clip`] uses, since there is one clip stack.
    ///
    /// Lets paint code express a radiused mask over content a rectangular clip
    /// cannot shape — a rounded bitmap (avatar/thumbnail) being the motivating
    /// case. Defaulted to a no-op so recorder scenes stay valid; the
    /// `SceneBuilder` implementation honors it by recording a real
    /// [`frust_scene::Command::PushClipRounded`]/[`frust_scene::Command::PopClip`]
    /// pair.
    fn push_clip_rounded(&mut self, _origin: Point, _size: Size, _radius: f64) {}

    /// Pop the most recently pushed clip, rectangular or rounded. Defaulted to
    /// a no-op; see [`PaintScene::push_clip`].
    fn pop_clip(&mut self) {}

    /// Emit a run of *unshaped* text anchored at `origin`.
    ///
    /// This records intent only — glyph shaping lives in `frust-text`.
    /// Real rendering uses [`PaintScene::draw_glyph_run`]; the
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
    /// A single additive method (added for `frust-widgets::Image`) on this
    /// otherwise layer-2 trait — authorized because `Command::Image`'s
    /// `peniko::ImageData` payload has to reach the scene through the same
    /// `&mut dyn PaintScene` seam every other paint call uses. Defaulted to a
    /// no-op so pre-existing recorder scenes stay valid; the `SceneBuilder`
    /// implementation records a real image command.
    fn draw_image(&mut self, _data: &peniko::ImageData, _dest: Rect) {}

    /// Draw a fragment-shader-filled rectangle, scaled to fill `dest`.
    ///
    /// An additive method (added for the shader-showcase feature) on this otherwise layer-2
    /// trait — authorized because the shader program and destination have to
    /// reach the scene through the same `&mut dyn PaintScene` seam every other
    /// paint call uses. `program` carries the WGSL source and process-unique
    /// id; `time` is seconds, app-supplied. Defaulted to a no-op so pre-existing
    /// recorder scenes stay valid; the `SceneBuilder` implementation records a
    /// real shader-quad command.
    ///
    /// # Cache-once contract
    ///
    /// `program` must be a retained, already-created `ShaderProgram` handle
    /// (see `ShaderProgram::new`'s own doc for the full contract) — never a
    /// fresh one minted inline in the call that invokes this method. This
    /// method is reached from [`Widget::paint`], which re-runs every frame,
    /// so a `ShaderProgram::new` call written directly at a `draw_shader`
    /// call site there mints a new process-unique id (and therefore a new
    /// GPU pipeline cache miss) every frame; the same applies to a
    /// [`crate::component::Component`]'s `build`, which re-runs every
    /// rebuild. Build the `ShaderProgram` once — in a `Component`'s `init`,
    /// or other retained widget state — and clone the handle in; a
    /// [`View::build`](crate::view::View::build) call, by contrast, runs
    /// exactly once per widget instance and is a correct place to construct
    /// one.
    fn draw_shader(&mut self, _program: &ShaderProgram, _dest: Rect, _time: f32) {}

    /// Draw a gaussian-blurred rounded-rectangle elevation shadow (an
    /// approximation of a CSS `box-shadow`) at `origin`/`size`.
    ///
    /// Defaulted to a no-op so pre-existing recorder scenes stay valid; the
    /// `SceneBuilder` implementation records a real
    /// [`frust_scene::Command::BlurredRoundedRect`].
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
    /// [`frust_scene::Command::RoundedRect`]'s zero-radius sibling
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
    /// [`frust_scene::Command::RoundedRect`] carrying the brush.
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
    /// [`frust_scene::Command::PushLayer`]/[`frust_scene::Command::PopLayer`]
    /// pair, nesting correctly with [`PaintScene::push_clip`]/[`PaintScene::pop_clip`].
    fn push_layer(&mut self, _origin: Point, _size: Size, _alpha: f32) {}

    /// Pop the most recently pushed layer. Defaulted to a no-op; see
    /// [`PaintScene::push_layer`].
    fn pop_layer(&mut self) {}

    /// Clear an axis-aligned rectangle (at `origin`/`size`) to full
    /// transparency (alpha 0), erasing everything already painted below it in
    /// this scene — a real destination-clearing composite, not merely skipping
    /// paint over the region.
    ///
    /// The platform-view hole-punch (`frust-widgets`' `PlatformViewWidget`) is
    /// the sole v1 consumer: on a translucent (Mode B) surface a slot punches
    /// its rect so an opaque app backdrop painted below it (the catalog's
    /// `AppBackground`) doesn't seal the hole the hosted native view shows
    /// through. Gated on [`PaintCtx::is_translucent`] by the widget — clearing
    /// on an opaque surface would erase real app content, and the clear is
    /// disregarded there anyway (see [`frust_scene::Command::ClearRect`]).
    ///
    /// Defaulted to a no-op so pre-existing recorder scenes stay valid; the
    /// `SceneBuilder` implementation records a real
    /// [`frust_scene::Command::ClearRect`].
    fn clear_rect(&mut self, _origin: Point, _size: Size) {}

    /// Fill an arbitrary vector path (e.g. an arc — see
    /// [`frust_scene::arc_path`]) at `origin`, using the nonzero winding
    /// rule and `brush`.
    ///
    /// `path` is in the widget's local coordinate space; `origin` translates
    /// it into the parent's space, mirroring every other `PaintScene`
    /// method's origin convention. Defaulted to a
    /// no-op so pre-existing recorder scenes stay valid; the `SceneBuilder`
    /// implementation records a real [`frust_scene::Command::Path`].
    fn fill_path(&mut self, _origin: Point, _path: &BezPath, _brush: &Brush) {}

    /// Stroke an arbitrary vector path (e.g. an arc) at `origin` with `width`
    /// and round caps/joins, using `brush`.
    ///
    /// Defaulted to a no-op so pre-existing recorder scenes stay valid; see
    /// [`PaintScene::fill_path`].
    fn stroke_path(&mut self, _origin: Point, _path: &BezPath, _width: f64, _brush: &Brush) {}

    /// Push an affine `transform`, composed with the current one, onto the
    /// backend transform stack; subsequent draws are transformed until the
    /// matching [`PaintScene::pop_transform`].
    ///
    /// Defaulted to a no-op so pre-existing recorder scenes stay valid; the
    /// `SceneBuilder` implementation composes and records it. Unlike the
    /// origin-offset convention every other method uses (a pure translation),
    /// this is the one seam that also carries scale/rotation — the
    /// shared-element ("hero") morph is the first consumer, repainting a tagged
    /// subtree under a rect→rect transform (position **and** scale) so it
    /// morphs between two pages during a navigation transition.
    fn push_transform(&mut self, _transform: Affine) {}

    /// Pop the most recently pushed transform, restoring the previous one.
    /// Defaulted to a no-op; see [`PaintScene::push_transform`].
    fn pop_transform(&mut self) {}
}

/// Bridges the provisional [`PaintScene`] boundary onto the real
/// `frust_scene::SceneBuilder`.
///
/// Widgets paint through `&mut dyn PaintScene`; the desktop shell
/// hands them a `SceneBuilder`, so filled rectangles and shaped glyph runs land
/// in the display list under the builder's current transform. Unshaped
/// [`PaintScene::draw_text`] is intentionally dropped here — text must be shaped
/// (by `frust-text`) into glyph runs before it can be drawn.
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

    fn push_clip_rounded(&mut self, origin: Point, size: Size, radius: f64) {
        SceneBuilder::push_clip_rounded(self, rect_at(origin, size), radius);
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

    fn draw_shader(&mut self, program: &ShaderProgram, dest: Rect, time: f32) {
        SceneBuilder::draw_shader(self, program, dest, time);
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

    fn clear_rect(&mut self, origin: Point, size: Size) {
        SceneBuilder::clear_rect(self, rect_at(origin, size));
    }

    fn fill_path(&mut self, origin: Point, path: &BezPath, brush: &Brush) {
        SceneBuilder::fill_path(self, path_at(origin, path), brush.clone());
    }

    fn stroke_path(&mut self, origin: Point, path: &BezPath, width: f64, brush: &Brush) {
        SceneBuilder::stroke_path(self, path_at(origin, path), width, brush.clone());
    }

    fn push_transform(&mut self, transform: Affine) {
        SceneBuilder::push_transform(self, transform);
    }

    fn pop_transform(&mut self) {
        SceneBuilder::pop_transform(self);
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
/// layout, plus the app's active theme (design tokens).
/// Both resources are **type-erased** (`&mut dyn Any` / `&dyn Any`) so
/// `frust-core` stays independent of `frust-text` (and thus of parley)
/// and of `frust-theme`; text widgets recover the shaping context with
/// [`LayoutCtx::text_context`] and themed widgets recover the theme with
/// [`LayoutCtx::theme_as`].
pub struct LayoutCtx<'a> {
    text_ctx: Option<&'a mut dyn Any>,
    /// The app's active theme, threaded down type-erased by the render root so
    /// this crate needs no `frust-theme` dependency. `None` in bare-core
    /// tests and pre-theme apps — a *supported* state (unlike the text context,
    /// whose absence at a text widget is a wiring bug), so [`LayoutCtx::theme_as`]
    /// returns `Option` rather than panicking.
    theme: Option<&'a dyn Any>,
    /// The window's insets ([`WindowInsets`]), threaded down by the render root
    /// (see [`crate::app::RenderRoot::set_insets`]). Unlike the theme this is a
    /// concrete core-owned type carried by copy — global (origin-independent,
    /// see the [`crate::insets`] module docs), so the single layout context the
    /// render root threads down carries it unchanged to every widget in the
    /// tree. Defaults to the zero inset in bare-core tests and pre-insets apps.
    window_insets: WindowInsets,
}

impl<'a> LayoutCtx<'a> {
    /// Create a layout context with no shared resources.
    ///
    /// Used by leaf-only unit tests and by containers that never lay out text.
    pub fn new() -> LayoutCtx<'static> {
        LayoutCtx {
            text_ctx: None,
            theme: None,
            window_insets: WindowInsets::default(),
        }
    }

    /// Create a layout context carrying the shared text-shaping context.
    ///
    /// The render root builds this so text widgets can shape their content
    /// during the layout pass; the concrete type is erased to keep this crate
    /// free of a `frust-text` dependency.
    pub fn with_text_context(text_ctx: &'a mut dyn Any) -> Self {
        LayoutCtx {
            text_ctx: Some(text_ctx),
            theme: None,
            window_insets: WindowInsets::default(),
        }
    }

    /// Create a layout context carrying both an optional text-shaping context
    /// and an optional type-erased theme.
    ///
    /// The render root uses this to thread both resources it owns into the
    /// layout pass in one shot (see [`crate::app::RenderRoot::layout`]).
    pub fn with_resources(text_ctx: Option<&'a mut dyn Any>, theme: Option<&'a dyn Any>) -> Self {
        LayoutCtx {
            text_ctx,
            theme,
            window_insets: WindowInsets::default(),
        }
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
    /// wiring bug. Widgets that read `frust_theme::Theme` downcast through
    /// this (or the `Theme::from_layout_ctx` convenience wrapper).
    pub fn theme_as<T: Any>(&self) -> Option<&T> {
        self.theme?.downcast_ref::<T>()
    }

    /// The window's insets ([`WindowInsets`]) for this layout pass (a cheap
    /// copy). Global and origin-independent (see the [`crate::insets`] module
    /// docs), so every widget in the tree reads the same value regardless of its
    /// position; defaults to the zero inset when no shell pushed one. A
    /// `SafeArea` widget insets by [`WindowInsets::padding`].
    pub fn window_insets(&self) -> WindowInsets {
        self.window_insets
    }

    /// Seed the window insets lent by the render root
    /// ([`crate::app::RenderRoot::layout`]). One layout context is threaded down
    /// the whole tree, so this is set once at the root; the insets are global,
    /// so no per-child adjustment is needed.
    pub(crate) fn set_window_insets(&mut self, insets: WindowInsets) {
        self.window_insets = insets;
    }
}

impl Default for LayoutCtx<'static> {
    fn default() -> Self {
        Self::new()
    }
}

/// The *class* of a continuation-frame request a widget makes during paint —
/// how urgent the next frame is, so the mobile frame gate can decide whether it
/// may be paced (see [`crate::app::RenderRoot::paint`] and the frame gate).
///
/// A widget continues an animation by asking for another frame during paint
/// ([`PaintCtx::request_frame`] / [`PaintCtx::request_frame_paced`]); this tag
/// says whether that next frame is user-visible motion that must land on the
/// very next vsync ([`TickClass::Transition`]) or a decorative loop whose cadence
/// can be throttled without a perceptible glitch ([`TickClass::CosmeticLoop`]).
///
/// **Aggregation is a max-lattice**: `Transition` dominates `CosmeticLoop`. Over
/// a whole paint pass, ANY [`TickClass::Transition`] request makes the frame
/// unpaced (must run every vsync, today's behavior); only when *every* request
/// this frame is [`TickClass::CosmeticLoop`] may the gate pace it. No request at
/// all leaves the frame as it is today — the class is only meaningful once a
/// frame was actually requested (see [`PaintCtx::frame_class`]).
///
/// The *gate-side* pacing behavior is implemented separately (the mobile frame
/// gate); this type is only the vocabulary a widget uses to declare intent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TickClass {
    /// A pacable decorative loop (e.g. a skeleton shimmer, an idle pulse) — the
    /// gate may throttle its cadence when this is the *only* class requested
    /// this frame. Never dominates a concurrent [`TickClass::Transition`].
    CosmeticLoop,
    /// User-visible motion that must reproduce every vsync — a page transition,
    /// a fling, a caret blink, a layout animation. Today's `request_frame`
    /// behavior, and the dominant class in the aggregation above.
    Transition,
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
///
/// A frame request also carries a [`TickClass`] (see [`PaintCtx::request_frame`]
/// vs [`PaintCtx::request_frame_paced`]): the aggregate class over the whole
/// paint pass — Transition-dominates-CosmeticLoop — surfaces on
/// [`PaintOutcome::needs_frame_paced_only`] for the mobile frame gate to pace a
/// purely-cosmetic frame.
pub struct PaintCtx<'a> {
    origin: Point,
    size: Size,
    needs_frame: bool,
    /// Whether a widget whose animation changes its *layout* (not just paint)
    /// asked, via [`PaintCtx::request_layout`], to have layout re-run next frame.
    /// Bubbles up through [`ChildPod::paint_child`] exactly like `needs_frame`
    /// and out of [`crate::app::RenderRoot::paint`] as [`PaintOutcome::needs_layout`],
    /// which folds into the render root's pending [`ChangeFlags`] so the mobile
    /// intra-frame layout skip re-runs layout while the animation is in flight.
    /// Deliberately opt-in: [`PaintCtx::request_frame`] alone never sets it, so
    /// paint-only animations stay layout-free.
    needs_layout: bool,
    /// Whether any continuation-frame request this (sub)paint was
    /// [`TickClass::Transition`] (unpaced, must run every vsync). The
    /// max-lattice half of the tick-class aggregation: it starts `false` and
    /// only ever flips `true` (a [`ChildPod::paint_child`]/
    /// [`PaintCtx::with_hero_registry`] bubble ORs it up), so ANY Transition
    /// request over the whole pass wins. Meaningful only when `needs_frame` is
    /// set: `needs_frame && !frame_unpaced` is the "paced-only" state the frame
    /// gate may throttle (see [`PaintCtx::frame_class`],
    /// [`PaintCtx::needs_frame_paced_only`]). `request_layout` implies
    /// Transition (a layout animation is user-visible motion), and the
    /// unchanged `request_frame` sets it too (today's every-vsync behavior).
    frame_unpaced: bool,
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
    /// The window's insets ([`WindowInsets`]), threaded down by the render root
    /// ([`crate::app::RenderRoot::paint`]) and seeded into each child by
    /// [`ChildPod::paint_child`], mirroring how `frame_time`/`theme` flow. A
    /// concrete core-owned type carried by copy; global/origin-independent (see
    /// the [`crate::insets`] module docs). Defaults to the zero inset in
    /// bare-core tests and pre-insets apps.
    window_insets: WindowInsets,
    /// The shell's running count of frames the render thread has actually
    /// presented, threaded down by the render root
    /// ([`crate::app::RenderRoot::paint`]) and seeded into each child by
    /// [`ChildPod::paint_child`], mirroring how `frame_time`/`theme`/
    /// `window_insets` flow. `None` when no shell pushed one (bare-core tests,
    /// pre-wiring shells) so a widget can fall back — see
    /// [`PaintCtx::presented_frames`]. Unlike the clock/theme/insets this is a
    /// pure *observation* the render root stores WITHOUT dirtying
    /// [`ChangeFlags`] (see [`crate::app::RenderRoot::set_presented_frames`]), so
    /// a ticking presented count never forces a relayout or feeds the mobile
    /// frame gate.
    presented_frames: Option<u64>,
    /// A tagged-rect ("hero") reporter a container installs over a subtree via
    /// [`PaintCtx::with_hero_registry`], threaded to descendants by
    /// [`ChildPod::paint_child`] like the theme/clock. `None` in the normal
    /// case (no shared-element transition in flight), so
    /// [`PaintCtx::report_hero`] is a no-op returning [`HeroDirective::Normal`].
    hero: Option<&'a RefCell<HeroFrames>>,
    /// The absolute (global-coordinate) rectangle a scroll ancestor is currently
    /// showing, threaded down by [`ChildPod::paint_child`] like the theme/clock so
    /// a container can cull paint of children fully outside it. `None` (the
    /// default) means "no viewport constraint — paint everything", so every
    /// pre-culling behavior is unchanged. A [`ScrollView`](crate::app) sets it to
    /// its viewport via [`PaintCtx::constrain_visible_rect`], which *intersects*
    /// (never widens) a nested rect, so an inner scroll surface can only ever
    /// narrow the visible region an outer one already established. Consulted by
    /// `Flex` (see [`PaintCtx::visible_rect`]); coordinates match
    /// [`PaintCtx::origin`]'s absolute space, so a child's absolute bounds test
    /// directly against it.
    visible_rect: Option<Rect>,
    /// [`PlatformViewFrame`]s published this (sub)paint via
    /// [`PaintCtx::publish_platform_view`], in paint order.
    ///
    /// Unlike `ime_state` above (an `Option` — at most one focused editable
    /// publishes per pass), this is a `Vec`: any number of platform-view slots
    /// can paint in the same pass, so [`ChildPod::paint_child`] must EXTEND it
    /// from each child rather than overwrite, or every slot but the last
    /// child's would silently vanish. See [`PaintCtx::publish_platform_view`].
    platform_views: Vec<PlatformViewFrame>,
    /// Absolute-coordinate z-shield rects reported this (sub)paint via
    /// [`PaintCtx::report_input_shield`], in paint order.
    ///
    /// The same `Vec`-extend discipline as `platform_views` above and for the
    /// same reason: any number of shields can paint in one pass, so
    /// [`ChildPod::paint_child`] EXTENDS rather than overwrites. Consumed by
    /// the shell-side platform-view differ, which intersects them against each
    /// interactive slot's rect — core stays dumb about what a shield means (see
    /// [`PaintCtx::report_input_shield`]).
    input_shields: Vec<Rect>,
    /// Whether the shell created a translucent (alpha-channel, "Mode B") GPU
    /// surface for this frame, threaded down by the render root
    /// ([`crate::app::RenderRoot::paint`], seeded from
    /// [`crate::app::RenderRoot::set_surface_translucent`]) and copied into each
    /// child by [`ChildPod::paint_child`], mirroring `theme`/`window_insets`.
    /// `false` in the normal opaque ("Mode A") case, bare-core tests, and every
    /// desktop app. Read by the platform-view hole-punch (see
    /// [`PaintCtx::is_translucent`]).
    translucent: bool,
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
            needs_layout: false,
            frame_unpaced: false,
            ime_state: None,
            has_focus: false,
            frame_time: FrameTime::ZERO,
            theme: None,
            window_insets: WindowInsets::default(),
            presented_frames: None,
            hero: None,
            visible_rect: None,
            platform_views: Vec::new(),
            input_shields: Vec::new(),
            translucent: false,
        }
    }

    /// Attach the app's active theme, type-erased. Chainable builder mirroring
    /// [`LayoutCtx::with_theme`] — used by widget unit tests that paint against a
    /// known theme; the render root threads it via [`PaintCtx::set_theme`].
    pub fn with_theme(mut self, theme: &'a dyn Any) -> Self {
        self.theme = Some(theme);
        self
    }

    /// Mark this paint pass as running against a translucent ("Mode B") surface.
    /// Chainable builder mirroring [`PaintCtx::with_theme`] — used by widget unit
    /// tests exercising the platform-view hole-punch; the render root threads the
    /// real flag via [`PaintCtx::set_translucent`] (see
    /// [`PaintCtx::is_translucent`]).
    pub fn with_translucent(mut self, translucent: bool) -> Self {
        self.translucent = translucent;
        self
    }

    /// Build a paint context at `origin`/`size` seeded with an arbitrary
    /// [`FrameTime`], for exercising clock-dependent paint logic (caret blink,
    /// a hand-advanced [`crate::anim::AnimationController`], …) from outside
    /// this crate.
    ///
    /// [`PaintCtx::set_frame_time`] is deliberately `pub(crate)` — only
    /// [`crate::app::RenderRoot::paint`] (the shell-owned clock source) may
    /// advance it in a production build — so an app crate testing a widget
    /// from its own `src/` has no other way to construct a `PaintCtx` at a
    /// chosen time. This constructor is that sanctioned seam. Gated behind
    /// `cfg(test)`/the `test-support` feature so the symbol does not exist in
    /// a normal app build; see the crate's `test-support` feature docs in
    /// `Cargo.toml`.
    #[cfg(any(test, feature = "test-support"))]
    pub fn for_test(origin: Point, size: Size, frame_time: FrameTime) -> Self {
        let mut ctx = Self::new(origin, size);
        ctx.frame_time = frame_time;
        ctx
    }

    /// Recover the threaded theme as `&T`, or `None` if no theme was threaded
    /// into this pass (a supported state — bare-core tests and pre-theme apps)
    /// or its concrete type differs from `T`.
    ///
    /// The paint-pass mirror of [`LayoutCtx::theme_as`]. Widgets that read
    /// `frust_theme::Theme` downcast through this (or the
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

    /// The window's insets ([`WindowInsets`]) for this paint pass (a cheap
    /// copy). The paint-pass mirror of [`LayoutCtx::window_insets`]:
    /// global/origin-independent, so every widget reads the same value; defaults
    /// to the zero inset when no shell pushed one.
    pub fn window_insets(&self) -> WindowInsets {
        self.window_insets
    }

    /// Seed the window insets lent by the render root. Called by
    /// [`crate::app::RenderRoot::paint`] at the root and by
    /// [`ChildPod::paint_child`] for each child, mirroring how `theme`/
    /// `frame_time` are threaded (copied down unchanged).
    pub(crate) fn set_window_insets(&mut self, insets: WindowInsets) {
        self.window_insets = insets;
    }

    /// The window insets this context carries, for re-lending to a child context
    /// (copied, so it holds no borrow of `self`).
    pub(crate) fn window_insets_ref(&self) -> WindowInsets {
        self.window_insets
    }

    /// The shell's running count of frames the render thread has actually
    /// presented, or `None` when no shell wired one in (bare-core tests,
    /// pre-wiring shells) — a supported state, so a widget can fall back to a
    /// paint-cadence measure.
    ///
    /// Under the render-thread split the UI thread paints faster than the render
    /// thread presents (a gate-skipped or coalesced frame is never presented), so
    /// a widget measuring *frames per second* must difference this presented
    /// count — not its own paint count — to report the rate a user actually sees
    /// (`examples/shadertoy`'s HUD is the reference consumer). A widget only ever
    /// *differences* two reads (`wrapping_sub`); the absolute value is a
    /// free-running monotonic counter it must never interpret directly. Seeded
    /// from the shell at the root ([`crate::app::RenderRoot::paint`]) and threaded
    /// unchanged into every child by [`ChildPod::paint_child`], mirroring
    /// `frame_time`.
    pub fn presented_frames(&self) -> Option<u64> {
        self.presented_frames
    }

    /// Seed the shell's presented-frame count. Called by
    /// [`crate::app::RenderRoot::paint`] at the root and by
    /// [`ChildPod::paint_child`] for each child, mirroring how `frame_time`/
    /// `window_insets` are threaded (copied down unchanged).
    pub(crate) fn set_presented_frames(&mut self, presented: Option<u64>) {
        self.presented_frames = presented;
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
    ///
    /// This requests a [`TickClass::Transition`] frame — the unpaced,
    /// every-vsync class, unchanged from today's behavior. A widget whose next
    /// frame is a *pacable* decorative loop calls [`Self::request_frame_paced`]
    /// (or [`Self::request_frame_class`]) instead so the mobile frame gate may
    /// throttle it.
    pub fn request_frame(&mut self) {
        self.request_frame_class(TickClass::Transition);
    }

    /// Request a continuation frame whose next tick is a *pacable* decorative
    /// loop ([`TickClass::CosmeticLoop`]) — a shimmer, an idle pulse, a spinner
    /// whose exact cadence is imperceptible.
    ///
    /// Bubbles like [`Self::request_frame`], but leaves the frame paceable: only
    /// if *every* request this frame is `CosmeticLoop` may the frame gate throttle
    /// it (see [`TickClass`]'s max-lattice aggregation). Any concurrent
    /// [`Self::request_frame`]/[`Self::request_layout`] elsewhere in the tree
    /// re-forces every-vsync cadence, so a paced request is never a downgrade of
    /// user-visible motion.
    pub fn request_frame_paced(&mut self) {
        self.request_frame_class(TickClass::CosmeticLoop);
    }

    /// Request a continuation frame of an explicit [`TickClass`] — the general
    /// form behind [`Self::request_frame`] (Transition) and
    /// [`Self::request_frame_paced`] (CosmeticLoop).
    ///
    /// Always sets `needs_frame`; a [`TickClass::Transition`] request additionally
    /// marks the aggregate unpaced (the max-lattice OR — see [`TickClass`]). A
    /// `CosmeticLoop` request never clears an already-unpaced aggregate.
    pub fn request_frame_class(&mut self, class: TickClass) {
        self.needs_frame = true;
        if class == TickClass::Transition {
            self.frame_unpaced = true;
        }
    }

    /// Whether a continuation frame was requested during this (sub)paint.
    pub fn needs_frame(&self) -> bool {
        self.needs_frame
    }

    /// The aggregate [`TickClass`] requested during this (sub)paint, or `None`
    /// if no frame was requested.
    ///
    /// Follows the [`TickClass`] max-lattice: [`TickClass::Transition`] if any
    /// request this pass was Transition-class (unpaced), else
    /// [`TickClass::CosmeticLoop`] when at least one paced request was made and
    /// no Transition one was. `None` means "as today — no continuation frame".
    pub fn frame_class(&self) -> Option<TickClass> {
        if !self.needs_frame {
            None
        } else if self.frame_unpaced {
            Some(TickClass::Transition)
        } else {
            Some(TickClass::CosmeticLoop)
        }
    }

    /// Whether a frame was requested and *every* request this (sub)paint was
    /// [`TickClass::CosmeticLoop`] — the paced-only state the mobile frame gate
    /// may throttle. Convenience for
    /// `frame_class() == Some(TickClass::CosmeticLoop)`.
    pub fn needs_frame_paced_only(&self) -> bool {
        self.needs_frame && !self.frame_unpaced
    }

    /// Signal that this paint advanced animation state that changes the widget's
    /// *layout* (not just its paint), so layout must re-run next frame.
    ///
    /// This is the layout counterpart to [`Self::request_frame`]: a widget whose
    /// animation only repaints (a color fade, a caret blink) calls
    /// `request_frame` alone and stays layout-free under the mobile intra-frame
    /// layout skip, whereas a widget whose animation resizes/repositions its
    /// children (an expanding accordion) calls this so layout is re-run while the
    /// animation is in flight. The flag bubbles up through
    /// [`ChildPod::paint_child`] exactly like `needs_frame` and out of
    /// [`crate::app::RenderRoot::paint`] as [`PaintOutcome::needs_layout`], which
    /// folds into the render root's pending [`crate::view::ChangeFlags`]
    /// (`LAYOUT`) so the *next* frame relayouts.
    ///
    /// Calling this also implies [`Self::request_frame`] (a widget animating its
    /// layout necessarily wants another frame), so a caller needs only one call
    /// per animating-layout frame. That implied frame is
    /// [`TickClass::Transition`] (unpaced): a layout animation is user-visible
    /// motion, so it never leaves the frame paceable.
    pub fn request_layout(&mut self) {
        self.needs_layout = true;
        // A widget animating its layout necessarily wants another frame; setting
        // `needs_frame` too means one call suffices per animating-layout frame.
        // A layout animation is user-visible motion, so the implied frame is
        // Transition-class (unpaced) — mark the aggregate accordingly.
        self.request_frame_class(TickClass::Transition);
    }

    /// Whether a layout re-run was requested during this (sub)paint.
    pub fn needs_layout(&self) -> bool {
        self.needs_layout
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

    /// Publish a platform-view child's paint-time frame (a
    /// `PlatformViewSlot`) for this paint pass.
    ///
    /// Pushes onto a `Vec` rather than setting an `Option` — deliberately NOT
    /// the same shape as [`PaintCtx::publish_ime_state`]. IME state has a
    /// single focused surface at most, so an overwrite is correct there; a
    /// platform view has no such "the" instance, so two slots publishing in
    /// one pass must both survive. [`ChildPod::paint_child`] bubbles this by
    /// `extend`, never overwrite, for exactly that reason.
    pub fn publish_platform_view(&mut self, frame: PlatformViewFrame) {
        self.platform_views.push(frame);
    }

    /// Take (and clear) every platform-view frame published during this
    /// (sub)paint, in paint order.
    pub fn take_platform_views(&mut self) -> Vec<PlatformViewFrame> {
        std::mem::take(&mut self.platform_views)
    }

    /// Report an absolute-coordinate region where frust content painted OVER a
    /// platform-view slot must keep winning pointer input (the "z-shield").
    ///
    /// The auto-collection half of the Mode B input contract: an interactive
    /// slot hands a touch-DOWN inside its rect to the
    /// native sibling, EXCEPT inside a shield. `frust-widgets`' `shield(child)`
    /// wrapper is the reporter — it paints its child unchanged and reports its
    /// own painted rect here — so an app marks chrome that overlaps a slot
    /// rather than hand-listing rects on the slot itself.
    ///
    /// Core stays dumb, exactly as it does for [`PlatformViewFrame`]: this is a
    /// flat, pass-scoped rect list with no slot association at all. The
    /// shell-side differ (`frust-shell-common::platform_view`) owns the
    /// intersection rule that decides which shields belong to which slot.
    /// Pushes (never overwrites) — see the `input_shields` field doc.
    pub fn report_input_shield(&mut self, rect: Rect) {
        self.input_shields.push(rect);
    }

    /// Take (and clear) every z-shield rect reported during this (sub)paint, in
    /// paint order. The [`PaintCtx::take_platform_views`] sibling for the shield
    /// channel (see [`PaintCtx::report_input_shield`]).
    pub fn take_input_shields(&mut self) -> Vec<Rect> {
        std::mem::take(&mut self.input_shields)
    }

    /// Report a tagged ("hero") element's absolute paint `bounds` and read back
    /// what it should do this frame.
    ///
    /// A no-op returning [`HeroDirective::Normal`] unless a container installed
    /// a reporter via [`PaintCtx::with_hero_registry`] over this subtree
    /// (the normal case — no shared-element transition in flight). When a
    /// reporter is installed, `bounds` is recorded (page-local, i.e. relative
    /// to the reporter's reference origin, so it stays stable under a page's
    /// per-frame animated transition offset), and the directive the installer
    /// set for `tag` is returned — [`HeroDirective::Suppress`] (skip painting,
    /// this endpoint is morphed by its counterpart) or
    /// [`HeroDirective::Morph`] (repaint under a rect→rect transform to the
    /// morph destination).
    pub fn report_hero(&mut self, tag: &str, bounds: Rect) -> HeroDirective {
        match self.hero {
            Some(cell) => {
                let mut frames = cell.borrow_mut();
                let local = Rect::from_origin_size(
                    bounds.origin() - frames.reference.to_vec2(),
                    bounds.size(),
                );
                frames.captured.insert(tag.to_string(), local);
                frames
                    .directives
                    .get(tag)
                    .copied()
                    .unwrap_or(HeroDirective::Normal)
            }
            None => HeroDirective::Normal,
        }
    }

    /// Whether a shared-element ("hero") transition is currently in flight over
    /// this subtree — i.e. an ancestor installed a hero reporter via
    /// [`PaintCtx::with_hero_registry`], the same condition that makes
    /// [`PaintCtx::report_hero`] record rather than no-op.
    ///
    /// The public, boolean sibling of the crate-private
    /// [`PaintCtx::hero_ref`], exposed so a container that culls far-offscreen
    /// children (a `Flex` under a `ScrollView`) can *stop* culling while a hero
    /// is morphing: a tagged descendant scrolled beyond the warm margin would
    /// otherwise never paint, and so never report its bounds
    /// ([`PaintCtx::report_hero`]) for the morph. `false` in the normal case
    /// (no transition), so culling is unaffected off the transition path.
    pub fn hero_active(&self) -> bool {
        self.hero.is_some()
    }

    /// Run `f` with a paint context that has `registry` installed as the
    /// tagged-rect ("hero") reporter, threading this context's clock/theme/
    /// focus/geometry down unchanged. A container paints a subtree inside the
    /// closure; descendants report through [`PaintCtx::report_hero`], and the
    /// container reads the captured rects back from `registry` afterward. Any
    /// continuation-frame request or IME publish made inside bubbles back onto
    /// `self`, mirroring [`ChildPod::paint_child`]'s absorb.
    pub fn with_hero_registry(
        &mut self,
        registry: &RefCell<HeroFrames>,
        f: impl FnOnce(&mut PaintCtx),
    ) {
        let mut child = PaintCtx {
            origin: self.origin,
            size: self.size,
            needs_frame: false,
            needs_layout: false,
            frame_unpaced: false,
            ime_state: None,
            has_focus: self.has_focus,
            frame_time: self.frame_time,
            theme: self.theme,
            window_insets: self.window_insets,
            presented_frames: self.presented_frames,
            hero: Some(registry),
            visible_rect: self.visible_rect,
            platform_views: Vec::new(),
            input_shields: Vec::new(),
            translucent: self.translucent,
        };
        f(&mut child);
        if child.needs_frame {
            self.needs_frame = true;
        }
        if child.needs_layout {
            self.needs_layout = true;
        }
        // Max-lattice OR: any Transition request inside dominates, keeping the
        // aggregate unpaced (mirrors `ChildPod::paint_child`'s absorb).
        if child.frame_unpaced {
            self.frame_unpaced = true;
        }
        if let Some(ime) = child.ime_state.take() {
            self.ime_state = Some(ime);
        }
        // EXTEND, never overwrite — mirrors `ChildPod::paint_child`'s absorb;
        // see `PaintCtx::publish_platform_view`'s doc comment for why this
        // channel is a `Vec` merge rather than the `ime_state` `Option` merge
        // above.
        self.platform_views.extend(child.platform_views);
        // The z-shield channel merges the same way, for the same reason (see
        // `PaintCtx::report_input_shield`).
        self.input_shields.extend(child.input_shields);
    }

    /// Seed the hero reporter lent by an ancestor. Called by
    /// [`ChildPod::paint_child`] for each child, mirroring how `theme` is
    /// threaded, so a nested hero wrapper observes the same reporter.
    pub(crate) fn set_hero(&mut self, hero: Option<&'a RefCell<HeroFrames>>) {
        self.hero = hero;
    }

    /// The hero reporter this context carries, for re-lending to a child
    /// context (copied, so it does not hold a borrow of `self`).
    pub(crate) fn hero_ref(&self) -> Option<&'a RefCell<HeroFrames>> {
        self.hero
    }

    /// The absolute-coordinate visible rectangle a scroll ancestor has threaded
    /// down, or `None` when no viewport constraint is in effect (paint
    /// everything). A container that culls offscreen children (`Flex` under a
    /// `ScrollView`) tests each child's absolute bounds against this; a widget
    /// with no interest in culling ignores it entirely. See
    /// [`PaintCtx::constrain_visible_rect`] for how a scroll surface sets it.
    pub fn visible_rect(&self) -> Option<Rect> {
        self.visible_rect
    }

    /// Constrain the threaded visible rectangle to `rect` (in absolute paint
    /// coordinates), the seam a [`ScrollView`](crate::app) uses to publish its
    /// viewport to descendants.
    ///
    /// When no rect is threaded yet this installs `rect`; when one already is
    /// (a nested scroll surface), the two are **intersected** — the visible
    /// region can only ever narrow, never widen, as scroll surfaces nest, so an
    /// inner viewport never re-reveals content an outer one clipped away. The
    /// value flows to children unchanged via [`ChildPod::paint_child`], mirroring
    /// how `window_insets`/`theme` are threaded.
    pub fn constrain_visible_rect(&mut self, rect: Rect) {
        self.visible_rect = Some(match self.visible_rect {
            Some(existing) => existing.intersect(rect),
            None => rect,
        });
    }

    /// Seed the visible rectangle lent by an ancestor. Called by
    /// [`ChildPod::paint_child`] for each child, mirroring how `theme` is
    /// threaded (copied down unchanged), so a descendant container observes the
    /// same viewport constraint the scroll ancestor established.
    pub(crate) fn set_visible_rect(&mut self, rect: Option<Rect>) {
        self.visible_rect = rect;
    }

    /// The visible rectangle this context carries, for re-lending to a child
    /// context (copied, so it holds no borrow of `self`).
    pub(crate) fn visible_rect_ref(&self) -> Option<Rect> {
        self.visible_rect
    }

    /// Whether the shell created a translucent (alpha-channel, "Mode B") GPU
    /// surface for this frame — threaded from the render root and copied down to
    /// every descendant like the theme/insets.
    ///
    /// `false` in the normal opaque ("Mode A") case, bare-core tests, and every
    /// desktop app. The platform-view hole-punch reads it: a slot only clears
    /// its rect ([`PaintScene::clear_rect`]) when this is `true`, so punching
    /// never erases app content on an opaque surface (see `frust-widgets`'
    /// `PlatformViewWidget::paint`).
    pub fn is_translucent(&self) -> bool {
        self.translucent
    }

    /// Seed the shell's surface-translucency flag. Called by
    /// [`crate::app::RenderRoot::paint`] at the root and by
    /// [`ChildPod::paint_child`] for each child, mirroring how `theme` is
    /// threaded (copied down unchanged).
    pub(crate) fn set_translucent(&mut self, translucent: bool) {
        self.translucent = translucent;
    }
}

/// A tagged-rect reporter threaded through the paint pass, letting a container
/// discover where tagged ("hero") descendants painted and drive a
/// shared-element morph between two of them across a navigation transition.
///
/// Generic vocabulary — `frust-core` carries no navigation knowledge here,
/// the same way its [`semantics`](crate::semantics) node collector carries no
/// widget-catalog knowledge. A container installs one over a subtree with
/// [`PaintCtx::with_hero_registry`]; descendants report through
/// [`PaintCtx::report_hero`].
#[derive(Debug, Default)]
pub struct HeroFrames {
    /// The absolute top-left of the enclosing surface this paint, subtracted
    /// from each reported rect so captures are stored surface-local (stable
    /// across a page's per-frame animated transition offset).
    reference: Point,
    /// Surface-local rects reported by tagged descendants this paint.
    captured: HashMap<String, Rect>,
    /// Per-tag paint directive the installer set for this paint.
    directives: HashMap<String, HeroDirective>,
}

impl HeroFrames {
    /// A reporter with `reference` as the enclosing surface's absolute top-left
    /// and per-tag paint `directives` (empty for a pure discovery pass).
    pub fn new(reference: Point, directives: HashMap<String, HeroDirective>) -> Self {
        Self {
            reference,
            captured: HashMap::new(),
            directives,
        }
    }

    /// The surface-local rects captured this paint, keyed by tag.
    pub fn captured(&self) -> &HashMap<String, Rect> {
        &self.captured
    }

    /// Consume the reporter, returning the captured surface-local rects.
    pub fn into_captured(self) -> HashMap<String, Rect> {
        self.captured
    }
}

/// What a reported hero should do this paint — the reply
/// [`PaintCtx::report_hero`] hands back to a tagged ("hero") wrapper.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HeroDirective {
    /// Paint normally (no active flight involves this tag) — the default.
    Normal,
    /// Skip painting the child: this endpoint is being morphed by the other
    /// surface's matching hero, so it must not also paint at its rest position.
    Suppress,
    /// Paint the child under a transform mapping the hero's own absolute paint
    /// bounds onto `dest` (absolute) — the morph overlay for this frame.
    Morph {
        /// The absolute destination rect the hero's bounds morph onto.
        dest: Rect,
    },
}

/// A process-wide monotonic counter backing [`next_slot_id`].
static NEXT_SLOT_ID: AtomicU64 = AtomicU64::new(1);

/// Allocate a fresh, stable platform-view slot id.
///
/// Called once per widget instance at construction (a
/// `PlatformViewSlot`) — the same stability class as [`ChildPod`]'s
/// semantics base id: identity that must survive a tree reorder, so it is
/// never derived from tree position. A flat process-wide counter rather than
/// a per-[`crate::app::RenderRoot`] allocator, since a widget has no
/// `RenderRoot` handle to draw one from at construction time.
pub fn next_slot_id() -> u64 {
    NEXT_SLOT_ID.fetch_add(1, Ordering::Relaxed)
}

/// Upper bound on the undrained retire list (see [`report_retired_slot`]).
///
/// Only reachable when nothing drains — a desktop app (no native compositor,
/// so no drain) that churns platform-view slots, or a mobile shell whose frame
/// loop has stopped. Past the cap the OLDEST id is dropped: the shell-side
/// differ's missing-streak backstop still disposes that slot, so a dropped id
/// costs a slower teardown, never a leak. Sized far above any realistic
/// per-frame teardown burst.
const MAX_PENDING_RETIRED_SLOTS: usize = 256;

/// The process-wide pending-retire list backing [`report_retired_slot`] /
/// [`take_retired_slots`].
///
/// A `Mutex<Vec<_>>` rather than a `RenderRoot` field for the same reason
/// [`NEXT_SLOT_ID`] is a process-wide counter: a widget being torn down has no
/// `RenderRoot` handle to reach — and unlike `build`/`rebuild`, the id-space is
/// already process-global, so a global drain is coherent. Same single-root
/// caveat as `next_slot_id`, revisited together with it if multi-root ever
/// lands. Panics are impossible while the lock is held (a `Vec` push/take), but
/// the poison-tolerant `unwrap_or_else(into_inner)` idiom is used anyway,
/// matching `frust-shell-common`'s process-global slots.
static RETIRED_SLOTS: Mutex<Vec<u64>> = Mutex::new(Vec::new());

/// Record that the widget owning `slot_id` was torn down (its `View::teardown`
/// ran), so the shell can dispose the native view promptly instead of waiting
/// out the differ's missing-streak heuristic.
///
/// The teardown half of the platform-view frame channel: `paint` says "this
/// slot exists here", this says "this slot is gone for good". Kept a flat
/// process-wide list (not a per-pass channel) because teardown does NOT run in
/// the paint pass — it runs mid-rebuild, arbitrarily deep inside a
/// `Component`'s own nested build context, so there is no threaded per-frame
/// sink every teardown can reach.
///
/// Drained by [`crate::app::RenderRoot::take_retired_platform_views`]; a shell
/// with no native compositor simply never drains, which is why the list is
/// capped (see [`MAX_PENDING_RETIRED_SLOTS`]).
pub fn report_retired_slot(slot_id: u64) {
    let mut pending = RETIRED_SLOTS.lock().unwrap_or_else(|e| e.into_inner());
    if pending.len() >= MAX_PENDING_RETIRED_SLOTS {
        // Drop-oldest: the differ's missing-streak backstop still catches the
        // dropped id (see the constant's doc).
        pending.remove(0);
    }
    pending.push(slot_id);
}

/// Take (and clear) every slot id reported to [`report_retired_slot`] since the
/// last call, in teardown order. Drained once per frame by a shell through
/// [`crate::app::RenderRoot::take_retired_platform_views`].
pub fn take_retired_slots() -> Vec<u64> {
    let mut pending = RETIRED_SLOTS.lock().unwrap_or_else(|e| e.into_inner());
    std::mem::take(&mut *pending)
}

/// A platform-view child's paint-time frame — everything a shell's native
/// compositor needs to place, size, clip, and dispose a native
/// sibling view for one paint pass.
///
/// All rects are logical px, **absolute window coordinates** — the same
/// space [`PaintCtx::report_hero`] callers use, built from the painting
/// pod's [`PaintCtx::origin`]/[`PaintCtx::size`]. Frames are
/// paint-pass-scoped: [`crate::app::RenderRoot::paint`] replaces the whole
/// collection every pass, so a slot that didn't paint this pass (a culled
/// subtree) simply has no frame in
/// [`crate::app::RenderRoot::platform_view_frames`] — the shell's differ owns
/// absent-means-hide/dispose semantics, not this crate.
#[derive(Clone, Debug, PartialEq)]
pub struct PlatformViewFrame {
    /// Stable per-widget-instance id, allocated once via [`next_slot_id`] at
    /// construction — never derived from tree position, so a reorder keeps
    /// identity.
    pub slot_id: u64,
    /// `"dev.frust.<Factory>"` convention naming which native view factory
    /// creates this slot's platform view.
    pub view_type: String,
    /// Opaque creation params for the native factory (may be empty).
    pub params_json: String,
    /// Bumped by the widget whenever `params_json` changes; this
    /// crate only ever carries the number through.
    pub params_generation: u64,
    /// Absolute paint bounds.
    pub rect: Rect,
    /// `visible_rect` (see [`PaintCtx::visible_rect`]) intersected with
    /// `rect`, or `None` when fully visible — no scroll ancestor is clipping
    /// it.
    pub clip: Option<Rect>,
    /// `false` ⇒ hidden (offscreen/culled by the widget itself, distinct from
    /// simply being absent from the collection this pass).
    pub visible: bool,
    /// Mode B input forwarding: whether
    /// the hosted native view should receive pointer input — a touch-DOWN
    /// inside `rect` (and outside every `shields` rect) hands the whole
    /// gesture to the native sibling in the embedding. `false` (the default)
    /// keeps the v1 no-input contract: the frust surface consumes everything.
    pub interactive: bool,
    /// The z-shield list: absolute-coordinate regions where frust content
    /// drawn OVER this slot must keep winning input. Only consulted when
    /// `interactive`. Same coordinate space as `rect`.
    ///
    /// Carries only the slot's own **manually declared** shields
    /// (`PlatformViewView::shield_local`, the escape hatch). The ordinary
    /// source is auto-collection: a `shield(child)` wrapper reports its painted
    /// rect through [`PaintCtx::report_input_shield`], and the shell-side differ
    /// merges whichever of those intersect this `rect` into the command it
    /// emits — so the shipped wire shape is the union of both, assembled one
    /// layer up.
    pub shields: Vec<Rect>,
}

/// The result of a whole [`crate::app::RenderRoot::paint`] pass.
///
/// `needs_frame` is whether any widget advanced animation state during paint and
/// asked (via [`PaintCtx::request_frame`]) to be re-invoked to continue. The
/// shell turns it into another scheduled frame — the desktop shell via
/// `window.request_redraw()`, the mobile shells implicitly through their
/// continuous loop. Mirrors [`crate::event::EventOutcome`]'s `needs_redraw`.
///
/// `needs_layout` is whether any widget asked (via [`PaintCtx::request_layout`])
/// to have layout re-run next frame because its animation changed its layout, not
/// just its paint. [`crate::app::RenderRoot::paint`] folds it into the render
/// root's pending [`crate::view::ChangeFlags`] (`LAYOUT`) so the next frame's
/// `take_change_flags().needs_layout()` reports it — driving the mobile
/// intra-frame layout skip to relayout while the animation is in flight.
///
/// `needs_frame_paced_only` is the aggregated [`TickClass`] verdict: `true` only
/// when a frame was requested and *every* request this frame was
/// [`TickClass::CosmeticLoop`] (a pacable decorative loop), `false` the instant
/// any [`TickClass::Transition`] request (including any `request_layout`) joined
/// in. The mobile frame gate may throttle such a purely-cosmetic frame
/// to a lower cadence; a `false` here means the frame runs every vsync as today.
/// Only meaningful when `needs_frame` is `true`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PaintOutcome {
    /// Whether the shell should schedule another frame to continue an animation.
    pub needs_frame: bool,
    /// Whether the render root folded a layout-continuation request into its
    /// pending change flags (a widget called [`PaintCtx::request_layout`]).
    pub needs_layout: bool,
    /// Whether a frame was requested and every request this frame was
    /// [`TickClass::CosmeticLoop`] — the paced-only state the mobile frame gate
    /// may throttle (see [`PaintCtx::needs_frame_paced_only`]). Meaningful only
    /// when `needs_frame` is set.
    pub needs_frame_paced_only: bool,
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
    /// Defaulted to [`EventResult::Ignored`] so non-interactive widgets are
    /// unaffected. Interactive widgets
    /// (Button/Checkbox/Slider) override this; containers forward to
    /// their [`ChildPod`] children via [`ChildPod::event_child`].
    fn event(&mut self, _ctx: &mut EventCtx, _event: &InputEvent) -> EventResult {
        EventResult::Ignored
    }

    /// Contribute this widget's accessibility node(s) into `ctx`.
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
/// Containers own their children directly as `ChildPod`s (a `Vec` for
/// Flex/Stack, named fields for Padding/Align) rather than as arena nodes — the
/// [`WidgetTree`](crate::tree::WidgetTree) arena stays single-root. This is a
/// deliberate divergence from the masonry "everything in the arena" model:
/// it needs zero global-id plumbing and no
/// disjoint-borrow gymnastics for the container features this crate ships. Arena-backed children
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
    /// This pod's persistent semantics base id, lazily assigned on
    /// the pod's first [`ChildPod::semantics_child`] visit from the
    /// [`RenderRoot`](crate::app::RenderRoot) allocator and reused for the whole
    /// pod lifetime — so the node id a widget contributes is stable across frames
    /// (and survives a keyed reorder, which relocates the whole pod). Interior
    /// mutability because `semantics_child` runs behind `&self` (mirroring
    /// `Widget::semantics`); `NonZeroU64` niche-packs the `Option` and encodes
    /// "id 0 is not a valid base". `None` until the first semantics pass reaches
    /// this pod.
    semantics_id: Cell<Option<NonZeroU64>>,
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
            semantics_id: Cell::new(None),
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
        // Thread the window insets down unchanged (copied — global and
        // origin-independent), mirroring the theme.
        child_ctx.set_window_insets(ctx.window_insets_ref());
        // Thread the shell's presented-frame count down unchanged (copied
        // `Option<u64>`), mirroring the clock/insets — global, origin-independent.
        child_ctx.set_presented_frames(ctx.presented_frames());
        // Thread the tagged-rect ("hero") reporter down the same way, so a hero
        // wrapper nested arbitrarily deep under an installer sees it. `None` in
        // the normal case (no shared-element transition in flight).
        child_ctx.set_hero(ctx.hero_ref());
        // Thread the scroll ancestor's visible rect down unchanged (absolute
        // coords, so a nested container tests its children directly against it),
        // mirroring the theme/insets. `None` in the normal case (no scroll
        // ancestor culling), so paint descends into every child as before.
        child_ctx.set_visible_rect(ctx.visible_rect_ref());
        // Thread the shell's surface-translucency flag down unchanged (copied
        // bool, global and origin-independent), mirroring the theme/insets — the
        // platform-view hole-punch reads it (see `PaintCtx::is_translucent`).
        child_ctx.set_translucent(ctx.is_translucent());
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
        // Bubble the child's continuation-frame request AND its tick class up
        // unchanged: forwarding the aggregate class (rather than always calling
        // `request_frame`, which is Transition) is what lets a purely-cosmetic
        // subtree stay paceable through nested containers. `frame_class()` is
        // `None` when the child asked for nothing, so a still child bubbles
        // nothing (the max-lattice identity).
        if let Some(class) = child_ctx.frame_class() {
            ctx.request_frame_class(class);
        }
        // Bubble the child's layout-continuation request the same way as
        // `needs_frame`, so a nested widget animating its layout keeps layout
        // re-running up the whole tree. (`request_layout` also re-forces the
        // Transition class via `request_frame_class` above's contract.)
        if child_ctx.needs_layout() {
            ctx.request_layout();
        }
        // Bubble a focused editable's republished IME surface up the paint path,
        // so `RenderRoot::paint` can refresh the shell-facing state after a
        // rebuild-driven controlled change (see `PaintCtx::publish_ime_state`).
        if let Some(ime) = child_ctx.take_ime_state() {
            ctx.publish_ime_state(ime);
        }
        // Bubble any platform-view frames published this paint by EXTENDING
        // the parent's Vec — deliberately NOT the `ime_state` overwrite shape
        // above. Two sibling slots publishing in the same pass must both
        // survive; an Option-based merge here would silently drop every slot
        // but the last child painted (see `PaintCtx::publish_platform_view`).
        ctx.platform_views.extend(child_ctx.take_platform_views());
        // Bubble any z-shield rects reported this paint the same way, and for
        // the same reason — two sibling shields must both survive (see
        // `PaintCtx::report_input_shield`).
        ctx.input_shields.extend(child_ctx.take_input_shields());
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
        let base = self.semantics_base(ctx);
        ctx.descend_into_pod(base, self.origin.to_vec2(), self.size, |ctx| {
            self.widget.semantics(ctx);
        });
    }

    /// This pod's stable semantics base id, assigning one from the allocator on
    /// the first visit and reusing the cached value thereafter —
    /// the mechanism that keeps a widget's node id stable across frames and keyed
    /// reorders. See [`ChildPod::semantics_id`].
    fn semantics_base(&self, ctx: &mut SemanticsCtx) -> NonZeroU64 {
        match self.semantics_id.get() {
            Some(id) => id,
            None => {
                let id = ctx.alloc_base();
                self.semantics_id.set(Some(id));
                id
            }
        }
    }

    /// Route an event into the child, translating its position into the child's
    /// local space and folding the child's redraw/capture flags back into `ctx`.
    ///
    /// If the child captured the pointer, this records the active path
    /// ([`ChildPod::is_active`]); the container clears it on `Up`/`Cancel`.
    ///
    /// Containers should not call this directly gated on an ad-hoc
    /// `contains()` check — that drops a captured gesture the instant it moves
    /// outside the child's bounds. Route through `frust-widgets`'
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
    /// falls — see `frust-widgets`' `route_event`/`route_event_single`.
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

    /// A leaf widget whose animation changes its layout: it signals
    /// [`PaintCtx::request_layout`] on every paint — stands in for an animating
    /// widget that resizes/repositions (e.g. an expanding accordion).
    struct LayoutAnimator;

    impl Widget for LayoutAnimator {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }

        fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
            ctx.request_layout();
        }
    }

    /// A leaf widget whose animation is a *pacable* decorative loop: it signals
    /// [`PaintCtx::request_frame_paced`] on every paint — stands in for a
    /// shimmer/idle-pulse whose cadence the frame gate may throttle.
    struct PacedAnimator;

    impl Widget for PacedAnimator {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }

        fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
            ctx.request_frame_paced();
        }
    }

    /// A scene recorder used to assert paint output without any GPU dependency.
    #[derive(Default)]
    struct RecordingScene {
        rects: Vec<(Point, Size)>,
        texts: Vec<(Point, String)>,
        shaders: Vec<(u64, Rect, f32)>,
    }

    impl PaintScene for RecordingScene {
        fn fill_rect(&mut self, origin: Point, size: Size, _color: Color) {
            self.rects.push((origin, size));
        }
        fn draw_text(&mut self, origin: Point, text: &str) {
            self.texts.push((origin, text.to_string()));
        }
        fn draw_shader(&mut self, program: &ShaderProgram, dest: Rect, time: f32) {
            self.shaders.push((program.id(), dest, time));
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
    fn widget_paint_emits_shader_into_scene() {
        /// A leaf widget that paints a shader quad.
        struct ShaderWidget {
            program: ShaderProgram,
            dest: Rect,
            time: f32,
        }

        impl Widget for ShaderWidget {
            fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
                bc.max()
            }

            fn paint(&mut self, _ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
                scene.draw_shader(&self.program, self.dest, self.time);
            }
        }

        let program = ShaderProgram::new("fn main() {}");
        let dest = Rect::new(10.0, 20.0, 100.0, 150.0);
        let time = 1.5;
        let mut w = ShaderWidget {
            program: program.clone(),
            dest,
            time,
        };
        let mut scene = RecordingScene::default();
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(200.0, 200.0));
        w.paint(&mut ctx, &mut scene);
        assert_eq!(scene.shaders.len(), 1);
        assert_eq!(scene.shaders[0].0, program.id());
        assert_eq!(scene.shaders[0].1, dest);
        assert_eq!(scene.shaders[0].2, time);
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

    /// A leaf widget that publishes a fixed [`PlatformViewFrame`] on every
    /// paint, unless `should_publish` is false — the `false` arm stands in for
    /// a slot that didn't paint this pass (culled subtree), exercising the
    /// "no publishers this pass" behavior at the `RenderRoot`
    /// level (see `app.rs`'s tests).
    struct PlatformViewProbe {
        slot_id: u64,
        should_publish: bool,
    }

    impl Widget for PlatformViewProbe {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }

        fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
            if self.should_publish {
                ctx.publish_platform_view(PlatformViewFrame {
                    slot_id: self.slot_id,
                    view_type: "dev.frust.Probe".to_string(),
                    params_json: String::new(),
                    params_generation: 0,
                    rect: Rect::from_origin_size(ctx.origin(), ctx.size()),
                    clip: None,
                    visible: true,
                    interactive: false,
                    shields: Vec::new(),
                });
            }
        }
    }

    #[test]
    fn child_pod_extends_platform_views_never_overwrites() {
        // Two slots publishing across two `paint_child` calls in one pass must
        // BOTH survive, in paint order — the regression test for the
        // Option-overwrite hazard: an ime_state-shaped merge here would leave
        // only the second slot's frame (see `PaintCtx::publish_platform_view`'s
        // doc comment).
        let mut lctx = LayoutCtx::new();

        let mut first = ChildPod::new(Box::new(PlatformViewProbe {
            slot_id: 1,
            should_publish: true,
        }));
        first.set_origin(Point::new(0.0, 0.0));
        first.layout_child(&mut lctx, &BoxConstraints::tight(Size::new(10.0, 10.0)));

        let mut second = ChildPod::new(Box::new(PlatformViewProbe {
            slot_id: 2,
            should_publish: true,
        }));
        second.set_origin(Point::new(20.0, 0.0));
        second.layout_child(&mut lctx, &BoxConstraints::tight(Size::new(10.0, 10.0)));

        let mut scene = RecordingScene::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(100.0, 100.0));
        first.paint_child(&mut pctx, &mut scene);
        second.paint_child(&mut pctx, &mut scene);

        let frames = pctx.take_platform_views();
        assert_eq!(
            frames.len(),
            2,
            "both slots' frames must survive, not just the last-painted one"
        );
        assert_eq!(frames[0].slot_id, 1);
        assert_eq!(frames[1].slot_id, 2);
    }

    /// A container widget wrapping a single child pod at a fixed offset —
    /// stands in for `frust-widgets::Padding` to test that a published frame's
    /// rect compounds correctly under nesting rather than staying local to the
    /// innermost pod.
    struct TranslatingWrapper {
        child: ChildPod,
    }

    impl Widget for TranslatingWrapper {
        fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            self.child.layout_child(ctx, bc)
        }

        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            self.child.paint_child(ctx, scene);
        }
    }

    #[test]
    fn platform_view_frame_rect_is_absolute_under_nested_translation() {
        // Wrap a publishing probe under two levels of translation — an inner
        // pod at (5, 7) inside a wrapper placed at (100, 200) — the published
        // rect must land in ABSOLUTE window coordinates (the same space
        // `PaintCtx::report_hero` callers use), not local to either level.
        let mut lctx = LayoutCtx::new();

        let inner = ChildPod::new(Box::new(PlatformViewProbe {
            slot_id: 9,
            should_publish: true,
        }));
        let mut wrapper = TranslatingWrapper { child: inner };
        wrapper.child.set_origin(Point::new(5.0, 7.0));
        wrapper
            .child
            .layout_child(&mut lctx, &BoxConstraints::tight(Size::new(10.0, 10.0)));

        let mut outer = ChildPod::new(Box::new(wrapper));
        outer.set_origin(Point::new(100.0, 200.0));
        outer.layout_child(&mut lctx, &BoxConstraints::tight(Size::new(10.0, 10.0)));

        let mut scene = RecordingScene::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(400.0, 400.0));
        outer.paint_child(&mut pctx, &mut scene);

        let frames = pctx.take_platform_views();
        assert_eq!(frames.len(), 1);
        assert_eq!(
            frames[0].rect,
            Rect::from_origin_size(Point::new(105.0, 207.0), Size::new(10.0, 10.0))
        );
    }

    #[test]
    fn request_frame_alone_does_not_set_needs_layout() {
        // A paint-only animation (request_frame, no request_layout) must leave
        // `needs_layout` clear — the mobile intra-frame layout-skip depends on this.
        let mut anim = ChildPod::new(Box::new(Animator));
        let mut lctx = LayoutCtx::new();
        anim.layout_child(&mut lctx, &BoxConstraints::tight(Size::new(10.0, 10.0)));
        let mut scene = RecordingScene::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        anim.paint_child(&mut pctx, &mut scene);
        assert!(pctx.needs_frame(), "request_frame sets needs_frame");
        assert!(
            !pctx.needs_layout(),
            "request_frame alone must NOT set needs_layout"
        );
    }

    #[test]
    fn request_layout_implies_needs_frame() {
        // `request_layout` also sets `needs_frame` so one call per
        // animating-layout frame suffices.
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        assert!(!ctx.needs_frame());
        assert!(!ctx.needs_layout());
        ctx.request_layout();
        assert!(ctx.needs_layout());
        assert!(ctx.needs_frame());
    }

    #[test]
    fn child_pod_bubbles_needs_layout_from_child_paint() {
        // A non-layout-animating child leaves the parent's layout flag clear.
        let mut still = ChildPod::new(Box::new(FixedBox {
            intrinsic: Size::new(10.0, 10.0),
        }));
        let mut lctx = LayoutCtx::new();
        still.layout_child(&mut lctx, &BoxConstraints::tight(Size::new(10.0, 10.0)));
        let mut scene = RecordingScene::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        still.paint_child(&mut pctx, &mut scene);
        assert!(!pctx.needs_layout());

        // A layout-animating child bubbles its request into the parent context.
        let mut anim = ChildPod::new(Box::new(LayoutAnimator));
        anim.layout_child(&mut lctx, &BoxConstraints::tight(Size::new(10.0, 10.0)));
        let mut pctx2 = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        assert!(!pctx2.needs_layout());
        anim.paint_child(&mut pctx2, &mut scene);
        assert!(pctx2.needs_layout());
        // Bubbling `request_layout` also carries the implied `needs_frame`.
        assert!(pctx2.needs_frame());
    }

    #[test]
    fn needs_layout_bubbles_through_nested_containers() {
        // A container holding a single `ChildPod` forwards paint via
        // `paint_child`; a layout-animating leaf two levels deep must still
        // surface `needs_layout` at the outermost paint context.
        struct SingleChildContainer {
            child: ChildPod,
        }
        impl Widget for SingleChildContainer {
            fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
                self.child.layout_child(ctx, bc)
            }
            fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
                self.child.paint_child(ctx, scene);
            }
        }

        let inner = SingleChildContainer {
            child: ChildPod::new(Box::new(LayoutAnimator)),
        };
        let mut outer = ChildPod::new(Box::new(SingleChildContainer {
            child: ChildPod::new(Box::new(inner)),
        }));
        let mut lctx = LayoutCtx::new();
        outer.layout_child(&mut lctx, &BoxConstraints::tight(Size::new(10.0, 10.0)));
        let mut scene = RecordingScene::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        outer.paint_child(&mut pctx, &mut scene);
        assert!(
            pctx.needs_layout(),
            "needs_layout bubbles up nested containers"
        );
        assert!(pctx.needs_frame());
    }

    /// A single-`ChildPod` container that forwards paint via `paint_child`,
    /// reused by the tick-class bubbling tests below.
    struct SingleChildContainer {
        child: ChildPod,
    }
    impl Widget for SingleChildContainer {
        fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            self.child.layout_child(ctx, bc)
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            self.child.paint_child(ctx, scene);
        }
    }

    #[test]
    fn no_request_yields_no_frame_class() {
        // No frame requested at all → `frame_class()` is `None` and the frame is
        // not paced-only (as today: nothing to schedule).
        let ctx = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        assert!(ctx.frame_class().is_none());
        assert!(!ctx.needs_frame());
        assert!(!ctx.needs_frame_paced_only());
    }

    #[test]
    fn request_frame_is_transition_unpaced() {
        // The unchanged `request_frame` is a Transition request: it must NOT be
        // paced-only, preserving today's every-vsync behavior for existing callers.
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        ctx.request_frame();
        assert!(ctx.needs_frame());
        assert_eq!(ctx.frame_class(), Some(TickClass::Transition));
        assert!(
            !ctx.needs_frame_paced_only(),
            "request_frame stays unpaced (Transition), unchanged behavior"
        );
    }

    #[test]
    fn request_frame_paced_is_cosmetic_loop() {
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        ctx.request_frame_paced();
        assert!(ctx.needs_frame());
        assert_eq!(ctx.frame_class(), Some(TickClass::CosmeticLoop));
        assert!(ctx.needs_frame_paced_only());
    }

    #[test]
    fn transition_dominates_cosmetic_regardless_of_order() {
        // Paced then Transition → unpaced.
        let mut a = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        a.request_frame_paced();
        a.request_frame();
        assert_eq!(a.frame_class(), Some(TickClass::Transition));
        assert!(!a.needs_frame_paced_only());

        // Transition then paced → still unpaced (a CosmeticLoop never clears it).
        let mut b = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        b.request_frame();
        b.request_frame_paced();
        assert_eq!(b.frame_class(), Some(TickClass::Transition));
        assert!(!b.needs_frame_paced_only());
    }

    #[test]
    fn request_layout_implies_transition_class() {
        // `request_layout` is user-visible motion, so it re-forces the unpaced
        // Transition class even if a paced request preceded it.
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        ctx.request_frame_paced();
        assert!(ctx.needs_frame_paced_only());
        ctx.request_layout();
        assert_eq!(ctx.frame_class(), Some(TickClass::Transition));
        assert!(
            !ctx.needs_frame_paced_only(),
            "request_layout implies Transition, leaving the frame unpaced"
        );
    }

    #[test]
    fn child_pod_bubbles_paced_class_from_child_paint() {
        // A purely-cosmetic child bubbles a paced request into the parent — the
        // parent's aggregate stays paced-only.
        let mut anim = ChildPod::new(Box::new(PacedAnimator));
        let mut lctx = LayoutCtx::new();
        anim.layout_child(&mut lctx, &BoxConstraints::tight(Size::new(10.0, 10.0)));
        let mut scene = RecordingScene::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        anim.paint_child(&mut pctx, &mut scene);
        assert!(pctx.needs_frame());
        assert_eq!(pctx.frame_class(), Some(TickClass::CosmeticLoop));
        assert!(pctx.needs_frame_paced_only());
    }

    #[test]
    fn paced_class_bubbles_through_nested_containers() {
        // A cosmetic-loop leaf two levels deep must still surface as paced-only
        // at the outermost paint context (the bubbling identity of the lattice).
        let inner = SingleChildContainer {
            child: ChildPod::new(Box::new(PacedAnimator)),
        };
        let mut outer = ChildPod::new(Box::new(SingleChildContainer {
            child: ChildPod::new(Box::new(inner)),
        }));
        let mut lctx = LayoutCtx::new();
        outer.layout_child(&mut lctx, &BoxConstraints::tight(Size::new(10.0, 10.0)));
        let mut scene = RecordingScene::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        outer.paint_child(&mut pctx, &mut scene);
        assert!(pctx.needs_frame());
        assert!(
            pctx.needs_frame_paced_only(),
            "a nested cosmetic-loop leaf stays paced-only up the tree"
        );
    }

    #[test]
    fn mixed_sibling_requests_aggregate_to_unpaced() {
        // Two sibling children under one container: one paced, one Transition.
        // The container's aggregate must be unpaced (any Transition dominates).
        struct TwoChildContainer {
            a: ChildPod,
            b: ChildPod,
        }
        impl Widget for TwoChildContainer {
            fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
                self.a.layout_child(ctx, bc);
                self.b.layout_child(ctx, bc)
            }
            fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
                self.a.paint_child(ctx, scene);
                self.b.paint_child(ctx, scene);
            }
        }

        let mut outer = ChildPod::new(Box::new(TwoChildContainer {
            a: ChildPod::new(Box::new(PacedAnimator)),
            b: ChildPod::new(Box::new(Animator)),
        }));
        let mut lctx = LayoutCtx::new();
        outer.layout_child(&mut lctx, &BoxConstraints::tight(Size::new(10.0, 10.0)));
        let mut scene = RecordingScene::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        outer.paint_child(&mut pctx, &mut scene);
        assert!(pctx.needs_frame());
        assert_eq!(pctx.frame_class(), Some(TickClass::Transition));
        assert!(
            !pctx.needs_frame_paced_only(),
            "a mixed paced+transition sibling set aggregates to unpaced"
        );
    }

    #[test]
    fn with_hero_registry_bubbles_paced_class() {
        // The hero-reporter sub-context absorbs a paced request the same way it
        // absorbs `needs_frame`/`needs_layout`.
        let registry = RefCell::new(HeroFrames::default());
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        ctx.with_hero_registry(&registry, |child| {
            child.request_frame_paced();
        });
        assert!(ctx.needs_frame());
        assert!(
            ctx.needs_frame_paced_only(),
            "with_hero_registry bubbles the paced class up"
        );

        // A Transition inside the closure dominates the outer aggregate too.
        let mut ctx2 = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        ctx2.request_frame_paced();
        ctx2.with_hero_registry(&registry, |child| {
            child.request_frame();
        });
        assert_eq!(ctx2.frame_class(), Some(TickClass::Transition));
        assert!(!ctx2.needs_frame_paced_only());
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

    /// A dummy theme type, standing in for `frust_theme::Theme` — proving the
    /// type-erased theme slot works for *any* `'static` type, not just the real
    /// theme (`frust-core` never names it).
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

    /// A leaf widget whose visible appearance is driven purely by
    /// [`PaintCtx::frame_time`] — stands in for a caret-blink widget: it fills
    /// a rect only during the "on" half of a 1-second blink cycle, with no
    /// internal state of its own.
    struct BlinkBox;

    impl Widget for BlinkBox {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(4.0, 4.0))
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            let millis = ctx.frame_time().as_nanos() / 1_000_000;
            if (millis / 500).is_multiple_of(2) {
                scene.fill_rect(ctx.origin(), ctx.size(), Color::BLACK);
            }
        }
    }

    #[test]
    fn for_test_seeds_the_requested_frame_time() {
        let ctx = PaintCtx::for_test(Point::ZERO, Size::new(1.0, 1.0), FrameTime::from_nanos(42));
        assert_eq!(ctx.frame_time(), FrameTime::from_nanos(42));
    }

    /// End-to-end proof the seam actually drives a clock-dependent widget:
    /// the same [`BlinkBox`] paints differently at two [`FrameTime`]s built
    /// via [`PaintCtx::for_test`] alone — no [`crate::app::RenderRoot`]
    /// involved, exactly how an app crate outside this workspace would use
    /// the seam.
    #[test]
    fn for_test_drives_a_clock_dependent_widget_to_two_appearances() {
        let mut widget = BlinkBox;
        let size = Size::new(4.0, 4.0);

        let mut on_ctx = PaintCtx::for_test(Point::ZERO, size, FrameTime::from_nanos(0));
        let mut on_scene = RecordingScene::default();
        widget.paint(&mut on_ctx, &mut on_scene);
        assert_eq!(on_scene.rects.len(), 1, "expected the caret painted on");

        let mut off_ctx = PaintCtx::for_test(Point::ZERO, size, FrameTime::from_nanos(500_000_000));
        let mut off_scene = RecordingScene::default();
        widget.paint(&mut off_ctx, &mut off_scene);
        assert!(
            off_scene.rects.is_empty(),
            "expected the caret painted off half a blink cycle later"
        );
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
