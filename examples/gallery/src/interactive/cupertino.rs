//! Stateful constructors for the Cupertino design system's cases (`cupertino/*`
//! slugs) — see the [module docs](super) for what this table is and why it
//! sits beside the registry rather than inside it.
//!
//! Five of the catalog's seven cases gain an entry here; `button` and `navbar`
//! own no app state and stay absent (their press feedback is already
//! widget-owned and live without any of this).
//!
//! # The activity indicator: not a state problem
//!
//! The recorded `activity-indicator` case pins `cupertino_activity_indicator()
//! .animating(false)` — `crate::cupertino::activity_indicator_case`, out of
//! this file's scope. [`CupertinoActivityIndicatorView`] carries no callback at
//! all (an indeterminate spinner reports nothing back to an app), so there is
//! no value here for a [`frust_core::Component`] to own, and no report a caption
//! could derive. The freeze was purely the recorder's own one-shot-paint
//! problem: `cupertino_activity_indicator` defaults `animating: true`, and a
//! live host's clock advances [`frust::AnimationController::advance`] on every
//! paint, so simply *not* calling `.animating(false)` is the whole fix —
//! [`activity_indicator_case`] below is three lines and holds no state at all.
//! **The static case should also change** (drop the `.animating(false)` call
//! so its own doc comment stops describing the spinner as "explicitly
//! frozen"), but that line lives in `crate::cupertino`, outside this file's
//! declared write scope, and moving it would legitimately change the recorded
//! poster (a spinner frozen at rest vs. one caught mid-rotation are different
//! pixels) — reported rather than done here, per the card.
//!
//! # The alert dialog and action sheet: not a plugin-API problem either
//!
//! The recorded `alert-dialog`/`action-sheet` cases build
//! [`CupertinoAlertDialogView`]/[`CupertinoActionSheetView`] directly, each with
//! a fresh [`NavigatorController::new()`] thrown away every build — so no
//! navigation state (in flight or otherwise) could ever survive a rebuild, and
//! [`action`]'s [`CupertinoDialogAction`] carries no callback for the recorded
//! view to report a choice through even if it could. That diagnosis holds
//! *for the raw view struct the recorder embeds directly*, but it does not
//! carry over to a live host: [`show_cupertino_alert`]/[`show_action_sheet`]
//! push the same view as a transparent page on a real
//! [`frust_widgets::NavigatorController`] and register an `on_result` callback
//! invoked with `&mut State` and the tapped action's index as a
//! [`frust_widgets::PopResult`] — exactly the reporting seam the raw struct
//! lacks. Retaining the controller (built once in [`Component::init`], never
//! rebuilt) and pushing through the sugared functions is the whole fix; no
//! plugin API change is needed, and this file reaches into no private
//! `frust_cupertino` API to make it work.
//!
//! Each of the two mounts a real [`frust_widgets::navigator`] as its root view
//! and pushes the dialog/sheet once at `init` — so the opening frame shows it
//! up, matching the poster's composition — with a "Show ⋯" button and a
//! readout of the last result underneath, reachable once the panel is
//! dismissed. The readout is an `Rc<RefCell<String>>` cell shared between the
//! `on_result` callback and the navigator's root-page closure rather than a
//! plain `State` field: [`frust_widgets`]'s own navigator keeps a pushed/root
//! page's builder closure fixed for the life of the retained page (only a
//! push/pop/replace op ever installs a new one — see
//! `NavigatorWidget::rebuild`'s reconcile loop), so a closure that captured a
//! `State` value *by value* would never see a later change; a shared cell read
//! afresh on every reconcile does. This is the same reactive-free
//! shared-handle idiom [`super::shadcn`]'s `OverlayAnchor`/`TooltipHover`
//! fields already use for the identical reason.
//!
//! Nothing here is a `Case::build`, so none of it reaches the snapshot oracle
//! and no poster moves.

use std::cell::RefCell;
use std::rc::Rc;

use frust_core::{AnyView, Component, View, any, component};
use frust_cupertino::{
    CupertinoActionStyle, action, cupertino_activity_indicator, cupertino_button, cupertino_switch,
    cupertino_tab_bar, show_action_sheet, show_cupertino_alert, tab_item,
};
use frust_widgets::{
    CrossAxisAlignment, NavigatorController, PopResult, SizedBox, column, navigator, row, text,
};

use super::{Entry, framed};

/// This catalog's slice of the side table [`super::entries`] concatenates, in
/// the registry's own slug order.
///
/// `cupertino/button` and `cupertino/navbar` are deliberately absent — see the
/// [module docs](self).
pub const INTERACTIVE: &[Entry] = &[
    ("cupertino/activity-indicator", activity_indicator_case),
    ("cupertino/alert-dialog", alert_dialog_case),
    ("cupertino/action-sheet", action_sheet_case),
    ("cupertino/switch", switch_case),
    ("cupertino/tabbar", tabbar_case),
];

// ---- activity-indicator ------------------------------------------------------

/// The `activity-indicator` case, unfrozen. See the [module docs](self)'s
/// "not a state problem" section: nothing here owns state, because nothing
/// about this widget reports anything back to an app to hold.
fn activity_indicator_case() -> AnyView<()> {
    any(framed(cupertino_activity_indicator()))
}

// ---- switch -------------------------------------------------------------------

/// Retained state for the interactive `switch` case: one bool per row, seeded
/// to the values the recorded case hardcodes (off, then on).
struct SwitchState {
    off_row: bool,
    on_row: bool,
}

/// The `switch` case with both rows app-owned.
///
/// `cupertino_switch` is controlled exactly like a Material `switch` — it
/// fires `on_toggle(state, !checked)` on release and never flips its own
/// `checked` — so the recorded pair's `|_: &mut (), _: bool| {}` callbacks
/// made both rows permanently inert. One bool per row is the whole
/// conversion; the thumb's spring-driven travel (see
/// `plugins/cupertino/src/switch.rs`'s module docs) is the widget's own and
/// needs nothing from this state.
struct SwitchCase;

impl Component for SwitchCase {
    type State = SwitchState;

    fn init(&self) -> Self::State {
        SwitchState {
            off_row: false,
            on_row: true,
        }
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        framed(
            column()
                .child(
                    row()
                        .child(text("Off"))
                        .child(SizedBox(Some(16.0), None))
                        .child(cupertino_switch(
                            state.off_row,
                            |state: &mut SwitchState, checked| state.off_row = checked,
                        ))
                        .cross_axis(CrossAxisAlignment::Center),
                )
                .child(SizedBox(None, Some(12.0)))
                .child(
                    row()
                        .child(text("On"))
                        .child(SizedBox(Some(16.0), None))
                        .child(cupertino_switch(
                            state.on_row,
                            |state: &mut SwitchState, checked| state.on_row = checked,
                        ))
                        .cross_axis(CrossAxisAlignment::Center),
                )
                .cross_axis(CrossAxisAlignment::Center),
        )
    }
}

fn switch_case() -> AnyView<()> {
    any(component(SwitchCase))
}

// ---- tabbar ---------------------------------------------------------------

/// Retained state for the interactive `tabbar` case: the selected destination
/// index, seeded to the one the recorded case pins.
struct TabBarState {
    selected: usize,
}

/// The `tabbar` case with a caption that stays honest.
///
/// The recorded caption is a hardcoded `"Tab bar (selected: 0)"` beside a
/// `cupertino_tab_bar` whose `on_select` writes to `()` — a claim two
/// constants agree on only because nothing could ever move the second one.
/// Deriving the caption from `state.selected` is the cheapest available proof
/// that a tap reached app state and came back down, the same shape
/// [`super::base`]'s slider percentage and [`super::material`]'s table caption
/// use.
struct TabBarCase;

impl Component for TabBarCase {
    type State = TabBarState;

    fn init(&self) -> Self::State {
        TabBarState { selected: 0 }
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        let caption = format!("Tab bar (selected: {})", state.selected);
        framed(
            column()
                .child(text(caption))
                .child(SizedBox(None, Some(12.0)))
                .child(cupertino_tab_bar::<TabBarState, _>(
                    vec![tab_item("Home"), tab_item("Search"), tab_item("Favorites")],
                    state.selected,
                    |state: &mut TabBarState, index| state.selected = index,
                ))
                .cross_axis(CrossAxisAlignment::Center),
        )
    }
}

fn tabbar_case() -> AnyView<()> {
    any(component(TabBarCase))
}

// ---- alert-dialog and action-sheet: shared shape ---------------------------
//
// Both push a design-system dialog as a transparent page on a retained
// `NavigatorController`, both report their result into a shared readout cell
// rather than a `State` field, and both offer a "Show ⋯" button once the panel
// is dismissed — see the [module docs](self) for why the retained controller
// and the shared cell are each load-bearing rather than a stylistic choice.

// alert-dialog ----------------------------------------------------------------

/// Retained state for the interactive `alert-dialog` case: the navigator
/// driving the pushed alert, and a readout of its last result.
struct AlertDialogState {
    controller: NavigatorController<AlertDialogState>,
    /// The last result's description, or empty before the alert has ever been
    /// dismissed. Shared with the navigator's root-page closure — see the
    /// [module docs](self) for why a plain `State` field cannot do this job.
    readout: Rc<RefCell<String>>,
}

/// Push the `alert-dialog` case's confirm alert — the recorded case's own
/// title/message/actions, unchanged — wiring its result into `readout`.
fn push_alert_dialog(
    controller: &NavigatorController<AlertDialogState>,
    readout: &Rc<RefCell<String>>,
) {
    let readout = readout.clone();
    show_cupertino_alert::<AlertDialogState>(
        controller,
        "Confirm",
        Some("Are you sure?".to_string()),
        vec![
            action("Cancel"),
            action("OK").style(CupertinoActionStyle::Default),
        ],
        move |_: &mut AlertDialogState, result: PopResult| {
            let outcome = match result.take::<usize>() {
                Some(0) => "chose Cancel".to_string(),
                Some(1) => "chose OK".to_string(),
                Some(other) => format!("chose action {other}"),
                None => "dismissed the scrim".to_string(),
            };
            *readout.borrow_mut() = outcome;
        },
    );
}

/// The `alert-dialog` case with a navigator that survives rebuild and a choice
/// that goes somewhere. See the [module docs](self)'s alert/sheet section for
/// why this needs no plugin API change, only a retained controller and the
/// sugared [`show_cupertino_alert`] in place of the raw view struct.
struct AlertDialogCase;

impl Component for AlertDialogCase {
    type State = AlertDialogState;

    fn init(&self) -> Self::State {
        let controller = NavigatorController::new();
        let readout = Rc::new(RefCell::new(String::new()));
        // Pushed before the navigator ever builds: `NavigatorView::build`
        // drains any ops queued this way, so the opening frame already shows
        // the alert up, matching the recorded poster's composition.
        push_alert_dialog(&controller, &readout);
        AlertDialogState {
            controller,
            readout,
        }
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        let controller = state.controller.clone();
        let root_controller = controller.clone();
        let root_readout = state.readout.clone();
        framed(navigator(&controller, move || {
            let outcome = root_readout.borrow().clone();
            let reopen_controller = root_controller.clone();
            let reopen_readout = root_readout.clone();
            any(column()
                .child(text(outcome))
                .child(SizedBox(None, Some(12.0)))
                .child(cupertino_button(
                    "Show alert",
                    move |_: &mut AlertDialogState| {
                        push_alert_dialog(&reopen_controller, &reopen_readout);
                    },
                ))
                .cross_axis(CrossAxisAlignment::Center))
        }))
    }
}

fn alert_dialog_case() -> AnyView<()> {
    any(component(AlertDialogCase))
}

// action-sheet ----------------------------------------------------------------

/// Retained state for the interactive `action-sheet` case: the navigator
/// driving the pushed sheet, and a readout of its last result.
struct ActionSheetState {
    controller: NavigatorController<ActionSheetState>,
    /// The last result's description, or empty before the sheet has ever been
    /// dismissed. Shared with the navigator's root-page closure — see
    /// [`AlertDialogState::readout`] for why.
    readout: Rc<RefCell<String>>,
}

/// Push the `action-sheet` case's actions — the recorded case's own
/// save/delete/cancel row, unchanged — wiring its result into `readout`.
fn push_action_sheet(
    controller: &NavigatorController<ActionSheetState>,
    readout: &Rc<RefCell<String>>,
) {
    let readout = readout.clone();
    show_action_sheet::<ActionSheetState>(
        controller,
        vec![
            action("Save"),
            action("Delete").style(CupertinoActionStyle::Destructive),
        ],
        Some("Cancel".to_string()),
        move |_: &mut ActionSheetState, result: PopResult| {
            let outcome = match result.take::<usize>() {
                Some(0) => "chose Save".to_string(),
                Some(1) => "chose Delete".to_string(),
                Some(other) => format!("chose action {other}"),
                None => "dismissed (cancel or scrim)".to_string(),
            };
            *readout.borrow_mut() = outcome;
        },
    );
}

/// The `action-sheet` case, converted the same way as [`AlertDialogCase`] —
/// see that type and the [module docs](self)'s alert/sheet section.
struct ActionSheetCase;

impl Component for ActionSheetCase {
    type State = ActionSheetState;

    fn init(&self) -> Self::State {
        let controller = NavigatorController::new();
        let readout = Rc::new(RefCell::new(String::new()));
        push_action_sheet(&controller, &readout);
        ActionSheetState {
            controller,
            readout,
        }
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        let controller = state.controller.clone();
        let root_controller = controller.clone();
        let root_readout = state.readout.clone();
        framed(navigator(&controller, move || {
            let outcome = root_readout.borrow().clone();
            let reopen_controller = root_controller.clone();
            let reopen_readout = root_readout.clone();
            any(column()
                .child(text(outcome))
                .child(SizedBox(None, Some(12.0)))
                .child(cupertino_button(
                    "Show action sheet",
                    move |_: &mut ActionSheetState| {
                        push_action_sheet(&reopen_controller, &reopen_readout);
                    },
                ))
                .cross_axis(CrossAxisAlignment::Center))
        }))
    }
}

fn action_sheet_case() -> AnyView<()> {
    any(component(ActionSheetCase))
}
