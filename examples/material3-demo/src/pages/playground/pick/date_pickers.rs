//! Date pickers: the reference's `DatePickersPlayground`.
//!
//! Two previews, both fed by the crate's own controlled
//! [`DatePickerState`]: an inline [`calendar_date_picker`] bound to
//! `state.picker`, and a "Pick date" trigger opening
//! [`date_picker_dialog`] through [`show_date_picker`]. Two knob-menu
//! controls — "Entry mode" (dialog only) and "Calendar mode" — each get their
//! own [`OverlayAnchor`] via the two-piece
//! [`play_enum_menu_field`]/[`play_enum_menu_panel`] control, the
//! `do_::buttons` precedent for more than one anchored dropdown on one page.
//!
//! # Two draft fields, per the module's own worked example
//!
//! [`DatePickerState`] is a *live* draft — [`Knobs::picker`] feeds both the
//! inline calendar (which writes back to it on every tap/page/toggle) and,
//! as a fresh snapshot each time it opens, the dialog (whose own edits also
//! write back to it while it is open). A confirmed **OK** writes the picked
//! date into a *separate* [`Knobs::confirmed`] field instead — the exact
//! split `frust_material::date_picker`'s own module docs' worked example
//! uses (`app.picker` vs. `app.due`), so the "Dialogs" preview's own "Date:
//! …" line only moves on a real confirm, never on an in-progress edit.
//!
//! # Why `page`/`Component::build` aren't tested directly
//!
//! `Component::build` mounts a [`frust::navigator`] (the pattern
//! [`crate::pages::playground`]'s module docs point to for an overlay-owning
//! page), which auto-wires back handling against the process's running
//! reactive runtime — the same reason `main.rs` never builds its own
//! navigator-mounted shell view in a host-side test. [`content`] below is the
//! part that actually varies with this page's knob state, built with no
//! navigator touched, and is what this file's tests exercise instead.
//!
//! # Why `Knobs`' mutable fields are [`RwSignal`]s, not plain data
//!
//! A [`frust::navigator`] captures its **root page builder once**, at build,
//! and re-runs *that* closure against live state on every later rebuild
//! (`frust_widgets::nav`'s reconcile loop) — it is never replaced by the
//! closure a later `Component::build` hands it. [`DatePickersPlayground::build`]
//! used to clone `picker`/`confirmed`/`entry_mode_open`/`calendar_mode_open`
//! into that closure by value; each would have frozen at its first-frame
//! reading — this page shipped exactly that bug until this fix. They are
//! signal handles instead, cloned into the closure and re-read with `.get()`
//! on every invocation right before the call into [`content`]: the sanctioned
//! "something outside the component's own `build` observes this write" case
//! in `docs/CODE_STANDARDS.md`'s State & Reactivity conventions. The reads
//! happen inside the rebuild pass, so the shell's own `TrackedScope`
//! subscribes to them exactly as it would to a read in `build`.
//! `entry_mode_anchor`/`calendar_mode_anchor` need no such wrapping —
//! [`OverlayAnchor`] is already a `Clone` handle over shared interior-mutable
//! state.
//!
//! # The *pushed* dialog needs the same live re-read — a second bug, same shape
//!
//! [`DatePickersPlayground::build`]'s root fix above only wired the outer
//! [`frust::navigator`] correctly; [`dialogs_preview`]'s "Pick date" trigger
//! opens [`date_picker_dialog`] through [`show_date_picker`], and *that*
//! `build` argument is itself a `Fn() -> View` a navigator retains and
//! re-invokes every later rebuild — the same contract, one level deeper (see
//! [`crate::pages::playground`]'s module docs). This page used to move a
//! plain, already-snapshotted `DatePickerState` into the pushed closure at
//! "Pick date" press time; every subsequent rebuild (including the one the
//! dialog's own calendar tap causes, via `on_change`) re-painted that frozen
//! snapshot, so a tap inside the open dialog appeared to do nothing. The
//! fix is [`date_picker_dialog_view`]: it takes the `RwSignal<DatePickerState>`
//! handle and calls `.get()` inside its own body, so each re-invocation reads
//! whatever `on_change` most recently wrote — the exact pattern
//! [`super::super::view::dialogs`]'s `open_selection` establishes for its own
//! pushed `.selected(selected.get())` read.
//!
//! # Descoped: the range dialog
//!
//! The reference's second trigger, "Pick range", opens
//! `M3EDatePicker.showRange`. `frust_material::date_picker`'s own module docs
//! descope the range **dialog** arc-wide ("No range dialog" — a second,
//! larger scrolling-month surface with no viewport to lean on this deep in a
//! modal's content tree); only the range **model** (`DateRange`) and the
//! inline calendar's own range visuals ship. There is no `show_date_range_picker`
//! route to call, so this page shows the supported single-date dialog only
//! and drops the range trigger/state entirely rather than faking one.
//!
//! # `today()`: a fixed stand-in, not a live clock
//!
//! `CalendarDatePicker::today`/`DatePickerDialog::today` are required for a
//! today ring to appear at all — this crate reads no wall clock (see their
//! own doc comments). This page hands both a fixed [`today`] constant rather
//! than nothing, so the ring's geometry has something to paint against; an
//! app with a real clock source substitutes its own reading.

use frust::{
    AnyView, Column, Component, CrossAxisAlignment, Get, GetUntracked, NavigatorController,
    PopResult, RwSignal, Set, SizedBox, Stack, View, any, component, navigator, text,
};

use frust_material::{
    DatePickerDialog, DatePickerEntryMode, DatePickerMode, DatePickerState, MaterialDate,
    OverlayAnchor, calendar_date_picker, date_picker_dialog, show_date_picker, tonal_button,
};

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{
    PlaySnippet, ambient_theme, control_panel, play_enum_menu_field, play_enum_menu_panel,
    play_preview_card, play_snippet, playground_body,
};

/// The selectable lower bound both the calendar and the dialog share — the
/// reference's own `_first`.
fn first_date() -> MaterialDate {
    MaterialDate::new(2020, 1, 1)
}

/// The selectable upper bound — the reference's own `_last`.
fn last_date() -> MaterialDate {
    MaterialDate::new(2030, 12, 31)
}

/// The initial selection — the reference's own `_date` seed.
fn seed_date() -> MaterialDate {
    MaterialDate::new(2026, 8, 11)
}

/// The fixed "today" this page hands every picker — see the [module
/// docs](self)' `today()` section.
fn today() -> MaterialDate {
    MaterialDate::new(2026, 8, 20)
}

/// `"yyyy-MM-dd"` — the reference's own
/// `date.toIso8601String().split('T').first`.
fn format_date(date: MaterialDate) -> String {
    format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day())
}

/// Every [`DatePickerEntryMode`] the "Entry mode" menu offers, the
/// reference's own `M3EDatePickerEntryMode.values` order.
const ENTRY_MODES: [DatePickerEntryMode; 4] = [
    DatePickerEntryMode::Calendar,
    DatePickerEntryMode::Input,
    DatePickerEntryMode::CalendarOnly,
    DatePickerEntryMode::InputOnly,
];

/// Entry-mode label for the menu/snippet — the reference's
/// `M3EDatePickerEntryMode.name`.
fn entry_mode_label(mode: DatePickerEntryMode) -> &'static str {
    match mode {
        DatePickerEntryMode::Calendar => "calendar",
        DatePickerEntryMode::Input => "input",
        DatePickerEntryMode::CalendarOnly => "calendarOnly",
        DatePickerEntryMode::InputOnly => "inputOnly",
    }
}

/// Every [`DatePickerMode`] the "Calendar mode" menu offers.
const CALENDAR_MODES: [DatePickerMode; 2] = [DatePickerMode::Day, DatePickerMode::Year];

/// Calendar-mode label — the reference's `M3EDatePickerMode.name`.
fn calendar_mode_label(mode: DatePickerMode) -> &'static str {
    match mode {
        DatePickerMode::Day => "day",
        DatePickerMode::Year => "year",
    }
}

/// This page's own knob state — held by [`DatePickersPlayground`], never
/// [`AppState`] (the page contract in [`crate::pages::playground`]). See the
/// [module docs](self)' two-draft-fields section for why [`Self::picker`] and
/// [`Self::confirmed`] are separate.
struct Knobs {
    nav: NavigatorController<Knobs>,
    picker: RwSignal<DatePickerState>,
    confirmed: RwSignal<Option<MaterialDate>>,
    /// Shared with the "Entry mode" [`play_enum_menu_panel`].
    entry_mode_anchor: OverlayAnchor,
    entry_mode_open: RwSignal<bool>,
    /// Shared with the "Calendar mode" [`play_enum_menu_panel`].
    calendar_mode_anchor: OverlayAnchor,
    calendar_mode_open: RwSignal<bool>,
}

impl Default for Knobs {
    /// The reference's own `_DatePickersPlaygroundState` field initializers.
    fn default() -> Self {
        let seed = seed_date();
        Self {
            nav: NavigatorController::new(),
            picker: RwSignal::new(DatePickerState::new(Some(seed), today())),
            confirmed: RwSignal::new(Some(seed)),
            entry_mode_anchor: OverlayAnchor::new(),
            entry_mode_open: RwSignal::new(false),
            calendar_mode_anchor: OverlayAnchor::new(),
            calendar_mode_open: RwSignal::new(false),
        }
    }
}

/// The "Calendar" preview: the inline calendar bound to `picker`.
fn calendar_preview(picker: &DatePickerState) -> AnyView<Knobs> {
    any(calendar_date_picker(
        picker.clone(),
        first_date(),
        last_date(),
        |s: &mut Knobs, next: DatePickerState| s.picker.set(next),
    )
    .today(today()))
}

/// The "Pick date" trigger's pushed dialog — what [`show_date_picker`]'s
/// `build` argument delegates to on every re-invocation the navigator makes
/// (see the [module docs](self)' pushed-dialog section). Extracted so a test
/// can call it twice around a live `picker` write and confirm the second
/// product's [`DatePickerDialog::confirmable`] reflects the write, the bug
/// class this page shipped until this fix: a frozen [`DatePickerState`]
/// snapshot captured once, outside this fn, at "Pick date" press time.
fn date_picker_dialog_view(picker: RwSignal<DatePickerState>) -> DatePickerDialog<Knobs> {
    date_picker_dialog(
        picker.get(),
        first_date(),
        last_date(),
        |s: &mut Knobs, next: DatePickerState| s.picker.set(next),
    )
    .today(today())
}

/// The "Dialogs" preview: a "Pick date" trigger over the currently confirmed
/// date's label — the reference's `Wrap` of buttons plus its `Text(...)`
/// pair, minus the descoped range trigger (see the [module docs](self)).
fn dialogs_preview(
    nav: NavigatorController<Knobs>,
    picker: RwSignal<DatePickerState>,
    confirmed: Option<MaterialDate>,
) -> AnyView<Knobs> {
    let theme = ambient_theme();
    let mut body_style = theme.type_scale.body_medium.clone();
    body_style.color = theme.scheme().on_surface_variant;
    let date_label = confirmed
        .map(format_date)
        .unwrap_or_else(|| "none".to_string());

    let trigger = tonal_button("Pick date", move |_: &mut Knobs| {
        show_date_picker(
            &nav,
            move || date_picker_dialog_view(picker),
            |s: &mut Knobs, result: PopResult| {
                if let Some(date) = result.take::<MaterialDate>() {
                    s.confirmed.set(Some(date));
                }
            },
        );
    });

    any(Column(vec![
        any(trigger),
        any(SizedBox::<Knobs>(None, Some(12.0))),
        any(text(format!("Date: {date_label}")).style(body_style)),
    ])
    .cross_axis(CrossAxisAlignment::Start))
}

/// The paste-ready "Calendar" snippet for the current picker state.
fn calendar_snippet(picker: &DatePickerState) -> PlaySnippet {
    let code = format!(
        "calendar_date_picker(\n    \
             state.picker.clone().with_mode(DatePickerMode::{mode:?}),\n    \
             MaterialDate::new(2020, 1, 1),\n    \
             MaterialDate::new(2030, 12, 31),\n    \
             |state, next| state.picker = next,\n\
         )\n\
         .today(MaterialDate::new(2026, 8, 20));",
        mode = picker.mode,
    );
    play_snippet("Calendar", code)
}

/// The paste-ready "Dialogs" snippet for the current picker state.
fn dialog_snippet(picker: &DatePickerState) -> PlaySnippet {
    let code = format!(
        "show_date_picker(\n    \
             &nav,\n    \
             || date_picker_dialog(\n        \
                 state.picker.clone().with_entry_mode(DatePickerEntryMode::{entry:?}),\n        \
                 MaterialDate::new(2020, 1, 1),\n        \
                 MaterialDate::new(2030, 12, 31),\n        \
                 |state, next| state.picker = next,\n    \
             )\n    \
             .today(MaterialDate::new(2026, 8, 20)),\n    \
             |state, result| {{\n        \
                 if let Some(date) = result.take::<MaterialDate>() {{\n            \
                     state.confirmed = Some(date);\n        \
                 }}\n    \
             }},\n\
         );",
        entry = picker.entry_mode,
    );
    play_snippet("Dialogs", code)
}

/// "Picker" controls: entry mode (dialog only), calendar mode.
fn controls(
    picker: &DatePickerState,
    entry_mode_anchor: &OverlayAnchor,
    entry_mode_open: bool,
    calendar_mode_anchor: &OverlayAnchor,
    calendar_mode_open: bool,
) -> AnyView<Knobs> {
    control_panel(
        "Picker",
        vec![
            play_enum_menu_field(
                "Entry mode",
                picker.entry_mode,
                &ENTRY_MODES,
                entry_mode_label,
                entry_mode_anchor,
                entry_mode_open,
                |s: &mut Knobs, open: bool| s.entry_mode_open.set(open),
            ),
            play_enum_menu_field(
                "Calendar mode",
                picker.mode,
                &CALENDAR_MODES,
                calendar_mode_label,
                calendar_mode_anchor,
                calendar_mode_open,
                |s: &mut Knobs, open: bool| s.calendar_mode_open.set(open),
            ),
        ],
    )
}

/// The "Entry mode" menu's popup half — mounted at this page's outer
/// [`Stack`].
fn entry_mode_menu_panel(
    value: DatePickerEntryMode,
    anchor: &OverlayAnchor,
    open: bool,
) -> AnyView<Knobs> {
    play_enum_menu_panel(
        value,
        &ENTRY_MODES,
        entry_mode_label,
        anchor,
        open,
        |s: &mut Knobs, open: bool| s.entry_mode_open.set(open),
        |s: &mut Knobs, next: DatePickerEntryMode| {
            s.picker.set(s.picker.get_untracked().with_entry_mode(next))
        },
    )
}

/// The "Calendar mode" menu's popup half — mounted at this page's outer
/// [`Stack`].
fn calendar_mode_menu_panel(
    value: DatePickerMode,
    anchor: &OverlayAnchor,
    open: bool,
) -> AnyView<Knobs> {
    play_enum_menu_panel(
        value,
        &CALENDAR_MODES,
        calendar_mode_label,
        anchor,
        open,
        |s: &mut Knobs, open: bool| s.calendar_mode_open.set(open),
        |s: &mut Knobs, next: DatePickerMode| {
            s.picker.set(s.picker.get_untracked().with_mode(next))
        },
    )
}

/// The playground content: both previews, both snippets, and the controls
/// panel plus its two dropdown panels — everything that varies with this
/// page's knob state, built with no navigator touched (see the [module
/// docs](self)). `picker` is the live signal handle — read once here with
/// `.get()` for every plain-value use, and threaded through unread to
/// [`dialogs_preview`]'s pushed dialog builder, which needs the *handle*
/// rather than a snapshot (see the [module docs](self)' pushed-dialog
/// section).
fn content(
    nav: &NavigatorController<Knobs>,
    picker: RwSignal<DatePickerState>,
    confirmed: Option<MaterialDate>,
    entry_mode_anchor: &OverlayAnchor,
    entry_mode_open: bool,
    calendar_mode_anchor: &OverlayAnchor,
    calendar_mode_open: bool,
) -> AnyView<Knobs> {
    let picker_value = picker.get();
    let playground = playground_body(
        vec![
            play_preview_card("Calendar", calendar_preview(&picker_value)),
            play_preview_card("Dialogs", dialogs_preview(nav.clone(), picker, confirmed)),
        ],
        vec![
            calendar_snippet(&picker_value),
            dialog_snippet(&picker_value),
        ],
        vec![controls(
            &picker_value,
            entry_mode_anchor,
            entry_mode_open,
            calendar_mode_anchor,
            calendar_mode_open,
        )],
    );
    any(Stack(vec![
        playground,
        entry_mode_menu_panel(picker_value.entry_mode, entry_mode_anchor, entry_mode_open),
        calendar_mode_menu_panel(picker_value.mode, calendar_mode_anchor, calendar_mode_open),
    ]))
}

struct DatePickersPlayground;

impl Component for DatePickersPlayground {
    type State = Knobs;

    fn init(&self) -> Knobs {
        Knobs::default()
    }

    fn build(&self, state: &mut Knobs) -> impl View<Knobs> {
        let nav = state.nav.clone();
        let picker = state.picker;
        let confirmed = state.confirmed;
        let entry_mode_anchor = state.entry_mode_anchor.clone();
        let entry_mode_open = state.entry_mode_open;
        let calendar_mode_anchor = state.calendar_mode_anchor.clone();
        let calendar_mode_open = state.calendar_mode_open;
        any(navigator(&state.nav, move || {
            content(
                &nav,
                picker,
                confirmed.get(),
                &entry_mode_anchor,
                entry_mode_open.get(),
                &calendar_mode_anchor,
                calendar_mode_open.get(),
            )
        }))
    }
}

/// See the page contract in [`crate::pages::playground`].
pub fn page(_entry: DemoEntry) -> AnyView<AppState> {
    any(component(DatePickersPlayground))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_builds_across_every_reachable_control_state() {
        let nav: NavigatorController<Knobs> = NavigatorController::new();
        let anchor = OverlayAnchor::new();
        let base = DatePickerState::new(Some(seed_date()), today());

        for entry_mode in ENTRY_MODES {
            let picker = base.clone().with_entry_mode(entry_mode);
            let _view = content(
                &nav,
                RwSignal::new(picker.clone()),
                Some(seed_date()),
                &anchor,
                false,
                &anchor,
                false,
            );
            let _calendar = calendar_snippet(&picker);
            let _dialog = dialog_snippet(&picker);
        }
        for calendar_mode in CALENDAR_MODES {
            let picker = base.clone().with_mode(calendar_mode);
            let _view = content(
                &nav,
                RwSignal::new(picker.clone()),
                None,
                &anchor,
                false,
                &anchor,
                false,
            );
        }

        // Both menu panels open at once.
        let _view = content(
            &nav,
            RwSignal::new(base.clone()),
            Some(seed_date()),
            &anchor,
            true,
            &anchor,
            true,
        );

        // A month-paged, year-sub-view picker still builds.
        let paged = base.clone().stepped_month(3, first_date(), last_date());
        let _view = content(
            &nav,
            RwSignal::new(paged.clone()),
            Some(seed_date()),
            &anchor,
            false,
            &anchor,
            false,
        );
        let year_view = base.clone().with_mode(DatePickerMode::Year);
        let _view = content(
            &nav,
            RwSignal::new(year_view.clone()),
            Some(seed_date()),
            &anchor,
            false,
            &anchor,
            false,
        );

        // No confirmation yet.
        let _view = content(
            &nav,
            RwSignal::new(base.clone()),
            None,
            &anchor,
            false,
            &anchor,
            false,
        );
    }

    #[test]
    fn the_page_fn_builds_from_its_catalog_entry() {
        let entry = crate::catalog::find_by_id("date_pickers").expect("catalog entry exists");
        let _view = super::page(entry);
    }

    /// A write through any knob's signal must be visible to the next read
    /// the navigator's frozen closure would perform — the exact round trip
    /// `DatePickersPlayground::build`'s `move || content(&nav, &picker.get(),
    /// ..)` relies on.
    #[test]
    fn a_knob_write_through_the_signal_is_visible_to_the_next_read() {
        let knobs = Knobs::default();
        let confirmed_date = MaterialDate::new(2027, 3, 4);
        let edited_picker = knobs.picker.get_untracked().with_mode(DatePickerMode::Year);

        knobs.picker.set(edited_picker);
        knobs.confirmed.set(Some(confirmed_date));
        knobs.entry_mode_open.set(true);
        knobs.calendar_mode_open.set(true);

        assert_eq!(knobs.picker.get().mode, DatePickerMode::Year);
        assert_eq!(knobs.confirmed.get(), Some(confirmed_date));
        assert!(knobs.entry_mode_open.get());
        assert!(knobs.calendar_mode_open.get());
    }

    /// The bug p5-16 shipped and this fix closes: a pushed dialog builder
    /// that closes over a plain, frozen snapshot instead of the live signal
    /// (see the [module docs](super)' pushed-dialog section). This calls
    /// [`date_picker_dialog_view`] — the exact fn [`dialogs_preview`]'s
    /// "Pick date" trigger delegates to — twice around a live `picker`
    /// write, and asserts the *second* product's own `confirmable()` (a
    /// public read of the dialog's resolved selection) reflects the write
    /// rather than the first call's snapshot. A frozen-snapshot regression
    /// would make both assertions see the seed date.
    #[test]
    fn the_pushed_dialog_builder_reads_the_live_signal_not_a_frozen_snapshot() {
        let seed = seed_date();
        let picker = RwSignal::new(DatePickerState::new(Some(seed), today()));

        let first = date_picker_dialog_view(picker);
        assert_eq!(first.confirmable(), Some(seed));

        let picked = MaterialDate::new(2028, 2, 14);
        picker.set(DatePickerState::new(Some(picked), today()));

        let second = date_picker_dialog_view(picker);
        assert_eq!(second.confirmable(), Some(picked));
        assert_ne!(second.confirmable(), first.confirmable());
    }
}
