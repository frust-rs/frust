//! Inputs & Table: text entry, grouped inputs, native select, a labelled
//! field, a data table, an attachment card, chat bubbles, and a scroll area
//! long enough to actually need its thumb.

use frust::{AnyView, Column, Component, ScrollInfo, SizedBox, View, any, component, text};
use frust_shadcn::{
    AttachmentSize, AttachmentState, BubbleAlign, BubbleVariant, FieldOrientation, attachment,
    attachment_actions, attachment_content, attachment_description, attachment_media,
    attachment_title, bubble, button, field, input, input_group, input_group_button,
    input_group_text, native_select, scroll_area, table, table_cell, table_row, textarea,
};

use crate::AppState;

pub struct State {
    pub name: String,
    pub bio: String,
    pub search: String,
    pub country: Option<usize>,
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
    let scroll_offset = state.scroll_offset;
    let scroll_max = state.scroll_max;

    Column(vec![
        any(crate::nav::heading("Inputs & Table")),
        any(SizedBox(None, Some(16.0))),
        any(field(
            input(name, |s: &mut AppState, v: String| {
                s.inputs_table.name = v;
            })
            .placeholder("Full name"),
        )
        .label("Name")
        .description("Shown on your public profile.")),
        gap(),
        any(field(
            textarea(bio, |s: &mut AppState, v: String| {
                s.inputs_table.bio = v;
            })
            .placeholder("A short bio"),
        )
        .label("Bio")
        .orientation(FieldOrientation::Vertical)),
        gap(),
        any(field(
            input("", |_: &mut AppState, _: String| {})
                .invalid(true)
                .placeholder("Required"),
        )
        .label("Invalid example")
        .error("This field is required.")),
        gap(),
        any(input_group(
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
        )),
        gap(),
        any(native_select(COUNTRIES, country, |s: &mut AppState| {
            let next = match s.inputs_table.country {
                Some(i) => (i + 1) % COUNTRIES.len(),
                None => 0,
            };
            s.inputs_table.country = Some(next);
        })
        .placeholder("Choose a country")),
        gap(),
        // --- Table ---
        any(table(vec![
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
        .caption("A short roster.")),
        gap(),
        // --- Attachment ---
        any(attachment(vec![
            any(attachment_media(any::<AppState, _>(text("\u{1F4C4}")))),
            any(attachment_content(vec![
                any(attachment_title("resume.pdf")),
                any(attachment_description("240 KB", false)),
            ])),
            any(attachment_actions(vec![any(button(
                "Remove",
                |_: &mut AppState| {},
            ))])),
        ])
        .size(AttachmentSize::Default)
        .state(AttachmentState::Done)),
        gap(),
        // --- Bubbles: `bubble()` implements `View<()>` only (it fires no
        // callback into any app state), so it mounts through `component(..)`
        // — the seam that hosts a state-independent subtree inside any
        // ambient state tree (`ComponentView<C>` implements `View<Outer>` for
        // every `Outer`).
        any(component(Bubbles)),
        gap(),
        // --- Scroll area: a fixed 200px viewport over 40 rows, so the thumb
        // (`ScrollInfo::max_offset > 0`) actually shows up.
        any(frust::SizedBox::<AppState>(None, Some(200.0)).child(
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
        )),
    ])
}

/// A tiny stateless component hosting a two-message chat exchange —
/// `bubble()`'s state boundary (see the call site above).
struct Bubbles;

impl Component for Bubbles {
    type State = ();

    fn init(&self) -> Self::State {}

    fn build(&self, _state: &mut Self::State) -> AnyView<Self::State> {
        any(Column(vec![
            any(bubble("Hey, is the design ready?").align(BubbleAlign::Start)),
            any(SizedBox(None, Some(8.0))),
            any(bubble("Yes, just shipped it.")
                .align(BubbleAlign::End)
                .variant(BubbleVariant::Default)),
        ]))
    }
}
