//! Cards: the reference's `CardsPlayground`.

use frust::{
    AnyView, Column, Component, CrossAxisAlignment, SizedBox, Theme, View, any, column, component,
    text,
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

/// [`all_variants_preview`]'s tile width, in logical px — the reference's own
/// `Wrap` tile width, unchanged by the reflow fix below (G16).
const ALL_VARIANTS_TILE_WIDTH: f64 = 180.0;
/// Gap between adjacent tiles in [`all_variants_preview`], in logical px —
/// the reference's `Wrap` `spacing`.
const ALL_VARIANTS_GAP: f64 = 12.0;

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

    fn build(&self, state: &mut Knobs) -> impl View<Knobs> {
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
fn body(state: &Knobs) -> impl View<Knobs> {
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
fn card_body<State: 'static>(theme: &Theme, title: &str, body: &str) -> impl View<State> {
    let scheme = theme.scheme();
    let mut title_style = theme.type_scale.title_medium.clone();
    title_style.color = scheme.on_surface;
    let mut body_style = theme.type_scale.body_medium.clone();
    body_style.color = scheme.on_surface_variant;
    column()
        .child(text(title.to_string()).style(title_style))
        .child(SizedBox::<State>(None, Some(4.0)))
        .child(text(body.to_string()).style(body_style))
        .cross_axis(CrossAxisAlignment::Start)
}

fn single_card_preview(theme: &Theme, state: &Knobs) -> AnyView<Knobs> {
    let content = card_body::<Knobs>(theme, &state.title, &state.body);
    let mut view = card(state.variant, content);
    if state.tappable {
        view = view.on_press(|_: &mut Knobs| {});
    }
    play_preview_card("Card", SizedBox::<Knobs>(Some(260.0), None).child(view))
}

/// One card per [`VARIANTS`] entry, stacked one per row — the reference's
/// `Wrap`. This catalog has no reflowing `Wrap` layout (see the crate-wide
/// note other playground pages carry), and no horizontal-scroll primitive to
/// pan a `Row` in either, so a plain `Row` of all-inflexible fixed-width
/// tiles is the G10 overflow class (`docs/LIMITATIONS.md`'s baseline `Clip`
/// note; the shared mechanism is documented at
/// `crate::pages::playground::find::progress`'s module docs): at a phone-ish
/// card's ~296px inner width (`360 - 4 * MaterialSpacing::LG`), a single
/// [`ALL_VARIANTS_TILE_WIDTH`]-wide tile fits but a second one plus its
/// [`ALL_VARIANTS_GAP`] does not (`180 + 12 + 180 = 372 > 296`, computed from
/// those two constants rather than guessed) — so this reflows into a plain
/// `Column` instead, one tile per row (G16). Split into [`all_variants_column`]
/// (this file's tests exercise it directly at the card's own inner width) and
/// this wrapper, the same split `crate::pages::playground::find::progress`'s
/// `all_styles_row`/G10 fix uses.
fn all_variants_preview(theme: &Theme, state: &Knobs) -> AnyView<Knobs> {
    play_preview_card("All variants", all_variants_column(theme, state))
}

/// The tile column [`all_variants_preview`] wraps in a [`play_preview_card`].
fn all_variants_column(theme: &Theme, state: &Knobs) -> impl View<Knobs> {
    let mut rows: Vec<AnyView<Knobs>> = Vec::new();
    for (index, variant) in VARIANTS.iter().enumerate() {
        if index > 0 {
            rows.push(any(SizedBox::<Knobs>(None, Some(ALL_VARIANTS_GAP))));
        }
        let content = card_body::<Knobs>(theme, variant_label(*variant), &state.body);
        let mut view = card(*variant, content);
        if state.tappable {
            view = view.on_press(|_: &mut Knobs| {});
        }
        rows.push(any(
            SizedBox::<Knobs>(Some(ALL_VARIANTS_TILE_WIDTH), None).child(view)
        ));
    }
    Column(rows)
}

fn controls(state: &Knobs) -> AnyView<Knobs> {
    control_panel::<Knobs>(
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
    )
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

    /// G16 regression: [`all_variants_preview`] must never paint past the
    /// "All variants" card's real inner width at a phone-ish device width —
    /// the same paint-extent idiom
    /// `crate::pages::playground::find::progress`'s G10 test uses.
    ///
    /// Reverting the fix (back to a `Row` of three fixed-180px tiles) fails
    /// this test: two tiles plus their gap alone (`180 + 12 + 180 = 372`)
    /// already exceed a phone-ish card's ~296px inner width, so the second
    /// card's right edge paints straight through the card.
    #[test]
    fn all_variants_preview_never_paints_past_the_cards_inner_width() {
        use frust::authoring::{
            BoxConstraints, BuildCtx, LayoutCtx, PaintCtx, PaintScene, View, Widget,
        };
        use frust::kurbo::{Point, Size};
        use frust::peniko::Color;

        const DEVICE_WIDTH: f64 = 360.0;
        let card_inner_width = DEVICE_WIDTH - 4.0 * frust_material::MaterialSpacing::LG;

        #[derive(Default)]
        struct MaxXScene {
            max_x: f64,
        }
        impl PaintScene for MaxXScene {
            fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
            fn draw_text(&mut self, _origin: Point, _text: &str) {}
            fn fill_rounded_rect(
                &mut self,
                origin: Point,
                size: Size,
                _radius: f64,
                _color: Color,
            ) {
                self.max_x = self.max_x.max(origin.x + size.width);
            }
        }

        let theme = frust_material::baseline();
        let state = knobs(CardVariant::Elevated, true, DEFAULT_TITLE, DEFAULT_BODY);
        let view = all_variants_column(&theme, &state);
        let mut counter = 0u64;
        let mut widget = view.build(&mut BuildCtx::new(&mut counter));
        let bc = BoxConstraints::new(Size::ZERO, Size::new(card_inner_width, 2000.0));
        let mut text_ctx = frust::authoring::text::TextContext::new();
        let mut layout_ctx = LayoutCtx::with_resources(
            Some(&mut text_ctx as &mut dyn std::any::Any),
            Some(&theme as &dyn std::any::Any),
        );
        let size = widget.layout(&mut layout_ctx, &bc);
        assert!(
            size.width <= card_inner_width + 1e-6,
            "the card's own reported size {size:?} must not exceed the inner \
             width {card_inner_width}"
        );

        let mut ctx = PaintCtx::new(Point::ZERO, size).with_theme(&theme);
        let mut scene = MaxXScene::default();
        widget.paint(&mut ctx, &mut scene);
        assert!(
            scene.max_x <= card_inner_width + 1e-6,
            "G16 regressed: painted x {} exceeds the card's inner width {} \
             (reported size {size:?})",
            scene.max_x,
            card_inner_width
        );
    }
}
