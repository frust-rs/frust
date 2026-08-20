//! The placeholder every not-yet-ported playground renders.
//!
//! The reference builds this out of its playground kit (a `PlaygroundBody`
//! holding one `PlayPreviewCard`); this port keeps it standalone — an
//! outlined card with the component's name and the batch its playground is
//! landing in — so a page stub depends on nothing the kit has not shipped yet.

use frust::{AnyView, Column, EdgeInsets, Padding, SizedBox, Theme, any, text, use_context};
use frust_material::outlined_card;

use crate::AppState;
use crate::catalog::DemoEntry;

/// The coming-soon body for `entry`, ready to drop into a playground slot.
pub fn coming_soon(entry: DemoEntry) -> AnyView<AppState> {
    let theme = use_context::<Theme>().unwrap_or_else(frust_material::baseline);
    let scheme = theme.scheme();
    let mut headline = theme.type_scale.title_medium.clone();
    headline.color = scheme.on_surface;
    let mut body = theme.type_scale.body_large.clone();
    body.color = scheme.on_surface_variant;

    any(Padding(
        EdgeInsets::all(16.0),
        outlined_card(Padding(
            EdgeInsets::all(16.0),
            Column(vec![
                any(text(entry.title).style(headline)),
                any(SizedBox::<AppState>(None, Some(8.0))),
                any(text(format!(
                    "Playground coming in batch {} ({}).",
                    entry.section.batch_number(),
                    entry.section.nav_label()
                ))
                .style(body)),
            ]),
        )),
    ))
}
