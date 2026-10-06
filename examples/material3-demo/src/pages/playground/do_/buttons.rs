//! Buttons: the reference's `ButtonsPlayground`.
//!
//! Every `frust_material::button`/`button_with_icon` prop the reference
//! exercises is ported and mirrored here — variant (style), size, shape,
//! enabled, an optional leading icon, the label text, and the gradient
//! decoration seam ([`GradientButtonDecoration`]). The one visual gap this
//! page inherits rather than introduces: [`mod@frust_material::button`]'s own
//! module docs note there is no ambient icon-theme propagation in this
//! framework, so — matching this kit's own [`copy_to_clipboard`] icon in
//! `code_snippet.rs`, which leaves its icon uncolored the same way — the
//! leading icon here carries no explicit ink either.
//!
//! Knobs live in a page-local [`Knobs`], owned by the nested [`ButtonsPlayground`]
//! `Component` (never [`AppState`]) per the page contract in
//! [`crate::pages::playground`]. `Style`/`Size` use the two-piece
//! [`play_enum_menu_field`]/[`play_enum_menu_panel`] control (each needs its
//! own [`OverlayAnchor`], both held in `Knobs`, with the panels mounted at
//! this page's own outer [`Stack`]); `Shape` fits a plain
//! [`play_enum_segmented`] since it is a 2-way choice.
//!
//! # `spaced_row` becomes `spaced_column` in "All styles"/"Gradient fill" (G10)
//!
//! Both previews used to lay their buttons out in a non-wrapping `spaced_row`
//! (this workspace has no reflowing `Wrap` layout and no horizontal-scroll
//! primitive either). A button's natural width is size/label-dependent
//! (`Knobs::size` ranges `Xs..=Xl`, whose `h_padding` alone spans 16px to
//! 64px per side — `frust_material::button::core`'s size-metrics table), so
//! no compile-time chunk count is derivable the way a fixed-tile catalog's
//! can be (`crate::pages::playground::view::shapes`' `CATALOG_COLUMNS`).
//! Measured at a phone-ish card's ~296px inner width: "All styles"'s five
//! buttons already overflow at the *default* `Sm` size (~379px natural
//! width), and "Gradient fill"'s two buttons overflow at `Xl` (~472px) even
//! though they fit at `Sm` — so both previews reflow into a [`spaced_column`]
//! (one button per row) rather than risk the class at some reachable knob
//! state.

use std::rc::Rc;

use frust::{AnyView, Color, Column, Component, SizedBox, Stack, View, any, component, icon};
use frust_material::{
    ButtonDecoration, ButtonShape, ButtonSize, ButtonVariant, GradientButtonDecoration,
    LinearGradientSpec, OverlayAnchor, button, button_with_icon, constant_gradient, icons,
};

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{
    PlaySnippet, control_panel, play_enum_menu_field, play_enum_menu_panel, play_enum_segmented,
    play_preview_card, play_snippet, play_switch, play_text_field, playground_body,
};

/// Every [`ButtonVariant`] this page's "All styles" preview and "Style" menu
/// iterate — the crate's own declaration order (the reference's own
/// `M3EButtonStyle.values` order differs, but nothing observable depends on
/// which order a menu lists options in).
const STYLES: [ButtonVariant; 5] = [
    ButtonVariant::Filled,
    ButtonVariant::Outlined,
    ButtonVariant::Tonal,
    ButtonVariant::Elevated,
    ButtonVariant::Text,
];

/// Every [`ButtonSize`] the "Size" menu offers.
const SIZES: [ButtonSize; 5] = [
    ButtonSize::Xs,
    ButtonSize::Sm,
    ButtonSize::Md,
    ButtonSize::Lg,
    ButtonSize::Xl,
];

/// Every [`ButtonShape`] the "Shape" segmented control offers.
const SHAPES: [ButtonShape; 2] = [ButtonShape::Round, ButtonShape::Square];

/// This page's own knob state — held by [`ButtonsPlayground`], never
/// [`AppState`] (the page contract in [`crate::pages::playground`]).
struct Knobs {
    style: ButtonVariant,
    size: ButtonSize,
    shape: ButtonShape,
    enabled: bool,
    show_icon: bool,
    label: String,
    /// Shared with [`style_menu_panel`] — the "Style" dropdown's anchor.
    style_anchor: OverlayAnchor,
    style_open: bool,
    /// Shared with [`size_menu_panel`] — the "Size" dropdown's anchor.
    size_anchor: OverlayAnchor,
    size_open: bool,
}

impl Default for Knobs {
    /// The reference's own `_ButtonsPlaygroundState` field initializers.
    fn default() -> Self {
        Self {
            style: ButtonVariant::Filled,
            size: ButtonSize::Sm,
            shape: ButtonShape::Round,
            enabled: true,
            show_icon: true,
            label: "Label".to_string(),
            style_anchor: OverlayAnchor::new(),
            style_open: false,
            size_anchor: OverlayAnchor::new(),
            size_open: false,
        }
    }
}

/// Style label for the menu/snippet — the reference's `M3EButtonStyle.name`.
fn style_label(style: ButtonVariant) -> &'static str {
    match style {
        ButtonVariant::Filled => "filled",
        ButtonVariant::Outlined => "outlined",
        ButtonVariant::Tonal => "tonal",
        ButtonVariant::Elevated => "elevated",
        ButtonVariant::Text => "text",
    }
}

/// Size label for the menu/snippet — the reference's `M3EButtonSize.name`.
fn size_label(size: ButtonSize) -> &'static str {
    match size {
        ButtonSize::Xs => "xs",
        ButtonSize::Sm => "sm",
        ButtonSize::Md => "md",
        ButtonSize::Lg => "lg",
        ButtonSize::Xl => "xl",
    }
}

/// Shape label for the segmented control — the reference's
/// `M3EButtonShape.name`.
fn shape_label(shape: ButtonShape) -> &'static str {
    match shape {
        ButtonShape::Round => "round",
        ButtonShape::Square => "square",
    }
}

/// The named constructor a plain (non-icon) button of `style` snippets as.
fn variant_ctor(style: ButtonVariant) -> &'static str {
    match style {
        ButtonVariant::Filled => "filled_button",
        ButtonVariant::Outlined => "outlined_button",
        ButtonVariant::Tonal => "tonal_button",
        ButtonVariant::Elevated => "elevated_button",
        ButtonVariant::Text => "text_button",
    }
}

/// The "Selected style" preview: one button built from every knob.
fn styled_button(state: &Knobs) -> AnyView<Knobs> {
    let mut view = button(state.label.clone(), |_: &mut Knobs| {})
        .variant(state.style)
        .size(state.size)
        .shape(state.shape)
        .enabled(state.enabled);
    if state.show_icon {
        view = view.icon(any(icon(icons::ADD)));
    }
    any(view)
}

/// The "All styles" preview: every [`ButtonVariant`] at the current size and
/// shape, one per row — see the module docs' `Wrap` divergence (G10).
fn all_styles(state: &Knobs) -> AnyView<Knobs> {
    let buttons: Vec<AnyView<Knobs>> = STYLES
        .iter()
        .map(|&style| {
            any(button(style_label(style), |_: &mut Knobs| {})
                .variant(style)
                .size(state.size)
                .shape(state.shape)
                .enabled(state.enabled))
        })
        .collect();
    spaced_column(buttons, 12.0)
}

/// The "Gradient fill" preview: a plain and an icon-carrying button, both
/// decorated via [`GradientButtonDecoration`] — the reference's two literal
/// gradient examples (`buttons_playground.dart`'s own hardcoded colors, not
/// knob-driven).
fn gradient_fill(state: &Knobs) -> AnyView<Knobs> {
    let purple: Rc<dyn ButtonDecoration> = Rc::new(
        GradientButtonDecoration::new()
            .background(constant_gradient(LinearGradientSpec::new(vec![
                Color::from_rgb8(0x67, 0x50, 0xA4),
                Color::from_rgb8(0x9A, 0x82, 0xDB),
            ])))
            .foreground(constant_gradient(LinearGradientSpec::new(vec![
                Color::from_rgb8(0xFF, 0xFF, 0xFF),
                Color::from_rgb8(0xEA, 0xDD, 0xFF),
            ])))
            .outline(constant_gradient(LinearGradientSpec::new(vec![
                Color::from_rgb8(0x4F, 0x37, 0x8B),
                Color::from_rgb8(0xD0, 0xBC, 0xFF),
            ]))),
    );
    let red: Rc<dyn ButtonDecoration> = Rc::new(
        GradientButtonDecoration::new()
            .background(constant_gradient(LinearGradientSpec::new(vec![
                Color::from_rgb8(0xB3, 0x26, 0x1E),
                Color::from_rgb8(0xE4, 0x69, 0x62),
            ])))
            .foreground(constant_gradient(LinearGradientSpec::new(vec![
                Color::from_rgb8(0xFF, 0xFF, 0xFF),
                Color::from_rgb8(0xFE, 0xCA, 0xCA),
            ]))),
    );
    let gradient = any(button("Gradient", |_: &mut Knobs| {})
        .size(state.size)
        .shape(state.shape)
        .decoration(purple));
    let gradient_icon =
        any(
            button_with_icon(any(icon(icons::FAVORITE)), "Favorite", |_: &mut Knobs| {})
                .size(state.size)
                .shape(state.shape)
                .decoration(red),
        );
    spaced_column(vec![gradient, gradient_icon], 12.0)
}

/// The paste-ready snippet for the current knob state — the reference's
/// `_snippets`, in Frust rather than Dart.
fn snippet(state: &Knobs) -> PlaySnippet {
    let code = if state.show_icon {
        format!(
            "button_with_icon(any(icon(icons::ADD)), \"{label}\", on_press)\n    .variant(ButtonVariant::{style:?})\n    .size(ButtonSize::{size:?})\n    .shape(ButtonShape::{shape:?})\n    .enabled({enabled});",
            label = state.label,
            style = state.style,
            size = state.size,
            shape = state.shape,
            enabled = state.enabled,
        )
    } else {
        format!(
            "{ctor}(\"{label}\", on_press)\n    .size(ButtonSize::{size:?})\n    .shape(ButtonShape::{shape:?})\n    .enabled({enabled});",
            ctor = variant_ctor(state.style),
            label = state.label,
            size = state.size,
            shape = state.shape,
            enabled = state.enabled,
        )
    };
    play_snippet("Selected style", code)
}

/// "Appearance" controls: style, shape, size.
fn appearance_panel(state: &Knobs) -> AnyView<Knobs> {
    control_panel(
        "Appearance",
        vec![
            play_enum_menu_field(
                "Style",
                state.style,
                &STYLES,
                style_label,
                &state.style_anchor,
                state.style_open,
                |state: &mut Knobs, open: bool| state.style_open = open,
            ),
            play_enum_segmented(
                "Shape",
                state.shape,
                &SHAPES,
                shape_label,
                |state: &mut Knobs, next: ButtonShape| state.shape = next,
            ),
            play_enum_menu_field(
                "Size",
                state.size,
                &SIZES,
                size_label,
                &state.size_anchor,
                state.size_open,
                |state: &mut Knobs, open: bool| state.size_open = open,
            ),
        ],
    )
}

/// "Content" controls: label, show-icon, enabled.
fn content_panel(state: &Knobs) -> AnyView<Knobs> {
    control_panel(
        "Content",
        vec![
            play_text_field("Label", state.label.clone(), |state: &mut Knobs, next| {
                state.label = next;
            }),
            play_switch("Show icon", state.show_icon, |state: &mut Knobs, next| {
                state.show_icon = next;
            }),
            play_switch("Enabled", state.enabled, |state: &mut Knobs, next| {
                state.enabled = next;
            }),
        ],
    )
}

/// The "Style" menu's popup half — mounted at this page's outer [`Stack`].
fn style_menu_panel(state: &Knobs) -> AnyView<Knobs> {
    play_enum_menu_panel(
        state.style,
        &STYLES,
        style_label,
        &state.style_anchor,
        state.style_open,
        |state: &mut Knobs, open: bool| state.style_open = open,
        |state: &mut Knobs, next: ButtonVariant| state.style = next,
    )
}

/// The "Size" menu's popup half — mounted at this page's outer [`Stack`].
fn size_menu_panel(state: &Knobs) -> AnyView<Knobs> {
    play_enum_menu_panel(
        state.size,
        &SIZES,
        size_label,
        &state.size_anchor,
        state.size_open,
        |state: &mut Knobs, open: bool| state.size_open = open,
        |state: &mut Knobs, next: ButtonSize| state.size = next,
    )
}

/// Lay `items` out vertically with `gap`px between each pair, one per row —
/// the reflow-safe alternative to a `spaced_row` of size/label-variable-width
/// buttons (see the module docs' `Wrap` divergence, G10). `Column` has no
/// spacing knob of its own, the same interleaved-spacer idiom
/// `theme_config_page`'s own `spaced_row` uses horizontally.
fn spaced_column<State: 'static>(items: Vec<AnyView<State>>, gap: f64) -> AnyView<State> {
    let mut children: Vec<AnyView<State>> = Vec::with_capacity(items.len() * 2);
    for item in items {
        if !children.is_empty() {
            children.push(any(SizedBox::<State>(None, Some(gap))));
        }
        children.push(item);
    }
    any(Column(children))
}

/// The page body: the playground content plus the two dropdown panels it
/// anchors, stacked so both can paint above the scrollable content.
fn body(state: &Knobs) -> AnyView<Knobs> {
    let content = playground_body(
        vec![
            play_preview_card("Selected style", styled_button(state)),
            play_preview_card("All styles", all_styles(state)),
            play_preview_card("Gradient fill", gradient_fill(state)),
        ],
        vec![snippet(state)],
        vec![appearance_panel(state), content_panel(state)],
    );
    any(Stack(vec![
        content,
        style_menu_panel(state),
        size_menu_panel(state),
    ]))
}

/// This page's knob component — see the [module docs](self).
struct ButtonsPlayground;

impl Component for ButtonsPlayground {
    type State = Knobs;

    fn init(&self) -> Self::State {
        Knobs::default()
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        body(state)
    }
}

/// See the page contract in [`crate::pages::playground`].
pub fn page(_entry: DemoEntry) -> AnyView<AppState> {
    any(component(ButtonsPlayground))
}

#[cfg(test)]
mod tests {
    use super::{Knobs, SHAPES, SIZES, STYLES, body, snippet};

    #[test]
    fn the_page_builds_across_every_style() {
        let mut knobs = Knobs::default();
        for style in STYLES {
            knobs.style = style;
            let _view = body(&knobs);
        }
    }

    #[test]
    fn the_page_builds_across_every_size() {
        let mut knobs = Knobs::default();
        for size in SIZES {
            knobs.size = size;
            let _view = body(&knobs);
        }
    }

    #[test]
    fn the_page_builds_across_every_shape() {
        let mut knobs = Knobs::default();
        for shape in SHAPES {
            knobs.shape = shape;
            let _view = body(&knobs);
        }
    }

    #[test]
    fn the_page_builds_with_the_icon_hidden_and_disabled() {
        let knobs = Knobs {
            show_icon: false,
            enabled: false,
            label: String::new(),
            ..Knobs::default()
        };
        let _view = body(&knobs);
    }

    #[test]
    fn the_page_builds_with_both_dropdown_menus_open() {
        let knobs = Knobs {
            style_open: true,
            size_open: true,
            ..Knobs::default()
        };
        let _view = body(&knobs);
    }

    #[test]
    fn the_snippet_switches_shape_between_the_icon_factory_and_a_named_constructor() {
        let mut knobs = Knobs::default();
        assert!(snippet(&knobs).code.contains("button_with_icon"));
        knobs.show_icon = false;
        assert!(snippet(&knobs).code.contains("filled_button"));
    }

    /// G10 regression: neither [`super::all_styles`] nor [`super::gradient_fill`]
    /// may ever paint past their card's real inner width at a phone-ish
    /// device width — the same paint-extent idiom
    /// `crate::pages::playground::find::progress`'s own G10 test uses.
    /// `all_styles` is checked at the *default* `Sm` size (already
    /// overflowing before the fix); `gradient_fill` at `Xl` (its own worst
    /// case — see the module docs).
    ///
    /// Reverting either fix (back to a `spaced_row`) fails this test.
    #[test]
    fn all_styles_and_gradient_fill_never_paint_past_the_cards_inner_width() {
        use std::any::Any;

        use frust::authoring::Point;
        use frust::authoring::text::TextContext;
        use frust::authoring::{
            BoxConstraints, BuildCtx, LayoutCtx, PaintCtx, PaintScene, Size, View, Widget,
        };
        use frust::kurbo::{BezPath, Shape as _};
        use frust::peniko::{Brush, Color};

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
            fn fill_rounded_rect_brush(
                &mut self,
                origin: Point,
                size: Size,
                _radius: f64,
                _brush: &Brush,
            ) {
                self.max_x = self.max_x.max(origin.x + size.width);
            }
            fn stroke_path(&mut self, origin: Point, path: &BezPath, _width: f64, _brush: &Brush) {
                self.max_x = self.max_x.max(origin.x + path.bounding_box().x1);
            }
        }

        fn assert_never_overflows(label: &str, view: super::AnyView<Knobs>, card_inner_width: f64) {
            let mut counter = 0u64;
            let mut widget = view.build(&mut BuildCtx::new(&mut counter));
            let bc = BoxConstraints::new(Size::ZERO, Size::new(card_inner_width, 2000.0));
            let mut text_ctx = TextContext::new();
            let mut layout_ctx =
                LayoutCtx::with_resources(Some(&mut text_ctx as &mut dyn Any), None);
            let size = widget.layout(&mut layout_ctx, &bc);
            assert!(
                size.width <= card_inner_width + 1e-6,
                "{label}'s own reported size {size:?} must not exceed the \
                 card's inner width {card_inner_width}"
            );

            let mut ctx = PaintCtx::new(Point::ZERO, size);
            let mut scene = MaxXScene::default();
            widget.paint(&mut ctx, &mut scene);
            assert!(
                scene.max_x <= card_inner_width + 1e-6,
                "G10 regressed in {label}: painted x {} exceeds the card's \
                 inner width {} (reported size {size:?})",
                scene.max_x,
                card_inner_width
            );
        }

        let sm_knobs = Knobs::default();
        assert_never_overflows("all_styles", super::all_styles(&sm_knobs), card_inner_width);

        let xl_knobs = Knobs {
            size: frust_material::ButtonSize::Xl,
            ..Knobs::default()
        };
        assert_never_overflows(
            "gradient_fill",
            super::gradient_fill(&xl_knobs),
            card_inner_width,
        );
    }
}
