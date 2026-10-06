//! Blocks · Forms: the five data-entry blocks — the one-time-code field, the
//! sign-up form, the upload queue, the feedback widget and the weekly
//! availability scheduler.
//!
//! Like the other Blocks pages this is a [`component`] over its own retained
//! [`State`]: every block here is controlled, and [`crate::AppState`] carries no
//! field for any of it.
//!
//! Three of the five carry a premise worth stating up front, and the captions
//! below say the same thing beside each block:
//!
//! - the **code field** is typed entry only — there is no paste or autofill
//!   path behind it;
//! - the **upload queue** is programmatic: frust publishes no file-drop signal,
//!   so the dropzone reports a press through `on_browse` and files arrive
//!   through `add_files`, with `dragging` left as a prop an app with its own
//!   drag signal can drive. Under `reduce_motion` a removed row vanishes at
//!   once instead of playing its exit;
//! - the **availability scheduler** is a list of seven day rows with time
//!   ranges and a copy-to-days menu. It is not a drag-select week grid; upstream
//!   has no time-slot cells and no drag gesture at all.
//!
//! # The two panels this page hosts itself
//!
//! The scheduler holds one open time panel for the whole week and reports which
//! one, leaving the app to mount the picker — the same division every
//! overlay-hosted control in the catalog makes. This page mounts both that
//! picker and the copy-to-days menu **inline**, as plain rows of buttons under
//! the scheduler, rather than through `frust_beui::overlay`: an anchored host
//! needs bounded constraints and must stay outside a scroll view, and the
//! gallery's page slot is a scroll view. The keys, the option lists and the
//! clamping are the block's own (`panel_key`, `start_options`/`end_options`,
//! `clamp_range`, `copy_day`); only the presentation is this page's.

use frust_beui::blocks::availability_scheduler::{
    DayAvailability, DayKey, TimeEdge, WeekAvailability, availability_scheduler, clamp_range,
    copy_day, default_week, end_options, label_12, panel_key, start_options, to_value,
};
use frust_beui::blocks::feedback_widget::{FeedbackPosition, FeedbackStatus, feedback_widget};
use frust_beui::blocks::file_upload::{
    FileUploadItem, FileUploadStatus, FileUploadVariant, add_files, file_upload, file_upload_item,
};
use frust_beui::blocks::otp_input::{OtpStatus, otp_input};
use frust_beui::blocks::signup_form::{SignupStatus, SignupValues, default_validate, signup_form};
use frust_beui::components::button::{ButtonSize, ButtonTone, button};

use frust::{
    AnyView, Axis, Column, Component, CrossAxisAlignment, FlexView, SizedBox, TextView, View, any,
    component, inflexible, text,
};

use crate::AppState;

// ---- The page's own state --------------------------------------------------

/// The code the OTP demo accepts, the way upstream's own preview does it.
const OTP_CODE: &str = "123456";

/// The address the sign-up demo rejects, upstream's own failure hook.
const TAKEN_EMAIL_PREFIX: &str = "taken@";

/// Minutes between the times the scheduler's picker offers. Kept in step with
/// the `step` handed to the block itself — the option list this page builds and
/// the grid the block clamps against have to be the same one.
const SCHEDULER_STEP: u32 = 60;

/// Everything the five blocks on this page are driven by.
pub struct State {
    otp_value: String,
    otp_status: OtpStatus,
    signup_values: SignupValues,
    signup_status: SignupStatus,
    signup_error: Option<String>,
    upload_items: Vec<FileUploadItem>,
    upload_variant: FileUploadVariant,
    /// The dropzone's dragging chrome — a prop here, driven by the demo's own
    /// toggle rather than by a drag signal frust does not publish.
    upload_dragging: bool,
    /// Serial for the synthetic files the browse button appends.
    upload_seq: usize,
    feedback_status: FeedbackStatus,
    feedback_message: String,
    /// How many times feedback has been submitted — the first attempt fails, so
    /// the failure view is reachable.
    feedback_attempts: usize,
    week: WeekAvailability,
    /// The one time panel the week has open, as a `panel_key`.
    open_panel: Option<String>,
    /// The day whose hours the copy menu is offering, when it is open.
    copy_source: Option<DayKey>,
    /// Serial for the range ids the copy menu mints.
    range_seq: usize,
    scheduler_log: String,
}

impl Default for State {
    fn default() -> Self {
        Self {
            otp_value: String::new(),
            otp_status: OtpStatus::Idle,
            signup_values: SignupValues::default(),
            signup_status: SignupStatus::Idle,
            signup_error: None,
            upload_items: seed_uploads(),
            upload_variant: FileUploadVariant::Centered,
            upload_dragging: false,
            upload_seq: 0,
            feedback_status: FeedbackStatus::Idle,
            feedback_message: String::new(),
            feedback_attempts: 0,
            week: default_week(),
            open_panel: None,
            copy_source: None,
            range_seq: 0,
            scheduler_log: "(no change yet)".to_string(),
        }
    }
}

/// The queue's starting rows, upstream's own preview data: one finished, one in
/// flight, one failed.
fn seed_uploads() -> Vec<FileUploadItem> {
    vec![
        file_upload_item("brand-assets", "brand-assets.zip", 18_400_000)
            .mime("application/zip")
            .progress(100.0)
            .status(FileUploadStatus::Success),
        file_upload_item("release-video", "release-cut.mov", 84_200_000)
            .mime("video/quicktime")
            .progress(58.0)
            .status(FileUploadStatus::Uploading),
        file_upload_item("contracts", "vendor-contract.pdf", 2_800_000)
            .mime("application/pdf")
            .progress(32.0)
            .error("Connection lost"),
    ]
}

// ---- Page chrome -----------------------------------------------------------

/// The page's own title.
fn heading(title: &str) -> TextView {
    text(title.to_string()).size(24.0)
}

/// A block's title.
fn section(title: &str) -> TextView {
    text(title.to_string()).size(16.0)
}

/// A block's small print.
fn caption(body: impl Into<String>) -> TextView {
    text(body.into()).size(12.0)
}

/// A vertical gap.
fn gap(height: f64) -> AnyView<State> {
    any(SizedBox(None, Some(height)))
}

/// A horizontal gap.
fn hgap(width: f64) -> AnyView<State> {
    any(SizedBox(Some(width), None))
}

/// A centred row of controls.
fn controls(children: Vec<AnyView<State>>) -> AnyView<State> {
    let mut spaced = Vec::with_capacity(children.len() * 2);
    for (index, child) in children.into_iter().enumerate() {
        if index > 0 {
            spaced.push(hgap(8.0));
        }
        spaced.push(child);
    }
    any(FlexView::new(
        Axis::Horizontal,
        spaced.into_iter().map(inflexible).collect(),
    )
    .cross_axis(CrossAxisAlignment::Center))
}

/// A small secondary button.
fn minor(label: impl Into<String>, on_press: impl Fn(&mut State) + 'static) -> AnyView<State> {
    any(button(label, on_press)
        .tone(ButtonTone::Secondary)
        .size(ButtonSize::Sm))
}

// ---- The one-time-code field -----------------------------------------------

/// The OTP block: six slots, typed entry, and the error state the wrong code
/// puts it in.
fn otp_block(state: &State) -> AnyView<State> {
    any(Column(vec![
        any(section("otp_input")),
        gap(6.0),
        any(caption(
            "Six slots that fill as digits are typed, the caret walking forward and Backspace \
             walking back. Typed entry only — there is no paste or platform autofill path behind \
             this block. Enter the hinted code for the success state, anything else for the error \
             one.",
        )),
        gap(10.0),
        any(
            otp_input(state.otp_value.clone(), |s: &mut State, code: String| {
                s.otp_value = code;
                s.otp_status = OtpStatus::Idle;
            })
            .label("Verification code")
            .hint(format!("Enter {OTP_CODE} to verify."))
            .success_message("Verified.")
            .error_message("Wrong code, try again.")
            .status(state.otp_status)
            .on_complete(|s: &mut State, code: String| {
                s.otp_status = if code == OTP_CODE {
                    OtpStatus::Success
                } else {
                    OtpStatus::Error
                };
            }),
        ),
        gap(10.0),
        controls(vec![minor("Clear", |s: &mut State| {
            s.otp_value.clear();
            s.otp_status = OtpStatus::Idle;
        })]),
    ]))
}

// ---- The sign-up form ------------------------------------------------------

/// The sign-up block: the five fields, the strength meter, and the rejection
/// upstream's own preview wires up.
fn signup_block(state: &State) -> AnyView<State> {
    any(Column(vec![
        any(section("signup_form")),
        gap(6.0),
        any(caption(
            "Five controlled fields under the block's own rule set — required names, an address \
             shape, a password floor, a confirmation match and the terms box — each error \
             appearing and clearing on its own ramp. Sign up with taken@example.com to see the \
             failure state.",
        )),
        gap(10.0),
        signup_view(state),
        gap(10.0),
        controls(vec![minor("Reset form", |s: &mut State| {
            s.signup_values = SignupValues::default();
            s.signup_status = SignupStatus::Idle;
            s.signup_error = None;
        })]),
    ]))
}

/// The form itself. `error_message` is only chained when there *is* one — the
/// setter wraps whatever it is given in `Some`, so handing it an empty string
/// would leave the banner permanently mounted.
fn signup_view(state: &State) -> AnyView<State> {
    let form = signup_form(
        state.signup_values.clone(),
        |s: &mut State, values: SignupValues| {
            s.signup_values = values;
            if s.signup_status != SignupStatus::Idle {
                s.signup_status = SignupStatus::Idle;
                s.signup_error = None;
            }
        },
    )
    .header(
        "Create your account",
        "Sign up with taken@example.com to see the failure state.",
    )
    .status(state.signup_status)
    .strength_meter(true)
    .validate(default_validate)
    .on_submit(|s: &mut State, values: SignupValues| {
        if values.email.to_lowercase().starts_with(TAKEN_EMAIL_PREFIX) {
            s.signup_status = SignupStatus::Error;
            s.signup_error = Some("That email is already registered.".to_string());
        } else {
            s.signup_status = SignupStatus::Success;
            s.signup_error = None;
        }
    });
    match &state.signup_error {
        Some(message) => any(form.error_message(message.clone())),
        None => any(form),
    }
}

// ---- The upload queue ------------------------------------------------------

/// The upload block: the dropzone, the queue, and the demo's stand-ins for the
/// two things a real host would supply — a file dialog and a transfer.
fn upload_block(state: &State) -> AnyView<State> {
    let variant_buttons: Vec<AnyView<State>> = [
        ("Centered", FileUploadVariant::Centered),
        ("Row", FileUploadVariant::Default),
    ]
    .into_iter()
    .map(|(label, variant)| {
        any(
            button(label, move |s: &mut State| s.upload_variant = variant)
                .tone(if state.upload_variant == variant {
                    ButtonTone::Primary
                } else {
                    ButtonTone::Outline
                })
                .size(ButtonSize::Sm),
        )
    })
    .collect();

    any(Column(vec![
        any(section("file_upload")),
        gap(6.0),
        any(caption(
            "A dropzone over a queue: one row per file with its own progress track, status glyph, \
             retry and remove. The block is programmatic — frust publishes no file-drop signal, so \
             the dropzone reports a press through `on_browse` and files arrive through \
             `add_files`, while `dragging` stays a prop for an app that has a drag signal of its \
             own. Under reduce_motion a removed row vanishes at once rather than playing its exit.",
        )),
        gap(10.0),
        controls(variant_buttons),
        gap(10.0),
        any(file_upload(state.upload_items.clone(), |s: &mut State| {
            // What a host's file dialog would hand back. `add_files` is the
            // block's own slot arithmetic, so the cap below is enforced here
            // exactly as it would be for real files.
            s.upload_seq += 1;
            let index = s.upload_seq;
            let incoming = vec![
                file_upload_item(
                    format!("picked-{index}"),
                    format!("capture-{index:02}.png"),
                    1_240_000 + (index as u64) * 90_000,
                )
                .mime("image/png"),
            ];
            add_files(&mut s.upload_items, incoming, true, Some(6));
        })
        .variant(state.upload_variant)
        .title("Drop files here")
        .description("Add files to the upload queue")
        .browse_label("Browse")
        .max_files(6)
        .dragging(state.upload_dragging)
        .on_remove(|s: &mut State, id: String| s.upload_items.retain(|item| item.id != id))
        .on_retry(|s: &mut State, id: String| {
            for item in &mut s.upload_items {
                if item.id == id {
                    *item = item.clone().retrying();
                }
            }
        })),
        gap(10.0),
        controls(vec![
            minor("Advance uploads", |s: &mut State| {
                // The stand-in for a transfer: a real app steps progress from
                // its own timer or IO completion, not from a button.
                for item in &mut s.upload_items {
                    if item.status == FileUploadStatus::Uploading {
                        let next = (item.progress + 14.0).min(100.0);
                        item.progress = next;
                        if next >= 100.0 {
                            item.status = FileUploadStatus::Success;
                        }
                    }
                }
            }),
            minor(
                if state.upload_dragging {
                    "Dragging: on"
                } else {
                    "Dragging: off"
                },
                |s: &mut State| s.upload_dragging = !s.upload_dragging,
            ),
            minor("Reset queue", |s: &mut State| {
                s.upload_items = seed_uploads();
                s.upload_seq = 0;
            }),
        ]),
        gap(8.0),
        any(caption(format!(
            "{} of 6 slots used",
            state.upload_items.len()
        ))),
    ]))
}

// ---- The feedback widget ---------------------------------------------------

/// The feedback block: the corner trigger and its four panel states.
fn feedback_block(state: &State) -> AnyView<State> {
    any(Column(vec![
        any(section("feedback_widget")),
        gap(6.0),
        any(caption(
            "A corner trigger that morphs open into a message panel and back, carrying its own \
             sending, thanks and failure views. The first submission here fails so the retry view \
             is reachable; the second succeeds.",
        )),
        gap(10.0),
        any(feedback_widget(
            state.feedback_status,
            state.feedback_message.clone(),
            |s: &mut State, status: FeedbackStatus| s.feedback_status = status,
            |s: &mut State, message: String| s.feedback_message = message,
        )
        .position(FeedbackPosition::BottomRight)
        .title("Help us improve")
        .placeholder("Share an idea or report a bug")
        .on_submit(|s: &mut State, message: String| {
            s.feedback_attempts += 1;
            s.feedback_message = message;
            // The send is synchronous here; a real app would sit in
            // `Sending` until its request answered.
            s.feedback_status = if s.feedback_attempts == 1 {
                FeedbackStatus::Error
            } else {
                s.feedback_message.clear();
                FeedbackStatus::Sent
            };
        })),
        gap(10.0),
        controls(vec![minor("Reset attempts", |s: &mut State| {
            s.feedback_attempts = 0;
            s.feedback_message.clear();
            s.feedback_status = FeedbackStatus::Idle;
        })]),
    ]))
}

// ---- The availability scheduler --------------------------------------------

/// Every time the picker offers, on the same grid the block clamps against.
fn time_options() -> Vec<String> {
    (0..(24 * 60))
        .step_by(SCHEDULER_STEP as usize)
        .map(to_value)
        .collect()
}

/// Resolve the `panel_key` the scheduler reported back to the field it names.
///
/// Matched by rebuilding each candidate's key with the block's own
/// [`panel_key`] rather than by parsing the string apart, so the key format
/// stays the block's business.
fn resolve_panel(week: &WeekAvailability, key: &str) -> Option<(DayKey, String, TimeEdge)> {
    for day in DayKey::WEEK {
        for range in &week.day(day).ranges {
            for edge in [TimeEdge::Start, TimeEdge::End] {
                if panel_key(day, &range.id, edge) == key {
                    return Some((day, range.id.clone(), edge));
                }
            }
        }
    }
    None
}

/// Apply a picked time to the week, through the block's own clamp.
fn apply_time(state: &mut State, day: DayKey, range_id: &str, edge: TimeEdge, picked: &str) {
    let options = time_options();
    let mut next = state.week.day(day).clone();
    for range in &mut next.ranges {
        if range.id != range_id {
            continue;
        }
        let (start, end) = match edge {
            TimeEdge::Start => clamp_range(picked, &range.end, &options, TimeEdge::Start),
            TimeEdge::End => clamp_range(&range.start, picked, &options, TimeEdge::End),
        };
        range.start = start;
        range.end = end;
    }
    state.week = state.week.with_day(day, next);
    state.open_panel = None;
    state.scheduler_log = format!(
        "{} {} \u{2192} {}",
        day.label(),
        edge.key(),
        label_12(picked)
    );
}

/// The time picker this page mounts for the one panel the scheduler reports
/// open. See the [module docs](self) for why it is inline rather than anchored.
fn time_panel(state: &State) -> Option<AnyView<State>> {
    let key = state.open_panel.as_deref()?;
    let (day, range_id, edge) = resolve_panel(&state.week, key)?;
    let range = state
        .week
        .day(day)
        .ranges
        .iter()
        .find(|range| range.id == range_id)?
        .clone();

    let options = time_options();
    let offered = match edge {
        TimeEdge::Start => start_options(&options, &range.end, Some(&range.start)),
        TimeEdge::End => end_options(&options, &range.start, Some(&range.end)),
    };
    let current = match edge {
        TimeEdge::Start => range.start.clone(),
        TimeEdge::End => range.end.clone(),
    };

    let mut rows: Vec<AnyView<State>> = Vec::new();
    for chunk in offered.chunks(6) {
        let buttons: Vec<AnyView<State>> = chunk
            .iter()
            .map(|option| {
                let option = option.clone();
                let selected = option == current;
                let range_id = range_id.clone();
                any(button(label_12(&option), move |s: &mut State| {
                    apply_time(s, day, &range_id, edge, &option);
                })
                .tone(if selected {
                    ButtonTone::Primary
                } else {
                    ButtonTone::Outline
                })
                .size(ButtonSize::Sm))
            })
            .collect();
        rows.push(controls(buttons));
        rows.push(gap(6.0));
    }
    rows.push(controls(vec![minor("Close picker", |s: &mut State| {
        s.open_panel = None;
    })]));

    Some(any(Column(
        std::iter::once(any(caption(format!(
            "Picking the {} of {}\u{2019}s {}\u{2013}{}",
            edge.key(),
            day.label(),
            label_12(&range.start),
            label_12(&range.end)
        ))))
        .chain(std::iter::once(gap(8.0)))
        .chain(rows)
        .collect(),
    )))
}

/// The copy-to-days menu this page mounts for a reported copy request.
fn copy_panel(state: &State) -> Option<AnyView<State>> {
    let from = state.copy_source?;
    let targets: Vec<AnyView<State>> = DayKey::WEEK
        .into_iter()
        .filter(|day| *day != from)
        .map(|day| {
            any(button(day.label(), move |s: &mut State| {
                let mut seq = s.range_seq;
                let next = copy_day(&s.week, from, &[day], |target| {
                    seq += 1;
                    format!("{}-copy-{seq}", target.key())
                });
                s.range_seq = seq;
                s.week = next;
                s.copy_source = None;
                s.scheduler_log = format!("copied {} to {}", from.label(), day.label());
            })
            .tone(ButtonTone::Outline)
            .size(ButtonSize::Sm))
        })
        .collect();

    Some(any(Column(vec![
        any(caption(format!(
            "Copy {}\u{2019}s hours to\u{2026}",
            from.label()
        ))),
        gap(8.0),
        controls(targets),
        gap(6.0),
        controls(vec![minor("Cancel copy", |s: &mut State| {
            s.copy_source = None;
        })]),
    ])))
}

/// How many hours the week currently holds, for the readout under the block.
fn weekly_hours(week: &WeekAvailability) -> f64 {
    DayKey::WEEK
        .into_iter()
        .map(|day| {
            let value: &DayAvailability = week.day(day);
            if !value.enabled {
                return 0.0;
            }
            value
                .ranges
                .iter()
                .map(|range| {
                    let start = to_minutes_local(&range.start);
                    let end = to_minutes_local(&range.end);
                    (end.saturating_sub(start)) as f64 / 60.0
                })
                .sum::<f64>()
        })
        .sum()
}

/// `"HH:MM"` in minutes past midnight — the readout above needs the arithmetic
/// and nothing else in this page does.
fn to_minutes_local(value: &str) -> u32 {
    match value.split_once(':') {
        Some((hours, minutes)) => {
            hours.trim().parse::<u32>().unwrap_or(0) * 60
                + minutes.trim().parse::<u32>().unwrap_or(0)
        }
        None => 0,
    }
}

/// The scheduler block: seven day rows, their ranges, and the two panels this
/// page hosts for it.
fn scheduler_block(state: &State) -> AnyView<State> {
    let mut children = vec![
        any(section("availability_scheduler")),
        gap(6.0),
        any(caption(
            "One row per weekday, each springing between available and unavailable, its time \
             ranges adding and removing on their own ramps. Per-day time ranges and a \
             copy-to-days menu \u{2014} not a drag-select week grid: the source has no time-slot \
             cells and no drag gesture. The block holds one open time panel for the whole week \
             and reports which; this page mounts the picker and the copy menu below it.",
        )),
        gap(10.0),
        any(availability_scheduler(
            state.week.clone(),
            |s: &mut State, week: WeekAvailability| {
                s.week = week;
                s.scheduler_log = "week edited".to_string();
            },
        )
        .step(SCHEDULER_STEP)
        .open_panel(state.open_panel.clone())
        .on_panel_open_change(|s: &mut State, key: Option<String>| {
            s.open_panel = key;
            s.copy_source = None;
        })
        .on_copy_request(|s: &mut State, day: DayKey| {
            s.copy_source = Some(day);
            s.open_panel = None;
        })),
        gap(12.0),
    ];

    if let Some(panel) = time_panel(state) {
        children.push(panel);
        children.push(gap(12.0));
    }
    if let Some(panel) = copy_panel(state) {
        children.push(panel);
        children.push(gap(12.0));
    }

    children.push(any(caption(format!(
        "{:.1} hours a week \u{b7} {}",
        weekly_hours(&state.week),
        state.scheduler_log
    ))));
    children.push(gap(8.0));
    children.push(controls(vec![minor("Reset week", |s: &mut State| {
        s.week = default_week();
        s.open_panel = None;
        s.copy_source = None;
        s.scheduler_log = "reset".to_string();
    })]));

    any(Column(children))
}

// ---- The page --------------------------------------------------------------

/// The Blocks · Forms page.
#[derive(Default)]
struct FormsPage;

impl Component for FormsPage {
    type State = State;

    fn init(&self) -> State {
        State::default()
    }

    fn build(&self, state: &mut State) -> impl View<State> {
        any(Column(vec![
            any(heading("Blocks \u{b7} Forms")),
            gap(8.0),
            any(caption(
                "Five data-entry blocks, every one of them controlled: the value, the status and \
                 the open panel are the caller\u{2019}s, and each block reports what it would like \
                 to happen instead of doing it.",
            )),
            gap(24.0),
            otp_block(state),
            gap(24.0),
            signup_block(state),
            gap(24.0),
            upload_block(state),
            gap(24.0),
            feedback_block(state),
            gap(24.0),
            scheduler_block(state),
        ]))
    }
}

/// The Blocks · Forms page, hosted over its own retained [`State`].
pub fn page() -> AnyView<AppState> {
    any(component(FormsPage))
}
