//! `CupertinoAlertDialog` (Phase 6c, PLAN.md D3/D5, task 13): the iOS alert —
//! a centered ~270pt panel with a title, optional message, and vertically
//! stacked action buttons separated by hairlines, over a dimming scrim.
//!
//! It is pushed as a **transparent** navigator page (task 06) via the
//! [`show_cupertino_alert`] convenience, entering with the
//! [`PageTransition::M3FadeThrough`](crate::PageTransition::M3FadeThrough)
//! preset (a fade is idiom-appropriate for an alert). An action tap pops the
//! page carrying the action's index as a [`PopResult`]; a scrim tap pops with
//! an empty result (dismiss). The pusher-registered `on_result` callback
//! (see [`NavigatorController::push_transparent_for_result`]) receives it.
//!
//! # Colors & metrics
//!
//! The panel corner radius is ~14pt (**community-approximate** — Apple
//! publishes no alert corner-radius spec, per this task's C13 flag). The title
//! is themed `on_surface` (iOS `label`) and the message `on_surface_variant`
//! (secondaryLabel), so both live-swap with the theme. Action labels are tinted
//! by [`CupertinoActionStyle`] from *community-measured* systemBlue/systemRed
//! (baked explicit — see [`crate::cupertino::tabbar`]'s Label color note for why
//! these accent colors don't live-swap).

use forgekit_core::accesskit::Role;
use forgekit_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, PointerPhase, SemanticsCtx, View, Widget,
};
use forgekit_text::{FontWeight, LineHeight};
use forgekit_theme::Theme;
use kurbo::{Point, Rect, Size};
use peniko::Color;

use crate::nav::navigator::{NavigatorController, PopResult};
use crate::text;
use crate::text::ThemeTextColor;

/// Panel width, in logical px.
///
/// **Community-approximate**: iOS does not publish an exact alert width; ~270pt
/// is the community-converged value (per this task's C13 flag).
const PANEL_W: f64 = 270.0;
/// Panel corner radius, in logical px (**community-approximate**, ~13-14pt — no
/// published Apple alert corner spec, per C13).
const PANEL_RADIUS: f64 = 14.0;
/// Horizontal content padding inside the panel, in logical px.
const H_PAD: f64 = 16.0;
/// Top padding above the title, in logical px.
const V_PAD_TOP: f64 = 20.0;
/// Bottom padding below the message block (above the action divider), px.
const V_PAD_BOTTOM: f64 = 20.0;
/// Gap between the title and the message, in logical px.
const TITLE_MESSAGE_GAP: f64 = 4.0;
/// Height of each stacked action row, in logical px.
///
/// **Community-approximate**: ~44pt matches the standard iOS tappable-row
/// height; no published alert-action-row spec exists.
const ACTION_H: f64 = 44.0;

/// Title type-role: SF *Headline*, 17pt Semibold (see [`super::navbar`]).
const TITLE_SIZE: f32 = 17.0;
const TITLE_LINE_HEIGHT: f32 = 22.0;
/// Message type-role: SF *Footnote*, 13pt Regular.
const MESSAGE_SIZE: f32 = 13.0;
const MESSAGE_LINE_HEIGHT: f32 = 18.0;
/// Action label type-role: SF *Body*, 17pt.
const ACTION_SIZE: f32 = 17.0;
const ACTION_LINE_HEIGHT: f32 = 22.0;

/// systemBlue — the default/cancel action tint (**community-measured**; see
/// [`super::tabbar`]'s `SYSTEM_BLUE`).
pub(crate) const SYSTEM_BLUE: Color = Color::from_rgb8(0x00, 0x7A, 0xFF);
/// systemRed — the destructive action tint (**community-measured**).
pub(crate) const SYSTEM_RED: Color = Color::from_rgb8(0xFF, 0x3B, 0x30);

/// Unthemed fallback panel fill (a theme resolves this from
/// `colors.surface_container_high`).
const PANEL_FILL: Color = Color::from_rgb8(0xF2, 0xF2, 0xF7);
/// Unthemed fallback hairline color (a theme resolves this from
/// `colors.outline_variant`, iOS separator).
const SEPARATOR: Color = Color::from_rgb8(0xC6, 0xC6, 0xC8);
/// The scrim alpha the modal dims the page below with.
///
/// **Community-approximate**: iOS composites a blurred dim behind an alert; a
/// flat ~0.2 black scrim is the closest backend-agnostic approximation.
const SCRIM_ALPHA: f32 = 0.2;
/// Hairline stroke width, logical px (see [`super::navbar`]).
const HAIRLINE_W: f64 = 1.0;

/// How a [`CupertinoAlertDialog`] action button reads and tints.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CupertinoActionStyle {
    /// A normal action — systemBlue, regular weight. The default.
    #[default]
    Default,
    /// The cancel/dismiss action — systemBlue, **bold** (iOS emphasizes the
    /// cancel action's weight).
    Cancel,
    /// A destructive action — systemRed.
    Destructive,
}

impl CupertinoActionStyle {
    /// The label color for this style.
    pub(crate) fn color(self) -> Color {
        match self {
            CupertinoActionStyle::Default | CupertinoActionStyle::Cancel => SYSTEM_BLUE,
            CupertinoActionStyle::Destructive => SYSTEM_RED,
        }
    }

    /// The label font weight for this style (cancel is bold; the rest regular).
    pub(crate) fn weight(self) -> FontWeight {
        match self {
            CupertinoActionStyle::Cancel => FontWeight::SEMI_BOLD,
            _ => FontWeight::REGULAR,
        }
    }
}

/// One alert action: its label and [`CupertinoActionStyle`].
#[derive(Clone, Debug)]
pub struct CupertinoDialogAction {
    pub label: String,
    pub style: CupertinoActionStyle,
}

/// A [`CupertinoActionStyle::Default`] action labelled `label`.
pub fn action(label: impl Into<String>) -> CupertinoDialogAction {
    CupertinoDialogAction {
        label: label.into(),
        style: CupertinoActionStyle::Default,
    }
}

impl CupertinoDialogAction {
    /// Set this action's [`CupertinoActionStyle`].
    pub fn style(mut self, style: CupertinoActionStyle) -> Self {
        self.style = style;
        self
    }
}

/// Build an action label's type-erased child view (explicit accent color +
/// weight, centered).
pub(crate) fn action_label_view<State: 'static>(a: &CupertinoDialogAction) -> AnyView<State> {
    forgekit_core::any::<State, _>(
        text(a.label.clone())
            .size(ACTION_SIZE)
            .weight(a.style.weight())
            .color(a.style.color())
            .line_height(LineHeight::Absolute(ACTION_LINE_HEIGHT)),
    )
}

/// Push a Cupertino alert onto `controller`'s stack. `on_result` receives the
/// tapped action's index (`usize`) as a [`PopResult`], or an empty result when
/// the scrim was tapped to dismiss.
pub fn show_cupertino_alert<State: 'static>(
    controller: &NavigatorController<State>,
    title: impl Into<String>,
    message: Option<String>,
    actions: Vec<CupertinoDialogAction>,
    on_result: impl Fn(&mut State, PopResult) + 'static,
) {
    let title = title.into();
    let controller_for_builder = controller.clone();
    controller.push_transparent_for_result(
        move || {
            forgekit_core::any::<State, _>(CupertinoAlertDialogView {
                title: title.clone(),
                message: message.clone(),
                actions: actions.clone(),
                controller: controller_for_builder.clone(),
            })
        },
        crate::TransitionSpec::duration(crate::PageTransition::M3FadeThrough),
        on_result,
    );
}

/// A declarative iOS alert dialog page. Usually created via
/// [`show_cupertino_alert`]; exposed so it can also be tested/embedded directly.
pub struct CupertinoAlertDialogView<State: 'static> {
    pub title: String,
    pub message: Option<String>,
    pub actions: Vec<CupertinoDialogAction>,
    pub controller: NavigatorController<State>,
}

/// The retained widget for a [`CupertinoAlertDialogView`].
pub struct CupertinoAlertDialogWidget<State: 'static> {
    title: ChildPod,
    /// The title string, retained for the semantics container's label.
    title_text: String,
    message: Option<ChildPod>,
    actions: Vec<ChildPod>,
    controller: NavigatorController<State>,
    /// Panel rect in the widget's local coordinate space (computed in layout).
    panel_rect: Rect,
    /// Each action row's rect in local coordinates (computed in layout).
    action_rects: Vec<Rect>,
    /// Armed by a `Down`, cleared on `Up`/`Cancel` (fire-on-up-inside).
    captured: bool,
}

fn title_view<State: 'static>(title: String) -> AnyView<State> {
    forgekit_core::any::<State, _>(
        text(title)
            .size(TITLE_SIZE)
            .weight(FontWeight::SEMI_BOLD)
            .themed_role(ThemeTextColor::OnSurface)
            .line_height(LineHeight::Absolute(TITLE_LINE_HEIGHT)),
    )
}

fn message_view<State: 'static>(message: String) -> AnyView<State> {
    forgekit_core::any::<State, _>(
        text(message)
            .size(MESSAGE_SIZE)
            .themed_role(ThemeTextColor::OnSurfaceVariant)
            .line_height(LineHeight::Absolute(MESSAGE_LINE_HEIGHT)),
    )
}

impl<State: 'static> View<State> for CupertinoAlertDialogView<State> {
    type Element = CupertinoAlertDialogWidget<State>;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> CupertinoAlertDialogWidget<State> {
        let title = crate::build_child(&title_view::<State>(self.title.clone()), ctx);
        let message = self
            .message
            .as_ref()
            .map(|m| crate::build_child(&message_view::<State>(m.clone()), ctx));
        let actions = self
            .actions
            .iter()
            .map(|a| crate::build_child(&action_label_view::<State>(a), ctx))
            .collect();
        CupertinoAlertDialogWidget {
            title,
            title_text: self.title.clone(),
            message,
            actions,
            controller: self.controller.clone(),
            panel_rect: Rect::ZERO,
            action_rects: Vec::new(),
            captured: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut CupertinoAlertDialogWidget<State>,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        element.controller = self.controller.clone();
        if prev.title != self.title {
            element.title_text = self.title.clone();
            flags |= crate::rebuild_child(
                &title_view::<State>(prev.title.clone()),
                &title_view::<State>(self.title.clone()),
                &mut element.title,
                ctx,
            );
        }
        // Message/action-set structural changes are not expected for a live
        // alert (its content is fixed at push time); a differing count would be
        // a rebuild against a different alert, which the navigator handles by
        // teardown+build, not in-place rebuild. Reconcile the common prefix.
        if let (Some(pm), Some(nm), Some(elem_m)) =
            (&prev.message, &self.message, element.message.as_mut())
        {
            flags |= crate::rebuild_child(
                &message_view::<State>(pm.clone()),
                &message_view::<State>(nm.clone()),
                elem_m,
                ctx,
            );
        }
        let common = prev.actions.len().min(self.actions.len());
        for i in 0..common {
            flags |= crate::rebuild_child(
                &action_label_view::<State>(&prev.actions[i]),
                &action_label_view::<State>(&self.actions[i]),
                &mut element.actions[i],
                ctx,
            );
        }
        flags
    }

    fn teardown(&self, element: &mut CupertinoAlertDialogWidget<State>, ctx: &mut BuildCtx<'_>) {
        crate::teardown_child(
            &title_view::<State>(self.title.clone()),
            &mut element.title,
            ctx,
        );
        if let (Some(m), Some(elem_m)) = (&self.message, element.message.as_mut()) {
            crate::teardown_child(&message_view::<State>(m.clone()), elem_m, ctx);
        }
        for (a, pod) in self.actions.iter().zip(element.actions.iter_mut()) {
            crate::teardown_child(&action_label_view::<State>(a), pod, ctx);
        }
    }
}

impl<State: 'static> Widget for CupertinoAlertDialogWidget<State> {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let full = bc.max();
        let content_w = PANEL_W - 2.0 * H_PAD;
        let content_bc = BoxConstraints::loose(Size::new(content_w, f64::INFINITY));

        let title_size = self.title.layout_child(ctx, &content_bc);
        let message_size = self
            .message
            .as_mut()
            .map(|m| m.layout_child(ctx, &content_bc));

        // Content block height (title [+ gap + message]) plus vertical padding.
        let mut content_h = V_PAD_TOP + title_size.height;
        if let Some(ms) = message_size {
            content_h += TITLE_MESSAGE_GAP + ms.height;
        }
        content_h += V_PAD_BOTTOM;

        let action_bc = BoxConstraints::loose(Size::new(content_w, ACTION_H));
        for pod in &mut self.actions {
            pod.layout_child(ctx, &action_bc);
        }

        let panel_h = content_h + self.actions.len() as f64 * ACTION_H;
        let panel_x = ((full.width - PANEL_W) / 2.0).max(0.0);
        let panel_y = ((full.height - panel_h) / 2.0).max(0.0);
        self.panel_rect = Rect::new(panel_x, panel_y, panel_x + PANEL_W, panel_y + panel_h);

        // Center the title (and message) horizontally within the panel.
        let title_x = panel_x + (PANEL_W - title_size.width) / 2.0;
        self.title
            .set_origin(Point::new(title_x, panel_y + V_PAD_TOP));
        if let (Some(pod), Some(ms)) = (self.message.as_mut(), message_size) {
            let msg_x = panel_x + (PANEL_W - ms.width) / 2.0;
            let msg_y = panel_y + V_PAD_TOP + title_size.height + TITLE_MESSAGE_GAP;
            pod.set_origin(Point::new(msg_x, msg_y));
        }

        // Stack action rows below the content block; center each label in its row.
        self.action_rects.clear();
        let actions_top = panel_y + content_h;
        for (i, pod) in self.actions.iter_mut().enumerate() {
            let row_y = actions_top + i as f64 * ACTION_H;
            self.action_rects.push(Rect::new(
                panel_x,
                row_y,
                panel_x + PANEL_W,
                row_y + ACTION_H,
            ));
            let label_size = pod.size();
            let lx = panel_x + (PANEL_W - label_size.width) / 2.0;
            let ly = row_y + (ACTION_H - label_size.height) / 2.0;
            pod.set_origin(Point::new(lx, ly));
        }

        bc.constrain(full)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let panel_fill = theme
            .map(|t| t.scheme().surface_container_high)
            .unwrap_or(PANEL_FILL);
        let separator = theme
            .map(|t| t.scheme().outline_variant)
            .unwrap_or(SEPARATOR);
        let scrim_base = theme.map(|t| t.scheme().scrim).unwrap_or(Color::BLACK);

        let origin = ctx.origin();
        let size = ctx.size();
        // Scrim over the whole page.
        let scrim = {
            let c = scrim_base.components;
            Color::new([c[0], c[1], c[2], SCRIM_ALPHA])
        };
        scene.fill_rect(origin, size, scrim);

        // Panel background (local rects offset by the paint origin).
        let panel_origin = Point::new(origin.x + self.panel_rect.x0, origin.y + self.panel_rect.y0);
        let panel_size = Size::new(self.panel_rect.width(), self.panel_rect.height());
        scene.fill_rounded_rect(panel_origin, panel_size, PANEL_RADIUS, panel_fill);

        // Hairlines above each action row.
        for rect in &self.action_rects {
            let y = origin.y + rect.y0 + HAIRLINE_W / 2.0;
            scene.stroke_line(
                Point::new(origin.x + rect.x0, y),
                Point::new(origin.x + rect.x1, y),
                HAIRLINE_W,
                separator,
            );
        }

        self.title.paint_child(ctx, scene);
        if let Some(m) = self.message.as_mut() {
            m.paint_child(ctx, scene);
        }
        for pod in &mut self.actions {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                self.captured = true;
                ctx.capture_pointer();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.captured = false;
                let pos = p.position;
                if let Some(idx) = self.action_rects.iter().position(|r| r.contains(pos)) {
                    // An action was tapped: pop carrying its index.
                    self.controller.pop_with_result(PopResult::of(idx));
                    ctx.request_redraw();
                } else if !self.panel_rect.contains(pos) {
                    // A scrim tap dismisses (empty result).
                    self.controller.pop();
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                self.captured = false;
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let title_label = self.title_text.clone();
        let title_pod = &self.title;
        let message = &self.message;
        let actions = &self.actions;
        ctx.push_container(
            Role::AlertDialog,
            move |node| node.set_label(title_label.as_str()),
            |ctx| {
                title_pod.semantics_child(ctx);
                if let Some(m) = message {
                    m.semantics_child(ctx);
                }
                for pod in actions {
                    pod.semantics_child(ctx);
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forgekit_core::{BuildCtx, PointerButton, PointerEvent};
    use forgekit_text::TextContext;
    use std::any::Any;

    fn alert_view() -> CupertinoAlertDialogView<()> {
        CupertinoAlertDialogView {
            title: "Delete?".into(),
            message: Some("This cannot be undone.".into()),
            actions: vec![
                action("Cancel").style(CupertinoActionStyle::Cancel),
                action("Delete").style(CupertinoActionStyle::Destructive),
            ],
            controller: NavigatorController::new(),
        }
    }

    fn build(view: &CupertinoAlertDialogView<()>) -> CupertinoAlertDialogWidget<()> {
        let mut counter = 0u64;
        View::<()>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut CupertinoAlertDialogWidget<()>, window: Size) {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), None);
        w.layout(&mut lctx, &BoxConstraints::tight(window));
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    #[test]
    fn panel_is_270_wide_and_centered() {
        let mut w = build(&alert_view());
        let window = Size::new(400.0, 800.0);
        layout(&mut w, window);
        assert_eq!(w.panel_rect.width(), PANEL_W);
        let center_x = (w.panel_rect.x0 + w.panel_rect.x1) / 2.0;
        assert!(
            (center_x - 200.0).abs() < 1e-6,
            "panel centered horizontally"
        );
    }

    #[test]
    fn one_action_row_per_action() {
        let mut w = build(&alert_view());
        layout(&mut w, Size::new(400.0, 800.0));
        assert_eq!(w.action_rects.len(), 2);
        // Rows are stacked (row 1 below row 0).
        assert!(w.action_rects[1].y0 > w.action_rects[0].y0);
    }

    // --- Full modal wiring: drive a real navigator, settle the entering
    //     transition, then tap through to the pop + on_result callback. ---

    use crate::nav::navigator::{NavigatorView, navigator};
    use forgekit_core::{FrameTime, RenderRoot, any};

    #[derive(Default)]
    struct DialogState {
        /// Total `on_result` invocations (fires on every pop, action or scrim).
        calls: u32,
        /// The action index a pop carried, if any (empty for a scrim dismiss).
        received: Option<usize>,
    }

    fn ft_secs(s: f64) -> FrameTime {
        FrameTime::from_nanos((s * 1_000_000_000.0) as u64)
    }

    const WINDOW: Size = Size::new(400.0, 800.0);

    /// A harness owning a `RenderRoot<DialogState, NavigatorView>` plus its
    /// controller, and a probe copy of the alert widget for geometry lookup.
    struct ModalHarness {
        root: RenderRoot<DialogState, NavigatorView<DialogState>>,
        controller: NavigatorController<DialogState>,
        state: DialogState,
        tcx: TextContext,
        probe: CupertinoAlertDialogWidget<()>,
    }

    impl ModalHarness {
        fn new() -> Self {
            let controller: NavigatorController<DialogState> = NavigatorController::new();
            let root: RenderRoot<DialogState, NavigatorView<DialogState>> = RenderRoot::new();
            // A probe widget with identical content discovers the action-row
            // geometry the (transparent, full-window) alert page lays out to.
            let mut probe = {
                let mut counter = 0u64;
                View::<()>::build(&alert_view(), &mut BuildCtx::new(&mut counter))
            };
            let mut tcx = TextContext::new();
            let mut lctx = LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), None);
            probe.layout(&mut lctx, &BoxConstraints::tight(WINDOW));
            ModalHarness {
                root,
                controller,
                state: DialogState::default(),
                tcx: TextContext::new(),
                probe,
            }
        }

        fn app(&self) -> impl FnMut(&mut DialogState) -> NavigatorView<DialogState> + use<> {
            let ctrl = self.controller.clone();
            move |_: &mut DialogState| navigator(&ctrl, || any(text("base").size(17.0)))
        }

        fn rebuild_layout(&mut self) {
            let mut app = self.app();
            self.root.rebuild(&mut app, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
        }

        fn show(&mut self) {
            self.rebuild_layout();
            show_cupertino_alert(
                &self.controller,
                "Delete?",
                Some("This cannot be undone.".into()),
                vec![
                    action("Cancel").style(CupertinoActionStyle::Cancel),
                    action("Delete").style(CupertinoActionStyle::Destructive),
                ],
                |s: &mut DialogState, r: PopResult| {
                    s.calls += 1;
                    if let Some(i) = r.take::<usize>() {
                        s.received = Some(i);
                    }
                },
            );
            self.rebuild_layout();
        }

        /// Paint past the entering M3FadeThrough transition, then finalize it so
        /// the navigator stops blocking input to the alert page.
        fn settle(&mut self) {
            let mut scene = crate::test_support::RecordingScene::default();
            self.root.paint(&mut scene, ft_secs(0.0));
            self.root.paint(&mut scene, ft_secs(1.0));
            self.rebuild_layout();
        }

        fn drain(&mut self) {
            self.rebuild_layout();
            // Any event pass flushes a queued on_result callback with state.
            self.root
                .event(&mut self.state, &ev(PointerPhase::Move, 1.0, 1.0));
        }

        fn tap(&mut self, x: f64, y: f64) {
            self.root
                .event(&mut self.state, &ev(PointerPhase::Down, x, y));
            self.root
                .event(&mut self.state, &ev(PointerPhase::Up, x, y));
        }

        fn action_center(&self, idx: usize) -> (f64, f64) {
            let r = self.probe.action_rects[idx];
            ((r.x0 + r.x1) / 2.0, (r.y0 + r.y1) / 2.0)
        }

        fn panel_top_center(&self) -> (f64, f64) {
            let r = self.probe.panel_rect;
            ((r.x0 + r.x1) / 2.0, r.y0 + 2.0)
        }
    }

    #[test]
    fn action_tap_pops_with_its_index() {
        let mut nav = ModalHarness::new();
        nav.show();
        nav.settle();
        let (cx, cy) = nav.action_center(1);
        nav.tap(cx, cy);
        nav.drain();
        assert_eq!(nav.state.calls, 1, "the pop fired on_result once");
        assert_eq!(
            nav.state.received,
            Some(1),
            "carrying the tapped action index"
        );
    }

    #[test]
    fn scrim_tap_pops_to_dismiss() {
        let mut nav = ModalHarness::new();
        nav.show();
        nav.settle();
        // Tap the top-left corner — well outside the centered panel.
        nav.tap(5.0, 5.0);
        nav.drain();
        assert_eq!(
            nav.state.calls, 1,
            "a scrim tap pops the alert (on_result fires)"
        );
        assert_eq!(
            nav.state.received, None,
            "a dismiss carries no action index"
        );
    }

    #[test]
    fn tap_inside_panel_but_not_an_action_is_a_noop() {
        let mut nav = ModalHarness::new();
        nav.show();
        nav.settle();
        // A tap inside the panel but above the action rows neither pops nor
        // dismisses.
        let (px, py) = nav.panel_top_center();
        nav.tap(px, py);
        nav.drain();
        assert_eq!(nav.state.calls, 0, "a non-action panel tap is a no-op");
        // The alert is still up and interactive: an action tap now fires.
        let (cx, cy) = nav.action_center(0);
        nav.tap(cx, cy);
        nav.drain();
        assert_eq!(nav.state.calls, 1, "the alert survived the no-op tap");
        assert_eq!(nav.state.received, Some(0));
    }

    #[test]
    fn destructive_action_tints_red_and_cancel_is_bold() {
        assert_eq!(CupertinoActionStyle::Destructive.color(), SYSTEM_RED);
        assert_eq!(CupertinoActionStyle::Default.color(), SYSTEM_BLUE);
        assert_eq!(CupertinoActionStyle::Cancel.weight(), FontWeight::SEMI_BOLD);
        assert_eq!(CupertinoActionStyle::Default.weight(), FontWeight::REGULAR);
    }

    #[test]
    fn semantics_is_an_alert_dialog_container() {
        fn logic(_s: &mut ()) -> CupertinoAlertDialogView<()> {
            CupertinoAlertDialogView {
                title: "Hi".into(),
                message: None,
                actions: vec![action("OK")],
                controller: NavigatorController::new(),
            }
        }
        let mut root: forgekit_core::RenderRoot<(), CupertinoAlertDialogView<()>> =
            forgekit_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(400.0, 800.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        assert!(
            update
                .nodes
                .iter()
                .any(|(_, n)| n.role() == Role::AlertDialog),
            "an AlertDialog container node is contributed"
        );
    }
}
