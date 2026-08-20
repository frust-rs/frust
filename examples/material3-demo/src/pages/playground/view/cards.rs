//! Cards: the reference's `CardsPlayground`.

use frust::{
    AnyView, Column, Component, CrossAxisAlignment, Row, SizedBox, Theme, any, component, text,
};
use frust_material::{CardVariant, card};

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{
    PlaySnippet, ambient_theme, control_panel, play_enum_segmented, play_preview_card,
    play_snippet, play_switch, play_text_field, playground_body,
};

/// Every [`CardVariant`] this playground cycles through — the reference's
/// `M3ECardVariant.values` order.
const VARIANTS: [CardVariant; 3] = [
    CardVariant::Elevated,
    CardVariant::Filled,
    CardVariant::Outlined,
];

/// Default card title — the reference's own `_title` seed.
const DEFAULT_TITLE: &str = "Card title";
/// Default card body — the reference's own `_body` seed.
const DEFAULT_BODY: &str = "Supporting text for the card body.";

/// This page's knob state.
struct Knobs {
    variant: CardVariant,
    tappable: bool,
    title: String,
    body: String,
}

struct CardsPlayground;

impl Component for CardsPlayground {
    type State = Knobs;

    fn init(&self) -> Knobs {
        Knobs {
            variant: CardVariant::Elevated,
            tappable: true,
            title: DEFAULT_TITLE.to_string(),
            body: DEFAULT_BODY.to_string(),
        }
    }

    fn build(&self, state: &mut Knobs) -> AnyView<Knobs> {
        body(state)
    }
}

/// See the page contract in [`crate::pages::playground`].
pub fn page(_entry: DemoEntry) -> AnyView<AppState> {
    any(component(CardsPlayground))
}

/// The playground body — everything that varies with `state`; the same
/// function [`CardsPlayground::build`] calls, and this file's tests exercise
/// directly across every knob state.
fn body(state: &Knobs) -> AnyView<Knobs> {
    let theme = ambient_theme();
    playground_body(
        vec![
            single_card_preview(&theme, state),
            all_variants_preview(&theme, state),
        ],
        vec![snippet(state)],
        vec![controls(state)],
    )
}

fn variant_label(variant: CardVariant) -> &'static str {
    match variant {
        CardVariant::Elevated => "Elevated",
        CardVariant::Filled => "Filled",
        CardVariant::Outlined => "Outlined",
    }
}

/// The card's own content — the reference's `_CardBody`.
fn card_body<State: 'static>(theme: &Theme, title: &str, body: &str) -> AnyView<State> {
    let scheme = theme.scheme();
    let mut title_style = theme.type_scale.title_medium.clone();
    title_style.color = scheme.on_surface;
    let mut body_style = theme.type_scale.body_medium.clone();
    body_style.color = scheme.on_surface_variant;
    any(Column(vec![
        any(text(title.to_string()).style(title_style)),
        any(SizedBox::<State>(None, Some(4.0))),
        any(text(body.to_string()).style(body_style)),
    ])
    .cross_axis(CrossAxisAlignment::Start))
}

fn single_card_preview(theme: &Theme, state: &Knobs) -> AnyView<Knobs> {
    let content = card_body::<Knobs>(theme, &state.title, &state.body);
    let mut view = card(state.variant, content);
    if state.tappable {
        view = view.on_press(|_: &mut Knobs| {});
    }
    any(play_preview_card(
        "Card",
        SizedBox::<Knobs>(Some(260.0), None).child(view),
    ))
}

/// One card per [`VARIANTS`] entry, side by side — the reference's `Wrap`.
/// This catalog has no reflowing `Wrap` layout, so a plain `Row` stands in
/// (three fixed-width tiles fit comfortably; a narrower viewport than the
/// reference targets would overflow rather than reflow).
fn all_variants_preview(theme: &Theme, state: &Knobs) -> AnyView<Knobs> {
    let mut cells: Vec<AnyView<Knobs>> = Vec::new();
    for (index, variant) in VARIANTS.iter().enumerate() {
        if index > 0 {
            cells.push(any(SizedBox::<Knobs>(Some(12.0), None)));
        }
        let content = card_body::<Knobs>(theme, variant_label(*variant), &state.body);
        let mut view = card(*variant, content);
        if state.tappable {
            view = view.on_press(|_: &mut Knobs| {});
        }
        cells.push(any(SizedBox::<Knobs>(Some(180.0), None).child(view)));
    }
    any(play_preview_card("All variants", Row(cells)))
}

fn controls(state: &Knobs) -> AnyView<Knobs> {
    any(control_panel::<Knobs>(
        "Appearance",
        vec![
            play_enum_segmented::<Knobs, CardVariant>(
                "Variant",
                state.variant,
                &VARIANTS,
                variant_label,
                |state: &mut Knobs, next: CardVariant| state.variant = next,
            ),
            play_text_field::<Knobs>(
                "Title",
                state.title.clone(),
                |state: &mut Knobs, next: String| state.title = next,
            ),
            play_text_field::<Knobs>(
                "Body",
                state.body.clone(),
                |state: &mut Knobs, next: String| state.body = next,
            ),
            play_switch::<Knobs>(
                "Tappable",
                state.tappable,
                |state: &mut Knobs, next: bool| {
                    state.tappable = next;
                },
            ),
        ],
    ))
}

fn snippet(state: &Knobs) -> PlaySnippet {
    let variant = format!("{:?}", state.variant);
    let mut code = format!(
        "card(CardVariant::{variant}, Column(vec![\n\
         \u{20}   any(text({title:?})),\n\
         \u{20}   any(SizedBox::<State>(None, Some(4.0))),\n\
         \u{20}   any(text({body:?})),\n\
         ]))",
        title = state.title,
        body = state.body,
    );
    if state.tappable {
        code.push_str("\n    .on_press(|_: &mut State| {});");
    } else {
        code.push(';');
    }
    play_snippet("Card", code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn knobs(variant: CardVariant, tappable: bool, title: &str, body: &str) -> Knobs {
        Knobs {
            variant,
            tappable,
            title: title.to_string(),
            body: body.to_string(),
        }
    }

    #[test]
    fn the_body_builds_across_every_knob_state() {
        for variant in VARIANTS {
            let state = knobs(variant, true, DEFAULT_TITLE, DEFAULT_BODY);
            let _view = body(&state);
        }
        for tappable in [true, false] {
            let state = knobs(CardVariant::Elevated, tappable, DEFAULT_TITLE, DEFAULT_BODY);
            let _view = body(&state);
        }
        let state = knobs(CardVariant::Filled, false, "", "");
        let _view = body(&state);
    }
}
