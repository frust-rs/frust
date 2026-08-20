// Ported from `material_3_expressive` v1.0.8's date input field (MIT, © 2026
// Paa Developments;
// `tmp/material_3_expressive/lib/components/date_pickers/components/m3e_input_date_picker_form_field.dart`,
// retrieved 2026-08-20).
// Upstream: <https://github.com/paadevelopments/material_3_expressive>
//
// Porting decisions (documented in the module docs below): validation is live
// rather than deferred behind Flutter's `AutovalidateMode.disabled`; the parse
// format is the fixed `en_US` compact one; and there is no autofocus, the
// framework exposing no programmatic-focus seam.

//! The date picker's **input mode**: [`crate::text_field`] wrapped in
//! `mm/dd/yyyy` parsing, bounds checking, and the two error strings the
//! reference's form validator produces.
//!
//! [`date_input_field`] is controlled like everything else in this family (see
//! [`mod@super`]): it shows [`DatePickerState::input_text`] and reports the next
//! state — the new draft text, plus the parsed selection whenever the text
//! resolves to a selectable date.
//!
//! # Live validation, not deferred
//!
//! The reference's dialog builds the field inside a `Form` with
//! `AutovalidateMode.disabled`, so nothing is flagged until OK is pressed
//! (`_handleOk` flips it to `always` on the first failure). This port validates
//! on every keystroke once the field is non-empty, and [`DateInputField::validate`]
//! turns that off for a caller that wants the deferred shape. The trade is
//! deliberate: an error appears while a half-typed `08/2` is still being typed,
//! but the user is never told "no" only after committing, and the picker needs
//! no extra "has OK been pressed yet" state to carry the reference's
//! `_RestorableAutovalidateMode`.
//!
//! An **empty** field never shows an error — the reference's `acceptEmptyDate`
//! branch reached from the other side: nothing typed is not yet wrong.
//!
//! # No autofocus
//!
//! The reference opens input mode with `autofocus: true` so the keyboard is up
//! immediately. The framework's baseline editable exposes no programmatic-focus
//! seam ([`crate::search`]'s view documents the same gap), so a user taps the
//! field once before the keyboard appears.

use std::rc::Rc;

use frust::authoring::{BuildCtx, ChangeFlags, View};

use super::calendar::SelectableDay;
use super::date::MaterialDate;
use super::{DatePickerState, DatePickerStrings, OnDatePickerChange};
use crate::text_field::{TextFieldView, TextFieldWidget, text_field};

/// Why a typed date was refused — the two errors the reference's `_validate`
/// distinguishes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateInputError {
    /// The text is not `mm/dd/yyyy` at all (`invalidDateFormatLabel`).
    Format,
    /// A real date, but outside the selectable bounds or refused by the
    /// caller's predicate (`invalidDateRangeLabel`).
    OutOfRange,
}

/// Parse `text` as a compact date and check it against `first..=last`.
///
/// The bounds half of the reference's `_isValidAcceptableDate`; a caller's
/// `selectableDayPredicate` is applied on top by
/// [`DateInputField::parse`], which owns the closure.
///
/// ```
/// use frust_material::{DateInputError, MaterialDate, parse_bounded};
///
/// let first = MaterialDate::new(2026, 1, 1);
/// let last = MaterialDate::new(2026, 12, 31);
/// assert_eq!(parse_bounded("08/20/2026", first, last), Ok(MaterialDate::new(2026, 8, 20)));
/// assert_eq!(parse_bounded("08/20/2027", first, last), Err(DateInputError::OutOfRange));
/// assert_eq!(parse_bounded("nonsense", first, last), Err(DateInputError::Format));
/// ```
pub fn parse_bounded(
    text: &str,
    first: MaterialDate,
    last: MaterialDate,
) -> Result<MaterialDate, DateInputError> {
    let date = MaterialDate::parse_compact(text).ok_or(DateInputError::Format)?;
    if date.is_within(first, last) {
        Ok(date)
    } else {
        Err(DateInputError::OutOfRange)
    }
}

/// A declarative date input field. See the [module docs](self).
pub struct DateInputField<State: 'static> {
    state: DatePickerState,
    first_date: MaterialDate,
    last_date: MaterialDate,
    strings: DatePickerStrings,
    selectable: Option<SelectableDay>,
    validate: bool,
    on_change: OnDatePickerChange<State>,
}

/// Create a controlled date input field over `state`, bounded by
/// `first_date..=last_date`, reporting each edit through `on_change`.
pub fn date_input_field<State: 'static, F>(
    state: DatePickerState,
    first_date: MaterialDate,
    last_date: MaterialDate,
    on_change: F,
) -> DateInputField<State>
where
    F: Fn(&mut State, DatePickerState) + 'static,
{
    let (first_date, last_date) = if last_date < first_date {
        (last_date, first_date)
    } else {
        (first_date, last_date)
    };
    DateInputField {
        state,
        first_date,
        last_date,
        strings: DatePickerStrings::ENGLISH,
        selectable: None,
        validate: true,
        on_change: Rc::new(on_change),
    }
}

impl<State: 'static> DateInputField<State> {
    /// Replace the localized strings (default [`DatePickerStrings::ENGLISH`]).
    pub fn strings(mut self, strings: DatePickerStrings) -> Self {
        self.strings = strings;
        self
    }

    /// Refuse individual dates inside the bounds — the reference's
    /// `selectableDayPredicate`, reported as [`DateInputError::OutOfRange`].
    pub fn selectable<F: Fn(MaterialDate) -> bool + 'static>(mut self, predicate: F) -> Self {
        self.selectable = Some(Rc::new(predicate));
        self
    }

    /// Whether the field shows its error while typing (default `true`). `false`
    /// is the reference's deferred `AutovalidateMode.disabled` shape — see the
    /// [module docs](self).
    pub fn validate(mut self, validate: bool) -> Self {
        self.validate = validate;
        self
    }

    /// Parse `text` against this field's bounds *and* its predicate.
    pub fn parse(&self, text: &str) -> Result<MaterialDate, DateInputError> {
        let date = parse_bounded(text, self.first_date, self.last_date)?;
        if self.selectable.as_ref().is_none_or(|p| p(date)) {
            Ok(date)
        } else {
            Err(DateInputError::OutOfRange)
        }
    }

    /// The error string this field currently shows, if any: `None` for an empty
    /// field, for a valid date, or when [`Self::validate`] is off.
    pub fn error(&self) -> Option<&'static str> {
        if !self.validate || self.state.input_text.trim().is_empty() {
            return None;
        }
        match self.parse(&self.state.input_text) {
            Ok(_) => None,
            Err(DateInputError::Format) => Some(self.strings.error_format),
            Err(DateInputError::OutOfRange) => Some(self.strings.error_invalid),
        }
    }

    /// The state typing `text` into this field requests: the draft text
    /// recorded always, and the selection updated too whenever `text` resolves
    /// to a selectable date.
    ///
    /// The draft text is re-applied *after* the selection so the user's own
    /// keystrokes survive — [`DatePickerState::with_selected`] would otherwise
    /// reformat the field out from under them.
    pub fn next_state(&self, text: &str) -> DatePickerState {
        match self.parse(text) {
            Ok(date) => self.state.with_selected(date).with_input_text(text),
            Err(_) => self.state.with_input_text(text),
        }
    }

    /// Compose the wrapped [`crate::text_field`] fresh from the current fields
    /// — the same recompose-per-pass shape [`crate::dialog`]'s own views take.
    fn compose(&self) -> TextFieldView<State> {
        let parsed = Parsed {
            state: self.state.clone(),
            first: self.first_date,
            last: self.last_date,
            selectable: self.selectable.clone(),
        };
        let on_change = self.on_change.clone();
        let field = text_field(
            self.state.input_text.clone(),
            move |app: &mut State, text: String| on_change(app, parsed.next_state(&text)),
        )
        .label(self.strings.field_label);
        match self.error() {
            Some(message) => field.error_text(message),
            None => field.supporting_text(self.strings.field_hint),
        }
    }
}

/// The parse inputs a composed field's `on_change` closure has to own (it
/// outlives the view it was built from).
struct Parsed {
    state: DatePickerState,
    first: MaterialDate,
    last: MaterialDate,
    selectable: Option<SelectableDay>,
}

impl Parsed {
    fn next_state(&self, text: &str) -> DatePickerState {
        match parse_bounded(text, self.first, self.last) {
            Ok(date) if self.selectable.as_ref().is_none_or(|p| p(date)) => {
                self.state.with_selected(date).with_input_text(text)
            }
            _ => self.state.with_input_text(text),
        }
    }
}

impl<State: 'static> View<State> for DateInputField<State> {
    type Element = TextFieldWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> TextFieldWidget {
        View::build(&self.compose(), ctx)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut TextFieldWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        View::rebuild(&self.compose(), &prev.compose(), element, ctx)
    }

    fn teardown(&self, element: &mut TextFieldWidget, ctx: &mut BuildCtx<'_>) {
        View::teardown(&self.compose(), element, ctx);
    }
}

/// The retained widget for a [`DateInputField`] — the wrapped baseline-backed
/// [`crate::text_field`]'s own, since this view contributes decoration only.
pub type DateInputFieldWidget = TextFieldWidget;

#[cfg(test)]
mod tests {
    use super::*;

    fn d(year: i32, month: u32, day: u32) -> MaterialDate {
        MaterialDate::new(year, month, day)
    }

    const FIRST: fn() -> MaterialDate = || MaterialDate::new(2020, 1, 1);
    const LAST: fn() -> MaterialDate = || MaterialDate::new(2030, 12, 31);

    #[derive(Default)]
    struct App {
        reported: Vec<DatePickerState>,
    }

    fn field(state: DatePickerState) -> DateInputField<App> {
        date_input_field(state, FIRST(), LAST(), |app: &mut App, next| {
            app.reported.push(next)
        })
    }

    fn seeded() -> DatePickerState {
        DatePickerState::new(None, d(2026, 8, 20))
    }

    // ---- parsing ---------------------------------------------------------

    #[test]
    fn parsing_splits_format_errors_from_range_errors() {
        let first = d(2026, 1, 1);
        let last = d(2026, 12, 31);
        assert_eq!(parse_bounded("06/15/2026", first, last), Ok(d(2026, 6, 15)));
        assert_eq!(parse_bounded("6/5/2026", first, last), Ok(d(2026, 6, 5)));
        // Both bounds are inclusive.
        assert_eq!(parse_bounded("01/01/2026", first, last), Ok(first));
        assert_eq!(parse_bounded("12/31/2026", first, last), Ok(last));

        assert_eq!(
            parse_bounded("12/31/2025", first, last),
            Err(DateInputError::OutOfRange)
        );
        assert_eq!(
            parse_bounded("01/01/2027", first, last),
            Err(DateInputError::OutOfRange)
        );
        for bad in ["", "08/2", "2026-06-15", "13/01/2026", "02/30/2026", "abc"] {
            assert_eq!(
                parse_bounded(bad, first, last),
                Err(DateInputError::Format),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_predicate_refusal_reads_as_out_of_range() {
        let f = field(seeded()).selectable(|date| date.day() % 2 == 0);
        assert_eq!(f.parse("06/16/2026"), Ok(d(2026, 6, 16)));
        assert_eq!(f.parse("06/15/2026"), Err(DateInputError::OutOfRange));
    }

    #[test]
    fn reversed_bounds_are_normalized_rather_than_asserted() {
        let f = date_input_field(seeded(), LAST(), FIRST(), |app: &mut App, next| {
            app.reported.push(next)
        });
        assert_eq!(f.parse("06/15/2026"), Ok(d(2026, 6, 15)));
    }

    // ---- error text -------------------------------------------------------

    #[test]
    fn an_empty_field_never_errors() {
        assert_eq!(field(seeded()).error(), None);
        assert_eq!(field(seeded().with_input_text("   ")).error(), None);
    }

    #[test]
    fn the_two_errors_map_onto_the_reference_strings() {
        let strings = DatePickerStrings::ENGLISH;
        assert_eq!(
            field(seeded().with_input_text("08/2")).error(),
            Some(strings.error_format)
        );
        assert_eq!(
            field(seeded().with_input_text("08/20/2045")).error(),
            Some(strings.error_invalid)
        );
        assert_eq!(field(seeded().with_input_text("08/20/2026")).error(), None);
    }

    #[test]
    fn validation_off_shows_nothing_at_all() {
        let f = field(seeded().with_input_text("nonsense")).validate(false);
        assert_eq!(f.error(), None);
        // The parse itself still answers — only the surfaced error is gated.
        assert_eq!(f.parse("nonsense"), Err(DateInputError::Format));
    }

    #[test]
    fn an_override_supplies_its_own_error_strings() {
        let f = field(seeded().with_input_text("08/2")).strings(DatePickerStrings {
            error_format: "Format invalide.",
            ..DatePickerStrings::ENGLISH
        });
        assert_eq!(f.error(), Some("Format invalide."));
    }

    // ---- reported state ---------------------------------------------------

    #[test]
    fn a_valid_entry_reports_the_date_and_keeps_the_typed_text() {
        let f = field(seeded());
        let next = f.next_state("06/15/2026");
        assert_eq!(next.selected, Some(d(2026, 6, 15)));
        assert_eq!(next.displayed_month, d(2026, 6, 1));
        assert_eq!(
            next.input_text, "06/15/2026",
            "the user's own keystrokes survive the reformat"
        );
    }

    #[test]
    fn an_unpadded_entry_keeps_its_own_text_rather_than_reformatting() {
        let next = field(seeded()).next_state("6/5/2026");
        assert_eq!(next.selected, Some(d(2026, 6, 5)));
        assert_eq!(next.input_text, "6/5/2026");
    }

    #[test]
    fn a_half_typed_entry_records_the_text_and_nothing_else() {
        let start = seeded().with_selected(d(2026, 8, 20));
        let next = field(start.clone()).next_state("08/2");
        assert_eq!(next.input_text, "08/2");
        assert_eq!(
            next.selected, start.selected,
            "an unparseable draft never clears the selection"
        );
        assert_eq!(next.displayed_month, start.displayed_month);
    }

    #[test]
    fn an_out_of_range_entry_records_the_text_and_nothing_else() {
        let next = field(seeded()).next_state("08/20/2045");
        assert_eq!(next.input_text, "08/20/2045");
        assert_eq!(next.selected, None);
    }

    #[test]
    fn a_refused_entry_records_the_text_and_nothing_else() {
        let f = field(seeded()).selectable(|date| date.day() % 2 == 0);
        let next = f.next_state("06/15/2026");
        assert_eq!(next.input_text, "06/15/2026");
        assert_eq!(next.selected, None);
    }

    // ---- composition ------------------------------------------------------

    #[test]
    fn the_parse_inputs_a_composed_closure_owns_report_the_same_state() {
        // `compose()`'s `on_change` closure captures a `Parsed` rather than the
        // view (which does not outlive the pass). It must answer identically to
        // the view's own `next_state`, which every case above pins.
        let cases = ["06/15/2026", "6/5/2026", "08/2", "08/20/2045", ""];
        for text in cases {
            let view = field(seeded().with_selected(d(2026, 8, 20)));
            let parsed = Parsed {
                state: view.state.clone(),
                first: view.first_date,
                last: view.last_date,
                selectable: view.selectable.clone(),
            };
            assert_eq!(parsed.next_state(text), view.next_state(text), "{text:?}");
        }
    }

    #[test]
    fn a_predicate_reaches_the_composed_closure_too() {
        let view = field(seeded()).selectable(|date| date.day() % 2 == 0);
        let parsed = Parsed {
            state: view.state.clone(),
            first: view.first_date,
            last: view.last_date,
            selectable: view.selectable.clone(),
        };
        assert_eq!(parsed.next_state("06/15/2026").selected, None);
        assert_eq!(
            parsed.next_state("06/16/2026").selected,
            Some(d(2026, 6, 16))
        );
    }

    #[test]
    fn the_view_builds_its_wrapped_field() {
        // Exercises `compose()` end to end: a real `View::build` over the
        // wrapped baseline-backed field.
        let mut counter = 0u64;
        let view = field(seeded().with_input_text("08/2"));
        let mut element = View::<App>::build(&view, &mut BuildCtx::new(&mut counter));
        // ...and a rebuild against a changed draft reconciles rather than
        // rebuilding from scratch.
        let next = field(seeded().with_input_text("08/20/2026"));
        let flags =
            View::<App>::rebuild(&next, &view, &mut element, &mut BuildCtx::new(&mut counter));
        assert!(!flags.is_empty(), "a value change dirties the field");
    }
}
