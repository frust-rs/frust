//! `page_view`: a scroll surface that pages — Flutter's `PageView` and
//! `PageController` (`widgets/page_view.dart`) over the horizontal scroll
//! surface and [`PageSnap`].
//!
//! Every page is laid out tight at `page_extent` (the viewport's main-axis
//! extent × [`PageController::viewport_fraction`]) and placed back to back with
//! no gap, so page `i` is current at offset `i × page_extent`.
//! [`PageView::pad_ends`] (default) pads both ends so the end pages can centre.
//!
//! A [`PageView`] composes [`custom_scroll_view`] under the sliver
//! [`ViewportWidget`] with one crate-private sliver, rather than embedding a
//! bare scroll core, so the viewport's axis-aware nested-claim rule stays intact.
//! [`PageController`] wraps a [`ScrollController`]; page commands are recorded
//! in pages and applied at the next layout or paint. Under `motion.reduce_motion`
//! [`PageController::animate_to_page`] lands in one frame.
//!
//! [`PageView::on_page_changed`] fires once per **settled** page change (the
//! initial page is recorded, not reported), where Flutter fires mid-drag; a
//! change seen at layout or paint is reported by the next `Housekeeping`.
//!
//! v1 limits: eager children (no builder, `keepPage`, `custom`, `reverse` or
//! `allowImplicitScrolling`) and a bounded main axis. A live, full-size page is
//! a navigator's or indexed stack's job, not this widget's. Flutter-parity
//! table: `docs/WIDGETS_ARCHITECTURE.md`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, ScrollAxis, SemanticsCtx, SliverConstraints, SliverGeometry,
    SliverWidget, View, ViewSeq, Widget,
};
use kurbo::{Point, Size};

use crate::authoring::{ErasedArgCallback, TypedArgCallback, erase_callback_arg};
use crate::physics::ScrollPhysics;
use crate::physics::page_snap::PageSnap;
use crate::scroll_controller::{AnimateTo, ScrollController};
use crate::scroll_core::ScrollSurface;
use crate::slivers::adapter::{axis_point, box_geometry, finite_extent};
use crate::viewport::{CustomScrollView, ViewportWidget, custom_scroll_view};

/// A viewport fraction made usable: anything non-finite or non-positive
/// becomes `1.0` (one page per viewport).
fn usable_fraction(fraction: f64) -> f64 {
    if fraction.is_finite() && fraction > 0.0 {
        fraction
    } else {
        1.0
    }
}

/// A page-addressed command recorded on a [`PageController`], converted to an
/// offset by the attached view.
#[derive(Clone, Copy, Debug, PartialEq)]
enum PageCommand {
    /// Jump to this page.
    Jump(usize),
    /// Ease to this page.
    Animate(usize, AnimateTo),
}

/// The state every clone of a [`PageController`] shares.
#[derive(Debug)]
struct PageShared {
    initial_page: usize,
    viewport_fraction: f64,
    /// Page commands recorded and not yet converted, in recording order.
    commands: Vec<PageCommand>,
}

/// A cloneable handle onto a [`PageView`] — Flutter's `PageController`.
/// Clones share one state; offset, `is_animating` and `on_change` are the
/// viewport's ([`PageController::scroll_controller`]). A drag that springs
/// back fires nothing; a change seen in an event is reported in that event.
///
/// ```
/// use frust_widgets::{PageController, PageView, page_view, text};
/// let controller = PageController::new().initial_page(1).viewport_fraction(0.8);
/// let view: PageView<()> = page_view((text("a"), text("b"), text("c"))).controller(controller.clone());
/// controller.jump_to_page(2); // applied at the view's next layout or paint
/// # let _ = view;
/// ```
#[derive(Clone, Debug)]
pub struct PageController {
    scroll: ScrollController,
    shared: Rc<RefCell<PageShared>>,
}

impl Default for PageController {
    fn default() -> Self {
        Self::new()
    }
}

impl PageController {
    /// A controller starting on page 0 with one page per viewport.
    pub fn new() -> Self {
        Self {
            scroll: ScrollController::new(),
            shared: Rc::new(RefCell::new(PageShared {
                initial_page: 0,
                viewport_fraction: 1.0,
                commands: Vec::new(),
            })),
        }
    }

    /// The page a view built with this controller starts on (Flutter's
    /// `initialPage`); `0` unless set. Applied when a view attaches this
    /// controller — at its first layout — without firing
    /// [`PageView::on_page_changed`]. Shared by every clone.
    pub fn initial_page(self, page: usize) -> Self {
        self.shared.borrow_mut().initial_page = page;
        self
    }

    /// The fraction of the viewport's main-axis extent each page occupies
    /// (Flutter's `viewportFraction`), `1.0` unless set. Shared by every
    /// clone.
    ///
    /// `fraction` must be finite and positive; this is a `debug_assert!`, and
    /// release builds fall back to `1.0`.
    pub fn viewport_fraction(self, fraction: f64) -> Self {
        debug_assert!(
            fraction.is_finite() && fraction > 0.0,
            "PageController::viewport_fraction needs a finite positive fraction, got {fraction}"
        );
        self.shared.borrow_mut().viewport_fraction = usable_fraction(fraction);
        self
    }

    /// The live, fractional page: the attached view's last published scroll
    /// offset over its page extent — `0.5` half way from page 0 to page 1,
    /// and integral once settled. Before a view has laid out, the initial
    /// page.
    pub fn page(&self) -> f64 {
        let stride = self.scroll.viewport_extent() * self.fraction();
        if !(stride.is_finite() && stride > 0.0) {
            return self.shared.borrow().initial_page as f64;
        }
        let page = self.scroll.offset() / stride;
        // A settled offset is `page × stride` computed from the same product,
        // so the quotient is integral up to float noise; report it exactly.
        let nearest = page.round();
        if (page - nearest).abs() < 1e-9 {
            nearest
        } else {
            page
        }
    }

    /// Jump to `page` at once, clamped to the scroll range. Recorded and
    /// applied at the attached view's next layout or paint, ending any motion
    /// in flight, like [`ScrollController::jump_to`].
    pub fn jump_to_page(&self, page: usize) {
        self.record(PageCommand::Jump(page));
    }

    /// Ease to `page` over `options`, clamped to the scroll range. Recorded
    /// and applied like [`PageController::jump_to_page`]; runs as a
    /// [`ScrollController::animate_to`] tween, so user input interrupts it
    /// and the theme's `motion.reduce_motion` collapses it to a jump.
    pub fn animate_to_page(&self, page: usize, options: AnimateTo) {
        self.record(PageCommand::Animate(page, options));
    }

    /// Whether the attached view is running an
    /// [`animate_to_page`](Self::animate_to_page) tween.
    pub fn is_animating(&self) -> bool {
        self.scroll.is_animating()
    }

    /// The [`ScrollController`] the attached view's viewport is bound to —
    /// for the raw offset, its extent, and [`ScrollController::on_change`].
    pub fn scroll_controller(&self) -> &ScrollController {
        &self.scroll
    }

    /// The configured viewport fraction, made usable.
    fn fraction(&self) -> f64 {
        usable_fraction(self.shared.borrow().viewport_fraction)
    }

    /// The configured initial page.
    fn initial(&self) -> usize {
        self.shared.borrow().initial_page
    }

    /// Queue `command`: a jump supersedes everything queued before it, an
    /// animate an earlier animate (the [`ScrollController`] queue's rule).
    /// Raises [`frust_core::mark_pending_result_flush`] so a frame runs to
    /// apply it.
    fn record(&self, command: PageCommand) {
        let mut shared = self.shared.borrow_mut();
        match command {
            PageCommand::Jump(_) => shared.commands.clear(),
            PageCommand::Animate(..) => shared
                .commands
                .retain(|queued| matches!(queued, PageCommand::Jump(_))),
        }
        shared.commands.push(command);
        drop(shared);
        frust_core::mark_pending_result_flush();
    }

    /// Whether page commands wait to be converted.
    fn has_pending(&self) -> bool {
        !self.shared.borrow().commands.is_empty()
    }

    /// Drain the queued page commands.
    fn take_commands(&self) -> Vec<PageCommand> {
        std::mem::take(&mut self.shared.borrow_mut().commands)
    }

    /// Whether `self` and `other` are clones of one handle.
    fn same(&self, other: &PageController) -> bool {
        Rc::ptr_eq(&self.shared, &other.shared)
    }
}

/// What the page sliver lays out with, shared by a [`PageView`] and the
/// sliver view inside its viewport (written by the page view's build and
/// rebuild).
#[derive(Clone, Copy, Debug, PartialEq)]
struct PageLayout {
    axis: ScrollAxis,
    fraction: f64,
    pad_ends: bool,
}

/// A view-held page-change callback.
type OnPageChanged<State> = TypedArgCallback<State, usize>;

/// A scroll surface that pages. See the [module docs](self).
pub struct PageView<State: 'static> {
    inner: CustomScrollView<State>,
    layout: Rc<Cell<PageLayout>>,
    page_count: usize,
    axis: ScrollAxis,
    pad_ends: bool,
    controller: Option<PageController>,
    physics_explicit: bool,
    on_page_changed: Option<OnPageChanged<State>>,
}

/// A horizontal [`PageView`] over `children`, one page each.
///
/// `children` is any [`ViewSeq`]: a tuple of mixed view types, a `Vec` or
/// array, an `Option` (`None` drops the slot), or `views(iter)`. Every page
/// is built eagerly (the [module docs](self)' *v1 limits*).
///
/// ```
/// use frust_widgets::{PageView, page_view, text};
/// let view: PageView<()> = page_view((text("one"), text("two"), text("three")))
///     .on_page_changed(|_: &mut (), page: usize| println!("on page {page}"));
/// # let _ = view;
/// ```
pub fn page_view<State: 'static, M>(children: impl ViewSeq<State, M>) -> PageView<State> {
    let mut pages = Vec::new();
    children.extend_views(&mut pages);
    let page_count = pages.len();
    let layout = Rc::new(Cell::new(PageLayout {
        axis: ScrollAxis::Horizontal,
        fraction: 1.0,
        pad_ends: true,
    }));
    let sliver = PageSliverView {
        pages,
        layout: Rc::clone(&layout),
    };
    PageView {
        inner: custom_scroll_view(sliver)
            .axis(ScrollAxis::Horizontal)
            .physics(PageSnap::fraction(1.0)),
        layout,
        page_count,
        axis: ScrollAxis::Horizontal,
        pad_ends: true,
        controller: None,
        physics_explicit: false,
        on_page_changed: None,
    }
}

impl<State: 'static> PageView<State> {
    /// Attach a [`PageController`] — its viewport fraction and initial page
    /// configure this view, and its page commands drive it. Unless
    /// [`PageView::physics`] names one, the physics becomes
    /// [`PageSnap::fraction`] of the controller's fraction.
    pub fn controller(mut self, controller: PageController) -> Self {
        if !self.physics_explicit {
            self.inner = self
                .inner
                .physics(PageSnap::fraction(controller.fraction()));
        }
        self.inner = self.inner.controller(controller.scroll.clone());
        self.controller = Some(controller);
        self
    }

    /// Page along `axis` — [`ScrollAxis::Horizontal`] by default (Flutter's
    /// `scrollDirection`).
    pub fn axis(mut self, axis: ScrollAxis) -> Self {
        self.axis = axis;
        self.inner = self.inner.axis(axis);
        self
    }

    /// Whether the first and last pages are padded so they can sit centred —
    /// `(viewport − page_extent) / 2` at each end, nothing at a fraction of
    /// `1.0`. `true` by default (Flutter's `padEnds`).
    pub fn pad_ends(mut self, pad_ends: bool) -> Self {
        self.pad_ends = pad_ends;
        self
    }

    /// Call `callback` with the page index once per settled page change.
    ///
    /// Unlike Flutter this fires on settle, not mid-drag. The initial page is
    /// recorded, not reported; a drag that springs back fires nothing; a change
    /// seen at layout or paint is reported by the next `Housekeeping`.
    pub fn on_page_changed<F: Fn(&mut State, usize) + 'static>(mut self, callback: F) -> Self {
        self.on_page_changed = Some(Rc::new(callback));
        self
    }

    /// Replace the default [`PageSnap`] physics — the
    /// [`CustomScrollView::physics`] contract. A physics that does not snap
    /// leaves the view resting between pages; `on_page_changed` then reports
    /// the nearest one.
    pub fn physics(mut self, physics: impl ScrollPhysics + 'static) -> Self {
        self.physics_explicit = true;
        self.inner = self.inner.physics(physics);
        self
    }

    /// The viewport fraction in effect.
    fn fraction(&self) -> f64 {
        self.controller
            .as_ref()
            .map_or(1.0, PageController::fraction)
    }

    /// Publish this view's page layout to its sliver, before the sliver's
    /// own build or rebuild reads it.
    fn sync_layout(&self) {
        self.layout.set(PageLayout {
            axis: self.axis,
            fraction: self.fraction(),
            pad_ends: self.pad_ends,
        });
    }
}

/// The retained widget for a [`PageView`]: the viewport, held inline, plus the
/// settle tracking behind `on_page_changed` and the controller's page queue.
pub struct PageViewWidget {
    viewport: ViewportWidget,
    fraction: f64,
    page_count: usize,
    controller: Option<PageController>,
    on_page_changed: Option<ErasedArgCallback<usize>>,
    /// The initial page owed to a newly attached controller, applied at the
    /// first layout or paint that can measure a page extent.
    initial_jump: Option<usize>,
    /// The page the surface last settled on, or `None` before it first did.
    reported_page: Option<usize>,
    /// A settled page change observed without an [`EventCtx`], reported on
    /// the next event.
    pending_page: Option<usize>,
}

impl PageViewWidget {
    /// The live scroll offset.
    pub fn offset(&self) -> f64 {
        self.viewport.offset()
    }

    /// The page extent from the last layout (`0.0` before one).
    fn page_extent(&self) -> f64 {
        stride(self.viewport.viewport_extent(), self.fraction)
    }

    /// Whether the viewport still holds the controller's scroll handle.
    fn holds_controller(&self) -> bool {
        self.viewport
            .core()
            .controller
            .as_ref()
            .is_some_and(|binding| binding.is_current())
    }

    /// Convert the owed initial page and the controller's queued page
    /// commands to offsets at `page_extent`, for the viewport to drain. Waits
    /// while the extent is unknown or another view holds the handle.
    fn convert_page_commands(&mut self, page_extent: f64) {
        if page_extent <= 0.0 || !self.holds_controller() {
            return;
        }
        let Some(controller) = self.controller.as_ref() else {
            return;
        };
        let at = |page: usize| page as f64 * page_extent;
        if let Some(page) = self.initial_jump.take() {
            controller.scroll.jump_to(at(page));
        }
        for command in controller.take_commands() {
            match command {
                PageCommand::Jump(page) => controller.scroll.jump_to(at(page)),
                PageCommand::Animate(page, options) => {
                    controller.scroll.animate_to(at(page), options);
                }
            }
        }
    }

    /// The page the surface rests on, or `None` while anything moves it or
    /// can still move it: a finger down, a drag, a fling or release spring, an
    /// edge settle, a controller tween.
    fn settled_page(&self) -> Option<usize> {
        let core = self.viewport.core();
        let moving = core.down_active || core.scrolling || core.settling || core.is_flinging();
        let page_extent = self.page_extent();
        if !core.laid_out || moving || page_extent <= 0.0 || self.page_count == 0 {
            return None;
        }
        let offset = self
            .viewport
            .offset()
            .clamp(0.0, self.viewport.max_offset());
        let page = (offset / page_extent).round() as usize;
        Some(page.min(self.page_count - 1))
    }

    /// Record a settled page change as owed to `on_page_changed`; the first
    /// page the surface settles on is recorded, not owed.
    fn observe(&mut self) {
        let Some(page) = self.settled_page() else {
            return;
        };
        if self.reported_page.is_some_and(|reported| reported != page) {
            self.pending_page = Some(page);
        }
        self.reported_page = Some(page);
    }

    /// Report an owed page change.
    fn deliver(&mut self, ctx: &mut EventCtx) {
        let Some(page) = self.pending_page.take() else {
            return;
        };
        if let Some(callback) = self.on_page_changed.as_mut() {
            callback(ctx, page);
            ctx.request_redraw();
        }
    }
}

/// `viewport × fraction` as a usable stride, `0.0` when it is not one.
fn stride(viewport: f64, fraction: f64) -> f64 {
    let stride = viewport * fraction;
    if stride.is_finite() && stride > 0.0 {
        stride
    } else {
        0.0
    }
}

impl<State: 'static> View<State> for PageView<State> {
    type Element = PageViewWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> PageViewWidget {
        self.sync_layout();
        let initial = self.controller.as_ref().map_or(0, PageController::initial);
        PageViewWidget {
            viewport: self.inner.build(ctx),
            fraction: self.fraction(),
            page_count: self.page_count,
            controller: self.controller.clone(),
            on_page_changed: self.on_page_changed.as_ref().map(erase_callback_arg),
            initial_jump: (initial > 0).then_some(initial),
            reported_page: None,
            pending_page: None,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut PageViewWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        self.sync_layout();
        let mut flags = self.inner.rebuild(&prev.inner, &mut element.viewport, ctx);
        // Closures are not comparable — always reinstall the erased adapter.
        element.on_page_changed = self.on_page_changed.as_ref().map(erase_callback_arg);
        element.page_count = self.page_count;
        let fraction = self.fraction();
        if element.fraction != fraction {
            element.fraction = fraction;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        let same = match (&element.controller, &self.controller) {
            (Some(held), Some(named)) => held.same(named),
            (None, None) => true,
            _ => false,
        };
        if !same {
            // A different handle is a new position: it starts on its own
            // initial page, as a Flutter `ScrollPosition` would.
            element.initial_jump = self.controller.as_ref().map(PageController::initial);
            element.controller = self.controller.clone();
        }
        if element.initial_jump.is_some()
            || element
                .controller
                .as_ref()
                .is_some_and(PageController::has_pending)
        {
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut PageViewWidget, ctx: &mut BuildCtx<'_>) {
        self.inner.teardown(&mut element.viewport, ctx);
    }

    fn message(&self, element: &mut PageViewWidget, state: &mut State) {
        self.inner.message(&mut element.viewport, state);
    }
}

impl Widget for PageViewWidget {
    /// Reports [`frust_core::InspectDetail::Clip`]: the page view (through its viewport) clips its content to its own box.
    fn inspect_detail(&self) -> Option<frust_core::InspectDetail> {
        self.viewport.inspect_detail()
    }

    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let main = self.viewport.axis().main_extent(bc.max());
        self.convert_page_commands(stride(finite_extent(main), self.fraction));
        let size = self.viewport.layout(ctx, bc);
        self.observe();
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.convert_page_commands(self.page_extent());
        self.viewport.paint(ctx, scene);
        self.observe();
        if self.pending_page.is_some() {
            // Paint has no `&mut State`: owe the report to the next frame's
            // Housekeeping broadcast.
            frust_core::mark_pending_result_flush();
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        self.deliver(ctx);
        let result = self.viewport.event(ctx, event);
        self.observe();
        self.deliver(ctx);
        result
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.viewport.semantics(ctx);
    }

    fn visit_children(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        self.viewport.visit_children(visitor);
    }

    fn is_focusable(&self) -> bool {
        self.viewport.is_focusable()
    }

    fn visit_focus_children(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        self.viewport.visit_focus_children(visitor);
    }
}

// --- The page sliver ---------------------------------------------------------

/// The sliver inside a [`PageView`]'s viewport: the pages, laid out at the
/// page stride with the end padding.
struct PageSliverView<State: 'static> {
    pages: Vec<AnyView<State>>,
    layout: Rc<Cell<PageLayout>>,
}

/// The retained widget for a [`PageSliverView`].
struct PageSliverWidget {
    pages: Vec<ChildPod>,
    layout: PageLayout,
    /// Each page's main-axis extent from the last layout.
    page_extent: f64,
    /// The paint extent from the last layout — where page culling stops.
    paint_extent: f64,
    /// Whether the last layout cut the run, so paint clips it.
    clip: bool,
}

impl<State: 'static> View<State> for PageSliverView<State> {
    type Element = PageSliverWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> PageSliverWidget {
        PageSliverWidget {
            pages: self
                .pages
                .iter()
                .map(|page| crate::authoring::build_child(page, ctx))
                .collect(),
            layout: self.layout.get(),
            page_extent: 0.0,
            paint_extent: 0.0,
            clip: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut PageSliverWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = crate::authoring::rebuild_children(
            &prev.pages,
            &self.pages,
            &mut element.pages,
            ctx,
            |view| view,
            |_| None,
        );
        let layout = self.layout.get();
        if element.layout != layout {
            element.layout = layout;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut PageSliverWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.pages.iter().zip(element.pages.iter_mut()) {
            crate::authoring::teardown_child(view, pod, ctx);
        }
    }
}

impl PageSliverWidget {
    /// Lay every page out tight at `page_extent` × `cross` and place it along
    /// `axis`, scrolled back by `scroll_offset`. Returns the run's total
    /// main-axis extent, end padding included.
    fn lay_pages(
        &mut self,
        ctx: &mut LayoutCtx,
        axis: ScrollAxis,
        viewport: f64,
        cross: f64,
        scroll_offset: f64,
    ) -> f64 {
        let page_extent = finite_extent(viewport * self.layout.fraction);
        let pad = if self.layout.pad_ends {
            ((viewport - page_extent) / 2.0).max(0.0)
        } else {
            0.0
        };
        let tight = BoxConstraints::tight(axis.size(page_extent, cross));
        for (index, pod) in self.pages.iter_mut().enumerate() {
            pod.layout_child(ctx, &tight);
            let main = pad + index as f64 * page_extent - scroll_offset;
            pod.set_origin(axis_point(axis, main, 0.0));
        }
        self.page_extent = page_extent;
        2.0 * pad + self.pages.len() as f64 * page_extent
    }

    /// Whether `pod` overlaps this sliver's painted region.
    fn shows(&self, pod: &ChildPod) -> bool {
        overlaps(self.layout.axis, pod, self.paint_extent, self.page_extent)
    }
}

impl Widget for PageSliverWidget {
    /// Reports [`frust_core::InspectDetail::Clip`]: the page run, while it is cut, clips its content to its own box.
    fn inspect_detail(&self) -> Option<frust_core::InspectDetail> {
        self.clip.then_some(frust_core::InspectDetail::Clip)
    }

    /// Outside a viewport the run is laid out unscrolled, its pages a fraction
    /// of the incoming main-axis maximum.
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let axis = self.layout.axis;
        let max = bc.max();
        let viewport = finite_extent(axis.main_extent(max));
        let cross = finite_extent(axis.cross_extent(max));
        let total = self.lay_pages(ctx, axis, viewport, cross, 0.0);
        let size = bc.constrain(axis.size(total, cross));
        self.paint_extent = axis.main_extent(size);
        self.clip = total > self.paint_extent;
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if self.clip {
            scene.push_clip(ctx.origin(), ctx.size());
        }
        let (axis, paint_extent, page_extent) =
            (self.layout.axis, self.paint_extent, self.page_extent);
        for pod in &mut self.pages {
            if overlaps(axis, pod, paint_extent, page_extent) {
                pod.paint_child(ctx, scene);
            }
        }
        if self.clip {
            scene.pop_clip();
        }
    }

    /// The viewport only delivers a positional event inside this sliver's
    /// hit extent (or to a captured page), so the ordinary multi-child
    /// routing applies.
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        crate::authoring::route_event(&mut self.pages, ctx, event)
    }

    /// The pages on screen.
    fn semantics(&self, ctx: &mut SemanticsCtx) {
        for pod in &self.pages {
            if self.shows(pod) {
                pod.semantics_child(ctx);
            }
        }
    }

    fn as_sliver(&mut self) -> Option<&mut dyn SliverWidget> {
        Some(self)
    }

    crate::authoring::visit_children!(pages);
}

impl SliverWidget for PageSliverWidget {
    fn layout_sliver(&mut self, ctx: &mut LayoutCtx, sc: &SliverConstraints) -> SliverGeometry {
        let viewport = finite_extent(sc.viewport_main_axis_extent);
        let cross = finite_extent(sc.cross_axis_extent);
        self.layout.axis = sc.axis;
        let total = self.lay_pages(ctx, sc.axis, viewport, cross, sc.scroll_offset);
        let geometry = box_geometry(total, sc);
        self.paint_extent = geometry.paint_extent;
        self.clip = geometry.has_visual_overflow;
        geometry
    }
}

/// `point`'s component along `axis`.
fn along(axis: ScrollAxis, point: Point) -> f64 {
    match axis {
        ScrollAxis::Vertical => point.y,
        ScrollAxis::Horizontal => point.x,
    }
}

/// Whether a page `page_extent` long at `pod`'s origin overlaps the painted
/// region `[0, paint_extent)` along `axis`.
fn overlaps(axis: ScrollAxis, pod: &ChildPod, paint_extent: f64, page_extent: f64) -> bool {
    let start = along(axis, pod.origin());
    start < paint_extent && start + page_extent > 0.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::Any;

    use frust_core::{Curve, FrameTime, PointerButton, PointerEvent, PointerPhase, RenderRoot};

    use crate::test_support::{RecordingScene, leaf, probe};

    const WINDOW: Size = Size::new(400.0, 400.0);

    fn ft(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn widget<State: 'static>(root: &RenderRoot<State, PageView<State>>) -> &PageViewWidget {
        let id = root.root_id().expect("root built");
        (root.tree().pod(id).expect("root pod").widget() as &dyn Any)
            .downcast_ref::<PageViewWidget>()
            .expect("root is a PageViewWidget")
    }

    /// The page sliver in the viewport's (`AnyView`-erased, so double-boxed)
    /// child pod.
    fn page_sliver(pod: &ChildPod) -> &PageSliverWidget {
        let boxed = (pod.widget() as &dyn Any)
            .downcast_ref::<Box<dyn Widget>>()
            .expect("AnyView-erased sliver: double-boxed ChildPod element");
        (boxed.as_ref() as &dyn Any)
            .downcast_ref::<PageSliverWidget>()
            .expect("the viewport's child is the page sliver")
    }

    /// Each page's main-axis position in viewport space, from the last layout.
    fn page_starts<State: 'static>(root: &RenderRoot<State, PageView<State>>) -> Vec<f64> {
        let w = widget(root);
        let axis = w.viewport.axis();
        let mut starts = Vec::new();
        w.viewport.visit_children(&mut |sliver| {
            for pod in &page_sliver(sliver).pages {
                starts.push(along(axis, sliver.origin()) + along(axis, pod.origin()));
            }
        });
        starts
    }

    fn page_sizes<State: 'static>(root: &RenderRoot<State, PageView<State>>) -> Vec<Size> {
        let mut sizes = Vec::new();
        widget(root).viewport.visit_children(&mut |sliver| {
            sizes.extend(page_sliver(sliver).pages.iter().map(ChildPod::size));
        });
        sizes
    }

    #[test]
    fn page_view_reports_clip_only_when_its_pages_are_cut() {
        use frust_core::InspectDetail;
        let rig = Rig::new(&PageController::new(), ScrollAxis::Vertical, None);
        let nodes = rig.root.inspect();
        assert_eq!(nodes[0].detail, Some(InspectDetail::Clip));

        let mut fit: RenderRoot<(), PageView<()>> = RenderRoot::new();
        fit.rebuild(&mut |_: &mut ()| page_view((leaf(1.0, 1.0),)), &mut ());
        fit.layout(WINDOW);
        let nodes = fit.inspect();
        // The page run fits its viewport, so its own pod reports no clip.
        assert!(nodes.iter().skip(1).any(|n| n.detail.is_none()));
    }

    /// The rig's build closure.
    type Logic = Box<dyn FnMut(&mut ()) -> PageView<()>>;

    /// A rig over four leaves (laid out tight to the page) in a
    /// [`WINDOW`]-sized page view, logging every `on_page_changed`.
    struct Rig {
        root: RenderRoot<(), PageView<()>>,
        logic: Logic,
        log: Rc<RefCell<Vec<usize>>>,
        t: f64,
    }

    impl Rig {
        fn new(
            controller: &PageController,
            axis: ScrollAxis,
            theme: Option<frust_theme::Theme>,
        ) -> Self {
            let log = Rc::new(RefCell::new(Vec::new()));
            let mut logic: Logic = {
                let log = Rc::clone(&log);
                let controller = controller.clone();
                Box::new(move |_: &mut ()| {
                    let log = Rc::clone(&log);
                    page_view((
                        leaf(1.0, 1.0),
                        leaf(1.0, 1.0),
                        leaf(1.0, 1.0),
                        leaf(1.0, 1.0),
                    ))
                    .axis(axis)
                    .controller(controller.clone())
                    .on_page_changed(move |_: &mut (), page: usize| {
                        log.borrow_mut().push(page);
                    })
                })
            };
            let mut root = RenderRoot::new();
            if let Some(theme) = theme {
                root.set_theme(Box::new(theme));
            }
            root.rebuild(&mut logic, &mut ());
            let mut rig = Self {
                root,
                logic,
                log,
                t: 0.0,
            };
            rig.frame(0.0);
            rig
        }

        /// One shell frame `dt_ms` after the last: rebuild (which delivers an
        /// owed Housekeeping flush), layout, paint.
        fn frame(&mut self, dt_ms: f64) {
            self.t += dt_ms;
            self.root.rebuild(&mut self.logic, &mut ());
            self.root.layout(WINDOW);
            self.root.paint(&mut RecordingScene::default(), ft(self.t));
        }

        /// Enough frames for any motion to rest and an owed report to land.
        fn settle(&mut self) {
            for _ in 0..600 {
                self.frame(16.0);
            }
        }

        fn pointer(&mut self, phase: PointerPhase, main: f64) {
            let position = match widget(&self.root).viewport.axis() {
                ScrollAxis::Horizontal => Point::new(main, 50.0),
                ScrollAxis::Vertical => Point::new(50.0, main),
            };
            self.root.event(
                &mut (),
                &InputEvent::Pointer(PointerEvent {
                    phase,
                    position,
                    button: PointerButton::Primary,
                }),
            );
        }

        /// Press at `from`, take the drag over past the slop, then move the
        /// finger back by `travel` px with `gap_ms` between samples.
        fn drag(&mut self, from: f64, travel: f64, gap_ms: f64) {
            self.pointer(PointerPhase::Down, from);
            self.frame(gap_ms);
            // Crossing the slop only takes the gesture over; the offset follows
            // the finger from the next move on.
            let taken = from - 20.0;
            self.pointer(PointerPhase::Move, taken);
            self.frame(gap_ms);
            for step in 1..=4 {
                self.pointer(PointerPhase::Move, taken - travel * step as f64 / 4.0);
                self.frame(gap_ms);
            }
        }

        fn release(&mut self, at: f64) {
            self.pointer(PointerPhase::Up, at);
        }

        fn offset(&self) -> f64 {
            widget(&self.root).offset()
        }

        fn log(&self) -> Vec<usize> {
            self.log.borrow().clone()
        }
    }

    fn mount(fraction: f64, pad_ends: bool, axis: ScrollAxis) -> RenderRoot<(), PageView<()>> {
        let controller = PageController::new().viewport_fraction(fraction);
        let mut logic = move |_: &mut ()| {
            page_view((
                leaf(1.0, 1.0),
                leaf(1.0, 1.0),
                leaf(1.0, 1.0),
                leaf(1.0, 1.0),
            ))
            .axis(axis)
            .pad_ends(pad_ends)
            .controller(controller.clone())
        };
        let mut root = RenderRoot::new();
        root.rebuild(&mut logic, &mut ());
        root.layout(WINDOW);
        root.paint(&mut RecordingScene::default(), FrameTime::ZERO);
        root
    }

    #[test]
    fn pages_lay_out_at_the_fraction_with_centring_end_padding() {
        for axis in [ScrollAxis::Horizontal, ScrollAxis::Vertical] {
            let root = mount(0.8, true, axis);
            assert_eq!(page_starts(&root), [40.0, 360.0, 680.0, 1000.0], "{axis:?}");
            let page = axis.size(320.0, 400.0);
            assert!(
                page_sizes(&root).iter().all(|size| *size == page),
                "{axis:?}"
            );
            // Page 3 is current at 3 × 320, where it sits centred.
            assert_eq!(widget(&root).viewport.max_offset(), 960.0, "{axis:?}");

            let root = mount(1.0, true, axis);
            assert_eq!(page_starts(&root), [0.0, 400.0, 800.0, 1200.0], "{axis:?}");
            assert_eq!(widget(&root).viewport.max_offset(), 1200.0, "{axis:?}");
        }
    }

    #[test]
    fn without_end_padding_the_run_starts_at_the_leading_edge() {
        let root = mount(0.8, false, ScrollAxis::Horizontal);
        assert_eq!(page_starts(&root), [0.0, 320.0, 640.0, 960.0]);
        assert_eq!(widget(&root).viewport.max_offset(), 880.0);
    }

    #[test]
    fn a_slow_release_short_of_half_a_page_settles_back_without_a_report() {
        for axis in [ScrollAxis::Horizontal, ScrollAxis::Vertical] {
            let controller = PageController::new();
            let mut rig = Rig::new(&controller, axis, None);
            // 0.4 pages in, with the finger nearly still at release.
            rig.drag(290.0, 160.0, 2000.0);
            assert!(
                (rig.offset() - 160.0).abs() < 1.0,
                "{axis:?}: dragged to {}",
                rig.offset()
            );
            rig.release(110.0);
            rig.settle();
            assert_eq!(rig.offset(), 0.0, "{axis:?}");
            assert_eq!(controller.page(), 0.0, "{axis:?}");
            assert!(rig.log().is_empty(), "{axis:?}: {:?}", rig.log());
        }
    }

    #[test]
    fn a_strong_fling_settles_on_the_next_page_and_reports_it_once() {
        for axis in [ScrollAxis::Horizontal, ScrollAxis::Vertical] {
            let controller = PageController::new();
            let mut rig = Rig::new(&controller, axis, None);
            rig.drag(290.0, 160.0, 8.0);
            rig.release(110.0);
            rig.settle();
            assert_eq!(rig.offset(), 400.0, "{axis:?}");
            assert_eq!(controller.page(), 1.0, "{axis:?}");
            assert_eq!(rig.log(), [1], "{axis:?}");
        }
    }

    #[test]
    fn page_is_fractional_mid_drag_and_integral_when_settled() {
        for axis in [ScrollAxis::Horizontal, ScrollAxis::Vertical] {
            let controller = PageController::new();
            let mut rig = Rig::new(&controller, axis, None);
            assert_eq!(controller.page(), 0.0);
            // Half a stride in, finger still down.
            rig.drag(290.0, 200.0, 16.0);
            let page = controller.page();
            assert!((page - 0.5).abs() < 0.01, "{axis:?}: mid-drag page {page}");
            assert_eq!(
                rig.log(),
                Vec::<usize>::new(),
                "{axis:?}: nothing settled yet"
            );
            rig.release(70.0);
            rig.settle();
            let page = controller.page();
            assert_eq!(page, page.round(), "{axis:?}: settled page {page}");
        }
    }

    #[test]
    fn jump_and_animate_to_page_land_and_report_once() {
        for axis in [ScrollAxis::Horizontal, ScrollAxis::Vertical] {
            let controller = PageController::new();
            let mut rig = Rig::new(&controller, axis, None);
            controller.jump_to_page(2);
            rig.frame(16.0);
            assert_eq!(rig.offset(), 800.0, "{axis:?}");
            rig.settle();
            assert_eq!(controller.page(), 2.0, "{axis:?}");
            assert_eq!(rig.log(), [2], "{axis:?}");

            let controller = PageController::new();
            let mut rig = Rig::new(&controller, axis, None);
            controller.animate_to_page(
                2,
                AnimateTo {
                    duration_ms: 300.0,
                    curve: Curve::Linear,
                },
            );
            rig.frame(16.0);
            rig.frame(16.0);
            rig.frame(100.0);
            let mid = rig.offset();
            assert!(mid > 0.0 && mid < 800.0, "{axis:?}: mid-tween at {mid}");
            assert!(controller.is_animating(), "{axis:?}");
            assert!(rig.log().is_empty(), "{axis:?}: no report mid-tween");
            rig.settle();
            assert_eq!(rig.offset(), 800.0, "{axis:?}");
            assert_eq!(rig.log(), [2], "{axis:?}");
        }
    }

    #[test]
    fn an_initial_page_is_where_the_view_starts_and_is_not_reported() {
        for axis in [ScrollAxis::Horizontal, ScrollAxis::Vertical] {
            let controller = PageController::new().initial_page(1);
            let mut rig = Rig::new(&controller, axis, None);
            assert_eq!(
                rig.offset(),
                400.0,
                "{axis:?}: on page 1 from the first frame"
            );
            assert_eq!(page_starts(&rig.root)[1], 0.0, "{axis:?}");
            rig.settle();
            assert_eq!(controller.page(), 1.0, "{axis:?}");
            assert!(rig.log().is_empty(), "{axis:?}: {:?}", rig.log());
        }
    }

    #[test]
    fn reduce_motion_lands_animate_to_page_in_one_frame() {
        let controller = PageController::new();
        let mut theme = frust_theme::Theme::neutral();
        theme.motion.reduce_motion = true;
        let mut rig = Rig::new(&controller, ScrollAxis::Horizontal, Some(theme));
        controller.animate_to_page(
            2,
            AnimateTo {
                duration_ms: 300.0,
                curve: Curve::Linear,
            },
        );
        rig.frame(16.0);
        assert_eq!(rig.offset(), 800.0);
        assert!(!controller.is_animating());
        rig.frame(16.0);
        assert_eq!(rig.log(), [2]);
    }

    #[test]
    fn a_drag_past_the_slop_cancels_the_page_under_the_finger() {
        let mut logic = |_: &mut Vec<u32>| page_view((probe(1), probe(2)));
        let mut root: RenderRoot<Vec<u32>, PageView<Vec<u32>>> = RenderRoot::new();
        let mut state = Vec::new();
        root.rebuild(&mut logic, &mut state);
        root.layout(WINDOW);
        root.paint(&mut RecordingScene::default(), FrameTime::ZERO);
        let ev = |phase, x| {
            InputEvent::Pointer(PointerEvent {
                phase,
                position: Point::new(x, 50.0),
                button: PointerButton::Primary,
            })
        };
        root.event(&mut state, &ev(PointerPhase::Down, 300.0));
        assert_eq!(state, [1], "the press reaches page 0");
        root.event(&mut state, &ev(PointerPhase::Move, 290.0));
        assert_eq!(state, [1, 1], "inside the slop the move still reaches it");
        root.event(&mut state, &ev(PointerPhase::Move, 270.0));
        assert_eq!(state, [1, 1, 1], "the takeover cancels it");
        root.event(&mut state, &ev(PointerPhase::Move, 200.0));
        root.event(&mut state, &ev(PointerPhase::Up, 200.0));
        assert_eq!(state, [1, 1, 1], "the drag is the surface's from then on");
    }

    #[test]
    fn a_page_controller_clone_shares_its_state() {
        let a = PageController::new();
        let b = a.clone().initial_page(3).viewport_fraction(0.5);
        assert!(a.same(&b));
        assert_eq!((a.initial(), a.fraction()), (3, 0.5));
        assert_eq!(a.page(), 3.0, "before any layout, the initial page");
        b.jump_to_page(1);
        assert!(a.has_pending());
        a.animate_to_page(
            2,
            AnimateTo {
                duration_ms: 10.0,
                curve: Curve::Linear,
            },
        );
        assert_eq!(
            b.take_commands().len(),
            2,
            "a jump keeps, the animate follows"
        );
    }
}
