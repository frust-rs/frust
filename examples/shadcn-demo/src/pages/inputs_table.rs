//! Inputs & Table: text entry, grouped inputs, native select, a labelled
//! field, one-time-code entry, a data table, an attachment card, chat bubbles,
//! and a scroll area long enough to actually need its thumb.
//!
//! The full data-table recipe (sorting, filtering, paging, selection, column
//! visibility) lives on its own page; the table here is the plain component.

use frust::{Column, Component, ScrollInfo, SizedBox, View, any, column, component, row, text};
use frust_shadcn::{
    AttachmentSize, AttachmentState, BubbleAlign, BubbleVariant, FieldOrientation, InputOtpMode,
    attachment, attachment_actions, attachment_content, attachment_description, attachment_media,
    attachment_title, bubble, button, field, input, input_group, input_group_button,
    input_group_text, input_otp, native_select, scroll_area, table, table_cell, table_row,
    textarea,
};

use crate::AppState;

pub struct State {
    pub name: String,
    pub bio: String,
    pub search: String,
    pub country: Option<usize>,
    pub otp: String,
    pub otp_alpha: String,
    pub otp_completed: Option<String>,
    pub scroll_offset: f64,
    pub scroll_max: f64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            name: "Ada Lovelace".to_string(),
            bio: "Mathematician and writer.".to_string(),
            search: String::new(),
            country: Some(0),
            otp: "12".to_string(),
            otp_alpha: String::new(),
            otp_completed: None,
            scroll_offset: 0.0,
            scroll_max: 0.0,
        }
    }
}

fn gap() -> frust::AnyView<AppState> {
    any(SizedBox(None, Some(16.0)))
}

const COUNTRIES: [&str; 4] = ["United Kingdom", "United States", "Japan", "Kenya"];

pub fn page(state: &mut State) -> impl View<AppState> + use<> {
    let name = state.name.clone();
    let bio = state.bio.clone();
    let search = state.search.clone();
    let country = state.country;
    let otp = state.otp.clone();
    let otp_alpha = state.otp_alpha.clone();
    let otp_completed = state.otp_completed.clone();
    let scroll_offset = state.scroll_offset;
    let scroll_max = state.scroll_max;

    column()
        .child(crate::nav::heading("Inputs & Table"))
        .child(SizedBox(None, Some(16.0)))
        .child(
            field(
                input(name, |s: &mut AppState, v: String| {
                    s.inputs_table.name = v;
                })
                .placeholder("Full name"),
            )
            .label("Name")
            .description("Shown on your public profile."),
        )
        .child(gap())
        .child(
            field(
                textarea(bio, |s: &mut AppState, v: String| {
                    s.inputs_table.bio = v;
                })
                .placeholder("A short bio"),
            )
            .label("Bio")
            .orientation(FieldOrientation::Vertical),
        )
        .child(gap())
        .child(
            field(
                input("", |_: &mut AppState, _: String| {})
                    .invalid(true)
                    .placeholder("Required"),
            )
            .label("Invalid example")
            .error("This field is required."),
        )
        .child(gap())
        .child(
            input_group(
                input(search, |s: &mut AppState, v: String| {
                    s.inputs_table.search = v;
                })
                .placeholder("Search…"),
            )
            .addon(
                frust_shadcn::InputGroupAlign::InlineStart,
                input_group_text("\u{1F50D}"),
            )
            .addon(
                frust_shadcn::InputGroupAlign::InlineEnd,
                input_group_button("Go", |_: &mut AppState| {}),
            ),
        )
        .child(gap())
        // `native_select` is the styled trigger only — it has no popup of its
        // own (the platform's native picker is what upstream defers to), so the
        // demo cycles the options on activation and says so in the caption.
        .child(
            field(
                native_select(COUNTRIES, country, |s: &mut AppState| {
                    let next = match s.inputs_table.country {
                        Some(i) => (i + 1) % COUNTRIES.len(),
                        None => 0,
                    };
                    s.inputs_table.country = Some(next);
                })
                .placeholder("Choose a country"),
            )
            .description(
                "Styled trigger primitive only — activating it cycles the options \
             here as a stand-in. The full dropdown is the select component on \
             the Anchored page.",
            ),
        )
        .child(gap())
        // --- One-time code: the whole group is ONE focusable control ---
        .child(crate::nav::caption(
            "input_otp \u{2014} click a slot to focus the group, then type: digits \
             fill forward, Backspace clears back, the arrows/Home/End move the \
             active slot, and the active empty slot blinks a caret (static under \
             reduced motion). Pasting a code into the focused group fills the \
             slots from the active one forward.",
        ))
        .child(SizedBox(None, Some(8.0)))
        .child(
            row()
                .child(
                    input_otp(otp, 6, |s: &mut AppState, v: String| {
                        s.inputs_table.otp = v;
                    })
                    .groups(vec![3, 3])
                    .on_complete(|s: &mut AppState, code: String| {
                        s.inputs_table.otp_completed = Some(code);
                    })
                    .label("Six-digit code"),
                )
                .child(SizedBox(Some(24.0), None))
                .child(
                    input_otp(otp_alpha, 4, |s: &mut AppState, v: String| {
                        s.inputs_table.otp_alpha = v;
                    })
                    .mode(InputOtpMode::Alphanumeric)
                    .label("Four-character code"),
                )
                .child(SizedBox(Some(24.0), None))
                .child(
                    input_otp("42", 4, |_: &mut AppState, _: String| {})
                        .disabled(true)
                        .label("Disabled code"),
                ),
        )
        .child(SizedBox(None, Some(8.0)))
        .child(crate::nav::caption(match otp_completed {
            Some(code) => format!("on_complete fired with {code}"),
            None => "on_complete has not fired yet (fill all six digits).".to_string(),
        }))
        .child(gap())
        // --- Table ---
        .child(
            table(vec![
                table_row(vec![
                    table_cell("Ada Lovelace"),
                    table_cell("Mathematician"),
                ]),
                table_row(vec![
                    table_cell("Alan Turing"),
                    table_cell("Computer scientist"),
                ])
                .selected(true),
                table_row(vec![table_cell("Grace Hopper"), table_cell("Rear Admiral")]),
            ])
            .header(["Name", "Role"])
            .caption("A short roster."),
        )
        .child(gap())
        // --- Attachment ---
        .child(
            attachment(vec![
                any(attachment_media(any::<AppState, _>(text("\u{1F4C4}")))),
                any(attachment_content(vec![
                    any(attachment_title("resume.pdf")),
                    any(attachment_description("240 KB", false)),
                ])),
                any(attachment_actions(vec![button(
                    "Remove",
                    |_: &mut AppState| {},
                )])),
            ])
            .size(AttachmentSize::Default)
            .state(AttachmentState::Done),
        )
        .child(gap())
        // --- Bubbles: `bubble()` implements `View<()>` only (it fires no
        // callback into any app state), so it mounts through `component(..)`
        // — the seam that hosts a state-independent subtree inside any
        // ambient state tree (`ComponentView<C>` implements `View<Outer>` for
        // every `Outer`).
        .child(component(Bubbles))
        .child(gap())
        // --- Scroll area: a fixed 200px viewport over 40 rows, so the thumb
        // (`ScrollInfo::max_offset > 0`) actually shows up.
        .child(
            frust::SizedBox::<AppState>(None, Some(200.0)).child(
                scroll_area(
                    Column(
                        (1..=40)
                            .map(|i| any::<AppState, _>(text(format!("Row {i}")).size(14.0)))
                            .collect(),
                    ),
                    |s: &mut AppState, info: ScrollInfo| {
                        s.inputs_table.scroll_offset = info.offset;
                        s.inputs_table.scroll_max = info.max_offset;
                    },
                )
                .position(scroll_offset, scroll_max),
            ),
        )
}

/// A tiny stateless component hosting a two-message chat exchange —
/// `bubble()`'s state boundary (see the call site above).
struct Bubbles;

impl Component for Bubbles {
    type State = ();

    fn init(&self) -> Self::State {}

    fn build(&self, _state: &mut Self::State) -> impl View<Self::State> {
        any(column()
            .child(bubble("Hey, is the design ready?").align(BubbleAlign::Start))
            .child(SizedBox(None, Some(8.0)))
            .child(
                bubble("Yes, just shipped it.")
                    .align(BubbleAlign::End)
                    .variant(BubbleVariant::Default),
            ))
    }
}
