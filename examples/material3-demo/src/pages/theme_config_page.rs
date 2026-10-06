//! The theme settings route: toggles, seed color, and type — the reference's
//! `ThemeConfigPage`. Every change applies to the running app immediately
//! through [`ThemeSettings`]'s mutators; see that type's own module docs for
//! the settings model and its two ported degrades (inverted-auto brightness,
//! the Regular/Emphasized-only type-style picker). This file adds one more,
//! recorded at each control below: dynamic color is shown disabled rather
//! than omitted (`crate::theme`'s module docs already explain why it isn't
//! modelled at all). The font-family and type-style pickers also swap the
//! reference's bespoke tappable swatches/chips for this catalog's own
//! [`frust_material::segmented_button`]; only the seed picker keeps a
//! hand-built swatch grid, since nothing in this catalog stands in for it.

use frust::{
    AnyView, Color, Column, CrossAxisAlignment, EdgeInsets, GestureDetector, Padding, Row,
    SizedBox, Theme, any, column, container, scroll_view, text, use_context,
};
use frust_material::{card_list_items, list_item, segment, segmented_button, switch};

use crate::AppState;
use crate::pages::playground_scaffold;
use crate::theme::{DemoFont, DemoTypeStyle, SEED_OPTIONS, ThemeSettings};

/// Seed swatch diameter, in logical px — the reference's `_SeedSwatch._size`
/// (its font swatch shares the same constant).
const SEED_SWATCH_SIZE: f64 = 56.0;

/// Selected-swatch ring width, in logical px — the reference's selected
/// border width.
const SEED_SWATCH_SELECTED_BORDER: f64 = 3.0;

/// Unselected-swatch ring width, in logical px — the reference's default
/// border width.
const SEED_SWATCH_BORDER: f64 = 1.0;

/// Gap between adjacent seed swatches, in logical px — matches [`body`]'s own
/// `EdgeInsets::all(16.0)` page padding.
const SEED_GAP: f64 = 16.0;
/// Swatches per row in [`seeds`]'s grid. A plain `Row` of all-inflexible
/// fixed-width children is the G10 overflow class
/// (`crate::pages::playground::find::progress`'s module docs): this page has
/// no reflowing `Wrap` layout, so [`SEED_OPTIONS`]' five swatches are chunked
/// into fixed rows instead, computed from [`SEED_SWATCH_SIZE`]/[`SEED_GAP`]
/// against this page's own inner width (`360 - 2 * 16 = 328`, one
/// `EdgeInsets::all(16.0)` layer — [`body`] wraps `seeds` directly, unlike a
/// playground page's `play_preview_card`): 5 columns is
/// `5 * 56 + 4 * 16 = 344 > 328` (overflows — this was the bug), 4 columns is
/// `4 * 56 + 3 * 16 = 272 <= 328` (fits).
const SEED_ROW_COLUMNS: usize = 4;

/// The theme settings screen, pushed over the gallery shell.
pub fn theme_config_page() -> AnyView<AppState> {
    let settings = use_context::<ThemeSettings>();
    let theme = use_context::<Theme>().unwrap_or_else(frust_material::baseline);
    let content = match settings {
        Some(settings) => body(&theme, settings),
        // No scope above this page — only reachable in a bare harness, never
        // in the running app.
        None => any(SizedBox::<AppState>(None, None)),
    };
    playground_scaffold("Theme settings", content)
}

/// The scrollable content: toggles, then seed, then type — the reference's
/// own section order.
fn body(theme: &Theme, settings: ThemeSettings) -> AnyView<AppState> {
    any(scroll_view(Padding(
        EdgeInsets::all(16.0),
        column()
            .child(toggles(theme, settings))
            .child(SizedBox::<AppState>(None, Some(24.0)))
            .child(seeds(theme, settings))
            .child(SizedBox::<AppState>(None, Some(24.0)))
            .child(type_section(theme, settings)),
    )))
}

/// Auto theming (live) and dynamic color (unavailable) — the reference's
/// `_toggles`. Dynamic color stays in the list, disabled, rather than
/// dropping the row: a reader who knows the reference notices the same two
/// rows and reads why the second does nothing, instead of a silently
/// shortened list.
fn toggles(theme: &Theme, settings: ThemeSettings) -> AnyView<AppState> {
    let brightness = theme.brightness;
    let rows = vec![
        list_item::<AppState>("Auto theming")
            .supporting("Follow the platform light and dark setting")
            .trailing(switch(
                settings.auto_theming(),
                move |state: &mut AppState, value: bool| {
                    state.settings.set_auto_theming(value, brightness);
                },
            )),
        list_item::<AppState>("Dynamic color")
            .supporting("Not available — frust has no device wallpaper palette to read")
            .trailing(switch(false, |_: &mut AppState, _: bool| {}).enabled(false)),
    ];
    any(card_list_items(rows))
}

/// Seed color swatches — the reference's `_seeds`. Always selectable:
/// dynamic coloring is never on (it isn't modelled at all — see the toggles
/// section above), so unlike the reference this picker has no disabled state
/// to show.
fn seeds(theme: &Theme, settings: ThemeSettings) -> AnyView<AppState> {
    let scheme = theme.scheme();
    let mut title_style = theme.type_scale.title_medium.clone();
    title_style.color = scheme.on_surface;
    let mut detail_style = theme.type_scale.body_medium.clone();
    detail_style.color = scheme.on_surface_variant;

    let brightness = theme.brightness;
    let selected_index = settings.seed_index();
    let swatches: Vec<AnyView<AppState>> = SEED_OPTIONS
        .iter()
        .enumerate()
        .map(|(index, (color, label))| {
            seed_swatch(
                theme,
                *color,
                label,
                index == selected_index,
                move |state: &mut AppState| {
                    state.settings.set_seed(index, brightness);
                },
            )
        })
        .collect();

    any(column()
        .child(text("Seed color").style(title_style))
        .child(SizedBox::<AppState>(None, Some(4.0)))
        .child(text("Generates the scheme for both brightnesses.").style(detail_style))
        .child(SizedBox::<AppState>(None, Some(16.0)))
        .child(swatch_grid(swatches)))
}

/// One seed color choice: a circular swatch, ringed when selected — the
/// reference's `_SeedSwatch`. This port skips the reference's
/// contrast-computed checkmark (this crate has no color-contrast helper to
/// pick a legible tick color against an arbitrary seed) and marks the
/// selection with a heavier ring instead, the same selected-state language
/// [`frust_material::list_item`] uses elsewhere in this app.
fn seed_swatch<F: Fn(&mut AppState) + 'static>(
    theme: &Theme,
    color: Color,
    label: &'static str,
    selected: bool,
    on_tap: F,
) -> AnyView<AppState> {
    let scheme = theme.scheme();
    let mut label_style = theme.type_scale.label_medium.clone();
    label_style.color = if selected {
        scheme.on_surface
    } else {
        scheme.on_surface_variant
    };
    let (border_color, border_width) = if selected {
        (scheme.on_surface, SEED_SWATCH_SELECTED_BORDER)
    } else {
        (scheme.outline_variant, SEED_SWATCH_BORDER)
    };

    any(GestureDetector(
        column()
            .child(
                container(SizedBox::<AppState>(None, None))
                    .size_centered(SEED_SWATCH_SIZE, SEED_SWATCH_SIZE)
                    .fill(color)
                    .radius(SEED_SWATCH_SIZE / 2.0)
                    .border(border_color, border_width),
            )
            .child(SizedBox::<AppState>(None, Some(8.0)))
            .child(text(label).style(label_style))
            .cross_axis(CrossAxisAlignment::Center),
    )
    .on_tap(on_tap))
}

/// Font family and type style, both segmented — the reference's `_type`.
/// Both pickers stay enabled throughout: unlike the reference, this port's
/// type style isn't a Roboto Flex variable-font axis tied to one specific
/// family, but a weight step applied to the type scale directly
/// (`crate::theme`'s module docs' Type styles section), so there is no
/// `stylesEnabled` gate to reproduce.
fn type_section(theme: &Theme, settings: ThemeSettings) -> AnyView<AppState> {
    let scheme = theme.scheme();
    let mut title_style = theme.type_scale.title_medium.clone();
    title_style.color = scheme.on_surface;
    let mut small_title_style = theme.type_scale.title_small.clone();
    small_title_style.color = scheme.on_surface;
    let mut detail_style = theme.type_scale.body_medium.clone();
    detail_style.color = scheme.on_surface_variant;

    let brightness = theme.brightness;
    let font_picker = any(segmented_button::<AppState, DemoFont, _>(
        DemoFont::ALL
            .iter()
            .map(|font| segment(*font).label(font.label())),
        [settings.font()],
        move |state: &mut AppState, selection: Vec<DemoFont>| {
            if let Some(font) = selection.into_iter().next() {
                state.settings.set_font(font, brightness);
            }
        },
    ));
    let style_picker = any(segmented_button::<AppState, DemoTypeStyle, _>(
        DemoTypeStyle::ALL
            .iter()
            .map(|style| segment(*style).label(style.label())),
        [settings.type_style()],
        move |state: &mut AppState, selection: Vec<DemoTypeStyle>| {
            if let Some(style) = selection.into_iter().next() {
                state.settings.set_type_style(style, brightness);
            }
        },
    ));

    any(column()
        .child(text("Type").style(title_style))
        .child(SizedBox::<AppState>(None, Some(4.0)))
        .child(text("Family applies to every role.").style(detail_style.clone()))
        .child(SizedBox::<AppState>(None, Some(16.0)))
        .child(font_picker)
        .child(SizedBox::<AppState>(None, Some(24.0)))
        .child(text("Style").style(small_title_style))
        .child(SizedBox::<AppState>(None, Some(4.0)))
        .child(text("Regular, or the M3 Expressive emphasized scale.").style(detail_style))
        .child(SizedBox::<AppState>(None, Some(16.0)))
        .child(style_picker))
}

/// Chunk `items` into [`SEED_ROW_COLUMNS`]-wide rows, `SEED_GAP`px apart in
/// both directions — see [`SEED_ROW_COLUMNS`]'s own doc comment for the
/// row-width arithmetic (G10). The same "chunk a fixed-size tile grid" idiom
/// `crate::pages::playground::view::shapes`'s `catalog_rows` uses (`drain`
/// rather than `chunks`, since `AnyView` — already-built views, not the raw
/// data `shapes` chunks — isn't `Clone`).
fn swatch_grid<State: 'static>(mut items: Vec<AnyView<State>>) -> AnyView<State> {
    let mut rows: Vec<AnyView<State>> = Vec::new();
    while !items.is_empty() {
        if !rows.is_empty() {
            rows.push(any(SizedBox::<State>(None, Some(SEED_GAP))));
        }
        let take = SEED_ROW_COLUMNS.min(items.len());
        let mut cells: Vec<AnyView<State>> = Vec::with_capacity(take * 2);
        for item in items.drain(..take) {
            if !cells.is_empty() {
                cells.push(any(SizedBox::<State>(Some(SEED_GAP), None)));
            }
            cells.push(item);
        }
        rows.push(any(Row(cells)));
    }
    any(Column(rows))
}

#[cfg(test)]
mod tests {
    use frust::Brightness;
    use frust_material::theme_from_seed;

    use super::body;
    use crate::theme::{DemoFont, DemoTypeStyle, SEED_OPTIONS, ThemeSettings};

    /// Every state this file's own pickers can reach still builds a page —
    /// the swatch/segment lookups this module computes from
    /// [`ThemeSettings`] never index out of bounds or under-fill a
    /// segmented button's `2..=5` bound.
    #[test]
    fn the_page_builds_across_every_reachable_control_state() {
        let settings = ThemeSettings::new();
        for index in 0..SEED_OPTIONS.len() {
            settings.set_seed(index, Brightness::Light);
            let theme = theme_from_seed(settings.seed_color(), Brightness::Light);
            let _view = body(&theme, settings);
        }
        for font in DemoFont::ALL {
            settings.set_font(font, Brightness::Light);
            let theme = theme_from_seed(settings.seed_color(), Brightness::Light);
            let _view = body(&theme, settings);
        }
        for style in DemoTypeStyle::ALL {
            settings.set_type_style(style, Brightness::Light);
            let theme = theme_from_seed(settings.seed_color(), Brightness::Light);
            let _view = body(&theme, settings);
        }
        for auto in [true, false] {
            settings.set_auto_theming(auto, Brightness::Light);
            let theme = theme_from_seed(settings.seed_color(), Brightness::Light);
            let _view = body(&theme, settings);
        }
    }

    /// A seed change resolves to a different scheme (the model-level
    /// contract [`crate::theme::settings`] already tests directly) — this
    /// re-check is scoped to what *this file* adds: the swatch grid reads
    /// [`ThemeSettings::seed_index`] to decide which ring is selected, so a
    /// selection that didn't move would be this file's own bug, not the
    /// model's.
    #[test]
    fn a_seed_change_moves_the_selected_swatch() {
        let settings = ThemeSettings::new();
        assert_eq!(settings.seed_index(), 0);
        settings.set_seed(2, Brightness::Light);
        assert_eq!(settings.seed_index(), 2);
    }

    /// G10 regression: [`super::seeds`]'s swatch grid must never paint past
    /// this page's real inner width at a phone-ish device width — the same
    /// paint-extent idiom `crate::pages::playground::find::progress`'s own
    /// G10 test uses.
    ///
    /// Reverting the fix (back to a `spaced_row` of all five swatches) fails
    /// this test: `5 * 56 + 4 * 16 = 344` already exceeds this page's own
    /// ~328px inner width.
    #[test]
    fn seeds_never_paints_past_the_pages_inner_width() {
        use std::any::Any;

        use frust::authoring::Point;
        use frust::authoring::text::TextContext;
        use frust::authoring::{
            BoxConstraints, BuildCtx, LayoutCtx, PaintCtx, PaintScene, Size, View, Widget,
        };
        use frust::peniko::Color;

        const DEVICE_WIDTH: f64 = 360.0;
        let page_inner_width = DEVICE_WIDTH - 2.0 * 16.0;

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

        let settings = ThemeSettings::new();
        let theme = theme_from_seed(settings.seed_color(), Brightness::Light);
        let view = super::seeds(&theme, settings);
        let mut counter = 0u64;
        let mut widget = view.build(&mut BuildCtx::new(&mut counter));
        let bc = BoxConstraints::new(Size::ZERO, Size::new(page_inner_width, 400.0));
        let mut text_ctx = TextContext::new();
        let mut layout_ctx = LayoutCtx::with_resources(
            Some(&mut text_ctx as &mut dyn Any),
            Some(&theme as &dyn Any),
        );
        let size = widget.layout(&mut layout_ctx, &bc);
        assert!(
            size.width <= page_inner_width + 1e-6,
            "the swatch grid's own reported size {size:?} must not exceed \
             the page's inner width {page_inner_width}"
        );

        let mut ctx = PaintCtx::new(Point::ZERO, size).with_theme(&theme);
        let mut scene = MaxXScene::default();
        widget.paint(&mut ctx, &mut scene);
        assert!(
            scene.max_x <= page_inner_width + 1e-6,
            "G10 regressed: painted x {} exceeds the page's inner width {} \
             (reported size {size:?})",
            scene.max_x,
            page_inner_width
        );
    }
}
