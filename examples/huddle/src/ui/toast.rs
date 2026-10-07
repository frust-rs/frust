//! The overlay toast/snackbar service.
//!
//! [`ToastController`] is a cheap, cloneable, `Send + Sync` handle — the same
//! shape as `NavigatorController`: callers only *record* a request (via
//! [`show`](ToastController::show) /
//! [`show_with_action`](ToastController::show_with_action)), and the live queue
//! lives behind an [`RwSignal`] the [`ToastOverlay`] tracks and re-renders. The
//! controller is created once in [`crate::HuddleState`] and mounted into the
//! shell's reserved overlay slot (`Stack` top layer) as
//! [`toast_overlay`]; every screen reaches it through the app state, so a toast
//! floats above whatever screen is on top.
//!
//! # Auto-dismiss
//!
//! Each queued toast schedules its own removal on the background reactive
//! runtime ([`frust::spawn`] + `clean_signals::time::sleep`), writing the
//! removal back through the queue signal, which wakes the shell. The dismiss
//! delay is configurable ([`ToastController::with_dismiss_after`]) so the
//! headless suite can drive a short-lived toast deterministically.
//!
//! # Entrance animation
//!
//! Each toast card is wrapped in [`toast_entrance`], a small paint-driven
//! `View`/`Widget` built directly on `frust::authoring` (the same
//! precedent [`crate::ui::sheet`]/[`crate::ui::swipeable`] use) that slides the
//! card up and fades it in via an [`AnimationController`] — the sheet's
//! paint-driven-controller pattern, not the drawer's off-screen timer, since a
//! retained `Widget` here *does* get a per-frame `PaintCtx`/`request_frame` hook.
//! It changes neither the [`ToastController`] contract nor the auto-dismiss
//! timer below. **Exit is instant** (a dismissed toast is simply dropped from
//! the queue): a fade-out would need the overlay to retain a leaving card past
//! its removal from the signal-backed queue, which the cheap
//! `retain`-on-a-`Vec` model deliberately doesn't do.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent, LayoutCtx,
    PaintCtx, PaintScene, Point, SemanticsCtx, Size, View, Widget,
};
use frust::{
    Align, Alignment, AnimationController, AnyView, Column, Curve, EdgeInsets, Get, GetUntracked,
    Padding, RwSignal, SizedBox, Update, any, text,
};
use frust_material::filled_card;

/// The default auto-dismiss delay (Material snackbar-ish ~3s).
pub const DEFAULT_DISMISS_AFTER: Duration = Duration::from_secs(3);

/// A toast's optional action button (the "undo" affordance).
#[derive(Clone)]
pub struct ToastAction {
    /// The button label (e.g. `"Undo"`).
    pub label: String,
    /// The callback fired when the action is tapped. Captured `Send + Sync`
    /// state only (typically `Copy` signals / cloneable controller handles),
    /// mirroring the app's signal-capture pattern for state-less page-builder
    /// closures — a toast action does not receive `&mut State`.
    pub callback: Arc<dyn Fn() + Send + Sync>,
}

/// One live toast in the queue.
#[derive(Clone)]
pub struct ToastEntry {
    /// Stable id (used to dismiss the exact toast, even as others come and go).
    pub id: u64,
    /// The toast message.
    pub text: String,
    /// The optional action button.
    pub action: Option<ToastAction>,
}

struct ToastInner {
    entries: RwSignal<Vec<ToastEntry>>,
    next_id: AtomicU64,
    dismiss_after: Duration,
}

/// A cloneable handle to the app's toast queue. See the [module docs](self).
#[derive(Clone)]
pub struct ToastController {
    inner: Arc<ToastInner>,
}

impl Default for ToastController {
    fn default() -> Self {
        Self::new()
    }
}

impl ToastController {
    /// A controller with the [`DEFAULT_DISMISS_AFTER`] delay.
    pub fn new() -> Self {
        Self::with_dismiss_after(DEFAULT_DISMISS_AFTER)
    }

    /// A controller with an explicit auto-dismiss delay — the headless suite
    /// uses a short delay so a `show` → auto-dismiss round trip completes fast.
    pub fn with_dismiss_after(dismiss_after: Duration) -> Self {
        Self {
            inner: Arc::new(ToastInner {
                entries: RwSignal::new(Vec::new()),
                next_id: AtomicU64::new(1),
                dismiss_after,
            }),
        }
    }

    /// Queue a plain informational toast. Returns the new toast's id.
    pub fn show(&self, text: impl Into<String>) -> u64 {
        self.push(ToastEntry {
            id: 0,
            text: text.into(),
            action: None,
        })
    }

    /// Queue a toast with an action button (the undo affordance). `callback`
    /// runs on tap; the toast is dismissed immediately afterward. Returns the
    /// new toast's id.
    pub fn show_with_action(
        &self,
        text: impl Into<String>,
        label: impl Into<String>,
        callback: impl Fn() + Send + Sync + 'static,
    ) -> u64 {
        self.push(ToastEntry {
            id: 0,
            text: text.into(),
            action: Some(ToastAction {
                label: label.into(),
                callback: Arc::new(callback),
            }),
        })
    }

    /// Remove a toast by id (a no-op if it already auto-dismissed).
    pub fn dismiss(&self, id: u64) {
        self.inner.entries.update(|v| v.retain(|e| e.id != id));
    }

    /// The live queue, as a tracked read (drives the overlay's rebuild).
    pub fn snapshot(&self) -> Vec<ToastEntry> {
        self.inner.entries.get()
    }

    /// The live queue, read without tracking — for tests/inspection.
    pub fn snapshot_untracked(&self) -> Vec<ToastEntry> {
        self.inner.entries.get_untracked()
    }

    fn push(&self, mut entry: ToastEntry) -> u64 {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        entry.id = id;
        self.inner.entries.update(|v| v.push(entry));

        // Schedule auto-dismiss on the background runtime; the signal write
        // wakes the shell (and the headless pump loop) when it fires.
        let entries = self.inner.entries;
        let after = self.inner.dismiss_after;
        frust::spawn(async move {
            clean_signals::time::sleep(after).await;
            entries.update(|v| v.retain(|e| e.id != id));
        });

        id
    }
}

/// The overlay `Component` mounted in the shell's `Stack` top layer. Reads the
/// [`ToastController`] queue and paints each toast bottom-anchored above every
/// screen.
pub fn toast_overlay(controller: ToastController) -> impl View<crate::HuddleState> {
    frust::component(ToastOverlay { controller })
}

/// The toast overlay component (see [`toast_overlay`]).
pub struct ToastOverlay {
    controller: ToastController,
}

/// Retained overlay state: just the controller handle (all live data is the
/// controller's signal).
pub struct ToastOverlayState {
    controller: ToastController,
}

impl frust::Component for ToastOverlay {
    type State = ToastOverlayState;

    fn init(&self) -> ToastOverlayState {
        ToastOverlayState {
            controller: self.controller.clone(),
        }
    }

    fn build(&self, state: &mut ToastOverlayState) -> impl View<ToastOverlayState> {
        let entries = state.controller.snapshot(); // tracked

        // Bottom-anchored stack of toast cards. An empty queue renders an empty
        // (zero-child) column that paints nothing, so the overlay is inert when
        // there is nothing to show.
        let mut rows: Vec<AnyView<ToastOverlayState>> = Vec::new();
        for entry in entries {
            rows.push(any(toast_card(entry)));
            rows.push(any(SizedBox(None, Some(8.0))));
        }

        // `Align` at the bottom-center places the toast column above the
        // keyboard/nav area; the overlay layer itself fills the window.
        any(Align(Alignment::new(0.0, 1.0), Column(rows)))
    }
}

/// Render one toast as a `filled_card` with its text and optional action button.
fn toast_card(entry: ToastEntry) -> impl View<ToastOverlayState> {
    let id = entry.id;
    let mut row: Vec<AnyView<ToastOverlayState>> = vec![any(text(entry.text).size(14.0))];

    if let Some(action) = entry.action {
        row.push(any(SizedBox(Some(12.0), None)));
        let callback = action.callback.clone();
        row.push(any(frust::Button(
            action.label,
            move |st: &mut ToastOverlayState| {
                (callback)();
                st.controller.dismiss(id);
            },
        )));
    }

    toast_entrance(Padding(
        EdgeInsets::all(8.0),
        filled_card(Padding(EdgeInsets::symmetric(16.0, 12.0), frust::Row(row))),
    ))
}

// ---------------------------------------------------------------------------
// Entrance animation — a paint-driven slide-up + fade wrapper
// ---------------------------------------------------------------------------

/// Entrance duration (Material snackbar-ish quick slide).
const ENTRANCE: Duration = Duration::from_millis(260);
/// Distance (logical px) the card travels upward into place.
const ENTRANCE_SLIDE: f64 = 28.0;

/// Wrap `content` so it slides up [`ENTRANCE_SLIDE`] px and fades in over
/// [`ENTRANCE`] on first appearance — see the [module docs](self)' entrance note.
pub fn toast_entrance<State: 'static, V: View<State>>(content: V) -> ToastEntrance<State> {
    ToastEntrance {
        content: any(content),
    }
}

/// A declarative toast entrance wrapper. See [`toast_entrance`].
pub struct ToastEntrance<State: 'static> {
    content: AnyView<State>,
}

/// The retained widget for a [`ToastEntrance`].
pub struct ToastEntranceWidget {
    content: ChildPod,
    entrance: AnimationController,
    size: Size,
}

impl ToastEntranceWidget {
    fn new(content: ChildPod) -> Self {
        let mut entrance = AnimationController::new(ENTRANCE).with_curve(Curve::EaseOut);
        entrance.forward();
        Self {
            content,
            entrance,
            size: Size::ZERO,
        }
    }
}

/// Build a [`ChildPod`] wrapping an [`AnyView`]'s element (mirrors
/// `crate::ui::sheet`'s crate-private helper, re-derived here).
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
        .expect("toast entrance child element is a boxed AnyView widget");
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

impl<State: 'static> View<State> for ToastEntrance<State> {
    type Element = ToastEntranceWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ToastEntranceWidget {
        ToastEntranceWidget::new(build_child(&self.content, ctx))
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ToastEntranceWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        rebuild_child(&prev.content, &self.content, &mut element.content, ctx)
    }

    fn teardown(&self, element: &mut ToastEntranceWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.content, &mut element.content, ctx);
    }
}

impl Widget for ToastEntranceWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self.content.layout_child(ctx, bc);
        self.size = size;
        // Layout size is the settled size; the entrance offset is a paint-time
        // translation only (it never changes how the toast column stacks).
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let animating = self.entrance.advance(ctx.frame_time());
        let v = self.entrance.value().clamp(0.0, 1.0);
        let slide = (1.0 - v) * ENTRANCE_SLIDE;
        self.content.set_origin(Point::new(0.0, slide));

        let origin = ctx.origin();
        // Fade in with the entrance progress.
        scene.push_layer(origin, self.size, v as f32);
        self.content.paint_child(ctx, scene);
        scene.pop_layer();

        if animating {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // Transparent wrapper: forward every event so the toast's own action
        // button (the undo affordance) stays interactive.
        self.content.event_child(ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.content.semantics_child(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_core::{FrameTime, RenderRoot};
    use peniko::Color;

    /// A leaf child painting a fill_rect at its own origin — so the recording
    /// scene can read the entrance widget's live slide offset off its origin.y.
    struct Probe;
    struct ProbeW;
    impl View<()> for Probe {
        type Element = ProbeW;
        fn build(&self, _c: &mut BuildCtx<'_>) -> ProbeW {
            ProbeW
        }
        fn rebuild(&self, _p: &Self, _e: &mut ProbeW, _c: &mut BuildCtx<'_>) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    impl Widget for ProbeW {
        fn layout(&mut self, _c: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(120.0, 40.0))
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            scene.fill_rect(ctx.origin(), ctx.size(), Color::BLACK);
        }
    }

    /// Records the child's painted origin.y (the live slide) and the entrance's
    /// fade layer alpha.
    #[derive(Default)]
    struct RecScene {
        last_fill_y: f64,
        last_alpha: f32,
    }
    impl PaintScene for RecScene {
        fn fill_rect(&mut self, origin: Point, _size: Size, _color: Color) {
            self.last_fill_y = origin.y;
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn push_layer(&mut self, _origin: Point, _size: Size, alpha: f32) {
            self.last_alpha = alpha;
        }
    }

    /// The entrance slides the card up (its offset shrinks toward 0) and fades it
    /// in (layer alpha grows toward 1) over [`ENTRANCE`], then settles and stops
    /// asking for frames.
    #[test]
    fn toast_entrance_slides_up_and_fades_in_then_settles() {
        let mut root: RenderRoot<(), ToastEntrance<()>> = RenderRoot::new();
        let mut state = ();
        let mut logic = |_s: &mut ()| toast_entrance(Probe);
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(200.0, 200.0));

        // Seed the animation clock (first advance is a zero-delta reference).
        let mut seed = RecScene::default();
        root.paint(&mut seed, FrameTime::ZERO);

        // Mid-entrance (~80ms of 260ms): still slid down and partially faded.
        let mut mid = RecScene::default();
        let mid_out = root.paint(&mut mid, FrameTime::from_nanos(80_000_000));
        assert!(
            mid_out.needs_frame,
            "the entrance is still animating mid-flight"
        );
        assert!(
            mid.last_fill_y > 0.5,
            "the card is still slid down mid-entrance (offset {})",
            mid.last_fill_y,
        );
        assert!(
            mid.last_alpha > 0.0 && mid.last_alpha < 0.99,
            "the card is partway through its fade (alpha {})",
            mid.last_alpha,
        );

        // Past the entrance (400ms): settled at its final position, fully opaque,
        // no longer requesting frames.
        let mut done = RecScene::default();
        let done_out = root.paint(&mut done, FrameTime::from_nanos(400_000_000));
        assert!(!done_out.needs_frame, "the entrance completed");
        assert!(
            done.last_fill_y.abs() < 0.5,
            "the card settled at its final position (offset {})",
            done.last_fill_y,
        );
        assert!(
            done.last_alpha > 0.99,
            "the card is fully opaque once settled (alpha {})",
            done.last_alpha,
        );
    }
}
