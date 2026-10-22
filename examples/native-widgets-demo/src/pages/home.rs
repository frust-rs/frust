//! The Home tab's root page: for now its header and one row that pushes a
//! [`detail`](super::detail) page, enough to exercise the chrome's pushed
//! title, subtitle, back item and toolbar.

use frust::{AnyView, inflexible};

use super::common::{S, block, caption, chip, gap, page_column, page_header, tab_body};
use super::detail;
use crate::{DetailRoute, Tab};

/// The pushed page's route for the sample row.
fn sample_route() -> DetailRoute {
    DetailRoute::new("Rendering: damage-aware present", "frust-rs/frust")
}

/// See the page-fn contract in [`crate::pages`].
pub fn page(state: &S) -> AnyView<S> {
    let rows = block(vec![
        inflexible(caption("My Work")),
        gap(6.0),
        inflexible(chip("Open a pull request", |state: &mut S| {
            let route = sample_route();
            let (handles, shown) = (state.clone(), route.clone());
            route.push(state.nav(Tab::Home), move || detail::page(&handles, &shown));
        })),
    ]);
    tab_body(
        state,
        Tab::Home,
        vec![inflexible(page_column(vec![page_header(Tab::Home), rows]))],
    )
}
