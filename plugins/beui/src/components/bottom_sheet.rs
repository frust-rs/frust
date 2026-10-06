//! Ports beUI's `bottom-sheet` component — `components/motion/bottom-sheet.tsx`
//! (beUI rev `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01).
//!
//! | upstream | here |
//! |---|---|
//! | `snapPoints = [0.5, 0.92]`, `defaultSnap = 0` | [`SHEET_SNAP_POINTS`] |
//! | `DRAWER = {duration: 0.5, ease: EASE_DRAWER}` | [`SHEET_GLIDE`] on [`EASE_DRAWER`] |
//! | `dismissThreshold = 120` | [`SHEET_DISMISS_THRESHOLD`] |
//! | `velocity > 600` dismiss, `< 800` step down instead | [`SHEET_DISMISS_VELOCITY`], [`SHEET_FLING_VELOCITY`] |
//! | `velocity < -500` next snap | [`SHEET_RAISE_VELOCITY`] |
//! | `offset > 80` / `< -80` step | [`SHEET_STEP_OFFSET`] |
//! | `dragElastic {top: 0.02, bottom: 0.4}` | [`SHEET_ELASTIC_DOWN`], [`SHEET_ELASTIC_UP`] |
//! | `max-w-2xl mx-auto` | [`SHEET_MAX_WIDTH`] |
//! | `rounded-t-3xl border border-border bg-background shadow-xl` | [`SheetPanelWidget`]'s surface |
//! | handle `h-1.5 w-10 rounded-full bg-muted-foreground/40` | [`SHEET_HANDLE_WIDTH`], [`SHEET_HANDLE_HEIGHT`] |
//! | scrim `bg-background/40`, same `DRAWER` curve | [`crate::overlay::scrim`] at [`SHEET_GLIDE`] |
//!
//! # What the host owns and what the sheet owns
//!
//! The scrim, the bottom-edge mount, the slide, the barrier, Escape, the
//! backdrop click, the back press and the staged exit are all
//! [`crate::overlay::modal`]'s — this component builds no host. What is the
//! sheet's own is the chrome (the top-rounded surface, the grip, the title
//! block) and the **drag**: a pull on the grip, a velocity-aware decision at the
//! end of it, and a snap between the two heights upstream ships.
//!
//! The snap is *reported*, not owned: the resolved height is
//! [`ModalConfig::height`], which the host resolves at layout, so the sheet
//! reports [`BottomSheetView::on_snap_change`] from the event pass the gesture
//! ends on and the app hands the new index back through
//! [`BottomSheetView::snap`]. Every other widget in this catalog is controlled
//! the same way, and this one has a real reason beyond consistency: the height
//! is a *constraint*, and only the host can change it.
//!
//! # Velocity without an event clock
//!
//! `PanInfo.velocity` has no frust equivalent: `InputEvent` carries no
//! timestamp, and `EventCtx` no clock. The drag therefore samples its own offset
//! against [`PaintCtx::frame_time`](frust::authoring::PaintCtx::frame_time) each
//! painted frame and differentiates that — the sheet is repainting every frame
//! of a drag anyway, so the samples are there for free. It is a frame-rate
//! resolution estimate rather than the pointer's own instantaneous velocity, and
//! the thresholds below are upstream's numbers read against it.
//!
//! # Degradations against the web original
//!
//! - **No backdrop blur.** `backdrop-blur-sm` has no `PaintScene` primitive; the
//!   scrim is the wash alone.
//! - **The `"auto"` snap point is not modelled.** `snapPoints` here is a list of
//!   fractions ([`SHEET_SNAP_POINTS`]); upstream's `"auto"` entry means "hug the
//!   content up to `92vh`", which is [`ModalExtent::Hug`] plus a cap — reachable
//!   by configuring the host directly, but not expressible as a *snap point* the
//!   drag steps through, since the step arithmetic compares fractions.
//! - **The drag grip is the whole handle row.** Upstream's `items-center`
//!   shrink-wraps the drag area to the 40px pill; that is a poor target with a
//!   mouse, so the full-width row that contains it starts the drag here.
//! - **No scroll lock and no `overscroll-contain`.** Nothing scrolls behind a
//!   frust modal — the host swallows every event its panel declined — so the
//!   `position: fixed` body lock upstream needs has nothing to do here.
//! - **`reduce_motion` collapses the glide** rather than running upstream's own
//!   180ms fade: [`crate::motion::Presence::collapsed`] is a jump, and the
//!   preference is about the travel either way.
//! - **The drag itself is never reduced.** A drag follows the pointer, so
//!   `reduce_motion` leaves it alone; only the glide it hands back to is
//!   collapsed.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Affine, AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, Color, CornerRadii,
    ErasedArgCallback, ErasedCallback, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx,
    PaintScene, Point, PointerPhase, Rect, SemanticsCtx, Size, Vec2, View, Widget, any,
    build_child, erase_callback, erase_callback_arg, rebuild_child, route_event_single,
    teardown_child,
    text::{FontWeight, TextStyle},
    visit_children,
};
use frust::{FrameTime, Theme};

use crate::components::popover::{
    PANEL_SHADOW_ALPHA, PANEL_SHADOW_BLUR, PANEL_SHADOW_Y_OFFSET, paint_panel_hairline,
    resolve_panel,
};
use crate::motion::Ramp;
use crate::overlay::{
    ModalConfig, ModalContent, ModalEdge, ModalExtent, ModalLimit, ModalMount, ModalView,
    ModalWidget, modal,
};
use crate::press::{inside, presses};
use crate::style;
use crate::text::{LabelRun, ThemeTextType, themed_style};
use crate::tokens::motion::EASE_DRAWER;

/// `snapPoints` — the sheet's own two resting heights, as fractions of the host
/// area. The first is `defaultSnap`.
pub const SHEET_SNAP_POINTS: [f64; 2] = [0.5, 0.92];

/// `DRAWER.duration` — the vaul-style glide the sheet and its scrim both travel
/// on. A long, fully damped tween reads smoother than a spring here: no settle,
/// no overshoot, one clean decel.
pub const SHEET_GLIDE: Duration = Duration::from_millis(500);

/// `max-w-2xl` — the sheet's width cap, in logical px.
pub const SHEET_MAX_WIDTH: f64 = 672.0;

/// `rounded-t-3xl` — the radius of the sheet's two top corners, in logical px.
pub const SHEET_RADIUS: f64 = style::RADIUS_3XL;

/// `w-10` — the grip pill's width, in logical px.
pub const SHEET_HANDLE_WIDTH: f64 = 40.0;

/// `h-1.5` — the grip pill's height, in logical px.
pub const SHEET_HANDLE_HEIGHT: f64 = 6.0;

/// The grip pill's alpha over the dimmed ink role
/// (`bg-muted-foreground/40`).
pub const SHEET_HANDLE_ALPHA: f32 = 0.40;

/// `pt-3` — the header's top padding, in logical px.
pub const SHEET_HEADER_TOP: f64 = 12.0;

/// `py-1` — the padding around the grip inside its row, in logical px.
pub const SHEET_HANDLE_PAD: f64 = 4.0;

/// `pb-2` — the header's bottom padding, in logical px.
pub const SHEET_HEADER_BOTTOM: f64 = 8.0;

/// `px-4` — the sheet's horizontal padding, in logical px.
pub const SHEET_PADDING_X: f64 = 16.0;

/// `pb-6` — the body's bottom padding, in logical px.
pub const SHEET_BODY_BOTTOM: f64 = 24.0;

/// `mt-2` — the gap between the grip row and the title block, in logical px.
pub const SHEET_TITLE_GAP: f64 = 8.0;

/// `mt-0.5` — the gap between the title and its description, in logical px.
pub const SHEET_DESCRIPTION_GAP: f64 = 2.0;

/// `dismissThreshold` — how far past the current snap a pull must travel to
/// dismiss, in logical px of *pointer* travel.
pub const SHEET_DISMISS_THRESHOLD: f64 = 120.0;

/// `velocity > 600` — a downward fling this fast dismisses (or steps down).
pub const SHEET_DISMISS_VELOCITY: f64 = 600.0;

/// `velocity < 800` — below this a fling past the threshold steps down to the
/// next snap instead of dismissing, when there is one.
pub const SHEET_FLING_VELOCITY: f64 = 800.0;

/// `velocity < -500` — an upward fling this fast raises the sheet a snap.
pub const SHEET_RAISE_VELOCITY: f64 = -500.0;

/// `offset > 80` / `< -80` — how far a slow drag must travel to step a snap.
pub const SHEET_STEP_OFFSET: f64 = 80.0;

/// The share of a downward pull the sheet actually travels
/// (`dragElastic.bottom`).
pub const SHEET_ELASTIC_DOWN: f64 = 0.4;

/// The share of an upward pull the sheet travels (`dragElastic.top`) — an
/// upward pull is almost fully resisted, since there is nothing above the
/// sheet's own snap to reach.
pub const SHEET_ELASTIC_UP: f64 = 0.02;

/// What a finished drag decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SheetRelease {
    /// Stay where it is.
    Hold,
    /// Move to this snap index.
    Snap(usize),
    /// Close the sheet.
    Dismiss,
}

/// Resolve a finished drag — `bottom-sheet.tsx`'s `onDragEnd`, verbatim.
///
/// `offset` is the pointer's own downward travel in logical px (positive is
/// down) and `velocity` is in px/s. `snap` is the index the sheet is resting at
/// within `count` snap points.
///
/// Pure and total, so the whole decision table is testable without a gesture.
pub fn resolve_release(
    offset: f64,
    velocity: f64,
    snap: usize,
    count: usize,
    dismiss_threshold: f64,
) -> SheetRelease {
    if velocity > SHEET_DISMISS_VELOCITY || offset > dismiss_threshold {
        // A pull that is firm but not a fling, with a lower snap to fall back
        // to, steps down instead of closing.
        if snap > 0 && velocity < SHEET_FLING_VELOCITY && offset < dismiss_threshold * 1.6 {
            return SheetRelease::Snap(snap - 1);
        }
        return SheetRelease::Dismiss;
    }
    if velocity < SHEET_RAISE_VELOCITY {
        return SheetRelease::Snap(count.saturating_sub(1).min(snap + 1));
    }
    if offset > SHEET_STEP_OFFSET && snap > 0 {
        return SheetRelease::Snap(snap - 1);
    }
    if offset < -SHEET_STEP_OFFSET && snap + 1 < count {
        return SheetRelease::Snap(snap + 1);
    }
    SheetRelease::Hold
}

/// How far the sheet travels for a pointer pull of `offset` — upstream's
/// `dragElastic` pair. The thresholds above read the *pointer's* offset; this is
/// only what is drawn.
pub fn elastic(offset: f64) -> f64 {
    if offset >= 0.0 {
        offset * SHEET_ELASTIC_DOWN
    } else {
        offset * SHEET_ELASTIC_UP
    }
}

/// The [`ModalConfig`] a sheet at `height` (a fraction of the area) is hosted
/// with.
pub fn bottom_sheet_config(height: f64) -> ModalConfig {
    let glide = Ramp::eased(SHEET_GLIDE, EASE_DRAWER);
    ModalConfig {
        mount: ModalMount::Edge(ModalEdge::Bottom),
        width: ModalExtent::Fraction(1.0),
        max_width: ModalLimit::Px(SHEET_MAX_WIDTH),
        height: ModalExtent::Fraction(height.clamp(0.0, 1.0)),
        max_height: ModalLimit::None,
        margin: 0.0,
        ..ModalConfig::sheet()
            .ramps(glide, glide)
            .scrim_fade(SHEET_GLIDE)
    }
}

// ---- The component ---------------------------------------------------------

/// The mutable chrome the outer builder writes and the panel reads.
type SheetHandle = Rc<std::cell::RefCell<SheetChrome>>;

/// The panel's two state-bearing hooks, written by the outer builder after the
/// panel view already exists.
///
/// The panel is the host's content, which [`ModalView`] holds erased, so a
/// callback arriving after [`bottom_sheet`] cannot be written onto it; it is
/// written here and read at build/rebuild instead — the same handle shape
/// [`crate::components::popover`] documents, carrying `State`-bearing closures
/// rather than plain data.
type HookHandle<State> = Rc<
    std::cell::RefCell<(
        Option<Rc<dyn Fn(&mut State, usize)>>,
        Option<Rc<dyn Fn(&mut State)>>,
    )>,
>;

/// The sheet's own chrome and gesture configuration.
#[derive(Clone, Debug, PartialEq)]
struct SheetChrome {
    title: Option<String>,
    description: Option<String>,
    snap: usize,
    snaps: usize,
    dismiss_threshold: f64,
}

impl Default for SheetChrome {
    fn default() -> Self {
        SheetChrome {
            title: None,
            description: None,
            snap: 0,
            snaps: SHEET_SNAP_POINTS.len(),
            dismiss_threshold: SHEET_DISMISS_THRESHOLD,
        }
    }
}

/// A declarative beUI bottom sheet. See [`bottom_sheet`].
pub struct BottomSheetView<State: 'static> {
    inner: ModalView<State>,
    chrome: SheetHandle,
    hooks: HookHandle<State>,
    points: Vec<f64>,
}

/// Build a bottom sheet over `content`, resting at the first of
/// [`SHEET_SNAP_POINTS`] and open by default.
///
/// Mount it as the top child of a full-area [`frust::Stack`] with
/// [`BottomSheetView::open`] carrying the app's own flag, or push it as a
/// transparent navigator page with [`crate::overlay::show_modal`].
pub fn bottom_sheet<State: 'static, V: View<State>>(content: V) -> BottomSheetView<State> {
    let chrome: SheetHandle = Rc::new(std::cell::RefCell::new(SheetChrome::default()));
    let hooks: HookHandle<State> = Rc::new(std::cell::RefCell::new((None, None)));
    let points = SHEET_SNAP_POINTS.to_vec();
    let panel = SheetPanelView {
        content: any(content),
        chrome: chrome.clone(),
        hooks: hooks.clone(),
    };
    BottomSheetView {
        inner: modal(panel, bottom_sheet_config(points[0])),
        chrome,
        hooks,
        points,
    }
}

impl<State: 'static> BottomSheetView<State> {
    /// Replace the snap points — fractions of the host area's height, lowest
    /// first (default [`SHEET_SNAP_POINTS`]).
    pub fn snap_points(mut self, points: Vec<f64>) -> Self {
        if !points.is_empty() {
            self.points = points;
            self.chrome.borrow_mut().snaps = self.points.len();
            self.resnap();
        }
        self
    }

    /// Rest the sheet at snap `index` — the app's own value, updated from
    /// [`on_snap_change`](Self::on_snap_change).
    pub fn snap(mut self, index: usize) -> Self {
        self.chrome.borrow_mut().snap = index.min(self.points.len() - 1);
        self.resnap();
        self
    }

    /// Re-resolve the host's height from the current snap, keeping every setting
    /// a caller has already made.
    fn resnap(&mut self) {
        let snap = self.chrome.borrow().snap.min(self.points.len() - 1);
        self.inner.config = ModalConfig {
            dismissable: self.inner.config.dismissable,
            enter: self.inner.config.enter,
            exit: self.inner.config.exit,
            ..bottom_sheet_config(self.points[snap])
        };
    }

    /// Set the sheet's title (`text-base font-semibold`).
    pub fn title(self, title: impl Into<String>) -> Self {
        self.chrome.borrow_mut().title = Some(title.into());
        self
    }

    /// Set the sheet's description (`text-sm text-muted-foreground`).
    pub fn description(self, description: impl Into<String>) -> Self {
        self.chrome.borrow_mut().description = Some(description.into());
        self
    }

    /// Set how far a pull must travel to dismiss, in logical px (default
    /// [`SHEET_DISMISS_THRESHOLD`]).
    pub fn dismiss_threshold(self, px: f64) -> Self {
        self.chrome.borrow_mut().dismiss_threshold = px.max(0.0);
        self
    }

    /// Hand the sheet the app's open flag. The default is `true`.
    pub fn open(mut self, open: bool) -> Self {
        self.inner = self.inner.open(open);
        self
    }

    /// The accessibility label the panel is announced with. A sheet with a
    /// [`title`](Self::title) is announced by that instead.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.inner = self.inner.label(label);
        self
    }

    /// Whether Escape, a backdrop click, a back press and a drag-dismiss close
    /// it (default `true`).
    pub fn dismissable(mut self, dismissable: bool) -> Self {
        let config = self.inner.config.dismissable(dismissable);
        self.inner = self.inner.config(config);
        self
    }

    /// Set the close callback: Escape, a backdrop click, a back press, or a drag
    /// past the dismiss threshold. Reports `false`.
    pub fn on_open_change<F: Fn(&mut State, bool) + 'static>(mut self, on_open_change: F) -> Self {
        let shared = Rc::new(on_open_change);
        let host = shared.clone();
        self.inner = self.inner.on_dismiss(move |state| host(state, false));
        self.hooks.borrow_mut().1 = Some(Rc::new(move |state: &mut State| shared(state, false)));
        self
    }

    /// Set the snap-change callback: a finished drag that landed on a different
    /// snap point reports its index, and the app hands it back through
    /// [`snap`](Self::snap).
    pub fn on_snap_change<F: Fn(&mut State, usize) + 'static>(self, on_snap_change: F) -> Self {
        self.hooks.borrow_mut().0 = Some(Rc::new(on_snap_change));
        self
    }

    /// Set the exit-settled callback — state-free, fired from the paint that
    /// finishes the exit a dismissal staged.
    pub fn on_close<F: Fn() + 'static>(mut self, on_close: F) -> Self {
        self.inner = self.inner.on_close(on_close);
        self
    }

    /// The snap fractions this sheet steps through.
    pub fn snap_fractions(&self) -> &[f64] {
        &self.points
    }
}

impl<State: 'static> View<State> for BottomSheetView<State> {
    type Element = ModalWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ModalWidget {
        View::build(&self.inner, ctx)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ModalWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        View::rebuild(&self.inner, &prev.inner, element, ctx)
    }

    fn teardown(&self, element: &mut ModalWidget, ctx: &mut BuildCtx<'_>) {
        View::teardown(&self.inner, element, ctx);
    }
}

impl<State: 'static> ModalContent<State> for BottomSheetView<State> {
    fn modal_dismissable(&self) -> bool {
        self.inner.config.dismissable
    }
}

// ---- The panel -------------------------------------------------------------

/// The sheet's own surface, header and drag.
struct SheetPanelView<State: 'static> {
    content: AnyView<State>,
    chrome: SheetHandle,
    hooks: HookHandle<State>,
}

/// The retained widget for a bottom-sheet panel.
pub struct SheetPanelWidget {
    content: ChildPod,
    chrome: SheetChrome,
    title: Option<LabelRun>,
    description: Option<LabelRun>,
    /// The full-width row the grip sits in, in this widget's own space.
    grip_row: Rect,
    /// The grip pill itself.
    grip: Rect,
    /// The pointer's own travel since the drag started, in logical px.
    offset: Cell<f64>,
    /// Where the drag started, in this widget's own space.
    from: Option<Point>,
    /// The last `(offset, frame)` pair a paint sampled, for the velocity
    /// estimate the module docs describe.
    sample: Cell<Option<(f64, FrameTime)>>,
    /// The drag's current velocity, in logical px per second.
    velocity: Cell<f64>,
    on_snap_change: Option<ErasedArgCallback<usize>>,
    on_dismiss: Option<ErasedCallback>,
}

impl SheetPanelWidget {
    /// The pointer's travel since the drag started, `0.0` when not dragging.
    pub fn drag_offset(&self) -> f64 {
        self.offset.get()
    }

    /// The drag's estimated velocity, in logical px per second.
    pub fn drag_velocity(&self) -> f64 {
        self.velocity.get()
    }

    /// Whether a drag is in flight.
    pub fn is_dragging(&self) -> bool {
        self.from.is_some()
    }

    /// The grip pill's box in this widget's own space.
    pub fn grip_rect(&self) -> Rect {
        self.grip
    }

    /// How far the sheet is actually drawn from its resting position.
    fn travel(&self) -> f64 {
        elastic(self.offset.get())
    }

    /// End the drag and act on what it decided.
    fn release(&mut self, ctx: &mut EventCtx) {
        let offset = self.offset.get();
        let velocity = self.velocity.get();
        self.from = None;
        self.offset.set(0.0);
        self.sample.set(None);
        self.velocity.set(0.0);
        ctx.request_redraw();
        match resolve_release(
            offset,
            velocity,
            self.chrome.snap,
            self.chrome.snaps,
            self.chrome.dismiss_threshold,
        ) {
            SheetRelease::Hold => {}
            SheetRelease::Snap(index) => {
                if index != self.chrome.snap
                    && let Some(on_snap_change) = self.on_snap_change.as_mut()
                {
                    on_snap_change(ctx, index);
                }
            }
            SheetRelease::Dismiss => {
                if let Some(on_dismiss) = self.on_dismiss.as_mut() {
                    on_dismiss(ctx);
                }
            }
        }
    }
}

/// The title's style (`text-base font-semibold`), in the theme's
/// `title_medium` family.
fn title_style(theme: Option<&Theme>) -> TextStyle {
    let style = TextStyle {
        weight: FontWeight::SEMI_BOLD,
        ..TextStyle::new(style::TEXT_BASE as f32, crate::text::SHAPING_INK)
    };
    themed_style(style, ThemeTextType::TitleMedium, theme)
}

/// The description's style (`text-sm`), in the theme's `body_medium` family.
fn description_style(theme: Option<&Theme>) -> TextStyle {
    let style = TextStyle::new(style::TEXT_SM as f32, crate::text::SHAPING_INK);
    themed_style(style, ThemeTextType::BodyMedium, theme)
}

impl<State: 'static> SheetPanelView<State> {
    fn build_widget(&self, ctx: &mut BuildCtx<'_>) -> SheetPanelWidget {
        let chrome = self.chrome.borrow().clone();
        let hooks = self.hooks.borrow();
        SheetPanelWidget {
            content: build_child(&self.content, ctx),
            title: chrome.title.clone().map(LabelRun::new),
            description: chrome.description.clone().map(LabelRun::new),
            chrome,
            grip_row: Rect::ZERO,
            grip: Rect::ZERO,
            offset: Cell::new(0.0),
            from: None,
            sample: Cell::new(None),
            velocity: Cell::new(0.0),
            on_snap_change: hooks.0.as_ref().map(erase_callback_arg),
            on_dismiss: hooks.1.as_ref().map(erase_callback),
        }
    }
}

impl Widget for SheetPanelWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = bc.max();
        let title_style = title_style(Theme::from_layout_ctx(ctx));
        let description_style = description_style(Theme::from_layout_ctx(ctx));
        let mut y = SHEET_HEADER_TOP;
        let row_height = SHEET_HANDLE_HEIGHT + SHEET_HANDLE_PAD * 2.0;
        self.grip_row = Rect::new(0.0, y, size.width, y + row_height);
        self.grip = Rect::new(
            (size.width - SHEET_HANDLE_WIDTH) / 2.0,
            y + SHEET_HANDLE_PAD,
            (size.width + SHEET_HANDLE_WIDTH) / 2.0,
            y + SHEET_HANDLE_PAD + SHEET_HANDLE_HEIGHT,
        );
        y += row_height;

        if self.title.is_some() || self.description.is_some() {
            y += SHEET_TITLE_GAP;
            if let Some(title) = &mut self.title {
                y += title.layout(ctx, &title_style).height;
            }
            if let Some(description) = &mut self.description {
                y += SHEET_DESCRIPTION_GAP + description.layout(ctx, &description_style).height;
            }
        }
        y += SHEET_HEADER_BOTTOM;

        let body = Size::new(
            (size.width - SHEET_PADDING_X * 2.0).max(0.0),
            (size.height - y - SHEET_BODY_BOTTOM).max(0.0),
        );
        self.content.layout_child(ctx, &BoxConstraints::loose(body));
        self.content.set_origin(Point::new(SHEET_PADDING_X, y));
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let chrome = resolve_panel(theme);
        let surface = match theme {
            Some(theme) => theme.scheme().surface,
            None => crate::BEUI_LIGHT.background,
        };
        // The velocity estimate: sample the pointer's own travel against the
        // frame clock, which is the only clock a widget has (see the module
        // docs).
        let now = ctx.frame_time();
        if self.from.is_some() {
            let offset = self.offset.get();
            if let Some((last, then)) = self.sample.get() {
                let dt = now.saturating_sub(then).as_secs_f64();
                if dt > 0.0 {
                    self.velocity.set((offset - last) / dt);
                }
            }
            self.sample.set(Some((offset, now)));
            ctx.request_frame();
        }

        let travel = Vec2::new(0.0, self.travel());
        let dragging = travel.y != 0.0;
        if dragging {
            scene.push_transform(Affine::translate(travel));
        }
        let size = ctx.size();
        let at = ctx.origin();
        // `rounded-t-3xl`: only the two corners that are not against the window
        // edge are rounded.
        let radii = CornerRadii::new(SHEET_RADIUS, SHEET_RADIUS, 0.0, 0.0);
        scene.draw_shadow(
            Point::new(at.x, at.y + PANEL_SHADOW_Y_OFFSET),
            size,
            SHEET_RADIUS,
            PANEL_SHADOW_BLUR,
            style::with_alpha(Color::BLACK, PANEL_SHADOW_ALPHA),
        );
        scene.fill_rounded_rect_radii(at, size, radii, surface);
        paint_panel_hairline(scene, at, size, SHEET_RADIUS, chrome.border);

        scene.fill_rounded_rect(
            at + self.grip.origin().to_vec2(),
            self.grip.size(),
            SHEET_HANDLE_HEIGHT / 2.0,
            style::with_alpha(chrome.dim_ink, SHEET_HANDLE_ALPHA),
        );

        let mut y = self.grip_row.y1 + SHEET_TITLE_GAP;
        if let Some(title) = &self.title {
            title.paint(at + Vec2::new(SHEET_PADDING_X, y), chrome.ink, scene);
            y += title.size().height + SHEET_DESCRIPTION_GAP;
        }
        if let Some(description) = &self.description {
            description.paint(at + Vec2::new(SHEET_PADDING_X, y), chrome.dim_ink, scene);
        }

        self.content.paint_child(ctx, scene);
        if dragging {
            scene.pop_transform();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // The grip owns its own gesture; everything else is the content's.
        if let InputEvent::Pointer(p) = event {
            match p.phase {
                PointerPhase::Down
                    if presses(p)
                        && self.grip_row.contains(p.position)
                        && inside(p.position, ctx.size()) =>
                {
                    self.from = Some(p.position);
                    self.offset.set(0.0);
                    self.velocity.set(0.0);
                    self.sample.set(None);
                    ctx.capture_pointer();
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                PointerPhase::Move => {
                    // A drag in flight has a start point; otherwise this
                    // falls through to the content below, same as any other
                    // move.
                    if let Some(from) = self.from {
                        self.offset.set(p.position.y - from.y);
                        ctx.request_redraw();
                        return EventResult::Handled;
                    }
                }
                PointerPhase::Up if self.from.is_some() => {
                    self.release(ctx);
                    return EventResult::Handled;
                }
                PointerPhase::Cancel if self.from.is_some() => {
                    // A cancel never reaches app state: the sheet simply returns
                    // to where it was.
                    self.from = None;
                    self.offset.set(0.0);
                    self.velocity.set(0.0);
                    self.sample.set(None);
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                _ => {}
            }
        }
        route_event_single(&mut self.content, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.content.semantics_child(ctx);
    }

    visit_children!(content);
}

impl<State: 'static> View<State> for SheetPanelView<State> {
    type Element = SheetPanelWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> SheetPanelWidget {
        self.build_widget(ctx)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SheetPanelWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(&prev.content, &self.content, &mut element.content, ctx);
        let chrome = self.chrome.borrow().clone();
        if element.chrome != chrome {
            if element.chrome.title != chrome.title {
                element.title = chrome.title.clone().map(LabelRun::new);
            }
            if element.chrome.description != chrome.description {
                element.description = chrome.description.clone().map(LabelRun::new);
            }
            element.chrome = chrome;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        // Closures are not comparable; reinstalling the adapters is cheap.
        let hooks = self.hooks.borrow();
        element.on_snap_change = hooks.0.as_ref().map(erase_callback_arg);
        element.on_dismiss = hooks.1.as_ref().map(erase_callback);
        flags
    }

    fn teardown(&self, element: &mut SheetPanelWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.content, &mut element.content, ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::popover::tests::{Recorder, WINDOW, escape, ft_ms, light, pointer};
    use frust::SizedBox;
    use frust::authoring::text::TextContext;
    use frust_core::RenderRoot;
    use std::any::Any;

    #[derive(Default)]
    struct App {
        open: bool,
        opens: Vec<bool>,
        snap: usize,
        snaps: Vec<usize>,
    }

    struct Harness {
        root: RenderRoot<App, frust::StackView<App>>,
        state: App,
        tcx: TextContext,
    }

    impl Harness {
        fn new() -> Self {
            let mut h = Harness {
                root: RenderRoot::new(),
                state: App {
                    open: true,
                    ..App::default()
                },
                tcx: TextContext::new(),
            };
            h.root.set_theme(Box::new(light()));
            h.frame(0.0);
            h
        }

        fn frame(&mut self, ms: f64) {
            let mut logic = move |s: &mut App| {
                frust::stack().child(
                    bottom_sheet(SizedBox(None, None))
                        .title("Share")
                        .description("Pick a destination")
                        .snap(s.snap)
                        .open(s.open)
                        .on_open_change(|s: &mut App, open| {
                            s.open = open;
                            s.opens.push(open);
                        })
                        .on_snap_change(|s: &mut App, index| {
                            s.snap = index;
                            s.snaps.push(index);
                        }),
                )
            };
            self.root.rebuild(&mut logic, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
            self.root.paint(&mut Recorder::default(), ft_ms(ms));
        }

        fn paint_at(&mut self, ms: f64) -> Recorder {
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, ft_ms(ms));
            rec
        }

        fn event(&mut self, event: InputEvent) {
            self.root.event(&mut self.state, &event);
        }

        /// Settle the sheet open.
        fn settle(&mut self) {
            self.frame(0.0);
            self.frame(2_000.0);
        }

        /// The grip row's centre, in window space.
        fn grip(&self) -> Point {
            let height = WINDOW.height * SHEET_SNAP_POINTS[self.state.snap];
            Point::new(
                WINDOW.width / 2.0,
                WINDOW.height - height
                    + SHEET_HEADER_TOP
                    + SHEET_HANDLE_HEIGHT / 2.0
                    + SHEET_HANDLE_PAD,
            )
        }

        /// Drag from the grip by `dy`, taking `frames` painted frames over
        /// `ms_per_frame`, then release.
        fn drag(&mut self, dy: f64, frames: u32, ms_per_frame: f64) {
            let start = self.grip();
            self.event(pointer(PointerPhase::Down, start.x, start.y));
            for step in 1..=frames {
                let t = step as f64 / frames as f64;
                self.event(pointer(PointerPhase::Move, start.x, start.y + dy * t));
                self.paint_at(2_000.0 + ms_per_frame * step as f64);
            }
            self.event(pointer(PointerPhase::Up, start.x, start.y + dy));
        }
    }

    // ---- The release table ------------------------------------------------

    #[test]
    fn a_slow_short_pull_holds_where_it_was() {
        assert_eq!(
            resolve_release(20.0, 50.0, 0, 2, SHEET_DISMISS_THRESHOLD),
            SheetRelease::Hold
        );
    }

    #[test]
    fn a_long_pull_from_the_lowest_snap_dismisses() {
        assert_eq!(
            resolve_release(200.0, 100.0, 0, 2, SHEET_DISMISS_THRESHOLD),
            SheetRelease::Dismiss,
            "there is nothing lower to fall back to"
        );
    }

    #[test]
    fn a_firm_pull_from_a_higher_snap_steps_down_instead_of_dismissing() {
        assert_eq!(
            resolve_release(150.0, 100.0, 1, 2, SHEET_DISMISS_THRESHOLD),
            SheetRelease::Snap(0)
        );
    }

    #[test]
    fn a_hard_fling_dismisses_from_any_snap() {
        assert_eq!(
            resolve_release(150.0, 900.0, 1, 2, SHEET_DISMISS_THRESHOLD),
            SheetRelease::Dismiss,
            "past `SHEET_FLING_VELOCITY` nothing catches it"
        );
        assert_eq!(
            resolve_release(400.0, 100.0, 1, 2, SHEET_DISMISS_THRESHOLD),
            SheetRelease::Dismiss,
            "and neither does a pull past 1.6x the threshold"
        );
    }

    #[test]
    fn a_fast_upward_fling_raises_a_snap_and_stops_at_the_top() {
        assert_eq!(
            resolve_release(-10.0, -700.0, 0, 2, SHEET_DISMISS_THRESHOLD),
            SheetRelease::Snap(1)
        );
        assert_eq!(
            resolve_release(-10.0, -700.0, 1, 2, SHEET_DISMISS_THRESHOLD),
            SheetRelease::Snap(1),
            "the highest snap is the ceiling"
        );
    }

    #[test]
    fn a_slow_drag_past_the_step_offset_moves_one_snap_each_way() {
        assert_eq!(
            resolve_release(100.0, 0.0, 1, 2, SHEET_DISMISS_THRESHOLD),
            SheetRelease::Snap(0)
        );
        assert_eq!(
            resolve_release(-100.0, 0.0, 0, 2, SHEET_DISMISS_THRESHOLD),
            SheetRelease::Snap(1)
        );
        assert_eq!(
            resolve_release(-100.0, 0.0, 1, 2, SHEET_DISMISS_THRESHOLD),
            SheetRelease::Hold,
            "there is no snap above the last one"
        );
    }

    #[test]
    fn the_elastic_resists_an_upward_pull_far_harder_than_a_downward_one() {
        assert_eq!(elastic(100.0), 100.0 * SHEET_ELASTIC_DOWN);
        assert_eq!(elastic(-100.0), -100.0 * SHEET_ELASTIC_UP);
        assert!(elastic(-100.0).abs() < elastic(100.0));
    }

    // ---- Geometry ---------------------------------------------------------

    #[test]
    fn the_sheet_rests_at_its_snap_fraction_capped_at_the_upstream_width() {
        let config = bottom_sheet_config(SHEET_SNAP_POINTS[0]);
        assert_eq!(config.height, ModalExtent::Fraction(0.5));
        assert_eq!(config.max_width, ModalLimit::Px(SHEET_MAX_WIDTH));
        assert!(matches!(config.mount, ModalMount::Edge(ModalEdge::Bottom)));
        assert_eq!(config.scrim_fade, SHEET_GLIDE);
    }

    #[test]
    fn the_panel_paints_a_top_rounded_surface_with_a_grip() {
        let mut h = Harness::new();
        h.settle();
        let rec = h.paint_at(2_000.0);
        let theme = light();
        assert!(
            rec.rrects
                .iter()
                .any(|(_, s, r, c)| *c == theme.scheme().surface
                    && *r == SHEET_RADIUS
                    && s.height == WINDOW.height * SHEET_SNAP_POINTS[0]),
            "the sheet's surface is `rounded-t-3xl` at its snap height: {:?}",
            rec.rrects
        );
        assert!(
            rec.rrects
                .iter()
                .any(|(_, s, r, _)| s.width == SHEET_HANDLE_WIDTH
                    && s.height == SHEET_HANDLE_HEIGHT
                    && *r == SHEET_HANDLE_HEIGHT / 2.0),
            "and carries the `h-1.5 w-10 rounded-full` grip"
        );
        assert!(
            !rec.inks.is_empty(),
            "the title and description are painted"
        );
    }

    // ---- The gesture ------------------------------------------------------

    #[test]
    fn a_drag_translates_the_sheet_by_the_elastic_share_of_the_pull() {
        let mut h = Harness::new();
        h.settle();
        let start = h.grip();
        h.event(pointer(PointerPhase::Down, start.x, start.y));
        h.event(pointer(PointerPhase::Move, start.x, start.y + 100.0));
        let rec = h.paint_at(2_016.0);
        assert!(
            rec.transforms
                .iter()
                .any(|(_, dy)| (*dy - elastic(100.0)).abs() < 0.001),
            "the panel is drawn `dragElastic.bottom` of the way down: {:?}",
            rec.transforms
        );
    }

    #[test]
    fn a_long_slow_drag_dismisses_the_sheet() {
        let mut h = Harness::new();
        h.settle();
        // Twelve slow frames: far past the threshold, nowhere near a fling.
        h.drag(200.0, 12, 100.0);
        assert_eq!(h.state.opens, vec![false]);
    }

    #[test]
    fn a_short_drag_leaves_the_sheet_where_it_was() {
        let mut h = Harness::new();
        h.settle();
        h.drag(20.0, 8, 100.0);
        assert!(h.state.opens.is_empty(), "no dismissal");
        assert!(h.state.snaps.is_empty(), "and no snap change");
    }

    #[test]
    fn a_fast_upward_drag_raises_the_sheet_a_snap() {
        let mut h = Harness::new();
        h.settle();
        // Four frames at 8ms each covering 120px up: well past
        // `SHEET_RAISE_VELOCITY`.
        h.drag(-120.0, 4, 8.0);
        assert_eq!(h.state.snaps, vec![1]);
        h.frame(2_100.0);
        assert_eq!(h.state.snap, 1, "the app handed the new snap back");
    }

    #[test]
    fn a_cancelled_drag_reaches_no_callback_at_all() {
        let mut h = Harness::new();
        h.settle();
        let start = h.grip();
        h.event(pointer(PointerPhase::Down, start.x, start.y));
        h.event(pointer(PointerPhase::Move, start.x, start.y + 300.0));
        h.event(InputEvent::Pointer(frust::authoring::PointerEvent {
            phase: PointerPhase::Cancel,
            position: Point::new(start.x, start.y + 300.0),
            button: frust::authoring::PointerButton::Primary,
        }));
        assert!(h.state.opens.is_empty());
        assert!(h.state.snaps.is_empty());
    }

    #[test]
    fn a_press_off_the_grip_never_starts_a_drag() {
        let mut h = Harness::new();
        h.settle();
        let start = h.grip();
        // Well below the grip row, inside the sheet's body.
        h.event(pointer(PointerPhase::Down, start.x, start.y + 120.0));
        h.event(pointer(PointerPhase::Move, start.x, start.y + 420.0));
        h.event(pointer(PointerPhase::Up, start.x, start.y + 420.0));
        assert!(h.state.opens.is_empty(), "the body does not drag the sheet");
    }

    // ---- Host-owned dismissal ---------------------------------------------

    #[test]
    fn the_backdrop_and_escape_still_close_it() {
        let mut h = Harness::new();
        h.settle();
        h.event(pointer(PointerPhase::Down, 10.0, 10.0));
        h.event(pointer(PointerPhase::Up, 10.0, 10.0));
        assert_eq!(h.state.opens, vec![false]);

        let mut h = Harness::new();
        h.settle();
        h.event(pointer(PointerPhase::Down, 10.0, 10.0));
        h.event(escape());
        assert_eq!(h.state.opens, vec![false]);
    }

    #[test]
    fn a_sheet_is_a_pushable_modal_component() {
        fn dismissable<V: ModalContent<App>>(view: &V) -> bool {
            view.modal_dismissable()
        }
        assert!(dismissable(&bottom_sheet::<App, _>(SizedBox(None, None))));
        assert!(!dismissable(
            &bottom_sheet::<App, _>(SizedBox(None, None)).dismissable(false)
        ));
    }

    // ---- Typeface: title and description follow the live theme -------------

    use crate::text::typeface_probe::{
        assert_follows_a_live_family_swap, assert_paints_only_in_geist,
    };

    /// An open sheet with both header runs over glyph-free content.
    fn probe_view(_: &mut ()) -> frust::StackView<()> {
        frust::stack().child(
            bottom_sheet::<(), _>(SizedBox(None, None))
                .title("Share")
                .description("Pick a destination")
                .open(true),
        )
    }

    #[test]
    fn the_header_paints_in_geist_under_the_beui_theme() {
        assert_paints_only_in_geist("the sheet's header", probe_view, WINDOW);
    }

    #[test]
    fn the_header_follows_a_live_theme_family_swap() {
        assert_follows_a_live_family_swap("the sheet's header", probe_view, WINDOW);
    }
}
