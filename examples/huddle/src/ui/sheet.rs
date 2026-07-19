//! `sheet` — a reusable bottom-sheet overlay (Phase D, task 20).
//!
//! A hand-rolled [`View`]/[`Widget`] pair built directly against
//! `forgekit-core` — the same "low-level escape hatch" precedent
//! [`crate::ui::swipeable`] uses (`SwipeableRow`), rotated from the horizontal
//! swipe axis to a vertical bottom sheet. No facade widget mounts an in-screen
//! sheet with a scrim + slide-in + drag-to-dismiss that a *plain* screen
//! function (not a `Component` hosting its own `NavigatorController`) can drive,
//! so this module owns one.
//!
//! # Why an in-screen overlay, not `show_bottom_sheet`
//!
//! The facade's `show_bottom_sheet`/`show_action_sheet` helpers push a
//! transparent modal *page* over a mounted [`NavigatorController`]
//! (`forgekit`'s catalog `widgets_modals` screen is the reference). Huddle's
//! feed/thread screens are plain functions that only reach the shell's router
//! through `&mut HuddleState`, with no per-screen navigator to push over, and
//! wiring one in would churn the frozen `routes.rs`. This overlay instead mounts
//! into the screen's own `Stack` top layer: the caller stores "which sheet is
//! open" in a screen-local signal and, while it is `Some`, wraps its content in
//! `Stack(vec![screen, sheet(panel).on_dismiss(..)])`.
//!
//! # Gesture contract (mirrors `ui::swipeable`)
//!
//! Full-bleed, so it intercepts every pointer event in its region (the scrim
//! that blocks the screen behind it). A `Down` outside the panel arms a
//! scrim-dismiss that fires on `Up`. A `Down` on the panel forwards to the
//! content so a row/emoji cell stays tappable; once a downward drag passes
//! [`TOUCH_SLOP`] it sends the content a synthetic `Cancel`, takes the gesture
//! over, and on release past [`DISMISS_FRACTION`] of the panel height fires
//! `on_dismiss` (otherwise the panel springs back open). A `Cancel` clears every
//! flag and snaps back without firing — the Cancel-clears-flags-only contract
//! (`docs/CODE_STANDARDS.md`). The entrance is a paint-driven
//! [`AnimationController`] slide-up (the `home.rs` `Shimmer` / `workspace_drawer`
//! `entrance_progress` pattern).
//!
//! # Content helpers
//!
//! [`sheet_action_row`] builds one full-width tappable row (the long-press menu
//! and attachment-sheet rows), and [`emoji_grid`] the curated color-emoji
//! reaction picker (real color glyphs, verified in wave A). Both are generic
//! over the app state so the feed and thread screens (and task 21's
//! create-channel) share them.

use std::rc::Rc;

use forgekit::{
    Axis, CrossAxisAlignment, EdgeInsets, FlexView, GestureDetector, IconSource, Padding, SizedBox,
    filled_card, flexible, icon, inflexible, text,
};
use forgekit_core::{
    AnimationController, AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, Curve, EventCtx,
    EventResult, FrameTime, InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerButton,
    PointerEvent, PointerPhase, SemanticsCtx, TOUCH_SLOP, View, Widget, any,
};
use kurbo::{Point, Size};
use peniko::Color;
use std::time::Duration;

/// Height (logical px) of the drag-handle region reserved at the top of the
/// panel, above the content. Public so a test can compute the content's top
/// edge from the panel-background rect the sheet paints.
pub const GRAB_HANDLE_H: f64 = 22.0;

/// Columns in the [`emoji_grid`] reaction picker. Public so a test can compute a
/// cell's center from the panel width.
pub const EMOJI_COLS: usize = 8;

/// The curated reaction-emoji set — real color glyphs, sized [`EMOJI_SIZE`] in
/// the picker grid. 24 emoji (a multiple of [`EMOJI_COLS`], so the grid is full
/// rows).
pub const REACTION_EMOJI: [&str; 24] = [
    "\u{1F44D}",        // 👍
    "\u{2764}\u{FE0F}", // ❤️
    "\u{1F602}",        // 😂
    "\u{1F389}",        // 🎉
    "\u{1F62E}",        // 😮
    "\u{1F622}",        // 😢
    "\u{1F525}",        // 🔥
    "\u{1F440}",        // 👀
    "\u{2705}",         // ✅
    "\u{1F64F}",        // 🙏
    "\u{1F4AF}",        // 💯
    "\u{1F680}",        // 🚀
    "\u{1F60D}",        // 😍
    "\u{1F914}",        // 🤔
    "\u{1F44F}",        // 👏
    "\u{1F605}",        // 😅
    "\u{1F64C}",        // 🙌
    "\u{1F4A1}",        // 💡
    "\u{2728}",         // ✨
    "\u{1F60E}",        // 😎
    "\u{1F973}",        // 🥳
    "\u{1F44C}",        // 👌
    "\u{1F4AA}",        // 💪
    "\u{1F91D}",        // 🤝
];

/// Font size (logical px) of an emoji cell in the picker grid.
const EMOJI_SIZE: f32 = 28.0;
/// Padding around each emoji cell (drives the cell's tappable height).
const EMOJI_CELL_PAD: f64 = 10.0;

/// Horizontal inset of a [`sheet_action_row`] from the panel edge — keeps the
/// row's filled card narrower than the full-width panel background so a test can
/// tell them apart.
const ROW_H_PAD: f64 = 16.0;
/// Vertical gap above/below a row's card.
const ROW_V_PAD: f64 = 4.0;

/// Panel corner radius / background fill.
const PANEL_RADIUS: f64 = 18.0;
const PANEL_FILL: Color = Color::from_rgb8(0xF4, 0xF3, 0xF7);
/// The grab-handle pill.
const GRAB_PILL_W: f64 = 40.0;
const GRAB_PILL_H: f64 = 4.0;
const GRAB_PILL: Color = Color::from_rgb8(0xC2, 0xC0, 0xCC);
/// Scrim fill at full open (alpha scales with the entrance progress).
const SCRIM_MAX_ALPHA: f64 = 0.45;

/// Entrance slide-up duration.
const ENTRANCE: Duration = Duration::from_millis(240);
/// Fraction of the panel height a downward drag must pass, on release, to
/// dismiss the sheet (below it the panel springs back open).
const DISMISS_FRACTION: f64 = 0.30;
/// Per-millisecond retain factor for the spring-back-open settle (mirrors
/// `ui::swipeable`'s `SETTLE_DECAY`). **Community-approximate.**
const SETTLE_DECAY: f64 = 0.988;
/// Distance (logical px) below which the settle snaps exactly to 0 and stops.
const SETTLE_STOP_PX: f64 = 0.5;

/// A view-held dismiss callback (erased on build).
type Callback<State> = Rc<dyn Fn(&mut State)>;
/// The erased shape the widget invokes on dismiss (mirrors `ui::swipeable`).
type Erased = Box<dyn FnMut(&mut EventCtx)>;

/// A declarative bottom sheet. See the [module docs](self).
pub struct SheetView<State: 'static> {
    content: AnyView<State>,
    on_dismiss: Option<Callback<State>>,
}

/// Wrap `content` in a bottom-sheet overlay (no dismiss handler until one is
/// attached with [`SheetView::on_dismiss`]).
pub fn sheet<State: 'static, V: View<State>>(content: V) -> SheetView<State> {
    SheetView {
        content: any(content),
        on_dismiss: None,
    }
}

impl<State: 'static> SheetView<State> {
    /// Fire `on_dismiss` when the sheet is dismissed by a scrim tap or a
    /// drag-down past [`DISMISS_FRACTION`] — the caller closes the sheet from it
    /// (typically by clearing its screen-local sheet signal).
    pub fn on_dismiss<F: Fn(&mut State) + 'static>(mut self, on_dismiss: F) -> Self {
        self.on_dismiss = Some(Rc::new(on_dismiss));
        self
    }
}

fn erase<State: 'static>(cb: &Callback<State>) -> Erased {
    let cb = cb.clone();
    Box::new(move |ctx: &mut EventCtx| {
        let state = ctx.state_mut::<State>();
        cb(state);
    })
}

fn build_child<State: 'static>(view: &AnyView<State>, ctx: &mut BuildCtx<'_>) -> ChildPod {
    let element: Box<dyn Widget> = view.build(ctx);
    ChildPod::new(Box::new(element))
}

fn rebuild_child<State: 'static>(
    prev: &AnyView<State>,
    next: &AnyView<State>,
    pod: &mut ChildPod,
    ctx: &mut BuildCtx<'_>,
) -> ChangeFlags {
    let element = pod
        .widget_mut()
        .downcast_mut::<Box<dyn Widget>>()
        .expect("sheet content element is a boxed AnyView widget");
    next.rebuild(prev, element, ctx)
}

fn teardown_child<State: 'static>(
    view: &AnyView<State>,
    pod: &mut ChildPod,
    ctx: &mut BuildCtx<'_>,
) {
    if let Some(element) = pod.widget_mut().downcast_mut::<Box<dyn Widget>>() {
        view.teardown(element, ctx);
    }
}

/// The retained widget for a [`SheetView`].
pub struct SheetWidget {
    content: ChildPod,
    /// This widget's own resolved (full-bleed) size.
    size: Size,
    /// The panel's natural height (grab handle + content), <= `size.height`.
    panel_height: f64,
    /// The panel's settled top edge (`size.height - panel_height`).
    panel_top: f64,
    /// The entrance slide-up controller (value 0→1, offset = `(1-v)*height`).
    entrance: AnimationController,
    /// Current entrance offset (downward px), refreshed each paint.
    slide: f64,
    /// A downward drag offset (px) added to `slide` while dragging/settling.
    drag_offset: f64,
    /// A `Down` landed on the panel (drag or content-tap still undecided).
    panel_down: bool,
    /// A `Down` landed on the scrim (dismiss on `Up` unless it became a drag).
    scrim_down: bool,
    /// We took the gesture over as a vertical drag.
    dragging: bool,
    down_start: Point,
    last: Point,
    /// A spring-back-open settle is returning `drag_offset` to 0 (driven at paint).
    settling: bool,
    last_anim: Option<FrameTime>,
    on_dismiss: Option<Erased>,
}

impl SheetWidget {
    fn new(content: ChildPod) -> Self {
        let mut entrance = AnimationController::new(ENTRANCE).with_curve(Curve::EaseOut);
        entrance.forward();
        Self {
            content,
            size: Size::ZERO,
            panel_height: 0.0,
            panel_top: 0.0,
            entrance,
            slide: 0.0,
            drag_offset: 0.0,
            panel_down: false,
            scrim_down: false,
            dragging: false,
            down_start: Point::ZERO,
            last: Point::ZERO,
            settling: false,
            last_anim: None,
            on_dismiss: None,
        }
    }

    /// The content pod's current origin (below the grab handle, shifted by the
    /// live entrance + drag offset).
    fn content_origin(&self) -> Point {
        Point::new(
            0.0,
            self.panel_top + GRAB_HANDLE_H + self.slide + self.drag_offset,
        )
    }

    fn sync_content_origin(&mut self) {
        let o = self.content_origin();
        self.content.set_origin(o);
    }

    /// The panel's live visual top edge (settled top + entrance + drag offset).
    fn panel_visual_top(&self) -> f64 {
        self.panel_top + self.slide + self.drag_offset
    }

    /// The distance a release must exceed to dismiss.
    fn dismiss_px(&self) -> f64 {
        self.panel_height * DISMISS_FRACTION
    }

    fn send_content_cancel(&mut self, ctx: &mut EventCtx, pos: Point) {
        let cancel = InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Cancel,
            position: pos,
            button: PointerButton::Primary,
        });
        self.content.event_child(ctx, &cancel);
    }

    /// Advance the spring-back-open settle by the delta since the last paint.
    fn pump_settle(&mut self, ctx: &mut PaintCtx) {
        if !self.settling {
            self.last_anim = None;
            return;
        }
        let now = ctx.frame_time();
        let dt = match self.last_anim {
            Some(t) => now.saturating_sub(t).as_secs_f64() * 1000.0,
            None => 0.0,
        };
        self.last_anim = Some(now);
        if dt > 0.0 {
            if self.drag_offset.abs() <= SETTLE_STOP_PX {
                self.drag_offset = 0.0;
                self.settling = false;
            } else {
                self.drag_offset *= SETTLE_DECAY.powf(dt);
            }
        }
        if self.settling {
            ctx.request_frame();
        }
    }
}

impl<State: 'static> View<State> for SheetView<State> {
    type Element = SheetWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> SheetWidget {
        let mut widget = SheetWidget::new(build_child(&self.content, ctx));
        widget.on_dismiss = self.on_dismiss.as_ref().map(erase);
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SheetWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_dismiss = self.on_dismiss.as_ref().map(erase);
        rebuild_child(&prev.content, &self.content, &mut element.content, ctx)
    }

    fn teardown(&self, element: &mut SheetWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.content, &mut element.content, ctx);
    }
}

impl Widget for SheetWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let w = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        let h = if bc.max().height.is_finite() {
            bc.max().height
        } else {
            0.0
        };
        self.size = Size::new(w, h);

        // The content lays out at the full panel width, natural height.
        let content_bc = BoxConstraints::new(Size::new(w, 0.0), Size::new(w, f64::INFINITY));
        let content_size = self.content.layout_child(ctx, &content_bc);

        self.panel_height = (GRAB_HANDLE_H + content_size.height).min(h);
        self.panel_top = (h - self.panel_height).max(0.0);
        self.sync_content_origin();
        bc.constrain(self.size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let animating = self.entrance.advance(ctx.frame_time());
        self.slide = (1.0 - self.entrance.value()).clamp(0.0, 1.0) * self.panel_height;
        self.pump_settle(ctx);
        self.sync_content_origin();

        let origin = ctx.origin();
        let size = ctx.size();

        // Scrim: fades in with the entrance progress.
        let scrim_a = (SCRIM_MAX_ALPHA * self.entrance.value().clamp(0.0, 1.0) * 255.0) as u8;
        scene.fill_rect(origin, size, Color::from_rgba8(0, 0, 0, scrim_a));

        // Panel background (the full-width rect a test anchors on).
        let panel_o = Point::new(origin.x, origin.y + self.panel_visual_top());
        let panel_s = Size::new(size.width, self.panel_height);
        scene.fill_rounded_rect(panel_o, panel_s, PANEL_RADIUS, PANEL_FILL);

        // Grab-handle pill, centered near the panel's top.
        let pill_o = Point::new(
            panel_o.x + (size.width - GRAB_PILL_W) / 2.0,
            panel_o.y + (GRAB_HANDLE_H - GRAB_PILL_H) / 2.0,
        );
        scene.fill_rounded_rect(
            pill_o,
            Size::new(GRAB_PILL_W, GRAB_PILL_H),
            GRAB_PILL_H / 2.0,
            GRAB_PILL,
        );

        self.content.paint_child(ctx, scene);

        if animating || self.settling {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            // Key/Ime/scroll: forward to the content if it holds a recorded path.
            return self.content.event_child(ctx, event);
        };
        match p.phase {
            PointerPhase::Down => {
                self.dragging = false;
                self.settling = false;
                self.last_anim = None;
                self.down_start = p.position;
                self.last = p.position;
                ctx.capture_pointer();
                if p.position.y >= self.panel_visual_top() {
                    self.panel_down = true;
                    self.scrim_down = false;
                    self.content.event_child(ctx, event);
                } else {
                    self.scrim_down = true;
                    self.panel_down = false;
                }
                EventResult::Handled
            }
            PointerPhase::Move => {
                if self.dragging {
                    let dy = p.position.y - self.last.y;
                    self.last = p.position;
                    self.drag_offset = (self.drag_offset + dy).max(0.0);
                    self.sync_content_origin();
                    ctx.request_redraw();
                } else if self.panel_down {
                    let dx = p.position.x - self.down_start.x;
                    let dy = p.position.y - self.down_start.y;
                    if dy > TOUCH_SLOP && dy.abs() > dx.abs() {
                        // Downward takeover: cancel the content, drag the panel.
                        self.dragging = true;
                        self.last = p.position;
                        self.send_content_cancel(ctx, p.position);
                        self.content.set_active(false);
                        ctx.request_redraw();
                    } else {
                        self.content.event_child(ctx, event);
                    }
                } else if self.scrim_down && (p.position - self.down_start).hypot() > TOUCH_SLOP {
                    // A drag started on the scrim is not a dismissing tap.
                    self.scrim_down = false;
                }
                EventResult::Handled
            }
            PointerPhase::Up => {
                if self.dragging {
                    if self.drag_offset > self.dismiss_px() {
                        if let Some(cb) = self.on_dismiss.as_mut() {
                            cb(ctx);
                        }
                    } else {
                        // Below threshold: spring the panel back open.
                        self.settling = true;
                        self.last_anim = None;
                    }
                } else if self.panel_down {
                    // A tap on the panel: let the content's row/cell fire.
                    self.content.event_child(ctx, event);
                } else if self.scrim_down
                    && let Some(cb) = self.on_dismiss.as_mut()
                {
                    cb(ctx);
                }
                self.content.set_active(false);
                self.dragging = false;
                self.panel_down = false;
                self.scrim_down = false;
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                self.content.event_child(ctx, event);
                self.content.set_active(false);
                self.dragging = false;
                self.panel_down = false;
                self.scrim_down = false;
                self.settling = false;
                self.last_anim = None;
                self.drag_offset = 0.0;
                self.sync_content_origin();
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // Transparent overlay container: contribute no node of its own.
        self.content.semantics_child(ctx);
    }
}

// ---------------------------------------------------------------------------
// Content helpers
// ---------------------------------------------------------------------------

/// One full-width tappable sheet row: a leading icon + label inside a
/// `filled_card`, wrapped in a [`GestureDetector`] firing `on_tap`. Used for the
/// long-press context menu and the attachment sheet. Meant to sit inside a
/// [`CrossAxisAlignment::Stretch`] column so the card fills the panel width.
pub fn sheet_action_row<State, F>(
    leading: IconSource,
    label: impl Into<String>,
    on_tap: F,
) -> AnyView<State>
where
    State: 'static,
    F: Fn(&mut State) + 'static,
{
    let row = FlexView::new(
        Axis::Horizontal,
        vec![
            inflexible(any(icon(leading).size(22.0))),
            inflexible(any(SizedBox(Some(14.0), None))),
            inflexible(any(text(label.into()).size(16.0))),
        ],
    )
    .cross_axis(CrossAxisAlignment::Center);

    any(GestureDetector(Padding(
        EdgeInsets::symmetric(ROW_H_PAD, ROW_V_PAD),
        filled_card(Padding(EdgeInsets::symmetric(16.0, 14.0), row)),
    ))
    .on_tap(on_tap))
}

/// The curated color-emoji reaction picker: an [`EMOJI_COLS`]-wide grid of
/// tappable text cells over [`REACTION_EMOJI`]. Tapping a cell calls
/// `on_pick(state, emoji)` — the caller toggles the reaction (or inserts the
/// emoji into the composer) and closes the sheet.
pub fn emoji_grid<State, F>(on_pick: F) -> AnyView<State>
where
    State: 'static,
    F: Fn(&mut State, &'static str) + Clone + 'static,
{
    let mut rows: Vec<forgekit::FlexChild<State>> = Vec::new();
    for chunk in REACTION_EMOJI.chunks(EMOJI_COLS) {
        let mut cells: Vec<forgekit::FlexChild<State>> = Vec::new();
        for &emoji in chunk {
            let pick = on_pick.clone();
            // No `Align` here: under a flex row's loose cross constraint it would
            // expand to the full available height. The `flexible` slot already
            // gives the cell a tight column width; `Padding` fills it and sizes
            // its height to the glyph, so each cell spans its whole column and
            // stays tappable end-to-end.
            let cell = GestureDetector(Padding(
                EdgeInsets::all(EMOJI_CELL_PAD),
                text(emoji).size(EMOJI_SIZE),
            ))
            .on_tap(move |st: &mut State| pick(st, emoji));
            cells.push(flexible(1, any(cell)));
        }
        // Pad a short final row so its cells keep the same column width.
        while cells.len() < EMOJI_COLS {
            cells.push(flexible(1, any(SizedBox(None, None))));
        }
        rows.push(inflexible(any(
            FlexView::new(Axis::Horizontal, cells).cross_axis(CrossAxisAlignment::Center)
        )));
    }
    any(Padding(
        EdgeInsets::symmetric(0.0, 8.0),
        FlexView::new(Axis::Vertical, rows).cross_axis(CrossAxisAlignment::Stretch),
    ))
}

/// Assemble sheet rows into the stretched column the [`sheet`] panel expects
/// (each row fills the panel width). A small convenience over building the
/// [`FlexView`] inline at every call site.
pub fn action_menu<State: 'static>(rows: Vec<AnyView<State>>) -> AnyView<State> {
    any(Padding(
        EdgeInsets::symmetric(0.0, 8.0),
        FlexView::new(
            Axis::Vertical,
            rows.into_iter().map(inflexible).collect::<Vec<_>>(),
        )
        .cross_axis(CrossAxisAlignment::Stretch),
    ))
}
