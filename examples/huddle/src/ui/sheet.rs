//! `sheet` — a reusable bottom-sheet overlay (Phase D, task 20).
//!
//! A hand-rolled [`View`]/[`Widget`] pair built directly against
//! `frust-core` — the same "low-level escape hatch" precedent
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
//! (`frust`'s catalog `widgets_modals` screen is the reference). Huddle's
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
//!
//! # Keyboard avoidance (device-parity task 14)
//!
//! [`SheetWidget::layout`] rides the panel above the on-screen keyboard: it
//! reads the *raw* `WindowInsets::view_insets.bottom` off `LayoutCtx` (not the
//! derived, safe-area `padding()` `frust_widgets::safe_area` consumes —
//! that formula intentionally clamps to zero under an IME overlap, backwards
//! for a widget that must proactively avoid it) and shrinks the height
//! available to the panel by that amount, since the shell never resizes the
//! window itself under edge-to-edge (`examples/huddle/android`/`ios`'s task-08
//! wiring) — a sheet must consume the inset itself. [`avoid_keyboard`] exposes
//! the same raw-inset bottom padding as a small standalone wrapper for content
//! that isn't a [`sheet`] (`channel_feed`'s composer bar).

use std::rc::Rc;

use frust::{
    Axis, CrossAxisAlignment, EdgeInsets, FlexView, GestureDetector, IconSource, Padding, SizedBox,
    Theme, filled_card, flexible, icon, inflexible, text,
};
use frust_core::{
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
/// Unthemed-fallback scrim base color (a theme resolves this from `colors.scrim`);
/// applied at [`SCRIM_MAX_ALPHA`].
const SCRIM_COLOR: Color = Color::from_rgb8(0x00, 0x00, 0x00);

/// Resolve the panel fill from the theme (defaults to [`PANEL_FILL`] if unthemed).
fn resolve_panel_fill(theme: Option<&Theme>) -> Color {
    match theme {
        Some(theme) => theme.scheme().surface_container_low,
        None => PANEL_FILL,
    }
}

/// Resolve the grab-handle pill color from the theme (defaults to [`GRAB_PILL`]
/// if unthemed).
fn resolve_grab_pill(theme: Option<&Theme>) -> Color {
    match theme {
        Some(theme) => theme.scheme().on_surface_variant,
        None => GRAB_PILL,
    }
}

/// Resolve the scrim base color from the theme (defaults to [`SCRIM_COLOR`]
/// if unthemed), then apply alpha.
fn resolve_scrim_color(theme: Option<&Theme>, alpha: f64) -> Color {
    let base = match theme {
        Some(theme) => theme.scheme().scrim,
        None => SCRIM_COLOR,
    };
    Color::from_rgba8(
        (base.components[0] * 255.0) as u8,
        (base.components[1] * 255.0) as u8,
        (base.components[2] * 255.0) as u8,
        (alpha * 255.0) as u8,
    )
}

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
    /// The panel's natural height (grab handle + content), clamped to the
    /// height still available above the live keyboard occlusion (device-
    /// parity task 14 — see `layout`'s `available` local).
    panel_height: f64,
    /// The panel's settled top edge (`available - panel_height`, where
    /// `available = size.height - view_insets.bottom` — see `layout`).
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

        // Keyboard avoidance (device-parity task 14, item 2): the panel rides
        // above the on-screen keyboard rather than being covered by it. Reads
        // the *raw* IME occlusion (`view_insets.bottom`, not the derived
        // safe-area `padding()` a `SafeArea` widget consumes — that formula
        // intentionally clamps to zero under an IME overlap, which is the
        // wrong direction here) directly off `LayoutCtx::window_insets`, since
        // the shell never resizes the window itself under edge-to-edge (see
        // `examples/huddle/android`/`ios`'s task-08 wiring) — the sheet must
        // avoid the keyboard, not rely on a shrunk window. `available` is the
        // full-bleed height minus that occlusion; the panel (and, transitively,
        // its content) never extends into it.
        let inset_bottom = ctx.window_insets().view_insets.bottom.max(0.0);
        let available = (h - inset_bottom).max(0.0);

        self.panel_height = (GRAB_HANDLE_H + content_size.height).min(available);
        self.panel_top = (available - self.panel_height).max(0.0);
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
        let theme = Theme::from_paint_ctx(ctx);

        // Scrim: fades in with the entrance progress.
        let scrim_a = SCRIM_MAX_ALPHA * self.entrance.value().clamp(0.0, 1.0);
        scene.fill_rect(origin, size, resolve_scrim_color(theme, scrim_a));

        // Panel background (the full-width rect a test anchors on).
        let panel_o = Point::new(origin.x, origin.y + self.panel_visual_top());
        let panel_s = Size::new(size.width, self.panel_height);
        scene.fill_rounded_rect(panel_o, panel_s, PANEL_RADIUS, resolve_panel_fill(theme));

        // Grab-handle pill, centered near the panel's top.
        let pill_o = Point::new(
            panel_o.x + (size.width - GRAB_PILL_W) / 2.0,
            panel_o.y + (GRAB_HANDLE_H - GRAB_PILL_H) / 2.0,
        );
        scene.fill_rounded_rect(
            pill_o,
            Size::new(GRAB_PILL_W, GRAB_PILL_H),
            GRAB_PILL_H / 2.0,
            resolve_grab_pill(theme),
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
    // Icon default 24, label bodyLarge 16 — Material sizing
    // reference; the 16/14 row insets keep the row's touch height in the
    // 48–56 band.
    let row = FlexView::new(
        Axis::Horizontal,
        vec![
            inflexible(any(icon(leading).size(24.0))),
            inflexible(any(SizedBox(Some(16.0), None))),
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
    let mut rows: Vec<frust::FlexChild<State>> = Vec::new();
    for chunk in REACTION_EMOJI.chunks(EMOJI_COLS) {
        let mut cells: Vec<frust::FlexChild<State>> = Vec::new();
        for &emoji in chunk {
            let pick = on_pick.clone();
            // No `Align` here: under a flex row's loose cross constraint it would
            // expand to the full available height. The `flexible` slot already
            // gives the cell a tight column width; `Padding` fills it and sizes
            // its height to the glyph, so each cell spans its whole column and
            // stays tappable end-to-end.
            // press_pop adds the pressed-state scale dip (task 22
            // micro-interaction); it forwards every event so the tap still fires.
            let cell = crate::ui::swipeable::press_pop(
                GestureDetector(Padding(
                    EdgeInsets::all(EMOJI_CELL_PAD),
                    text(emoji).size(EMOJI_SIZE),
                ))
                .on_tap(move |st: &mut State| pick(st, emoji)),
            );
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

// ---------------------------------------------------------------------------
// Drag-up-to-dismiss (task 21: the workspace-switcher top drawer)
// ---------------------------------------------------------------------------

/// A vertical drag-*up*-to-dismiss wrapper — [`SheetWidget`]'s downward
/// drag-dismiss mechanics mirrored to the opposite direction, for a
/// top-anchored panel that dismisses on an upward drag instead of a downward
/// one (`screens::workspace_drawer`'s drawer, task 21). Unlike [`sheet`] this
/// wraps a natural-sized child in place — no full-bleed scrim of its own; the
/// drawer already paints a separate scrim layer alongside it in its own
/// `Stack` — and tracks the drag as an upward `lift` added to the content's
/// own paint origin. Reuses [`DISMISS_FRACTION`]/[`SETTLE_DECAY`]/
/// [`SETTLE_STOP_PX`] and the [`Callback`]/[`Erased`] plumbing [`SheetView`]
/// already defines above.
pub struct DragUpDismissView<State: 'static> {
    content: AnyView<State>,
    on_dismiss: Option<Callback<State>>,
}

/// Wrap `content` so an upward drag past [`DISMISS_FRACTION`] of its own
/// height, then released, fires `on_dismiss` (attach with
/// [`DragUpDismissView::on_dismiss`]); short of that, the content springs back
/// into place exactly like [`sheet`]'s downward release-below-threshold case.
/// Every event is otherwise forwarded through untouched, so a tap on the
/// wrapped content (e.g. a workspace row) still fires normally.
pub fn drag_up_dismiss<State: 'static, V: View<State>>(content: V) -> DragUpDismissView<State> {
    DragUpDismissView {
        content: any(content),
        on_dismiss: None,
    }
}

impl<State: 'static> DragUpDismissView<State> {
    /// Fire `on_dismiss` when the content is dragged upward past
    /// [`DISMISS_FRACTION`] of its own height and released.
    pub fn on_dismiss<F: Fn(&mut State) + 'static>(mut self, on_dismiss: F) -> Self {
        self.on_dismiss = Some(Rc::new(on_dismiss));
        self
    }
}

/// The retained widget for a [`DragUpDismissView`].
pub struct DragUpDismissWidget {
    content: ChildPod,
    /// The content's own natural (unlifted) height, measured at layout — the
    /// basis for [`DragUpDismissWidget::dismiss_px`].
    content_height: f64,
    /// Current upward drag offset (px, `>= 0`) applied to the content's paint
    /// origin.
    lift: f64,
    down_start: Point,
    last: Point,
    /// We took the gesture over as a vertical (upward) drag.
    dragging: bool,
    /// A spring-back-down settle is returning `lift` to 0 (driven at paint).
    settling: bool,
    last_anim: Option<FrameTime>,
    on_dismiss: Option<Erased>,
}

impl DragUpDismissWidget {
    fn new(content: ChildPod) -> Self {
        Self {
            content,
            content_height: 0.0,
            lift: 0.0,
            down_start: Point::ZERO,
            last: Point::ZERO,
            dragging: false,
            settling: false,
            last_anim: None,
            on_dismiss: None,
        }
    }

    fn sync_content_origin(&mut self) {
        self.content.set_origin(Point::new(0.0, -self.lift));
    }

    /// The distance an upward drag must exceed, on release, to dismiss.
    fn dismiss_px(&self) -> f64 {
        self.content_height * DISMISS_FRACTION
    }

    fn send_content_cancel(&mut self, ctx: &mut EventCtx, pos: Point) {
        let cancel = InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Cancel,
            position: pos,
            button: PointerButton::Primary,
        });
        self.content.event_child(ctx, &cancel);
    }

    /// Advance the spring-back-down settle by the delta since the last paint
    /// (mirrors [`SheetWidget::pump_settle`]).
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
            if self.lift.abs() <= SETTLE_STOP_PX {
                self.lift = 0.0;
                self.settling = false;
            } else {
                self.lift *= SETTLE_DECAY.powf(dt);
            }
        }
        if self.settling {
            ctx.request_frame();
        }
    }
}

impl<State: 'static> View<State> for DragUpDismissView<State> {
    type Element = DragUpDismissWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> DragUpDismissWidget {
        let mut widget = DragUpDismissWidget::new(build_child(&self.content, ctx));
        widget.on_dismiss = self.on_dismiss.as_ref().map(erase);
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut DragUpDismissWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_dismiss = self.on_dismiss.as_ref().map(erase);
        rebuild_child(&prev.content, &self.content, &mut element.content, ctx)
    }

    fn teardown(&self, element: &mut DragUpDismissWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.content, &mut element.content, ctx);
    }
}

impl Widget for DragUpDismissWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self.content.layout_child(ctx, bc);
        self.content_height = size.height;
        self.sync_content_origin();
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.pump_settle(ctx);
        self.sync_content_origin();
        self.content.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
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
                self.content.event_child(ctx, event);
                EventResult::Handled
            }
            PointerPhase::Move => {
                if self.dragging {
                    let dy = p.position.y - self.last.y;
                    self.last = p.position;
                    // Dragging upward (dy negative) grows the lift.
                    self.lift = (self.lift - dy).max(0.0);
                    self.sync_content_origin();
                    ctx.request_redraw();
                } else {
                    let dx = p.position.x - self.down_start.x;
                    let dy = p.position.y - self.down_start.y;
                    if dy < -TOUCH_SLOP && dy.abs() > dx.abs() {
                        // Upward takeover: cancel the content, drag the panel.
                        self.dragging = true;
                        self.last = p.position;
                        self.send_content_cancel(ctx, p.position);
                        self.content.set_active(false);
                        ctx.request_redraw();
                    } else {
                        self.content.event_child(ctx, event);
                    }
                }
                EventResult::Handled
            }
            PointerPhase::Up => {
                if self.dragging {
                    if self.lift > self.dismiss_px() {
                        if let Some(cb) = self.on_dismiss.as_mut() {
                            cb(ctx);
                        }
                    } else {
                        // Below threshold: spring the content back down.
                        self.settling = true;
                        self.last_anim = None;
                    }
                } else {
                    // A tap on the content: let it fire normally.
                    self.content.event_child(ctx, event);
                }
                self.content.set_active(false);
                self.dragging = false;
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                self.content.event_child(ctx, event);
                self.content.set_active(false);
                self.dragging = false;
                self.settling = false;
                self.last_anim = None;
                self.lift = 0.0;
                self.sync_content_origin();
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // Transparent wrapper: contribute no node of its own.
        self.content.semantics_child(ctx);
    }
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

// ---------------------------------------------------------------------------
// Standalone keyboard avoidance (device-parity task 14, item 2)
// ---------------------------------------------------------------------------

/// Pads `child`'s bottom edge by the window's *raw* live keyboard occlusion
/// (`WindowInsets::view_insets.bottom`) — the same inset [`SheetWidget::layout`]
/// consumes for its own panel (see the [module docs](self)'s Keyboard
/// avoidance section), exposed standalone for bottom-anchored content that
/// isn't a [`sheet`] overlay (`channel_feed`'s composer bar). Unlike
/// `frust_widgets::safe_area`, this reads the *raw* inset, not the derived
/// safe-area `padding()` (which intentionally clamps to zero under an IME
/// overlap — the wrong direction for proactive keyboard avoidance).
pub struct KeyboardAvoidView<State: 'static> {
    child: AnyView<State>,
}

/// Wrap `child` so its bottom edge tracks the window's live keyboard
/// occlusion. See the [`KeyboardAvoidView`] docs.
pub fn avoid_keyboard<State: 'static, V: View<State>>(child: V) -> KeyboardAvoidView<State> {
    KeyboardAvoidView { child: any(child) }
}

/// The retained widget for a [`KeyboardAvoidView`].
pub struct KeyboardAvoidWidget {
    child: ChildPod,
}

impl<State: 'static> View<State> for KeyboardAvoidView<State> {
    type Element = KeyboardAvoidWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> KeyboardAvoidWidget {
        KeyboardAvoidWidget {
            child: build_child(&self.child, ctx),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut KeyboardAvoidWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        rebuild_child(&prev.child, &self.child, &mut element.child, ctx)
    }

    fn teardown(&self, element: &mut KeyboardAvoidWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for KeyboardAvoidWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // Dynamic, like `SheetWidget::layout` above: the inset arrives via
        // `ctx` fresh every pass rather than being a view-declared constant
        // (mirrors `frust_widgets::safe_area`'s rationale for not wrapping
        // a `Padding`).
        let bottom = ctx.window_insets().view_insets.bottom.max(0.0);
        let child_bc = BoxConstraints::new(
            Size::new(bc.min().width, (bc.min().height - bottom).max(0.0)),
            Size::new(bc.max().width, (bc.max().height - bottom).max(0.0)),
        );
        let child_size = self.child.layout_child(ctx, &child_bc);
        self.child.set_origin(Point::new(0.0, 0.0));
        bc.constrain(Size::new(child_size.width, child_size.height + bottom))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.child.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        self.child.event_child(ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // Transparent inset wrapper, mirroring `SafeArea`: forward to the child.
        self.child.semantics_child(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_core::{BuildCtx, LayoutCtx, PaintCtx, RenderRoot, WindowEdgeInsets, WindowInsets};
    use kurbo::Size;
    use std::any::Any;

    /// A recording paint scene that captures filled rects for color inspection.
    #[derive(Default)]
    struct ColorRecorder {
        rects: Vec<(Point, Size, Color)>,
        rrects: Vec<(Point, Size, f64, Color)>,
    }

    impl PaintScene for ColorRecorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.rects.push((o, s, c));
        }

        fn draw_text(&mut self, _o: Point, _t: &str) {}

        fn fill_rounded_rect(&mut self, o: Point, s: Size, r: f64, c: Color) {
            self.rrects.push((o, s, r, c));
        }

        fn fill_path(&mut self, _origin: Point, _path: &kurbo::BezPath, _brush: &peniko::Brush) {}
    }

    #[test]
    fn unthemed_paint_uses_fallback_colors() {
        let view: SheetView<()> = sheet(SizedBox(Some(300.0), Some(100.0)));
        let mut counter = 0u64;
        let mut widget = view.build(&mut BuildCtx::new(&mut counter));

        let mut lctx = LayoutCtx::new();
        widget.layout(&mut lctx, &BoxConstraints::tight(Size::new(400.0, 600.0)));

        let mut rec = ColorRecorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(400.0, 600.0));
        widget.paint(&mut pctx, &mut rec);

        // Scrim should be painted at the full area (alpha may be 0 if the entrance
        // hasn't animated yet, which is fine).
        assert!(!rec.rects.is_empty(), "scrim rect is painted");
        let scrim = &rec.rects[0];
        assert_eq!(scrim.1, Size::new(400.0, 600.0), "scrim fills the area");
        // Scrim color should be black with the applied alpha (even if 0 at entrance start).
        let scrim_r = (scrim.2.components[0] * 255.0) as u8;
        let scrim_g = (scrim.2.components[1] * 255.0) as u8;
        let scrim_b = (scrim.2.components[2] * 255.0) as u8;
        assert_eq!(
            (scrim_r, scrim_g, scrim_b),
            (0, 0, 0),
            "scrim uses fallback black"
        );

        // Panel should be the fallback light color (PANEL_FILL).
        let panel = rec
            .rrects
            .iter()
            .find(|(_, s, _, _)| s.width == 400.0)
            .expect("panel is painted as a rounded rect");
        assert_eq!(panel.3, PANEL_FILL, "unthemed panel uses fallback color");

        // Grab pill should be the fallback light gray.
        let pill = rec
            .rrects
            .iter()
            .find(|(_, s, _, _)| s.width == GRAB_PILL_W)
            .expect("grab pill is painted");
        assert_eq!(pill.3, GRAB_PILL, "unthemed grab pill uses fallback color");
    }

    #[test]
    fn dark_theme_paint_resolves_surface_colors() {
        let view: SheetView<()> = sheet(SizedBox(Some(300.0), Some(100.0)));
        let mut counter = 0u64;
        let mut widget = view.build(&mut BuildCtx::new(&mut counter));

        let mut lctx = LayoutCtx::new();
        widget.layout(&mut lctx, &BoxConstraints::tight(Size::new(400.0, 600.0)));

        // Build a dark theme (M3 baseline is light; we'd need a dark variant, but
        // for now we verify that a theme is applied by checking the colors differ).
        let dark_theme = Theme::m3_baseline();
        let scheme = dark_theme.scheme();

        let mut rec = ColorRecorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(400.0, 600.0)).with_theme(&dark_theme);
        widget.paint(&mut pctx, &mut rec);

        // Panel should be theme-resolved (surface_container_low).
        let panel = rec
            .rrects
            .iter()
            .find(|(_, s, _, _)| s.width == 400.0)
            .expect("panel is painted as a rounded rect");
        assert_eq!(
            panel.3, scheme.surface_container_low,
            "themed panel uses surface_container_low"
        );

        // Grab pill should be theme-resolved (on_surface_variant).
        let pill = rec
            .rrects
            .iter()
            .find(|(_, s, _, _)| s.width == GRAB_PILL_W)
            .expect("grab pill is painted");
        assert_eq!(
            pill.3, scheme.on_surface_variant,
            "themed grab pill uses on_surface_variant"
        );

        // Verify these differ from the fallback constants.
        assert_ne!(
            scheme.surface_container_low, PANEL_FILL,
            "theme surface differs from fallback"
        );
    }

    fn sheet_widget(root: &RenderRoot<(), SheetView<()>>) -> &SheetWidget {
        let id = root.root_id().expect("root built");
        (root.tree().pod(id).expect("root pod").widget() as &dyn Any)
            .downcast_ref::<SheetWidget>()
            .expect("root is a SheetWidget")
    }

    /// A pushed IME inset (device-parity task 14, item 2) lifts the panel's
    /// settled top edge above the simulated keyboard — driven via
    /// `RenderRoot::set_insets` (mirrors `frust-widgets::safe_area`'s test
    /// harness pattern).
    #[test]
    fn ime_inset_lifts_the_panel_above_the_keyboard() {
        fn logic(_: &mut ()) -> SheetView<()> {
            sheet(SizedBox(Some(300.0), Some(100.0)))
        }
        let mut root: RenderRoot<(), SheetView<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);

        // No insets: the panel settles flush with the window bottom.
        root.layout(Size::new(400.0, 600.0));
        let w = sheet_widget(&root);
        let flush_panel_height = w.panel_height;
        assert_eq!(
            w.panel_top + w.panel_height,
            600.0,
            "with no keyboard the panel's bottom edge sits at the window bottom"
        );

        // A 340px keyboard occlusion: the panel's bottom edge rises by exactly
        // that amount, well clear of the simulated IME.
        root.set_insets(WindowInsets::new(
            WindowEdgeInsets::ZERO,
            WindowEdgeInsets::new(0.0, 0.0, 0.0, 340.0),
        ));
        root.layout(Size::new(400.0, 600.0));
        let w = sheet_widget(&root);
        assert_eq!(
            w.panel_height, flush_panel_height,
            "short content is unaffected by the clamp"
        );
        assert_eq!(
            w.panel_top + w.panel_height,
            600.0 - 340.0,
            "the panel's bottom edge rises to clear the keyboard exactly"
        );
    }

    /// Tall content clamps the panel to the height still available above the
    /// keyboard, rather than extending into (or past) it.
    #[test]
    fn ime_inset_clamps_panel_height_for_tall_content() {
        fn logic(_: &mut ()) -> SheetView<()> {
            sheet(SizedBox(Some(300.0), Some(500.0)))
        }
        let mut root: RenderRoot<(), SheetView<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        root.set_insets(WindowInsets::new(
            WindowEdgeInsets::ZERO,
            WindowEdgeInsets::new(0.0, 0.0, 0.0, 340.0),
        ));
        root.layout(Size::new(400.0, 600.0));
        let w = sheet_widget(&root);
        let available = 600.0 - 340.0;
        assert_eq!(
            w.panel_height, available,
            "the panel clamps to the height available above the keyboard"
        );
        assert_eq!(w.panel_top, 0.0, "a clamped panel starts at the very top");
    }

    #[test]
    fn avoid_keyboard_pads_the_bottom_by_the_raw_view_insets() {
        fn logic(_: &mut ()) -> KeyboardAvoidView<()> {
            avoid_keyboard(SizedBox(Some(100.0), Some(40.0)))
        }
        let mut root: RenderRoot<(), KeyboardAvoidView<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);

        // No keyboard: no added padding.
        let size = root.layout(Size::new(200.0, 200.0));
        assert_eq!(size, Size::new(100.0, 40.0));

        // A 120px keyboard adds exactly that much to the reported height —
        // *raw* `view_insets`, unlike `safe_area`'s derived `padding()` (which
        // would clamp toward zero here since there is no system-bar
        // `view_padding` to subtract against).
        root.set_insets(WindowInsets::new(
            WindowEdgeInsets::ZERO,
            WindowEdgeInsets::new(0.0, 0.0, 0.0, 120.0),
        ));
        let size = root.layout(Size::new(200.0, 200.0));
        assert_eq!(size, Size::new(100.0, 160.0));
    }
}
