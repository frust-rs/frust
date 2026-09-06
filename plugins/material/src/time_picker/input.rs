// Ported from `material_3_expressive` v1.0.8 (MIT, © 2026 Paa Developments;
// `tmp/material_3_expressive/lib/components/time_pickers/components/m3e_input_time_picker_form_field.dart`
// plus `utils/m3e_time_picker_utils.dart`'s validation/parse helpers,
// retrieved 2026-08-20).
// Upstream: <https://github.com/paadevelopments/material_3_expressive>
//
// Porting decisions (documented below): the reference's two internal
// `TextEditingController`s become one caller-owned `TimeEntry` prop, since
// this crate's whole editable family is controlled; and `Form`/`FormField`
// autovalidation becomes a plain `autovalidate` flag, since there is no form
// registry to save/validate through.

//! The M3E time picker's **text entry** mode: an hour field, a minute field,
//! and (in 12-hour mode) an AM/PM button pair.
//!
//! [`time_input`] is a controlled component over [`TimeEntry`] — the *raw*
//! text of both fields plus the AM/PM flag — rather than over a
//! [`super::TimeOfDay`], because a half-typed field ("1", "", "9x") has no
//! [`super::TimeOfDay`] to be. The caller threads its own `TimeEntry` back in on
//! every edit, exactly the way `crate::dialog`'s `selection_dialog` threads
//! `.selected(..)`; [`TimeEntry::parse`] turns a complete entry back into a
//! time.
//!
//! # Validation (cited)
//!
//! [`TimeEntry::hour_is_valid`]/[`TimeEntry::minute_is_valid`] are
//! `M3ETimePickerUtils.isValidHourText`/`isValidMinuteText`: an hour parses to
//! `0..=23` in 24-hour mode and `1..=12` in 12-hour mode, a minute to
//! `0..=59`, and anything that does not parse at all is invalid. Text is
//! trimmed first (`_parseFields`). [`TimeEntry::parse`] is
//! `parseInputTime`: both halves must validate, then the 12-hour hour is
//! folded through `to24Hour(hour12, pm)`.
//!
//! # Errors appear on demand, not while typing
//!
//! The reference starts at `AutovalidateMode.disabled` and only flips to
//! `always` once an OK press fails validation
//! (`_M3ETimePickerDialogState._handleOk`). [`TimeInputView::autovalidate`]
//! is that flip: `false` (the default) paints no error however malformed the
//! text is; `true` paints `strings.invalid_time` under whichever field is
//! invalid.

use std::rc::Rc;

use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, TypedArgCallback, View, Widget, any,
    build_child, rebuild_child, route_event, route_event_single, teardown_child, visit_children,
};
use kurbo::{Point, Size};

use super::{TimeEntry, TimePickerStrings};
use crate::button::{ButtonVariant, button};
use crate::text_field::text_field;

/// Gap between the hour and minute fields, and between the field row and the
/// AM/PM row (`_buildHourMinuteRow`'s `SizedBox(width: 16)` and
/// `build`'s `SizedBox(height: 16)`).
const FIELD_GAP: f64 = 16.0;
/// Gap between the AM and PM buttons (`_buildMeridiemRow`'s
/// `SizedBox(width: 8)`).
const PERIOD_GAP: f64 = 8.0;

/// A declarative time entry form. See the [module docs](self).
pub struct TimeInputView<State: 'static> {
    entry: TimeEntry,
    use_24_hour: bool,
    autovalidate: bool,
    strings: Rc<TimePickerStrings>,
    on_change: TypedArgCallback<State, TimeEntry>,
}

/// Create a controlled time entry form over `entry`, reporting each requested
/// entry through `on_change(state, requested)`.
pub fn time_input<State: 'static, F: Fn(&mut State, TimeEntry) + 'static>(
    entry: TimeEntry,
    on_change: F,
) -> TimeInputView<State> {
    TimeInputView {
        entry,
        use_24_hour: false,
        autovalidate: false,
        strings: Rc::new(TimePickerStrings::default()),
        on_change: Rc::new(on_change),
    }
}

impl<State: 'static> TimeInputView<State> {
    /// Validate the hour against `0..=23` and drop the AM/PM row
    /// (`alwaysUse24HourFormat`).
    pub fn use_24_hour(mut self, use_24_hour: bool) -> Self {
        self.use_24_hour = use_24_hour;
        self
    }

    /// Show the error line under any invalid field — see the [module
    /// docs](self)' *Errors appear on demand*.
    pub fn autovalidate(mut self, autovalidate: bool) -> Self {
        self.autovalidate = autovalidate;
        self
    }

    /// Override the field labels, the AM/PM labels and the error text.
    pub fn strings(mut self, strings: TimePickerStrings) -> Self {
        self.strings = Rc::new(strings);
        self
    }

    /// Share an already-`Rc`'d string bundle (used by
    /// [`crate::time_picker::time_picker`]).
    pub(super) fn shared_strings(mut self, strings: Rc<TimePickerStrings>) -> Self {
        self.strings = strings;
        self
    }

    /// The error text for a field, or `None` while autovalidation is off or
    /// the field is well-formed.
    fn error_for(&self, valid: bool) -> Option<&str> {
        (self.autovalidate && !valid).then_some(self.strings.invalid_time.as_str())
    }

    fn hour_field(&self) -> AnyView<State> {
        let on_change = self.on_change.clone();
        let entry = self.entry.clone();
        let mut field = text_field(
            self.entry.hour_text.clone(),
            move |state: &mut State, text: String| {
                on_change(state, entry.clone().with_hour_text(text));
            },
        )
        .label(self.strings.hour_label.clone());
        if let Some(error) = self.error_for(self.entry.hour_is_valid(self.use_24_hour)) {
            field = field.error_text(error);
        }
        any(field)
    }

    fn minute_field(&self) -> AnyView<State> {
        let on_change = self.on_change.clone();
        let entry = self.entry.clone();
        let mut field = text_field(
            self.entry.minute_text.clone(),
            move |state: &mut State, text: String| {
                on_change(state, entry.clone().with_minute_text(text));
            },
        )
        .label(self.strings.minute_label.clone());
        if let Some(error) = self.error_for(self.entry.minute_is_valid()) {
            field = field.error_text(error);
        }
        any(field)
    }

    /// One AM/PM button — filled while selected, outlined otherwise
    /// (`_buildMeridiemRow`).
    fn period_button(&self, pm: bool) -> AnyView<State> {
        let on_change = self.on_change.clone();
        let entry = self.entry.clone();
        let label = if pm {
            self.strings.pm.clone()
        } else {
            self.strings.am.clone()
        };
        let selected = self.entry.pm == pm;
        any(button(label, move |state: &mut State| {
            on_change(state, entry.clone().with_pm(pm));
        })
        .variant(if selected {
            ButtonVariant::Filled
        } else {
            ButtonVariant::Outlined
        }))
    }

    /// The period row's two buttons, or an empty list in 24-hour mode.
    fn period_buttons(&self) -> Vec<AnyView<State>> {
        if self.use_24_hour {
            Vec::new()
        } else {
            vec![self.period_button(false), self.period_button(true)]
        }
    }
}

/// The retained widget for a [`TimeInputView`].
pub struct TimeInputWidget {
    hour: ChildPod,
    minute: ChildPod,
    /// AM then PM, empty in 24-hour mode.
    period: Vec<ChildPod>,
}

impl<State: 'static> View<State> for TimeInputView<State> {
    type Element = TimeInputWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> TimeInputWidget {
        TimeInputWidget {
            hour: build_child(&self.hour_field(), ctx),
            minute: build_child(&self.minute_field(), ctx),
            period: self
                .period_buttons()
                .iter()
                .map(|v| build_child(v, ctx))
                .collect(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut TimeInputWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(
            &prev.hour_field(),
            &self.hour_field(),
            &mut element.hour,
            ctx,
        );
        flags |= rebuild_child(
            &prev.minute_field(),
            &self.minute_field(),
            &mut element.minute,
            ctx,
        );

        let prev_period = prev.period_buttons();
        let next_period = self.period_buttons();
        if prev_period.len() == next_period.len() {
            for ((p, n), pod) in prev_period
                .iter()
                .zip(next_period.iter())
                .zip(element.period.iter_mut())
            {
                flags |= rebuild_child(p, n, pod, ctx);
            }
        } else {
            // The format flipped: tear the old row down and build the new one.
            for (view, pod) in prev_period.iter().zip(element.period.iter_mut()) {
                teardown_child(view, pod, ctx);
            }
            element.period = next_period.iter().map(|v| build_child(v, ctx)).collect();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut TimeInputWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.hour_field(), &mut element.hour, ctx);
        teardown_child(&self.minute_field(), &mut element.minute, ctx);
        for (view, pod) in self.period_buttons().iter().zip(element.period.iter_mut()) {
            teardown_child(view, pod, ctx);
        }
    }
}

impl Widget for TimeInputWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = bc.max().width.max(bc.min().width);
        // Two equal-width fields with one gap between them (the reference's
        // paired `Expanded`s).
        let field_w = ((width - FIELD_GAP) / 2.0).max(0.0);
        let field_bc =
            BoxConstraints::new(Size::new(field_w, 0.0), Size::new(field_w, f64::INFINITY));
        let hour = self.hour.layout_child(ctx, &field_bc);
        let minute = self.minute.layout_child(ctx, &field_bc);
        self.hour.set_origin(Point::ZERO);
        self.minute.set_origin(Point::new(field_w + FIELD_GAP, 0.0));

        let mut y = hour.height.max(minute.height);

        if !self.period.is_empty() {
            y += FIELD_GAP;
            let loose = BoxConstraints::loose(Size::new(width, f64::INFINITY));
            let mut x = 0.0;
            let mut row_h: f64 = 0.0;
            for (i, pod) in self.period.iter_mut().enumerate() {
                let s = pod.layout_child(ctx, &loose);
                if i > 0 {
                    x += PERIOD_GAP;
                }
                pod.set_origin(Point::new(x, y));
                x += s.width;
                row_h = row_h.max(s.height);
            }
            y += row_h;
        }

        bc.constrain(Size::new(width, y))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.hour.paint_child(ctx, scene);
        self.minute.paint_child(ctx, scene);
        for pod in &mut self.period {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if route_event_single(&mut self.hour, ctx, event) == EventResult::Handled {
            return EventResult::Handled;
        }
        if route_event_single(&mut self.minute, ctx, event) == EventResult::Handled {
            return EventResult::Handled;
        }
        route_event(&mut self.period, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.hour.semantics_child(ctx);
        self.minute.semantics_child(ctx);
        for pod in &self.period {
            pod.semantics_child(ctx);
        }
    }

    visit_children!(hour, minute, period);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time_picker::TimeOfDay;
    use frust::authoring::text::TextContext;
    use std::any::Any;

    #[test]
    fn hour_validity_follows_the_format() {
        for (text, ok_24, ok_12) in [
            ("0", true, false),
            ("00", true, false),
            ("1", true, true),
            ("12", true, true),
            ("13", true, false),
            ("23", true, false),
            ("24", false, false),
            ("", false, false),
            ("ab", false, false),
            ("1 2", false, false),
            ("-1", false, false),
            ("999999999999999999999", false, false),
        ] {
            let entry = TimeEntry::default().with_hour_text(text);
            assert_eq!(entry.hour_is_valid(true), ok_24, "24-hour `{text}`");
            assert_eq!(entry.hour_is_valid(false), ok_12, "12-hour `{text}`");
        }
    }

    #[test]
    fn minute_validity_is_zero_through_fifty_nine() {
        for (text, ok) in [
            ("0", true),
            ("00", true),
            ("59", true),
            ("60", false),
            ("", false),
            ("x", false),
        ] {
            assert_eq!(
                TimeEntry::default()
                    .with_minute_text(text)
                    .minute_is_valid(),
                ok,
                "`{text}`"
            );
        }
    }

    #[test]
    fn parse_folds_the_twelve_hour_half_through_the_period_flag() {
        let am = TimeEntry {
            hour_text: "12".into(),
            minute_text: "05".into(),
            pm: false,
        };
        assert_eq!(
            am.parse(false),
            Some(TimeOfDay::new(0, 5)),
            "12 AM is 00:05"
        );
        let pm = am.clone().with_pm(true);
        assert_eq!(
            pm.parse(false),
            Some(TimeOfDay::new(12, 5)),
            "12 PM is 12:05"
        );
        let nine_pm = TimeEntry {
            hour_text: "9".into(),
            minute_text: "30".into(),
            pm: true,
        };
        assert_eq!(nine_pm.parse(false), Some(TimeOfDay::new(21, 30)));
        // In 24-hour mode the period flag is ignored outright.
        assert_eq!(nine_pm.parse(true), Some(TimeOfDay::new(9, 30)));
    }

    #[test]
    fn parse_refuses_an_incomplete_or_out_of_range_entry() {
        for entry in [
            TimeEntry::default(),
            TimeEntry::default().with_hour_text("10"),
            TimeEntry::default().with_minute_text("10"),
            TimeEntry {
                hour_text: "25".into(),
                minute_text: "00".into(),
                pm: false,
            },
            TimeEntry {
                hour_text: "10".into(),
                minute_text: "61".into(),
                pm: false,
            },
        ] {
            assert_eq!(entry.parse(true), None, "{entry:?}");
        }
    }

    #[test]
    fn surrounding_whitespace_is_trimmed_before_validating() {
        let entry = TimeEntry {
            hour_text: " 09 ".into(),
            minute_text: "\t15\n".into(),
            pm: false,
        };
        assert!(entry.hour_is_valid(true));
        assert!(entry.minute_is_valid());
        assert_eq!(entry.parse(true), Some(TimeOfDay::new(9, 15)));
    }

    #[test]
    fn from_time_seeds_zero_padded_text_and_the_period_flag() {
        let entry = TimeEntry::from_time(TimeOfDay::new(21, 5), false);
        assert_eq!(entry.hour_text, "09");
        assert_eq!(entry.minute_text, "05");
        assert!(entry.pm);
        let entry24 = TimeEntry::from_time(TimeOfDay::new(21, 5), true);
        assert_eq!(entry24.hour_text, "21");
        assert_eq!(entry24.minute_text, "05");
    }

    // ------------------------------------------------------------------
    // Widget-level behaviour
    // ------------------------------------------------------------------

    #[derive(Default)]
    struct App {
        entry: TimeEntry,
    }

    fn view(entry: TimeEntry, use_24_hour: bool) -> TimeInputView<App> {
        time_input(entry, |s: &mut App, e: TimeEntry| s.entry = e).use_24_hour(use_24_hour)
    }

    /// Build the form widget directly and lay it out against a text context —
    /// the harness `crate::switch`'s own tests use.
    fn mount(view: &TimeInputView<App>) -> (TimeInputWidget, TextContext) {
        let mut counter = 0u64;
        let mut widget = View::<App>::build(view, &mut BuildCtx::new(&mut counter));
        let mut tcx = TextContext::new();
        {
            let mut layout_ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
            widget.layout(
                &mut layout_ctx,
                &BoxConstraints::tight(Size::new(320.0, 400.0)),
            );
        }
        (widget, tcx)
    }

    #[test]
    fn twelve_hour_mode_shows_the_period_row_and_twenty_four_hour_mode_does_not() {
        let entry = TimeEntry::from_time(TimeOfDay::new(9, 15), false);
        let twelve = view(entry.clone(), false);
        let (mut widget, _tcx) = mount(&twelve);
        assert_eq!(widget.period.len(), 2);

        let mut counter = 0u64;
        let twenty_four = view(entry.clone(), true);
        View::<App>::rebuild(
            &twenty_four,
            &twelve,
            &mut widget,
            &mut BuildCtx::new(&mut counter),
        );
        assert_eq!(widget.period.len(), 0, "the row is torn down");

        let back = view(entry, false);
        View::<App>::rebuild(
            &back,
            &twenty_four,
            &mut widget,
            &mut BuildCtx::new(&mut counter),
        );
        assert_eq!(widget.period.len(), 2, "and rebuilt on the way back");
    }

    #[test]
    fn autovalidate_gates_the_error_line() {
        let entry = TimeEntry {
            hour_text: "99".into(),
            minute_text: "".into(),
            pm: false,
        };
        let quiet: TimeInputView<App> = time_input(entry.clone(), |_: &mut App, _| {});
        assert_eq!(quiet.error_for(entry.hour_is_valid(false)), None);
        let loud: TimeInputView<App> =
            time_input(entry.clone(), |_: &mut App, _| {}).autovalidate(true);
        assert_eq!(
            loud.error_for(entry.hour_is_valid(false)),
            Some(TimePickerStrings::default().invalid_time.as_str())
        );
        assert_eq!(
            loud.error_for(true),
            None,
            "a well-formed field stays quiet even under autovalidation"
        );
    }

    #[test]
    fn the_form_lays_both_fields_out_side_by_side() {
        let entry = TimeEntry::from_time(TimeOfDay::new(9, 15), false);
        let (widget, _tcx) = mount(&view(entry, false));
        assert_eq!(widget.hour.origin(), Point::ZERO);
        assert_eq!(widget.hour.origin().y, widget.minute.origin().y, "same row");
        assert!(
            (widget.minute.origin().x - (widget.hour.size().width + FIELD_GAP)).abs() < 1e-9,
            "one 16dp gap between the fields"
        );
        assert!(
            widget.period[0].origin().y >= widget.hour.size().height + FIELD_GAP,
            "the period row sits one gap below the fields"
        );
    }
}
