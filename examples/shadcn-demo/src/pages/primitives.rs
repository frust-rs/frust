//! Primitives: the mostly-static, non-modal, non-anchored components —
//! badges/avatars/labels/dividers, a card, an item row, a breadcrumb trail,
//! an empty state, a marker, and the button/button-group family with a
//! disabled example (the `NotAllowed` cursor check).

use frust::{Row, SizedBox, View, any, column, text};
use frust_shadcn::{
    AlertVariant, AvatarSize, BadgeVariant, ButtonGroupOrientation, ButtonSize, ButtonVariant,
    EmptyMediaVariant, ItemVariant, MarkerVariant, alert, alert_description, alert_title,
    aspect_ratio, avatar, badge, breadcrumb_ellipsis, breadcrumb_item, breadcrumb_link,
    breadcrumb_list, breadcrumb_page, breadcrumb_separator, button, button_group, card,
    card_content, card_description, card_footer, card_header, card_title, empty, empty_content,
    empty_description, empty_header, empty_media, empty_title, item, item_actions, item_content,
    item_description, item_media, item_title, kbd, kbd_group, label, marker, progress, separator,
    skeleton, spinner,
};

use crate::AppState;

/// No interactive state on this page yet — every widget shown is either
/// static or a self-contained `on_press`.
#[derive(Default)]
pub struct State;

fn row(children: Vec<frust::AnyView<AppState>>) -> impl View<AppState> + use<> {
    Row(children)
}

fn gap() -> impl frust::View<AppState> {
    SizedBox(Some(12.0), None)
}

pub fn page(_state: &mut State) -> impl View<AppState> + use<> {
    column()
        .child(crate::nav::heading("Primitives"))
        .child(SizedBox(None, Some(16.0)))
        // --- Badges, avatars, kbd, labels ---
        .child(row(vec![
            any(badge("Default")),
            any(gap()),
            any(badge("Secondary").variant(BadgeVariant::Secondary)),
            any(gap()),
            any(badge("Destructive").variant(BadgeVariant::Destructive)),
            any(gap()),
            any(badge("Outline").variant(BadgeVariant::Outline)),
            any(gap()),
            any(avatar::<AppState>().fallback("ED")),
            any(gap()),
            any(avatar::<AppState>().fallback("SM").size(AvatarSize::Sm)),
            any(gap()),
            any(kbd("Ctrl")),
            any(SizedBox(Some(4.0), None)),
            any(kbd_group(vec![kbd("Shift"), kbd("P")])),
        ]))
        .child(SizedBox(None, Some(16.0)))
        .child(row(vec![
            any(label("A plain label")),
            any(gap()),
            any(label("Disabled").disabled(true)),
        ]))
        .child(SizedBox(None, Some(16.0)))
        .child(separator())
        .child(SizedBox(None, Some(16.0)))
        // --- Progress / spinner / skeleton / marker ---
        .child(row(vec![
            any(progress(0.65)),
            any(gap()),
            any(spinner()),
            any(gap()),
            any(skeleton(120.0, 20.0)),
            any(gap()),
            any(marker("1")),
            any(gap()),
            any(marker("Section").variant(MarkerVariant::Separator)),
        ]))
        .child(SizedBox(None, Some(16.0)))
        // --- Aspect ratio ---
        .child(aspect_ratio(
            16.0 / 9.0,
            any::<AppState, _>(skeleton(240.0, 0.0).radius(8.0)),
        ))
        .child(SizedBox(None, Some(16.0)))
        // --- Breadcrumb ---
        .child(breadcrumb_list((
            breadcrumb_item(breadcrumb_link("Home").on_click(|_: &mut AppState| {})),
            breadcrumb_separator::<AppState>(),
            breadcrumb_item(breadcrumb_link("Components")),
            breadcrumb_separator::<AppState>(),
            breadcrumb_ellipsis(),
            breadcrumb_separator::<AppState>(),
            breadcrumb_item::<AppState, _>(breadcrumb_page("Primitives")),
        )))
        .child(SizedBox(None, Some(16.0)))
        // --- Alert ---
        .child(alert(
            AlertVariant::Default,
            vec![
                alert_title("Heads up", AlertVariant::Default),
                alert_description("This is a default alert.", AlertVariant::Default),
            ],
        ))
        .child(SizedBox(None, Some(8.0)))
        .child(alert(
            AlertVariant::Destructive,
            vec![
                alert_title("Error", AlertVariant::Destructive),
                alert_description("Something went wrong.", AlertVariant::Destructive),
            ],
        ))
        .child(SizedBox(None, Some(16.0)))
        // --- Card ---
        .child(card(vec![
            card_header(vec![
                card_title("Card title"),
                card_description("A short supporting line."),
            ]),
            card_content(text("Card body content goes here.")),
            card_footer((
                button("Cancel", |_: &mut AppState| {}).variant(ButtonVariant::Ghost),
                button("Save", |_: &mut AppState| {}),
            )),
        ]))
        .child(SizedBox(None, Some(16.0)))
        // --- Item ---
        .child(
            item((
                item_media(any::<AppState, _>(avatar::<AppState>().fallback("IT"))),
                item_content((
                    item_title("Item title"),
                    item_description("A trailing detail."),
                )),
                item_actions(vec![
                    button("Open", |_: &mut AppState| {}).size(ButtonSize::Sm),
                ]),
            ))
            .variant(ItemVariant::Outline),
        )
        .child(SizedBox(None, Some(16.0)))
        // --- Empty state ---
        .child(empty((
            empty_header((
                empty_media(any::<AppState, _>(text("\u{1F4ED}").size(28.0)))
                    .variant(EmptyMediaVariant::Icon),
                empty_title("Nothing here yet"),
                empty_description("Once you add data it shows up in this space."),
            )),
            empty_content(vec![button("Add data", |_: &mut AppState| {})]),
        )))
        .child(SizedBox(None, Some(16.0)))
        .child(separator())
        .child(SizedBox(None, Some(16.0)))
        // --- Buttons, sizes, disabled ---
        .child(row(vec![
            any(button("Default", |_: &mut AppState| {})),
            any(gap()),
            any(button("Secondary", |_: &mut AppState| {}).variant(ButtonVariant::Secondary)),
            any(gap()),
            any(button("Outline", |_: &mut AppState| {}).variant(ButtonVariant::Outline)),
            any(gap()),
            any(button("Destructive", |_: &mut AppState| {}).variant(ButtonVariant::Destructive)),
            any(gap()),
            any(button("Link", |_: &mut AppState| {}).variant(ButtonVariant::Link)),
            any(gap()),
            any(button("Disabled", |_: &mut AppState| {}).disabled(true)),
        ]))
        .child(SizedBox(None, Some(12.0)))
        .child(
            button_group(vec![
                button("Left", |_: &mut AppState| {}).size(ButtonSize::Sm),
                button("Middle", |_: &mut AppState| {}).size(ButtonSize::Sm),
                button("Right", |_: &mut AppState| {}).size(ButtonSize::Sm),
            ])
            .orientation(ButtonGroupOrientation::Horizontal),
        )
}
