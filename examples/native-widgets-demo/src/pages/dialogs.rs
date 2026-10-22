//! The Dialogs tab's root page: the Alerts and Sheet sections, one scroll.

use frust::{AnyView, inflexible};

use super::common::{S, page_column, page_header, tab_body};
use super::{alerts, sheet};
use crate::Tab;

/// See the page-fn contract in [`crate::pages`].
pub fn page(state: &S) -> AnyView<S> {
    tab_body(
        state,
        Tab::Dialogs,
        vec![
            inflexible(page_column(vec![page_header(Tab::Dialogs)])),
            inflexible(alerts::page(state)),
            inflexible(sheet::page(state)),
        ],
    )
}
