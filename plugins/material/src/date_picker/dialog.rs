// Ported from `material_3_expressive` v1.0.8's date picker dialog (MIT, © 2026
// Paa Developments;
// `tmp/material_3_expressive/lib/components/date_pickers/m3e_date_picker_dialog.dart`,
// `m3e_date_pickers.dart` (`M3EDatePicker.show`), and
// `components/{m3e_date_picker_header, m3e_date_picker_actions,
// m3e_date_picker_dialog_content}.dart`, retrieved 2026-08-20).
// Upstream: <https://github.com/paadevelopments/material_3_expressive>
//
// Porting decisions (documented in the module docs below): the portrait layout
// only; the panel is content on the merged modal host rather than its own
// `Material` + `showGeneralDialog`; the OK/Cancel result plumbing runs through
// the navigator's own pop-with-result seam; and OK/Cancel dismiss through the
// host's public staged-dismiss handle rather than popping the navigator
// themselves.

//! The modal **date picker dialog**: [`date_picker_dialog`] builds the panel,
//! [`show_date_picker`] pushes it and delivers the picked date back through the
//! navigator's pop result.
//!
//! # The panel is content, the host is chrome
//!
//! Like every other modal in this catalog, the panel is a
//! [`crate::overlay::modal::overlay_modal`] content view: the host contributes
//! the scrim, the fill/radius/elevation, the entrance and exit ramps, and the
//! scrim-tap/`Escape`/Android-back dismissal; this module contributes the
//! header (help line, headline date, entry-mode affordance), the divider, the
//! body (calendar or input field) and the Cancel/OK row.
//!
//! The panel is capped at [`DIALOG_PORTRAIT_CALENDAR_WIDTH`] in calendar mode
//! and [`DIALOG_PORTRAIT_INPUT_WIDTH`] in input mode — the reference's
//! `calendarPortraitDialogSize`/`inputPortraitDialogSize` widths.
//!
//! # Result plumbing
//!
//! [`show_date_picker`] wires three exits, matching `_handleOk`/`_handleCancel`
//! plus the barrier:
//!
//! | Exit | Pops with |
//! |---|---|
//! | **OK** | `PopResult::of(`[`MaterialDate`]`)` when a date is picked, an empty result otherwise |
//! | **Cancel** | an empty result |
//! | scrim tap / `Escape` / Android back | an empty result (the host's own staged path) |
//!
//! so an app reads `result.take::<MaterialDate>()` and gets `Some` only for a
//! confirmed pick — the reference's `Future<DateTime?>` in the shape this
//! framework already has.
//!
//! **OK in input mode refuses an invalid entry**: it pops nothing at all when
//! [`DatePickerState::input_text`] does not parse into a selectable date,
//! exactly as `_handleOk`'s `if (!form.validate()) { return; }` does. The field
//! is already showing why (see [`mod@super::input`]'s live validation).
//!
//! # OK and Cancel animate out, like every other exit
//!
//! Both actions sit *inside* the panel, so neither can reach the host's exit
//! ramp on its own: popping the navigator from a button takes the page out from
//! under the host and the panel vanishes mid-fade. [`show_date_picker`]
//! therefore mints a [`ModalDismiss`] and hands it to the dialog
//! ([`DatePickerDialog::dismiss_handle`]) — OK dismisses with its
//! [`PopResult`], Cancel dismisses empty, and both take the same staged exit
//! the scrim tap, `Escape`, and an Android back press take.
//!
//! With no handle installed (a `Stack`-mounted dialog, or one pushed by hand),
//! Cancel falls back to firing [`DatePickerDialog::on_dismiss`] directly, which
//! is unstaged — the only route open to it.
//!
//! With a handle installed but nothing to stage behind it — a `Stack`-mounted
//! dialog the app handed a handle anyway — Cancel reaches that same
//! `on_dismiss`, but through the *host*: the request is raised here and the
//! host, which alone knows whether a staged close hook exists, delivers the
//! unstaged fallback in the event pass (`crate::overlay::modal`'s *What a
//! request consumes*). Both mounts dismiss, exactly once, whichever way the
//! dialog was mounted.
//!
//! # Deliberate cuts
//!
//! The landscape layout (`calendarLandscapeDialogSize`, a side header) is not
//! ported — see [`mod@super`]'s scope section. Neither is the reference's
//! `RestorationMixin` state restoration, which has no framework counterpart.

use std::rc::Rc;

use frust::authoring::text::{FontWeight, TextContext, TextLayout, TextStyle};
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, View, Widget, any, build_child, rebuild_child,
    rebuild_children, route_event, route_event_single, teardown_child, visit_children,
};
use frust::{Color, NavigatorController, PopResult, Theme, icon};
use kurbo::{Point, Size};
use peniko::Brush;

use super::calendar::{SelectableDay, calendar_date_picker};
use super::date::MaterialDate;
use super::input::date_input_field;
use super::{
    ACTIONS_MIN_HEIGHT, DIALOG_PORTRAIT_CALENDAR_WIDTH, DIALOG_PORTRAIT_INPUT_WIDTH,
    DatePickerState, DatePickerStrings, HEADER_PORTRAIT_HEIGHT, OnDatePickerChange,
};
use crate::button::{button, text_button};
use crate::divider::divider;
use crate::icon_button::icon_button;
use crate::overlay::{
    ModalDismiss, OverlayModalConfig, OverlayModalContent, OverlayModalView, OverlayModalWidget,
    overlay_modal, show_overlay_modal,
};

/// The panel's inset on all four edges, in logical px (`M3EDialogTheme.padding`,
/// which the reference's header/actions/input body all read).
const PANEL_PADDING: f64 = 24.0;
/// The gap between the Cancel and OK buttons (`M3EDialogTheme.actionGap`).
const ACTION_GAP: f64 = 8.0;
/// The entry-mode affordance's slot, in logical px — one M3 48dp touch target.
const TOGGLE_SLOT: f64 = 48.0;
/// The gap between the header's help line and its headline date.
const HEADLINE_GAP: f64 = 4.0;

/// The help line's type role (M3 `labelLarge`, `headerHelpStyle`).
const HELP_TEXT_SIZE: f32 = 14.0;
/// The headline date's type role (M3 `headlineLarge`, `headerHeadlineStyle`).
const HEADLINE_TEXT_SIZE: f32 = 32.0;
/// The ink every retained run is shaped with; each is re-brushed at paint time.
const SHAPING_INK: Color = Color::BLACK;

/// Unthemed-fallback `onSurface` (M3 baseline light).
const FALLBACK_ON_SURFACE: Color = Color::from_rgb8(0x1D, 0x1B, 0x20);
/// Unthemed-fallback `onSurfaceVariant`.
const FALLBACK_ON_SURFACE_VARIANT: Color = Color::from_rgb8(0x49, 0x45, 0x4F);

/// A view-held, typed dismiss callback.
type OnDismiss<State> = Rc<dyn Fn(&mut State)>;
/// A view-held, typed confirm callback carrying whatever OK resolved to.
type OnConfirm<State> = Rc<dyn Fn(&mut State, Option<MaterialDate>)>;

/// The **OK** action's whole body, shared by the composed button's callback and
/// [`DatePickerDialog::confirm`] so the two cannot drift.
///
/// A `can_confirm` of `false` (an input mode whose text does not resolve) fires
/// nothing at all — the reference's `_handleOk` early `return`.
fn confirm_action<State: 'static>(
    on_confirm: Option<&OnConfirm<State>>,
    can_confirm: bool,
    confirmable: Option<MaterialDate>,
    app: &mut State,
) {
    if !can_confirm {
        return;
    }
    if let Some(on_confirm) = on_confirm {
        on_confirm(app, confirmable);
    }
}

/// The **Cancel** action's whole body — see [`confirm_action`].
///
/// A handle takes precedence over the callback rather than firing alongside it:
/// on the [`show_date_picker`] path that callback *is* the host's raw
/// `controller.pop()`, so running both would pop the page out from under the
/// exit ramp the handle just staged.
///
/// **The unstaged fallback is the host's, not this function's.** A handle
/// raises a request; whether that request can be *staged* depends on how the
/// dialog is mounted, which only the host knows — with no staged close hook
/// behind it (a `Stack` mount), the host falls back to firing
/// [`DatePickerDialog::on_dismiss`] itself, in the event pass, so Cancel still
/// dismisses (`crate::overlay::modal`'s *What a request consumes*). Firing the
/// callback from here as well would dismiss twice on the mount that *can*
/// stage, and this function cannot tell the two apart — the host observes the
/// request after this returns.
fn cancel_action<State: 'static>(
    dismiss: Option<&ModalDismiss>,
    on_dismiss: Option<&OnDismiss<State>>,
    app: &mut State,
) {
    if let Some(dismiss) = dismiss {
        dismiss.dismiss();
        return;
    }
    if let Some(on_dismiss) = on_dismiss {
        on_dismiss(app);
    }
}

/// A declarative M3E date picker dialog. See the [module docs](self).
pub struct DatePickerDialog<State: 'static> {
    state: DatePickerState,
    first_date: MaterialDate,
    last_date: MaterialDate,
    today: Option<MaterialDate>,
    strings: DatePickerStrings,
    selectable: Option<SelectableDay>,
    on_change: OnDatePickerChange<State>,
    on_confirm: Option<OnConfirm<State>>,
    dismissable: bool,
    on_dismiss: Option<OnDismiss<State>>,
    dismiss_handle: Option<ModalDismiss>,
}

/// Create a controlled date picker dialog over `state`, bounded by
/// `first_date..=last_date`, reporting every requested state change through
/// `on_change`.
///
/// Hand it to [`show_date_picker`], which wires OK/Cancel/back for you:
///
/// ```ignore
/// show_date_picker(
///     &app.nav,
///     || date_picker_dialog(
///         app.picker.clone(),
///         MaterialDate::new(2020, 1, 1),
///         MaterialDate::new(2030, 12, 31),
///         |app: &mut App, next| app.picker = next,
///     ),
///     |app: &mut App, result: PopResult| {
///         if let Some(date) = result.take::<MaterialDate>() {
///             app.due = Some(date);
///         }
///     },
/// );
/// ```
pub fn date_picker_dialog<State: 'static, F>(
    state: DatePickerState,
    first_date: MaterialDate,
    last_date: MaterialDate,
    on_change: F,
) -> DatePickerDialog<State>
where
    F: Fn(&mut State, DatePickerState) + 'static,
{
    let (first_date, last_date) = if last_date < first_date {
        (last_date, first_date)
    } else {
        (first_date, last_date)
    };
    DatePickerDialog {
        state,
        first_date,
        last_date,
        today: None,
        strings: DatePickerStrings::ENGLISH,
        selectable: None,
        on_change: Rc::new(on_change),
        on_confirm: None,
        dismissable: true,
        on_dismiss: None,
        dismiss_handle: None,
    }
}

impl<State: 'static> DatePickerDialog<State> {
    /// Mark `today` with the today ring — see
    /// [`CalendarDatePicker::today`](super::CalendarDatePicker::today) for why
    /// this is required rather than defaulted.
    pub fn today(mut self, today: MaterialDate) -> Self {
        self.today = Some(today);
        self
    }

    /// Replace the localized strings (default [`DatePickerStrings::ENGLISH`]).
    pub fn strings(mut self, strings: DatePickerStrings) -> Self {
        self.strings = strings;
        self
    }

    /// Refuse individual dates inside the bounds — the reference's
    /// `selectableDayPredicate`, applied by both the calendar and the field.
    pub fn selectable<F: Fn(MaterialDate) -> bool + 'static>(mut self, predicate: F) -> Self {
        self.selectable = Some(Rc::new(predicate));
        self
    }

    /// Observe OK. [`show_date_picker`] wires this to a staged dismissal
    /// carrying the picked date ([`ModalDismiss::dismiss_with`]); a caller
    /// mounting the dialog directly (in a `Stack`, with no navigator) wires its
    /// own.
    pub fn on_confirm<F: Fn(&mut State, Option<MaterialDate>) + 'static>(
        mut self,
        on_confirm: F,
    ) -> Self {
        self.on_confirm = Some(Rc::new(on_confirm));
        self
    }

    /// Whether the user can dismiss the dialog at all — the scrim tap,
    /// `Escape`, and an Android back press (default `true`). `false` disables
    /// all three; Cancel and OK still work.
    pub fn dismissable(mut self, dismissable: bool) -> Self {
        self.dismissable = dismissable;
        self
    }

    /// Observe a dismissal — the callback the **Cancel** button fires when no
    /// [`dismiss_handle`](Self::dismiss_handle) is installed (see the [module
    /// docs](self)' OK-and-Cancel section). [`show_date_picker`] installs a
    /// handle, so on that path Cancel stages the host's exit instead and this
    /// callback is the *host's* own unstaged fallback, nothing more.
    pub fn on_dismiss<F: Fn(&mut State) + 'static>(mut self, on_dismiss: F) -> Self {
        self.on_dismiss = Some(Rc::new(on_dismiss));
        self
    }

    /// Install the app-triggered staged-dismiss handle, so **Cancel** closes
    /// the dialog through the host's exit ramp instead of firing
    /// [`on_dismiss`](Self::on_dismiss) into an immediate pop (see
    /// [`ModalDismiss`]). [`show_date_picker`] installs one for you — and wires
    /// OK to it too, since OK's own pop is its to make.
    pub fn dismiss_handle(mut self, dismiss: ModalDismiss) -> Self {
        self.dismiss_handle = Some(dismiss);
        self
    }

    /// Whether `date` is inside the bounds and accepted by the predicate.
    fn is_selectable(&self, date: MaterialDate) -> bool {
        date.is_within(self.first_date, self.last_date)
            && self.selectable.as_ref().is_none_or(|p| p(date))
    }

    /// What pressing OK would confirm right now.
    ///
    /// In calendar mode that is [`DatePickerState::selected`] (or `None` — the
    /// reference pops a null date from an untouched calendar too). In input
    /// mode it is the parsed field text, and `None` there means **OK does
    /// nothing at all** rather than confirming an empty pick — see the
    /// [module docs](self).
    pub fn confirmable(&self) -> Option<MaterialDate> {
        if self.state.entry_mode.is_input() {
            MaterialDate::parse_compact(&self.state.input_text)
                .filter(|date| self.is_selectable(*date))
        } else {
            self.state.selected.filter(|date| self.is_selectable(*date))
        }
    }

    /// Whether pressing OK does anything at all — false only for an input mode
    /// whose text does not resolve.
    pub fn can_confirm(&self) -> bool {
        !self.state.entry_mode.is_input() || self.confirmable().is_some()
    }

    /// The panel's own max width for the current entry mode.
    fn panel_width(&self) -> f64 {
        if self.state.entry_mode.is_input() {
            DIALOG_PORTRAIT_INPUT_WIDTH
        } else {
            DIALOG_PORTRAIT_CALENDAR_WIDTH
        }
    }

    /// The headline the header shows: the selected date in medium form, or the
    /// unspecified-date placeholder.
    ///
    /// The reference hides the headline outright while nothing is selected
    /// (`showTitle: _selectedDate.value != null`); this port shows
    /// [`DatePickerStrings::no_date_selected`] (Flutter's own `unspecifiedDate`)
    /// so the header does not change height on the first pick.
    fn headline(&self) -> String {
        match self.state.selected {
            Some(date) => self.strings.medium_date(date),
            None => self.strings.no_date_selected.to_string(),
        }
    }

    /// The entry-mode affordance, or `None` for a locked (`*Only`) mode.
    fn toggle_view(&self) -> Option<AnyView<State>> {
        if !self.state.entry_mode.is_toggleable() {
            return None;
        }
        let glyph = if self.state.entry_mode.is_input() {
            crate::icons::CALENDAR_TODAY
        } else {
            crate::icons::EDIT_OUTLINED
        };
        let state = self.state.clone();
        let on_change = self.on_change.clone();
        Some(any::<State, _>(icon_button(
            any::<State, _>(icon(glyph)),
            move |app: &mut State| on_change(app, state.toggled_entry_mode()),
        )))
    }

    /// The body: the calendar, or the input field.
    fn body_view(&self) -> AnyView<State> {
        let on_change = self.on_change.clone();
        if self.state.entry_mode.is_input() {
            let mut field = date_input_field(
                self.state.clone(),
                self.first_date,
                self.last_date,
                move |app: &mut State, next| on_change(app, next),
            )
            .strings(self.strings);
            if let Some(predicate) = &self.selectable {
                let predicate = predicate.clone();
                field = field.selectable(move |date| predicate(date));
            }
            any::<State, _>(field)
        } else {
            let mut calendar = calendar_date_picker(
                self.state.clone(),
                self.first_date,
                self.last_date,
                move |app: &mut State, next| on_change(app, next),
            )
            .strings(self.strings);
            if let Some(today) = self.today {
                calendar = calendar.today(today);
            }
            if let Some(predicate) = &self.selectable {
                let predicate = predicate.clone();
                calendar = calendar.selectable(move |date| predicate(date));
            }
            any::<State, _>(calendar)
        }
    }

    /// Cancel — stages the host's exit through the dismiss handle, or fires
    /// the dismiss callback when there is none (see the [module docs](self)).
    fn cancel_view(&self) -> AnyView<State> {
        let on_dismiss = self.on_dismiss.clone();
        let dismiss = self.dismiss_handle.clone();
        any::<State, _>(text_button(
            self.strings.cancel_label,
            move |app: &mut State| cancel_action(dismiss.as_ref(), on_dismiss.as_ref(), app),
        ))
    }

    /// OK — fires the confirm callback with whatever [`Self::confirmable`]
    /// resolved to, or nothing at all when the input text does not parse.
    fn confirm_view(&self) -> AnyView<State> {
        let on_confirm = self.on_confirm.clone();
        let confirmable = self.confirmable();
        let can_confirm = self.can_confirm();
        any::<State, _>(button(
            self.strings.confirm_label,
            move |app: &mut State| {
                confirm_action(on_confirm.as_ref(), can_confirm, confirmable, app)
            },
        ))
    }

    /// Run the **OK** action's body directly — the same code path
    /// [`Self::confirm_view`]'s button callback runs, reachable without an
    /// event pass (both routes share [`confirm_action`], so a test of one is a
    /// test of the other).
    pub fn confirm(&self, app: &mut State) {
        confirm_action(
            self.on_confirm.as_ref(),
            self.can_confirm(),
            self.confirmable(),
            app,
        );
    }

    /// Run the **Cancel** action's body directly — see [`Self::confirm`].
    pub fn cancel(&self, app: &mut State) {
        cancel_action(self.dismiss_handle.as_ref(), self.on_dismiss.as_ref(), app);
    }

    /// Compose the host-facing modal view fresh from the current fields — the
    /// same recompose-per-pass shape [`crate::dialog`] takes.
    fn compose(&self) -> OverlayModalView<State> {
        let panel = DatePickerPanel {
            help: self.strings.help_text.to_string(),
            headline: self.headline(),
            toggle: self.toggle_view(),
            body: self.body_view(),
            body_inset: if self.state.entry_mode.is_input() {
                PANEL_PADDING
            } else {
                0.0
            },
            cancel: self.cancel_view(),
            confirm: self.confirm_view(),
        };
        let mut view = overlay_modal(panel, OverlayModalConfig::centered(self.panel_width()))
            .dismissable(self.dismissable)
            .label(self.strings.help_text);
        if let Some(on_dismiss) = &self.on_dismiss {
            let on_dismiss = on_dismiss.clone();
            view = view.on_dismiss(move |state: &mut State| on_dismiss(state));
        }
        if let Some(dismiss) = &self.dismiss_handle {
            view = view.dismiss_handle(dismiss.clone());
        }
        view
    }
}

impl<State: 'static> View<State> for DatePickerDialog<State> {
    type Element = OverlayModalWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> OverlayModalWidget {
        View::build(&self.compose(), ctx)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut OverlayModalWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        View::rebuild(&self.compose(), &prev.compose(), element, ctx)
    }

    fn teardown(&self, element: &mut OverlayModalWidget, ctx: &mut BuildCtx<'_>) {
        View::teardown(&self.compose(), element, ctx);
    }
}

impl<State: 'static> OverlayModalContent<State> for DatePickerDialog<State> {
    fn on_modal_dismiss(mut self, on_dismiss: Rc<dyn Fn(&mut State)>) -> Self {
        self.on_dismiss = Some(on_dismiss);
        self
    }

    fn modal_dismissable(&self) -> bool {
        self.dismissable
    }
}

/// Push `build`'s dialog as a transparent navigator page over the host's scrim,
/// and register `on_result` for the date it pops with.
///
/// See the [module docs](self)' result-plumbing table: OK pops
/// `PopResult::of(`[`MaterialDate`]`)` when it resolves to a date, and every
/// other exit pops empty — so `result.take::<MaterialDate>()` is `Some` exactly
/// for a confirmed pick.
///
/// Both in-panel actions dismiss through one [`ModalDismiss`] minted here (the
/// module docs' OK-and-Cancel section), so they animate out exactly like the
/// scrim tap does. It is minted **outside** the page builder on purpose: the
/// builder re-runs on every navigator rebuild, and the handle must stay the one
/// the mounted host latched.
pub fn show_date_picker<State, B, R>(
    controller: &NavigatorController<State>,
    build: B,
    on_result: R,
) where
    State: 'static,
    B: Fn() -> DatePickerDialog<State> + 'static,
    R: Fn(&mut State, PopResult) + 'static,
{
    let dismiss = ModalDismiss::new();
    show_overlay_modal(
        controller,
        move || {
            let confirm_dismiss = dismiss.clone();
            build()
                .on_confirm(
                    move |_state: &mut State, picked: Option<MaterialDate>| match picked {
                        Some(date) => confirm_dismiss.dismiss_with(PopResult::of(date)),
                        None => confirm_dismiss.dismiss(),
                    },
                )
                .dismiss_handle(dismiss.clone())
        },
        on_result,
    );
}

// ============================================================================
// DatePickerPanel — the modal host's content: header, divider, body, actions.
// ============================================================================

/// The dialog's panel content. See the [module docs](self).
struct DatePickerPanel<State: 'static> {
    help: String,
    headline: String,
    toggle: Option<AnyView<State>>,
    body: AnyView<State>,
    body_inset: f64,
    cancel: AnyView<State>,
    confirm: AnyView<State>,
}

/// A lazily shaped, paint-time re-brushed header run — the idiom
/// [`crate::badge`]'s `LabelRun` establishes.
struct HeaderRun {
    content: String,
    size: f32,
    weight: FontWeight,
    layout: Option<TextLayout>,
}

impl HeaderRun {
    fn new(size: f32, weight: FontWeight) -> Self {
        Self {
            content: String::new(),
            size,
            weight,
            layout: None,
        }
    }

    fn set_content(&mut self, content: &str) {
        if self.content != content {
            self.content = content.to_string();
            self.layout = None;
        }
    }

    /// Shape (or reuse) the run, returning its measured size.
    ///
    /// The cache is keyed on the **content only**, not on `max_width`: both
    /// header lines are short single lines that never wrap at either panel
    /// width ([`DIALOG_PORTRAIT_INPUT_WIDTH`]/[`DIALOG_PORTRAIT_CALENDAR_WIDTH`]),
    /// so a width change alone cannot change the shaping. A caller that
    /// narrowed the panel far enough to force a wrap would need to invalidate
    /// through [`Self::set_content`].
    fn shape(&mut self, ctx: &mut LayoutCtx, max_width: f64) -> Size {
        if let Some(layout) = &self.layout {
            return layout.size();
        }
        let style = TextStyle {
            weight: self.weight,
            ..TextStyle::new(self.size, SHAPING_INK)
        };
        let laid = ctx.text_context::<TextContext>().layout(
            &self.content,
            &style,
            Some(max_width.max(0.0) as f32),
        );
        let size = laid.size();
        self.layout = Some(laid);
        size
    }

    fn paint(&self, origin: Point, color: Color, scene: &mut dyn PaintScene) {
        let Some(layout) = &self.layout else {
            return;
        };
        for mut run in layout.to_scene_runs(origin) {
            run.brush = Brush::Solid(color);
            scene.draw_glyph_run(run);
        }
    }
}

/// The retained widget for a [`DatePickerPanel`].
struct DatePickerPanelWidget {
    help: HeaderRun,
    headline: HeaderRun,
    toggle: Option<ChildPod>,
    divider_pod: ChildPod,
    body: ChildPod,
    body_inset: f64,
    /// Cancel then OK, in reading order — the shape `route_event` walks.
    actions: Vec<ChildPod>,
    /// The header's own resolved height, recorded at layout for paint.
    header_height: f64,
    help_origin: Point,
    headline_origin: Point,
}

/// Build/teardown/reconcile one optional erased child in place — the same
/// three-way match [`crate::dialog`]'s `reconcile_any` runs.
fn reconcile_optional<State: 'static>(
    prev: Option<&AnyView<State>>,
    next: Option<&AnyView<State>>,
    pod: &mut Option<ChildPod>,
    ctx: &mut BuildCtx<'_>,
) -> ChangeFlags {
    match (prev, next) {
        (None, Some(view)) => {
            *pod = Some(build_child(view, ctx));
            ChangeFlags::LAYOUT | ChangeFlags::PAINT
        }
        (Some(view), None) => {
            if let Some(p) = pod.as_mut() {
                teardown_child(view, p, ctx);
            }
            *pod = None;
            ChangeFlags::LAYOUT | ChangeFlags::PAINT
        }
        (Some(a), Some(b)) => match pod.as_mut() {
            Some(p) => rebuild_child(a, b, p, ctx),
            None => ChangeFlags::NONE,
        },
        (None, None) => ChangeFlags::NONE,
    }
}

impl<State: 'static> View<State> for DatePickerPanel<State> {
    type Element = DatePickerPanelWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> DatePickerPanelWidget {
        let mut help = HeaderRun::new(HELP_TEXT_SIZE, FontWeight::MEDIUM);
        help.set_content(&self.help);
        let mut headline = HeaderRun::new(HEADLINE_TEXT_SIZE, FontWeight::REGULAR);
        headline.set_content(&self.headline);
        DatePickerPanelWidget {
            help,
            headline,
            toggle: self.toggle.as_ref().map(|v| build_child(v, ctx)),
            divider_pod: build_child(&any::<State, _>(divider()), ctx),
            body: build_child(&self.body, ctx),
            body_inset: self.body_inset,
            actions: vec![
                build_child(&self.cancel, ctx),
                build_child(&self.confirm, ctx),
            ],
            header_height: 0.0,
            help_origin: Point::ZERO,
            headline_origin: Point::ZERO,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut DatePickerPanelWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.help != self.help {
            element.help.set_content(&self.help);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.headline != self.headline {
            element.headline.set_content(&self.headline);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.body_inset != self.body_inset {
            element.body_inset = self.body_inset;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags |= reconcile_optional(
            prev.toggle.as_ref(),
            self.toggle.as_ref(),
            &mut element.toggle,
            ctx,
        );
        flags |= rebuild_child(&prev.body, &self.body, &mut element.body, ctx);
        flags |= rebuild_child(
            &any::<State, _>(divider()),
            &any::<State, _>(divider()),
            &mut element.divider_pod,
            ctx,
        );
        let prev_actions = [&prev.cancel, &prev.confirm];
        let next_actions = [&self.cancel, &self.confirm];
        flags |= rebuild_children(
            &prev_actions,
            &next_actions,
            &mut element.actions,
            ctx,
            |view: &&AnyView<State>| *view,
            |_| None,
        );
        flags
    }

    fn teardown(&self, element: &mut DatePickerPanelWidget, ctx: &mut BuildCtx<'_>) {
        if let (Some(view), Some(pod)) = (&self.toggle, element.toggle.as_mut()) {
            teardown_child(view, pod, ctx);
        }
        teardown_child(&any::<State, _>(divider()), &mut element.divider_pod, ctx);
        teardown_child(&self.body, &mut element.body, ctx);
        for (view, pod) in [&self.cancel, &self.confirm]
            .into_iter()
            .zip(element.actions.iter_mut())
        {
            teardown_child(view, pod, ctx);
        }
    }
}

impl Widget for DatePickerPanelWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // The host gives a centred panel a tight width; the fallback covers the
        // degenerate unbounded case a `Stack`-mounted panel could hand in.
        let offered = bc.max().width.max(bc.min().width);
        let width = if offered.is_finite() {
            offered
        } else {
            DIALOG_PORTRAIT_CALENDAR_WIDTH
        };
        let inner_w = (width - 2.0 * PANEL_PADDING).max(0.0);

        // --- header: help row (help text + optional mode affordance), headline.
        let toggle_size = match self.toggle.as_mut() {
            Some(pod) => pod.layout_child(
                ctx,
                &BoxConstraints::loose(Size::new(TOGGLE_SLOT, TOGGLE_SLOT)),
            ),
            None => Size::ZERO,
        };
        let help_w = (inner_w
            - if self.toggle.is_some() {
                TOGGLE_SLOT
            } else {
                0.0
            })
        .max(0.0);
        let help_size = self.help.shape(ctx, help_w);
        let headline_size = self.headline.shape(ctx, inner_w);

        let help_row_h = help_size.height.max(toggle_size.height);
        let natural =
            PANEL_PADDING + help_row_h + HEADLINE_GAP + headline_size.height + PANEL_PADDING;
        self.header_height = natural.max(HEADER_PORTRAIT_HEIGHT);
        self.help_origin = Point::new(
            PANEL_PADDING,
            PANEL_PADDING + (help_row_h - help_size.height) / 2.0,
        );
        self.headline_origin = Point::new(
            PANEL_PADDING,
            self.header_height - PANEL_PADDING - headline_size.height,
        );
        if let Some(pod) = self.toggle.as_mut() {
            pod.set_origin(Point::new(
                width - PANEL_PADDING - toggle_size.width,
                PANEL_PADDING + (help_row_h - toggle_size.height) / 2.0,
            ));
        }

        let mut y = self.header_height;

        // --- divider.
        let divider_size = self
            .divider_pod
            .layout_child(ctx, &BoxConstraints::tight(Size::new(width, 1.0)));
        self.divider_pod.set_origin(Point::new(0.0, y));
        y += divider_size.height;

        // --- body.
        let body_w = (width - 2.0 * self.body_inset).max(0.0);
        let body_size = self.body.layout_child(
            ctx,
            &BoxConstraints::new(Size::new(body_w, 0.0), Size::new(body_w, f64::INFINITY)),
        );
        self.body
            .set_origin(Point::new(self.body_inset, y + self.body_inset));
        y += body_size.height + 2.0 * self.body_inset;

        // --- actions: right-aligned Cancel then OK.
        let loose = BoxConstraints::loose(Size::new(inner_w, f64::INFINITY));
        let mut sizes = Vec::with_capacity(self.actions.len());
        for pod in &mut self.actions {
            sizes.push(pod.layout_child(ctx, &loose));
        }
        let row_h = sizes.iter().fold(0.0_f64, |acc, s| acc.max(s.height));
        let band = (row_h + PANEL_PADDING).max(ACTIONS_MIN_HEIGHT);
        let total_w = sizes.iter().map(|s| s.width).sum::<f64>()
            + ACTION_GAP * (sizes.len().saturating_sub(1)) as f64;
        let mut x = width - PANEL_PADDING - total_w;
        for (pod, size) in self.actions.iter_mut().zip(sizes.iter()) {
            pod.set_origin(Point::new(x, y + (row_h - size.height) / 2.0));
            x += size.width + ACTION_GAP;
        }
        y += band;

        bc.constrain(Size::new(width, y))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let (on_surface, on_surface_variant) = {
            let theme = Theme::from_paint_ctx(ctx);
            (
                theme.map_or(FALLBACK_ON_SURFACE, |t| t.scheme().on_surface),
                theme.map_or(FALLBACK_ON_SURFACE_VARIANT, |t| {
                    t.scheme().on_surface_variant
                }),
            )
        };
        let origin = ctx.origin();
        self.help.paint(
            Point::new(origin.x + self.help_origin.x, origin.y + self.help_origin.y),
            on_surface_variant,
            scene,
        );
        self.headline.paint(
            Point::new(
                origin.x + self.headline_origin.x,
                origin.y + self.headline_origin.y,
            ),
            on_surface,
            scene,
        );
        if let Some(pod) = self.toggle.as_mut() {
            pod.paint_child(ctx, scene);
        }
        self.divider_pod.paint_child(ctx, scene);
        self.body.paint_child(ctx, scene);
        for pod in &mut self.actions {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if let Some(toggle) = self.toggle.as_mut()
            && route_event_single(toggle, ctx, event) == EventResult::Handled
        {
            return EventResult::Handled;
        }
        if route_event_single(&mut self.body, ctx, event) == EventResult::Handled {
            return EventResult::Handled;
        }
        if route_event(&mut self.actions, ctx, event) == EventResult::Handled {
            return EventResult::Handled;
        }
        EventResult::Ignored
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        if let Some(pod) = &self.toggle {
            pod.semantics_child(ctx);
        }
        self.body.semantics_child(ctx);
        for pod in &self.actions {
            pod.semantics_child(ctx);
        }
    }

    visit_children!(toggle, divider_pod, body, actions);
}

#[cfg(test)]
mod tests {
    use std::any::Any;
    use std::cell::Cell;

    use frust::FrameTime;
    use frust::authoring::{PointerButton, PointerEvent, PointerPhase};

    use super::super::{DatePickerEntryMode, DatePickerMode};
    use super::*;

    fn d(year: i32, month: u32, day: u32) -> MaterialDate {
        MaterialDate::new(year, month, day)
    }

    const FIRST: fn() -> MaterialDate = || MaterialDate::new(2020, 1, 1);
    const LAST: fn() -> MaterialDate = || MaterialDate::new(2030, 12, 31);

    #[derive(Default)]
    struct App {
        reported: Vec<DatePickerState>,
        confirmed: Vec<Option<MaterialDate>>,
        dismissed: usize,
    }

    fn dialog(state: DatePickerState) -> DatePickerDialog<App> {
        date_picker_dialog(state, FIRST(), LAST(), |app: &mut App, next| {
            app.reported.push(next)
        })
        .on_confirm(|app: &mut App, picked| app.confirmed.push(picked))
        .on_dismiss(|app: &mut App| app.dismissed += 1)
    }

    fn seeded() -> DatePickerState {
        DatePickerState::new(Some(d(2026, 8, 20)), d(2026, 8, 20))
    }

    // ---- confirm resolution ----------------------------------------------

    #[test]
    fn calendar_mode_confirms_whatever_is_selected() {
        assert_eq!(dialog(seeded()).confirmable(), Some(d(2026, 8, 20)));
        assert!(dialog(seeded()).can_confirm());

        // Nothing selected still confirms — the reference pops a null date.
        let empty = DatePickerState::new(None, d(2026, 8, 20));
        assert_eq!(dialog(empty.clone()).confirmable(), None);
        assert!(
            dialog(empty).can_confirm(),
            "an untouched calendar still lets OK close the dialog"
        );
    }

    #[test]
    fn input_mode_confirms_the_parsed_text_and_refuses_an_invalid_one() {
        let valid = seeded()
            .with_entry_mode(DatePickerEntryMode::Input)
            .with_input_text("06/15/2026");
        assert_eq!(dialog(valid.clone()).confirmable(), Some(d(2026, 6, 15)));
        assert!(dialog(valid).can_confirm());

        for bad in ["08/2", "", "08/20/2045"] {
            let invalid = seeded()
                .with_entry_mode(DatePickerEntryMode::Input)
                .with_input_text(bad);
            assert_eq!(dialog(invalid.clone()).confirmable(), None, "{bad:?}");
            assert!(
                !dialog(invalid).can_confirm(),
                "OK must refuse {bad:?} rather than confirm nothing"
            );
        }
    }

    #[test]
    fn a_selection_the_predicate_refuses_is_not_confirmable() {
        let state = seeded().with_selected(d(2026, 8, 15));
        let refused = date_picker_dialog(state.clone(), FIRST(), LAST(), |_: &mut App, _| {})
            .selectable(|date| date.day() % 2 == 0);
        assert_eq!(refused.confirmable(), None);

        let allowed = date_picker_dialog(state, FIRST(), LAST(), |_: &mut App, _| {})
            .selectable(|date| date.day() % 2 == 1);
        assert_eq!(allowed.confirmable(), Some(d(2026, 8, 15)));
    }

    #[test]
    fn a_selection_outside_the_bounds_is_not_confirmable() {
        let state = DatePickerState::new(Some(d(2045, 1, 1)), d(2026, 8, 20));
        assert_eq!(dialog(state).confirmable(), None);
    }

    // ---- OK / Cancel wiring ----------------------------------------------

    #[test]
    fn ok_reports_the_confirmable_date_once() {
        let mut app = App::default();
        dialog(seeded()).confirm(&mut app);
        assert_eq!(app.confirmed, vec![Some(d(2026, 8, 20))]);
        assert_eq!(app.dismissed, 0);
    }

    #[test]
    fn ok_with_nothing_selected_still_confirms_an_empty_pick() {
        let mut app = App::default();
        dialog(DatePickerState::new(None, d(2026, 8, 20))).confirm(&mut app);
        assert_eq!(app.confirmed, vec![None]);
    }

    #[test]
    fn ok_in_input_mode_reports_nothing_for_an_invalid_entry() {
        let mut app = App::default();
        dialog(
            seeded()
                .with_entry_mode(DatePickerEntryMode::Input)
                .with_input_text("08/2"),
        )
        .confirm(&mut app);
        assert!(app.confirmed.is_empty(), "an invalid entry never confirms");
        assert_eq!(app.dismissed, 0, "and never dismisses either");
    }

    #[test]
    fn ok_in_input_mode_confirms_the_typed_date() {
        let mut app = App::default();
        dialog(
            seeded()
                .with_entry_mode(DatePickerEntryMode::Input)
                .with_input_text("06/15/2026"),
        )
        .confirm(&mut app);
        assert_eq!(app.confirmed, vec![Some(d(2026, 6, 15))]);
    }

    #[test]
    fn cancel_fires_the_dismiss_callback() {
        let mut app = App::default();
        dialog(seeded()).cancel(&mut app);
        assert_eq!(app.dismissed, 1);
        assert!(app.confirmed.is_empty());
    }

    #[test]
    fn cancel_stages_through_the_dismiss_handle_when_one_is_installed() {
        // The `show_date_picker` path: the callback Cancel used to fire is the
        // host's own raw `controller.pop()`, so with a handle installed Cancel
        // asks the host to animate out instead — and fires nothing itself.
        let dismiss = ModalDismiss::new();
        let mut app = App::default();
        dialog(seeded())
            .dismiss_handle(dismiss.clone())
            .cancel(&mut app);
        assert_eq!(app.dismissed, 0, "the unstaged callback is not fired");
        assert!(app.confirmed.is_empty());

        // The request is real: mounted the way `show_date_picker` mounts it (a
        // staged close hook on the host), Cancel's body makes the host animate
        // out and pop only on settle, instead of the panel vanishing at once.
        let dismiss = ModalDismiss::new();
        let picker = dialog(seeded()).dismiss_handle(dismiss.clone());
        let closed = Rc::new(Cell::new(0u32));
        let hook = closed.clone();
        let view = picker.compose().on_close(move || hook.set(hook.get() + 1));
        let mut counter = 0u64;
        let mut w = View::<App>::build(&view, &mut BuildCtx::new(&mut counter));
        let window = Size::new(600.0, 800.0);
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::tight(window));
        for ms in [0.0, 400.0] {
            let mut ctx = PaintCtx::for_test(Point::ORIGIN, window, ft_ms(ms));
            w.paint(&mut ctx, &mut NoopScene);
        }
        assert!(!w.is_exiting(), "entered");

        picker.cancel(&mut app);
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, window, ft_ms(500.0));
        w.paint(&mut ctx, &mut NoopScene);
        assert!(w.is_exiting(), "Cancel stages the host's exit ramp");
        assert_eq!(closed.get(), 0, "and pops nothing mid-ramp");

        let mut ctx = PaintCtx::for_test(Point::ORIGIN, window, ft_ms(2000.0));
        w.paint(&mut ctx, &mut NoopScene);
        assert_eq!(closed.get(), 1, "the pop lands when the ramp settles");
        assert_eq!(app.dismissed, 0, "never through the unstaged callback");
    }

    #[test]
    fn cancel_with_a_handle_but_no_staged_route_still_dismisses() {
        // A `Stack`-mounted picker the app handed a handle anyway: there is no
        // close hook behind it to stage, so the host — the only party that
        // knows that — delivers the unstaged `on_dismiss` itself, in the event
        // pass. Without that fallback the handle is inert and Cancel does
        // nothing at all on this mount.
        let dismiss = ModalDismiss::new();
        let picker = dialog(seeded()).dismiss_handle(dismiss.clone());
        let mut counter = 0u64;
        let mut w = View::<App>::build(&picker, &mut BuildCtx::new(&mut counter));
        let window = Size::new(600.0, 800.0);
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::tight(window));
        for ms in [0.0, 400.0] {
            let mut ctx = PaintCtx::for_test(Point::ORIGIN, window, ft_ms(ms));
            w.paint(&mut ctx, &mut NoopScene);
        }

        let mut app = App::default();
        picker.cancel(&mut app);
        assert_eq!(
            app.dismissed, 0,
            "Cancel fires nothing itself — the handle owns the dismissal"
        );

        // The paint backstop reaches no app state, so it leaves the request
        // pending rather than swallowing it…
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, window, ft_ms(500.0));
        w.paint(&mut ctx, &mut NoopScene);
        assert_eq!(app.dismissed, 0);
        assert!(!w.is_exiting(), "there is nothing to stage on this mount");

        // …and the next event pass delivers it, exactly once.
        for _ in 0..2 {
            let state_any: &mut dyn Any = &mut app;
            let mut ectx = EventCtx::new(state_any, Point::ORIGIN, window);
            w.event(
                &mut ectx,
                &InputEvent::Pointer(PointerEvent {
                    phase: PointerPhase::Move,
                    position: Point::new(5.0, 5.0),
                    button: PointerButton::Primary,
                }),
            );
        }
        assert_eq!(app.dismissed, 1, "Cancel is not dead on a `Stack` mount");
    }

    /// A frame time `ms` milliseconds in.
    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    /// A paint sink for a test that only needs the pass to run.
    struct NoopScene;

    impl PaintScene for NoopScene {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
    }

    #[test]
    fn the_modal_content_contract_reports_the_dismissable_flag() {
        assert!(OverlayModalContent::modal_dismissable(&dialog(seeded())));
        assert!(!OverlayModalContent::modal_dismissable(
            &dialog(seeded()).dismissable(false)
        ));
    }

    #[test]
    fn the_installed_dismiss_callback_replaces_the_builders() {
        // `show_overlay_modal` installs `controller.pop()` through the trait;
        // Cancel then routes to *that*, not to whatever the builder set.
        let installed = OverlayModalContent::on_modal_dismiss(
            dialog(seeded()),
            Rc::new(|app: &mut App| app.dismissed += 100),
        );
        let mut app = App::default();
        installed.cancel(&mut app);
        assert_eq!(app.dismissed, 100);
    }

    // ---- entry mode -------------------------------------------------------

    #[test]
    fn the_mode_affordance_exists_only_for_an_unlocked_entry_mode() {
        assert!(dialog(seeded()).toggle_view().is_some());
        assert!(
            dialog(seeded().with_entry_mode(DatePickerEntryMode::Input))
                .toggle_view()
                .is_some()
        );
        assert!(
            dialog(seeded().with_entry_mode(DatePickerEntryMode::CalendarOnly))
                .toggle_view()
                .is_none()
        );
        assert!(
            dialog(seeded().with_entry_mode(DatePickerEntryMode::InputOnly))
                .toggle_view()
                .is_none()
        );
    }

    #[test]
    fn the_panel_width_follows_the_entry_mode() {
        assert_eq!(
            dialog(seeded()).panel_width(),
            DIALOG_PORTRAIT_CALENDAR_WIDTH
        );
        assert_eq!(
            dialog(seeded().with_entry_mode(DatePickerEntryMode::Input)).panel_width(),
            DIALOG_PORTRAIT_INPUT_WIDTH
        );
    }

    #[test]
    fn the_headline_shows_the_selection_or_the_placeholder() {
        assert_eq!(dialog(seeded()).headline(), "Thu, Aug 20");
        assert_eq!(
            dialog(DatePickerState::new(None, d(2026, 8, 20))).headline(),
            DatePickerStrings::ENGLISH.no_date_selected
        );
    }

    // ---- panel composition -------------------------------------------------

    #[test]
    fn the_panel_builds_and_reconciles_across_an_entry_mode_swap() {
        // The body swaps concrete view types (calendar <-> input field); the
        // erased child handles that, and the affordance pod survives.
        let mut counter = 0u64;
        let calendar = dialog(seeded());
        let mut element = View::<App>::build(&calendar, &mut BuildCtx::new(&mut counter));
        let input = dialog(seeded().with_entry_mode(DatePickerEntryMode::Input));
        let flags = View::<App>::rebuild(
            &input,
            &calendar,
            &mut element,
            &mut BuildCtx::new(&mut counter),
        );
        assert!(!flags.is_empty(), "an entry-mode swap dirties the panel");

        // ...and back, plus a swap into a locked mode that drops the affordance.
        let locked = dialog(seeded().with_entry_mode(DatePickerEntryMode::CalendarOnly));
        View::<App>::rebuild(
            &locked,
            &input,
            &mut element,
            &mut BuildCtx::new(&mut counter),
        );
        View::<App>::teardown(&locked, &mut element, &mut BuildCtx::new(&mut counter));
    }

    #[test]
    fn a_calendar_sub_view_change_reconciles_in_place() {
        let mut counter = 0u64;
        let day = dialog(seeded());
        let mut element = View::<App>::build(&day, &mut BuildCtx::new(&mut counter));
        let year = dialog(seeded().with_mode(DatePickerMode::Year));
        let flags =
            View::<App>::rebuild(&year, &day, &mut element, &mut BuildCtx::new(&mut counter));
        assert!(!flags.is_empty());
    }
}
