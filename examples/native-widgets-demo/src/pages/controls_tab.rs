//! The Controls tab's root page: the Controls, New and Composite sections,
//! one scroll.

use frust::{AnyView, inflexible};

use super::common::{S, page_column, page_header, tab_body};
use super::{composite, controls, new_controls};
use crate::Tab;

/// See the page-fn contract in [`crate::pages`].
pub fn page(state: &S) -> AnyView<S> {
    tab_body(
        state,
        Tab::Controls,
        vec![
            inflexible(page_column(vec![page_header(Tab::Controls)])),
            inflexible(controls::page(state)),
            inflexible(new_controls::page(state)),
            inflexible(composite::page(state)),
        ],
    )
}
