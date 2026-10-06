//! Ports beUI's `drawer` component — `components/motion/drawer.tsx` (beUI rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01).
//!
//! | upstream | here |
//! |---|---|
//! | `w-80` | [`DRAWER_WIDTH`] |
//! | `max-w-[85vw]` | [`DRAWER_WIDTH_FRACTION`] |
//! | `inset-y-0`, full height | [`ModalConfig::drawer`]'s cross-axis fraction |
//! | `x: "100%"` / `"-100%"` on `SPRING_PANEL` | the host's [`ModalMount::Edge`] slide |
//! | backdrop `bg-black/40`, `0.25s EASE_OUT` | [`crate::overlay::scrim`], [`DRAWER_SCRIM_FADE`] |
//! | `bg-background shadow-2xl` | [`DrawerPanelWidget`]'s surface |
//! | `border-l` (right) / `border-r` (left) | the inner-edge rule |
//! | Escape, backdrop click, `dismissable` | the host's own dismissal |
//!
//! # This component is a configured host, not a new one
//!
//! Everything a drawer does beyond painting its own surface — the scrim, the
//! edge mount, the slide, the barrier, Escape, the backdrop click, the back
//! press and the staged exit — is [`crate::overlay::modal`]'s. This module is
//! the panel plus the two upstream numbers that shape the host
//! ([`ModalConfig::drawer`] already carries them), so [`DrawerView`] is a thin
//! wrapper whose element *is* [`ModalWidget`]. That is what lets
//! [`crate::overlay::show_modal`] push a drawer as a transparent navigator page
//! with no per-component plumbing — [`ModalContent`] is implemented below for
//! exactly that.
//!
//! # Degradations against the web original
//!
//! - **No backdrop blur.** Upstream's backdrop is `bg-black/40 backdrop-blur-sm`;
//!   `PaintScene` has no blur primitive, so the scrim is the wash alone. The
//!   dimming is identical, the blur is gone.
//! - **`shadow-2xl` becomes the `.glass` chrome shadow** every panel in this
//!   catalog casts (`crate::tokens::glass_scale`), the one recipe both overlay
//!   hosts reserve layer room for.
//! - **`reduce_motion` fades rather than slides**, which is upstream's own
//!   reduced branch (`initial={{opacity: 0}}`) reached through
//!   [`crate::motion::Presence::collapsed`]: the host collapses both ramps to a
//!   jump instead of running a 200ms fade of its own. The slide is what the
//!   preference is about, and it is gone either way.
//! - **`top`/`bottom` edges are reachable**, where upstream offers only
//!   `left`/`right` — the host mounts on any edge, and
//!   [`crate::components::bottom_sheet`] is the bottom one with its own chrome.
//!   [`DrawerSide`] is the upstream pair; [`DrawerView::edge`] takes the others
//!   for a caller who wants a plain sliding panel there.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use frust::Theme;
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, Color, EventCtx, EventResult,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, Rect, SemanticsCtx, Size, View, Widget,
    any, build_child, rebuild_child, route_event_single, teardown_child, visit_children,
};

use crate::components::popover::{
    PANEL_SHADOW_ALPHA, PANEL_SHADOW_BLUR, PANEL_SHADOW_Y_OFFSET, resolve_panel,
};
use crate::motion::Ramp;
use crate::overlay::{
    DRAWER_WIDTH, DRAWER_WIDTH_FRACTION, ModalConfig, ModalContent, ModalEdge, ModalView,
    ModalWidget, modal,
};
use crate::style;

/// How long the drawer's backdrop takes to fade, each way (`drawer.tsx` gives
/// its own backdrop `0.25s`, where the modal seam's default is 200ms).
pub const DRAWER_SCRIM_FADE: Duration = Duration::from_millis(250);

/// Which edge the drawer is pinned to — upstream's own `side` prop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DrawerSide {
    /// `left-0 border-r`.
    Left,
    /// `right-0 border-l` — `drawer.tsx`'s own default.
    #[default]
    Right,
}

impl DrawerSide {
    /// The host edge this side mounts on.
    pub const fn edge(self) -> ModalEdge {
        match self {
            DrawerSide::Left => ModalEdge::Left,
            DrawerSide::Right => ModalEdge::Right,
        }
    }
}

/// The edge cell the builder writes and the panel reads.
///
/// The panel view is constructed inside [`drawer`] before any setter has run, so
/// [`DrawerView::side`] cannot rebuild it; it writes this shared cell instead —
/// the handle shape [`crate::components::popover`] documents for the same
/// reason.
type EdgeHandle = Rc<Cell<ModalEdge>>;

/// A declarative beUI drawer. See [`drawer`].
pub struct DrawerView<State: 'static> {
    inner: ModalView<State>,
    edge: EdgeHandle,
}

/// The [`ModalConfig`] a drawer on `edge` is hosted with: [`ModalConfig::drawer`]
/// plus upstream's own slower backdrop.
pub fn drawer_config(edge: ModalEdge) -> ModalConfig {
    ModalConfig::drawer(edge).scrim_fade(DRAWER_SCRIM_FADE)
}

/// Build a drawer over `content`, pinned to [`DrawerSide::Right`] and open by
/// default.
///
/// Mount it as the top child of a full-area [`frust::Stack`] with
/// [`DrawerView::open`] carrying the app's own flag, or push it as a transparent
/// navigator page with [`crate::overlay::show_modal`] — the page's own lifetime
/// is the flag on that route.
pub fn drawer<State: 'static, V: View<State>>(content: V) -> DrawerView<State> {
    let edge: EdgeHandle = Rc::new(Cell::new(DrawerSide::default().edge()));
    DrawerView {
        inner: modal(
            DrawerPanelView {
                content: any(content),
                edge: edge.clone(),
            },
            drawer_config(edge.get()),
        ),
        edge,
    }
}

impl<State: 'static> DrawerView<State> {
    /// Pin the drawer to `side` (default [`DrawerSide::Right`]).
    pub fn side(self, side: DrawerSide) -> Self {
        self.edge(side.edge())
    }

    /// Pin the drawer to any host edge — the superset of upstream's `side` (see
    /// the [module docs](self)).
    pub fn edge(mut self, edge: ModalEdge) -> Self {
        self.edge.set(edge);
        let config = ModalConfig {
            dismissable: self.inner.config.dismissable,
            enter: self.inner.config.enter,
            exit: self.inner.config.exit,
            ..drawer_config(edge)
        };
        self.inner = self.inner.config(config);
        self
    }

    /// Hand the drawer the app's open flag. The default is `true`: a mounted
    /// drawer is an open one, which is the shape `show_modal` pushes.
    pub fn open(mut self, open: bool) -> Self {
        self.inner = self.inner.open(open);
        self
    }

    /// The accessibility label the panel is announced with (`ariaLabel`).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.inner = self.inner.label(label);
        self
    }

    /// Whether Escape, a backdrop click and a back press close it (default
    /// `true`).
    pub fn dismissable(mut self, dismissable: bool) -> Self {
        let config = self.inner.config.dismissable(dismissable);
        self.inner = self.inner.config(config);
        self
    }

    /// Replace the panel's entrance and exit ramps (default: the seam's
    /// [`SPRING_PANEL`](crate::tokens::motion::SPRING_PANEL) in, a short eased
    /// ramp out).
    pub fn ramps(mut self, enter: Ramp, exit: Ramp) -> Self {
        let config = self.inner.config.ramps(enter, exit);
        self.inner = self.inner.config(config);
        self
    }

    /// Set the dismiss-requested callback: Escape, a backdrop click, or a back
    /// press. Reports `false`, once per dismissal.
    pub fn on_open_change<F: Fn(&mut State, bool) + 'static>(mut self, on_open_change: F) -> Self {
        self.inner = self
            .inner
            .on_dismiss(move |state| on_open_change(state, false));
        self
    }

    /// Set the exit-settled callback — state-free, fired from the paint that
    /// finishes the exit a dismissal staged. [`crate::overlay::show_modal`]
    /// installs the navigator's own pop here.
    pub fn on_close<F: Fn() + 'static>(mut self, on_close: F) -> Self {
        self.inner = self.inner.on_close(on_close);
        self
    }

    /// The edge this drawer is pinned to.
    pub fn mounted_edge(&self) -> ModalEdge {
        self.edge.get()
    }
}

impl<State: 'static> View<State> for DrawerView<State> {
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

impl<State: 'static> ModalContent<State> for DrawerView<State> {
    fn modal_dismissable(&self) -> bool {
        self.inner.config.dismissable
    }
}

// ---- The panel -------------------------------------------------------------

/// The drawer's own surface: `bg-background`, a shadow, and one hairline on the
/// edge facing the page.
struct DrawerPanelView<State: 'static> {
    content: AnyView<State>,
    edge: EdgeHandle,
}

/// The retained widget for a drawer panel.
pub struct DrawerPanelWidget {
    content: ChildPod,
    edge: ModalEdge,
}

impl DrawerPanelWidget {
    /// The edge this panel is pinned to.
    pub fn edge(&self) -> ModalEdge {
        self.edge
    }

    /// The hairline the panel draws on the edge facing the page — `border-l` on
    /// a right drawer, `border-r` on a left one.
    fn inner_rule(&self, size: Size) -> (Point, Size) {
        let w = style::BORDER_WIDTH;
        match self.edge {
            ModalEdge::Right => (Point::ORIGIN, Size::new(w, size.height)),
            ModalEdge::Left => (Point::new(size.width - w, 0.0), Size::new(w, size.height)),
            ModalEdge::Bottom => (Point::ORIGIN, Size::new(size.width, w)),
            ModalEdge::Top => (Point::new(0.0, size.height - w), Size::new(size.width, w)),
        }
    }
}

impl<State: 'static> View<State> for DrawerPanelView<State> {
    type Element = DrawerPanelWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> DrawerPanelWidget {
        DrawerPanelWidget {
            content: build_child(&self.content, ctx),
            edge: self.edge.get(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut DrawerPanelWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(&prev.content, &self.content, &mut element.content, ctx);
        if element.edge != self.edge.get() {
            element.edge = self.edge.get();
            flags |= ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut DrawerPanelWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.content, &mut element.content, ctx);
    }
}

impl Widget for DrawerPanelWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // The host has already resolved the drawer's column: a fixed width
        // capped at 85% of the area, the full height. The panel takes all of it
        // and hands the same box to its content.
        let size = bc.max();
        self.content.layout_child(ctx, &BoxConstraints::tight(size));
        self.content.set_origin(Point::ORIGIN);
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let chrome = resolve_panel(theme);
        let surface = match theme {
            Some(theme) => theme.scheme().surface,
            None => crate::BEUI_LIGHT.background,
        };
        let size = ctx.size();
        // The drawer is flush against a window edge, so its own corners are
        // square and the shadow is what separates it from the page.
        scene.draw_shadow(
            Point::new(ctx.origin().x, ctx.origin().y + PANEL_SHADOW_Y_OFFSET),
            size,
            0.0,
            PANEL_SHADOW_BLUR,
            style::with_alpha(Color::BLACK, PANEL_SHADOW_ALPHA),
        );
        scene.fill_rect(ctx.origin(), size, surface);
        let (at, rule) = self.inner_rule(size);
        scene.fill_rect(ctx.origin() + at.to_vec2(), rule, chrome.border);
        self.content.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        route_event_single(&mut self.content, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.content.semantics_child(ctx);
    }

    visit_children!(content);
}

/// The panel's resting column inside an `area`-sized host, for a caller sizing
/// its own content against it: [`DRAWER_WIDTH`] capped at
/// [`DRAWER_WIDTH_FRACTION`] of the area, the full height.
pub fn drawer_rect(area: Size, side: DrawerSide) -> Rect {
    let width = DRAWER_WIDTH.min(area.width * DRAWER_WIDTH_FRACTION);
    let x0 = match side {
        DrawerSide::Left => 0.0,
        DrawerSide::Right => area.width - width,
    };
    Rect::new(x0, 0.0, x0 + width, area.height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::popover::tests::{Recorder, WINDOW, escape, ft_ms, light, pointer};
    use crate::overlay::ModalMount;
    use frust::SizedBox;
    use frust::authoring::PointerPhase;
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
        side: DrawerSide,
        dismissable: bool,
    }

    impl Harness {
        fn new(side: DrawerSide) -> Self {
            let mut h = Harness {
                root: RenderRoot::new(),
                state: App {
                    open: true,
                    ..App::default()
                },
                tcx: TextContext::new(),
                side,
                dismissable: true,
            };
            h.root.set_theme(Box::new(light()));
            h.frame(0.0);
            h
        }

        fn frame(&mut self, ms: f64) {
            let side = self.side;
            let dismissable = self.dismissable;
            let mut logic = move |s: &mut App| {
                frust::stack().child(
                    drawer(SizedBox(None, None))
                        .side(side)
                        .open(s.open)
                        .label("Filters")
                        .dismissable(dismissable)
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

        /// The drawer's resting column.
        fn column(&self) -> Rect {
            drawer_rect(WINDOW, self.side)
        }
    }

    // ---- Geometry ---------------------------------------------------------

    #[test]
    fn the_column_is_the_upstream_width_capped_at_its_own_viewport_share() {
        let wide = drawer_rect(Size::new(1200.0, 800.0), DrawerSide::Right);
        assert_eq!(wide.width(), DRAWER_WIDTH, "`w-80` on a desktop window");
        assert_eq!(wide.x1, 1200.0, "flush to the trailing edge");
        assert_eq!(wide.height(), 800.0, "`inset-y-0`");

        let narrow = drawer_rect(Size::new(320.0, 640.0), DrawerSide::Left);
        assert_eq!(
            narrow.width(),
            320.0 * DRAWER_WIDTH_FRACTION,
            "`max-w-[85vw]` binds on a phone-width window"
        );
        assert_eq!(narrow.x0, 0.0, "flush to the leading edge");
    }

    #[test]
    fn the_host_places_the_panel_exactly_on_that_column() {
        let mut h = Harness::new(DrawerSide::Right);
        h.frame(0.0);
        h.frame(2_000.0);
        let rec = h.paint_at(2_000.0);
        let surface = light().scheme().surface;
        let (origin, size, _) = *rec
            .rects
            .iter()
            .find(|(_, _, c)| *c == surface)
            .expect("the drawer's surface");
        assert_eq!(Rect::from_origin_size(origin, size), h.column());
    }

    #[test]
    fn a_left_drawer_rules_its_trailing_edge_and_a_right_one_its_leading_edge() {
        for (side, expect_leading) in [(DrawerSide::Right, true), (DrawerSide::Left, false)] {
            let mut h = Harness::new(side);
            h.frame(0.0);
            h.frame(2_000.0);
            let rec = h.paint_at(2_000.0);
            let border = light().scheme().outline_variant;
            let (origin, size, _) = *rec
                .rects
                .iter()
                .find(|(_, s, c)| *c == border && s.width == style::BORDER_WIDTH)
                .expect("the inner rule");
            assert_eq!(size.height, WINDOW.height, "the rule runs the full height");
            let column = h.column();
            if expect_leading {
                assert_eq!(origin.x, column.x0);
            } else {
                assert_eq!(origin.x, column.x1 - style::BORDER_WIDTH);
            }
        }
    }

    // ---- The slide --------------------------------------------------------

    #[test]
    fn the_panel_slides_its_own_width_in_from_the_edge_it_is_pinned_to() {
        let mut h = Harness::new(DrawerSide::Right);
        let first = h.paint_at(0.0);
        let surface = light().scheme().surface;
        let (start, _, _) = *first
            .rects
            .iter()
            .find(|(_, _, c)| *c == surface)
            .expect("the drawer's surface");
        // The paint records the panel's *resting* rect — the slide is a scene
        // transform, which is the host's documented hit-test contract — so the
        // travel is asserted through the recorded transform instead.
        assert_eq!(start.x, h.column().x0);
        assert!(
            first.transforms.iter().any(|t| t.0.abs() > 1.0),
            "the first frame is translated a long way off the edge: {:?}",
            first.transforms
        );
        h.frame(0.0);
        h.frame(2_000.0);
        let settled = h.paint_at(2_000.0);
        assert!(
            settled.transforms.iter().all(|t| t.0.abs() < 0.5),
            "a settled drawer sits on its column: {:?}",
            settled.transforms
        );
    }

    #[test]
    fn the_scrim_fades_over_the_drawers_own_slower_ramp() {
        assert_eq!(
            drawer_config(ModalEdge::Right).scrim_fade,
            DRAWER_SCRIM_FADE
        );
        let mut h = Harness::new(DrawerSide::Right);
        let rec = h.paint_at(0.0);
        let scrim = crate::overlay::scrim(Some(&light()));
        assert!(
            rec.rects.iter().any(|(o, s, c)| *o == Point::ZERO
                && *s == WINDOW
                && c.components[3] < scrim.components[3]),
            "the scrim starts transparent and fades up"
        );
    }

    // ---- Dismissal --------------------------------------------------------

    #[test]
    fn a_backdrop_click_and_escape_each_close_it_once() {
        let mut h = Harness::new(DrawerSide::Right);
        h.frame(0.0);
        h.frame(2_000.0);
        // Press and release on the backdrop — upstream's backdrop is a button,
        // so it takes a whole click rather than a bare press.
        h.event(pointer(PointerPhase::Down, 10.0, 10.0));
        assert!(h.state.opens.is_empty(), "a press alone is not a click");
        h.event(pointer(PointerPhase::Up, 10.0, 10.0));
        assert_eq!(h.state.opens, vec![false]);

        let mut h = Harness::new(DrawerSide::Right);
        h.frame(0.0);
        h.frame(2_000.0);
        h.event(pointer(PointerPhase::Down, 10.0, 10.0));
        h.event(escape());
        assert_eq!(h.state.opens, vec![false], "reported once");
    }

    #[test]
    fn a_non_dismissable_drawer_answers_neither() {
        let mut h = Harness::new(DrawerSide::Right);
        h.dismissable = false;
        h.frame(0.0);
        h.frame(2_000.0);
        h.event(pointer(PointerPhase::Down, 10.0, 10.0));
        h.event(pointer(PointerPhase::Up, 10.0, 10.0));
        h.event(escape());
        assert!(h.state.opens.is_empty());
    }

    #[test]
    fn the_exit_completes_before_the_panel_stops_painting() {
        let mut h = Harness::new(DrawerSide::Right);
        h.frame(0.0);
        h.frame(2_000.0);
        h.state.open = false;
        h.frame(2_016.0);
        let surface = light().scheme().surface;
        let mid = h.paint_at(2_100.0);
        assert!(
            mid.rects.iter().any(|(_, _, c)| *c == surface),
            "the panel is still on screen, sliding out"
        );
        assert!(
            mid.transforms.iter().any(|t| t.0.abs() > 1.0),
            "and is translated off its column"
        );
        let done = h.paint_at(5_000.0);
        assert!(
            !done.rects.iter().any(|(_, _, c)| *c == surface),
            "a settled-closed drawer paints nothing"
        );
    }

    #[test]
    fn a_drawer_is_a_pushable_modal_component() {
        // The whole point of the thin wrapper: `show_modal` is generic over
        // `ModalContent`, and a drawer satisfies it without any per-component
        // plumbing.
        fn assert_pushable<V: ModalContent<App>>(view: &V) -> bool {
            view.modal_dismissable()
        }
        let view = drawer::<App, _>(SizedBox(None, None));
        assert!(assert_pushable(&view));
        assert!(!assert_pushable(
            &drawer::<App, _>(SizedBox(None, None)).dismissable(false)
        ));
    }

    #[test]
    fn the_upstream_side_pair_maps_onto_the_hosts_own_edges() {
        assert_eq!(DrawerSide::default(), DrawerSide::Right);
        assert_eq!(DrawerSide::Left.edge(), ModalEdge::Left);
        assert_eq!(DrawerSide::Right.edge(), ModalEdge::Right);
        let view = drawer::<App, _>(SizedBox(None, None)).edge(ModalEdge::Top);
        assert_eq!(view.mounted_edge(), ModalEdge::Top);
        assert!(matches!(
            drawer_config(ModalEdge::Top).mount,
            ModalMount::Edge(ModalEdge::Top)
        ));
    }
}
