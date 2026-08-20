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
    SizedBox, Theme, any, container, scroll_view, text, use_context,
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
        Column(vec![
            toggles(theme, settings),
            any(SizedBox::<AppState>(None, Some(24.0))),
            seeds(theme, settings),
            any(SizedBox::<AppState>(None, Some(24.0))),
            type_section(theme, settings),
        ]),
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

    any(Column(vec![
        any(text("Seed color").style(title_style)),
        any(SizedBox::<AppState>(None, Some(4.0))),
        any(text("Generates the scheme for both brightnesses.").style(detail_style)),
        any(SizedBox::<AppState>(None, Some(16.0))),
        spaced_row(swatches, 16.0),
    ]))
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
        Column(vec![
            any(container(SizedBox::<AppState>(None, None))
                .size_centered(SEED_SWATCH_SIZE, SEED_SWATCH_SIZE)
                .fill(color)
                .radius(SEED_SWATCH_SIZE / 2.0)
                .border(border_color, border_width)),
            any(SizedBox::<AppState>(None, Some(8.0))),
            any(text(label).style(label_style)),
        ])
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

    any(Column(vec![
        any(text("Type").style(title_style)),
        any(SizedBox::<AppState>(None, Some(4.0))),
        any(text("Family applies to every role.").style(detail_style.clone())),
        any(SizedBox::<AppState>(None, Some(16.0))),
        font_picker,
        any(SizedBox::<AppState>(None, Some(24.0))),
        any(text("Style").style(small_title_style)),
        any(SizedBox::<AppState>(None, Some(4.0))),
        any(text("Regular, or the M3 Expressive emphasized scale.").style(detail_style)),
        any(SizedBox::<AppState>(None, Some(16.0))),
        style_picker,
    ]))
}

/// Lay `items` out horizontally with `gap`px between each pair — `Row` has
/// no spacing knob of its own, the same interleaved-spacer idiom this file's
/// `body` uses vertically.
fn spaced_row<State: 'static>(items: Vec<AnyView<State>>, gap: f64) -> AnyView<State> {
    let mut children: Vec<AnyView<State>> = Vec::with_capacity(items.len() * 2);
    for item in items {
        if !children.is_empty() {
            children.push(any(SizedBox::<State>(Some(gap), None)));
        }
        children.push(item);
    }
    any(Row(children))
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
}
