//! The gallery's page picker: a narrow left-hand panel built out of shadcn
//! components themselves (buttons + a separator), never a bespoke `sidebar`
//! widget — the task brief is explicit that this demo composes what the
//! catalog already ships.

use frust::{Column, EdgeInsets, Padding, Row, SizedBox, any, text};
use frust_shadcn::{ButtonVariant, button, separator};

use crate::AppState;

/// The six gallery pages, in nav order.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Primitives,
    Controls,
    InputsTable,
    Overlays,
    Anchored,
    Theming,
}

impl Page {
    const ALL: [(Page, &'static str); 6] = [
        (Page::Primitives, "Primitives"),
        (Page::Controls, "Controls"),
        (Page::InputsTable, "Inputs & Table"),
        (Page::Overlays, "Overlays"),
        (Page::Anchored, "Anchored"),
        (Page::Theming, "Theming"),
    ];
}

/// A fixed-width column of nav buttons, the active page shown as
/// [`ButtonVariant::Secondary`] and every other as [`ButtonVariant::Ghost`].
pub fn sidebar(active: Page) -> impl frust::View<AppState> + use<> {
    let mut children: Vec<frust::AnyView<AppState>> = Vec::with_capacity(Page::ALL.len() * 2 + 1);
    for (page, label) in Page::ALL {
        let variant = if page == active {
            ButtonVariant::Secondary
        } else {
            ButtonVariant::Ghost
        };
        children.push(any(
            button(label, move |s: &mut AppState| s.page = page).variant(variant)
        ));
        children.push(any(SizedBox(None, Some(4.0))));
    }
    children.push(any(separator()));
    Padding(
        EdgeInsets::all(12.0),
        Column(vec![
            any(Padding(
                EdgeInsets {
                    left: 0.0,
                    top: 0.0,
                    right: 0.0,
                    bottom: 12.0,
                },
                text("shadcn demo").size(16.0),
            )),
            any(Column(children)),
        ]),
    )
}

/// A section heading used at the top of every gallery page.
pub fn heading(title: &str) -> impl frust::View<AppState> + use<> {
    Row(vec![any(text(title.to_string()).size(24.0))])
}
