//! Layer 2: the retained [`Widget`] trait and its layout/paint contexts.
//!
//! Widgets are the long-lived counterpart to [`crate::view::View`]s: a view is
//! rebuilt every frame, while its widget persists and is mutated in place.
//! Widgets take part in layout (constraints in, [`Size`] out) and paint.

use std::any::Any;
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Re-exported so widgets can name [`PaintScene`]'s argument types.
pub use frust_scene::{CornerRadii, DashPattern};
use frust_scene::{GlyphRun, SceneBuilder, ShaderProgram};
use kurbo::{Affine, BezPath, Point, Rect, Size};
use peniko::{Brush, Color};

use crate::anim::FrameTime;
use crate::event::{EventCtx, EventResult, ImeState, InputEvent};
use crate::insets::WindowInsets;
use crate::layout::BoxConstraints;
use crate::semantics::SemanticsCtx;
use crate::shortcuts::ShortcutId;
use crate::sliver::{SliverConstraints, SliverGeometry, SliverWidget};

/// The renderer-agnostic paint target a widget draws into; `SceneBuilder`
/// implements it, and defaulted methods keep recorder test scenes valid.
/// A renderer that never publishes a ledger paints every retained body inline;
/// one that does must serve every key it lists or refuse the frame.
pub trait PaintScene {
    /// Fill an axis-aligned rectangle at `origin`/`size` with solid `color`.
    fn fill_rect(&mut self, origin: Point, size: Size, color: Color);

    /// Fill a rectangle with uniformly rounded corners.
    fn fill_rounded_rect(&mut self, _origin: Point, _size: Size, _radius: f64, _color: Color) {}

    /// Fill a rectangle with per-corner `radii` (defaults to the largest corner).
    fn fill_rounded_rect_radii(
        &mut self,
        origin: Point,
        size: Size,
        radii: CornerRadii,
        color: Color,
    ) {
        self.fill_rounded_rect(origin, size, radii.largest(), color);
    }

    /// Stroke a straight line from `p0` to `p1` with `width` and solid `color`.
    fn stroke_line(&mut self, _p0: Point, _p1: Point, _width: f64, _color: Color) {}

    /// Push a rectangular clip until the matching [`PaintScene::pop_clip`].
    fn push_clip(&mut self, _origin: Point, _size: Size) {}

    /// Push a clip with uniformly rounded corners; one clip stack for all kinds.
    fn push_clip_rounded(&mut self, _origin: Point, _size: Size, _radius: f64) {}

    /// Push a clip with per-corner `radii`. Defaults to the largest corner, not a
    /// no-op, so a recorder's clip stack stays balanced.
    fn push_clip_rounded_radii(&mut self, origin: Point, size: Size, radii: CornerRadii) {
        self.push_clip_rounded(origin, size, radii.largest());
    }

    /// Push a clip to `path`'s interior (nonzero, local space plus `origin`); a
    /// retained body under it paints inline. Defaults to the bounding-box clip.
    fn push_clip_path(&mut self, origin: Point, path: &BezPath) {
        let bounds = kurbo::Shape::bounding_box(path);
        self.push_clip(origin + bounds.origin().to_vec2(), bounds.size());
    }

    /// Pop the most recently pushed clip, rectangular, rounded or path.
    fn pop_clip(&mut self) {}

    /// Emit unshaped text (recorder intent only); `SceneBuilder` ignores it.
    fn draw_text(&mut self, origin: Point, text: &str);

    /// Emit a run of already-shaped glyphs.
    fn draw_glyph_run(&mut self, _run: GlyphRun) {}

    /// Draw a decoded image scaled from its natural size to fill `dest`.
    fn draw_image(&mut self, _data: &peniko::ImageData, _dest: Rect) {}

    /// Draw a fragment-shader-filled rect into `dest`; `time` is app seconds
    /// (no CPU golden: `docs/LIMITATIONS.md` `engine-shader-quad-goldens-uncomparable`).
    /// `program` must be a retained handle — minting one per paint or `build`
    /// misses the pipeline cache every frame.
    fn draw_shader(&mut self, _program: &ShaderProgram, _dest: Rect, _time: f32) {}

    /// Composite the scene texture `id` (bound by `frust_gpu::ExternalPass::record`)
    /// into `dest`, blended; an unbound id draws nothing and warns once.
    fn draw_scene_texture(&mut self, _id: u64, _dest: Rect) {}

    /// Draw a Gaussian-blurred rounded-rect elevation shadow at `origin`/`size`.
    fn draw_shadow(
        &mut self,
        _origin: Point,
        _size: Size,
        _radius: f64,
        _std_dev: f64,
        _color: Color,
    ) {
    }

    /// Fill a rectangle with `brush`; defaults to `fill_rect` with its solid
    /// color (transparent for a gradient).
    fn fill_rect_brush(&mut self, origin: Point, size: Size, brush: &Brush) {
        let color = match brush {
            Brush::Solid(color) => *color,
            _ => Color::TRANSPARENT,
        };
        self.fill_rect(origin, size, color);
    }

    /// Fill a rounded rectangle with an arbitrary `brush`.
    fn fill_rounded_rect_brush(
        &mut self,
        _origin: Point,
        _size: Size,
        _radius: f64,
        _brush: &Brush,
    ) {
    }

    /// Push a layer composited at `alpha` until [`PaintScene::pop_layer`].
    fn push_layer(&mut self, _origin: Point, _size: Size, _alpha: f32) {}

    /// Pop the most recently pushed layer.
    fn pop_layer(&mut self) {}

    /// Clear a rect to alpha 0, erasing what lies below — the platform-view
    /// hole-punch on a translucent surface; opaque surfaces disregard it.
    fn clear_rect(&mut self, _origin: Point, _size: Size) {}

    /// Fill a vector `path` (local space plus `origin`) with `brush`, nonzero.
    fn fill_path(&mut self, _origin: Point, _path: &BezPath, _brush: &Brush) {}

    /// Stroke a vector `path` with `width`, round caps/joins and `brush`.
    fn stroke_path(&mut self, _origin: Point, _path: &BezPath, _width: f64, _brush: &Brush) {}

    /// Stroke `path` dashed by `dash`; defaults to a solid stroke.
    fn stroke_path_dashed(
        &mut self,
        origin: Point,
        path: &BezPath,
        width: f64,
        _dash: DashPattern,
        brush: &Brush,
    ) {
        self.stroke_path(origin, path, width, brush);
    }

    /// Push an affine `transform` (the one seam carrying scale/rotation) until
    /// [`PaintScene::pop_transform`].
    fn push_transform(&mut self, _transform: Affine) {}

    /// Pop the most recently pushed transform.
    fn pop_transform(&mut self) {}

    /// Push a retained bracket cached by `key`, presented at `alpha` and uniform
    /// `scale`; the default emulates it with a transform then a layer.
    fn push_retained(&mut self, _key: u64, origin: Point, size: Size, alpha: f32, scale: f64) {
        let center = Point::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
        self.push_transform(Affine::scale_about(scale, center));
        self.push_layer(origin, size, alpha);
    }

    /// Pop a retained bracket: by default the layer, then the transform.
    fn pop_retained(&mut self) {
        self.pop_layer();
        self.pop_transform();
    }

    /// Draw the body cached under `key`, sent only for keys the ledger listed.
    fn draw_retained(&mut self, _key: u64, _origin: Point, _size: Size, _alpha: f32, _scale: f64) {}

    /// The transform the next command records under (device scale included),
    /// or `None` (identity); a retained boundary requests pages at this scale.
    fn current_transform(&self) -> Option<Affine> {
        None
    }

    /// Blur what is already painted under `origin`/`size` with a Gaussian of
    /// standard deviation `radius` (frosted glass).
    fn backdrop_blur(&mut self, _origin: Point, _size: Size, _radius: f64) {}

    /// The painter-order index of the next command (`0` for a recorder), which
    /// a platform-view slot stamps on [`PlatformViewFrame::command_index`].
    fn command_index(&self) -> usize {
        0
    }
}

/// A [`PaintScene`] that records nothing, for painting a subtree (an alpha-0
/// page) only for its side effects — hero rects, animating children.
pub struct DiscardScene;

impl PaintScene for DiscardScene {
    fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}

    fn draw_text(&mut self, _origin: Point, _text: &str) {}
}

/// Bridges [`PaintScene`] onto `SceneBuilder`; unshaped text is dropped.
impl PaintScene for SceneBuilder<'_> {
    fn fill_rect(&mut self, origin: Point, size: Size, color: Color) {
        SceneBuilder::fill_rect(self, rect_at(origin, size), Brush::Solid(color));
    }

    fn fill_rounded_rect(&mut self, origin: Point, size: Size, radius: f64, color: Color) {
        SceneBuilder::fill_rounded_rect(self, rect_at(origin, size), radius, Brush::Solid(color));
    }

    fn fill_rounded_rect_radii(
        &mut self,
        origin: Point,
        size: Size,
        radii: CornerRadii,
        color: Color,
    ) {
        SceneBuilder::fill_rounded_rect_radii(
            self,
            rect_at(origin, size),
            radii,
            Brush::Solid(color),
        );
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

    fn push_clip_rounded_radii(&mut self, origin: Point, size: Size, radii: CornerRadii) {
        SceneBuilder::push_clip_rounded_radii(self, rect_at(origin, size), radii);
    }

    fn push_clip_path(&mut self, origin: Point, path: &BezPath) {
        SceneBuilder::push_clip_path(self, path_at(origin, path));
    }

    fn pop_clip(&mut self) {
        SceneBuilder::pop_clip(self);
    }

    fn draw_text(&mut self, _origin: Point, _text: &str) {
        // Unshaped text is not renderable.
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

    fn draw_scene_texture(&mut self, id: u64, dest: Rect) {
        SceneBuilder::scene_texture(self, id, dest);
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

    fn stroke_path_dashed(
        &mut self,
        origin: Point,
        path: &BezPath,
        width: f64,
        dash: DashPattern,
        brush: &Brush,
    ) {
        SceneBuilder::stroke_path_dashed(self, path_at(origin, path), width, dash, brush.clone());
    }

    fn push_transform(&mut self, transform: Affine) {
        SceneBuilder::push_transform(self, transform);
    }

    fn pop_transform(&mut self) {
        SceneBuilder::pop_transform(self);
    }

    fn push_retained(&mut self, key: u64, origin: Point, size: Size, alpha: f32, scale: f64) {
        SceneBuilder::push_retained(self, key, rect_at(origin, size), alpha, scale);
    }

    fn pop_retained(&mut self) {
        SceneBuilder::pop_retained(self);
    }

    fn draw_retained(&mut self, key: u64, origin: Point, size: Size, alpha: f32, scale: f64) {
        SceneBuilder::draw_retained(self, key, rect_at(origin, size), alpha, scale);
    }

    fn current_transform(&self) -> Option<Affine> {
        Some(SceneBuilder::current_transform(self))
    }

    fn backdrop_blur(&mut self, origin: Point, size: Size, radius: f64) {
        SceneBuilder::backdrop_blur(self, rect_at(origin, size), radius);
    }

    fn command_index(&self) -> usize {
        SceneBuilder::command_count(self)
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

/// Translate `path` from local space by `origin`, as [`rect_at`] does for rects.
fn path_at(origin: Point, path: &BezPath) -> BezPath {
    Affine::translate((origin.x, origin.y)) * path.clone()
}

/// Context passed to [`Widget::layout`], carrying the text-shaping context and
/// theme type-erased so `frust-core` depends on neither `frust-text` nor
/// `frust-theme`.
pub struct LayoutCtx<'a> {
    text_ctx: Option<&'a mut dyn Any>,
    /// The active theme; `None` in bare-core tests (a supported state).
    theme: Option<&'a dyn Any>,
    /// The window insets, global except inside a `with_window_insets` scope.
    window_insets: WindowInsets,
    /// The window's logical size, for laying out overlay pods.
    window_size: Size,
    /// The root's layout epoch, bumped for global inputs that bypass constraints.
    epoch: u64,
    /// The layout-skip kill switch: `true` lays out every pod.
    force: bool,
}

impl<'a> LayoutCtx<'a> {
    /// Create a layout context with no shared resources (leaf unit tests).
    pub fn new() -> LayoutCtx<'static> {
        LayoutCtx {
            text_ctx: None,
            theme: None,
            window_insets: WindowInsets::default(),
            window_size: Size::ZERO,
            epoch: 0,
            force: true,
        }
    }

    /// Create a layout context carrying the shared text-shaping context.
    pub fn with_text_context(text_ctx: &'a mut dyn Any) -> Self {
        LayoutCtx {
            text_ctx: Some(text_ctx),
            theme: None,
            window_insets: WindowInsets::default(),
            window_size: Size::ZERO,
            epoch: 0,
            force: true,
        }
    }

    /// Create a layout context carrying an optional text context and theme.
    pub fn with_resources(text_ctx: Option<&'a mut dyn Any>, theme: Option<&'a dyn Any>) -> Self {
        LayoutCtx {
            text_ctx,
            theme,
            window_insets: WindowInsets::default(),
            window_size: Size::ZERO,
            epoch: 0,
            force: true,
        }
    }

    /// Attach the app's active theme, type-erased.
    pub fn with_theme(mut self, theme: &'a dyn Any) -> Self {
        self.theme = Some(theme);
        self
    }

    /// Recover the text-shaping context as `&mut T`; panics if absent or of
    /// another type (a wiring bug).
    pub fn text_context<T: Any>(&mut self) -> &mut T {
        self.text_ctx
            .as_deref_mut()
            .expect("no text context threaded into this layout pass")
            .downcast_mut::<T>()
            .expect("threaded layout resource is not the expected text-context type")
    }

    /// Recover the theme as `&T`, or `None` if absent or of another type.
    pub fn theme_as<T: Any>(&self) -> Option<&T> {
        self.theme?.downcast_ref::<T>()
    }

    /// The window insets for this pass, possibly narrowed by an ancestor.
    pub fn window_insets(&self) -> WindowInsets {
        self.window_insets
    }

    /// Seed the window insets lent by the render root.
    pub(crate) fn set_window_insets(&mut self, insets: WindowInsets) {
        self.window_insets = insets;
    }

    /// Run `f` with `insets` as the window insets, then restore them (how
    /// `SafeArea` removes consumed edges); a panic in `f` skips the restore.
    pub fn with_window_insets<R>(
        &mut self,
        insets: WindowInsets,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let saved = std::mem::replace(&mut self.window_insets, insets);
        let result = f(self);
        self.window_insets = saved;
        result
    }

    /// The window's logical size ([`Size::ZERO`] before the first root layout);
    /// overlay pods are laid out against it, not their owner's box.
    pub fn window_size(&self) -> Size {
        self.window_size
    }

    /// Seed the window size lent by the render root.
    pub(crate) fn set_window_size(&mut self, size: Size) {
        self.window_size = size;
    }

    /// The root's global layout epoch for this pass; `0` outside a root.
    pub fn layout_epoch(&self) -> u64 {
        self.epoch
    }

    /// Seed the layout epoch lent by the render root.
    pub(crate) fn set_layout_epoch(&mut self, epoch: u64) {
        self.epoch = epoch;
    }

    /// Whether this pass ignores layout caches (`true` without a render root).
    pub fn is_forced(&self) -> bool {
        self.force
    }

    /// Seed the force flag for this pass.
    pub(crate) fn set_layout_force(&mut self, force: bool) {
        self.force = force;
    }
}

impl Default for LayoutCtx<'static> {
    fn default() -> Self {
        Self::new()
    }
}

/// The class of a paint-time frame request: must it land on the next vsync, or
/// may the frame gate pace it? Any `Transition` makes the frame unpaced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TickClass {
    /// A pacable decorative loop (shimmer, pulse), paced only when alone.
    CosmeticLoop,
    /// User-visible motion that must run every vsync; the dominant class.
    Transition,
}

/// Context passed to [`Widget::paint`]: resolved geometry, frame-request
/// signals that bubble into a [`PaintOutcome`], and root-threaded globals.
pub struct PaintCtx<'a> {
    origin: Point,
    size: Size,
    needs_frame: bool,
    /// Whether a layout-changing animation asked for layout next frame.
    needs_layout: bool,
    /// The paint pass's layout-path bit: set when a pod below asked for layout
    /// or an unpaced frame; a paced-only request leaves it clear.
    layout_path_dirty: bool,
    /// Whether any frame request this (sub)paint was unpaced.
    frame_unpaced: bool,
    /// The min over paced intervals; a bare paced request contributes zero.
    paced_interval: Option<Duration>,
    ime_state: Option<ImeState>,
    /// Whether this widget holds the focus path on the live session.
    has_focus: bool,
    /// Whether the root's [`FocusRingPolicy`](crate::app::FocusRingPolicy) shows a ring now.
    focus_ring: bool,
    /// Whether this widget is on the recorded hover path.
    hovered: bool,
    hover_epoch: u64,
    /// The live focus session epoch (see [`set_live_focus_session`]).
    focus_epoch: u64,
    frame_time: FrameTime,
    theme: Option<&'a dyn Any>,
    window_insets: WindowInsets,
    /// The presented-frame count, stored without dirtying change flags.
    presented_frames: Option<u64>,
    /// The hero reporter, `None` outside a shared-element transition.
    hero: Option<&'a RefCell<HeroFrames>>,
    /// The absolute rect a scroll ancestor shows, for culling; only narrows.
    visible_rect: Option<Rect>,
    /// Platform-view frames published this (sub)paint; extended, never replaced.
    platform_views: Vec<PlatformViewFrame>,
    /// Z-shield rects reported this (sub)paint; extended like `platform_views`.
    input_shields: Vec<Rect>,
    translucent: bool,
    /// The root's paint epoch: a pod painted under another one is not clean.
    /// Covers translucency and every layout-epoch bump.
    paint_epoch: u64,
    /// Whether retained layers are enabled.
    retained_layers: bool,
    /// The renderer's resident-key ledger; `None` reads as empty.
    retained_ledger: Option<&'a crate::retained::RetainedLedger>,
    /// The paint serial; `0` without a root.
    paint_serial: u64,
    /// Side-channel publications this (sub)paint made, summed over children.
    paint_publications: u32,
}

impl<'a> PaintCtx<'a> {
    /// Create a paint context at `origin`/`size` with a zero clock and no theme.
    pub fn new(origin: Point, size: Size) -> Self {
        Self {
            origin,
            size,
            needs_frame: false,
            needs_layout: false,
            layout_path_dirty: false,
            frame_unpaced: false,
            paced_interval: None,
            ime_state: None,
            has_focus: false,
            focus_ring: false,
            hovered: false,
            hover_epoch: 0,
            focus_epoch: 0,
            frame_time: FrameTime::ZERO,
            theme: None,
            window_insets: WindowInsets::default(),
            presented_frames: None,
            hero: None,
            visible_rect: None,
            platform_views: Vec::new(),
            input_shields: Vec::new(),
            translucent: false,
            paint_epoch: 0,
            retained_layers: true,
            retained_ledger: None,
            paint_serial: 0,
            paint_publications: 0,
        }
    }

    /// Attach the app's active theme, type-erased (widget tests).
    pub fn with_theme(mut self, theme: &'a dyn Any) -> Self {
        self.theme = Some(theme);
        self
    }

    /// Mark the pass as painting a translucent surface (widget tests).
    pub fn with_translucent(mut self, translucent: bool) -> Self {
        self.translucent = translucent;
        self
    }

    /// Thread a resident-key ledger into the pass (widget tests).
    pub fn with_retained_ledger(mut self, ledger: &'a crate::retained::RetainedLedger) -> Self {
        self.retained_ledger = Some(ledger);
        self
    }

    /// Build a paint context seeded with `frame_time`, for clock-dependent tests
    /// outside this crate (`cfg(test)` / `test-support` only).
    #[cfg(any(test, feature = "test-support"))]
    pub fn for_test(origin: Point, size: Size, frame_time: FrameTime) -> Self {
        let mut ctx = Self::new(origin, size);
        ctx.frame_time = frame_time;
        ctx
    }

    /// Recover the theme as `&T`, or `None` if absent or of another type.
    pub fn theme_as<T: Any>(&self) -> Option<&T> {
        self.theme?.downcast_ref::<T>()
    }

    /// Seed the type-erased theme lent by the render root.
    pub(crate) fn set_theme(&mut self, theme: Option<&'a dyn Any>) {
        self.theme = theme;
    }

    /// The theme reference, copied for re-lending to a child context.
    pub(crate) fn theme_ref(&self) -> Option<&'a dyn Any> {
        self.theme
    }

    /// The window insets for this paint pass.
    pub fn window_insets(&self) -> WindowInsets {
        self.window_insets
    }

    /// Seed the window insets lent by the render root.
    pub(crate) fn set_window_insets(&mut self, insets: WindowInsets) {
        self.window_insets = insets;
    }

    /// The window insets, copied for re-lending to a child context.
    pub(crate) fn window_insets_ref(&self) -> WindowInsets {
        self.window_insets
    }

    /// Run `f` with `insets` as the window insets, then restore them; wrapping
    /// `paint_child` hands the override to the whole subtree.
    pub fn with_window_insets<R>(
        &mut self,
        insets: WindowInsets,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let saved = std::mem::replace(&mut self.window_insets, insets);
        let result = f(self);
        self.window_insets = saved;
        result
    }

    /// The shell's count of frames actually presented, or `None`. An FPS meter
    /// differences this (`wrapping_sub`), not its own paint count.
    pub fn presented_frames(&self) -> Option<u64> {
        self.presented_frames
    }

    /// Seed the shell's presented-frame count.
    pub(crate) fn set_presented_frames(&mut self, presented: Option<u64>) {
        self.presented_frames = presented;
    }

    /// The widget's absolute window-space origin — the one to anchor overlays
    /// and reported rects from ([`ChildPod::origin`] is parent-relative).
    pub fn origin(&self) -> Point {
        self.origin
    }

    /// The widget's resolved size.
    pub fn size(&self) -> Size {
        self.size
    }

    /// Whether the widget holds the focus path — authoritative at paint, since
    /// a container-routed blur never reaches the widget's `event()`.
    pub fn has_focus(&self) -> bool {
        self.has_focus
    }

    /// Seed whether the widget being painted holds focus.
    pub(crate) fn set_has_focus(&mut self, has_focus: bool) {
        self.has_focus = has_focus;
    }

    /// The previous paint's shortcut declarations, as `BuildCtx::shortcut_hints` returns them.
    pub fn shortcut_hints(&self) -> std::rc::Rc<[crate::shortcuts::ShortcutHint]> {
        crate::shortcuts::last_hints()
    }

    /// Whether to draw a focus ring: [`PaintCtx::has_focus`] and the root's
    /// [`FocusRingPolicy`](crate::app::FocusRingPolicy) allowing one right now.
    pub fn is_focused_for_ring(&self) -> bool {
        self.has_focus && self.focus_ring
    }

    pub(crate) fn set_focus_ring(&mut self, visible: bool) {
        self.focus_ring = visible;
    }

    /// Whether this widget or a descendant is on the hover path (CSS `:hover`).
    /// Authoritative: the widget the pointer left gets no event, so a hover
    /// consumer self-corrects from this each paint.
    pub fn is_hovered(&self) -> bool {
        self.hovered
    }

    /// Seed whether the widget being painted holds the hover link.
    pub(crate) fn set_hovered(&mut self, hovered: bool) {
        self.hovered = hovered;
    }

    /// Seed the live hover epoch.
    pub(crate) fn set_hover_epoch(&mut self, epoch: u64) {
        self.hover_epoch = epoch;
    }

    /// The live hover epoch a pod's recorded stamp is compared against.
    pub(crate) fn hover_epoch(&self) -> u64 {
        self.hover_epoch
    }

    /// Seed the live focus epoch.
    pub(crate) fn set_focus_epoch(&mut self, epoch: u64) {
        self.focus_epoch = epoch;
    }

    /// The live focus epoch a pod's recorded stamp is compared against.
    pub(crate) fn focus_epoch(&self) -> u64 {
        self.focus_epoch
    }

    /// The shell clock for this frame; only ever difference it against a
    /// stored earlier value.
    pub fn frame_time(&self) -> FrameTime {
        self.frame_time
    }

    /// Seed the shell-provided frame time.
    pub(crate) fn set_frame_time(&mut self, frame_time: FrameTime) {
        self.frame_time = frame_time;
    }

    /// Request another frame because paint advanced animation state
    /// ([`TickClass::Transition`]).
    pub fn request_frame(&mut self) {
        self.request_frame_class(TickClass::Transition);
    }

    /// Request a pacable ([`TickClass::CosmeticLoop`]) frame at the theme's
    /// `cosmetic_loop_rate`.
    pub fn request_frame_paced(&mut self) {
        self.request_frame_paced_at(Duration::ZERO);
    }

    /// Request a pacable repaint at most once per `interval` (a caret blink).
    /// Intervals fold to the minimum, so a slow request never starves a fast
    /// one; capped at [`Self::MAX_PACED_INTERVAL`].
    pub fn request_frame_paced_at(&mut self, interval: Duration) {
        self.needs_frame = true;
        self.merge_paced_interval(interval.min(Self::MAX_PACED_INTERVAL));
    }

    /// Ceiling on paced intervals: above every real cadence, far below where
    /// the shells' pacing arithmetic (`interval * 2`, nanos `as u64`) overflows.
    pub const MAX_PACED_INTERVAL: Duration = Duration::from_secs(10);

    /// Request a frame of an explicit [`TickClass`].
    pub fn request_frame_class(&mut self, class: TickClass) {
        match class {
            TickClass::Transition => {
                self.needs_frame = true;
                self.frame_unpaced = true;
            }
            TickClass::CosmeticLoop => self.request_frame_paced(),
        }
    }

    /// Fold one paced interval into the min aggregate (sole mutation point).
    fn merge_paced_interval(&mut self, interval: Duration) {
        self.paced_interval = Some(match self.paced_interval {
            Some(current) => current.min(interval),
            None => interval,
        });
    }

    /// Fold a bubbled child's interval in, skipping `None` (a zero default
    /// would re-tighten to the theme cap though nothing asked).
    fn absorb_paced_interval(&mut self, interval: Option<Duration>) {
        if let Some(interval) = interval {
            self.merge_paced_interval(interval);
        }
    }

    /// Whether a continuation frame was requested during this (sub)paint.
    pub fn needs_frame(&self) -> bool {
        self.needs_frame
    }

    /// The aggregate [`TickClass`] requested, or `None` if no frame was.
    pub fn frame_class(&self) -> Option<TickClass> {
        if !self.needs_frame {
            None
        } else if self.frame_unpaced {
            Some(TickClass::Transition)
        } else {
            Some(TickClass::CosmeticLoop)
        }
    }

    /// Whether every frame request was [`TickClass::CosmeticLoop`] (paceable).
    pub fn needs_frame_paced_only(&self) -> bool {
        self.needs_frame && !self.frame_unpaced
    }

    /// The tightest paced interval named, or `None`; both mean the theme rate.
    pub fn paced_interval(&self) -> Option<Duration> {
        self.paced_interval
    }

    /// Request layout next frame because this paint's animation changes layout;
    /// implies an unpaced [`Self::request_frame`].
    pub fn request_layout(&mut self) {
        self.needs_layout = true;
        self.request_frame_class(TickClass::Transition);
    }

    /// Whether a layout re-run was requested during this (sub)paint.
    pub fn needs_layout(&self) -> bool {
        self.needs_layout
    }

    /// Whether this (sub)paint marked a pod at or below it layout-dirty.
    pub(crate) fn is_layout_path_dirty(&self) -> bool {
        self.layout_path_dirty
    }

    /// Publish the focused editable's IME surface, so an app-driven change that
    /// never crosses the event pass still reaches the shell.
    pub fn publish_ime_state(&mut self, state: ImeState) {
        self.note_publication();
        self.ime_state = Some(state);
    }

    /// Take the IME surface published during this (sub)paint, if any.
    pub fn take_ime_state(&mut self) -> Option<ImeState> {
        self.ime_state.take()
    }

    /// Publish a platform-view slot's frame (pushed; many slots per pass).
    pub fn publish_platform_view(&mut self, frame: PlatformViewFrame) {
        self.note_publication();
        self.platform_views.push(frame);
    }

    /// Take (and clear) every platform-view frame published this (sub)paint.
    pub fn take_platform_views(&mut self) -> Vec<PlatformViewFrame> {
        std::mem::take(&mut self.platform_views)
    }

    /// Report an absolute rect where frust content over a platform-view slot
    /// keeps winning input (a "z-shield"); the shell differ assigns it to slots.
    pub fn report_input_shield(&mut self, rect: Rect) {
        self.note_publication();
        self.input_shields.push(rect);
    }

    /// Take (and clear) every z-shield rect reported this (sub)paint.
    pub fn take_input_shields(&mut self) -> Vec<Rect> {
        std::mem::take(&mut self.input_shields)
    }

    /// Float `entry`'s pod above the app this frame and register it for input
    /// routing. The root paints it after the main tree, so the owner must not;
    /// the registry clears each pass, so the owner re-registers every paint.
    pub fn register_overlay(&mut self, entry: crate::overlay::OverlayEntry) {
        self.note_publication();
        crate::overlay::register(entry);
    }

    /// Declare a keyboard shortcut for this frame (see [`crate::shortcuts`]). The table clears
    /// each paint, so the declarer re-declares every paint; it answers its own
    /// `InputEvent::Shortcut` with `Handled`.
    pub fn register_shortcut(&mut self, decl: crate::shortcuts::ShortcutDecl) {
        self.note_publication();
        crate::shortcuts::register(decl);
    }

    /// Publish this frame's focused-field verbs and menu anchor for the
    /// platform edit menu. A focused field publishes every paint, so hardware
    /// shortcuts stay answerable; no publish means nothing is focused.
    pub fn publish_selection_toolbar(
        &mut self,
        request: crate::selection_toolbar::SelectionToolbarRequest,
    ) {
        self.note_publication();
        crate::selection_toolbar::publish(request);
    }

    /// Report a hero's absolute `bounds` and read back its directive; a no-op
    /// returning [`HeroDirective::Normal`] with no reporter installed.
    pub fn report_hero(&mut self, tag: &str, bounds: Rect) -> HeroDirective {
        self.note_publication();
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

    /// Whether a hero reporter is installed (culling containers stop culling).
    pub fn hero_active(&self) -> bool {
        self.hero.is_some()
    }

    /// Run `f` with `registry` as the hero reporter; requests and publications
    /// inside bubble back onto `self`.
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
            layout_path_dirty: false,
            frame_unpaced: false,
            paced_interval: None,
            ime_state: None,
            has_focus: self.has_focus,
            focus_ring: self.focus_ring,
            hovered: self.hovered,
            hover_epoch: self.hover_epoch,
            focus_epoch: self.focus_epoch,
            frame_time: self.frame_time,
            theme: self.theme,
            window_insets: self.window_insets,
            presented_frames: self.presented_frames,
            hero: Some(registry),
            visible_rect: self.visible_rect,
            platform_views: Vec::new(),
            input_shields: Vec::new(),
            translucent: self.translucent,
            paint_epoch: self.paint_epoch,
            retained_layers: self.retained_layers,
            retained_ledger: self.retained_ledger,
            paint_serial: self.paint_serial,
            paint_publications: 0,
        };
        f(&mut child);
        self.paint_publications = self
            .paint_publications
            .saturating_add(child.paint_publications);
        if child.needs_frame {
            self.needs_frame = true;
        }
        if child.needs_layout {
            self.needs_layout = true;
        }
        if child.layout_path_dirty {
            self.layout_path_dirty = true;
        }
        // Same absorb rules as `ChildPod::paint_child`.
        if child.frame_unpaced {
            self.frame_unpaced = true;
        }
        self.absorb_paced_interval(child.paced_interval);
        if let Some(ime) = child.ime_state.take() {
            self.ime_state = Some(ime);
        }
        self.platform_views.extend(child.platform_views);
        self.input_shields.extend(child.input_shields);
    }

    /// Seed the hero reporter lent by an ancestor.
    pub(crate) fn set_hero(&mut self, hero: Option<&'a RefCell<HeroFrames>>) {
        self.hero = hero;
    }

    /// The hero reporter, copied for re-lending to a child context.
    pub(crate) fn hero_ref(&self) -> Option<&'a RefCell<HeroFrames>> {
        self.hero
    }

    /// The absolute visible rect for culling, or `None` (paint everything).
    pub fn visible_rect(&self) -> Option<Rect> {
        self.visible_rect
    }

    /// Constrain the visible rect to `rect` (absolute); nested rects intersect.
    pub fn constrain_visible_rect(&mut self, rect: Rect) {
        self.visible_rect = Some(match self.visible_rect {
            Some(existing) => existing.intersect(rect),
            None => rect,
        });
    }

    /// Seed the visible rectangle lent by an ancestor.
    pub(crate) fn set_visible_rect(&mut self, rect: Option<Rect>) {
        self.visible_rect = rect;
    }

    /// The visible rectangle, copied for re-lending to a child context.
    pub(crate) fn visible_rect_ref(&self) -> Option<Rect> {
        self.visible_rect
    }

    /// Whether the shell surface is translucent ("Mode B"); the hole-punch
    /// clears only then.
    pub fn is_translucent(&self) -> bool {
        self.translucent
    }

    /// Seed the shell's surface-translucency flag.
    pub(crate) fn set_translucent(&mut self, translucent: bool) {
        self.translucent = translucent;
    }

    /// Seed the paint epoch, retained-layers switch, ledger and paint serial.
    pub(crate) fn set_paint_state(
        &mut self,
        paint_epoch: u64,
        retained_layers: bool,
        retained_ledger: Option<&'a crate::retained::RetainedLedger>,
        paint_serial: u64,
    ) {
        self.paint_epoch = paint_epoch;
        self.retained_layers = retained_layers;
        self.retained_ledger = retained_ledger;
        self.paint_serial = paint_serial;
    }

    /// The serial the root minted for this pass (`0` with no root), echoed by
    /// the ledger's [`frame`](crate::retained::RetainedLedger::frame).
    pub fn paint_serial(&self) -> u64 {
        self.paint_serial
    }

    /// Whether the renderer holds a page for `key` at `rect_px` and `scale`, per
    /// a ledger covering `recorded_serial` (guarding against a stale ledger).
    pub fn retained_available(
        &self,
        key: crate::retained::RetainedKey,
        rect_px: kurbo::Size,
        scale: f32,
        recorded_serial: u64,
    ) -> bool {
        let extent = (
            rect_px.width.round().max(0.0) as u32,
            rect_px.height.round().max(0.0) as u32,
        );
        self.retained_layers
            && self
                .retained_ledger
                .is_some_and(|l| l.covers(recorded_serial) && l.is_resident(key, extent, scale))
    }

    /// Whether nothing under `pod` changed since its last paint (ask about the
    /// child pod); hover and focus moves bump the paint epoch.
    pub fn subtree_clean(&self, pod: &ChildPod) -> bool {
        self.retained_layers && !pod.paint_dirty && pod.painted_epoch == self.paint_epoch
    }

    /// Whether a boundary may bracket its body (retained layers on, ledger set).
    pub fn retained_enabled(&self) -> bool {
        self.retained_layers && self.retained_ledger.is_some()
    }

    /// Side-channel publications (IME, platform view, shield, overlay, toolbar,
    /// hero) this (sub)paint made; a body that published must not be replayed.
    pub fn paint_publications(&self) -> u32 {
        self.paint_publications
    }

    /// Count one paint-time side-channel publication.
    fn note_publication(&mut self) {
        self.paint_publications = self.paint_publications.saturating_add(1);
    }
}

/// A tagged-rect ("hero") reporter a container installs over a subtree with
/// [`PaintCtx::with_hero_registry`] to find where tagged descendants painted.
#[derive(Debug, Default)]
pub struct HeroFrames {
    /// The enclosing surface's absolute top-left, subtracted from each report.
    reference: Point,
    /// Surface-local rects reported by tagged descendants this paint.
    captured: HashMap<String, Rect>,
    /// Per-tag paint directive the installer set for this paint.
    directives: HashMap<String, HeroDirective>,
}

impl HeroFrames {
    /// A reporter at `reference` (absolute top-left) with per-tag `directives`.
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

/// What a reported hero should do this paint ([`PaintCtx::report_hero`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HeroDirective {
    /// Paint normally — the default.
    Normal,
    /// Skip painting: the matching hero on the other surface morphs this one.
    Suppress,
    /// Paint under a transform mapping the hero's absolute bounds onto `dest`.
    Morph {
        /// The absolute destination rect the hero's bounds morph onto.
        dest: Rect,
    },
}

static NEXT_SLOT_ID: AtomicU64 = AtomicU64::new(1);

/// Allocate a fresh platform-view slot id once per widget instance — never
/// from tree position, so a reorder keeps identity.
pub fn next_slot_id() -> u64 {
    NEXT_SLOT_ID.fetch_add(1, Ordering::Relaxed)
}

/// Cap on the undrained retire list (nothing drains on desktop). Past it the
/// oldest id drops; the differ's missing-streak backstop still disposes it.
const MAX_PENDING_RETIRED_SLOTS: usize = 256;

/// Process-wide: a widget being torn down has no `RenderRoot` handle.
static RETIRED_SLOTS: Mutex<Vec<u64>> = Mutex::new(Vec::new());

/// Record that the widget owning `slot_id` was torn down, so the shell disposes
/// the native view promptly (teardown runs mid-rebuild, outside paint).
pub fn report_retired_slot(slot_id: u64) {
    let mut pending = RETIRED_SLOTS.lock().unwrap_or_else(|e| e.into_inner());
    if pending.len() >= MAX_PENDING_RETIRED_SLOTS {
        pending.remove(0);
    }
    pending.push(slot_id);
}

/// Take (and clear) every retired slot id since the last call, in teardown order.
pub fn take_retired_slots() -> Vec<u64> {
    let mut pending = RETIRED_SLOTS.lock().unwrap_or_else(|e| e.into_inner());
    std::mem::take(&mut *pending)
}

/// A platform-view slot's paint-time frame, in logical px and absolute window
/// coordinates. Per pass: a slot that did not paint has none, and the shell
/// differ decides hide/dispose.
#[derive(Clone, Debug, PartialEq)]
pub struct PlatformViewFrame {
    /// Stable per-instance id from [`next_slot_id`].
    pub slot_id: u64,
    /// `"dev.frust.<Factory>"`: the native view factory creating this slot.
    pub view_type: String,
    /// Opaque creation params for the native factory (may be empty).
    pub params_json: String,
    /// Bumped by the widget whenever `params_json` changes.
    pub params_generation: u64,
    /// Absolute paint bounds.
    pub rect: Rect,
    /// `visible_rect` intersected with `rect`, or `None` if fully visible.
    pub clip: Option<Rect>,
    /// `false` when the widget itself hides it (distinct from being absent).
    pub visible: bool,
    /// Mode B input forwarding: a touch-DOWN in `rect` outside every shield
    /// goes to the native view.
    pub interactive: bool,
    /// Manually declared z-shields, used only when `interactive`; the shell
    /// differ merges in the auto-collected ones.
    pub shields: Vec<Rect>,
    /// Painter-order position ([`PaintScene::command_index`]) taken before the
    /// slot's own punch; later commands paint over the native view.
    pub command_index: usize,
}

/// The result of a whole [`crate::app::RenderRoot::paint`] pass.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PaintOutcome {
    /// Whether the shell should schedule another frame.
    pub needs_frame: bool,
    /// Whether a widget called [`PaintCtx::request_layout`].
    pub needs_layout: bool,
    /// Whether every frame request was [`TickClass::CosmeticLoop`] (paceable).
    pub needs_frame_paced_only: bool,
    /// The tightest paced interval; `None` and `Some(ZERO)` both mean the
    /// theme rate.
    pub paced_interval: Option<Duration>,
    /// The paint serial, handed to the renderer with the scene.
    pub paint_serial: u64,
}

/// A retained UI element living in the widget tree. `Widget: Any` so a boxed
/// widget can be downcast back to its concrete type during rebuild.
pub trait Widget: Any {
    /// Choose a size within `bc` and return it. Under a render root the call
    /// may be skipped (see [`ChildPod::layout_child`]), so the result must
    /// depend only on `bc`, the context's globals and state changed through a
    /// pod-dirtying channel: a `LAYOUT` rebuild, `request_layout`, an unpaced
    /// frame request, or a handled / redraw-requesting event.
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size;

    /// Emit draw commands into `scene`. Paint may advance animation state; it
    /// must then call [`PaintCtx::request_frame`] until the animation rests.
    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene);

    /// Handle an input event and report whether it was consumed; defaults to
    /// [`EventResult::Ignored`]. Containers forward via [`ChildPod::event_child`].
    fn event(&mut self, _ctx: &mut EventCtx, _event: &InputEvent) -> EventResult {
        EventResult::Ignored
    }

    /// Contribute accessibility node(s) after layout; defaults to none. A
    /// container forwards via [`ChildPod::semantics_child`].
    fn semantics(&self, _ctx: &mut SemanticsCtx) {}

    /// This widget's concrete type name, for diagnostics only (`docs/LIMITATIONS.md`
    /// `focus-wrapper-erasure-swap-blind`); override only in a transparent wrapper.
    fn type_name(&self) -> &'static str {
        core::any::type_name::<Self>()
    }

    /// Visit this widget's owned [`ChildPod`]s in paint order, for read-only
    /// tooling; defaults to none and must not mutate pass state.
    fn visit_children(&self, _visitor: &mut dyn FnMut(&ChildPod)) {}

    /// Whether keyboard traversal (Tab / Shift+Tab at the root) may stop on this widget's pod.
    fn is_focusable(&self) -> bool {
        false
    }

    /// Visit the children traversal may enter, in focus order; defaults to
    /// [`Widget::visit_children`]. A container showing only some children narrows it.
    fn visit_focus_children(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        self.visit_children(visitor);
    }

    /// This widget as a sliver ([`crate::sliver`]), if it speaks that protocol.
    /// Defaults to `None`; transparent wrappers forward it.
    fn as_sliver(&mut self) -> Option<&mut dyn SliverWidget> {
        None
    }
}

impl dyn Widget {
    /// Attempt to downcast this trait object to a concrete widget type.
    pub fn downcast_mut<W: Widget>(&mut self) -> Option<&mut W> {
        let any: &mut dyn Any = self;
        any.downcast_mut::<W>()
    }
}

/// A boxed widget is itself a [`Widget`], delegating every pass — what lets an
/// `AnyView`'s `Box<dyn Widget>` element sit in a [`ChildPod`].
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

    fn visit_children(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        (**self).visit_children(visitor);
    }

    fn is_focusable(&self) -> bool {
        (**self).is_focusable()
    }

    fn visit_focus_children(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        (**self).visit_focus_children(visitor);
    }

    /// Forward the boxed widget's sliver face, so erasure never hides a sliver.
    fn as_sliver(&mut self) -> Option<&mut dyn SliverWidget> {
        (**self).as_sliver()
    }

    /// Report the boxed widget's name, through any number of box layers.
    fn type_name(&self) -> &'static str {
        (**self).type_name()
    }
}

/// A container's owned child: a boxed widget plus the geometry and
/// capture/focus/hover bookkeeping the container keeps for it. Children are
/// pods, not arena nodes; `origin`/`size` are in the container's space.
pub struct ChildPod {
    widget: Box<dyn Widget>,
    origin: Point,
    size: Size,
    /// An optional transform relative to `origin` ([`ChildPod::set_transform`]).
    transform: Option<Affine>,
    /// Set while the child holds a pointer capture.
    active: bool,
    /// Whether the child itself opted into contacts on this capture.
    contacts_captor: bool,
    /// Whether the child or a descendant opted into contacts on this capture.
    contacts_path: bool,
    /// Set when a focus claim passed through; says nothing about the live
    /// session on its own (read [`ChildPod::holds_live_focus`]).
    focused: bool,
    /// The focus session epoch this link was stamped with; never cleared, so a
    /// moved session strands older links by arithmetic.
    focus_epoch: u64,
    /// The root identity paired with `focus_epoch` (per-root epochs collide).
    focus_root: u64,
    /// The hover epoch this link was stamped with; never cleared. Hover has no
    /// leave event, so an advancing live epoch strands stale stamps.
    hover_epoch: u64,
    /// The root identity paired with `hover_epoch`, read only by `Drop`.
    hover_root: u64,
    /// Persistent semantics base id, assigned on the first semantics visit.
    semantics_id: Cell<Option<NonZeroU64>>,
    /// An optional human name for tooling.
    debug_label: Option<Cow<'static, str>>,
    /// Persistent tooling id, assigned on the first `inspect_id` call.
    inspect_id: Cell<Option<NonZeroU64>>,
    /// Whether the next layout must run whatever the constraints.
    layout_dirty: bool,
    /// Whether anything under this pod may have changed since its last paint.
    paint_dirty: bool,
    painted_epoch: u64,
    /// The skip key: constraints, epoch and insets of the last layout.
    last_constraints: Option<BoxConstraints>,
    last_epoch: u64,
    last_insets: WindowInsets,
    /// Whether the last paint requested a frame or layout (for the landing paint).
    painted_animating: bool,
    /// The sliver skip key; at most one of this and `last_constraints` is set.
    last_sliver_constraints: Option<SliverConstraints>,
    /// The geometry the last sliver layout reported.
    sliver_geometry: Option<SliverGeometry>,
}

thread_local! {
    /// The next tooling id [`ChildPod::inspect_id`] hands out (UI-thread only).
    static NEXT_INSPECT_ID: Cell<u64> = const { Cell::new(ChildPod::INSPECT_ID_BASE) };

    /// The inspect id of the pod that recorded this dispatch's hover claim.
    static HOVER_CLAIMANT: Cell<Option<u64>> = const { Cell::new(None) };

    /// The thread's focus session as `(root, live epoch, claim epoch)`; the
    /// claim epoch is a dispatch's candidate session. `(0, 0, 0)` until a root
    /// publishes, which a real root never does.
    static LIVE_FOCUS_SESSION: Cell<(u64, u64, u64)> = const { Cell::new((0, 0, 0)) };

    /// The open root→pod focus grant as `(carrier, target inspect id, landed)`; lands once.
    static FOCUS_GRANT: Cell<Option<(ShortcutId, u64, bool)>> = const { Cell::new(None) };
}

/// A root→pod focus grant ([`RenderRoot`](crate::app::RenderRoot)'s Tab traversal), open for one
/// delivery of `InputEvent::Shortcut(carrier)`: the pod whose [`ChildPod::inspect_id`] is the
/// target turns it into the state [`EventCtx::request_focus`] produces, so its own pod and every
/// ancestor's record the link as for a claim. Closed on drop.
pub(crate) struct FocusGrant;

impl FocusGrant {
    pub(crate) fn enter(carrier: ShortcutId, target: u64) -> Self {
        FOCUS_GRANT.with(|grant| grant.set(Some((carrier, target, false))));
        FocusGrant
    }
}

impl Drop for FocusGrant {
    fn drop(&mut self) {
        FOCUS_GRANT.with(|grant| grant.set(None));
    }
}

/// Publish the focus session standing on this thread: `live` is the session a
/// link must name to count, `claim` the one a link recorded now takes. A
/// thread-local because pod destructors run with no context.
pub(crate) fn set_live_focus_session(root: u64, live: u64, claim: u64) {
    LIVE_FOCUS_SESSION.with(|slot| slot.set((root, live, claim)));
}

/// The `(root, epoch)` a link recorded now is stamped with.
fn focus_claim_stamp() -> (u64, u64) {
    let (root, _live, claim) = LIVE_FOCUS_SESSION.with(|slot| slot.get());
    (root, claim)
}

/// Whether `(root, epoch)` names the live session or the open candidate.
fn names_live_focus_session(root: u64, epoch: u64) -> bool {
    let (live_root, live, claim) = LIVE_FOCUS_SESSION.with(|slot| slot.get());
    root == live_root && (epoch == live || epoch == claim)
}

impl Drop for ChildPod {
    /// Report a live hover link severed by this pod's removal, so
    /// [`RenderRoot::rebuild`](crate::app::RenderRoot::rebuild) ends the hover.
    fn drop(&mut self) {
        if crate::event::live_hover_link_is(self.hover_root, self.hover_epoch) {
            crate::event::mark_hover_orphaned(self.hover_root);
        }
        // Focus half: an overlay-floated pod is severed by no reconciler. A zero
        // epoch means the link was never held.
        if self.focused
            && self.focus_epoch != 0
            && names_live_focus_session(self.focus_root, self.focus_epoch)
        {
            crate::event::mark_focus_orphaned();
        }
    }
}

/// Forget the recorded hover claimant (called before a dispatch).
pub(crate) fn clear_hover_claimant() {
    HOVER_CLAIMANT.with(|claimant| claimant.set(None));
}

/// Take the inspect id of the pod that recorded the last hover claim.
pub(crate) fn take_hover_claimant() -> Option<u64> {
    HOVER_CLAIMANT.with(Cell::take)
}

impl ChildPod {
    /// Where [`ChildPod::inspect_id`]'s allocator starts: 2^48, far from arena
    /// `WidgetId`s.
    pub const INSPECT_ID_BASE: u64 = 1 << 48;

    /// Wrap a freshly built child widget at the origin with zero size.
    pub fn new(widget: Box<dyn Widget>) -> Self {
        Self {
            widget,
            origin: Point::ZERO,
            size: Size::ZERO,
            transform: None,
            active: false,
            contacts_captor: false,
            contacts_path: false,
            focused: false,
            focus_epoch: 0,
            focus_root: 0,
            hover_epoch: 0,
            hover_root: 0,
            semantics_id: Cell::new(None),
            debug_label: None,
            inspect_id: Cell::new(None),
            layout_dirty: true,
            paint_dirty: true,
            painted_epoch: 0,
            last_constraints: None,
            last_epoch: 0,
            last_insets: WindowInsets::default(),
            painted_animating: false,
            last_sliver_constraints: None,
            sliver_geometry: None,
        }
    }

    /// The wrapped widget's concrete type name, asked of the live widget
    /// ([`Widget::type_name`]). Diagnostic only; never parse it.
    pub fn type_name(&self) -> &'static str {
        self.widget.type_name()
    }

    /// The human name attached for tooling, if any. `None` by default.
    pub fn debug_label(&self) -> Option<&str> {
        self.debug_label.as_deref()
    }

    /// Attach a human name for tooling; nothing in the passes reads it.
    pub fn set_debug_label(&mut self, label: impl Into<Cow<'static, str>>) {
        self.debug_label = Some(label.into());
    }

    /// Drop any attached debug label.
    pub fn clear_debug_label(&mut self) {
        self.debug_label = None;
    }

    /// The attached label as an owned [`Cow`], for an inspect snapshot.
    pub(crate) fn debug_label_cow(&self) -> Option<Cow<'static, str>> {
        self.debug_label.clone()
    }

    /// This pod's tooling id, assigned on first call and stable thereafter.
    pub fn inspect_id(&self) -> crate::view::WidgetId {
        let id = match self.inspect_id.get() {
            Some(id) => id,
            None => {
                let id = NEXT_INSPECT_ID.with(|next| {
                    let id = next.get();
                    next.set(id + 1);
                    NonZeroU64::new(id).expect("the allocator starts at 2^48, never zero")
                });
                self.inspect_id.set(Some(id));
                id
            }
        };
        crate::view::WidgetId(id.get())
    }

    /// Shared access to the boxed child widget.
    pub fn widget(&self) -> &dyn Widget {
        &*self.widget
    }

    /// Mutable access to the boxed widget; marks the pod layout-dirty.
    pub fn widget_mut(&mut self) -> &mut dyn Widget {
        self.layout_dirty = true;
        self.paint_dirty = true;
        &mut *self.widget
    }

    /// Mutable access without marking the pod dirty; the caller marks it.
    pub fn widget_mut_for_rebuild(&mut self) -> &mut dyn Widget {
        &mut *self.widget
    }

    /// Replace the boxed child widget; marks the pod layout-dirty.
    pub fn set_widget(&mut self, widget: Box<dyn Widget>) {
        self.widget = widget;
        self.layout_dirty = true;
        self.paint_dirty = true;
    }

    /// Whether the next layout must run (tooling and tests).
    pub fn is_layout_dirty(&self) -> bool {
        self.layout_dirty
    }

    /// Mark this pod (only) layout-dirty; a caller outside a rebuild must also
    /// dirty its path with `request_layout`.
    pub fn mark_layout_dirty(&mut self) {
        self.layout_dirty = true;
        self.paint_dirty = true;
    }

    /// Whether the subtree must be painted afresh (tooling and tests).
    pub fn is_paint_dirty(&self) -> bool {
        self.paint_dirty
    }

    /// Mark this pod (only) paint-dirty.
    pub fn mark_paint_dirty(&mut self) {
        self.paint_dirty = true;
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

    /// The child's transform, if any (see [`ChildPod::set_transform`]).
    pub fn transform(&self) -> Option<Affine> {
        self.transform
    }

    /// Place the child under `transform` (local space, before `origin`: `p`
    /// lands at `origin + transform * p`), or clear it with `None`. Paint, hit
    /// tests and events map through it; semantics reports its bounding box. A
    /// non-invertible transform draws and hits nothing. Window-space rects
    /// reported from `paint` are not mapped.
    pub fn set_transform(&mut self, transform: Option<Affine>) {
        self.transform = transform;
    }

    /// The child's local→container mapping: `translate(origin) * transform`.
    fn local_to_container(&self, transform: Affine) -> Affine {
        Affine::translate(self.origin.to_vec2()) * transform
    }

    /// Whether this child currently holds the recorded active (captured) path.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Set (or clear) the recorded active path. A clear is refused during a
    /// non-claimant contact's walk, so it cannot strand the claimant.
    pub fn set_active(&mut self, active: bool) {
        if !active && crate::event::in_secondary_contact_pass() {
            return;
        }
        self.active = active;
        if !active {
            self.contacts_captor = false;
            self.contacts_path = false;
        }
    }

    /// Whether the active path leads to the widget that opted into the
    /// gesture's other contacts ([`EventCtx::capture_contacts`]).
    pub(crate) fn holds_contact_opt_in(&self) -> bool {
        self.active && self.contacts_path
    }

    /// Whether this child has a recorded focus path at all; routing needs
    /// [`ChildPod::holds_live_focus`].
    pub fn is_focused(&self) -> bool {
        self.focused
    }

    /// Record (or drop) this child's focus path, stamping the live session.
    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        if focused {
            let (root, epoch) = focus_claim_stamp();
            self.focus_root = root;
            self.focus_epoch = epoch;
        }
    }

    /// The focus epoch stamp (diagnostic; non-zero does not mean focused).
    pub fn focus_epoch(&self) -> u64 {
        self.focus_epoch
    }

    /// Whether this child holds a focus link on the root's live session (flag
    /// set and stamp matching). Without a root both read `(0, 0)`.
    pub fn holds_live_focus(&self) -> bool {
        self.focused && names_live_focus_session(self.focus_root, self.focus_epoch)
    }

    /// Drop a focus link the live session already stranded, reporting whether
    /// one was dropped; a live link is never retired.
    pub fn retire_stale_focus_link(&mut self) -> bool {
        if self.focused && !self.holds_live_focus() {
            self.focused = false;
            true
        } else {
            false
        }
    }

    /// The hover epoch stamp (`0`: never claimed). Diagnostic only — read
    /// [`PaintCtx::is_hovered`] instead.
    pub fn hover_epoch(&self) -> u64 {
        self.hover_epoch
    }

    /// Lay the child out under `bc` and return its size, or return the recorded
    /// size without calling [`Widget::layout`] when the pass is not forced, the
    /// pod is clean, and `bc`, the epoch and the insets match last time's.
    /// Dirty sources: a `LAYOUT` rebuild or swap, a frame or layout request
    /// below (plus the landing paint), a handled or redraw event,
    /// [`ChildPod::widget_mut`], a fresh build.
    pub fn layout_child(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        if !ctx.force
            && !self.layout_dirty
            && self.last_constraints == Some(*bc)
            && self.last_epoch == ctx.epoch
            && self.last_insets == ctx.window_insets
        {
            return self.size;
        }
        let size = self.widget.layout(ctx, bc);
        self.size = size;
        self.last_constraints = Some(*bc);
        self.last_sliver_constraints = None;
        self.sliver_geometry = None;
        self.last_epoch = ctx.epoch;
        self.last_insets = ctx.window_insets;
        self.layout_dirty = false;
        self.paint_dirty = true;
        size
    }

    /// Lay the child out as a sliver under `sc`, or return `None` for a
    /// non-sliver child. Skips like [`ChildPod::layout_child`]; a geometry with
    /// a scroll-offset correction leaves the pod dirty so it is never replayed.
    pub fn layout_sliver_child(
        &mut self,
        ctx: &mut LayoutCtx,
        sc: &SliverConstraints,
    ) -> Option<SliverGeometry> {
        if !ctx.force
            && !self.layout_dirty
            && self.last_sliver_constraints == Some(*sc)
            && self.last_epoch == ctx.epoch
            && self.last_insets == ctx.window_insets
            && let Some(geometry) = self.sliver_geometry
        {
            return Some(geometry);
        }
        sc.debug_assert_valid();
        let geometry = self.widget.as_sliver()?.layout_sliver(ctx, sc);
        geometry.debug_assert_valid(sc);
        self.size = sc.axis.size(geometry.paint_extent, sc.cross_axis_extent);
        self.sliver_geometry = Some(geometry);
        self.last_sliver_constraints = Some(*sc);
        self.last_constraints = None;
        self.last_epoch = ctx.epoch;
        self.last_insets = ctx.window_insets;
        self.layout_dirty = geometry.scroll_offset_correction.is_some();
        self.paint_dirty = true;
        Some(geometry)
    }

    /// The geometry of the last sliver layout, if it was one.
    pub fn sliver_geometry(&self) -> Option<SliverGeometry> {
        self.sliver_geometry
    }

    /// Whether the child speaks the sliver protocol (does not dirty the pod).
    pub fn is_sliver(&mut self) -> bool {
        self.widget.as_sliver().is_some()
    }

    /// Paint the child at its absolute position, bubbling its requests and
    /// publications into `ctx`; a non-invertible transform skips it.
    pub fn paint_child(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let Some(transform) = self.transform else {
            let visible_rect = ctx.visible_rect_ref();
            self.paint_child_in(ctx, scene, visible_rect);
            return;
        };
        // A singular transform leaves nothing to draw and no inverse.
        if crate::hit::checked_inverse(&transform).is_none() {
            return;
        }
        // Conjugate the local transform by the absolute origin: a local point
        // drawn at `abs + p` lands at `abs + transform * p`.
        let abs = (ctx.origin() + self.origin.to_vec2()).to_vec2();
        let around = Affine::translate(abs) * transform * Affine::translate(-abs);
        // Culling descendants get the visible rect's preimage bounding box.
        let visible_rect = match (ctx.visible_rect_ref(), crate::hit::checked_inverse(&around)) {
            (Some(rect), Some(inverse)) => Some(inverse.transform_rect_bbox(rect)),
            _ => None,
        };
        scene.push_transform(around);
        self.paint_child_in(ctx, scene, visible_rect);
        scene.pop_transform();
    }

    /// [`ChildPod::paint_child`]'s body, with `visible_rect` as the culling rect.
    fn paint_child_in(
        &mut self,
        ctx: &mut PaintCtx,
        scene: &mut dyn PaintScene,
        visible_rect: Option<Rect>,
    ) {
        let child_origin = ctx.origin() + self.origin.to_vec2();
        let mut child_ctx = PaintCtx::new(child_origin, self.size);
        // Copy the root-threaded globals down unchanged.
        child_ctx.set_frame_time(ctx.frame_time());
        child_ctx.set_theme(ctx.theme_ref());
        child_ctx.set_window_insets(ctx.window_insets_ref());
        child_ctx.set_presented_frames(ctx.presented_frames());
        child_ctx.set_hero(ctx.hero_ref());
        child_ctx.set_visible_rect(visible_rect);
        child_ctx.set_translucent(ctx.is_translucent());
        child_ctx.set_paint_state(
            ctx.paint_epoch,
            ctx.retained_layers,
            ctx.retained_ledger,
            ctx.paint_serial,
        );
        // Focus: a recorded link, on the live session (paint reaches branches no
        // pass clears), under a focused ancestor chain.
        child_ctx.set_focus_epoch(ctx.focus_epoch());
        child_ctx.set_has_focus(
            self.focused && self.focus_epoch == ctx.focus_epoch() && ctx.has_focus(),
        );
        child_ctx.set_focus_ring(ctx.focus_ring);
        // Hover likewise; the chain keeps a bare context's `0` from matching.
        child_ctx.set_hover_epoch(ctx.hover_epoch());
        child_ctx.set_hovered(self.hover_epoch == ctx.hover_epoch() && ctx.is_hovered());
        self.widget.paint(&mut child_ctx, scene);
        ctx.paint_publications = ctx
            .paint_publications
            .saturating_add(child_ctx.paint_publications);
        match child_ctx.frame_class() {
            Some(TickClass::Transition) => ctx.request_frame_class(TickClass::Transition),
            // Not `request_frame_class(CosmeticLoop)`: that folds in
            // `Duration::ZERO` and would re-tighten a slower child cadence.
            Some(TickClass::CosmeticLoop) => {
                ctx.needs_frame = true;
                debug_assert!(
                    child_ctx.paced_interval().is_some(),
                    "CosmeticLoop frame_class with no merged paced_interval — \
                     a new caller must be bypassing request_frame_paced_at"
                );
                ctx.absorb_paced_interval(child_ctx.paced_interval());
            }
            None => {}
        }
        if child_ctx.needs_layout() {
            ctx.request_layout();
        }
        // Layout-skip marks: requests and the landing paint dirty the pod; the
        // path bit goes up except for paced-only requests, preserving pacing.
        let animating = child_ctx.needs_layout() || child_ctx.needs_frame();
        let dirtied = animating || self.painted_animating || child_ctx.layout_path_dirty;
        if dirtied {
            self.layout_dirty = true;
        }
        // Painted this pass: paint-dirty only while still changing.
        self.paint_dirty = dirtied;
        self.painted_epoch = ctx.paint_epoch;
        let unpaced = child_ctx.needs_layout()
            || (child_ctx.needs_frame() && !child_ctx.needs_frame_paced_only());
        let landing = self.painted_animating && !animating;
        if unpaced || landing || child_ctx.layout_path_dirty {
            ctx.layout_path_dirty = true;
        }
        self.painted_animating = animating;
        if let Some(ime) = child_ctx.take_ime_state() {
            ctx.publish_ime_state(ime);
        }
        // Extend, never overwrite: sibling slots and shields must all survive.
        ctx.platform_views.extend(child_ctx.take_platform_views());
        ctx.input_shields.extend(child_ctx.take_input_shields());
    }

    /// Collect the child's semantics at its absolute origin (a transformed pod
    /// reports its bounding box).
    pub fn semantics_child(&self, ctx: &mut SemanticsCtx) {
        let base = self.semantics_base(ctx);
        let (offset, size) = match self.transform {
            None => (self.origin.to_vec2(), self.size),
            Some(transform) => {
                let bounds = self
                    .local_to_container(transform)
                    .transform_rect_bbox(self.size.to_rect());
                if bounds.is_finite() {
                    (bounds.origin().to_vec2(), bounds.size())
                } else {
                    // A non-finite transform reports an empty frame, not NaN.
                    (self.origin.to_vec2(), Size::ZERO)
                }
            }
        };
        ctx.descend_into_pod(base, offset, size, |ctx| {
            self.widget.semantics(ctx);
        });
    }

    /// This pod's stable semantics base id, assigned on first visit.
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

    /// Route an event into the child in its local space, folding its flags into
    /// `ctx` and recording a capture as the active path. Containers use
    /// `frust-widgets`' `route_event` helpers, not an ad-hoc `contains()` gate.
    ///
    /// A non-claimant contact walks forward-only: pods above the captor hand
    /// their child an inert carrier, so no container re-arms its gesture from
    /// the wrong finger; the captor receives the real event.
    pub fn event_child(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if let Some(real) = crate::event::secondary_walk_event(event) {
            return self.walk_secondary(ctx, &real);
        }
        let local = self.localize(event);
        self.dispatch_local(ctx, &local)
    }

    /// `event` mapped into the child's local space (`-origin` or the inverse).
    fn localize(&self, event: &InputEvent) -> InputEvent {
        match self.transform {
            None => event.translated(-self.origin.to_vec2()),
            Some(transform) => {
                match crate::hit::checked_inverse(&self.local_to_container(transform)) {
                    Some(inverse) => event.transformed(&inverse),
                    // No inverse: only a capture/focus path reaches here; deliver
                    // it untransformed so that path can still end.
                    None => event.translated(-self.origin.to_vec2()),
                }
            }
        }
    }

    /// One step of a non-claimant contact's forward-only walk.
    fn walk_secondary(&mut self, ctx: &mut EventCtx, real: &InputEvent) -> EventResult {
        if !self.active {
            return EventResult::Ignored;
        }
        let local = self.localize(real);
        let result = if self.contacts_captor {
            crate::event::without_secondary_walk(|| self.dispatch_local(ctx, &local))
        } else {
            let carrier = crate::event::secondary_walk_carrier();
            let (_, delivered) = crate::event::run_secondary_walk(local.clone(), || {
                self.dispatch_local(ctx, &carrier)
            });
            match delivered {
                Some(result) => result,
                None => crate::event::without_secondary_walk(|| self.dispatch_local(ctx, &local)),
            }
        };
        crate::event::note_secondary_delivered(result);
        result
    }

    /// Dispatch `local` to the child widget and fold what it bubbled into `ctx`.
    fn dispatch_local(&mut self, ctx: &mut EventCtx, local: &InputEvent) -> EventResult {
        let hovered = self.hover_epoch == ctx.hover_epoch() && ctx.is_hovered();
        // A capturing pod never lets a hover claim through, and the first
        // claim recorded (the topmost claimant's) closes the pass.
        let hover_eligible = ctx.is_hover_eligible() && !ctx.is_hover_claimed() && !self.active;
        let claim_epoch = ctx.hover_claim_epoch();
        let ((opted_in, opted_in_at_or_below), fold, (layout_requested, layout_path_dirty), result) = {
            let mut child_ctx = ctx.child_ctx(
                self.origin,
                self.size,
                self.focused,
                hovered,
                hover_eligible,
            );
            if let InputEvent::Shortcut(id) = local
                && self.takes_focus_grant(*id)
            {
                child_ctx.request_focus();
            }
            // Attributes a `capture_contacts` call to this child or below.
            let frame = crate::event::ContactFrame::enter(self.active && self.contacts_captor);
            let result = self.widget.event(&mut child_ctx, local);
            if matches!(local, InputEvent::Shortcut(_)) && result == EventResult::Handled {
                crate::shortcuts::note_handled();
            }
            let (opted_in, opted_in_at_or_below) = frame.close();
            (
                (opted_in, opted_in_at_or_below),
                child_ctx.fold(),
                (child_ctx.needs_layout(), child_ctx.is_layout_path_dirty()),
                result,
            )
        };
        let (captured, hover_claimed) = (fold.captured, fold.hover_claimed);
        let (focus_req, focus_rel, redraw) =
            (fold.focus_requested, fold.focus_released, fold.needs_redraw);
        // The event pass's layout-skip source, folded into `ctx` as the path
        // bit: a broadcast container returns `Ignored` whatever its child did.
        let path_dirty = layout_path_dirty
            || layout_requested
            || redraw
            || matches!(result, EventResult::Handled);
        if path_dirty {
            self.layout_dirty = true;
            self.paint_dirty = true;
        }
        ctx.absorb_layout(layout_requested, path_dirty);
        if captured {
            if !self.active {
                // A fresh capture drops the last gesture's opt-in.
                self.contacts_captor = false;
                self.contacts_path = false;
            }
            self.active = true;
            self.contacts_captor |= opted_in;
            self.contacts_path |= opted_in_at_or_below;
        }
        // Stamp the claim; a stale stamp is stranded by the next epoch.
        if hover_claimed {
            self.hover_epoch = claim_epoch;
            self.hover_root = ctx.hover_root();
            // Stamps land bottom-up, so the first is the claimant's own pod.
            if HOVER_CLAIMANT.with(Cell::get).is_none() {
                let id = self.inspect_id().0;
                HOVER_CLAIMANT.with(|claimant| claimant.set(Some(id)));
            }
        }
        // A focus request wins over a release in one dispatch.
        if focus_rel {
            self.set_focused(false);
        }
        if focus_req {
            self.set_focused(true);
        }
        ctx.absorb_child(fold);
        result
    }

    /// Whether this pod is the open [`FocusGrant`]'s target for `carrier`; marks it landed.
    fn takes_focus_grant(&self, carrier: ShortcutId) -> bool {
        let Some(id) = self.inspect_id.get() else {
            return false;
        };
        FOCUS_GRANT.with(|grant| match grant.get() {
            Some((open, target, false)) if open == carrier && target == id.get() => {
                grant.set(Some((open, target, true)));
                true
            }
            _ => false,
        })
    }

    /// Whether `point` (container space) lies in this child's bounds, half-open
    /// (`origin <= p < origin + size`). The initial hit test only.
    pub fn contains(&self, point: Point) -> bool {
        if let Some(transform) = self.transform {
            return crate::hit::point_in_transformed_rect(
                point,
                self.size.to_rect(),
                &self.local_to_container(transform),
            );
        }
        point.x >= self.origin.x
            && point.x < self.origin.x + self.size.width
            && point.y >= self.origin.y
            && point.y < self.origin.y + self.size.height
    }
}

#[cfg(test)]
mod focus_link_tests {
    use super::*;

    /// A guard restoring the focus channel this thread started with, so a test
    /// that publishes a session of its own cannot leak it into a neighbour.
    struct Session(u64, u64, u64);

    impl Session {
        fn enter(root: u64, live: u64, claim: u64) -> Self {
            let previous = LIVE_FOCUS_SESSION.with(|slot| slot.get());
            set_live_focus_session(root, live, claim);
            Session(previous.0, previous.1, previous.2)
        }
    }

    impl Drop for Session {
        fn drop(&mut self) {
            set_live_focus_session(self.0, self.1, self.2);
        }
    }

    /// A do-nothing leaf: these tests exercise the pod's own bookkeeping, so
    /// the widget inside it never runs.
    struct Inert;

    impl Widget for Inert {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(10.0, 10.0))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    }

    fn pod() -> ChildPod {
        ChildPod::new(Box::new(Inert))
    }

    #[test]
    fn a_recorded_link_counts_only_while_its_stamp_names_the_live_session() {
        let _session = Session::enter(7, 3, 3);
        let mut pod = pod();
        pod.set_focused(true);
        assert!(pod.holds_live_focus(), "recorded against the live session");
        assert_eq!(pod.focus_epoch(), 3);

        // The session moves on. Nothing visits this pod; its flag is untouched.
        set_live_focus_session(7, 4, 4);
        assert!(
            pod.is_focused(),
            "the raw record survives, as it always did"
        );
        assert!(
            !pod.holds_live_focus(),
            "but it no longer names the session the root has"
        );
    }

    #[test]
    fn a_link_of_another_root_never_counts_however_the_epochs_line_up() {
        let _session = Session::enter(7, 3, 3);
        let mut pod = pod();
        pod.set_focused(true);

        // A second root on the same thread, with the identical epoch integer —
        // which two roots hold as a rule, not as a fluke.
        set_live_focus_session(8, 3, 3);
        assert!(!pod.holds_live_focus());
    }

    #[test]
    fn a_claim_recorded_during_a_dispatch_counts_before_the_dispatch_commits_it() {
        // The root opened a candidate session: reads still answer against the
        // live one, and a link recorded now takes the candidate.
        let _session = Session::enter(7, 3, 4);
        let mut claimed = pod();
        claimed.set_focused(true);
        assert_eq!(claimed.focus_epoch(), 4, "stamped with the candidate");
        assert!(
            claimed.holds_live_focus(),
            "and observable to the container still unwinding around the claim"
        );

        let mut standing = pod();
        standing.focused = true;
        standing.focus_root = 7;
        standing.focus_epoch = 3;
        assert!(
            standing.holds_live_focus(),
            "while a link recorded before this dispatch still reads live"
        );

        // The dispatch commits the claim: the older link is stranded.
        set_live_focus_session(7, 4, 4);
        assert!(claimed.holds_live_focus());
        assert!(!standing.holds_live_focus());
    }

    #[test]
    fn retiring_a_link_drops_only_one_the_live_session_has_already_stranded() {
        let _session = Session::enter(7, 3, 3);
        let mut pod = pod();
        pod.set_focused(true);
        assert!(
            !pod.retire_stale_focus_link(),
            "a live link is never retired by a pass that merely walked past it"
        );
        assert!(pod.is_focused());

        set_live_focus_session(7, 4, 4);
        assert!(pod.retire_stale_focus_link(), "a stranded link is dropped");
        assert!(
            !pod.is_focused(),
            "so the raw flag and the stamp tell one story"
        );
        assert!(
            !pod.retire_stale_focus_link(),
            "and retiring is idempotent — there is nothing left to drop"
        );
    }

    #[test]
    fn a_pod_that_never_held_a_link_reports_nothing_to_retire() {
        let _session = Session::enter(7, 3, 3);
        let mut pod = pod();
        assert!(!pod.holds_live_focus());
        assert!(!pod.retire_stale_focus_link());
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

    /// The interval a [`SlowPacedAnimator`] asks for — a ~2Hz caret blink,
    /// deliberately far slower than any theme's cosmetic-loop cap.
    const SLOW_PACE: Duration = Duration::from_millis(500);

    /// A leaf widget whose decorative loop names its own (slow) cadence via
    /// [`PaintCtx::request_frame_paced_at`] — stands in for a blinking caret.
    struct SlowPacedAnimator;

    impl Widget for SlowPacedAnimator {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }

        fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
            ctx.request_frame_paced_at(SLOW_PACE);
        }
    }

    /// A scene recorder used to assert paint output without any GPU dependency.
    #[derive(Default)]
    struct RecordingScene {
        rects: Vec<(Point, Size)>,
        texts: Vec<(Point, String)>,
        shaders: Vec<(u64, Rect, f32)>,
        scene_textures: Vec<(u64, Rect)>,
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
        fn draw_scene_texture(&mut self, id: u64, dest: Rect) {
            self.scene_textures.push((id, dest));
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
    fn with_window_insets_scopes_and_restores_on_both_contexts() {
        use crate::insets::EdgeInsets;
        let root = WindowInsets::new(EdgeInsets::new(1.0, 2.0, 3.0, 4.0), EdgeInsets::ZERO);
        let scoped = root.consuming(true, true, true, true);

        let mut lctx = LayoutCtx::new();
        lctx.set_window_insets(root);
        let inside = lctx.with_window_insets(scoped, |ctx| ctx.window_insets());
        assert_eq!(inside, scoped);
        assert_eq!(lctx.window_insets(), root, "layout override restored");

        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        pctx.set_window_insets(root);
        let inside = pctx.with_window_insets(scoped, |ctx| ctx.window_insets());
        assert_eq!(inside, scoped);
        assert_eq!(pctx.window_insets(), root, "paint override restored");
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
    fn widget_paint_emits_scene_texture_into_scene() {
        /// A leaf widget that paints a scene texture.
        struct SceneTextureWidget {
            id: u64,
            dest: Rect,
        }

        impl Widget for SceneTextureWidget {
            fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
                bc.max()
            }

            fn paint(&mut self, _ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
                scene.draw_scene_texture(self.id, self.dest);
            }
        }

        let id = 42u64;
        let dest = Rect::new(10.0, 20.0, 100.0, 150.0);
        let mut w = SceneTextureWidget { id, dest };
        let mut scene = RecordingScene::default();
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(200.0, 200.0));
        w.paint(&mut ctx, &mut scene);
        assert_eq!(scene.scene_textures.len(), 1);
        assert_eq!(scene.scene_textures[0].0, id);
        assert_eq!(scene.scene_textures[0].1, dest);
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
    /// level (see `app/tests/platform_views.rs`).
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
                    command_index: 0,
                });
            }
        }
    }

    #[test]
    fn scene_builder_command_index_reports_the_live_count() {
        let mut scene = frust_scene::Scene::new();
        {
            let mut builder = frust_scene::SceneBuilder::new(&mut scene);
            let paint: &mut dyn PaintScene = &mut builder;
            assert_eq!(paint.command_index(), 0);

            paint.fill_rect(Point::ZERO, Size::new(4.0, 4.0), Color::BLACK);
            assert_eq!(paint.command_index(), 1);

            // Structural commands count too: the index is a position in the
            // scene's command list, not a tally of what draws.
            paint.push_clip(Point::ZERO, Size::new(4.0, 4.0));
            paint.pop_clip();
            assert_eq!(paint.command_index(), 3);

            // A transform push records nothing, so the index stays put.
            paint.push_transform(Affine::translate((1.0, 1.0)));
            assert_eq!(paint.command_index(), 3);
            paint.pop_transform();
        }
        assert_eq!(scene.commands().len(), 3);
    }

    #[test]
    fn default_command_index_is_zero() {
        // A recorder with no command list keeps the defaulted 0, however much
        // it is asked to paint.
        let mut discard = DiscardScene;
        discard.fill_rect(Point::ZERO, Size::new(4.0, 4.0), Color::BLACK);
        assert_eq!(discard.command_index(), 0);

        let mut recording = RecordingScene::default();
        recording.fill_rect(Point::ZERO, Size::new(4.0, 4.0), Color::BLACK);
        assert_eq!(recording.command_index(), 0);
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
    fn paint_ctx_origin_is_absolute_through_nested_offsets() {
        // Nest a leaf widget under two levels of offset containers: the leaf
        // must observe the summed absolute window-space origin, not a
        // parent-relative one. This pins the accumulation contract that external
        // design systems (anchored overlays) depend on.
        use std::cell::Cell;

        struct OriginRecorder {
            recorded_origin: Cell<Option<Point>>,
        }

        impl Widget for OriginRecorder {
            fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
                bc.max()
            }

            fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
                self.recorded_origin.set(Some(ctx.origin()));
            }
        }

        let mut lctx = LayoutCtx::new();

        // Inner recorder at (20, 30) relative to middle container.
        let recorder = OriginRecorder {
            recorded_origin: Cell::new(None),
        };
        let inner = ChildPod::new(Box::new(recorder));
        let mut middle = TranslatingWrapper { child: inner };
        middle.child.set_origin(Point::new(20.0, 30.0));
        middle
            .child
            .layout_child(&mut lctx, &BoxConstraints::tight(Size::new(10.0, 10.0)));

        // Middle container at (50, 70) relative to outer.
        let mut outer = ChildPod::new(Box::new(middle));
        outer.set_origin(Point::new(50.0, 70.0));
        outer.layout_child(&mut lctx, &BoxConstraints::tight(Size::new(10.0, 10.0)));

        // Paint from root at (0, 0).
        let mut scene = RecordingScene::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(400.0, 400.0));
        outer.paint_child(&mut pctx, &mut scene);

        // Verify: leaf's origin must be sum of all offsets:
        // root(0,0) + outer(50,70) + middle(20,30) = (70, 100).
        let leaf = outer
            .widget_mut()
            .downcast_mut::<TranslatingWrapper>()
            .unwrap()
            .child
            .widget_mut()
            .downcast_mut::<OriginRecorder>()
            .unwrap();
        assert_eq!(
            leaf.recorded_origin.get(),
            Some(Point::new(70.0, 100.0)),
            "leaf must observe summed absolute origin"
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
        // The bare form names no interval: `Duration::ZERO` is "at the theme's
        // own cosmetic rate" — unchanged behavior for all six shimmer widgets.
        assert_eq!(ctx.paced_interval(), Some(Duration::ZERO));
    }

    #[test]
    fn request_frame_paced_at_carries_its_interval() {
        // The explicit form is the same class, only slower: a ~500ms caret.
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        ctx.request_frame_paced_at(Duration::from_millis(500));
        assert!(ctx.needs_frame());
        assert_eq!(ctx.frame_class(), Some(TickClass::CosmeticLoop));
        assert!(ctx.needs_frame_paced_only());
        assert_eq!(ctx.paced_interval(), Some(Duration::from_millis(500)));
    }

    #[test]
    fn request_frame_paced_at_clamps_past_the_ten_second_ceiling() {
        // Past the ceiling: clamps DOWN to it rather than carrying the raw
        // caller value out to the shell's pacing arithmetic unbounded.
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        ctx.request_frame_paced_at(Duration::from_secs(10) + Duration::from_secs(1));
        assert_eq!(ctx.paced_interval(), Some(PaintCtx::MAX_PACED_INTERVAL));
    }

    #[test]
    fn request_frame_paced_at_leaves_a_real_cadence_untouched() {
        // Every shipped cadence (the ~500ms caret above, muxr's ~550ms blink)
        // sits nowhere near the ceiling and must pass through byte-for-byte —
        // the clamp must never change behavior for any existing caller.
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        ctx.request_frame_paced_at(Duration::from_millis(550));
        assert_eq!(ctx.paced_interval(), Some(Duration::from_millis(550)));
    }

    #[test]
    fn paced_intervals_aggregate_on_the_min_lattice() {
        // Two paced requests at different rates in one pass: the TIGHTEST wins,
        // so both are honored (the slower one is merely repainted more often
        // than it asked — see `request_frame_paced_at`'s contract).
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        ctx.request_frame_paced_at(Duration::from_millis(500));
        ctx.request_frame_paced_at(Duration::from_millis(33));
        assert_eq!(ctx.paced_interval(), Some(Duration::from_millis(33)));
        assert!(ctx.needs_frame_paced_only());

        // Order-independent (a lattice fold, not a last-writer-wins slot).
        let mut reversed = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        reversed.request_frame_paced_at(Duration::from_millis(33));
        reversed.request_frame_paced_at(Duration::from_millis(500));
        assert_eq!(reversed.paced_interval(), Some(Duration::from_millis(33)));

        // A bare `request_frame_paced` (the theme cap, `Duration::ZERO`) is the
        // absorbing element: a 30Hz shimmer beside a 2Hz caret paces at 30Hz.
        let mut shimmer_and_caret = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        shimmer_and_caret.request_frame_paced_at(Duration::from_millis(500));
        shimmer_and_caret.request_frame_paced();
        assert_eq!(shimmer_and_caret.paced_interval(), Some(Duration::ZERO));
    }

    #[test]
    fn request_frame_class_cosmetic_matches_the_bare_paced_request() {
        // The general form must stay interchangeable with `request_frame_paced`
        // (the widgets' `cull_pacing`/`pacing_integration` suites drive it).
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        ctx.request_frame_class(TickClass::CosmeticLoop);
        assert_eq!(ctx.frame_class(), Some(TickClass::CosmeticLoop));
        assert_eq!(ctx.paced_interval(), Some(Duration::ZERO));
    }

    #[test]
    fn no_paced_request_names_no_interval() {
        // Nothing requested, and a Transition-only request, both leave the
        // interval unset — there is no paced loop to pace.
        let ctx = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        assert_eq!(ctx.paced_interval(), None);
        let mut transition = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        transition.request_frame();
        assert_eq!(transition.paced_interval(), None);
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
    fn child_pod_bubbles_a_named_interval_unchanged() {
        // A slow caret nested under a container must reach the root with its own
        // interval intact — bubbling must never re-tighten it to the theme cap
        // (which a blanket `request_frame_class(CosmeticLoop)` forward would).
        let mut outer = ChildPod::new(Box::new(SingleChildContainer {
            child: ChildPod::new(Box::new(SlowPacedAnimator)),
        }));
        let mut lctx = LayoutCtx::new();
        outer.layout_child(&mut lctx, &BoxConstraints::tight(Size::new(10.0, 10.0)));
        let mut scene = RecordingScene::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        outer.paint_child(&mut pctx, &mut scene);
        assert!(pctx.needs_frame_paced_only());
        assert_eq!(
            pctx.paced_interval(),
            Some(SLOW_PACE),
            "a nested slow paced loop keeps its own cadence up the tree"
        );
    }

    #[test]
    fn absorb_paced_interval_skips_on_none_without_tightening_to_zero() {
        // The rule shared by both paced-interval absorb sites
        // (`ChildPod::paint_child`, `with_hero_registry`): a `None` interval
        // leaves the aggregate untouched rather than defaulting to
        // `Duration::ZERO`, the most-tightening value.
        //
        // Exercised directly against the fold: driving this state through
        // `paint_child` would trip its `debug_assert!` instead, since the
        // public `request_frame_paced*` API cannot produce it.
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        ctx.absorb_paced_interval(None);
        assert_eq!(
            ctx.paced_interval(),
            None,
            "a None interval must not tighten the aggregate to Duration::ZERO"
        );

        // A pre-existing aggregate is likewise untouched by a `None` fold.
        let mut ctx2 = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        ctx2.request_frame_paced_at(Duration::from_millis(500));
        ctx2.absorb_paced_interval(None);
        assert_eq!(
            ctx2.paced_interval(),
            Some(Duration::from_millis(500)),
            "a None fold must not override an already-merged interval either"
        );
    }

    #[test]
    fn sibling_paced_intervals_fold_to_the_tightest() {
        // A shimmer (theme cap) beside a slow caret under one container: the
        // container's aggregate paces at the shimmer's rate. The caret is then
        // repainted more often than it asked — no visual harm, by design.
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
            a: ChildPod::new(Box::new(SlowPacedAnimator)),
            b: ChildPod::new(Box::new(PacedAnimator)),
        }));
        let mut lctx = LayoutCtx::new();
        outer.layout_child(&mut lctx, &BoxConstraints::tight(Size::new(10.0, 10.0)));
        let mut scene = RecordingScene::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        outer.paint_child(&mut pctx, &mut scene);
        assert!(pctx.needs_frame_paced_only());
        assert_eq!(
            pctx.paced_interval(),
            Some(Duration::ZERO),
            "the tightest sibling request (the theme cap) wins the fold"
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

        // The interval half of the absorb: a named interval inside surfaces
        // outside, and folds on the MIN-lattice with an outer request.
        let mut ctx3 = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        ctx3.with_hero_registry(&registry, |child| {
            child.request_frame_paced_at(SLOW_PACE);
        });
        assert_eq!(ctx3.paced_interval(), Some(SLOW_PACE));
        ctx3.request_frame_paced();
        assert_eq!(
            ctx3.paced_interval(),
            Some(Duration::ZERO),
            "the outer theme-cap request tightens the folded aggregate"
        );
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
    fn child_pod_stamps_a_hover_claim_and_seeds_it_by_epoch() {
        use std::cell::Cell;
        use std::rc::Rc;

        /// A probe that claims hover on every event it receives and records what
        /// `is_hovered()` each pass reported.
        struct HoverProbe {
            event_seen: Rc<Cell<Option<bool>>>,
            paint_seen: Rc<Cell<Option<bool>>>,
        }
        impl Widget for HoverProbe {
            fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
                bc.constrain(Size::new(10.0, 10.0))
            }
            fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
                self.paint_seen.set(Some(ctx.is_hovered()));
            }
            fn event(&mut self, ctx: &mut EventCtx, _event: &InputEvent) -> EventResult {
                self.event_seen.set(Some(ctx.is_hovered()));
                ctx.claim_hover();
                EventResult::Ignored
            }
        }

        let event_seen = Rc::new(Cell::new(None));
        let paint_seen = Rc::new(Cell::new(None));
        let mut pod = ChildPod::new(Box::new(HoverProbe {
            event_seen: event_seen.clone(),
            paint_seen: paint_seen.clone(),
        }));
        let mut lctx = LayoutCtx::new();
        pod.layout_child(&mut lctx, &BoxConstraints::tight(Size::new(10.0, 10.0)));
        let mut scene = RecordingScene::default();
        let mut state = ();
        let event = InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Move,
            position: Point::new(5.0, 5.0),
            button: PointerButton::Primary,
        });

        // A never-claimed pod carries no stamp, and reads unhovered against any
        // live epoch (the root's starts past `0` for exactly this reason).
        assert_eq!(pod.hover_epoch(), 0);

        // Dispatch on an INELIGIBLE pass (the default): the claim records nothing.
        {
            let mut ctx = EventCtx::new(&mut state as &mut dyn Any, Point::ZERO, pod.size());
            ctx.set_hover_epoch(4);
            pod.event_child(&mut ctx, &event);
            assert!(!ctx.is_hover_claimed(), "nothing bubbled up");
        }
        assert_eq!(pod.hover_epoch(), 0, "no stamp from an ineligible pass");

        // Dispatch on an eligible pass: the pod is stamped with the *next* epoch
        // (the root advances its own when the pass ends) and the claim bubbles.
        {
            let mut ctx = EventCtx::new(&mut state as &mut dyn Any, Point::ZERO, pod.size());
            ctx.set_hover_epoch(4);
            ctx.set_hovered(true);
            ctx.set_hover_eligible(true);
            pod.event_child(&mut ctx, &event);
            assert!(ctx.is_hover_claimed());
            assert_eq!(
                event_seen.get(),
                Some(false),
                "the pod held no link going into this pass"
            );
        }
        assert_eq!(pod.hover_epoch(), 5);

        // Paint against the epoch the claim recorded: hovered, and the ancestor
        // chain is ANDed in exactly like focus.
        let mut live = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        live.set_hover_epoch(5);
        live.set_hovered(true);
        pod.paint_child(&mut live, &mut scene);
        assert_eq!(paint_seen.get(), Some(true));

        // Same stamp under an UNHOVERED ancestor: the chain wins.
        let mut unhovered_parent = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        unhovered_parent.set_hover_epoch(5);
        unhovered_parent.set_hovered(false);
        pod.paint_child(&mut unhovered_parent, &mut scene);
        assert_eq!(paint_seen.get(), Some(false));

        // The epoch moves on (the pointer went elsewhere): the stamp is stranded
        // with nobody clearing it — the whole point of stamping rather than
        // flagging.
        let mut later = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        later.set_hover_epoch(6);
        later.set_hovered(true);
        pod.paint_child(&mut later, &mut scene);
        assert_eq!(paint_seen.get(), Some(false));

        // A pod holding the capture path refuses a claim outright, whatever the
        // pass says — a drag never paints hover under the pointer.
        pod.set_active(true);
        {
            let mut ctx = EventCtx::new(&mut state as &mut dyn Any, Point::ZERO, pod.size());
            ctx.set_hover_epoch(6);
            ctx.set_hover_eligible(true);
            pod.event_child(&mut ctx, &event);
            assert!(!ctx.is_hover_claimed());
        }
        assert_eq!(pod.hover_epoch(), 5, "the captured pod kept its old stamp");
    }

    /// Two overlapping children can both hit-test a point (a Stack, or any
    /// container whose topmost child ignores the move). Only the first claim in
    /// **dispatch order** is recorded, which is the primitive; "the topmost
    /// claimant wins" is the consequence of dispatching topmost-first *and* of
    /// every container claiming after it routes (see `EventCtx::claim_hover`),
    /// not a rule this level enforces. Either way, no pass can leave two widgets
    /// hovered.
    #[test]
    fn only_the_first_hover_claim_in_a_pass_is_recorded() {
        struct Claimer;
        impl Widget for Claimer {
            fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
                bc.constrain(Size::new(10.0, 10.0))
            }
            fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
            fn event(&mut self, ctx: &mut EventCtx, _event: &InputEvent) -> EventResult {
                ctx.claim_hover();
                EventResult::Ignored
            }
        }

        let mut first = ChildPod::new(Box::new(Claimer));
        let mut second = ChildPod::new(Box::new(Claimer));
        let mut state = ();
        let event = InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Move,
            position: Point::new(5.0, 5.0),
            button: PointerButton::Primary,
        });
        let mut ctx = EventCtx::new(&mut state as &mut dyn Any, Point::ZERO, Size::ZERO);
        ctx.set_hover_epoch(2);
        ctx.set_hover_eligible(true);

        first.event_child(&mut ctx, &event);
        second.event_child(&mut ctx, &event);
        assert_eq!(first.hover_epoch(), 3, "the first claim is recorded");
        assert_eq!(
            second.hover_epoch(),
            0,
            "the pass was closed to further claims"
        );
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

    /// A scene recorder that overrides the shadow/layer `PaintScene` additions, to
    /// prove they reach an implementing scene that opts in.
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
        // `RecordingScene` (defined above) does not override the shadow/layer
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
        // `RecordingScene` does not override the path additions
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

    /// A scene recorder overriding the path additions, proving they
    /// reach an implementing scene that opts in.
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

    #[test]
    fn a_pods_type_name_resolves_through_every_box() {
        // A pod's widget is always erased, and the container plumbing stores it
        // double-boxed — the name must still be the widget's own, at any depth.
        let single = ChildPod::new(Box::new(Animator));
        assert!(
            single.type_name().ends_with("Animator"),
            "{}",
            single.type_name()
        );

        let boxed: Box<dyn Widget> = Box::new(Animator);
        let double = ChildPod::new(Box::new(boxed));
        assert!(
            double.type_name().ends_with("Animator"),
            "{}",
            double.type_name()
        );
    }

    #[test]
    fn a_pods_tooling_id_is_assigned_once_and_never_shared() {
        let a = ChildPod::new(Box::new(Animator));
        let b = ChildPod::new(Box::new(Animator));
        let first = a.inspect_id();
        assert_eq!(a.inspect_id(), first, "a pod keeps the id it was given");
        assert_ne!(b.inspect_id(), first, "two pods never share an id");
        assert!(
            first.0 >= ChildPod::INSPECT_ID_BASE,
            "pod ids stay in the range reserved against arena WidgetIds"
        );
    }

    #[test]
    fn a_leaf_visits_no_children_and_a_container_visits_all_of_its_own() {
        struct Two {
            children: Vec<ChildPod>,
        }
        impl Widget for Two {
            fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
                bc.max()
            }
            fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
            fn visit_children(&self, visitor: &mut dyn FnMut(&ChildPod)) {
                for child in &self.children {
                    visitor(child);
                }
            }
        }

        // The trait default: a widget that says nothing publishes nothing.
        let mut seen = 0;
        Animator.visit_children(&mut |_| seen += 1);
        assert_eq!(seen, 0);

        let container = Two {
            children: vec![
                ChildPod::new(Box::new(Animator)),
                ChildPod::new(Box::new(Animator)),
            ],
        };
        let mut ids = Vec::new();
        container.visit_children(&mut |child| ids.push(child.inspect_id()));
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1]);
    }

    /// A recorder that implements only the *uniform* rounded methods and the
    /// solid stroke — i.e. every pre-per-corner recorder scene in the wild —
    /// so the per-corner/dashed trait defaults can be observed falling back.
    #[derive(Default)]
    struct UniformOnlyScene {
        fills: Vec<(Point, Size, f64)>,
        clips: Vec<(Point, Size, f64)>,
        strokes: Vec<(Point, f64)>,
        pops: usize,
    }

    impl PaintScene for UniformOnlyScene {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn fill_rounded_rect(&mut self, origin: Point, size: Size, radius: f64, _color: Color) {
            self.fills.push((origin, size, radius));
        }
        fn push_clip_rounded(&mut self, origin: Point, size: Size, radius: f64) {
            self.clips.push((origin, size, radius));
        }
        fn pop_clip(&mut self) {
            self.pops += 1;
        }
        fn stroke_path(&mut self, origin: Point, _path: &BezPath, width: f64, _brush: &Brush) {
            self.strokes.push((origin, width));
        }
    }

    fn corner_probe_path() -> BezPath {
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((10.0, 0.0));
        path
    }

    #[test]
    fn per_corner_paint_defaults_fall_back_to_the_uniform_methods() {
        // A scene that predates per-corner radii still paints something, at the
        // largest corner — and its clip stack stays balanced, which is why the
        // push delegates rather than no-op'ing against a real `pop_clip`.
        let mut scene = UniformOnlyScene::default();
        let origin = Point::new(2.0, 3.0);
        let size = Size::new(20.0, 10.0);
        let radii = CornerRadii::new(4.0, 12.0, 0.0, 1.0);

        scene.fill_rounded_rect_radii(origin, size, radii, Color::BLACK);
        scene.push_clip_rounded_radii(origin, size, radii);
        scene.pop_clip();

        assert_eq!(scene.fills, vec![(origin, size, 12.0)]);
        assert_eq!(scene.clips, vec![(origin, size, 12.0)]);
        assert_eq!(scene.pops, 1);
    }

    /// A recorder that predates path clips: it knows only the rectangular
    /// push and the shared pop.
    #[derive(Default)]
    struct RectClipOnlyScene {
        clips: Vec<(Point, Size)>,
        pops: usize,
    }

    impl PaintScene for RectClipOnlyScene {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn push_clip(&mut self, origin: Point, size: Size) {
            self.clips.push((origin, size));
        }
        fn pop_clip(&mut self) {
            self.pops += 1;
        }
    }

    fn triangle_probe_path() -> BezPath {
        let mut path = BezPath::new();
        path.move_to((4.0, 2.0));
        path.line_to((14.0, 2.0));
        path.line_to((9.0, 12.0));
        path.close_path();
        path
    }

    #[test]
    fn path_clip_default_pushes_the_translated_bounding_box() {
        // A scene with no path-clip concept still clips — to the path's
        // bounding box, offset by `origin` — and its clip stack stays balanced
        // against the shared `pop_clip`.
        let mut scene = RectClipOnlyScene::default();
        scene.push_clip_path(Point::new(10.0, 20.0), &triangle_probe_path());
        scene.pop_clip();

        assert_eq!(
            scene.clips,
            vec![(Point::new(14.0, 22.0), Size::new(10.0, 10.0))]
        );
        assert_eq!(scene.pops, 1);
    }

    #[test]
    fn scene_builder_records_a_path_clip_translated_by_origin() {
        let mut scene = frust_scene::Scene::new();
        {
            let mut builder = frust_scene::SceneBuilder::new(&mut scene);
            let paint: &mut dyn PaintScene = &mut builder;
            paint.push_clip_path(Point::new(10.0, 20.0), &triangle_probe_path());
            paint.pop_clip();
        }

        match &scene.commands()[0] {
            frust_scene::Command::PushClipPath { path, transform } => {
                let expected = Affine::translate((10.0, 20.0)) * triangle_probe_path();
                assert_eq!(*path, expected);
                assert_eq!(*transform, Affine::IDENTITY);
            }
            other => panic!("expected PushClipPath, got {other:?}"),
        }
        assert!(matches!(scene.commands()[1], frust_scene::Command::PopClip));
    }

    #[test]
    fn dashed_stroke_default_falls_back_to_the_solid_stroke() {
        let mut scene = UniformOnlyScene::default();
        let origin = Point::new(5.0, 6.0);
        scene.stroke_path_dashed(
            origin,
            &corner_probe_path(),
            2.0,
            DashPattern::new(4.0, 2.0),
            &Brush::Solid(Color::BLACK),
        );

        assert_eq!(scene.strokes, vec![(origin, 2.0)]);
    }

    #[test]
    fn scene_builder_records_per_corner_radii_translated_by_origin() {
        // The `SceneBuilder` impl overrides the defaults above: every corner
        // reaches the command intact, under the same origin→rect convention the
        // uniform methods use.
        let mut scene = frust_scene::Scene::new();
        let radii = CornerRadii::new(4.0, 12.0, 0.0, 1.0);
        {
            let mut builder = frust_scene::SceneBuilder::new(&mut scene);
            let paint: &mut dyn PaintScene = &mut builder;
            paint.fill_rounded_rect_radii(
                Point::new(2.0, 3.0),
                Size::new(20.0, 10.0),
                radii,
                Color::BLACK,
            );
            paint.push_clip_rounded_radii(Point::new(0.0, 0.0), Size::new(5.0, 5.0), radii);
            paint.pop_clip();
        }

        match &scene.commands()[0] {
            frust_scene::Command::RoundedRect {
                rect, radii: got, ..
            } => {
                assert_eq!(*rect, Rect::new(2.0, 3.0, 22.0, 13.0));
                assert_eq!(*got, radii);
            }
            other => panic!("expected RoundedRect, got {other:?}"),
        }
        match &scene.commands()[1] {
            frust_scene::Command::PushClipRounded { radii: got, .. } => assert_eq!(*got, radii),
            other => panic!("expected PushClipRounded, got {other:?}"),
        }
        assert!(matches!(scene.commands()[2], frust_scene::Command::PopClip));
    }

    #[test]
    fn scene_builder_records_a_dashed_stroke_translated_by_origin() {
        let mut scene = frust_scene::Scene::new();
        let dash = DashPattern::new(4.0, 2.0).with_phase(1.0);
        {
            let mut builder = frust_scene::SceneBuilder::new(&mut scene);
            let paint: &mut dyn PaintScene = &mut builder;
            paint.stroke_path_dashed(
                Point::new(5.0, 0.0),
                &corner_probe_path(),
                2.0,
                dash,
                &Brush::Solid(Color::BLACK),
            );
        }

        match &scene.commands()[0] {
            frust_scene::Command::Path { path, style, .. } => {
                assert_eq!(
                    *style,
                    frust_scene::PathStyle::Stroke {
                        width: 2.0,
                        dash: Some(dash)
                    }
                );
                // Same origin translation `stroke_path` applies.
                assert_eq!(path.elements().len(), 2);
                assert!(matches!(
                    path.elements()[0],
                    kurbo::PathEl::MoveTo(p) if p == Point::new(5.0, 0.0)
                ));
            }
            other => panic!("expected Path, got {other:?}"),
        }
    }

    #[test]
    fn uniform_paint_calls_still_encode_the_commands_they_always_did() {
        // The additive contract: the uniform methods every existing call site
        // uses keep producing the same commands, now spelled as four equal
        // corners / an undashed stroke.
        let mut scene = frust_scene::Scene::new();
        {
            let mut builder = frust_scene::SceneBuilder::new(&mut scene);
            let paint: &mut dyn PaintScene = &mut builder;
            paint.fill_rounded_rect(
                Point::new(0.0, 0.0),
                Size::new(10.0, 10.0),
                4.0,
                Color::BLACK,
            );
            paint.push_clip_rounded(Point::new(0.0, 0.0), Size::new(10.0, 10.0), 4.0);
            paint.pop_clip();
            paint.draw_shadow(
                Point::new(0.0, 0.0),
                Size::new(10.0, 10.0),
                4.0,
                2.0,
                Color::BLACK,
            );
            paint.stroke_path(
                Point::new(0.0, 0.0),
                &corner_probe_path(),
                1.0,
                &Brush::Solid(Color::BLACK),
            );
        }

        let uniform = CornerRadii::uniform(4.0);
        match &scene.commands()[0] {
            frust_scene::Command::RoundedRect { radii, .. } => assert_eq!(*radii, uniform),
            other => panic!("expected RoundedRect, got {other:?}"),
        }
        match &scene.commands()[1] {
            frust_scene::Command::PushClipRounded { radii, .. } => assert_eq!(*radii, uniform),
            other => panic!("expected PushClipRounded, got {other:?}"),
        }
        match &scene.commands()[3] {
            frust_scene::Command::BlurredRoundedRect { radii, .. } => assert_eq!(*radii, uniform),
            other => panic!("expected BlurredRoundedRect, got {other:?}"),
        }
        match &scene.commands()[4] {
            frust_scene::Command::Path { style, .. } => assert_eq!(
                *style,
                frust_scene::PathStyle::Stroke {
                    width: 1.0,
                    dash: None
                }
            ),
            other => panic!("expected Path, got {other:?}"),
        }
    }

    #[test]
    fn every_paint_scene_method_terminates_on_discard_scene() {
        // Every `PaintScene` method — including the delegating defaults
        // (per-corner/dashed variants, the additive image/shader/shadow
        // surface) — is reachable and terminates on `DiscardScene` without
        // panicking; a future `todo!()` or an infinitely-recursive default
        // would fail here. The records-nothing guarantee itself is asserted
        // where a scene actually exists, at the navigator/switcher call
        // sites, not here.
        use frust_scene::{FontHandle, Glyph};

        let mut scene = DiscardScene;
        let origin = Point::new(2.0, 3.0);
        let size = Size::new(20.0, 10.0);
        let color = Color::BLACK;
        let brush = Brush::Solid(color);
        let radii = CornerRadii::new(4.0, 12.0, 0.0, 1.0);
        let path = corner_probe_path();

        scene.fill_rect(origin, size, color);
        scene.fill_rounded_rect(origin, size, 4.0, color);
        scene.fill_rounded_rect_radii(origin, size, radii, color);
        scene.stroke_line(origin, Point::new(10.0, 10.0), 1.0, color);
        scene.push_clip(origin, size);
        scene.push_clip_rounded(origin, size, 4.0);
        scene.push_clip_rounded_radii(origin, size, radii);
        scene.pop_clip();
        scene.draw_text(origin, "discarded");

        let font = FontHandle::new(peniko::FontData::new(
            peniko::Blob::from(Vec::<u8>::new()),
            0,
        ));
        let run = GlyphRun {
            font,
            font_size: 16.0,
            brush: brush.clone(),
            transform: Affine::IDENTITY,
            glyphs: vec![Glyph {
                id: 1,
                x: 0.0,
                y: 0.0,
            }],
        };
        scene.draw_glyph_run(run);

        let image = peniko::ImageData {
            data: peniko::Blob::from(vec![0u8; 2 * 2 * 4]),
            format: peniko::ImageFormat::Rgba8,
            alpha_type: peniko::ImageAlphaType::Alpha,
            width: 2,
            height: 2,
        };
        scene.draw_image(&image, Rect::new(0.0, 0.0, 20.0, 20.0));

        let program = ShaderProgram::new("fn main() {}");
        scene.draw_shader(&program, Rect::new(0.0, 0.0, 10.0, 10.0), 1.5);

        scene.draw_shadow(origin, size, 4.0, 2.0, color);
        scene.fill_rect_brush(origin, size, &brush);
        scene.fill_rounded_rect_brush(origin, size, 4.0, &brush);
        scene.push_layer(origin, size, 0.5);
        scene.pop_layer();
        scene.clear_rect(origin, size);
        scene.fill_path(origin, &path, &brush);
        scene.stroke_path(origin, &path, 1.0, &brush);
        scene.stroke_path_dashed(origin, &path, 1.0, DashPattern::new(4.0, 2.0), &brush);
        scene.push_transform(Affine::translate((1.0, 2.0)));
        scene.pop_transform();
    }

    /// A minimal recorder that tracks only transform and layer stack operations
    /// to verify the default [`PaintScene::push_retained`]/[`PaintScene::pop_retained`]
    /// implementation emulates the presentation correctly.
    #[derive(Default)]
    struct MinimalRetainedRecorder {
        operations: Vec<String>,
    }

    impl PaintScene for MinimalRetainedRecorder {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn push_transform(&mut self, _transform: Affine) {
            self.operations.push("push_transform".to_string());
        }
        fn pop_transform(&mut self) {
            self.operations.push("pop_transform".to_string());
        }
        fn push_layer(&mut self, _origin: Point, _size: Size, _alpha: f32) {
            self.operations.push("push_layer".to_string());
        }
        fn pop_layer(&mut self) {
            self.operations.push("pop_layer".to_string());
        }
    }

    #[test]
    fn default_push_retained_pop_retained_emulates_presentation_for_recorders() {
        // The default `push_retained` and `pop_retained` implementation
        // ensures recorders that override only the transform/layer methods
        // see a consistent sequence: push_transform, push_layer, paint body,
        // pop_layer, pop_transform — the reverse order matching the push ops.
        let mut recorder = MinimalRetainedRecorder::default();
        let origin = Point::new(10.0, 20.0);
        let size = Size::new(100.0, 50.0);
        let alpha = 0.8;
        let scale = 0.9;

        // Emulate a body that doesn't call any paint methods (just has side effects).
        recorder.push_retained(1, origin, size, alpha, scale);
        // Body paint code would go here
        recorder.pop_retained();

        // Verify the sequence: transform, layer, pop_layer, pop_transform.
        assert_eq!(
            recorder.operations,
            vec![
                "push_transform".to_string(),
                "push_layer".to_string(),
                "pop_layer".to_string(),
                "pop_transform".to_string(),
            ]
        );
    }

    #[test]
    fn scene_builder_records_push_pop_retained_commands_directly_without_extra_transforms() {
        // The `SceneBuilder` impl overrides `push_retained`/`pop_retained` to
        // record the retained commands directly, without the emulating
        // transform/layer pairs. It records Command::PushRetained with the
        // key, rect (converted from origin/size), alpha, scale, and the current
        // transform, and Command::PopRetained with no extra layer/transform commands.
        let mut scene = frust_scene::Scene::new();
        {
            let mut builder = frust_scene::SceneBuilder::new(&mut scene);
            let paint: &mut dyn PaintScene = &mut builder;

            paint.push_retained(7, Point::new(5.0, 10.0), Size::new(80.0, 40.0), 0.5, 1.2);
            // Paint some content inside the retained bracket.
            paint.fill_rect(Point::new(10.0, 15.0), Size::new(20.0, 25.0), Color::BLACK);
            paint.pop_retained();
        }

        // Verify the command sequence: PushRetained, FillRect, PopRetained.
        // No PushLayer, PopLayer, PushTransform, or PopTransform commands.
        let commands = scene.commands();
        assert_eq!(commands.len(), 3);

        match &commands[0] {
            frust_scene::Command::PushRetained {
                key,
                rect,
                alpha,
                scale,
                ..
            } => {
                assert_eq!(*key, 7);
                assert_eq!(*rect, Rect::new(5.0, 10.0, 85.0, 50.0));
                assert_eq!(*alpha, 0.5);
                assert_eq!(*scale, 1.2);
            }
            other => panic!("expected PushRetained, got {other:?}"),
        }

        match &commands[1] {
            frust_scene::Command::FillRect { rect, .. } => {
                assert_eq!(*rect, Rect::new(10.0, 15.0, 30.0, 40.0));
            }
            other => panic!("expected FillRect, got {other:?}"),
        }

        assert!(
            matches!(commands[2], frust_scene::Command::PopRetained),
            "expected PopRetained, got {:?}",
            commands[2]
        );
    }

    #[test]
    fn current_transform_is_the_scene_builders_own_and_none_for_a_recorder() {
        assert_eq!(
            MinimalRetainedRecorder::default().current_transform(),
            None,
            "a recorder tracks no transform"
        );
        let mut scene = frust_scene::Scene::new();
        let mut builder = frust_scene::SceneBuilder::new(&mut scene);
        let paint: &mut dyn PaintScene = &mut builder;
        assert_eq!(paint.current_transform(), Some(Affine::IDENTITY));
        paint.push_transform(Affine::scale(2.0));
        paint.push_transform(Affine::translate((3.0, 4.0)));
        assert_eq!(
            paint.current_transform(),
            Some(Affine::scale(2.0) * Affine::translate((3.0, 4.0))),
            "every open transform, composed"
        );
    }

    #[test]
    fn backdrop_blur_is_a_no_op_by_default_and_recorded_by_the_scene_builder() {
        // The default body: a recorder scene that never heard of a backdrop
        // takes the call and records nothing.
        let mut recorder = MinimalRetainedRecorder::default();
        recorder.backdrop_blur(Point::new(1.0, 2.0), Size::new(3.0, 4.0), 5.0);

        let mut scene = frust_scene::Scene::new();
        {
            let mut builder = frust_scene::SceneBuilder::new(&mut scene);
            let paint: &mut dyn PaintScene = &mut builder;
            paint.push_transform(Affine::translate((10.0, 20.0)));
            paint.backdrop_blur(Point::new(1.0, 2.0), Size::new(3.0, 4.0), 5.0);
        }
        match scene.commands() {
            [
                frust_scene::Command::BackdropBlur {
                    rect,
                    radius,
                    transform,
                },
            ] => {
                assert_eq!(*rect, Rect::new(1.0, 2.0, 4.0, 6.0));
                assert_eq!(*radius, 5.0);
                assert_eq!(*transform, Affine::translate((10.0, 20.0)));
            }
            other => panic!("expected one BackdropBlur, got {other:?}"),
        }
    }

    #[test]
    fn retained_enabled_needs_the_switch_and_a_threaded_ledger() {
        let ledger = crate::retained::RetainedLedger::empty();
        assert!(
            !PaintCtx::new(Point::ZERO, Size::ZERO).retained_enabled(),
            "no ledger threaded"
        );
        let mut ctx = PaintCtx::new(Point::ZERO, Size::ZERO).with_retained_ledger(&ledger);
        assert!(
            ctx.retained_enabled(),
            "an empty ledger still enables brackets"
        );
        ctx.set_paint_state(0, false, Some(&ledger), 0);
        assert!(!ctx.retained_enabled(), "the kill switch disables them");
    }

    /// Publishes on every paint channel a boundary must not skip.
    struct Publisher;

    impl Widget for Publisher {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }
        fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
            ctx.report_input_shield(Rect::new(0.0, 0.0, 1.0, 1.0));
            ctx.report_hero("tag", Rect::new(0.0, 0.0, 1.0, 1.0));
        }
    }

    #[test]
    fn paint_publications_sum_up_through_paint_child_and_hero_registries() {
        let mut pod = ChildPod::new(Box::new(Publisher));
        pod.layout_child(
            &mut LayoutCtx::new(),
            &BoxConstraints::tight(Size::new(5.0, 5.0)),
        );
        let mut quiet = ChildPod::new(Box::new(FixedBox {
            intrinsic: Size::new(5.0, 5.0),
        }));
        quiet.layout_child(
            &mut LayoutCtx::new(),
            &BoxConstraints::tight(Size::new(5.0, 5.0)),
        );

        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(10.0, 10.0));
        quiet.paint_child(&mut ctx, &mut DiscardScene);
        assert_eq!(
            ctx.paint_publications(),
            0,
            "a plain body publishes nothing"
        );
        pod.paint_child(&mut ctx, &mut DiscardScene);
        assert_eq!(ctx.paint_publications(), 2, "a shield and a hero report");

        let registry = RefCell::new(HeroFrames::new(Point::ZERO, HashMap::new()));
        ctx.with_hero_registry(&registry, |inner| {
            pod.paint_child(inner, &mut DiscardScene);
        });
        assert_eq!(
            ctx.paint_publications(),
            4,
            "a hero registry's subtree counts too"
        );
    }
}

#[cfg(test)]
mod transform_tests {
    use std::f64::consts::FRAC_PI_4;

    use kurbo::Vec2;

    use super::*;
    use crate::event::{
        PointerButton, PointerEvent, PointerId, PointerPhase, ScaleEvent, ScalePhase, ScrollDelta,
    };

    /// One recorded paint operation, in order.
    #[derive(Clone, Debug, PartialEq)]
    enum Op {
        Rect(Point, Size),
        Push(Affine),
        Pop,
    }

    /// A scene recorder that keeps the transform stack interleaved with draws,
    /// so a test can see exactly what a pod pushed around its child.
    #[derive(Default)]
    struct OpLog(Vec<Op>);

    impl PaintScene for OpLog {
        fn fill_rect(&mut self, origin: Point, size: Size, _color: Color) {
            self.0.push(Op::Rect(origin, size));
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn push_transform(&mut self, transform: Affine) {
            self.0.push(Op::Push(transform));
        }
        fn pop_transform(&mut self) {
            self.0.push(Op::Pop);
        }
    }

    /// A leaf that fills its whole box, records every event it receives (in
    /// local space), captures on `Down`, and contributes one semantics node.
    #[derive(Default)]
    struct Recorder {
        events: Vec<InputEvent>,
    }

    impl Widget for Recorder {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            scene.fill_rect(ctx.origin(), ctx.size(), Color::BLACK);
        }
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            self.events.push(event.clone());
            if matches!(
                event,
                InputEvent::Pointer(PointerEvent {
                    phase: PointerPhase::Down,
                    ..
                })
            ) {
                ctx.capture_pointer();
            }
            EventResult::Handled
        }
        fn semantics(&self, ctx: &mut SemanticsCtx) {
            ctx.push_node(accesskit::Role::Label, |_| {});
        }
    }

    /// A leaf that records the visible rect its paint context carries.
    struct VisibleProbe(std::rc::Rc<Cell<Option<Rect>>>);

    impl Widget for VisibleProbe {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }
        fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
            self.0.set(ctx.visible_rect());
        }
    }

    fn pointer(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    /// A laid-out `Recorder` pod of `size` at `origin`.
    fn recorder_pod(origin: Point, size: Size) -> ChildPod {
        let mut pod = ChildPod::new(Box::new(Recorder::default()));
        pod.layout_child(&mut LayoutCtx::new(), &BoxConstraints::tight(size));
        pod.set_origin(origin);
        pod
    }

    fn recorded(pod: &mut ChildPod) -> Vec<InputEvent> {
        pod.widget_mut()
            .downcast_mut::<Recorder>()
            .expect("recorder pod")
            .events
            .clone()
    }

    fn paint(pod: &mut ChildPod, visible: Option<Rect>) -> (Vec<Op>, Option<Rect>) {
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(400.0, 400.0));
        ctx.set_visible_rect(visible);
        let mut scene = OpLog::default();
        pod.paint_child(&mut ctx, &mut scene);
        (scene.0, ctx.visible_rect_ref())
    }

    fn dispatch(pod: &mut ChildPod, event: &InputEvent) -> EventResult {
        let mut state = 0u32;
        let mut ctx = EventCtx::new(&mut state, Point::ZERO, Size::new(400.0, 400.0));
        pod.event_child(&mut ctx, event)
    }

    #[test]
    fn a_child_under_scale_and_translate_receives_down_at_its_local_point() {
        let mut pod = recorder_pod(Point::new(10.0, 20.0), Size::new(50.0, 50.0));
        // Local p lands at origin + translate(5, 5) * scale(2) * p.
        pod.set_transform(Some(
            Affine::translate(Vec2::new(5.0, 5.0)) * Affine::scale(2.0),
        ));
        // Local (7, 9) is drawn at (10 + 5 + 14, 20 + 5 + 18) = (29, 43).
        let at = Point::new(29.0, 43.0);
        assert!(pod.contains(at));
        assert_eq!(
            dispatch(&mut pod, &pointer(PointerPhase::Down, at.x, at.y)),
            EventResult::Handled
        );
        assert!(pod.is_active(), "capture bookkeeping is unchanged");
        // Scroll positions and scale focal points take the same mapping; their
        // magnitudes do not.
        dispatch(
            &mut pod,
            &InputEvent::Scroll {
                position: at,
                delta: ScrollDelta::Pixels(0.0, 12.0),
            },
        );
        dispatch(
            &mut pod,
            &InputEvent::Scale(ScaleEvent {
                phase: ScalePhase::Update,
                scale_delta: 1.1,
                focal: at,
                velocity: 0.0,
            }),
        );
        let local = Point::new(7.0, 9.0);
        assert_eq!(
            recorded(&mut pod),
            vec![
                pointer(PointerPhase::Down, local.x, local.y),
                InputEvent::Scroll {
                    position: local,
                    delta: ScrollDelta::Pixels(0.0, 12.0),
                },
                InputEvent::Scale(ScaleEvent {
                    phase: ScalePhase::Update,
                    scale_delta: 1.1,
                    focal: local,
                    velocity: 0.0,
                }),
            ]
        );
        // The child now covers (15..115, 25..125): the untransformed box's
        // corner misses, and a point past its untransformed extent hits.
        assert!(!pod.contains(Point::new(12.0, 22.0)));
        assert!(pod.contains(Point::new(100.0, 100.0)));
        assert!(!pod.contains(Point::new(116.0, 60.0)));
        // A contact's inner event is mapped the same way.
        let contact = InputEvent::PointerContact {
            pointer_id: PointerId::touch(1),
            event: PointerEvent {
                phase: PointerPhase::Move,
                position: at,
                button: PointerButton::Primary,
            },
        };
        dispatch(&mut pod, &contact);
        assert_eq!(
            recorded(&mut pod).last(),
            Some(&InputEvent::PointerContact {
                pointer_id: PointerId::touch(1),
                event: PointerEvent {
                    phase: PointerPhase::Move,
                    position: local,
                    button: PointerButton::Primary,
                },
            })
        );
    }

    #[test]
    fn contains_agrees_with_paint_for_rotated_content_at_the_corners() {
        let size = Size::new(100.0, 100.0);
        let mut pod = recorder_pod(Point::new(40.0, 40.0), size);
        pod.set_transform(Some(Affine::rotate_about(
            FRAC_PI_4,
            Point::new(50.0, 50.0),
        )));
        let (ops, _) = paint(&mut pod, None);
        let [Op::Push(drawn), Op::Rect(rect_origin, rect_size), Op::Pop] = ops.as_slice() else {
            panic!("expected push/rect/pop, got {ops:?}");
        };
        assert_eq!((*rect_origin, *rect_size), (Point::new(40.0, 40.0), size));
        let rect = Rect::from_origin_size(*rect_origin, *rect_size);
        let center = *drawn * rect.center();
        for corner in [
            Point::new(rect.x0, rect.y0),
            Point::new(rect.x1, rect.y0),
            Point::new(rect.x1, rect.y1),
            Point::new(rect.x0, rect.y1),
        ] {
            // Where paint actually put this corner (the parent paints at the
            // absolute origin zero, so absolute space is the container's space).
            let painted = *drawn * corner;
            let inward = (center - painted).normalize();
            assert!(
                pod.contains(painted + inward),
                "just inside painted corner {painted:?}"
            );
            assert!(
                !pod.contains(painted - inward),
                "just outside painted corner {painted:?}"
            );
            // The untransformed box's own corner is outside the painted
            // diamond: the hit test is not an AABB test.
            let unrotated = corner + (rect.center() - corner) * 0.02;
            assert!(!pod.contains(unrotated), "AABB corner {unrotated:?}");
        }
    }

    #[test]
    fn an_untransformed_pod_paints_and_routes_exactly_as_before() {
        let origin = Point::new(12.0, 34.0);
        let size = Size::new(60.0, 40.0);
        let events = [
            pointer(PointerPhase::Down, 20.0, 40.0),
            pointer(PointerPhase::Move, 90.0, 10.0),
            pointer(PointerPhase::Up, 30.0, 50.0),
            InputEvent::Scroll {
                position: Point::new(25.0, 45.0),
                delta: ScrollDelta::Lines(0.0, 1.0),
            },
            InputEvent::Scale(ScaleEvent {
                phase: ScalePhase::Begin,
                scale_delta: 1.0,
                focal: Point::new(15.0, 35.0),
                velocity: 0.0,
            }),
            InputEvent::Housekeeping,
        ];
        let visible = Some(Rect::new(0.0, 0.0, 50.0, 50.0));

        // Baseline: a pod that never had a transform.
        let mut baseline = recorder_pod(origin, size);
        let baseline_paint = paint(&mut baseline, visible);
        for event in &events {
            dispatch(&mut baseline, event);
        }
        let baseline_events = recorded(&mut baseline);
        // The baseline is the pre-transform contract: no transform pushed, the
        // child drawn at its origin, the visible rect threaded unchanged, and
        // every event translated by `-origin` and nothing else.
        assert_eq!(baseline_paint.0, vec![Op::Rect(origin, size)]);
        assert_eq!(baseline_paint.1, visible);
        let translated: Vec<_> = events
            .iter()
            .map(|e| e.translated(-origin.to_vec2()))
            .collect();
        assert_eq!(baseline_events, translated);

        // A pod whose transform was set and then cleared is indistinguishable.
        let mut cleared = recorder_pod(origin, size);
        cleared.set_transform(Some(Affine::rotate(1.0) * Affine::scale(3.0)));
        cleared.set_transform(None);
        assert_eq!(cleared.transform(), None);
        assert_eq!(paint(&mut cleared, visible), baseline_paint);
        for event in &events {
            dispatch(&mut cleared, event);
        }
        assert_eq!(recorded(&mut cleared), baseline_events);
        for point in [
            Point::new(12.0, 34.0),
            Point::new(71.9, 73.9),
            Point::new(72.0, 50.0),
            Point::new(11.9, 50.0),
        ] {
            assert_eq!(cleared.contains(point), baseline.contains(point));
        }
    }

    #[test]
    fn a_singular_transform_hits_nothing_and_never_panics() {
        let origin = Point::new(10.0, 10.0);
        for singular in [
            Affine::scale(0.0),
            Affine::scale_non_uniform(2.0, 0.0),
            Affine::new([1.0, 2.0, 2.0, 4.0, 0.0, 0.0]),
            Affine::new([f64::NAN, 0.0, 0.0, 1.0, 0.0, 0.0]),
        ] {
            let mut pod = recorder_pod(origin, Size::new(50.0, 50.0));
            pod.set_transform(Some(singular));
            for point in [origin, Point::new(20.0, 20.0), Point::new(10.0, 30.0)] {
                assert!(!pod.contains(point), "{singular:?} hit {point:?}");
            }
            // Paint skips the collapsed subtree entirely.
            assert_eq!(
                paint(&mut pod, Some(Rect::new(0.0, 0.0, 99.0, 99.0))).0,
                vec![]
            );
            // An event reaching it through a recorded path still arrives,
            // translated as if untransformed, so a capture can end.
            dispatch(&mut pod, &pointer(PointerPhase::Up, 20.0, 25.0));
            assert_eq!(
                recorded(&mut pod),
                vec![pointer(PointerPhase::Up, 10.0, 15.0)]
            );
            // Semantics never reports NaN bounds.
            let mut sem = SemanticsCtx::new(Size::new(200.0, 200.0), 2);
            pod.semantics_child(&mut sem);
            let update = sem.finish(accesskit::NodeId(1));
            let bounds = update.nodes[0].1.bounds().expect("bounds");
            assert!(
                [bounds.x0, bounds.y0, bounds.x1, bounds.y1]
                    .iter()
                    .all(|v| v.is_finite()),
                "{singular:?} gave {bounds:?}"
            );
        }
    }

    #[test]
    fn a_transformed_pod_maps_the_visible_rect_and_reports_its_bounding_box() {
        let mut pod = recorder_pod(Point::new(10.0, 20.0), Size::new(50.0, 50.0));
        pod.set_transform(Some(Affine::scale(2.0)));
        // Paint: the transform is conjugated by the absolute origin, and the
        // visible rect reaches the child as its preimage. The parent's own
        // visible rect is untouched.
        let visible = Some(Rect::new(10.0, 20.0, 110.0, 120.0));
        let (ops, parent_visible) = paint(&mut pod, visible);
        assert_eq!(parent_visible, visible);
        assert_eq!(
            ops,
            vec![
                Op::Push(
                    Affine::translate(Vec2::new(10.0, 20.0))
                        * Affine::scale(2.0)
                        * Affine::translate(Vec2::new(-10.0, -20.0))
                ),
                Op::Rect(Point::new(10.0, 20.0), Size::new(50.0, 50.0)),
                Op::Pop,
            ]
        );
        // What the child itself culls against: the visible rect mapped back
        // through the inverse — (10, 20) .. (60, 70) in its own absolute frame.
        let seen = std::rc::Rc::new(Cell::new(None));
        let mut probe = ChildPod::new(Box::new(VisibleProbe(seen.clone())));
        probe.layout_child(
            &mut LayoutCtx::new(),
            &BoxConstraints::tight(Size::new(50.0, 50.0)),
        );
        probe.set_origin(Point::new(10.0, 20.0));
        probe.set_transform(Some(Affine::scale(2.0)));
        paint(&mut probe, visible);
        assert_eq!(seen.get(), Some(Rect::new(10.0, 20.0, 60.0, 70.0)));
        // Semantics: the transformed box, (10, 20) .. (110, 120).
        let mut sem = SemanticsCtx::new(Size::new(400.0, 400.0), 2);
        pod.semantics_child(&mut sem);
        let update = sem.finish(accesskit::NodeId(1));
        let bounds = update.nodes[0].1.bounds().expect("bounds");
        assert_eq!(
            (bounds.x0, bounds.y0, bounds.x1, bounds.y1),
            (10.0, 20.0, 110.0, 120.0)
        );
    }
}

#[cfg(test)]
mod contact_release_tests {
    use super::*;
    use crate::event::{ContactPass, PointerButton, PointerEvent, PointerId, PointerPhase};

    /// A leaf that captures on `Down`, opting into the gesture's other contacts
    /// when `opt_in` is set.
    struct Grab {
        opt_in: bool,
    }
    impl Widget for Grab {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(20.0, 20.0))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            if let InputEvent::Pointer(p) = event
                && p.phase == PointerPhase::Down
            {
                ctx.capture_pointer();
                if self.opt_in {
                    ctx.capture_contacts();
                }
            }
            EventResult::Handled
        }
    }

    /// A single-child container that takes the gesture over from its captured
    /// child on any `Move`.
    struct Taker {
        inner: ChildPod,
    }
    impl Widget for Taker {
        fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            self.inner.layout_child(ctx, bc);
            bc.constrain(Size::new(20.0, 20.0))
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            self.inner.paint_child(ctx, scene);
        }
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            match event {
                InputEvent::Pointer(p)
                    if p.phase == PointerPhase::Move && self.inner.is_active() =>
                {
                    ctx.release_captured_child(&mut self.inner);
                    EventResult::Handled
                }
                _ => self.inner.event_child(ctx, event),
            }
        }
    }

    /// An outer pod around a [`Taker`] around a [`Grab`].
    fn nested(opt_in: bool) -> ChildPod {
        let mut pod = ChildPod::new(Box::new(Taker {
            inner: ChildPod::new(Box::new(Grab { opt_in })),
        }));
        pod.layout_child(
            &mut LayoutCtx::new(),
            &BoxConstraints::tight(Size::new(20.0, 20.0)),
        );
        pod
    }

    fn pointer(phase: PointerPhase) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(5.0, 5.0),
            button: PointerButton::Primary,
        })
    }

    /// Dispatch into `pod` from a fresh context; whether a capture release
    /// bubbled out of it.
    fn released(pod: &mut ChildPod, phase: PointerPhase) -> bool {
        let mut unit = ();
        let mut ctx = EventCtx::new(&mut unit, Point::ZERO, Size::new(20.0, 20.0));
        pod.event_child(&mut ctx, &pointer(phase));
        ctx.is_capture_released()
    }

    fn inner_active(pod: &mut ChildPod) -> bool {
        pod.widget_mut()
            .downcast_mut::<Taker>()
            .expect("a Taker")
            .inner
            .is_active()
    }

    #[test]
    fn releasing_the_opted_in_child_signals_and_bubbles() {
        let mut pod = nested(true);
        assert!(!released(&mut pod, PointerPhase::Down));
        assert!(pod.holds_contact_opt_in(), "the opt-in below is recorded");
        assert!(
            released(&mut pod, PointerPhase::Move),
            "and its release bubbles"
        );
        assert!(!inner_active(&mut pod), "the child left the active path");
        assert!(pod.is_active(), "the container that took over keeps it");
    }

    #[test]
    fn releasing_a_child_without_the_opt_in_clears_the_link_silently() {
        let mut pod = nested(false);
        released(&mut pod, PointerPhase::Down);
        assert!(!pod.holds_contact_opt_in());
        assert!(!released(&mut pod, PointerPhase::Move));
        assert!(!inner_active(&mut pod));
    }

    #[test]
    fn a_non_claimant_contact_cannot_release_anything() {
        let mut pod = nested(true);
        released(&mut pod, PointerPhase::Down);
        let _pass = ContactPass::enter(PointerId::touch(1), true);
        assert!(!released(&mut pod, PointerPhase::Move));
        assert!(inner_active(&mut pod), "the claimant's link stands");
    }
}
