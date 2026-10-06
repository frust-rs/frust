// Ported from `material_3_expressive` v1.0.8 (MIT, © 2026 Paa Developments;
// `tmp/material_3_expressive/lib/components/time_pickers/` —
// `m3e_time_pickers.dart` (`M3ETimePicker.show`), `m3e_time_picker_dialog.dart`,
// `models/m3e_time.dart`, `enums/m3e_time_picker_enums.dart`,
// `utils/m3e_time_picker_utils.dart`, `res/m3e_time_picker_constants.dart`,
// `styles/m3e_time_picker_theme.dart`, and `components/` —
// `m3e_time_picker_header.dart`, `m3e_time_picker_actions.dart`,
// `m3e_time_picker_dialog_content.dart`; retrieved 2026-08-20).
// Upstream: <https://github.com/paadevelopments/material_3_expressive>
//
// Porting decisions (each documented in the module docs below): every piece of
// picker state is a controlled prop rather than `setState`/`RestorableValue`;
// the reference's `MaterialLocalizations` reads become one English-defaulted
// `TimePickerStrings` bundle; the orientation is an explicit prop rather than
// a `MediaQuery` read; and the panel is content-sized rather than animating
// between the reference's four fixed dialog boxes.

//! The M3E **time picker**: a clock dial ([`mod@dial`]), a text entry form
//! ([`mod@input`]), and the modal dialog that presents either of them
//! ([`time_picker`]/[`show_time_picker`]).
//!
//! ```ignore
//! // `time`/`entry`/`entry_mode`/`autovalidate` all live in the app's own
//! // state; the picker only ever *reports* the change it wants.
//! let time = state.time;
//! // Minted outside the page builder: OK/Cancel dismiss through the host's
//! // exit ramp rather than popping the navigator from inside the panel.
//! let dismiss = ModalDismiss::new();
//! show_time_picker(
//!     &state.nav,
//!     move || {
//!         let confirm = dismiss.clone();
//!         let cancel = dismiss.clone();
//!         time_picker(time)
//!             .on_change(|s: &mut State, t| s.time = t)
//!             .on_confirm(move |s: &mut State, confirmed| match confirmed {
//!                 Some(picked) => confirm.dismiss_with(PopResult::of(picked)),
//!                 // The typed entry didn't parse — light the error lines up.
//!                 None => s.time_autovalidate = true,
//!             })
//!             .on_cancel(move |_s: &mut State| cancel.dismiss())
//!             .dismiss_handle(dismiss.clone())
//!     },
//!     |state: &mut State, result: PopResult| {
//!         if let Some(picked) = result.take::<TimeOfDay>() {
//!             state.time = picked;
//!         }
//!     },
//! );
//! ```
//!
//! # Controlled all the way down
//!
//! The reference's dialog is a `StatefulWidget` holding the selected time, the
//! entry mode and the autovalidate flag in `RestorableValue`s, and its input
//! form holds two `TextEditingController`s besides. This port holds **none**
//! of that: [`TimePickerView`] takes [`value`](TimePickerView::value),
//! [`entry`](TimePickerView::entry), [`entry_mode`](TimePickerView::entry_mode)
//! and [`autovalidate`](TimePickerView::autovalidate) as props and reports
//! each requested change back out — the same contract
//! [`crate::dialog::selection_dialog`] documents, and for the same reason: a
//! navigator-pushed page has no local-component-state hook to lean on, and
//! this crate's controls are controlled by charter
//! (`docs/CODE_STANDARDS.md`'s Interaction Semantics). The one exception is
//! which unit the ring is editing, which stays inside [`TimeDialWidget`] the
//! way `pressed` does.
//!
//! [`TimePickerView::entry`] is optional: leave it unset and the input form
//! seeds itself from `value` on every pass ([`TimeEntry::from_time`]), which is
//! right for a dial-only picker and wrong the moment the user types, so an
//! app offering input mode threads its own [`TimeEntry`] in.
//!
//! # Strings: English defaults, overridable as a bundle
//!
//! Every user-visible string the reference reads from `MaterialLocalizations`
//! lives in [`TimePickerStrings`], defaulted to Flutter's own English values.
//! An app localizes by handing the picker a filled-in bundle
//! ([`TimePickerView::strings`]); this crate reaches no i18n dependency of its
//! own (the design-system charter forbids depending on a sibling plugin — see
//! `docs/PLUGINS_ARCHITECTURE.md`'s Layer Dependencies).
//!
//! # Orientation is a prop, not a `MediaQuery` read
//!
//! The reference resolves `widget.orientation ?? MediaQuery.orientationOf(context)`.
//! A widget here has no window handle, so [`TimePickerView::orientation`] is
//! explicit and [`TimePickerOrientation::for_size`] is the rule an app applies
//! to its own window size — the same shape [`crate::search::SearchViewMode::for_width`]
//! already uses for its presentation split. Both layouts are ported:
//! portrait stacks header / divider / body / actions, landscape puts a
//! [`HEADER_LANDSCAPE_WIDTH`]-wide header column beside a vertical divider and
//! the body/actions column.
//!
//! # Panel geometry (cited)
//!
//! [`DIALOG_PORTRAIT_WIDTH`] (328) and [`DIALOG_LANDSCAPE_WIDTH`] (544) are
//! `M3ETimePickerConstants`' `dialPortraitDialogSize.width` /
//! `dialLandscapeDialogSize.width` (its input sizes share both widths), fed to
//! the host's `OverlayModalConfig::centered`. The reference's fixed dialog
//! *heights* and its 200ms `AnimatedContainer`/`AnimatedSize` crossfade
//! between them are deliberately **not** ported: this panel is content-sized,
//! so a taller type scale grows the dialog rather than clipping it, and the
//! host owns the entrance/exit motion for every modal in this crate. The dial
//! body's natural height (80 header + 24 gap + 256 ring = 360) equals the
//! reference's own `dialDialogBodyHeight`, so the two agree anyway at the
//! default type scale.

pub mod dial;
pub mod input;

pub use dial::{DIAL_SIZE, DIAL_SLOTS, TimeDialView, TimeDialWidget, time_dial};
pub use input::{TimeInputView, TimeInputWidget, time_input};

use std::rc::Rc;

use frust::authoring::text::{FontWeight, LineHeight};
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, ThemeTextColor, ThemeTextType, View, Widget,
    any, build_child, rebuild_child, route_event, route_event_single, teardown_child,
    visit_children,
};
use frust::{NavigatorController, PopResult, text};
use kurbo::{Point, Size};

use crate::button::{button, text_button};
use crate::divider::divider;
use crate::icon_button::icon_button;
use crate::overlay::{
    ModalDismiss, OverlayModalConfig, OverlayModalContent, OverlayModalView, OverlayModalWidget,
    overlay_modal, show_overlay_modal,
};

/// Panel padding on all four edges (`M3EDialogTheme.padding`, the padding the
/// reference's header/actions/content all inset by).
const PADDING: f64 = 24.0;
/// Horizontal spacing between the cancel and confirm actions
/// (`M3EDialogTheme.actionGap`).
const ACTION_GAP: f64 = 8.0;
/// Width of the landscape header column (`M3ETimePickerTheme.headerLandscapeWidth`).
pub const HEADER_LANDSCAPE_WIDTH: f64 = 176.0;
/// Panel width in portrait (`M3ETimePickerConstants.dialPortraitDialogSize.width`,
/// shared with `inputPortraitDialogSize`).
pub const DIALOG_PORTRAIT_WIDTH: f64 = 328.0;
/// Panel width in landscape (`dialLandscapeDialogSize.width`, shared with
/// `inputLandscapeDialogSize`).
pub const DIALOG_LANDSCAPE_WIDTH: f64 = 544.0;

/// Help-line type-scale token (M3 `labelLarge`, 14/20 w500).
const HELP_SIZE: f32 = 14.0;
const HELP_LINE_HEIGHT: f32 = 20.0;
const HELP_WEIGHT: FontWeight = FontWeight::MEDIUM;
/// Headline token for the formatted time (M3 `headlineLarge`, 32/40 w400 —
/// `M3ETimePickerTheme.headerHeadlineStyle`).
const TITLE_SIZE: f32 = 32.0;
const TITLE_LINE_HEIGHT: f32 = 40.0;
/// The short headline the landscape input layout uses instead
/// (`headerHeadlineShortStyle`, M3 `headlineSmall`).
const TITLE_SHORT_SIZE: f32 = 24.0;
const TITLE_SHORT_LINE_HEIGHT: f32 = 32.0;
/// Divider thickness (`crate::divider`'s own 1dp line).
const DIVIDER_THICKNESS: f64 = 1.0;

// ============================================================================
// Model
// ============================================================================

/// An immutable wall-clock time in 24-hour form (`M3ETime`).
///
/// Every constructor clamps (`M3ETimePickerUtils.clampRaw`), so an instance is
/// always `hour in 0..=23`, `minute in 0..=59` — the reference's `assert`s made
/// total, since a release-mode Dart assert is a silent no-op and this crate
/// must not hand a malformed hour to the dial's arithmetic.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TimeOfDay {
    hour: u8,
    minute: u8,
}

impl TimeOfDay {
    /// 00:00.
    pub const MIDNIGHT: TimeOfDay = TimeOfDay { hour: 0, minute: 0 };

    /// A time, with both halves clamped into range.
    pub fn new(hour: u8, minute: u8) -> Self {
        TimeOfDay {
            hour: hour.min(23),
            minute: minute.min(59),
        }
    }

    /// A time from a 12-hour hour (`1..=12`) plus a period flag —
    /// `M3ETimePickerUtils.to24Hour`: `(hour12 % 12) + if pm { 12 }`.
    pub fn from_hour_12(hour_12: u8, pm: bool, minute: u8) -> Self {
        let base = hour_12 % 12;
        TimeOfDay::new(if pm { base + 12 } else { base }, minute)
    }

    /// The hour, `0..=23`.
    pub fn hour(self) -> u8 {
        self.hour
    }

    /// The minute, `0..=59`.
    pub fn minute(self) -> u8 {
        self.minute
    }

    /// Whether this time is noon or later (`M3ETime.isPm`).
    pub fn is_pm(self) -> bool {
        self.hour >= 12
    }

    /// The hour in 12-hour form, `1..=12` (`M3ETime.hourOf12`).
    pub fn hour_of_12(self) -> u8 {
        let value = self.hour % 12;
        if value == 0 { 12 } else { value }
    }

    /// This time with a different hour (clamped).
    pub fn with_hour(self, hour: u8) -> Self {
        TimeOfDay::new(hour, self.minute)
    }

    /// This time with a different minute (clamped).
    pub fn with_minute(self, minute: u8) -> Self {
        TimeOfDay::new(self.hour, minute)
    }

    /// This time moved to AM or PM, keeping the clock face
    /// (`_M3EDialTimePickerState._setPeriod`).
    pub fn with_period(self, pm: bool) -> Self {
        TimeOfDay::from_hour_12(self.hour_of_12(), pm, self.minute)
    }

    /// The zero-padded two-digit hour the header field shows — the 24-hour
    /// hour or the 12-hour one, per format (`_buildHeader`'s `hourText`).
    pub fn hour_label(self, use_24_hour: bool) -> String {
        let value = if use_24_hour {
            self.hour
        } else {
            self.hour_of_12()
        };
        format!("{value:02}")
    }

    /// The zero-padded two-digit minute (`M3ETime.minuteLabel`).
    pub fn minute_label(self) -> String {
        format!("{:02}", self.minute)
    }

    /// The dialog headline: `"HH:MM"` in 24-hour form, `"H:MM AM"` otherwise —
    /// this port's stand-in for `MaterialLocalizations.formatTimeOfDay`, whose
    /// per-locale patterns have no home in this crate (see the [module
    /// docs](self)' *Strings*).
    pub fn format(self, use_24_hour: bool, strings: &TimePickerStrings) -> String {
        if use_24_hour {
            format!("{:02}:{:02}", self.hour, self.minute)
        } else {
            let period = if self.is_pm() {
                &strings.pm
            } else {
                &strings.am
            };
            format!("{}:{:02} {}", self.hour_of_12(), self.minute, period)
        }
    }
}

/// The raw contents of the entry form's two fields plus its AM/PM flag — see
/// [`mod@input`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TimeEntry {
    /// The hour field's text, exactly as typed.
    pub hour_text: String,
    /// The minute field's text, exactly as typed.
    pub minute_text: String,
    /// Whether the PM half is selected (ignored in 24-hour mode).
    pub pm: bool,
}

impl TimeEntry {
    /// Seed both fields from `time`, zero-padded (`_applyInitialTime`).
    pub fn from_time(time: TimeOfDay, use_24_hour: bool) -> Self {
        TimeEntry {
            hour_text: time.hour_label(use_24_hour),
            minute_text: time.minute_label(),
            pm: time.is_pm(),
        }
    }

    /// Both fields blank, AM selected (`emptyInitialInput`).
    pub fn empty() -> Self {
        TimeEntry::default()
    }

    /// This entry with a different hour text.
    pub fn with_hour_text(mut self, text: impl Into<String>) -> Self {
        self.hour_text = text.into();
        self
    }

    /// This entry with a different minute text.
    pub fn with_minute_text(mut self, text: impl Into<String>) -> Self {
        self.minute_text = text.into();
        self
    }

    /// This entry with a different period flag.
    pub fn with_pm(mut self, pm: bool) -> Self {
        self.pm = pm;
        self
    }

    /// Whether the hour text parses into range — `isValidHourText`: `0..=23`
    /// in 24-hour mode, `1..=12` otherwise.
    pub fn hour_is_valid(&self, use_24_hour: bool) -> bool {
        match self.hour_text.trim().parse::<i64>() {
            Ok(value) if use_24_hour => (0..=23).contains(&value),
            Ok(value) => (1..=12).contains(&value),
            Err(_) => false,
        }
    }

    /// Whether the minute text parses into `0..=59` (`isValidMinuteText`).
    pub fn minute_is_valid(&self) -> bool {
        matches!(self.minute_text.trim().parse::<i64>(), Ok(v) if (0..=59).contains(&v))
    }

    /// The time this entry spells out, or `None` if either half is invalid
    /// (`parseInputTime`).
    pub fn parse(&self, use_24_hour: bool) -> Option<TimeOfDay> {
        if !self.hour_is_valid(use_24_hour) || !self.minute_is_valid() {
            return None;
        }
        let hour: i64 = self.hour_text.trim().parse().ok()?;
        let minute: i64 = self.minute_text.trim().parse().ok()?;
        let (hour, minute) = (hour as u8, minute as u8);
        Some(if use_24_hour {
            TimeOfDay::new(hour, minute)
        } else {
            TimeOfDay::from_hour_12(hour, self.pm, minute)
        })
    }
}

/// Which face the picker opens on, and whether the mode toggle is offered
/// (`M3ETimePickerEntryMode`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TimePickerEntryMode {
    /// The dial, with a toggle to text entry.
    #[default]
    Dial,
    /// Text entry, with a toggle back to the dial.
    Input,
    /// The dial, with no toggle.
    DialOnly,
    /// Text entry, with no toggle.
    InputOnly,
}

impl TimePickerEntryMode {
    /// Whether this mode shows the entry form rather than the dial.
    pub fn is_input(self) -> bool {
        matches!(
            self,
            TimePickerEntryMode::Input | TimePickerEntryMode::InputOnly
        )
    }

    /// Whether the header offers the mode-toggle affordance.
    pub fn shows_toggle(self) -> bool {
        matches!(self, TimePickerEntryMode::Dial | TimePickerEntryMode::Input)
    }

    /// The mode a toggle press asks for — `_handleEntryModeToggle`: the two
    /// `*Only` modes are fixed and report themselves unchanged.
    pub fn toggled(self) -> Self {
        match self {
            TimePickerEntryMode::Dial => TimePickerEntryMode::Input,
            TimePickerEntryMode::Input => TimePickerEntryMode::Dial,
            fixed => fixed,
        }
    }
}

/// Which unit the dial's ring is editing (`M3ETimePickerMode`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TimePickerMode {
    /// The hour hand/field.
    #[default]
    Hour,
    /// The minute hand/field.
    Minute,
}

/// The panel arrangement — see the [module docs](self)' *Orientation*.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TimePickerOrientation {
    /// Header above the body, actions below (`Orientation.portrait`).
    #[default]
    Portrait,
    /// Header beside the body (`Orientation.landscape`).
    Landscape,
}

impl TimePickerOrientation {
    /// The orientation a window of `size` calls for — landscape iff it is
    /// wider than it is tall, matching `MediaQuery.orientationOf`.
    pub fn for_size(size: Size) -> Self {
        if size.width > size.height {
            TimePickerOrientation::Landscape
        } else {
            TimePickerOrientation::Portrait
        }
    }

    /// Whether this is the landscape arrangement.
    pub fn is_landscape(self) -> bool {
        matches!(self, TimePickerOrientation::Landscape)
    }

    /// The panel width this arrangement asks the host for.
    pub fn panel_width(self) -> f64 {
        if self.is_landscape() {
            DIALOG_LANDSCAPE_WIDTH
        } else {
            DIALOG_PORTRAIT_WIDTH
        }
    }
}

/// Every user-visible string the picker needs, defaulted to Flutter's own
/// English `MaterialLocalizations` values — see the [module docs](self)'
/// *Strings*.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimePickerStrings {
    /// Help line above the dial (`timePickerDialHelpText`).
    pub dial_help: String,
    /// Help line above the entry form (`timePickerInputHelpText`).
    pub input_help: String,
    /// Hour field label (`timePickerHourLabel`).
    pub hour_label: String,
    /// Minute field label (`timePickerMinuteLabel`).
    pub minute_label: String,
    /// Error shown under a malformed field (`invalidTimeLabel`).
    pub invalid_time: String,
    /// AM abbreviation (`anteMeridiemAbbreviation`).
    pub am: String,
    /// PM abbreviation (`postMeridiemAbbreviation`).
    pub pm: String,
    /// Cancel action (`cancelButtonLabel`).
    pub cancel: String,
    /// Confirm action (`okButtonLabel`).
    pub confirm: String,
    /// Tooltip on the toggle that switches to the dial (`dialModeButtonLabel`).
    pub dial_mode_tooltip: String,
    /// Tooltip on the toggle that switches to text entry
    /// (`inputTimeModeButtonLabel`).
    pub input_mode_tooltip: String,
    /// Announced while the ring edits hours (`timePickerHourModeAnnouncement`).
    pub hour_mode_announcement: String,
    /// Announced while the ring edits minutes
    /// (`timePickerMinuteModeAnnouncement`).
    pub minute_mode_announcement: String,
}

impl Default for TimePickerStrings {
    fn default() -> Self {
        TimePickerStrings {
            dial_help: "Select time".into(),
            input_help: "Enter time".into(),
            hour_label: "Hour".into(),
            minute_label: "Minute".into(),
            invalid_time: "Enter a valid time".into(),
            am: "AM".into(),
            pm: "PM".into(),
            cancel: "Cancel".into(),
            confirm: "OK".into(),
            dial_mode_tooltip: "Switch to dial picker mode".into(),
            input_mode_tooltip: "Switch to text input mode".into(),
            hour_mode_announcement: "Select hours".into(),
            minute_mode_announcement: "Select minutes".into(),
        }
    }
}

// ============================================================================
// Panel
// ============================================================================

/// A view-held, typed no-argument callback (erased on build).
type OnAction<State> = Rc<dyn Fn(&mut State)>;

/// The dialog panel's content: help line + headline + optional mode toggle,
/// a divider, the picker body, and the cancel/confirm row.
struct TimePickerPanelView<State: 'static> {
    help: String,
    title: String,
    short_title: bool,
    orientation: TimePickerOrientation,
    input_mode: bool,
    mode_button: Option<PanelModeButton<State>>,
    body: Rc<AnyView<State>>,
    cancel_label: String,
    confirm_label: String,
    on_cancel: Option<OnAction<State>>,
    on_confirm: Option<OnAction<State>>,
}

/// The mode-toggle affordance's icon/tooltip/callback triple.
struct PanelModeButton<State: 'static> {
    to_input: bool,
    tooltip: String,
    on_press: OnAction<State>,
}

/// The retained widget for a [`TimePickerPanelView`].
struct TimePickerPanelWidget {
    help: ChildPod,
    title: ChildPod,
    mode_button: Option<ChildPod>,
    divider: ChildPod,
    body: ChildPod,
    actions: Vec<ChildPod>,
    orientation: TimePickerOrientation,
    input_mode: bool,
}

/// The help line: `labelLarge` metrics and family, `on_surface_variant`.
fn help_view<State: 'static>(s: &str) -> AnyView<State> {
    any::<State, _>(
        text(s.to_string())
            .size(HELP_SIZE)
            .weight(HELP_WEIGHT)
            .line_height(LineHeight::Absolute(HELP_LINE_HEIGHT))
            .themed_role(ThemeTextColor::OnSurfaceVariant)
            .themed_family(ThemeTextType::LabelLarge),
    )
}

/// The formatted-time headline: `headlineLarge` metrics and family, or
/// `headlineSmall`'s for the `short` landscape-input variant.
fn title_view<State: 'static>(s: &str, short: bool) -> AnyView<State> {
    let (size, line_height, family) = if short {
        (
            TITLE_SHORT_SIZE,
            TITLE_SHORT_LINE_HEIGHT,
            ThemeTextType::HeadlineSmall,
        )
    } else {
        (TITLE_SIZE, TITLE_LINE_HEIGHT, ThemeTextType::HeadlineLarge)
    };
    any::<State, _>(
        text(s.to_string())
            .size(size)
            .line_height(LineHeight::Absolute(line_height))
            .themed_family(family),
    )
}

fn divider_view<State: 'static>(vertical: bool) -> AnyView<State> {
    let d = divider();
    any::<State, _>(if vertical { d.vertical() } else { d })
}

impl<State: 'static> TimePickerPanelView<State> {
    fn mode_button_view(&self) -> Option<AnyView<State>> {
        let spec = self.mode_button.as_ref()?;
        let on_press = spec.on_press.clone();
        let glyph = if spec.to_input {
            crate::icons::EDIT_OUTLINED
        } else {
            crate::icons::SCHEDULE
        };
        Some(any(icon_button(
            frust::icon(glyph),
            move |state: &mut State| {
                on_press(state);
            },
        )
        .semantic_label(spec.tooltip.clone())))
    }

    fn action_views(&self) -> Vec<AnyView<State>> {
        let cancel = self.on_cancel.clone();
        let confirm = self.on_confirm.clone();
        vec![
            any(text_button(
                self.cancel_label.clone(),
                move |state: &mut State| {
                    if let Some(cancel) = &cancel {
                        cancel(state);
                    }
                },
            )),
            any(button(
                self.confirm_label.clone(),
                move |state: &mut State| {
                    if let Some(confirm) = &confirm {
                        confirm(state);
                    }
                },
            )),
        ]
    }
}

impl<State: 'static> View<State> for TimePickerPanelView<State> {
    type Element = TimePickerPanelWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> TimePickerPanelWidget {
        TimePickerPanelWidget {
            help: build_child(&help_view::<State>(&self.help), ctx),
            title: build_child(&title_view::<State>(&self.title, self.short_title), ctx),
            mode_button: self.mode_button_view().map(|v| build_child(&v, ctx)),
            divider: build_child(&divider_view::<State>(self.orientation.is_landscape()), ctx),
            body: build_child(self.body.as_ref(), ctx),
            actions: self
                .action_views()
                .iter()
                .map(|v| build_child(v, ctx))
                .collect(),
            orientation: self.orientation,
            input_mode: self.input_mode,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut TimePickerPanelWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(
            &help_view::<State>(&prev.help),
            &help_view::<State>(&self.help),
            &mut element.help,
            ctx,
        );
        flags |= rebuild_child(
            &title_view::<State>(&prev.title, prev.short_title),
            &title_view::<State>(&self.title, self.short_title),
            &mut element.title,
            ctx,
        );

        match (prev.mode_button_view(), self.mode_button_view()) {
            (Some(p), Some(n)) => {
                if let Some(pod) = element.mode_button.as_mut() {
                    flags |= rebuild_child(&p, &n, pod, ctx);
                }
            }
            (Some(p), None) => {
                if let Some(pod) = element.mode_button.as_mut() {
                    teardown_child(&p, pod, ctx);
                }
                element.mode_button = None;
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            (None, Some(n)) => {
                element.mode_button = Some(build_child(&n, ctx));
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            (None, None) => {}
        }

        flags |= rebuild_child(
            &divider_view::<State>(prev.orientation.is_landscape()),
            &divider_view::<State>(self.orientation.is_landscape()),
            &mut element.divider,
            ctx,
        );
        flags |= rebuild_child(
            prev.body.as_ref(),
            self.body.as_ref(),
            &mut element.body,
            ctx,
        );
        for ((p, n), pod) in prev
            .action_views()
            .iter()
            .zip(self.action_views().iter())
            .zip(element.actions.iter_mut())
        {
            flags |= rebuild_child(p, n, pod, ctx);
        }

        if element.orientation != self.orientation || element.input_mode != self.input_mode {
            element.orientation = self.orientation;
            element.input_mode = self.input_mode;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut TimePickerPanelWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&help_view::<State>(&self.help), &mut element.help, ctx);
        teardown_child(
            &title_view::<State>(&self.title, self.short_title),
            &mut element.title,
            ctx,
        );
        if let (Some(view), Some(pod)) = (self.mode_button_view(), element.mode_button.as_mut()) {
            teardown_child(&view, pod, ctx);
        }
        teardown_child(
            &divider_view::<State>(self.orientation.is_landscape()),
            &mut element.divider,
            ctx,
        );
        teardown_child(self.body.as_ref(), &mut element.body, ctx);
        for (view, pod) in self.action_views().iter().zip(element.actions.iter_mut()) {
            teardown_child(view, pod, ctx);
        }
    }
}

impl TimePickerPanelWidget {
    /// Lay the help line and (optional) mode toggle out as one row inside
    /// `[x, x + width]`, returning the row's height (`M3ETimePickerHeader`'s
    /// `helpRow`).
    fn layout_help_row(&mut self, ctx: &mut LayoutCtx, x: f64, y: f64, width: f64) -> f64 {
        let button_w = match self.mode_button.as_mut() {
            Some(pod) => {
                pod.layout_child(ctx, &BoxConstraints::loose(Size::new(width, f64::INFINITY)))
                    .width
            }
            None => 0.0,
        };
        let help_w = (width - button_w).max(0.0);
        let help_h = self
            .help
            .layout_child(
                ctx,
                &BoxConstraints::new(Size::new(help_w, 0.0), Size::new(help_w, f64::INFINITY)),
            )
            .height;
        let button_h = self.mode_button.as_ref().map_or(0.0, |p| p.size().height);
        let row_h = help_h.max(button_h);
        self.help
            .set_origin(Point::new(x, y + (row_h - help_h) / 2.0));
        if let Some(pod) = self.mode_button.as_mut() {
            pod.set_origin(Point::new(
                x + width - button_w,
                y + (row_h - button_h) / 2.0,
            ));
        }
        row_h
    }

    /// Lay the cancel/confirm row out right-aligned inside `[x, x + width]`,
    /// returning its height (`M3ETimePickerActions`).
    fn layout_actions(&mut self, ctx: &mut LayoutCtx, x: f64, y: f64, width: f64) -> f64 {
        let loose = BoxConstraints::loose(Size::new(width, f64::INFINITY));
        let mut row_h: f64 = 0.0;
        for pod in &mut self.actions {
            let s = pod.layout_child(ctx, &loose);
            row_h = row_h.max(s.height);
        }
        let mut right = x + width;
        for pod in self.actions.iter_mut().rev() {
            let w = pod.size().width;
            right -= w;
            pod.set_origin(Point::new(right, y));
            right -= ACTION_GAP;
        }
        row_h
    }

    /// The body's own inset — the reference's `M3ETimePickerDialogContent`:
    /// full padding in input mode, vertical-only for the dial (whose ring
    /// centres itself across the full panel width).
    fn body_inset(&self) -> f64 {
        if self.input_mode { PADDING } else { 0.0 }
    }

    fn layout_portrait(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints, width: f64) -> Size {
        let inner = (width - 2.0 * PADDING).max(0.0);
        let mut y = PADDING;
        y += self.layout_help_row(ctx, PADDING, y, inner);
        let title_h = self
            .title
            .layout_child(
                ctx,
                &BoxConstraints::new(Size::new(inner, 0.0), Size::new(inner, f64::INFINITY)),
            )
            .height;
        self.title.set_origin(Point::new(PADDING, y));
        y += title_h + PADDING;

        self.divider.layout_child(
            ctx,
            &BoxConstraints::tight(Size::new(width, DIVIDER_THICKNESS)),
        );
        self.divider.set_origin(Point::new(0.0, y));
        y += DIVIDER_THICKNESS;

        let inset = self.body_inset();
        let body_w = (width - 2.0 * inset).max(0.0);
        y += PADDING;
        let body_h = self
            .body
            .layout_child(
                ctx,
                &BoxConstraints::new(Size::new(body_w, 0.0), Size::new(body_w, f64::INFINITY)),
            )
            .height;
        self.body.set_origin(Point::new(inset, y));
        y += body_h + PADDING;

        y += self.layout_actions(ctx, PADDING, y, inner);
        y += PADDING;

        bc.constrain(Size::new(width, y))
    }

    fn layout_landscape(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints, width: f64) -> Size {
        let header_w = HEADER_LANDSCAPE_WIDTH.min(width);
        let header_inner = (header_w - 2.0 * PADDING).max(0.0);
        let mut header_y = PADDING;
        header_y += self.layout_help_row(ctx, PADDING, header_y, header_inner);
        let title_h = self
            .title
            .layout_child(
                ctx,
                &BoxConstraints::new(
                    Size::new(header_inner, 0.0),
                    Size::new(header_inner, f64::INFINITY),
                ),
            )
            .height;
        self.title.set_origin(Point::new(PADDING, header_y));
        header_y += title_h + PADDING;

        let body_x = header_w + DIVIDER_THICKNESS;
        let column_w = (width - body_x).max(0.0);
        let inset = self.body_inset();
        let body_w = (column_w - 2.0 * inset).max(0.0);
        let mut column_y = PADDING;
        let body_h = self
            .body
            .layout_child(
                ctx,
                &BoxConstraints::new(Size::new(body_w, 0.0), Size::new(body_w, f64::INFINITY)),
            )
            .height;
        self.body.set_origin(Point::new(body_x + inset, column_y));
        column_y += body_h + PADDING;
        let actions_w = (column_w - 2.0 * PADDING).max(0.0);
        column_y += self.layout_actions(ctx, body_x + PADDING, column_y, actions_w);
        column_y += PADDING;

        let height = header_y.max(column_y);
        self.divider.layout_child(
            ctx,
            &BoxConstraints::tight(Size::new(DIVIDER_THICKNESS, height)),
        );
        self.divider.set_origin(Point::new(header_w, 0.0));

        bc.constrain(Size::new(width, height))
    }
}

impl Widget for TimePickerPanelWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // The host gives content a tight width (`OverlayGeometry::Centered`).
        let width = bc.max().width.max(bc.min().width);
        if self.orientation.is_landscape() {
            self.layout_landscape(ctx, bc, width)
        } else {
            self.layout_portrait(ctx, bc, width)
        }
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.help.paint_child(ctx, scene);
        self.title.paint_child(ctx, scene);
        if let Some(pod) = self.mode_button.as_mut() {
            pod.paint_child(ctx, scene);
        }
        self.divider.paint_child(ctx, scene);
        self.body.paint_child(ctx, scene);
        for pod in &mut self.actions {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if route_event_single(&mut self.body, ctx, event) == EventResult::Handled {
            return EventResult::Handled;
        }
        if let Some(pod) = self.mode_button.as_mut()
            && route_event_single(pod, ctx, event) == EventResult::Handled
        {
            return EventResult::Handled;
        }
        route_event(&mut self.actions, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.help.semantics_child(ctx);
        self.title.semantics_child(ctx);
        if let Some(pod) = &self.mode_button {
            pod.semantics_child(ctx);
        }
        self.body.semantics_child(ctx);
        for pod in &self.actions {
            pod.semantics_child(ctx);
        }
    }

    visit_children!(help, title, mode_button, divider, body, actions);
}

// ============================================================================
// time_picker() / TimePickerView / show_time_picker()
// ============================================================================

/// A view-held, typed one-argument callback (erased on build).
type OnValue<State, T> = Rc<dyn Fn(&mut State, T)>;

/// A declarative M3E time picker dialog. See the [module docs](self).
pub struct TimePickerView<State: 'static> {
    value: TimeOfDay,
    entry: Option<TimeEntry>,
    entry_mode: TimePickerEntryMode,
    orientation: TimePickerOrientation,
    use_24_hour: bool,
    autovalidate: bool,
    strings: Rc<TimePickerStrings>,
    on_change: Option<OnValue<State, TimeOfDay>>,
    on_entry_change: Option<OnValue<State, TimeEntry>>,
    on_entry_mode_change: Option<OnValue<State, TimePickerEntryMode>>,
    on_confirm: Option<OnValue<State, Option<TimeOfDay>>>,
    on_cancel: Option<OnAction<State>>,
    dismissable: bool,
    on_dismiss: Option<OnAction<State>>,
    dismiss_handle: Option<ModalDismiss>,
}

/// Create a time picker showing `value`. Chain
/// [`on_change`](TimePickerView::on_change) to observe the dial,
/// [`on_confirm`](TimePickerView::on_confirm) to handle OK, and the rest as
/// the picker's modes require — see the [module docs](self).
pub fn time_picker<State: 'static>(value: TimeOfDay) -> TimePickerView<State> {
    TimePickerView {
        value,
        entry: None,
        entry_mode: TimePickerEntryMode::default(),
        orientation: TimePickerOrientation::default(),
        use_24_hour: false,
        autovalidate: false,
        strings: Rc::new(TimePickerStrings::default()),
        on_change: None,
        on_entry_change: None,
        on_entry_mode_change: None,
        on_confirm: None,
        on_cancel: None,
        dismissable: true,
        on_dismiss: None,
        dismiss_handle: None,
    }
}

/// PascalCase alias for [`time_picker`].
#[allow(non_snake_case)]
pub fn TimePicker<State: 'static>(value: TimeOfDay) -> TimePickerView<State> {
    time_picker(value)
}

impl<State: 'static> TimePickerView<State> {
    /// The time the dial shows and the confirm action reports (controlled).
    pub fn value(mut self, value: TimeOfDay) -> Self {
        self.value = value;
        self
    }

    /// The entry form's raw text (controlled). Unset, the form re-seeds from
    /// [`value`](Self::value) every pass — see the [module docs](self).
    pub fn entry(mut self, entry: TimeEntry) -> Self {
        self.entry = Some(entry);
        self
    }

    /// Which face to show, and whether to offer the mode toggle
    /// (`initialEntryMode`, controlled here rather than seeded).
    pub fn entry_mode(mut self, entry_mode: TimePickerEntryMode) -> Self {
        self.entry_mode = entry_mode;
        self
    }

    /// The panel arrangement (`orientation`) — see the [module docs](self).
    pub fn orientation(mut self, orientation: TimePickerOrientation) -> Self {
        self.orientation = orientation;
        self
    }

    /// Use the 24-hour ring/validation and drop the AM/PM selector
    /// (`alwaysUse24HourFormat`).
    pub fn use_24_hour(mut self, use_24_hour: bool) -> Self {
        self.use_24_hour = use_24_hour;
        self
    }

    /// Show the entry form's error lines — set this from
    /// [`on_confirm`](Self::on_confirm)'s `None` arm, the reference's own
    /// `AutovalidateMode.always` flip.
    pub fn autovalidate(mut self, autovalidate: bool) -> Self {
        self.autovalidate = autovalidate;
        self
    }

    /// Replace every user-visible string (see the [module docs](self)).
    pub fn strings(mut self, strings: TimePickerStrings) -> Self {
        self.strings = Rc::new(strings);
        self
    }

    /// Observe a dial edit — the *requested* time, clamped and complete.
    pub fn on_change<F: Fn(&mut State, TimeOfDay) + 'static>(mut self, on_change: F) -> Self {
        self.on_change = Some(Rc::new(on_change));
        self
    }

    /// Observe a keystroke or AM/PM press in the entry form — the *requested*
    /// [`TimeEntry`], which the caller threads back through [`entry`](Self::entry).
    pub fn on_entry_change<F: Fn(&mut State, TimeEntry) + 'static>(mut self, on_change: F) -> Self {
        self.on_entry_change = Some(Rc::new(on_change));
        self
    }

    /// Observe a mode-toggle press — the requested [`TimePickerEntryMode`].
    pub fn on_entry_mode_change<F: Fn(&mut State, TimePickerEntryMode) + 'static>(
        mut self,
        on_change: F,
    ) -> Self {
        self.on_entry_mode_change = Some(Rc::new(on_change));
        self
    }

    /// Observe the confirm action. `Some(time)` is the confirmed value —
    /// `value` on the dial, the parsed entry in input mode. `None` is the
    /// reference's failed-validation branch (`_handleOk`'s early return): the
    /// typed entry does not spell a time, so the caller flips
    /// [`autovalidate`](Self::autovalidate) on instead of popping.
    pub fn on_confirm<F: Fn(&mut State, Option<TimeOfDay>) + 'static>(
        mut self,
        on_confirm: F,
    ) -> Self {
        self.on_confirm = Some(Rc::new(on_confirm));
        self
    }

    /// Observe the cancel action (`_handleCancel`).
    pub fn on_cancel<F: Fn(&mut State) + 'static>(mut self, on_cancel: F) -> Self {
        self.on_cancel = Some(Rc::new(on_cancel));
        self
    }

    /// Whether a scrim tap, `Escape` or Android back may dismiss the dialog —
    /// see [`crate::overlay::modal::OverlayModalView::dismissable`].
    pub fn dismissable(mut self, dismissable: bool) -> Self {
        self.dismissable = dismissable;
        self
    }

    /// Observe a scrim-tap/`Escape`/back dismissal ([`show_time_picker`]
    /// already wires the pop).
    pub fn on_dismiss<F: Fn(&mut State) + 'static>(mut self, on_dismiss: F) -> Self {
        self.on_dismiss = Some(Rc::new(on_dismiss));
        self
    }

    /// Install the app-triggered staged-dismiss handle, so the OK/Cancel
    /// callbacks can close the dialog through the host's exit ramp
    /// ([`ModalDismiss::dismiss_with`] / [`ModalDismiss::dismiss`]) instead of
    /// popping the navigator themselves, which skips the ramp entirely.
    ///
    /// Unlike [`crate::show_date_picker`], this family leaves both actions to
    /// the caller (the module docs' worked example), so the handle is the
    /// caller's to mint and wire — outside [`show_time_picker`]'s page builder.
    pub fn dismiss_handle(mut self, dismiss: ModalDismiss) -> Self {
        self.dismiss_handle = Some(dismiss);
        self
    }

    /// The entry form's current contents — the caller's own, or one seeded
    /// from `value`.
    fn resolved_entry(&self) -> TimeEntry {
        self.entry
            .clone()
            .unwrap_or_else(|| TimeEntry::from_time(self.value, self.use_24_hour))
    }

    /// What the confirm action reports — see [`Self::on_confirm`].
    fn confirmed(&self) -> Option<TimeOfDay> {
        if self.entry_mode.is_input() {
            self.resolved_entry().parse(self.use_24_hour)
        } else {
            Some(self.value)
        }
    }

    fn body_view(&self) -> AnyView<State> {
        if self.entry_mode.is_input() {
            let on_change = self.on_entry_change.clone();
            any(
                time_input(self.resolved_entry(), move |state: &mut State, e| {
                    if let Some(on_change) = &on_change {
                        on_change(state, e);
                    }
                })
                .use_24_hour(self.use_24_hour)
                .autovalidate(self.autovalidate)
                .shared_strings(self.strings.clone()),
            )
        } else {
            let on_change = self.on_change.clone();
            any(time_dial(self.value, move |state: &mut State, t| {
                if let Some(on_change) = &on_change {
                    on_change(state, t);
                }
            })
            .use_24_hour(self.use_24_hour)
            .shared_strings(self.strings.clone()))
        }
    }

    /// Compose the host-facing modal fresh from the current fields (the same
    /// per-pass recompose [`crate::dialog`] documents).
    fn compose(&self) -> OverlayModalView<State> {
        let input_mode = self.entry_mode.is_input();
        let help = if input_mode {
            self.strings.input_help.clone()
        } else {
            self.strings.dial_help.clone()
        };
        let confirmed = self.confirmed();
        let on_confirm = self.on_confirm.clone();
        let mode_button = self.entry_mode.shows_toggle().then(|| {
            let requested = self.entry_mode.toggled();
            let on_mode_change = self.on_entry_mode_change.clone();
            PanelModeButton {
                to_input: !input_mode,
                tooltip: if input_mode {
                    self.strings.dial_mode_tooltip.clone()
                } else {
                    self.strings.input_mode_tooltip.clone()
                },
                on_press: Rc::new(move |state: &mut State| {
                    if let Some(on_mode_change) = &on_mode_change {
                        on_mode_change(state, requested);
                    }
                }),
            }
        });
        let panel = TimePickerPanelView {
            help,
            title: self.value.format(self.use_24_hour, &self.strings),
            // `isShort`: the landscape input layout only.
            short_title: self.orientation.is_landscape() && input_mode,
            orientation: self.orientation,
            input_mode,
            mode_button,
            body: Rc::new(self.body_view()),
            cancel_label: self.strings.cancel.clone(),
            confirm_label: self.strings.confirm.clone(),
            on_cancel: self.on_cancel.clone(),
            on_confirm: Some(Rc::new(move |state: &mut State| {
                if let Some(on_confirm) = &on_confirm {
                    on_confirm(state, confirmed);
                }
            })),
        };
        let mut view = overlay_modal(
            panel,
            OverlayModalConfig::centered(self.orientation.panel_width()),
        )
        .dismissable(self.dismissable)
        .label(if input_mode {
            self.strings.input_help.clone()
        } else {
            self.strings.dial_help.clone()
        });
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

impl<State: 'static> View<State> for TimePickerView<State> {
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

impl<State: 'static> OverlayModalContent<State> for TimePickerView<State> {
    fn on_modal_dismiss(mut self, on_dismiss: Rc<dyn Fn(&mut State)>) -> Self {
        self.on_dismiss = Some(on_dismiss);
        self
    }

    fn modal_dismissable(&self) -> bool {
        self.dismissable
    }
}

/// Push `build`'s time picker as a transparent navigator page over the host's
/// scrim, and register `on_result` for the value it pops with — the same push
/// helper every modal component in this crate uses
/// ([`crate::overlay::modal::show_overlay_modal`]).
///
/// Dismissal (scrim tap, `Escape`, Android back) is wired to
/// `controller.pop()` for you, staged behind the host's exit ramp. Confirm and
/// cancel are the caller's own actions: dismiss with a value from
/// [`TimePickerView::on_confirm`]'s `Some` arm and bare from
/// [`TimePickerView::on_cancel`], both through a [`ModalDismiss`] the caller
/// mints and installs with [`TimePickerView::dismiss_handle`] (see the [module
/// docs](self)' worked example) — a direct `controller.pop()` from either
/// action would skip that ramp.
pub fn show_time_picker<State, B, R>(
    controller: &NavigatorController<State>,
    build: B,
    on_result: R,
) where
    State: 'static,
    B: Fn() -> TimePickerView<State> + 'static,
    R: Fn(&mut State, PopResult) + 'static,
{
    show_overlay_modal(controller, build, on_result);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::text::TextContext;
    use frust::authoring::{
        CornerRadii, Key, KeyEvent, Modifiers, NamedKey, PointerButton, PointerEvent, PointerPhase,
    };
    use frust::{Color, FrameTime, NavigatorView};
    use frust_core::RenderRoot;
    use frust_widgets::navigator;
    use kurbo::Rect;
    use std::any::Any;

    #[test]
    fn a_time_clamps_both_halves_on_construction() {
        assert_eq!(TimeOfDay::new(99, 99), TimeOfDay::new(23, 59));
        assert_eq!(TimeOfDay::new(23, 59).hour(), 23);
        assert_eq!(TimeOfDay::default(), TimeOfDay::MIDNIGHT);
        assert_eq!(TimeOfDay::MIDNIGHT.hour_of_12(), 12, "00:00 reads as 12 AM");
    }

    #[test]
    fn the_twelve_hour_view_of_every_hour_round_trips() {
        for hour in 0..24u8 {
            let t = TimeOfDay::new(hour, 0);
            let back = TimeOfDay::from_hour_12(t.hour_of_12(), t.is_pm(), 0);
            assert_eq!(back, t, "hour {hour} must survive the 12-hour round trip");
        }
    }

    #[test]
    fn with_period_keeps_the_clock_face() {
        let ten_am = TimeOfDay::new(10, 30);
        assert_eq!(ten_am.with_period(true), TimeOfDay::new(22, 30));
        assert_eq!(ten_am.with_period(false), ten_am, "already AM");
        // Noon and midnight are the interesting pair: both read as "12".
        assert_eq!(
            TimeOfDay::new(12, 0).with_period(false),
            TimeOfDay::new(0, 0)
        );
        assert_eq!(
            TimeOfDay::new(0, 0).with_period(true),
            TimeOfDay::new(12, 0)
        );
    }

    #[test]
    fn labels_are_zero_padded_per_format() {
        let t = TimeOfDay::new(9, 5);
        assert_eq!(t.hour_label(true), "09");
        assert_eq!(t.hour_label(false), "09");
        assert_eq!(TimeOfDay::new(21, 5).hour_label(true), "21");
        assert_eq!(TimeOfDay::new(21, 5).hour_label(false), "09");
        assert_eq!(t.minute_label(), "05");
    }

    #[test]
    fn the_headline_follows_the_format() {
        let strings = TimePickerStrings::default();
        assert_eq!(TimeOfDay::new(21, 5).format(true, &strings), "21:05");
        assert_eq!(TimeOfDay::new(21, 5).format(false, &strings), "9:05 PM");
        assert_eq!(TimeOfDay::new(0, 0).format(false, &strings), "12:00 AM");
        assert_eq!(TimeOfDay::new(12, 0).format(false, &strings), "12:00 PM");
    }

    #[test]
    fn the_toggle_is_a_no_op_for_the_fixed_modes() {
        assert_eq!(
            TimePickerEntryMode::Dial.toggled(),
            TimePickerEntryMode::Input
        );
        assert_eq!(
            TimePickerEntryMode::Input.toggled(),
            TimePickerEntryMode::Dial
        );
        for fixed in [
            TimePickerEntryMode::DialOnly,
            TimePickerEntryMode::InputOnly,
        ] {
            assert_eq!(fixed.toggled(), fixed);
            assert!(!fixed.shows_toggle());
        }
        assert!(TimePickerEntryMode::Dial.shows_toggle());
        assert!(TimePickerEntryMode::Input.shows_toggle());
        assert!(TimePickerEntryMode::InputOnly.is_input());
        assert!(!TimePickerEntryMode::DialOnly.is_input());
    }

    #[test]
    fn the_orientation_rule_matches_media_query() {
        assert_eq!(
            TimePickerOrientation::for_size(Size::new(392.0, 800.0)),
            TimePickerOrientation::Portrait
        );
        assert_eq!(
            TimePickerOrientation::for_size(Size::new(800.0, 392.0)),
            TimePickerOrientation::Landscape
        );
        // A square window is portrait (`width > height` is strict).
        assert_eq!(
            TimePickerOrientation::for_size(Size::new(600.0, 600.0)),
            TimePickerOrientation::Portrait
        );
        assert_eq!(
            TimePickerOrientation::Portrait.panel_width(),
            DIALOG_PORTRAIT_WIDTH
        );
        assert_eq!(
            TimePickerOrientation::Landscape.panel_width(),
            DIALOG_LANDSCAPE_WIDTH
        );
    }

    #[test]
    fn confirm_reports_the_dial_value_in_dial_mode() {
        let view: TimePickerView<()> = time_picker(TimeOfDay::new(14, 45));
        assert_eq!(view.confirmed(), Some(TimeOfDay::new(14, 45)));
    }

    #[test]
    fn confirm_parses_the_entry_in_input_mode_and_reports_none_when_it_cannot() {
        let good: TimePickerView<()> = time_picker(TimeOfDay::MIDNIGHT)
            .entry_mode(TimePickerEntryMode::Input)
            .entry(TimeEntry {
                hour_text: "7".into(),
                minute_text: "05".into(),
                pm: true,
            });
        assert_eq!(good.confirmed(), Some(TimeOfDay::new(19, 5)));

        let bad: TimePickerView<()> = time_picker(TimeOfDay::MIDNIGHT)
            .entry_mode(TimePickerEntryMode::InputOnly)
            .entry(TimeEntry {
                hour_text: "99".into(),
                minute_text: "".into(),
                pm: false,
            });
        assert_eq!(
            bad.confirmed(),
            None,
            "the failed-validation branch reports None instead of a guessed time"
        );
    }

    #[test]
    fn an_unset_entry_seeds_itself_from_the_value() {
        let view: TimePickerView<()> = time_picker(TimeOfDay::new(21, 5))
            .entry_mode(TimePickerEntryMode::Input)
            .use_24_hour(true);
        assert_eq!(view.resolved_entry().hour_text, "21");
        assert_eq!(view.confirmed(), Some(TimeOfDay::new(21, 5)));
    }

    // ------------------------------------------------------------------
    // Panel layout
    // ------------------------------------------------------------------

    /// A fixed-size leaf standing in for the picker body, so the panel's own
    /// geometry can be asserted without depending on text-shaping metrics.
    struct Sized {
        size: Size,
    }
    struct SizedW {
        size: Size,
    }
    impl View<NavState> for Sized {
        type Element = SizedW;
        fn build(&self, _c: &mut BuildCtx<'_>) -> SizedW {
            SizedW { size: self.size }
        }
        fn rebuild(&self, _p: &Self, _e: &mut SizedW, _c: &mut BuildCtx<'_>) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    impl Widget for SizedW {
        fn layout(&mut self, _c: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(self.size)
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            scene.fill_rect(ctx.origin(), ctx.size(), Color::BLACK);
        }
    }

    fn panel_view(
        orientation: TimePickerOrientation,
        input_mode: bool,
    ) -> TimePickerPanelView<NavState> {
        TimePickerPanelView {
            help: "Select time".into(),
            title: "10:30 AM".into(),
            short_title: false,
            orientation,
            input_mode,
            mode_button: None,
            body: Rc::new(any(Sized {
                size: Size::new(200.0, 100.0),
            })),
            cancel_label: "Cancel".into(),
            confirm_label: "OK".into(),
            on_cancel: None,
            on_confirm: None,
        }
    }

    fn mount_panel(
        view: &TimePickerPanelView<NavState>,
        width: f64,
    ) -> (TimePickerPanelWidget, Size, TextContext) {
        let mut counter = 0u64;
        let mut widget = View::<NavState>::build(view, &mut BuildCtx::new(&mut counter));
        let mut tcx = TextContext::new();
        let size = {
            let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
            widget.layout(
                &mut ctx,
                &BoxConstraints::new(Size::new(width, 0.0), Size::new(width, f64::INFINITY)),
            )
        };
        (widget, size, tcx)
    }

    #[test]
    fn the_portrait_panel_stacks_header_divider_body_and_actions() {
        let view = panel_view(TimePickerOrientation::Portrait, false);
        let (widget, size, _tcx) = mount_panel(&view, DIALOG_PORTRAIT_WIDTH);
        assert_eq!(size.width, DIALOG_PORTRAIT_WIDTH);
        assert_eq!(widget.help.origin().x, PADDING);
        assert!(
            widget.title.origin().y > widget.help.origin().y,
            "title below help"
        );
        assert!(
            widget.divider.origin().y > widget.title.origin().y,
            "divider below the header block"
        );
        assert_eq!(
            widget.divider.size().width,
            DIALOG_PORTRAIT_WIDTH,
            "full bleed"
        );
        assert!(widget.body.origin().y > widget.divider.origin().y);
        assert_eq!(
            widget.body.origin().x,
            0.0,
            "the dial body is not inset — it centres itself"
        );
        // The action row is right-aligned, confirm outermost.
        let cancel = widget.actions[0].origin().x;
        let confirm = widget.actions[1].origin().x;
        assert!(
            confirm > cancel,
            "confirm sits closest to the trailing edge"
        );
        assert!(
            (confirm + widget.actions[1].size().width - (DIALOG_PORTRAIT_WIDTH - PADDING)).abs()
                < 1e-9
        );
        assert!(widget.actions[0].origin().y > widget.body.origin().y);
    }

    #[test]
    fn the_input_body_is_inset_but_the_dial_body_is_not() {
        let dial = panel_view(TimePickerOrientation::Portrait, false);
        let (dial_widget, _, _t1) = mount_panel(&dial, DIALOG_PORTRAIT_WIDTH);
        assert_eq!(dial_widget.body.origin().x, 0.0);
        assert_eq!(dial_widget.body.size().width, DIALOG_PORTRAIT_WIDTH);

        let input = panel_view(TimePickerOrientation::Portrait, true);
        let (input_widget, _, _t2) = mount_panel(&input, DIALOG_PORTRAIT_WIDTH);
        assert_eq!(input_widget.body.origin().x, PADDING);
        assert_eq!(
            input_widget.body.size().width,
            DIALOG_PORTRAIT_WIDTH - 2.0 * PADDING
        );
    }

    #[test]
    fn the_landscape_panel_puts_the_header_column_beside_the_body() {
        let view = panel_view(TimePickerOrientation::Landscape, false);
        let (widget, size, _tcx) = mount_panel(&view, DIALOG_LANDSCAPE_WIDTH);
        assert_eq!(size.width, DIALOG_LANDSCAPE_WIDTH);
        assert_eq!(widget.help.origin().x, PADDING, "header column is inset");
        assert!(widget.help.origin().x < HEADER_LANDSCAPE_WIDTH);
        assert_eq!(
            widget.divider.origin(),
            Point::new(HEADER_LANDSCAPE_WIDTH, 0.0)
        );
        assert_eq!(
            widget.divider.size().width,
            DIVIDER_THICKNESS,
            "vertical rule"
        );
        assert_eq!(widget.divider.size().height, size.height, "full height");
        assert!(
            widget.body.origin().x >= HEADER_LANDSCAPE_WIDTH + DIVIDER_THICKNESS,
            "the body sits right of the divider"
        );
        assert!(
            widget.actions[0].origin().y > widget.body.origin().y,
            "actions sit under the body, inside the right-hand column"
        );
        assert!(widget.actions[0].origin().x > HEADER_LANDSCAPE_WIDTH);
    }

    // ------------------------------------------------------------------
    // Dialog presentation and result plumbing
    // ------------------------------------------------------------------

    /// The window every dialog test lays out in — tall enough for the dial
    /// panel's full content height, so the actions row is reachable.
    const WINDOW: Size = Size::new(420.0, 900.0);

    #[derive(Default)]
    struct NavState {
        results: Vec<Option<TimeOfDay>>,
        confirmed: Vec<Option<TimeOfDay>>,
        cancels: u32,
        value: TimeOfDay,
    }

    #[derive(Default)]
    struct Recorder {
        glyphs: usize,
    }
    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn draw_glyph_run(&mut self, _run: GlyphRun) {
            self.glyphs += 1;
        }
        fn fill_rounded_rect(&mut self, _o: Point, _s: Size, _r: f64, _c: Color) {}
        fn fill_rounded_rect_radii(&mut self, _o: Point, _s: Size, _radii: CornerRadii, _c: Color) {
        }
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn escape_event() -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::Escape),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    type NavLogic = Box<dyn FnMut(&mut NavState) -> NavigatorView<NavState>>;

    struct Harness {
        root: RenderRoot<NavState, NavigatorView<NavState>>,
        state: NavState,
        tcx: TextContext,
        controller: NavigatorController<NavState>,
        logic: NavLogic,
    }

    impl Harness {
        fn new() -> Self {
            let controller: NavigatorController<NavState> = NavigatorController::new();
            let ctrl = controller.clone();
            let mut h = Harness {
                root: RenderRoot::new(),
                state: NavState::default(),
                tcx: TextContext::new(),
                controller,
                logic: Box::new(move |_: &mut NavState| {
                    navigator(&ctrl, || {
                        any::<NavState, _>(Sized {
                            size: Size::new(WINDOW.width, WINDOW.height),
                        })
                    })
                }),
            };
            h.pass();
            h
        }

        fn pass(&mut self) {
            self.root.rebuild(&mut self.logic, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
        }

        /// Settle the modal's entrance ramp.
        fn settle(&mut self) {
            self.root.paint(&mut Recorder::default(), FrameTime::ZERO);
            self.root.paint(
                &mut Recorder::default(),
                FrameTime::from_nanos(1_000_000_000),
            );
            self.pass();
        }

        /// Settle a running staged-exit ramp so its enqueued pop drains.
        fn settle_exit(&mut self) {
            self.root.paint(
                &mut Recorder::default(),
                FrameTime::from_nanos(2_000_000_000),
            );
            self.root.paint(
                &mut Recorder::default(),
                FrameTime::from_nanos(3_000_000_000),
            );
            self.pass();
        }

        fn event(&mut self, event: &InputEvent) {
            self.root.event(&mut self.state, event);
        }

        fn flush(&mut self) {
            self.pass();
            self.event(&ev(PointerPhase::Move, 2.0, 2.0));
        }

        fn paint(&mut self) -> Recorder {
            let mut rec = Recorder::default();
            self.root
                .paint(&mut rec, FrameTime::from_nanos(4_000_000_000));
            rec
        }

        /// The absolute bounds of the picker's action buttons, leading first.
        /// Both actions are `crate::button`'s widget (the confirm one filled,
        /// the cancel one text), so they are found by type and ordered by x.
        fn action_bounds(&self) -> Vec<Rect> {
            let mut found: Vec<Rect> = self
                .root
                .inspect()
                .into_iter()
                .filter(|n| n.type_name.contains("::ButtonWidget"))
                .map(|n| n.bounds)
                .collect();
            found.sort_by(|a, b| a.x0.partial_cmp(&b.x0).expect("finite bounds"));
            found
        }

        fn tap(&mut self, rect: Rect) {
            let c = rect.center();
            self.event(&ev(PointerPhase::Down, c.x, c.y));
            self.event(&ev(PointerPhase::Up, c.x, c.y));
            self.flush();
        }
    }

    fn show_dial_picker(h: &mut Harness) {
        let ctrl = h.controller.clone();
        show_time_picker(
            &h.controller,
            move || {
                let ctrl = ctrl.clone();
                let cancel_ctrl = ctrl.clone();
                time_picker(TimeOfDay::new(14, 45))
                    .on_confirm(move |state: &mut NavState, confirmed| {
                        state.confirmed.push(confirmed);
                        if let Some(time) = confirmed {
                            ctrl.pop_with_result(PopResult::of(time));
                        }
                    })
                    .on_cancel(move |state: &mut NavState| {
                        state.cancels += 1;
                        cancel_ctrl.pop();
                    })
            },
            |state: &mut NavState, result: PopResult| {
                state.results.push(result.take::<TimeOfDay>());
            },
        );
        h.pass();
        h.settle();
    }

    #[test]
    fn the_picker_renders_its_header_body_and_actions() {
        let mut h = Harness::new();
        show_dial_picker(&mut h);
        let rec = h.paint();
        // Help line, headline, two header fields, the colon, AM/PM, twelve
        // ring labels, and both action labels all shape.
        assert!(
            rec.glyphs >= 15,
            "the whole picker shapes, got {} runs",
            rec.glyphs
        );
    }

    #[test]
    fn confirming_pops_the_chosen_time_through_to_the_result_callback() {
        let mut h = Harness::new();
        show_dial_picker(&mut h);
        let actions = h.action_bounds();
        assert_eq!(actions.len(), 2, "cancel and confirm");
        // The confirm action is the trailing one.
        h.tap(actions[1]);
        h.settle_exit();
        assert_eq!(
            h.state.confirmed,
            vec![Some(TimeOfDay::new(14, 45))],
            "the dial's value is what confirm reports"
        );
        assert_eq!(
            h.state.results,
            vec![Some(TimeOfDay::new(14, 45))],
            "and it reaches the result callback through the pop"
        );
    }

    #[test]
    fn cancelling_pops_with_no_result() {
        let mut h = Harness::new();
        show_dial_picker(&mut h);
        let actions = h.action_bounds();
        h.tap(actions[0]);
        h.settle_exit();
        assert_eq!(h.state.cancels, 1);
        assert_eq!(h.state.confirmed, Vec::new(), "confirm never fired");
        assert_eq!(h.state.results, vec![None], "an empty pop result");
    }

    #[test]
    fn escape_dismisses_the_picker_with_an_empty_result() {
        let mut h = Harness::new();
        show_dial_picker(&mut h);
        // A press inside the panel's header band claims focus (host-owned)
        // without touching the dial or an action.
        h.event(&ev(PointerPhase::Down, 200.0, 200.0));
        h.event(&ev(PointerPhase::Up, 200.0, 200.0));
        h.flush();
        assert!(h.state.results.is_empty());
        h.event(&escape_event());
        h.flush();
        assert!(h.state.results.is_empty(), "still animating out");
        h.settle_exit();
        h.flush();
        assert_eq!(h.state.results, vec![None]);
        assert_eq!(h.state.confirmed, Vec::new());
    }

    #[test]
    fn an_invalid_entry_refuses_to_confirm_and_leaves_the_dialog_up() {
        let mut h = Harness::new();
        let ctrl = h.controller.clone();
        show_time_picker(
            &h.controller,
            move || {
                let ctrl = ctrl.clone();
                time_picker(TimeOfDay::MIDNIGHT)
                    .entry_mode(TimePickerEntryMode::InputOnly)
                    .entry(TimeEntry {
                        hour_text: "99".into(),
                        minute_text: String::new(),
                        pm: false,
                    })
                    .on_confirm(move |state: &mut NavState, confirmed| {
                        state.confirmed.push(confirmed);
                        if let Some(time) = confirmed {
                            ctrl.pop_with_result(PopResult::of(time));
                        }
                    })
            },
            |state: &mut NavState, result: PopResult| {
                state.results.push(result.take::<TimeOfDay>());
            },
        );
        h.pass();
        h.settle();
        let actions = h.action_bounds();
        // The entry form's own AM/PM pair are buttons too, so the picker's
        // actions are the last two in reading order.
        let confirm = *actions.last().expect("a confirm action");
        h.tap(confirm);
        h.settle_exit();
        assert_eq!(
            h.state.confirmed,
            vec![None],
            "the failed-validation branch fires with None"
        );
        assert!(h.state.results.is_empty(), "and nothing pops");
    }

    #[test]
    fn the_mode_toggle_reports_the_other_entry_mode() {
        let mut h = Harness::new();
        let requested: Rc<std::cell::RefCell<Vec<TimePickerEntryMode>>> = Rc::default();
        let sink = requested.clone();
        show_time_picker(
            &h.controller,
            move || {
                let sink = sink.clone();
                time_picker(TimeOfDay::new(9, 0)).on_entry_mode_change(
                    move |_state: &mut NavState, mode| sink.borrow_mut().push(mode),
                )
            },
            |_state: &mut NavState, _result: PopResult| {},
        );
        h.pass();
        h.settle();
        let toggle = h
            .root
            .inspect()
            .into_iter()
            .find(|n| n.type_name.contains("::IconButtonWidget"))
            .expect("the dial mode offers a toggle")
            .bounds;
        h.tap(toggle);
        assert_eq!(
            requested.borrow().as_slice(),
            &[TimePickerEntryMode::Input],
            "the dial's toggle asks for text entry"
        );
    }

    #[test]
    fn a_dial_edit_reports_the_requested_time_out_of_the_dialog() {
        let mut h = Harness::new();
        let ctrl = h.controller.clone();
        show_time_picker(
            &h.controller,
            move || {
                let _ = &ctrl;
                time_picker(TimeOfDay::new(14, 45))
                    .on_change(|state: &mut NavState, time| state.value = time)
            },
            |_state: &mut NavState, _result: PopResult| {},
        );
        h.pass();
        h.settle();
        // The ring's centre sits inside the panel; a press at the ring's
        // 12 o'clock slot is found from the dial widget's own bounds.
        let ring = h
            .root
            .inspect()
            .into_iter()
            .find(|n| n.type_name.contains("::TimeDialWidget"))
            .expect("the dial is mounted")
            .bounds;
        // The ring occupies the dial's bottom `DIAL_SIZE` square, centred.
        let ring_center = Point::new(ring.center().x, ring.y1 - DIAL_SIZE / 2.0);
        let top = Point::new(ring_center.x, ring_center.y - DIAL_SIZE / 2.0 + 30.0);
        h.event(&ev(PointerPhase::Down, top.x, top.y));
        h.event(&ev(PointerPhase::Up, top.x, top.y));
        h.flush();
        assert_eq!(
            h.state.value,
            TimeOfDay::new(12, 45),
            "12 o'clock in the PM half is 12:45"
        );
    }

    #[test]
    fn an_installed_dismiss_handle_reaches_the_host_view() {
        // OK/Cancel are the caller's own here, so the handle they dismiss
        // through has to survive the recompose; the staging itself is the
        // host's, and tested there.
        let dismiss = ModalDismiss::new();
        assert!(
            time_picker::<NavState>(TimeOfDay::new(9, 30))
                .dismiss_handle(dismiss)
                .compose()
                .dismiss_handle
                .is_some()
        );
        assert!(
            time_picker::<NavState>(TimeOfDay::new(9, 30))
                .compose()
                .dismiss_handle
                .is_none(),
            "and none is invented for a picker that was handed none"
        );
    }
}
