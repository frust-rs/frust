//! A pushed detail page. Its bar title and subtitle travel as its
//! [`DetailRoute`]; the page itself only draws the body, on the shell's
//! detail scroll controller.

use frust::{AnyView, Get, any, inflexible, safe_area, scroll_view};

use super::common::{S, block, caption, gap, label, page_column};
use crate::DetailRoute;

/// See the page-fn contract in [`crate::pages`]; `route` is what this page
/// was pushed with.
pub fn page(state: &S, route: &DetailRoute) -> AnyView<S> {
    let composer = if state.composer_open.get() {
        "Composer: open"
    } else {
        "Composer: closed"
    };
    let body = page_column(vec![block(vec![
        inflexible(label(route.title.clone())),
        gap(4.0),
        inflexible(caption(route.subtitle.clone())),
        gap(6.0),
        inflexible(caption(composer)),
    ])]);
    any(scroll_view(safe_area(body)).controller(state.detail_scroll.clone()))
}
