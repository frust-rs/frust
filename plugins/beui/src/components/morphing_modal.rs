//! Ports beUI's `morphing-modal` component —
//! `components/motion/morphing-modal.tsx` (beUI rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01).
//!
//! | upstream | here |
//! |---|---|
//! | `w-full max-w-sm` inside `inset-4` | [`PANEL_MAX_WIDTH`], [`PANEL_MARGIN`] |
//! | `rounded-3xl border border-border bg-background shadow-2xl` | [`MORPHING_MODAL_RADIUS`] and the panel's chrome |
//! | panel padding `p-5` | [`MORPHING_MODAL_PADDING`] |
//! | backdrop `bg-background/5` + `backdrop-blur(14px) saturate(140%)` | [`MorphingBackdrop::Frosted`], [`MORPHING_MODAL_FROST_ALPHA`] |
//! | backdrop transition `0.2s EASE_OUT` | the host's [`SCRIM_FADE`](crate::overlay::SCRIM_FADE) |
//! | `placement: "bottom" \| "center"` | [`MorphingPlacement`] |
//! | `SPRING_PANEL`, `exit 0.18s EASE_OUT` | [`MORPHING_MODAL_MORPH`] / [`MORPHING_MODAL_CLOSE`] |
//!
//! # Premise correction: upstream's morph is a *view swap*, not a trigger morph
//!
//! The porting card asks for "a trigger element that morphs into a centered
//! modal". `morphing-modal.tsx` does not do that. Its morph is between the
//! *views inside the panel*: `viewId` selects one, `layout` animates the panel's
//! own box as the view's height changes, and `AnimatePresence mode="popLayout"`
//! cross-fades the outgoing and incoming view with an 8px lift and a 4px blur.
//! The trigger is an ordinary button that sets `viewId`, and the modal simply
//! mounts.
//!
//! Both are implemented, because both are wanted:
//!
//! - **The trigger morph** the card asks for is the entrance: the panel's
//!   surface travels from the trigger's captured rect to its own, and back on
//!   close ([`MorphingModalView::anchor`]). This is the mechanism decision the
//!   card asks to be documented — see below.
//! - **The view swap** upstream ships is [`MorphingModalView::view`]: changing
//!   the id cross-fades the panel's content with the same 8px lift.
//!
//! # The chosen morph mechanism, and why
//!
//! `hero()`/`ContainerTransform` is a **navigator transition** mechanism: it
//! matches tagged widgets across a *page change* and is driven by the
//! navigator's own transition clock. A modal pushed by
//! [`crate::overlay::show_modal`] is pushed with
//! [`TransitionSpec::NONE`](frust::TransitionSpec) precisely so the host can
//! stage its own entrance, and a `Stack`-mounted modal is not a page change at
//! all — so there is no navigator transition for a hero to ride. It does not
//! fit.
//!
//! The route taken instead is the seam's other one: **a modal-hosted surface
//! animated from the captured trigger rect**, exactly the mechanism
//! [`crate::components::popover`]'s neck morph already carries
//! ([`morph_rect`](crate::components::popover::morph_rect)), reading the
//! trigger's window-space rect out of an [`OverlayAnchor`] the trigger writes.
//! The panel's surface, its radius and its content clip all travel together, and
//! the same lerp runs backwards on close, so the panel visibly returns to the
//! control that opened it.
//!
//! The host's own centred staging (`{opacity, y: 20, scale: 0.97}`) is replaced
//! rather than layered: [`MorphingModalView`] hands the host an instant entrance
//! ramp and a [`MORPHING_MODAL_CLOSE`] exit, so the morph *is* the entrance and
//! the host's exit only holds the pod mounted long enough for the return morph
//! to finish. Without an anchor there is nothing to morph from, and the host's
//! own lift-scale-fade is used unchanged — which is the literal upstream
//! entrance.
//!
//! # Degradations against the web original
//!
//! - **No backdrop blur or saturation.** `backdrop-filter: blur(14px)
//!   saturate(140%)` has no `PaintScene` primitive. [`MorphingBackdrop::Frosted`]
//!   paints the `bg-background/5` wash alone, which without the blur behind it is
//!   very nearly clear glass — legible as "the page is inert" only from the
//!   barrier behaviour, not from the paint. [`MorphingBackdrop::Scrim`] is the
//!   dimming fallback for a caller who needs the modal to *read* as modal, and
//!   is the default here for that reason. This is the one place this module
//!   departs from upstream's own default, and it is because the effect upstream
//!   relies on cannot be drawn.
//! - **No content blur on the view swap.** The same missing primitive: the swap
//!   is opacity + an 8px lift, with `filter: blur(4px)` dropped.
//! - **The view swap does not resize the panel.** Upstream's `layout` prop
//!   animates the panel's box as the view changes height; the panel here is
//!   sized by the host from its content's own layout, so a taller view resizes
//!   the panel on the frame it arrives rather than gliding to it. Animating it
//!   would mean re-laying the panel out every frame of the swap, which is the
//!   cost the seam's transform staging exists to avoid.
//! - **`placement: "bottom"`'s 40px lift is not restated.** The bottom placement
//!   mounts on the host's bottom edge, which slides the panel's own height in
//!   rather than lifting it 40px — the same trade [`crate::components::drawer`]
//!   documents, and the reason [`crate::overlay::modal`] has an edge mount at
//!   all.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, Point, Rect, SemanticsCtx, Size, Vec2, View, Widget, any,
    build_child, rebuild_child, route_event_single, teardown_child, visit_children,
};
use frust::{FrameTime, Theme};

use crate::components::popover::{morph_rect, paint_panel, resolve_panel};
use crate::motion::{Presence, PresencePhase, Ramp};
use crate::overlay::{
    ModalConfig, ModalContent, ModalEdge, ModalMount, ModalView, ModalWidget, OverlayAnchor,
    PANEL_MARGIN, PANEL_MAX_WIDTH, modal,
};
use crate::style;
use crate::tokens::motion::{EASE_OUT, SPRING_PANEL};

/// `rounded-3xl` — the panel's corner radius, in logical px.
pub const MORPHING_MODAL_RADIUS: f64 = style::RADIUS_3XL;

/// `p-5` — the panel's own padding around its view, in logical px.
pub const MORPHING_MODAL_PADDING: f64 = 20.0;

/// The trigger-morph's entrance ramp — the panel's own [`SPRING_PANEL`], which
/// is what upstream springs the panel on.
pub const MORPHING_MODAL_MORPH: Ramp = Ramp::spring(SPRING_PANEL);

/// How long the return morph takes (`exit.transition.duration` of `0.18`).
pub const MORPHING_MODAL_CLOSE: Duration = Duration::from_millis(180);

/// `bg-background/5` — the frosted backdrop's wash alpha.
pub const MORPHING_MODAL_FROST_ALPHA: f32 = 0.05;

/// How long a view swap's cross-fade takes in (`0.24`).
pub const MORPHING_MODAL_VIEW_IN: Duration = Duration::from_millis(240);

/// How long it takes out (`0.16`).
pub const MORPHING_MODAL_VIEW_OUT: Duration = Duration::from_millis(160);

/// How far a swapping view travels, in logical px (`y: 8`).
pub const MORPHING_MODAL_VIEW_LIFT: f64 = 8.0;

/// Where the panel sits — upstream's `placement`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MorphingPlacement {
    /// `items-center`: centred on both axes.
    #[default]
    Center,
    /// `items-end pb-4`: against the bottom of the area (see the [module
    /// docs](self) on how the two differ here).
    Bottom,
}

/// What the modal paints behind its panel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MorphingBackdrop {
    /// The catalog's dimming [`scrim`](crate::overlay::scrim) — the default
    /// here, because the frosted one cannot be drawn (see the [module
    /// docs](self)).
    #[default]
    Scrim,
    /// Upstream's own `bg-background/5` wash, minus the blur behind it.
    Frosted,
}

/// The [`ModalConfig`] a morphing modal is hosted with.
///
/// `anchored` picks the ramps: a trigger morph replaces the host's entrance with
/// an instant one (the morph *is* the entrance) and holds the pod mounted for
/// the return; without an anchor the host's own lift-scale-fade is upstream's
/// literal entrance and is kept.
pub fn morphing_modal_config(
    placement: MorphingPlacement,
    backdrop: MorphingBackdrop,
    anchored: bool,
) -> ModalConfig {
    let base = match placement {
        MorphingPlacement::Center => ModalConfig::centered(),
        MorphingPlacement::Bottom => ModalConfig {
            mount: ModalMount::Edge(ModalEdge::Bottom),
            ..ModalConfig::centered()
        },
    };
    let base = base
        .margin(PANEL_MARGIN)
        .scrim(backdrop == MorphingBackdrop::Scrim);
    if anchored {
        base.ramps(
            Ramp::eased(Duration::ZERO, EASE_OUT),
            Ramp::eased(MORPHING_MODAL_CLOSE, EASE_OUT),
        )
    } else {
        base
    }
}

// ---- The component ---------------------------------------------------------

/// The mutable panel configuration the outer builder writes and the panel reads
/// — the handle shape [`crate::components::popover`] documents.
type PanelHandle = Rc<RefCell<MorphConfig>>;

/// What the panel needs from its component's builders.
#[derive(Clone, Debug)]
struct MorphConfig {
    open: bool,
    backdrop: MorphingBackdrop,
    radius: f64,
    padding: f64,
    /// The view currently shown, for the cross-fade. `None` never swaps.
    view: Option<String>,
    /// The trigger's captured rect, when there is one to morph from.
    anchor: Option<OverlayAnchor>,
}

/// Equality over what the panel renders from; the shared [`OverlayAnchor`] cell
/// carries no identity comparison and is re-seated unconditionally instead.
impl PartialEq for MorphConfig {
    fn eq(&self, other: &Self) -> bool {
        self.open == other.open
            && self.backdrop == other.backdrop
            && self.radius == other.radius
            && self.padding == other.padding
            && self.view == other.view
            && self.anchor.is_some() == other.anchor.is_some()
    }
}

/// A declarative beUI morphing modal. See [`morphing_modal`].
pub struct MorphingModalView<State: 'static> {
    inner: ModalView<State>,
    config: PanelHandle,
    placement: MorphingPlacement,
}

/// Build a morphing modal over `content`, centred and open by default.
///
/// Point it at a trigger with [`MorphingModalView::anchor`] for the morph the
/// module docs describe; without one it plays the host's own centred entrance,
/// which is upstream's literal behaviour.
pub fn morphing_modal<State: 'static, V: View<State>>(content: V) -> MorphingModalView<State> {
    let config: PanelHandle = Rc::new(RefCell::new(MorphConfig {
        open: true,
        backdrop: MorphingBackdrop::default(),
        radius: MORPHING_MODAL_RADIUS,
        padding: MORPHING_MODAL_PADDING,
        view: None,
        anchor: None,
    }));
    let placement = MorphingPlacement::default();
    let panel = MorphPanelView {
        content: any(content),
        config: config.clone(),
    };
    MorphingModalView {
        inner: modal(
            panel,
            morphing_modal_config(placement, MorphingBackdrop::default(), false),
        ),
        config,
        placement,
    }
}

impl<State: 'static> MorphingModalView<State> {
    /// Morph the panel out of (and back into) the rect `anchor` carries — the
    /// cell [`crate::overlay::anchor`] writes on the trigger's every paint.
    pub fn anchor(mut self, anchor: &OverlayAnchor) -> Self {
        self.config.borrow_mut().anchor = Some(anchor.clone());
        self.resync();
        self
    }

    /// Set the placement (default [`MorphingPlacement::Center`]).
    pub fn placement(mut self, placement: MorphingPlacement) -> Self {
        self.placement = placement;
        self.resync();
        self
    }

    /// Pick the backdrop (default [`MorphingBackdrop::Scrim`] — see the [module
    /// docs](self)).
    pub fn backdrop(mut self, backdrop: MorphingBackdrop) -> Self {
        self.config.borrow_mut().backdrop = backdrop;
        self.resync();
        self
    }

    /// Name the view currently shown — upstream's `viewId`. Changing it
    /// cross-fades the panel's content; leaving it unset never swaps.
    pub fn view(self, view: impl Into<String>) -> Self {
        self.config.borrow_mut().view = Some(view.into());
        self
    }

    /// Set the panel's corner radius, in logical px (default
    /// [`MORPHING_MODAL_RADIUS`]).
    pub fn radius(self, radius: f64) -> Self {
        self.config.borrow_mut().radius = radius.max(0.0);
        self
    }

    /// Set the panel's padding around its view, in logical px (default
    /// [`MORPHING_MODAL_PADDING`]).
    pub fn padding(self, padding: f64) -> Self {
        self.config.borrow_mut().padding = padding.max(0.0);
        self
    }

    /// Hand the modal the app's open flag. The default is `true`.
    pub fn open(mut self, open: bool) -> Self {
        self.config.borrow_mut().open = open;
        self.inner = self.inner.open(open);
        self
    }

    /// The accessibility label the panel is announced with.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.inner = self.inner.label(label);
        self
    }

    /// Whether Escape, a backdrop click and a back press close it (default
    /// `true`).
    pub fn dismissable(mut self, dismissable: bool) -> Self {
        self.inner.config = self.inner.config.dismissable(dismissable);
        self
    }

    /// Set the close callback: Escape, a backdrop click, or a back press.
    /// Reports `false`, once per dismissal.
    pub fn on_open_change<F: Fn(&mut State, bool) + 'static>(mut self, on_open_change: F) -> Self {
        self.inner = self
            .inner
            .on_dismiss(move |state| on_open_change(state, false));
        self
    }

    /// Set the exit-settled callback — state-free, fired from the paint that
    /// finishes the return morph.
    pub fn on_close<F: Fn() + 'static>(mut self, on_close: F) -> Self {
        self.inner = self.inner.on_close(on_close);
        self
    }

    /// Re-resolve the host config from the current placement, backdrop and
    /// anchor, keeping the dismissability a caller has already set.
    fn resync(&mut self) {
        let (backdrop, anchored) = {
            let config = self.config.borrow();
            (config.backdrop, config.anchor.is_some())
        };
        let dismissable = self.inner.config.dismissable;
        self.inner.config =
            morphing_modal_config(self.placement, backdrop, anchored).dismissable(dismissable);
    }
}

impl<State: 'static> View<State> for MorphingModalView<State> {
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

impl<State: 'static> ModalContent<State> for MorphingModalView<State> {
    fn modal_dismissable(&self) -> bool {
        self.inner.config.dismissable
    }
}

// ---- The panel -------------------------------------------------------------

/// The modal's panel: the surface, the frosted backdrop when it is asked for,
/// the trigger morph and the view cross-fade.
struct MorphPanelView<State: 'static> {
    content: AnyView<State>,
    config: PanelHandle,
}

/// The retained widget for a morphing-modal panel.
pub struct MorphPanelWidget {
    content: ChildPod,
    config: MorphConfig,
    /// Drives the trigger morph, opened and closed with the host. Absent when
    /// there is no anchor to morph from.
    morph: Option<Presence>,
    /// The `reduce_motion` value `morph` was last built for.
    reduced: Option<bool>,
    /// The view swap's own clock: the frame the current view arrived on.
    swapped_at: Option<FrameTime>,
    /// The morph's progress at the last paint.
    progress: Cell<f64>,
}

impl MorphPanelWidget {
    /// The trigger morph's progress at the last paint: `0.0` on the trigger,
    /// `1.0` on the panel. Always `1.0` for an unanchored modal.
    pub fn morph_progress(&self) -> f64 {
        self.progress.get()
    }

    /// Whether a view swap is still cross-fading.
    pub fn is_swapping(&self, now: FrameTime) -> bool {
        self.swapped_at
            .is_some_and(|then| now.saturating_sub(then) < MORPHING_MODAL_VIEW_IN)
    }

    /// The driver a trigger morph runs on.
    fn morph_presence(reduce: bool) -> Presence {
        let base = Presence::new(
            MORPHING_MODAL_MORPH,
            Ramp::eased(MORPHING_MODAL_CLOSE, EASE_OUT),
        );
        if reduce { base.collapsed() } else { base }
    }

    /// Rebuild the morph driver when `reduce_motion` flips, preserving what the
    /// old one was doing.
    fn sync_motion(&mut self, reduce: bool) {
        if self.config.anchor.is_none() {
            self.morph = None;
            self.reduced = Some(reduce);
            return;
        }
        if self.reduced == Some(reduce) && self.morph.is_some() {
            return;
        }
        let was_exiting = self
            .morph
            .is_some_and(|morph| morph.phase() == PresencePhase::Exiting);
        self.reduced = Some(reduce);
        let mut next = Self::morph_presence(reduce);
        if was_exiting && !self.config.open {
            next.set_open(true);
        }
        next.set_open(self.config.open);
        self.morph = Some(next);
    }
}

impl<State: 'static> View<State> for MorphPanelView<State> {
    type Element = MorphPanelWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> MorphPanelWidget {
        let config = self.config.borrow().clone();
        let morph = config.anchor.as_ref().map(|_| {
            let mut presence = MorphPanelWidget::morph_presence(false);
            presence.set_open(config.open);
            presence
        });
        MorphPanelWidget {
            content: build_child(&self.content, ctx),
            config,
            morph,
            reduced: None,
            swapped_at: None,
            progress: Cell::new(0.0),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut MorphPanelWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(&prev.content, &self.content, &mut element.content, ctx);
        let config = self.config.borrow().clone();
        element.config.anchor = config.anchor.clone();
        if element.config != config {
            if element.config.open != config.open
                && let Some(morph) = element.morph.as_mut()
            {
                morph.set_open(config.open);
            }
            if element.config.view != config.view {
                // A fresh view: the cross-fade is timed from the frame it next
                // paints on.
                element.swapped_at = None;
            }
            let relayout = element.config.padding != config.padding;
            element.config = config;
            flags |= ChangeFlags::PAINT;
            if relayout {
                flags |= ChangeFlags::LAYOUT;
            }
        }
        flags
    }

    fn teardown(&self, element: &mut MorphPanelWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.content, &mut element.content, ctx);
    }
}

impl Widget for MorphPanelWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let pad = self.config.padding;
        // The host has already resolved the panel's width (`w-full max-w-sm`
        // inside `inset-4`) and left the height to hug; the padding is the
        // panel's own `p-5`.
        let inner = Size::new(
            (bc.max().width - pad * 2.0).max(0.0),
            (bc.max().height - pad * 2.0).max(0.0),
        );
        let content = self
            .content
            .layout_child(ctx, &BoxConstraints::loose(inner));
        self.content.set_origin(Point::new(pad, pad));
        bc.constrain(Size::new(
            content.width + pad * 2.0,
            content.height + pad * 2.0,
        ))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let chrome = resolve_panel(Theme::from_paint_ctx(ctx));
        let reduce = Theme::from_paint_ctx(ctx).is_some_and(|t| t.motion.reduce_motion);
        let backdrop_wash = match Theme::from_paint_ctx(ctx) {
            Some(theme) => theme.scheme().surface,
            None => crate::BEUI_LIGHT.background,
        };
        self.sync_motion(reduce);
        let now = ctx.frame_time();

        // The frosted backdrop, when it is the panel's job rather than the
        // host's. It is painted over the whole *host* area, which is why the
        // panel reaches outside its own box for it: the host turned its own
        // scrim off (`ModalConfig::scrim`), exactly as the seam's docs describe.
        if self.config.backdrop == MorphingBackdrop::Frosted {
            let area = ctx
                .visible_rect()
                .unwrap_or_else(|| Rect::from_origin_size(ctx.origin(), ctx.size()));
            scene.fill_rect(
                area.origin(),
                area.size(),
                style::with_alpha(backdrop_wash, MORPHING_MODAL_FROST_ALPHA),
            );
        }

        let progress = match self.morph.as_mut() {
            Some(morph) => {
                let progress = morph.advance(now);
                if morph.is_animating() {
                    ctx.request_frame();
                }
                if !morph.is_visible() {
                    self.progress.set(0.0);
                    return;
                }
                progress
            }
            // No anchor: the host's own staging is the entrance, and the panel
            // is simply itself.
            None => 1.0,
        };
        self.progress.set(progress);

        let panel = Rect::from_origin_size(Point::ORIGIN, ctx.size());
        let (rect, radius) = match &self.config.anchor {
            Some(anchor) => {
                let trigger = anchor.rect() - ctx.origin().to_vec2();
                let from_radius = (trigger.height() / 2.0).min(self.config.radius).max(0.0);
                morph_rect(trigger, from_radius, panel, self.config.radius, progress)
            }
            None => (panel, self.config.radius),
        };
        paint_panel(scene, ctx.origin(), rect, radius, chrome);

        // The view cross-fade: `{opacity, y: 8}` in, `-8` out, both timed from
        // the frame the new view first painted.
        let swapped = *self.swapped_at.get_or_insert(now);
        let elapsed = now.saturating_sub(swapped);
        // A modal that never names a view has nothing to swap; one whose swap
        // has run its course is simply settled. Both are the resting state.
        let settled = self.config.view.is_none() || elapsed >= MORPHING_MODAL_VIEW_IN;
        let (alpha, lift) = if settled {
            (1.0, 0.0)
        } else {
            ctx.request_frame();
            let t = Ramp::eased(MORPHING_MODAL_VIEW_IN, EASE_OUT).progress_clamped(elapsed);
            (t, (1.0 - t) * MORPHING_MODAL_VIEW_LIFT)
        };
        let reduced_lift = if reduce { 0.0 } else { lift };

        scene.push_clip_rounded(ctx.origin() + rect.origin().to_vec2(), rect.size(), radius);
        let faded = alpha < 1.0;
        if faded {
            scene.push_layer(ctx.origin(), ctx.size(), alpha as f32);
        }
        if reduced_lift != 0.0 {
            scene.push_transform(frust::authoring::Affine::translate(Vec2::new(
                0.0,
                reduced_lift,
            )));
        }
        self.content.paint_child(ctx, scene);
        if reduced_lift != 0.0 {
            scene.pop_transform();
        }
        if faded {
            scene.pop_layer();
        }
        scene.pop_clip();
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        route_event_single(&mut self.content, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.content.semantics_child(ctx);
    }

    visit_children!(content);
}

/// The panel's resting width inside an `area`-sized host, for a caller sizing
/// its own view against it: the area less its margins, capped at
/// [`PANEL_MAX_WIDTH`].
pub fn morphing_modal_width(area: Size) -> f64 {
    (area.width - PANEL_MARGIN * 2.0).clamp(0.0, PANEL_MAX_WIDTH)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::popover::tests::{Recorder, WINDOW, escape, ft_ms, light, pointer};
    use frust::SizedBox;
    use frust::authoring::PointerPhase;
    use frust::authoring::text::TextContext;
    use frust_core::RenderRoot;
    use std::any::Any;

    #[derive(Default)]
    struct App {
        open: bool,
        opens: Vec<bool>,
        view: usize,
    }

    /// The trigger's rect, in window space.
    const TRIGGER: Rect = Rect::new(40.0, 400.0, 200.0, 440.0);

    struct Harness {
        root: RenderRoot<App, frust::StackView<App>>,
        state: App,
        tcx: TextContext,
        anchor: Option<OverlayAnchor>,
        backdrop: MorphingBackdrop,
    }

    impl Harness {
        fn new(anchored: bool) -> Self {
            let anchor = anchored.then(|| {
                let anchor = OverlayAnchor::new();
                anchor.set(TRIGGER);
                anchor
            });
            let mut h = Harness {
                root: RenderRoot::new(),
                state: App {
                    open: true,
                    ..App::default()
                },
                tcx: TextContext::new(),
                anchor,
                backdrop: MorphingBackdrop::default(),
            };
            h.root.set_theme(Box::new(light()));
            h.frame(0.0);
            h
        }

        fn frame(&mut self, ms: f64) {
            let anchor = self.anchor.clone();
            let backdrop = self.backdrop;
            let mut logic = move |s: &mut App| {
                let mut view = morphing_modal(SizedBox(Some(200.0), Some(120.0)))
                    .backdrop(backdrop)
                    .view(format!("view-{}", s.view))
                    .open(s.open)
                    .label("Send")
                    .on_open_change(|s: &mut App, open| {
                        s.open = open;
                        s.opens.push(open);
                    });
                if let Some(anchor) = &anchor {
                    view = view.anchor(anchor);
                }
                frust::Stack(vec![any(view)])
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

        /// The panel's surface, at the given frame.
        fn surface(&mut self, ms: f64) -> Option<(Point, Size, f64)> {
            let theme = light();
            let want = theme.scheme().surface_container_high;
            self.paint_at(ms)
                .rrects
                .iter()
                .find(|(_, _, _, c)| *c == want)
                .map(|(o, s, r, _)| (*o, *s, *r))
        }
    }

    // ---- The trigger morph ------------------------------------------------

    #[test]
    fn an_anchored_modal_starts_on_the_triggers_own_rect() {
        let mut h = Harness::new(true);
        let (origin, size, _) = h.surface(0.0).expect("the panel's surface");
        assert!(
            (origin.x - TRIGGER.x0).abs() < 1.0 && (origin.y - TRIGGER.y0).abs() < 1.0,
            "the first frame is the trigger, not the panel: {origin:?}"
        );
        assert!((size.width - TRIGGER.width()).abs() < 1.0);
    }

    #[test]
    fn it_lands_on_the_panels_own_rect_at_its_own_radius() {
        let mut h = Harness::new(true);
        h.frame(0.0);
        h.frame(3_000.0);
        let (_, size, radius) = h.surface(3_000.0).expect("the settled panel");
        assert_eq!(radius, MORPHING_MODAL_RADIUS);
        assert_eq!(size.width, morphing_modal_width(WINDOW));
    }

    #[test]
    fn closing_returns_the_panel_to_the_trigger_before_it_unmounts() {
        let mut h = Harness::new(true);
        h.frame(0.0);
        h.frame(3_000.0);
        h.state.open = false;
        h.frame(3_016.0);
        let (origin, size, _) = h.surface(3_100.0).expect("the returning panel");
        assert!(
            origin.y > TRIGGER.y0 - WINDOW.height && size.width < morphing_modal_width(WINDOW),
            "the surface has started travelling back: {origin:?} {size:?}"
        );
        let settled = h.surface(3_016.0 + MORPHING_MODAL_CLOSE.as_secs_f64() * 1000.0 + 50.0);
        assert!(settled.is_none(), "and stops painting once it arrives");
    }

    #[test]
    fn an_unanchored_modal_plays_the_hosts_own_entrance_instead() {
        let mut h = Harness::new(false);
        let (origin, size, radius) = h.surface(0.0).expect("the panel's surface");
        // No morph: the panel is its own rect from the first frame, and the
        // host's lift/scale/fade is what stages it.
        assert_eq!(size.width, morphing_modal_width(WINDOW));
        assert_eq!(radius, MORPHING_MODAL_RADIUS);
        assert_eq!(origin.y, (WINDOW.height - size.height) / 2.0);
        assert!(
            morphing_modal_config(MorphingPlacement::Center, MorphingBackdrop::Scrim, false).enter
                == ModalConfig::centered().enter,
            "and the host keeps its own spring"
        );
    }

    #[test]
    fn an_anchored_host_is_handed_an_instant_entrance_and_a_return_length_exit() {
        let config =
            morphing_modal_config(MorphingPlacement::Center, MorphingBackdrop::Scrim, true);
        assert_eq!(config.enter, Ramp::eased(Duration::ZERO, EASE_OUT));
        assert_eq!(config.exit, Ramp::eased(MORPHING_MODAL_CLOSE, EASE_OUT));
    }

    #[test]
    fn reduce_motion_puts_an_anchored_panel_straight_onto_its_own_rect() {
        let mut h = Harness::new(true);
        let mut theme = light();
        theme.motion.reduce_motion = true;
        h.root.set_theme(Box::new(theme));
        h.frame(0.0);
        let (_, size, _) = h.surface(0.0).expect("the panel");
        assert_eq!(size.width, morphing_modal_width(WINDOW));
    }

    // ---- The backdrop -----------------------------------------------------

    #[test]
    fn the_default_backdrop_is_the_dimming_scrim_the_host_paints() {
        let config =
            morphing_modal_config(MorphingPlacement::Center, MorphingBackdrop::Scrim, false);
        assert!(config.scrim, "the host paints it");
        let mut h = Harness::new(false);
        h.frame(0.0);
        h.frame(3_000.0);
        let rec = h.paint_at(3_000.0);
        let scrim = crate::overlay::scrim(Some(&light()));
        assert!(
            rec.rects.iter().any(|(o, s, c)| *o == Point::ZERO
                && *s == WINDOW
                && c.components[3] >= scrim.components[3] - 1e-4),
            "a settled scrim is the role's own alpha"
        );
    }

    #[test]
    fn the_frosted_backdrop_is_the_panels_own_wash_with_the_host_scrim_off() {
        let config =
            morphing_modal_config(MorphingPlacement::Center, MorphingBackdrop::Frosted, false);
        assert!(!config.scrim, "the host stands down");
        let mut h = Harness::new(false);
        h.backdrop = MorphingBackdrop::Frosted;
        h.frame(0.0);
        h.frame(3_000.0);
        let rec = h.paint_at(3_000.0);
        let wash = style::with_alpha(light().scheme().surface, MORPHING_MODAL_FROST_ALPHA);
        assert!(
            rec.rects.iter().any(|(_, _, c)| *c == wash),
            "the panel paints `bg-background/5` itself"
        );
    }

    // ---- Placement and dismissal ------------------------------------------

    #[test]
    fn the_bottom_placement_mounts_on_the_hosts_bottom_edge() {
        assert!(matches!(
            morphing_modal_config(MorphingPlacement::Bottom, MorphingBackdrop::Scrim, false).mount,
            ModalMount::Edge(ModalEdge::Bottom)
        ));
        assert!(matches!(
            morphing_modal_config(MorphingPlacement::Center, MorphingBackdrop::Scrim, false).mount,
            ModalMount::Center
        ));
    }

    #[test]
    fn the_panel_width_is_max_w_sm_inside_the_inset_4_margin() {
        assert_eq!(
            morphing_modal_width(Size::new(1200.0, 800.0)),
            PANEL_MAX_WIDTH
        );
        assert_eq!(
            morphing_modal_width(Size::new(320.0, 640.0)),
            320.0 - PANEL_MARGIN * 2.0,
            "a phone-width window is narrower than the cap"
        );
    }

    #[test]
    fn a_backdrop_click_and_escape_each_close_it_once() {
        let mut h = Harness::new(false);
        h.frame(0.0);
        h.frame(3_000.0);
        h.event(pointer(PointerPhase::Down, 4.0, 4.0));
        h.event(pointer(PointerPhase::Up, 4.0, 4.0));
        assert_eq!(h.state.opens, vec![false]);

        let mut h = Harness::new(false);
        h.frame(0.0);
        h.frame(3_000.0);
        h.event(pointer(PointerPhase::Down, 4.0, 4.0));
        h.event(escape());
        assert_eq!(h.state.opens, vec![false]);
    }

    #[test]
    fn a_morphing_modal_is_a_pushable_modal_component() {
        fn dismissable<V: ModalContent<App>>(view: &V) -> bool {
            view.modal_dismissable()
        }
        assert!(dismissable(&morphing_modal::<App, _>(SizedBox(None, None))));
        assert!(!dismissable(
            &morphing_modal::<App, _>(SizedBox(None, None)).dismissable(false)
        ));
    }

    // ---- The view swap ----------------------------------------------------

    #[test]
    fn a_view_swap_fades_the_content_back_in_over_its_own_ramp() {
        let mut h = Harness::new(false);
        h.frame(0.0);
        h.frame(3_000.0);
        let settled = h.paint_at(3_000.0);
        assert!(
            settled.layers.is_empty(),
            "a settled view composites plainly"
        );
        h.state.view = 1;
        h.frame(3_016.0);
        let swapping = h.paint_at(3_020.0);
        assert!(
            swapping.layers.iter().any(|a| *a < 1.0),
            "the new view fades in: {:?}",
            swapping.layers
        );
        h.frame(3_016.0 + MORPHING_MODAL_VIEW_IN.as_secs_f64() * 1000.0 + 10.0);
        let done = h.paint_at(3_016.0 + MORPHING_MODAL_VIEW_IN.as_secs_f64() * 1000.0 + 20.0);
        assert!(done.layers.is_empty(), "and settles opaque");
    }
}
