//! Ports beUI's `center-morph-modal` component —
//! `components/motion/center-morph-modal.tsx` (beUI rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01).
//!
//! | upstream | here |
//! |---|---|
//! | `CENTER_FOLDED_CLIP` `inset(48% …)` | [`CENTER_MORPH_FOLDED`] |
//! | `CENTER_OPEN_CLIP` `inset(0% …)` | the panel's own rect |
//! | `round 30px`, held constant through the unfold | [`CENTER_MORPH_RADIUS`] |
//! | `CENTER_UNFOLD_TRANSITION` `0.43s` on `[0.2, 0, 0.2, 1]` | [`CENTER_MORPH_UNFOLD`], [`CENTER_UNFOLD_EASE`] |
//! | `w-full max-w-[26rem]` inside `inset-4` | [`CENTER_MORPH_MAX_WIDTH`], [`PANEL_MARGIN`] |
//! | backdrop `bg-background/10 backdrop-blur-sm`, `0.28s` | [`MorphingBackdrop`], [`CENTER_MORPH_FROST_ALPHA`], [`CENTER_MORPH_SCRIM_FADE`] |
//! | close `absolute right-4 top-4 size-8 rounded-full bg-foreground/[0.05]` | [`CENTER_MORPH_CLOSE_SIZE`], [`CENTER_MORPH_CLOSE_INSET`], [`CENTER_MORPH_CLOSE_ALPHA`] |
//! | close entrance `delay 0.16, duration 0.2, scale 0.8` | [`CENTER_MORPH_CLOSE_DELAY`], [`CENTER_MORPH_CLOSE_ENTER`], [`CENTER_MORPH_CLOSE_SCALE`] |
//!
//! # The surface unfolds from its own centre
//!
//! This is [`crate::components::morphing_modal`]'s mechanism with a different
//! source rect. Where that one travels from a *trigger*, this one starts as a
//! 4%-of-each-axis sliver at the panel's own centre and unfolds outward — which
//! is exactly what `inset(48% 48% 48% 48%)` leaves showing. The lerp is the
//! shared [`morph_rect`](crate::components::popover::morph_rect); the radius is
//! held at [`CENTER_MORPH_RADIUS`] throughout rather than lerped, and the
//! upstream comment says why: a radius resolving on its own spends the last
//! frames rounding corners instead of unfolding.
//!
//! The unfold replaces the host's centred lift-scale-fade rather than layering
//! over it — [`center_morph_modal_config`] hands the host an instant entrance
//! and a [`CENTER_MORPH_UNFOLD`]-long exit, so the fold-back completes before
//! the pod is unmounted. Everything else (the scrim, the barrier, Escape, the
//! backdrop click, the back press, the staged pop) is
//! [`crate::overlay::modal`]'s; this component builds no host.
//!
//! # Degradations against the web original
//!
//! - **No backdrop blur.** `backdrop-blur-sm` has no `PaintScene` primitive.
//!   [`MorphingBackdrop::Frosted`] paints the `bg-background/10` wash alone,
//!   which reads as near-clear glass without it, so the dimming
//!   [`MorphingBackdrop::Scrim`] is the default here — the same call, for the
//!   same reason, as [`crate::components::morphing_modal`]'s.
//! - **No focus trap, and no focus restore.** Upstream cycles Tab inside the
//!   panel and returns focus to the trigger on close. frust has no "focus that
//!   widget" call a plugin-tier widget can aim, and no tab-order API to close a
//!   ring with; [`crate::overlay::anchored`] records the same gap for its own
//!   dismissal. The host's barrier is what keeps interaction inside the panel.
//! - **No auto-focus on open.** Upstream focuses the first focusable descendant
//!   on the frame the modal appears. There is no auto-focus-on-appear hook; the
//!   host claims focus on the first press that lands on it, which is what makes
//!   Escape reachable.
//! - **`drop-shadow-2xl` reads the clipped silhouette upstream.** The shadow
//!   here is the `.glass` chrome recipe cast from the *unfolding rect*, so it
//!   grows with the surface — the same effect, from a rounded-rect shadow rather
//!   than an alpha-derived one.
//! - **The close mark is drawn, not iconographic.** The catalog has no icon set,
//!   so lucide's `X` is two strokes at [`CENTER_MORPH_CLOSE_ICON`] with lucide's
//!   own 2px weight.
//! - **`reduce_motion` fades instead of unfolding**, which is upstream's own
//!   reduced branch: [`crate::motion::Presence::collapsed`] lands the unfold on
//!   the first frame and the host's scrim carries the arrival.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, Affine, AnyView, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod,
    ErasedCallback, EventCtx, EventResult, InputEvent, KeyEvent, LayoutCtx, PaintCtx, PaintScene,
    Point, PointerPhase, Rect, Role, SemanticsCtx, Size, View, Widget, any, build_child,
    erase_callback, rebuild_child, route_event_single, teardown_child, visit_children,
};
use frust::{Curve, FrameTime, Theme};

use crate::components::morphing_modal::MorphingBackdrop;
use crate::components::popover::{morph_rect, paint_panel, resolve_panel};
use crate::motion::{Presence, PresencePhase, Ramp};
use crate::overlay::{
    ModalConfig, ModalContent, ModalExtent, ModalLimit, ModalView, ModalWidget, PANEL_MARGIN, modal,
};
use crate::press::{inside, is_activation_key, presses};
use crate::style;

/// `max-w-[26rem]` — the panel's width cap, in logical px.
pub const CENTER_MORPH_MAX_WIDTH: f64 = 416.0;

/// `round 30px` — the panel's corner radius, held constant through the unfold.
pub const CENTER_MORPH_RADIUS: f64 = 30.0;

/// `inset(48% …)` — the share of each axis the folded surface still shows
/// (`100% - 2 * 48%`).
pub const CENTER_MORPH_FOLDED: f64 = 0.04;

/// `CENTER_UNFOLD_EASE` — the unfold's own curve.
pub const CENTER_UNFOLD_EASE: Curve = Curve::Cubic(0.2, 0.0, 0.2, 1.0);

/// `CENTER_UNFOLD_TRANSITION.duration` — how long the surface takes to unfold,
/// and to fold back.
pub const CENTER_MORPH_UNFOLD: Duration = Duration::from_millis(430);

/// How long the backdrop takes to fade, each way (`0.28`).
pub const CENTER_MORPH_SCRIM_FADE: Duration = Duration::from_millis(280);

/// `bg-background/10` — the frosted backdrop's wash alpha.
pub const CENTER_MORPH_FROST_ALPHA: f32 = 0.10;

/// `size-8` — the close control's box, in logical px.
pub const CENTER_MORPH_CLOSE_SIZE: f64 = 32.0;

/// `right-4 top-4` — how far the close control sits from the panel's trailing
/// top corner, in logical px.
pub const CENTER_MORPH_CLOSE_INSET: f64 = 16.0;

/// `h-4 w-4` — the close mark's box inside the control, in logical px.
pub const CENTER_MORPH_CLOSE_ICON: f64 = 16.0;

/// lucide's default `strokeWidth`, in logical px.
pub const CENTER_MORPH_CLOSE_STROKE: f64 = 2.0;

/// `bg-foreground/[0.05]` — the close control's rest fill alpha over the ink
/// role.
pub const CENTER_MORPH_CLOSE_ALPHA: f32 = 0.05;

/// `hover:bg-foreground/[0.08]` — its hovered fill alpha.
pub const CENTER_MORPH_CLOSE_HOVER_ALPHA: f32 = 0.08;

/// `delay: 0.16` — how long after the unfold starts the close control arrives.
pub const CENTER_MORPH_CLOSE_DELAY: Duration = Duration::from_millis(160);

/// `duration: 0.2` — how long its own entrance takes.
pub const CENTER_MORPH_CLOSE_ENTER: Duration = Duration::from_millis(200);

/// `scale: 0.8` — the scale it enters from.
pub const CENTER_MORPH_CLOSE_SCALE: f64 = 0.8;

/// The [`ModalConfig`] a centre-morph modal is hosted with: `w-full
/// max-w-[26rem]` inside `inset-4`, an instant entrance (the unfold *is* the
/// entrance) and an exit long enough for the fold-back.
pub fn center_morph_modal_config(backdrop: MorphingBackdrop) -> ModalConfig {
    ModalConfig::centered()
        .width(
            ModalExtent::Fraction(1.0),
            ModalLimit::Px(CENTER_MORPH_MAX_WIDTH),
        )
        .margin(PANEL_MARGIN)
        .scrim(backdrop == MorphingBackdrop::Scrim)
        .scrim_fade(CENTER_MORPH_SCRIM_FADE)
        .ramps(
            Ramp::eased(Duration::ZERO, CENTER_UNFOLD_EASE),
            Ramp::eased(CENTER_MORPH_UNFOLD, CENTER_UNFOLD_EASE),
        )
}

// ---- The component ---------------------------------------------------------

/// The mutable panel configuration the outer builder writes and the panel reads
/// — the handle shape [`crate::components::popover`] documents.
type PanelHandle = Rc<RefCell<CenterConfig>>;

/// The panel's own state-bearing close hook, written after the panel view
/// already exists (the host holds its content erased).
type CloseHandle<State> = Rc<RefCell<Option<Rc<dyn Fn(&mut State)>>>>;

/// What the panel needs from its component's builders.
#[derive(Clone, Debug, PartialEq)]
struct CenterConfig {
    open: bool,
    backdrop: MorphingBackdrop,
    close_button: bool,
    close_label: String,
}

/// A declarative beUI centre-morph modal. See [`center_morph_modal`].
pub struct CenterMorphModalView<State: 'static> {
    inner: ModalView<State>,
    config: PanelHandle,
    close: CloseHandle<State>,
}

/// Build a centre-morph modal over `content`, open by default and carrying its
/// own close control.
///
/// Mount it as the top child of a full-area [`frust::Stack`] with
/// [`CenterMorphModalView::open`] carrying the app's own flag, or push it as a
/// transparent navigator page with [`crate::overlay::show_modal`].
pub fn center_morph_modal<State: 'static, V: View<State>>(
    content: V,
) -> CenterMorphModalView<State> {
    let config: PanelHandle = Rc::new(RefCell::new(CenterConfig {
        open: true,
        backdrop: MorphingBackdrop::default(),
        close_button: true,
        close_label: "Close modal".to_string(),
    }));
    let close: CloseHandle<State> = Rc::new(RefCell::new(None));
    let panel = CenterPanelView {
        content: any(content),
        config: config.clone(),
        close: close.clone(),
    };
    CenterMorphModalView {
        inner: modal(
            panel,
            center_morph_modal_config(MorphingBackdrop::default()),
        ),
        config,
        close,
    }
}

impl<State: 'static> CenterMorphModalView<State> {
    /// Pick the backdrop (default [`MorphingBackdrop::Scrim`] — see the [module
    /// docs](self)).
    pub fn backdrop(mut self, backdrop: MorphingBackdrop) -> Self {
        self.config.borrow_mut().backdrop = backdrop;
        let dismissable = self.inner.config.dismissable;
        self.inner.config = center_morph_modal_config(backdrop).dismissable(dismissable);
        self
    }

    /// Draw the close control in the panel's trailing top corner (default
    /// `true`, upstream's `showCloseButton`).
    pub fn close_button(self, close_button: bool) -> Self {
        self.config.borrow_mut().close_button = close_button;
        self
    }

    /// Set the close control's accessible name (`closeButtonLabel`).
    pub fn close_label(self, label: impl Into<String>) -> Self {
        self.config.borrow_mut().close_label = label.into();
        self
    }

    /// Hand the modal the app's open flag. The default is `true`.
    pub fn open(mut self, open: bool) -> Self {
        self.config.borrow_mut().open = open;
        self.inner = self.inner.open(open);
        self
    }

    /// The accessibility label the panel is announced with (`ariaLabel`).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.inner = self.inner.label(label);
        self
    }

    /// Whether Escape, a backdrop click and a back press close it (default
    /// `true`, upstream's `dismissible`). The close control is unaffected — it
    /// is the panel's own, and upstream keeps it live either way.
    pub fn dismissable(mut self, dismissable: bool) -> Self {
        self.inner.config = self.inner.config.dismissable(dismissable);
        self
    }

    /// Set the close callback: Escape, a backdrop click, a back press, or the
    /// panel's own close control. Reports `false`.
    pub fn on_open_change<F: Fn(&mut State, bool) + 'static>(mut self, on_open_change: F) -> Self {
        let shared = Rc::new(on_open_change);
        let host = shared.clone();
        self.inner = self.inner.on_dismiss(move |state| host(state, false));
        *self.close.borrow_mut() = Some(Rc::new(move |state: &mut State| shared(state, false)));
        self
    }

    /// Set the exit-settled callback — state-free, fired from the paint that
    /// finishes the fold-back.
    pub fn on_close<F: Fn() + 'static>(mut self, on_close: F) -> Self {
        self.inner = self.inner.on_close(on_close);
        self
    }
}

impl<State: 'static> View<State> for CenterMorphModalView<State> {
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

impl<State: 'static> ModalContent<State> for CenterMorphModalView<State> {
    fn modal_dismissable(&self) -> bool {
        self.inner.config.dismissable
    }
}

// ---- The panel -------------------------------------------------------------

/// The modal's panel: the unfolding surface, the frosted backdrop when it is
/// asked for, and the close control.
struct CenterPanelView<State: 'static> {
    content: AnyView<State>,
    config: PanelHandle,
    close: CloseHandle<State>,
}

/// The retained widget for a centre-morph panel.
pub struct CenterPanelWidget {
    content: ChildPod,
    config: CenterConfig,
    /// Drives the unfold, opened and closed with the host.
    unfold: Presence,
    /// The `reduce_motion` value `unfold` was last built for.
    reduced: Option<bool>,
    /// The frame the unfold started on, for the close control's own delay.
    started: Option<FrameTime>,
    /// The close control's box in this widget's own space.
    close_rect: Rect,
    /// Whether the close control is holding a press.
    pressed: bool,
    /// The unfold's progress at the last paint.
    progress: Cell<f64>,
    on_close: Option<ErasedCallback>,
}

impl CenterPanelWidget {
    /// The unfold's progress at the last paint: `0.0` folded onto the panel's
    /// centre, `1.0` the whole panel.
    pub fn unfold_progress(&self) -> f64 {
        self.progress.get()
    }

    /// The close control's box in this widget's own space.
    pub fn close_rect(&self) -> Rect {
        self.close_rect
    }

    /// The folded source rect: `CENTER_MORPH_FOLDED` of each axis, centred.
    fn folded(panel: Rect) -> Rect {
        let w = panel.width() * CENTER_MORPH_FOLDED;
        let h = panel.height() * CENTER_MORPH_FOLDED;
        let centre = panel.center();
        Rect::new(
            centre.x - w / 2.0,
            centre.y - h / 2.0,
            centre.x + w / 2.0,
            centre.y + h / 2.0,
        )
    }

    /// How present the close control is at `elapsed` into the unfold: its own
    /// delayed 200ms ramp.
    fn close_reveal(elapsed: Duration, reduce: bool) -> f64 {
        if reduce {
            return 1.0;
        }
        let Some(since) = elapsed.checked_sub(CENTER_MORPH_CLOSE_DELAY) else {
            return 0.0;
        };
        Ramp::eased(CENTER_MORPH_CLOSE_ENTER, crate::tokens::motion::EASE_OUT)
            .progress_clamped(since)
    }

    /// The close mark: lucide's `X`, two strokes across `rect`.
    fn close_mark(rect: Rect) -> BezPath {
        let mut path = BezPath::new();
        path.move_to(Point::new(rect.x0, rect.y0));
        path.line_to(Point::new(rect.x1, rect.y1));
        path.move_to(Point::new(rect.x1, rect.y0));
        path.line_to(Point::new(rect.x0, rect.y1));
        path
    }

    /// Rebuild the unfold driver when `reduce_motion` flips, preserving what the
    /// old one was doing.
    fn sync_motion(&mut self, reduce: bool) {
        if self.reduced == Some(reduce) {
            return;
        }
        let was_exiting = self.unfold.phase() == PresencePhase::Exiting;
        self.reduced = Some(reduce);
        let base = Presence::symmetric(Ramp::eased(CENTER_MORPH_UNFOLD, CENTER_UNFOLD_EASE));
        let mut next = if reduce { base.collapsed() } else { base };
        if was_exiting && !self.config.open {
            next.set_open(true);
        }
        next.set_open(self.config.open);
        self.unfold = next;
    }

    /// Fire the close hook, if one is installed.
    fn fire_close(&mut self, ctx: &mut EventCtx) {
        if let Some(on_close) = self.on_close.as_mut() {
            on_close(ctx);
        }
    }

    /// Whether `key` activates the close control.
    fn activates(&self, key: &KeyEvent) -> bool {
        self.config.close_button && is_activation_key(key)
    }
}

impl<State: 'static> View<State> for CenterPanelView<State> {
    type Element = CenterPanelWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> CenterPanelWidget {
        let config = self.config.borrow().clone();
        let mut unfold = Presence::symmetric(Ramp::eased(CENTER_MORPH_UNFOLD, CENTER_UNFOLD_EASE));
        unfold.set_open(config.open);
        CenterPanelWidget {
            content: build_child(&self.content, ctx),
            config,
            unfold,
            reduced: None,
            started: None,
            close_rect: Rect::ZERO,
            pressed: false,
            progress: Cell::new(0.0),
            on_close: self.close.borrow().as_ref().map(erase_callback),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut CenterPanelWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(&prev.content, &self.content, &mut element.content, ctx);
        let config = self.config.borrow().clone();
        if element.config != config {
            if element.config.open != config.open {
                element.unfold.set_open(config.open);
                if config.open {
                    // A fresh open episode: the close control's delay is timed
                    // from the frame the panel next paints.
                    element.started = None;
                }
            }
            element.config = config;
            flags |= ChangeFlags::PAINT;
        }
        // Closures are not comparable; reinstalling the adapter is cheap.
        element.on_close = self.close.borrow().as_ref().map(erase_callback);
        flags
    }

    fn teardown(&self, element: &mut CenterPanelWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.content, &mut element.content, ctx);
    }
}

impl Widget for CenterPanelWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let content = self
            .content
            .layout_child(ctx, &BoxConstraints::loose(bc.max()));
        self.content.set_origin(Point::ORIGIN);
        let size = bc.constrain(content);
        let x1 = size.width - CENTER_MORPH_CLOSE_INSET;
        self.close_rect = Rect::new(
            x1 - CENTER_MORPH_CLOSE_SIZE,
            CENTER_MORPH_CLOSE_INSET,
            x1,
            CENTER_MORPH_CLOSE_INSET + CENTER_MORPH_CLOSE_SIZE,
        );
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let chrome = resolve_panel(Theme::from_paint_ctx(ctx));
        let reduce = Theme::from_paint_ctx(ctx).is_some_and(|t| t.motion.reduce_motion);
        let wash = match Theme::from_paint_ctx(ctx) {
            Some(theme) => theme.scheme().surface,
            None => crate::BEUI_LIGHT.background,
        };
        self.sync_motion(reduce);
        let now = ctx.frame_time();

        // The frosted backdrop, when it is the panel's job rather than the
        // host's — the host turned its own scrim off for it (see the seam docs).
        if self.config.backdrop == MorphingBackdrop::Frosted {
            let area = ctx
                .visible_rect()
                .unwrap_or_else(|| Rect::from_origin_size(ctx.origin(), ctx.size()));
            scene.fill_rect(
                area.origin(),
                area.size(),
                style::with_alpha(wash, CENTER_MORPH_FROST_ALPHA),
            );
        }

        let progress = self.unfold.advance(now);
        if self.unfold.is_animating() {
            ctx.request_frame();
        }
        if !self.unfold.is_visible() {
            self.progress.set(0.0);
            self.started = None;
            return;
        }
        self.progress.set(progress);
        let started = *self.started.get_or_insert(now);

        let panel = Rect::from_origin_size(Point::ORIGIN, ctx.size());
        let (rect, radius) = morph_rect(
            Self::folded(panel),
            CENTER_MORPH_RADIUS,
            panel,
            CENTER_MORPH_RADIUS,
            progress,
        );
        paint_panel(scene, ctx.origin(), rect, radius, chrome);

        // `overflow-hidden`: the content is revealed by the unfolding surface.
        scene.push_clip_rounded(ctx.origin() + rect.origin().to_vec2(), rect.size(), radius);
        self.content.paint_child(ctx, scene);

        if self.config.close_button {
            let reveal = Self::close_reveal(now.saturating_sub(started), reduce);
            if reveal > 0.0 {
                if reveal < 1.0 {
                    ctx.request_frame();
                }
                let scale = CENTER_MORPH_CLOSE_SCALE + (1.0 - CENTER_MORPH_CLOSE_SCALE) * reveal;
                let centre = ctx.origin() + self.close_rect.center().to_vec2();
                scene.push_layer(
                    ctx.origin() + self.close_rect.origin().to_vec2(),
                    self.close_rect.size(),
                    reveal as f32,
                );
                scene.push_transform(
                    Affine::translate(centre.to_vec2())
                        * Affine::scale(scale)
                        * Affine::translate(-centre.to_vec2()),
                );
                let fill_alpha = if self.pressed {
                    CENTER_MORPH_CLOSE_HOVER_ALPHA
                } else {
                    CENTER_MORPH_CLOSE_ALPHA
                };
                scene.fill_rounded_rect(
                    ctx.origin() + self.close_rect.origin().to_vec2(),
                    self.close_rect.size(),
                    CENTER_MORPH_CLOSE_SIZE / 2.0,
                    style::with_alpha(chrome.ink, fill_alpha),
                );
                let mark = Rect::from_center_size(
                    self.close_rect.center(),
                    Size::new(CENTER_MORPH_CLOSE_ICON / 2.0, CENTER_MORPH_CLOSE_ICON / 2.0),
                );
                scene.stroke_path(
                    ctx.origin(),
                    &Self::close_mark(mark),
                    CENTER_MORPH_CLOSE_STROKE,
                    &Brush::Solid(chrome.dim_ink),
                );
                scene.pop_transform();
                scene.pop_layer();
            }
        }
        scene.pop_clip();
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // The close control owns its own press; everything else is the
        // content's, which is routed first so a control inside the panel keeps
        // its own gestures (the container-claims-after-routing rule).
        if let InputEvent::Key(key) = event
            && self.activates(key)
            && ctx.has_focus()
        {
            self.fire_close(ctx);
            return EventResult::Handled;
        }
        if route_event_single(&mut self.content, ctx, event) == EventResult::Handled {
            return EventResult::Handled;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        if !self.config.close_button {
            return EventResult::Ignored;
        }
        match p.phase {
            PointerPhase::Down if presses(p) && self.close_rect.contains(p.position) => {
                self.pressed = true;
                ctx.capture_pointer();
                ctx.request_focus();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up if self.pressed => {
                self.pressed = false;
                ctx.request_redraw();
                if self.close_rect.contains(p.position) && inside(p.position, ctx.size()) {
                    self.fire_close(ctx);
                }
                EventResult::Handled
            }
            PointerPhase::Cancel if self.pressed => {
                // A cancel never reaches app state.
                self.pressed = false;
                ctx.request_redraw();
                EventResult::Handled
            }
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.content.semantics_child(ctx);
        if self.config.close_button {
            let label = self.config.close_label.clone();
            ctx.push_node(Role::Button, |node| {
                node.set_label(label.as_str());
                node.add_action(Action::Click);
            });
        }
    }

    visit_children!(content);
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
    }

    struct Harness {
        root: RenderRoot<App, frust::StackView<App>>,
        state: App,
        tcx: TextContext,
        backdrop: MorphingBackdrop,
        close_button: bool,
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
                backdrop: MorphingBackdrop::default(),
                close_button: true,
            };
            h.root.set_theme(Box::new(light()));
            h.frame(0.0);
            h
        }

        fn frame(&mut self, ms: f64) {
            let backdrop = self.backdrop;
            let close_button = self.close_button;
            let mut logic = move |s: &mut App| {
                frust::stack().child(
                    center_morph_modal(SizedBox(Some(300.0), Some(200.0)))
                        .backdrop(backdrop)
                        .close_button(close_button)
                        .open(s.open)
                        .label("Details")
                        .on_open_change(|s: &mut App, open| {
                            s.open = open;
                            s.opens.push(open);
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

        /// The panel's surface at `ms`.
        fn surface(&mut self, ms: f64) -> Option<(Point, Size, f64)> {
            let want = light().scheme().surface_container_high;
            self.paint_at(ms)
                .rrects
                .iter()
                .find(|(_, _, _, c)| *c == want)
                .map(|(o, s, r, _)| (*o, *s, *r))
        }

        /// Settle the modal fully open.
        fn settle(&mut self) {
            self.frame(0.0);
            self.frame(3_000.0);
        }

        /// The close control's centre, in window space.
        fn close_centre(&mut self) -> Point {
            let (origin, size, _) = self.surface(3_000.0).expect("the settled panel");
            Point::new(
                origin.x + size.width - CENTER_MORPH_CLOSE_INSET - CENTER_MORPH_CLOSE_SIZE / 2.0,
                origin.y + CENTER_MORPH_CLOSE_INSET + CENTER_MORPH_CLOSE_SIZE / 2.0,
            )
        }
    }

    // ---- The unfold -------------------------------------------------------

    #[test]
    fn the_folded_source_is_four_percent_of_each_axis_at_the_panels_centre() {
        let panel = Rect::new(0.0, 0.0, 400.0, 300.0);
        let folded = CenterPanelWidget::folded(panel);
        assert_eq!(folded.width(), 400.0 * CENTER_MORPH_FOLDED);
        assert_eq!(folded.height(), 300.0 * CENTER_MORPH_FOLDED);
        assert_eq!(folded.center(), panel.center());
    }

    #[test]
    fn the_first_frame_shows_only_that_sliver() {
        let mut h = Harness::new();
        let (origin, size, radius) = h.surface(0.0).expect("the folded surface");
        let (panel_origin, panel_size, _) = {
            h.frame(0.0);
            h.frame(3_000.0);
            h.surface(3_000.0).expect("the open surface")
        };
        assert!(
            (size.width - panel_size.width * CENTER_MORPH_FOLDED).abs() < 1.0,
            "folded to 4% of the panel: {size:?} vs {panel_size:?}"
        );
        let folded_centre = Point::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
        let panel_centre = Point::new(
            panel_origin.x + panel_size.width / 2.0,
            panel_origin.y + panel_size.height / 2.0,
        );
        assert!((folded_centre.x - panel_centre.x).abs() < 1.0);
        assert!((folded_centre.y - panel_centre.y).abs() < 1.0);
        assert_eq!(radius, CENTER_MORPH_RADIUS, "the radius never moves");
    }

    #[test]
    fn the_unfold_lands_on_the_panels_own_rect_capped_at_the_upstream_width() {
        let mut h = Harness::new();
        h.settle();
        let (_, size, radius) = h.surface(3_000.0).expect("the open surface");
        assert_eq!(size.width, CENTER_MORPH_MAX_WIDTH);
        assert_eq!(radius, CENTER_MORPH_RADIUS);
    }

    #[test]
    fn closing_folds_back_to_the_centre_before_the_panel_unmounts() {
        let mut h = Harness::new();
        h.settle();
        h.state.open = false;
        h.frame(3_016.0);
        let (_, mid, _) = h.surface(3_200.0).expect("the folding surface");
        assert!(mid.width < CENTER_MORPH_MAX_WIDTH, "still folding: {mid:?}");
        let done = 3_016.0 + CENTER_MORPH_UNFOLD.as_secs_f64() * 1000.0 + 50.0;
        assert!(h.surface(done).is_none(), "and stops painting once folded");
    }

    #[test]
    fn the_host_is_handed_an_instant_entrance_and_a_fold_back_length_exit() {
        let config = center_morph_modal_config(MorphingBackdrop::Scrim);
        assert_eq!(
            config.enter,
            Ramp::eased(Duration::ZERO, CENTER_UNFOLD_EASE)
        );
        assert_eq!(
            config.exit,
            Ramp::eased(CENTER_MORPH_UNFOLD, CENTER_UNFOLD_EASE)
        );
        assert_eq!(config.scrim_fade, CENTER_MORPH_SCRIM_FADE);
        assert_eq!(config.max_width, ModalLimit::Px(CENTER_MORPH_MAX_WIDTH));
    }

    #[test]
    fn reduce_motion_shows_the_whole_panel_on_the_first_frame() {
        let mut h = Harness::new();
        let mut theme = light();
        theme.motion.reduce_motion = true;
        h.root.set_theme(Box::new(theme));
        h.frame(0.0);
        let (_, size, _) = h.surface(0.0).expect("the panel");
        assert_eq!(size.width, CENTER_MORPH_MAX_WIDTH, "no unfold at all");
    }

    // ---- The close control ------------------------------------------------

    #[test]
    fn the_close_control_arrives_after_its_own_delay() {
        assert_eq!(
            CenterPanelWidget::close_reveal(Duration::from_millis(100), false),
            0.0,
            "nothing before the 160ms delay"
        );
        assert!(CenterPanelWidget::close_reveal(Duration::from_millis(200), false) > 0.0);
        assert_eq!(
            CenterPanelWidget::close_reveal(Duration::from_millis(1_000), false),
            1.0
        );
        assert_eq!(
            CenterPanelWidget::close_reveal(Duration::ZERO, true),
            1.0,
            "`reduce_motion` puts it straight there"
        );
    }

    #[test]
    fn the_close_control_is_a_round_wash_in_the_trailing_top_corner() {
        let mut h = Harness::new();
        h.settle();
        let rec = h.paint_at(3_000.0);
        let want = style::with_alpha(light().scheme().on_surface, CENTER_MORPH_CLOSE_ALPHA);
        let (origin, size, radius, _) = *rec
            .rrects
            .iter()
            .find(|(_, _, _, c)| *c == want)
            .expect("the close control");
        assert_eq!(
            size,
            Size::new(CENTER_MORPH_CLOSE_SIZE, CENTER_MORPH_CLOSE_SIZE)
        );
        assert_eq!(radius, CENTER_MORPH_CLOSE_SIZE / 2.0);
        let (panel_origin, panel_size, _) = h.surface(3_000.0).expect("the panel");
        assert_eq!(
            origin.x,
            panel_origin.x + panel_size.width - CENTER_MORPH_CLOSE_INSET - CENTER_MORPH_CLOSE_SIZE
        );
        assert_eq!(origin.y, panel_origin.y + CENTER_MORPH_CLOSE_INSET);
        assert!(rec.strokes > 0, "and carries the X mark");
    }

    #[test]
    fn pressing_the_close_control_closes_the_modal_once() {
        let mut h = Harness::new();
        h.settle();
        let centre = h.close_centre();
        h.event(pointer(PointerPhase::Down, centre.x, centre.y));
        assert!(h.state.opens.is_empty(), "a press alone does not close it");
        h.event(pointer(PointerPhase::Up, centre.x, centre.y));
        assert_eq!(h.state.opens, vec![false]);
    }

    #[test]
    fn a_press_that_leaves_the_close_control_closes_nothing() {
        let mut h = Harness::new();
        h.settle();
        let centre = h.close_centre();
        h.event(pointer(PointerPhase::Down, centre.x, centre.y));
        h.event(pointer(
            PointerPhase::Up,
            centre.x - CENTER_MORPH_CLOSE_SIZE * 2.0,
            centre.y,
        ));
        assert!(h.state.opens.is_empty());
    }

    #[test]
    fn a_modal_with_no_close_control_paints_none_and_answers_no_press() {
        let mut h = Harness::new();
        h.close_button = false;
        h.settle();
        let rec = h.paint_at(3_000.0);
        let want = style::with_alpha(light().scheme().on_surface, CENTER_MORPH_CLOSE_ALPHA);
        assert!(!rec.rrects.iter().any(|(_, _, _, c)| *c == want));
        let (origin, size, _) = h.surface(3_000.0).expect("the panel");
        let where_it_would_be = Point::new(
            origin.x + size.width - CENTER_MORPH_CLOSE_INSET - CENTER_MORPH_CLOSE_SIZE / 2.0,
            origin.y + CENTER_MORPH_CLOSE_INSET + CENTER_MORPH_CLOSE_SIZE / 2.0,
        );
        h.event(pointer(
            PointerPhase::Down,
            where_it_would_be.x,
            where_it_would_be.y,
        ));
        h.event(pointer(
            PointerPhase::Up,
            where_it_would_be.x,
            where_it_would_be.y,
        ));
        assert!(h.state.opens.is_empty());
    }

    // ---- Host-owned dismissal and the backdrop -----------------------------

    #[test]
    fn a_backdrop_click_and_escape_each_close_it_once() {
        let mut h = Harness::new();
        h.settle();
        h.event(pointer(PointerPhase::Down, 4.0, 4.0));
        h.event(pointer(PointerPhase::Up, 4.0, 4.0));
        assert_eq!(h.state.opens, vec![false]);

        let mut h = Harness::new();
        h.settle();
        h.event(pointer(PointerPhase::Down, 4.0, 4.0));
        h.event(escape());
        assert_eq!(h.state.opens, vec![false]);
    }

    #[test]
    fn the_frosted_backdrop_is_the_panels_own_wash_with_the_host_scrim_off() {
        assert!(!center_morph_modal_config(MorphingBackdrop::Frosted).scrim);
        assert!(center_morph_modal_config(MorphingBackdrop::Scrim).scrim);
        let mut h = Harness::new();
        h.backdrop = MorphingBackdrop::Frosted;
        h.settle();
        let rec = h.paint_at(3_000.0);
        let wash = style::with_alpha(light().scheme().surface, CENTER_MORPH_FROST_ALPHA);
        assert!(rec.rects.iter().any(|(_, _, c)| *c == wash));
    }

    #[test]
    fn a_centre_morph_modal_is_a_pushable_modal_component() {
        fn dismissable<V: ModalContent<App>>(view: &V) -> bool {
            view.modal_dismissable()
        }
        assert!(dismissable(&center_morph_modal::<App, _>(SizedBox(
            None, None
        ))));
        assert!(!dismissable(
            &center_morph_modal::<App, _>(SizedBox(None, None)).dismissable(false)
        ));
    }
}
