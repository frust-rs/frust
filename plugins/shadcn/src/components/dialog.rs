//! `dialog`: shadcn's modal dialog — a `bg-black/50` scrim and a centred
//! `rounded-lg border bg-background p-6 shadow-lg` panel.
//!
//! Source: `apps/v4/registry/new-york-v4/ui/dialog.tsx` (shadcn/ui v4, rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved 2026-08-17) — a Radix
//! `Dialog` whose `Overlay` is `fixed inset-0 z-50 bg-black/50` and whose
//! `Content` is `fixed top-[50%] left-[50%] … w-full max-w-[calc(100%-2rem)]
//! sm:max-w-lg gap-4 rounded-lg border bg-background p-6 shadow-lg`, plus the
//! `showCloseButton` X at `absolute top-4 right-4`.
//!
//! # Anatomy
//!
//! The chrome — scrim, panel, border, shadow, entrance, close X, and every
//! dismiss gesture — is [`crate::overlay::modal`]'s, configured here; this
//! module contributes the panel's own metrics and its slot vocabulary. Upstream
//! `DialogHeader`/`DialogTitle`/`DialogDescription`/`DialogFooter` are plain
//! `div`s with a class list, so they port as **child-composing functions** —
//! [`dialog_header`], [`dialog_title`], [`dialog_description`],
//! [`dialog_footer`] — the same shape [`crate::card`]'s slots take, rather than
//! as builder methods:
//!
//! ```ignore
//! dialog(vec![
//!     dialog_header(vec![
//!         dialog_title("Delete project?"),
//!         dialog_description("This cannot be undone."),
//!     ]),
//!     dialog_footer(vec![cancel_button, confirm_button]),
//! ])
//! ```
//!
//! `DialogTrigger`/`DialogPortal`/`DialogClose` have no port: the trigger is
//! whatever view the app calls [`show_dialog`] from, the portal is the
//! navigator page itself, and close is the panel's own X (or any action that
//! pops).
//!
//! # Mounting
//!
//! [`show_dialog`] pushes the dialog as a transparent navigator page — the
//! primary path, `frust_material::dialog`'s architecture exactly. The view also
//! works as the top child of a full-area [`frust::Stack`]; see
//! [`crate::overlay`]'s mounting note for both.

use std::rc::Rc;

use frust::authoring::{AnyView, BuildCtx, ChangeFlags, View};
use frust::{EdgeInsets, NavigatorController, PopResult};

use crate::overlay::modal::MAX_WIDTH_LG;
use crate::overlay::{
    ModalConfig, ModalContent, ModalView, ModalWidget, modal, panel_description, panel_title,
    stack_slots, trailing_row,
};

/// `p-6` — the panel's own padding.
const PANEL_PAD: f64 = 24.0;
/// `gap-4` — the gap between the panel's slots.
const SLOT_GAP: f64 = 16.0;
/// `gap-2` — the gap between a header's title and description.
const HEADER_GAP: f64 = 8.0;
/// `gap-2` — the gap between footer buttons.
const FOOTER_GAP: f64 = 8.0;
/// `text-lg` — the dialog title's type size, in logical px.
const TITLE_SIZE: f64 = 18.0;

/// The dialog panel's chrome: centred, capped at `sm:max-w-lg`, with the close
/// X shown (upstream's `showCloseButton` default).
fn config() -> ModalConfig {
    ModalConfig::centered(MAX_WIDTH_LG).close_button(true)
}

/// Create a dialog whose panel stacks `children` `gap-4` apart inside `p-6`.
///
/// Typically `children` is a [`dialog_header`] and a [`dialog_footer`] with
/// arbitrary content between them; see the [module docs](self).
pub fn dialog<State: 'static>(children: Vec<AnyView<State>>) -> DialogView<State> {
    DialogView {
        inner: modal(
            stack_slots(children, SLOT_GAP, EdgeInsets::all(PANEL_PAD)),
            config(),
        ),
    }
}

/// A header slot: a `flex flex-col gap-2` stack (title, description).
pub fn dialog_header<State: 'static>(children: Vec<AnyView<State>>) -> AnyView<State> {
    stack_slots(children, HEADER_GAP, EdgeInsets::all(0.0))
}

/// A title row: `text-lg leading-none font-semibold`.
pub fn dialog_title<State: 'static>(title: impl Into<String>) -> AnyView<State> {
    panel_title(title, TITLE_SIZE)
}

/// A description row: `text-sm text-muted-foreground`.
pub fn dialog_description<State: 'static>(description: impl Into<String>) -> AnyView<State> {
    panel_description(description)
}

/// A footer slot: a trailing-aligned `gap-2` row of actions
/// (`sm:flex-row sm:justify-end` — see [`trailing_row`]'s breakpoint note).
pub fn dialog_footer<State: 'static>(children: Vec<AnyView<State>>) -> AnyView<State> {
    trailing_row(children, FOOTER_GAP, EdgeInsets::all(0.0))
}

/// A declarative shadcn dialog. See the [module docs](self).
pub struct DialogView<State: 'static> {
    inner: ModalView<State>,
}

impl<State: 'static> DialogView<State> {
    /// Label the dialog's accessibility node — its title text, which the
    /// composed [`dialog_title`] view cannot report on the container's behalf.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.inner = self.inner.label(label);
        self
    }

    /// Show or hide the `top-4 right-4` close X (upstream's `showCloseButton`,
    /// default `true`).
    pub fn close_button(mut self, close_button: bool) -> Self {
        self.inner.config = self.inner.config.close_button(close_button);
        self
    }

    /// Set the dismiss callback — a scrim tap, the close X, or Escape.
    /// [`show_dialog`] wires this to `controller.pop()`.
    pub fn on_dismiss<F: Fn(&mut State) + 'static>(mut self, on_dismiss: F) -> Self {
        self.inner = self.inner.on_dismiss(on_dismiss);
        self
    }
}

impl<State: 'static> View<State> for DialogView<State> {
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

impl<State: 'static> ModalContent<State> for DialogView<State> {
    fn on_modal_dismiss(mut self, on_dismiss: Rc<dyn Fn(&mut State)>) -> Self {
        self.inner = self.inner.on_modal_dismiss(on_dismiss);
        self
    }
}

/// Push `build`'s dialog as a transparent navigator page and register
/// `on_result` for the value it pops with.
///
/// Dismissal (scrim tap, close X, Escape) is wired to `controller.pop()` — an
/// empty [`PopResult`]; an action button pops with a value through
/// `controller.pop_with_result(..)`. See [`crate::overlay::show_modal`], which
/// this is a typed wrapper over.
///
/// ```ignore
/// show_dialog(
///     &state.nav,
///     || dialog(vec![dialog_header(vec![dialog_title("Delete?")])]),
///     |state: &mut State, result: PopResult| {
///         state.confirmed = result.take::<bool>().unwrap_or(false);
///     },
/// );
/// ```
pub fn show_dialog<State, B, R>(controller: &NavigatorController<State>, build: B, on_result: R)
where
    State: 'static,
    B: Fn() -> DialogView<State> + 'static,
    R: Fn(&mut State, PopResult) + 'static,
{
    crate::overlay::show_modal(controller, build, on_result);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay::modal::tests::{Block, Recorder, WINDOW, escape, pointer};
    use crate::style::spacing;
    use frust::authoring::{
        BoxConstraints, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point,
        PointerPhase, Rect, Size, Widget, any, text::TextContext,
    };
    use frust::authoring::{BuildCtx as Bc, ChangeFlags as Cf, ErasedCallback};
    use frust::{Brightness, FrameTime, NavigatorView, TransitionSpec};
    use frust_core::RenderRoot;
    use frust_widgets::navigator;
    use std::any::Any;

    /// The app-logic closure a [`NavHarness`] rebuilds through.
    type NavLogic = Box<dyn FnMut(&mut NavState) -> NavigatorView<NavState>>;

    fn build<S: 'static>(view: &DialogView<S>) -> ModalWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut Bc::new(&mut counter))
    }

    fn layout(w: &mut ModalWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::tight(WINDOW))
    }

    fn sample() -> DialogView<()> {
        dialog(vec![
            dialog_header(vec![
                dialog_title("Delete project?"),
                dialog_description("This cannot be undone."),
            ]),
            dialog_footer(vec![any(Block(Size::new(80.0, 36.0)))]),
        ])
    }

    #[test]
    fn the_panel_is_centred_capped_and_padded_by_six() {
        let mut w = build(&sample());
        let size = layout(&mut w);
        assert_eq!(size, WINDOW, "the host fills its area");
        let panel = w.panel_rect();
        assert!(panel.width() <= MAX_WIDTH_LG);
        assert!((panel.center().x - WINDOW.width / 2.0).abs() < 1e-9);
        // The composed stack sits inside the panel, `p-6` in on every side.
        let content = {
            let mut origin = None;
            Widget::visit_children(&w, &mut |pod| origin = Some(pod.origin()));
            origin.expect("the composed content pod")
        };
        assert_eq!(content, panel.origin());
        assert!(
            panel.height() > 2.0 * PANEL_PAD,
            "the header/footer stack has real height"
        );
        assert_eq!(spacing(6.0), PANEL_PAD, "`p-6` is six spacing steps");
    }

    #[test]
    fn the_panel_paints_shadcn_chrome_and_a_close_x() {
        let theme = crate::theme().with_brightness(Brightness::Light);
        let mut w = build(&sample());
        let size = layout(&mut w);
        // Two frames: the first seeds the entrance ramp (everything composites
        // at alpha 0), the second is past its 200ms and paints at rest.
        let paint = |w: &mut ModalWidget, ms: u64| {
            let mut rec = Recorder::default();
            let mut ctx = PaintCtx::for_test(Point::ORIGIN, size, FrameTime::from_nanos(ms))
                .with_theme(&theme as &dyn Any);
            w.paint(&mut ctx, &mut rec);
            rec
        };
        paint(&mut w, 0);
        let rec = paint(&mut w, 1_000_000_000);
        // Scrim, then the panel fill, its border, and `shadow-lg`.
        assert_eq!(rec.rects[0].2, theme.scheme().scrim);
        assert_eq!(rec.paths[0].2, theme.scheme().surface);
        assert!(
            rec.strokes
                .iter()
                .any(|(_, w, c)| *w == crate::style::BORDER_WIDTH && *c == theme.scheme().outline)
        );
        assert_eq!(rec.shadows[0].3, crate::style::SHADOW_LG.std_dev);
        // The close X: two lucide strokes at 70% of the foreground ink.
        let x_alpha = crate::style::with_alpha(theme.scheme().on_surface, 0.7);
        assert!(rec.strokes.iter().any(|(_, _, c)| *c == x_alpha));
        assert!(rec.glyphs >= 2, "title and description shaped runs");
    }

    #[test]
    fn the_close_button_can_be_turned_off() {
        let mut w = build(&sample().close_button(false));
        layout(&mut w);
        assert!(w.close_rect().is_none());
    }

    // --- Navigator integration, mirroring `frust_material::dialog`'s suite. ---

    #[derive(Default)]
    struct NavState {
        results: Vec<Option<bool>>,
    }

    /// A fixed-size action that pops with `true` on release inside it.
    struct TapView {
        size: Size,
        on_tap: Rc<dyn Fn(&mut NavState)>,
    }

    /// The retained half of [`TapView`].
    struct TapWidget {
        size: Size,
        on_tap: ErasedCallback,
        captured: bool,
    }

    impl View<NavState> for TapView {
        type Element = TapWidget;
        fn build(&self, _ctx: &mut Bc<'_>) -> TapWidget {
            TapWidget {
                size: self.size,
                on_tap: frust::authoring::erase_callback(&self.on_tap),
                captured: false,
            }
        }
        fn rebuild(&self, _prev: &Self, element: &mut TapWidget, _ctx: &mut Bc<'_>) -> Cf {
            element.on_tap = frust::authoring::erase_callback(&self.on_tap);
            Cf::NONE
        }
    }

    impl Widget for TapWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(self.size)
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
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
                PointerPhase::Up => {
                    if self.captured
                        && Rect::from_origin_size(Point::ORIGIN, self.size).contains(p.position)
                    {
                        (self.on_tap)(ctx);
                    }
                    self.captured = false;
                    EventResult::Handled
                }
                _ => {
                    self.captured &= p.phase != PointerPhase::Cancel;
                    EventResult::Handled
                }
            }
        }
    }

    struct NavHarness {
        root: RenderRoot<NavState, NavigatorView<NavState>>,
        state: NavState,
        tcx: TextContext,
        controller: NavigatorController<NavState>,
        logic: NavLogic,
    }

    impl NavHarness {
        fn new() -> Self {
            let controller: NavigatorController<NavState> = NavigatorController::new();
            let ctrl = controller.clone();
            let mut h = NavHarness {
                root: RenderRoot::new(),
                state: NavState::default(),
                tcx: TextContext::new(),
                controller,
                logic: Box::new(move |_: &mut NavState| {
                    navigator(&ctrl, || any::<NavState, _>(Block(WINDOW)))
                }),
            };
            h.root.set_theme(Box::new(crate::theme()));
            h.pass();
            h
        }

        fn pass(&mut self) {
            self.root.rebuild(&mut self.logic, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
        }

        /// Settle the modal's own entrance so the panel is at rest.
        fn settle(&mut self) {
            self.root.paint(&mut Recorder::default(), FrameTime::ZERO);
            self.root.paint(
                &mut Recorder::default(),
                FrameTime::from_nanos(1_000_000_000),
            );
            self.pass();
        }

        fn event(&mut self, event: &InputEvent) {
            self.root.event(&mut self.state, event);
        }

        /// Flush the navigator's pop-result callback (delivered on the event
        /// pass after the pop's rebuild).
        fn flush(&mut self) {
            self.pass();
            self.event(&pointer(PointerPhase::Move, 2.0, 2.0));
        }
    }

    #[test]
    fn a_scrim_tap_pops_the_dialog_with_an_empty_result() {
        let mut h = NavHarness::new();
        show_dialog(
            &h.controller,
            || dialog(vec![dialog_header(vec![dialog_title("Confirm")])]),
            |state: &mut NavState, result: PopResult| {
                state.results.push(result.take::<bool>());
            },
        );
        h.pass();
        h.settle();
        h.event(&pointer(PointerPhase::Down, 5.0, 5.0));
        h.event(&pointer(PointerPhase::Up, 5.0, 5.0));
        h.flush();
        assert_eq!(h.state.results, vec![None]);
    }

    #[test]
    fn an_action_pops_with_a_value_and_the_panel_swallows_a_background_press() {
        // A footer-only dialog, so the panel's geometry is exact without
        // depending on shaped text: `p-6` + a 36px action row.
        const ACTION: Size = Size::new(80.0, 36.0);
        let mut h = NavHarness::new();
        let action_ctrl = h.controller.clone();
        h.controller.push_transparent_for_result(
            move || {
                let c = action_ctrl.clone();
                let tap = TapView {
                    size: ACTION,
                    on_tap: Rc::new(move |_s: &mut NavState| {
                        c.pop_with_result(PopResult::of(true))
                    }),
                };
                any::<NavState, _>(dialog(vec![dialog_footer(vec![any(tap)])]))
            },
            TransitionSpec::NONE,
            |state: &mut NavState, result: PopResult| {
                state.results.push(result.take::<bool>());
            },
        );
        h.pass();
        h.settle();

        // `max-w-[calc(100%-2rem)]` bites at this width, so the panel is
        // `WINDOW.width - 32` wide and `2 * p-6 + 36` tall, centred.
        let panel_w = WINDOW.width - 32.0;
        let panel_h = 2.0 * PANEL_PAD + ACTION.height;
        let panel = Rect::from_origin_size(
            Point::new(
                (WINDOW.width - panel_w) / 2.0,
                (WINDOW.height - panel_h) / 2.0,
            ),
            Size::new(panel_w, panel_h),
        );

        // A press on the panel background (the footer row's leading spacer) is
        // swallowed and pops nothing.
        let background = Point::new(panel.x0 + PANEL_PAD + 10.0, panel.center().y);
        h.event(&pointer(PointerPhase::Down, background.x, background.y));
        h.event(&pointer(PointerPhase::Up, background.x, background.y));
        h.flush();
        assert!(h.state.results.is_empty(), "the panel swallowed the press");

        // The action hugs the panel's trailing edge, `p-6` in.
        let action = Point::new(panel.x1 - PANEL_PAD - ACTION.width / 2.0, panel.center().y);
        h.event(&pointer(PointerPhase::Down, action.x, action.y));
        h.event(&pointer(PointerPhase::Up, action.x, action.y));
        h.flush();
        assert_eq!(h.state.results, vec![Some(true)]);
    }

    #[test]
    fn escape_dismisses_once_the_dialog_has_focus() {
        let mut h = NavHarness::new();
        show_dialog(
            &h.controller,
            || dialog(vec![dialog_header(vec![dialog_title("Confirm")])]),
            |state: &mut NavState, result: PopResult| {
                state.results.push(result.take::<bool>());
            },
        );
        h.pass();
        h.settle();

        // No focus yet: the navigator finds no focused page pod for the key.
        h.event(&escape());
        h.flush();
        assert!(h.state.results.is_empty());

        // A press on the panel background claims focus without dismissing…
        let center = Point::new(WINDOW.width / 2.0, WINDOW.height / 2.0);
        h.event(&pointer(PointerPhase::Down, center.x, center.y));
        h.event(&pointer(PointerPhase::Up, center.x, center.y));
        h.flush();
        assert!(h.state.results.is_empty());
        // …and Escape now travels the focus chain to the dialog.
        h.event(&escape());
        h.flush();
        assert_eq!(h.state.results, vec![None]);
    }
}
