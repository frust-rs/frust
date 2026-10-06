//! `alert_dialog`: the dialog variant that demands an explicit answer.
//!
//! Source: `apps/v4/registry/new-york-v4/ui/alert-dialog.tsx` (shadcn/ui v4,
//! rev `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved 2026-08-17). Its
//! `Content` class list is `dialog.tsx`'s verbatim apart from the size axis
//! (`data-[size=sm]:max-w-xs` / `data-[size=default]:sm:max-w-lg`), so the port
//! is [`crate::dialog`]'s chrome with three deliberate differences, all of them
//! Radix's `AlertDialog` primitive rather than class lists:
//!
//! 1. **No scrim-tap dismissal.** Radix's `AlertDialog` does not close on an
//!    outside press; the user must choose an action.
//! 2. **No close X.** `AlertDialogContent` renders no `Close` button at all —
//!    [`alert_dialog_cancel`] and [`alert_dialog_action`] are the only ways out
//!    (plus Escape, which Radix keeps).
//! 3. **`Role::AlertDialog`**, not `Role::Dialog`.
//!
//! # Slots
//!
//! Same child-composing shape as [`crate::dialog`]'s (see its module docs), with
//! the source's own tighter header gap (`gap-1.5`). [`alert_dialog_action`] and
//! [`alert_dialog_cancel`] are *labels* for the footer's ordering convention
//! rather than widgets — upstream they are `Button`s with `variant="default"`
//! and `variant="outline"`, which an app builds with [`crate::button`] and
//! passes here.
//!
//! # Not ported
//!
//! `AlertDialogMedia` (a `size-16 rounded-md bg-muted` icon chip above the
//! title) has no port in v1: it needs a filled-chip container this module would
//! have to add, and [`crate::empty_media`]'s `Icon` variant is the nearest
//! shipped shape (`size-10 rounded-lg bg-muted`).

use std::rc::Rc;

use frust::authoring::{AnyView, BuildCtx, ChangeFlags, View};
use frust::{EdgeInsets, NavigatorController, PopResult};

use crate::overlay::modal::{MAX_WIDTH_LG, MAX_WIDTH_XS};
use crate::overlay::{
    ModalConfig, ModalContent, ModalRole, ModalView, ModalWidget, modal, panel_description,
    panel_title, stack_slots, trailing_row,
};

/// `p-6` — the panel's own padding.
const PANEL_PAD: f64 = 24.0;
/// `gap-4` — the gap between the panel's slots.
const SLOT_GAP: f64 = 16.0;
/// `gap-1.5` — the gap between a header's title and description.
const HEADER_GAP: f64 = 6.0;
/// `gap-2` — the gap between footer buttons.
const FOOTER_GAP: f64 = 8.0;
/// `text-lg` — the title's type size, in logical px.
const TITLE_SIZE: f64 = 18.0;

/// `AlertDialogContent`'s `size` prop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AlertDialogSize {
    /// `data-[size=default]:sm:max-w-lg`.
    #[default]
    Default,
    /// `data-[size=sm]:max-w-xs`.
    Sm,
}

impl AlertDialogSize {
    /// This size's panel width cap, in logical px.
    pub fn max_width(self) -> f64 {
        match self {
            AlertDialogSize::Default => MAX_WIDTH_LG,
            AlertDialogSize::Sm => MAX_WIDTH_XS,
        }
    }
}

/// The alert-dialog panel's chrome: [`crate::dialog`]'s, minus the scrim
/// dismissal and the close X, reported as an alert.
fn config(size: AlertDialogSize) -> ModalConfig {
    ModalConfig::centered(size.max_width())
        .scrim_dismiss(false)
        .role(ModalRole::AlertDialog)
}

/// Create an alert dialog whose panel stacks `children` `gap-4` apart inside
/// `p-6`.
///
/// The list takes any iterator of one [`View`] type, so a homogeneous list
/// needs no `any(..)`; a mixed list keeps `vec![any(..), ..]`.
pub fn alert_dialog<State: 'static, V: View<State>>(
    children: impl IntoIterator<Item = V>,
) -> AlertDialogView<State> {
    AlertDialogView {
        inner: modal(
            stack_slots(children, SLOT_GAP, EdgeInsets::all(PANEL_PAD)),
            config(AlertDialogSize::default()),
        ),
        size: AlertDialogSize::default(),
    }
}

/// A header slot: a `gap-1.5` stack (title, description).
///
/// The list takes any iterator of one [`View`] type, so a homogeneous list
/// needs no `any(..)`; a mixed list keeps `vec![any(..), ..]`.
pub fn alert_dialog_header<State: 'static, V: View<State>>(
    children: impl IntoIterator<Item = V>,
) -> AnyView<State> {
    stack_slots(children, HEADER_GAP, EdgeInsets::all(0.0))
}

/// A title row: `text-lg font-semibold`.
pub fn alert_dialog_title<State: 'static>(title: impl Into<String>) -> AnyView<State> {
    panel_title(title, TITLE_SIZE)
}

/// A description row: `text-sm text-muted-foreground`.
pub fn alert_dialog_description<State: 'static>(description: impl Into<String>) -> AnyView<State> {
    panel_description(description)
}

/// A footer slot: a trailing-aligned `gap-2` row, cancel first, action last
/// (see [`trailing_row`]'s breakpoint note).
///
/// The source's `sm` size lays its footer out as a two-column grid; this port
/// keeps the row at every size — a documented simplification, since the row
/// already trails both buttons together.
///
/// The list takes any iterator of one [`View`] type, so a homogeneous list
/// needs no `any(..)`; a mixed list keeps `vec![any(..), ..]`.
pub fn alert_dialog_footer<State: 'static, V: View<State>>(
    children: impl IntoIterator<Item = V>,
) -> AnyView<State> {
    trailing_row(children, FOOTER_GAP, EdgeInsets::all(0.0))
}

/// The confirming action, for [`alert_dialog_footer`]'s trailing slot — a
/// pass-through that names the convention (upstream: a `default`-variant
/// [`crate::button`]).
pub fn alert_dialog_action<State: 'static>(action: impl View<State>) -> AnyView<State> {
    AnyView::new(action)
}

/// The dismissing action, for [`alert_dialog_footer`]'s leading slot — a
/// pass-through that names the convention (upstream: an `outline`-variant
/// [`crate::button`]).
pub fn alert_dialog_cancel<State: 'static>(cancel: impl View<State>) -> AnyView<State> {
    AnyView::new(cancel)
}

/// A declarative shadcn alert dialog. See the [module docs](self).
pub struct AlertDialogView<State: 'static> {
    inner: ModalView<State>,
    size: AlertDialogSize,
}

impl<State: 'static> AlertDialogView<State> {
    /// Set the panel's size (its `max-w-*` cap).
    pub fn size(mut self, size: AlertDialogSize) -> Self {
        self.size = size;
        self.inner.config = config(size);
        self
    }

    /// Label the alert dialog's accessibility node — its title text.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.inner = self.inner.label(label);
        self
    }

    /// Set the **unstaged** dismiss callback — Escape only, since neither a
    /// scrim tap nor a close X exists here — used when no
    /// [`on_close`](Self::on_close) hook is wired (see
    /// [`crate::overlay::modal`]'s exit-motion contract).
    /// [`show_alert_dialog`] wires both.
    pub fn on_dismiss<F: Fn(&mut State) + 'static>(mut self, on_dismiss: F) -> Self {
        self.inner = self.inner.on_dismiss(on_dismiss);
        self
    }

    /// Set the state-free close hook fired once the exit ramp has settled — an
    /// Escape on a `Stack`-mounted alert dialog fades and zooms it back out
    /// before this runs. [`show_alert_dialog`] wires it to `controller.pop()`.
    pub fn on_close<F: Fn() + 'static>(mut self, on_close: F) -> Self {
        self.inner = self.inner.on_close(on_close);
        self
    }
}

impl<State: 'static> View<State> for AlertDialogView<State> {
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

impl<State: 'static> ModalContent<State> for AlertDialogView<State> {
    fn on_modal_dismiss(mut self, on_dismiss: Rc<dyn Fn(&mut State)>) -> Self {
        self.inner = self.inner.on_modal_dismiss(on_dismiss);
        self
    }
}

/// Push `build`'s alert dialog as a transparent navigator page and register
/// `on_result` for the value it pops with — [`crate::show_dialog`]'s shape, over
/// the alert chrome.
pub fn show_alert_dialog<State, B, R>(
    controller: &NavigatorController<State>,
    build: B,
    on_result: R,
) where
    State: 'static,
    B: Fn() -> AlertDialogView<State> + 'static,
    R: Fn(&mut State, PopResult) + 'static,
{
    crate::overlay::show_modal(controller, build, on_result);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay::modal::tests::{Block, Recorder, WINDOW, escape, pointer};
    use frust::FrameTime;
    use frust::authoring::{
        BoxConstraints, EventCtx, InputEvent, LayoutCtx, PaintCtx, Point, PointerPhase, Role, Size,
        Widget, any, text::TextContext,
    };
    use frust_core::RenderRoot;
    use std::any::Any;
    use std::cell::Cell;

    #[derive(Default)]
    struct Flags {
        dismissed: u32,
    }

    fn sample() -> AlertDialogView<Flags> {
        alert_dialog(vec![
            alert_dialog_header(vec![
                alert_dialog_title("Delete project?"),
                alert_dialog_description("This cannot be undone."),
            ]),
            alert_dialog_footer(vec![
                alert_dialog_cancel(any(Block(Size::new(80.0, 36.0)))),
                alert_dialog_action(any(Block(Size::new(80.0, 36.0)))),
            ]),
        ])
        .on_dismiss(|s: &mut Flags| s.dismissed += 1)
    }

    fn build(view: &AlertDialogView<Flags>) -> ModalWidget {
        let mut counter = 0u64;
        View::<Flags>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut ModalWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::tight(WINDOW))
    }

    fn dispatch(w: &mut ModalWidget, state: &mut Flags, event: &InputEvent) {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, WINDOW);
        w.event(&mut ctx, event);
    }

    #[test]
    fn a_scrim_tap_does_not_dismiss_but_escape_does() {
        let mut w = build(&sample());
        layout(&mut w);
        let mut state = Flags::default();
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, 5.0, 5.0));
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Up, 5.0, 5.0));
        assert_eq!(
            state.dismissed, 0,
            "an alert dialog demands an explicit choice"
        );
        dispatch(&mut w, &mut state, &escape());
        assert_eq!(state.dismissed, 1);
    }

    #[test]
    fn escape_stages_the_exit_and_closes_only_at_the_settle() {
        let closed = Rc::new(Cell::new(0u32));
        let hook = closed.clone();
        let mut w = build(&sample().on_close(move || hook.set(hook.get() + 1)));
        let size = layout(&mut w);
        let paint = |w: &mut ModalWidget, ms: f64| {
            let mut ctx = PaintCtx::for_test(
                Point::ORIGIN,
                size,
                FrameTime::from_nanos((ms * 1e6) as u64),
            );
            w.paint(&mut ctx, &mut Recorder::default());
        };
        paint(&mut w, 0.0);
        paint(&mut w, 1000.0);
        assert!((w.progress() - 1.0).abs() < 1e-9);

        let mut state = Flags::default();
        dispatch(&mut w, &mut state, &escape());
        assert!(w.is_exiting());
        assert_eq!(closed.get(), 0, "deferred behind the ramp");
        paint(&mut w, 2000.0);
        paint(&mut w, 2100.0);
        assert!(w.progress() > 0.0 && w.progress() < 1.0, "mid-exit");
        assert_eq!(closed.get(), 0);
        paint(&mut w, 3000.0);
        assert_eq!(w.progress(), 0.0);
        assert_eq!(closed.get(), 1);
        assert_eq!(state.dismissed, 0, "the staged hook, not the raw callback");
    }

    #[test]
    fn there_is_no_close_button() {
        let mut w = build(&sample());
        layout(&mut w);
        assert!(w.close_rect().is_none());
    }

    #[test]
    fn the_sm_size_caps_the_panel_at_max_w_xs() {
        let mut w = build(&sample().size(AlertDialogSize::Sm));
        layout(&mut w);
        assert_eq!(w.panel_rect().width(), MAX_WIDTH_XS);
        assert_eq!(AlertDialogSize::default().max_width(), MAX_WIDTH_LG);

        // The default size is only capped by the viewport at this width.
        let mut w = build(&sample());
        layout(&mut w);
        assert_eq!(w.panel_rect().width(), WINDOW.width - 32.0);
    }

    #[test]
    fn semantics_reports_an_alert_dialog_role() {
        fn logic(_s: &mut Flags) -> AlertDialogView<Flags> {
            alert_dialog(vec![alert_dialog_header(vec![alert_dialog_title(
                "Delete project?",
            )])])
            .label("Delete project?")
        }
        let mut root: RenderRoot<Flags, AlertDialogView<Flags>> = RenderRoot::new();
        let mut state = Flags::default();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::AlertDialog)
            .expect("an AlertDialog node");
        assert!(node.is_modal());
        assert_eq!(node.label(), Some("Delete project?"));
    }
}
