//! The Lab tab's root page: the Stress, Sandwich, macOS and TabBar sections,
//! one scroll.

use frust::{AnyView, inflexible};

use super::common::{S, page_column, page_header, tab_body};
use super::{macos, sandwich, stress, tab_bar};
use crate::Tab;

/// See the page-fn contract in [`crate::pages`].
pub fn page(state: &S) -> AnyView<S> {
    tab_body(
        state,
        Tab::Lab,
        vec![
            inflexible(page_column(vec![page_header(Tab::Lab)])),
            inflexible(stress::page(state)),
            inflexible(sandwich::page(state)),
            inflexible(macos::page(state)),
            inflexible(tab_bar::page(state)),
        ],
    )
}
