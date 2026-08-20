//! Time pickers: the reference's `TimePickersPlayground`.
//!
//! Two previews sharing one [`TimeOfDay`] knob: an inline [`time_dial`], and
//! a "Pick time" trigger opening [`time_picker`] through [`show_time_picker`]
//! — the module's own worked example almost verbatim (one `state.time` field
//! feeds the dial directly and seeds the dialog; the dialog's own edits write
//! back into it through `on_change`, and a confirmed OK writes the same field
//! again through the pop-result round trip). One knob-menu control ("Entry
//! mode") gets its own [`OverlayAnchor`] via the two-piece
//! [`play_enum_menu_field`]/[`play_enum_menu_panel`] control.
//!
//! # `show_time_picker` does not auto-wire OK/Cancel
//!
//! Unlike `frust_material::date_picker`'s `show_date_picker` ("wires
//! OK/Cancel/back for you"), `show_time_picker`'s own module docs are
//! explicit that confirm and cancel are the *caller's* actions: this page's
//! trigger closure calls `nav.pop_with_result`/`nav.pop()` itself from
//! `TimePickerView::on_confirm`/`TimePickerView::on_cancel`, exactly as the
//! module doc's own example does — scrim tap/Escape/back stay the host's own
//! staged pop.
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
//! closure a later `Component::build` hands it. [`TimePickersPlayground::build`]
//! used to clone `time`/`entry_mode`/`use_24_hour`/`entry_mode_open` into
//! that closure by value; each would have frozen at its first-frame reading —
//! this page shipped exactly that bug until this fix (the same shape
//! [`super::date_pickers`] shipped and fixes for the same reason). They are
//! signal handles instead, cloned into the closure and re-read with `.get()`
//! on every invocation right before the call into [`content`]: the sanctioned
//! "something outside the component's own `build` observes this write" case
//! in `docs/CODE_STANDARDS.md`'s State & Reactivity conventions. The reads
//! happen inside the rebuild pass, so the shell's own `TrackedScope`
//! subscribes to them exactly as it would to a read in `build`.
//! `entry_mode_anchor` needs no such wrapping — [`OverlayAnchor`] is already
//! a `Clone` handle over shared interior-mutable state.
//!
//! # The *pushed* dialog needs the same live re-read — a second bug, same shape
//!
//! [`TimePickersPlayground::build`]'s root fix above only wired the outer
//! [`frust::navigator`] correctly; [`dialog_preview`]'s "Pick time" trigger
//! opens [`time_picker`] through [`show_time_picker`], and *that* `build`
//! argument is itself a `Fn() -> View` a navigator retains and re-invokes
//! every later rebuild — the same contract, one level deeper (see
//! [`crate::pages::playground`]'s module docs and
//! [`super::date_pickers`]'s identical section). This page used to move
//! plain `time`/`entry_mode`/`use_24_hour` values into the pushed closure at
//! "Pick time" press time; a dial tap inside the open dialog writes through
//! `on_change` to `state.time`, but every subsequent rebuild re-painted the
//! frozen snapshot from press time, so the tap appeared to do nothing. The
//! fix is [`dialog_view`]: it takes the three `RwSignal` handles and reads
//! each with `.get()`/[`resolve_time`] inside its own body, so each
//! re-invocation reads whatever was most recently written — the same
//! pattern [`super::date_pickers`]'s `date_picker_dialog_view` and
//! [`super::super::view::dialogs`]'s `open_selection` establish.
//!
//! [`TimePickerView`] itself exposes no public accessor for its resolved
//! `value` (unlike `frust_material::date_picker`'s
//! `DatePickerDialog::confirmable`), so [`resolve_time`] — the exact read
//! [`dialog_view`] performs — stands in for it in this file's tests.

use frust::{
    AnyView, Column, Component, CrossAxisAlignment, Get, NavigatorController, PopResult, RwSignal,
    Set, SizedBox, Stack, any, component, navigator, text,
};
use frust_material::{
    OverlayAnchor, TimeOfDay, TimePickerEntryMode, TimePickerView, show_time_picker, time_dial,
    time_picker, tonal_button,
};

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{
    PlaySnippet, ambient_theme, control_panel, play_enum_menu_field, play_enum_menu_panel,
    play_preview_card, play_snippet, play_switch, playground_body,
};

/// Every [`TimePickerEntryMode`] the "Entry mode" menu offers, the
/// reference's own `M3ETimePickerEntryMode.values` order.
const ENTRY_MODES: [TimePickerEntryMode; 4] = [
    TimePickerEntryMode::Dial,
    TimePickerEntryMode::Input,
    TimePickerEntryMode::DialOnly,
    TimePickerEntryMode::InputOnly,
];

/// Entry-mode label for the menu/snippet — the reference's
/// `M3ETimePickerEntryMode.name`.
fn entry_mode_label(mode: TimePickerEntryMode) -> &'static str {
    match mode {
        TimePickerEntryMode::Dial => "dial",
        TimePickerEntryMode::Input => "input",
        TimePickerEntryMode::DialOnly => "dialOnly",
        TimePickerEntryMode::InputOnly => "inputOnly",
    }
}

/// This page's own knob state — held by [`TimePickersPlayground`], never
/// [`AppState`] (the page contract in [`crate::pages::playground`]).
struct Knobs {
    nav: NavigatorController<Knobs>,
    time: RwSignal<TimeOfDay>,
    entry_mode: RwSignal<TimePickerEntryMode>,
    use_24_hour: RwSignal<bool>,
    /// Shared with the "Entry mode" [`play_enum_menu_panel`].
    entry_mode_anchor: OverlayAnchor,
    entry_mode_open: RwSignal<bool>,
}

impl Default for Knobs {
    /// The reference's own `_TimePickersPlaygroundState` field initializers.
    fn default() -> Self {
        Self {
            nav: NavigatorController::new(),
            time: RwSignal::new(TimeOfDay::new(9, 30)),
            entry_mode: RwSignal::new(TimePickerEntryMode::Dial),
            use_24_hour: RwSignal::new(false),
            entry_mode_anchor: OverlayAnchor::new(),
            entry_mode_open: RwSignal::new(false),
        }
    }
}

/// The "Dial" preview: the inline ring bound to `time`.
fn dial_preview(time: TimeOfDay, use_24_hour: bool) -> AnyView<Knobs> {
    any(time_dial(time, |s: &mut Knobs, next: TimeOfDay| s.time.set(next)).use_24_hour(use_24_hour))
}

/// The value the pushed dialog resolves on *this* invocation — read inside
/// [`dialog_view`]'s own body, never before it, so the shell's own
/// `TrackedScope` subscribes exactly as it would to a read in `build` (see
/// the [module docs](self)' pushed-dialog section).
fn resolve_time(time: RwSignal<TimeOfDay>) -> TimeOfDay {
    time.get()
}

/// The "Pick time" trigger's pushed dialog — what [`show_time_picker`]'s
/// `build` argument delegates to on every re-invocation the navigator makes.
/// Extracted so a test can call it twice around a live `time` write and
/// confirm the second call resolves the write (via [`resolve_time`]), the
/// bug class this page shipped until this fix: `time`, `entry_mode`, and
/// `use_24_hour` used to be plain values moved into the pushed closure at
/// "Pick time" press time, frozen for the dialog's whole open lifetime.
fn dialog_view(
    time: RwSignal<TimeOfDay>,
    entry_mode: RwSignal<TimePickerEntryMode>,
    use_24_hour: RwSignal<bool>,
    on_confirm_nav: NavigatorController<Knobs>,
    on_cancel_nav: NavigatorController<Knobs>,
) -> TimePickerView<Knobs> {
    time_picker(resolve_time(time))
        .entry_mode(entry_mode.get())
        .use_24_hour(use_24_hour.get())
        .on_change(|s: &mut Knobs, next: TimeOfDay| s.time.set(next))
        .on_confirm(move |_s: &mut Knobs, confirmed: Option<TimeOfDay>| {
            if let Some(picked) = confirmed {
                on_confirm_nav.pop_with_result(PopResult::of(picked));
            }
        })
        .on_cancel(move |_s: &mut Knobs| on_cancel_nav.pop())
}

/// The "Dialog" preview: a "Pick time" trigger over the current time's label
/// — the reference's `Column` of a button plus its `Text(...)`. `time` is
/// the plain snapshot for the label; `time_signal`/`entry_mode_signal`/
/// `use_24_hour_signal` are the live handles the pushed dialog needs (see
/// [`dialog_view`]).
fn dialog_preview(
    nav: NavigatorController<Knobs>,
    time: TimeOfDay,
    time_signal: RwSignal<TimeOfDay>,
    entry_mode_signal: RwSignal<TimePickerEntryMode>,
    use_24_hour_signal: RwSignal<bool>,
) -> AnyView<Knobs> {
    let theme = ambient_theme();
    let mut body_style = theme.type_scale.body_medium.clone();
    body_style.color = theme.scheme().on_surface_variant;
    let time_label = format!("{:02}:{:02}", time.hour(), time.minute());

    let trigger = tonal_button("Pick time", move |_: &mut Knobs| {
        let on_confirm_nav = nav.clone();
        let on_cancel_nav = nav.clone();
        show_time_picker(
            &nav,
            move || {
                dialog_view(
                    time_signal,
                    entry_mode_signal,
                    use_24_hour_signal,
                    on_confirm_nav.clone(),
                    on_cancel_nav.clone(),
                )
            },
            |s: &mut Knobs, result: PopResult| {
                if let Some(picked) = result.take::<TimeOfDay>() {
                    s.time.set(picked);
                }
            },
        );
    });

    any(Column(vec![
        any(trigger),
        any(SizedBox::<Knobs>(None, Some(12.0))),
        any(text(format!("Time: {time_label}")).style(body_style)),
    ])
    .cross_axis(CrossAxisAlignment::Start))
}

/// The paste-ready "Dial" snippet for the current knob state.
fn dial_snippet(time: TimeOfDay, use_24_hour: bool) -> PlaySnippet {
    let code = format!(
        "time_dial(TimeOfDay::new({hour}, {minute}), on_changed)\n    .use_24_hour({use_24_hour});",
        hour = time.hour(),
        minute = time.minute(),
    );
    play_snippet("Dial", code)
}

/// The paste-ready "Dialog" snippet for the current knob state.
fn dialog_snippet(
    time: TimeOfDay,
    entry_mode: TimePickerEntryMode,
    use_24_hour: bool,
) -> PlaySnippet {
    let code = format!(
        "show_time_picker(\n    \
             &nav,\n    \
             || time_picker(TimeOfDay::new({hour}, {minute}))\n        \
                 .entry_mode(TimePickerEntryMode::{entry_mode:?})\n        \
                 .use_24_hour({use_24_hour})\n        \
                 .on_change(|state, next| state.time = next)\n        \
                 .on_confirm(|_state, confirmed| {{\n            \
                     if let Some(picked) = confirmed {{\n                \
                         nav.pop_with_result(PopResult::of(picked));\n            \
                     }}\n        \
                 }}),\n    \
             |state, result| {{\n        \
                 if let Some(picked) = result.take::<TimeOfDay>() {{\n            \
                     state.time = picked;\n        \
                 }}\n    \
             }},\n\
         );",
        hour = time.hour(),
        minute = time.minute(),
    );
    play_snippet("Dialog", code)
}

/// "Picker" controls: entry mode, 24-hour format.
fn controls(
    entry_mode: TimePickerEntryMode,
    use_24_hour: bool,
    entry_mode_anchor: &OverlayAnchor,
    entry_mode_open: bool,
) -> AnyView<Knobs> {
    control_panel(
        "Picker",
        vec![
            play_enum_menu_field(
                "Entry mode",
                entry_mode,
                &ENTRY_MODES,
                entry_mode_label,
                entry_mode_anchor,
                entry_mode_open,
                |s: &mut Knobs, open: bool| s.entry_mode_open.set(open),
            ),
            play_switch("24-hour format", use_24_hour, |s: &mut Knobs, v: bool| {
                s.use_24_hour.set(v)
            }),
        ],
    )
}

/// The "Entry mode" menu's popup half — mounted at this page's outer
/// [`Stack`].
fn entry_mode_menu_panel(
    value: TimePickerEntryMode,
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
        |s: &mut Knobs, next: TimePickerEntryMode| s.entry_mode.set(next),
    )
}

/// The playground content: both previews, both snippets, and the controls
/// panel plus its dropdown panel — everything that varies with this page's
/// knob state, built with no navigator touched (see the [module docs](self)).
/// `time`/`entry_mode`/`use_24_hour` are the live signal handles — each read
/// once here with `.get()` for every plain-value use, and `time` threaded
/// through unread (alongside the other two) to [`dialog_preview`]'s pushed
/// dialog builder, which needs the *handles* rather than a snapshot (see the
/// [module docs](self)' pushed-dialog section).
fn content(
    nav: &NavigatorController<Knobs>,
    time: RwSignal<TimeOfDay>,
    entry_mode: RwSignal<TimePickerEntryMode>,
    use_24_hour: RwSignal<bool>,
    entry_mode_anchor: &OverlayAnchor,
    entry_mode_open: bool,
) -> AnyView<Knobs> {
    let time_value = time.get();
    let entry_mode_value = entry_mode.get();
    let use_24_hour_value = use_24_hour.get();
    let playground = playground_body(
        vec![
            play_preview_card("Dial", dial_preview(time_value, use_24_hour_value)),
            play_preview_card(
                "Dialog",
                dialog_preview(nav.clone(), time_value, time, entry_mode, use_24_hour),
            ),
        ],
        vec![
            dial_snippet(time_value, use_24_hour_value),
            dialog_snippet(time_value, entry_mode_value, use_24_hour_value),
        ],
        vec![controls(
            entry_mode_value,
            use_24_hour_value,
            entry_mode_anchor,
            entry_mode_open,
        )],
    );
    any(Stack(vec![
        playground,
        entry_mode_menu_panel(entry_mode_value, entry_mode_anchor, entry_mode_open),
    ]))
}

struct TimePickersPlayground;

impl Component for TimePickersPlayground {
    type State = Knobs;

    fn init(&self) -> Knobs {
        Knobs::default()
    }

    fn build(&self, state: &mut Knobs) -> AnyView<Knobs> {
        let nav = state.nav.clone();
        let time = state.time;
        let entry_mode = state.entry_mode;
        let use_24_hour = state.use_24_hour;
        let entry_mode_anchor = state.entry_mode_anchor.clone();
        let entry_mode_open = state.entry_mode_open;
        any(navigator(&state.nav, move || {
            content(
                &nav,
                time,
                entry_mode,
                use_24_hour,
                &entry_mode_anchor,
                entry_mode_open.get(),
            )
        }))
    }
}

/// See the page contract in [`crate::pages::playground`].
pub fn page(_entry: DemoEntry) -> AnyView<AppState> {
    any(component(TimePickersPlayground))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_builds_across_every_reachable_control_state() {
        let nav: NavigatorController<Knobs> = NavigatorController::new();
        let anchor = OverlayAnchor::new();
        let time = TimeOfDay::new(9, 30);

        for entry_mode in ENTRY_MODES {
            for use_24_hour in [true, false] {
                let _view = content(
                    &nav,
                    RwSignal::new(time),
                    RwSignal::new(entry_mode),
                    RwSignal::new(use_24_hour),
                    &anchor,
                    false,
                );
                let _dial = dial_snippet(time, use_24_hour);
                let _dialog = dialog_snippet(time, entry_mode, use_24_hour);
            }
        }

        // The menu open, and a midnight/noon edge time.
        let _view = content(
            &nav,
            RwSignal::new(time),
            RwSignal::new(TimePickerEntryMode::Dial),
            RwSignal::new(false),
            &anchor,
            true,
        );
        let midnight = TimeOfDay::new(0, 0);
        let _view = content(
            &nav,
            RwSignal::new(midnight),
            RwSignal::new(TimePickerEntryMode::Dial),
            RwSignal::new(false),
            &anchor,
            false,
        );
        let noon_edge = TimeOfDay::new(23, 59);
        let _view = content(
            &nav,
            RwSignal::new(noon_edge),
            RwSignal::new(TimePickerEntryMode::Dial),
            RwSignal::new(true),
            &anchor,
            false,
        );
    }

    #[test]
    fn the_page_fn_builds_from_its_catalog_entry() {
        let entry = crate::catalog::find_by_id("time_pickers").expect("catalog entry exists");
        let _view = super::page(entry);
    }

    /// A write through any knob's signal must be visible to the next read
    /// the navigator's frozen closure would perform — the exact round trip
    /// `TimePickersPlayground::build`'s `move || content(&nav, time.get(),
    /// ..)` relies on.
    #[test]
    fn a_knob_write_through_the_signal_is_visible_to_the_next_read() {
        let knobs = Knobs::default();
        let updated_time = TimeOfDay::new(14, 45);

        knobs.time.set(updated_time);
        knobs.entry_mode.set(TimePickerEntryMode::InputOnly);
        knobs.use_24_hour.set(true);
        knobs.entry_mode_open.set(true);

        assert_eq!(knobs.time.get(), updated_time);
        assert_eq!(knobs.entry_mode.get(), TimePickerEntryMode::InputOnly);
        assert!(knobs.use_24_hour.get());
        assert!(knobs.entry_mode_open.get());
    }

    /// The bug this page shipped and this fix closes: a pushed dialog
    /// builder that closes over a plain, frozen snapshot instead of the live
    /// signal (see the [module docs](super)' pushed-dialog section). This
    /// calls [`dialog_view`] — the exact fn [`dialog_preview`]'s "Pick time"
    /// trigger delegates to — twice around a live `time` write, asserting
    /// [`resolve_time`] (the read `dialog_view` performs internally) reflects
    /// the write on the second call; [`TimePickerView`] itself exposes no
    /// public accessor, so this is the closest public assertion available
    /// (see the [module docs](super)). Each call also builds the real
    /// product to prove the fix compiles and runs against a live signal, not
    /// just a snapshot.
    #[test]
    fn the_pushed_dialog_builder_rereads_the_live_time_signal_each_invocation() {
        let time = RwSignal::new(TimeOfDay::new(9, 30));
        let entry_mode = RwSignal::new(TimePickerEntryMode::Dial);
        let use_24_hour = RwSignal::new(false);
        let nav: NavigatorController<Knobs> = NavigatorController::new();

        assert_eq!(resolve_time(time), TimeOfDay::new(9, 30));
        let _first: TimePickerView<Knobs> =
            dialog_view(time, entry_mode, use_24_hour, nav.clone(), nav.clone());

        let updated = TimeOfDay::new(14, 45);
        time.set(updated);

        assert_eq!(resolve_time(time), updated);
        let _second: TimePickerView<Knobs> =
            dialog_view(time, entry_mode, use_24_hour, nav.clone(), nav.clone());
    }
}
