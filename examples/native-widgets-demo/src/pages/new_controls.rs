//! **New** — the four later builders, each beside a frust-drawn counterpart
//! where the seeded design system (Glyph) has one:
//!
//! | Native | Drawn column |
//! |---|---|
//! | `native_spinner` (animating toggle) | Glyph `dots_loader` |
//! | `native_segmented` (three segments, controlled, refuse toggle) | Glyph `segmented_control` |
//! | `native_stepper` (min/max/step/wraps) | baseline `−`/`+` buttons over the same arithmetic — Glyph has no stepper |
//! | `native_date_picker` (compact ↔ inline, min/max) | a `Text` readout — Glyph has no date picker |
//!
//! Every interactive one is controlled: its callback reports the *requested*
//! value and the app feeds the confirmed value back
//! (`docs/CODE_STANDARDS.md`'s Interaction Semantics). The segmented
//! control's refuse toggle is the Controls page's write-back affordance
//! applied to a third control, with the same test-only accessibility-label
//! refusal count (see `controls.rs`' module doc for why a refusal has to
//! move a prop).
//!
//! `native_segmented`/`native_stepper` have no Android arm: there (and in a
//! Linux/Windows desktop preview) each renders the plugin's frust-drawn
//! refusal banner in place (`native-widgets-segmented-stepper-apple-only`).
//! Android fixes a date picker's style when it is created, so the style
//! switch applies there only after the picker remounts (leave the page and
//! come back).
//!
//! Native slots at rest: 4 on iOS/macOS, 2 elsewhere.

use frust::{AnyView, Get, GetUntracked, Set, any, button, checkbox, inflexible, text};
use frust_glyph::{dots_loader, segmented_control};
use frust_native_widgets::{
    CivilDate, NativeDatePickerStyle, native_date_picker, native_segmented, native_spinner,
    native_stepper,
};

use super::common::{
    CellFit, NativeClasses, PAIR_CELL_W, S, block, bump, caption, chip, gap, gap_h, local_sig,
    page_column, page_header, pair_row, pair_row_sized, readout, row,
};

/// This page's index in [`SECTION_LABELS`](crate::SECTION_LABELS).
const SECTION: usize = 2;

/// The segmented control's labels.
const SEGMENTS: [&str; 3] = ["Day", "Week", "Month"];

/// The stepper's range.
const STEPPER_MIN: i32 = 0;
/// See [`STEPPER_MIN`].
const STEPPER_MAX: i32 = 10;

local_sig!(spinner_animating_sig, bool, true);
local_sig!(segment_sig, usize, 0);
local_sig!(segment_refuse_sig, bool, false);
local_sig!(segment_refused_sig, u32, 0);
local_sig!(segment_events_sig, u32, 0);
local_sig!(stepper_sig, i32, 5);
local_sig!(stepper_wraps_sig, bool, false);
local_sig!(stepper_big_step_sig, bool, false);
local_sig!(stepper_events_sig, u32, 0);
local_sig!(date_sig, CivilDate, initial_date());
local_sig!(date_inline_sig, bool, false);
local_sig!(date_events_sig, u32, 0);

/// A known-valid date, or the crate's floor if a literal were ever wrong —
/// `CivilDate::new` is the only public constructor and returns `Option`.
fn date(year: i32, month: u8, day: u8) -> CivilDate {
    CivilDate::new(year, month, day).unwrap_or(CivilDate::MIN)
}

/// The picker's starting value.
fn initial_date() -> CivilDate {
    date(2026, 9, 29)
}

/// The earliest selectable date.
fn date_min() -> CivilDate {
    date(2026, 1, 1)
}

/// The latest selectable date.
fn date_max() -> CivilDate {
    date(2027, 12, 31)
}

/// `YYYY-MM-DD`.
fn format_date(d: CivilDate) -> String {
    format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day())
}

/// Confirm (or refuse) a requested segment from EITHER column — the
/// Controls page's write-back shape.
fn confirm_segment(requested: usize) {
    let value = segment_sig();
    if segment_refuse_sig().get_untracked() {
        bump(segment_refused_sig());
        value.set(value.get_untracked());
    } else if requested < SEGMENTS.len() {
        value.set(requested);
    }
}

/// The step a tap applies.
fn stepper_step(big: bool) -> i32 {
    if big { 2 } else { 1 }
}

/// The value one drawn `−`/`+` tap requests: `value + delta`, clamped to the
/// range, or wrapped past a bound when `wraps` — the native stepper's own
/// arithmetic, so both columns agree.
fn stepped(value: i32, delta: i32, wraps: bool) -> i32 {
    let next = value + delta;
    if wraps {
        if next > STEPPER_MAX {
            STEPPER_MIN
        } else if next < STEPPER_MIN {
            STEPPER_MAX
        } else {
            next
        }
    } else {
        next.clamp(STEPPER_MIN, STEPPER_MAX)
    }
}

/// Confirm a requested stepper value (accept, clamped to the range).
fn confirm_stepper(requested: i32) {
    stepper_sig().set(requested.clamp(STEPPER_MIN, STEPPER_MAX));
}

/// The spinner pair plus its animating toggle.
fn spinner_block(animating: bool) -> Vec<FlexChildS> {
    let drawn = if animating {
        any(dots_loader())
    } else {
        any(text("(stopped)").size(12.0))
    };
    vec![
        pair_row(
            "Spinner",
            NativeClasses {
                android: "ProgressBar (circular)",
                ios: "UIActivityIndicatorView",
                macos: "NSProgressIndicator (Spinning)",
            },
            "Display-only. Toggle it: both columns stop and start together.",
            CellFit::Natural,
            44.0,
            any(native_spinner(animating)
                .content_description("Native spinner")
                .size(44.0, 44.0)),
            drawn,
        ),
        block(vec![inflexible(chip(
            if animating {
                "Stop spinning"
            } else {
                "Start spinning"
            },
            |_: &mut S| {
                let sig = spinner_animating_sig();
                sig.set(!sig.get_untracked());
            },
        ))]),
    ]
}

/// The segmented pair, its refuse toggle and readout.
fn segmented_block(selected: usize, refusing: bool, refused: u32, events: u32) -> Vec<FlexChildS> {
    let labels: Vec<String> = SEGMENTS.iter().map(|s| s.to_string()).collect();
    let description = if refusing {
        format!("Native segmented control \u{2014} REFUSE, {refused} refused")
    } else {
        "Native segmented control".to_string()
    };
    vec![
        pair_row(
            "Segmented control",
            NativeClasses {
                android: "none (refusal banner)",
                ios: "UISegmentedControl",
                macos: "NSSegmentedControl",
            },
            "Controlled: a tap reports the requested segment; the app feeds it back.",
            CellFit::Stretch,
            32.0,
            any(native_segmented(labels.clone(), selected)
                .content_description(description)
                .size(PAIR_CELL_W, 32.0)
                .on_select(|index| {
                    bump(segment_events_sig());
                    confirm_segment(index);
                })),
            any(segmented_control(labels, selected, |_: &mut S, index| {
                confirm_segment(index)
            })),
        ),
        block(vec![
            inflexible(any(checkbox(
                refusing,
                "REFUSE segment changes",
                |_: &mut S, on: bool| segment_refuse_sig().set(on),
            ))),
            gap(4.0),
            inflexible(readout(format!(
                "Segment: {} \u{2014} events {events}, refused {refused}",
                SEGMENTS[selected.min(SEGMENTS.len() - 1)]
            ))),
            gap(2.0),
            inflexible(caption(
                "While REFUSE is on the native control's accessibility label carries the \
                 refusal count (test-only, as on Controls) so the refusal reaches the wire.",
            )),
        ]),
    ]
}

/// The stepper pair: native stepper vs baseline `−`/`+` buttons.
fn stepper_block(value: i32, wraps: bool, big_step: bool, events: u32) -> Vec<FlexChildS> {
    let step = stepper_step(big_step);
    let drawn = row(vec![
        inflexible(any(button("\u{2212}", move |_: &mut S| {
            confirm_stepper(stepped(stepper_sig().get_untracked(), -step, wraps))
        })
        .small())),
        gap_h(8.0),
        inflexible(readout(format!("{value}"))),
        gap_h(8.0),
        inflexible(any(button("+", move |_: &mut S| {
            confirm_stepper(stepped(stepper_sig().get_untracked(), step, wraps))
        })
        .small())),
    ]);
    vec![
        pair_row(
            "Stepper",
            NativeClasses {
                android: "none (refusal banner)",
                ios: "UIStepper",
                macos: "NSStepper",
            },
            "Glyph has no stepper: the drawn column is two baseline buttons over the same \
             step/wrap arithmetic.",
            CellFit::Natural,
            36.0,
            any(native_stepper(value, STEPPER_MIN, STEPPER_MAX)
                .step(step)
                .wraps(wraps)
                .content_description("Native stepper")
                .size(100.0, 32.0)
                .on_change(|requested| {
                    bump(stepper_events_sig());
                    confirm_stepper(requested);
                })),
            drawn,
        ),
        block(vec![
            inflexible(row(vec![
                inflexible(any(checkbox(wraps, "wraps", |_: &mut S, on: bool| {
                    stepper_wraps_sig().set(on)
                }))),
                gap_h(12.0),
                inflexible(any(checkbox(big_step, "step 2", |_: &mut S, on: bool| {
                    stepper_big_step_sig().set(on)
                }))),
            ])),
            gap(4.0),
            inflexible(readout(format!(
                "Stepper: {value} (range {STEPPER_MIN}..={STEPPER_MAX}, step {step}, wraps {}) \
                 \u{2014} events {events}",
                if wraps { "on" } else { "off" }
            ))),
        ]),
    ]
}

/// The date picker: compact (140×40pt in a pair) or inline (300×330pt native
/// cell beside the usual 140pt drawn readout, in the same pair structure) —
/// always built through [`pair_row_sized`], at the same [`FlexChildS`]
/// position, so a compact/inline switch never changes the concrete view type
/// `AnyView::rebuild` walks at that position (only the widths, `height`, and
/// `style` *values* differ). Inline, the row is 452pt — over the 319pt phone
/// pair budget [`PAIR_CELL_W`] documents — because the platform's inline
/// picker is ~300pt wide by itself; the readout cell that a phone pushes
/// off-screen is repeated in the block under the row. A version that instead swapped in a differently shaped
/// tree per mode (the picker bare in one branch, wrapped in a pair's `row`
/// in the other) tore the native picker down and rebuilt it on every
/// toggle: `AnyView::rebuild` compares the *inner* boxed view's concrete
/// type, not the erasing `AnyView` wrapper (`frust-core`'s
/// `any_view_type_swap_tears_down_and_replaces` test), so a
/// differently-shaped child at the same position is a teardown + rebuild,
/// never an in-place update. Kept structurally stable instead, the style
/// switch reaches the mounted picker as a single in-place
/// `Setter::DatePickerStyle` update; on Android, the constructor-baked mode
/// changes only on a remount — the structural stability fix preserves the
/// ability to detect and suppress those rebuilds for this native type.
fn date_block(value: CivilDate, inline: bool, events: u32) -> Vec<FlexChildS> {
    let style = if inline {
        NativeDatePickerStyle::Inline
    } else {
        NativeDatePickerStyle::Compact
    };
    let native_w = if inline { 300.0 } else { PAIR_CELL_W };
    let h = if inline { 330.0 } else { 40.0 };
    let picker = any(native_date_picker(value)
        .min(date_min())
        .max(date_max())
        .style(style)
        .content_description("Native date picker")
        .size(native_w, h)
        .on_change(|requested| {
            bump(date_events_sig());
            date_sig().set(requested);
        }));
    let classes = NativeClasses {
        android: "DatePicker",
        ios: "UIDatePicker",
        macos: "NSDatePicker",
    };
    let title = if inline {
        "Date picker (inline)"
    } else {
        "Date picker (compact)"
    };
    let note = "Glyph has no date picker: the drawn column is a Text readout of the confirmed \
                date. Range 2026-01-01..=2027-12-31.";
    let mut rows = vec![pair_row_sized(
        title,
        classes,
        note,
        CellFit::Natural,
        (native_w, PAIR_CELL_W),
        h,
        picker,
        readout(format_date(value)),
    )];
    rows.push(block(vec![
        inflexible(chip(
            if inline {
                "Switch to compact"
            } else {
                "Switch to inline"
            },
            |_: &mut S| {
                let sig = date_inline_sig();
                sig.set(!sig.get_untracked());
            },
        )),
        gap(4.0),
        inflexible(readout(format!(
            "Date: {} \u{2014} events {events}",
            format_date(value)
        ))),
        gap(2.0),
        inflexible(caption(
            "iOS/macOS apply the style switch in place, immediately (no remount). Android's \
             DatePicker bakes its spinner/calendar mode into the constructor with no setter \
             (plugins/native-widgets/src/controls/date_picker.rs's Android `apply` arm warns \
             once and ignores a later Setter::DatePickerStyle), so the switch has no visible \
             effect there until the picker remounts (leave this page and return).",
        )),
    ]));
    rows
}

/// A page-local alias so the block helpers read cleanly.
type FlexChildS = frust::FlexChild<S>;

/// See the page-fn contract in [`crate::pages`] and the [module docs](self).
pub fn page(_state: &S) -> AnyView<S> {
    // Tracked reads — native listener writes wake the page through these.
    let animating = spinner_animating_sig().get();
    let selected = segment_sig().get();
    let refusing = segment_refuse_sig().get();
    let refused = segment_refused_sig().get();
    let segment_events = segment_events_sig().get();
    let stepper_value = stepper_sig().get();
    let wraps = stepper_wraps_sig().get();
    let big_step = stepper_big_step_sig().get();
    let stepper_events = stepper_events_sig().get();
    let date_value = date_sig().get();
    let inline = date_inline_sig().get();
    let date_events = date_events_sig().get();

    let mut children = vec![page_header(SECTION)];
    children.extend(spinner_block(animating));
    children.extend(segmented_block(selected, refusing, refused, segment_events));
    children.extend(stepper_block(
        stepper_value,
        wraps,
        big_step,
        stepper_events,
    ));
    children.extend(date_block(date_value, inline, date_events));
    page_column(children)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stepped_clamps_without_wrap_and_wraps_with_it() {
        assert_eq!(stepped(10, 1, false), 10);
        assert_eq!(stepped(0, -1, false), 0);
        assert_eq!(stepped(10, 1, true), STEPPER_MIN);
        assert_eq!(stepped(0, -2, true), STEPPER_MAX);
        assert_eq!(stepped(4, 2, true), 6);
    }

    #[test]
    fn the_date_literals_are_real_dates() {
        assert_eq!(format_date(initial_date()), "2026-09-29");
        assert_eq!(format_date(date_min()), "2026-01-01");
        assert_eq!(format_date(date_max()), "2027-12-31");
    }
}
