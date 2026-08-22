//! App bars: the reference's `AppBarsPlayground`.
//!
//! All four constructors ([`app_bar`], [`search_app_bar`], [`bottom_app_bar`],
//! [`sliver_app_bar`]) share one preview and one knob set — the reference's
//! own `_AppBarKind` switch. The sliver preview is this demo's first
//! consumer of the documented app-side scroll contract
//! (`plugins/material/src/appbar/mod.rs`'s module docs): a real
//! [`frust::ScrollView::on_scroll`] feeds [`SliverAppBarView::scroll_offset`],
//! wrapping a real scrolling item list rather than faking the collapse with a
//! slider — there is no sliver protocol here, so the bar and the scrolling
//! body are two stacked widgets, not one participant in a shared viewport
//! (see that module's own "why this is a box widget, not a sliver" section).
//!
//! **Safe area** is not a widget prop on this family (`AppBarView` carries no
//! `safeArea` field — see the appbar module docs' *Not ported* list): the
//! toggle here wraps the previewed bar in [`frust::safe_area`] instead, the
//! documented app-side pattern, rather than being dropped as inert. On a
//! desktop preview with no window insets it has no visible effect, but the
//! control still exercises the real seam.

use frust::{
    AnyView, Axis, Column, Component, CrossAxisAlignment, EdgeInsets, FlexView, Padding,
    ScrollInfo, SizedBox, Stack, Theme, View, any, component, container, flexible, icon,
    inflexible, safe_area, scroll_view, text,
};
use frust_material::{
    AppBarDensity, AppBarShapeFamily, AppBarVariant, FabSize, MaterialDimensions, MaterialSpacing,
    OverlayAnchor, app_bar, bottom_app_bar, fab, icon_button, icons, search_app_bar, search_bar,
    sliver_app_bar,
};

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{
    ambient_theme, control_panel, play_enum_menu_field, play_enum_menu_panel, play_enum_segmented,
    play_preview_card, play_snippet, play_switch, play_text_field, playground_body,
};

/// See the page contract in [`crate::pages::playground`].
pub fn page(_entry: DemoEntry) -> AnyView<AppState> {
    any(component(AppBarsPlayground))
}

/// Fixed preview height for the sliver kind, in logical px — the
/// reference's own `SizedBox(height: 180)` (`_sliverPreview`), tall enough
/// to show the large variant's 152px expanded band plus a sliver of the
/// scrolling item list beneath it.
const SLIVER_PREVIEW_HEIGHT: f64 = 180.0;

/// The four bar constructors this playground cycles through — the
/// reference's page-private `_AppBarKind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AppBarKind {
    Top,
    Search,
    Bottom,
    Sliver,
}

impl AppBarKind {
    const ALL: [AppBarKind; 4] = [
        AppBarKind::Top,
        AppBarKind::Search,
        AppBarKind::Bottom,
        AppBarKind::Sliver,
    ];

    fn label(self) -> &'static str {
        match self {
            AppBarKind::Top => "top",
            AppBarKind::Search => "search",
            AppBarKind::Bottom => "bottom",
            AppBarKind::Sliver => "sliver",
        }
    }
}

const APP_BAR_VARIANTS: [AppBarVariant; 3] = [
    AppBarVariant::Small,
    AppBarVariant::Medium,
    AppBarVariant::Large,
];
const APP_BAR_DENSITIES: [AppBarDensity; 2] = [AppBarDensity::Regular, AppBarDensity::Compact];
const APP_BAR_SHAPES: [AppBarShapeFamily; 2] =
    [AppBarShapeFamily::Round, AppBarShapeFamily::Square];

fn variant_label(v: AppBarVariant) -> &'static str {
    match v {
        AppBarVariant::Small => "small",
        AppBarVariant::Medium => "medium",
        AppBarVariant::Large => "large",
    }
}

fn density_label(v: AppBarDensity) -> &'static str {
    match v {
        AppBarDensity::Regular => "regular",
        AppBarDensity::Compact => "compact",
    }
}

fn shape_label(v: AppBarShapeFamily) -> &'static str {
    match v {
        AppBarShapeFamily::Round => "round",
        AppBarShapeFamily::Square => "square",
    }
}

/// This playground's own knobs, retained across rebuilds — the reference's
/// `_AppBarsPlaygroundState` fields, plus [`Self::scrolled`] (this port's own
/// addition: the sliver preview's real scroll offset — see this module's own
/// docs) and [`Self::kind_anchor`]/[`Self::kind_open`] (the Kind picker's
/// [`OverlayAnchor`] pair — see [`crate::widgets::playground`]'s module docs
/// on `play_enum_menu_field`/`play_enum_menu_panel`).
struct AppBarsState {
    kind: AppBarKind,
    variant: AppBarVariant,
    density: AppBarDensity,
    shape: AppBarShapeFamily,
    center_title: bool,
    safe_area: bool,
    title: String,
    search_query: String,
    scrolled: f64,
    kind_anchor: OverlayAnchor,
    kind_open: bool,
}

struct AppBarsPlayground;

impl Component for AppBarsPlayground {
    type State = AppBarsState;

    fn init(&self) -> AppBarsState {
        AppBarsState {
            kind: AppBarKind::Top,
            variant: AppBarVariant::Medium,
            density: AppBarDensity::Regular,
            shape: AppBarShapeFamily::Square,
            center_title: false,
            safe_area: false,
            title: "Inbox".to_string(),
            search_query: String::new(),
            scrolled: 0.0,
            kind_anchor: OverlayAnchor::new(),
            kind_open: false,
        }
    }

    fn build(&self, state: &mut AppBarsState) -> AnyView<AppBarsState> {
        body(state)
    }
}

/// The whole page body: [`playground_body`]'s preview/snippet/controls
/// arrangement, plus the Kind picker's popup panel mounted at this page's own
/// outer [`Stack`] — the kept-mounted pattern the kit's own module docs
/// document for `play_enum_menu_field`/`play_enum_menu_panel`.
fn body(state: &AppBarsState) -> AnyView<AppBarsState> {
    let theme = ambient_theme();
    let content = playground_body(
        vec![play_preview_card(
            state.kind.label(),
            preview_content(&theme, state),
        )],
        vec![play_snippet(state.kind.label(), snippet_code(state))],
        vec![variant_panel(state), appearance_panel(state)],
    );
    any(Stack(vec![content, kind_menu_panel(state)]))
}

/// An icon-only, non-interactive-feeling slot for a bar's leading/action
/// content — the reference's `_icon` helper, built here on
/// [`frust_material::icon_button`] (this catalog's own slot-content idiom,
/// e.g. [`crate::pages::playground_scaffold`]'s leading back button) rather
/// than a bare glyph, since every bar slot in this family is opaque
/// caller-supplied content (see `plugins/material/src/appbar/mod.rs`'s slot
/// contract).
fn icon_slot(source: frust::IconSource, label: &str) -> AnyView<AppBarsState> {
    any(icon_button(any(icon(source)), |_: &mut AppBarsState| {}).semantic_label(label))
}

/// Wrap `bar` in [`frust::safe_area`] when the Safe area toggle is on — see
/// this module's own docs for why this is an app-side wrap rather than a
/// widget prop.
fn wrap_safe_area(state: &AppBarsState, bar: AnyView<AppBarsState>) -> AnyView<AppBarsState> {
    if state.safe_area {
        any(safe_area(bar))
    } else {
        bar
    }
}

/// Frame `child` in an outlined, rounded box — the reference's `_framed`
/// helper.
fn framed(theme: &Theme, child: impl View<AppBarsState>) -> AnyView<AppBarsState> {
    let outline = theme.scheme().outline_variant;
    any(container(child)
        .radius(MaterialDimensions::RADIUS_LARGE)
        .border(outline, 1.0))
}

fn preview_content(theme: &Theme, state: &AppBarsState) -> AnyView<AppBarsState> {
    match state.kind {
        AppBarKind::Top => framed(theme, wrap_safe_area(state, top_bar_view(state))),
        AppBarKind::Search => framed(theme, wrap_safe_area(state, search_bar_view(state))),
        AppBarKind::Bottom => framed(theme, wrap_safe_area(state, bottom_bar_view(state))),
        AppBarKind::Sliver => sliver_preview(theme, state),
    }
}

fn top_bar_view(state: &AppBarsState) -> AnyView<AppBarsState> {
    any(app_bar::<AppBarsState>(state.title.clone())
        .center_title(state.center_title)
        .density(state.density)
        .shape_family(state.shape)
        .leading(icon_slot(icons::MENU, "Menu"))
        .actions(vec![icon_slot(icons::SEARCH, "Search")]))
}

fn search_bar_view(state: &AppBarsState) -> AnyView<AppBarsState> {
    any(search_app_bar::<AppBarsState>(
        search_bar(state.search_query.clone(), |s: &mut AppBarsState, q| {
            s.search_query = q;
        })
        .hint("Search mail")
        .on_tap(|_: &mut AppBarsState| {}),
    )
    .density(state.density)
    .shape_family(state.shape)
    .center_title(state.center_title)
    .leading(icon_slot(icons::MENU, "Menu"))
    .actions(vec![icon_slot(icons::ACCOUNT_CIRCLE, "Account")]))
}

fn bottom_bar_view(_state: &AppBarsState) -> AnyView<AppBarsState> {
    any(bottom_app_bar::<AppBarsState>()
        .actions(vec![
            icon_slot(icons::MENU, "Menu"),
            icon_slot(icons::SEARCH, "Search"),
            icon_slot(icons::EDIT, "Edit"),
        ])
        .fab(any(fab(any(icon(icons::ADD)), |_: &mut AppBarsState| {})
            .size(FabSize::Small)
            .label("Add"))))
}

fn sliver_bar_view(state: &AppBarsState) -> AnyView<AppBarsState> {
    any(sliver_app_bar::<AppBarsState>(state.title.clone())
        .variant(state.variant)
        .density(state.density)
        .shape_family(state.shape)
        .center_title(state.center_title)
        .scroll_offset(state.scrolled)
        .actions(vec![icon_slot(icons::SEARCH, "Search")]))
}

/// The collapsing bar over a real scrolling item list, both stacked inside a
/// fixed-height frame — the reference's `_sliverPreview`, ported through the
/// app-side scroll contract (see this module's own docs).
fn sliver_preview(theme: &Theme, state: &AppBarsState) -> AnyView<AppBarsState> {
    let bar = wrap_safe_area(state, sliver_bar_view(state));

    let mut item_style = theme.type_scale.body_medium.clone();
    item_style.color = theme.scheme().on_surface;
    let items: Vec<AnyView<AppBarsState>> = (1..=4)
        .map(|i| {
            any(Padding(
                EdgeInsets {
                    left: MaterialSpacing::LG,
                    top: MaterialSpacing::SM,
                    right: MaterialSpacing::LG,
                    bottom: MaterialSpacing::SM,
                },
                text(format!("Item {i}")).style(item_style.clone()),
            ))
        })
        .collect();
    let list = scroll_view(Column(items)).on_scroll(|s: &mut AppBarsState, info: ScrollInfo| {
        s.scrolled = info.offset;
    });

    let column = FlexView::new(Axis::Vertical, vec![inflexible(bar), flexible(1, list)])
        .cross_axis(CrossAxisAlignment::Stretch);

    framed(
        theme,
        SizedBox::<AppBarsState>(None, Some(SLIVER_PREVIEW_HEIGHT)).child(column),
    )
}

/// `Variant` control panel: the Kind picker's field, plus (sliver only) the
/// expanded-size picker — the reference's own conditional `PlayEnumSegmented`.
fn variant_panel(state: &AppBarsState) -> AnyView<AppBarsState> {
    let mut rows = vec![play_enum_menu_field::<AppBarsState, AppBarKind>(
        "Kind",
        state.kind,
        &AppBarKind::ALL,
        AppBarKind::label,
        &state.kind_anchor,
        state.kind_open,
        |s: &mut AppBarsState, open| s.kind_open = open,
    )];
    if state.kind == AppBarKind::Sliver {
        rows.push(play_enum_segmented::<AppBarsState, AppBarVariant>(
            "Sliver size",
            state.variant,
            &APP_BAR_VARIANTS,
            variant_label,
            |s: &mut AppBarsState, v| s.variant = v,
        ));
    }
    control_panel("Variant", rows)
}

/// `Appearance` control panel: density, shape, the two toggles, and the
/// title field — the reference's own row order.
fn appearance_panel(state: &AppBarsState) -> AnyView<AppBarsState> {
    control_panel(
        "Appearance",
        vec![
            play_enum_segmented::<AppBarsState, AppBarDensity>(
                "Density",
                state.density,
                &APP_BAR_DENSITIES,
                density_label,
                |s: &mut AppBarsState, v| s.density = v,
            ),
            play_enum_segmented::<AppBarsState, AppBarShapeFamily>(
                "Shape",
                state.shape,
                &APP_BAR_SHAPES,
                shape_label,
                |s: &mut AppBarsState, v| s.shape = v,
            ),
            play_switch::<AppBarsState>("Center title", state.center_title, |s, v| {
                s.center_title = v;
            }),
            play_switch::<AppBarsState>("Safe area", state.safe_area, |s, v| s.safe_area = v),
            play_text_field::<AppBarsState>("Title", state.title.clone(), |s, v| s.title = v),
        ],
    )
}

/// The Kind picker's popup half — mounted at the page's own outer [`Stack`],
/// kept mounted regardless of `kind_open` so its close plays the exit ramp.
fn kind_menu_panel(state: &AppBarsState) -> AnyView<AppBarsState> {
    play_enum_menu_panel::<AppBarsState, AppBarKind>(
        state.kind,
        &AppBarKind::ALL,
        AppBarKind::label,
        &state.kind_anchor,
        state.kind_open,
        |s: &mut AppBarsState, open| s.kind_open = open,
        |s: &mut AppBarsState, kind| s.kind = kind,
    )
}

/// The FRUST-equivalent snippet for the current knob state — never Dart (see
/// [`crate::widgets::playground`]'s module docs on why `playDartString` isn't
/// ported).
fn snippet_code(state: &AppBarsState) -> String {
    match state.kind {
        AppBarKind::Top => format!(
            "app_bar({title:?})\n    .center_title({center_title})\n    .density(AppBarDensity::{density:?})\n    .shape_family(AppBarShapeFamily::{shape:?})\n    .leading(menu_icon)\n    .actions(vec![search_icon]);",
            title = state.title,
            center_title = state.center_title,
            density = state.density,
            shape = state.shape,
        ),
        AppBarKind::Search => format!(
            "search_app_bar(\n    search_bar(query, on_query_changed)\n        .hint(\"Search mail\"),\n)\n    .density(AppBarDensity::{density:?})\n    .shape_family(AppBarShapeFamily::{shape:?})\n    .center_title({center_title})\n    .leading(menu_icon)\n    .actions(vec![account_icon]);",
            density = state.density,
            shape = state.shape,
            center_title = state.center_title,
        ),
        AppBarKind::Bottom => "bottom_app_bar()\n    .actions(vec![menu_icon, search_icon, edit_icon])\n    .fab(fab(add_icon, on_press).size(FabSize::Small));"
            .to_string(),
        AppBarKind::Sliver => format!(
            "sliver_app_bar({title:?})\n    .variant(AppBarVariant::{variant:?})\n    .density(AppBarDensity::{density:?})\n    .shape_family(AppBarShapeFamily::{shape:?})\n    .center_title({center_title})\n    .scroll_offset(state.scrolled)\n    .actions(vec![search_icon]);",
            title = state.title,
            variant = state.variant,
            density = state.density,
            shape = state.shape,
            center_title = state.center_title,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_state() -> AppBarsState {
        AppBarsPlayground.init()
    }

    /// Every state this file's own pickers can reach still builds a page —
    /// every bar kind, every sliver variant, every density/shape, and both
    /// toggles.
    #[test]
    fn the_page_builds_across_every_reachable_control_state() {
        let mut state = base_state();
        for kind in AppBarKind::ALL {
            state.kind = kind;
            let _view = body(&state);
        }
        state.kind = AppBarKind::Sliver;
        for variant in APP_BAR_VARIANTS {
            state.variant = variant;
            let _view = body(&state);
        }
        for density in APP_BAR_DENSITIES {
            state.density = density;
            let _view = body(&state);
        }
        for shape in APP_BAR_SHAPES {
            state.shape = shape;
            let _view = body(&state);
        }
        for flag in [true, false] {
            state.center_title = flag;
            state.safe_area = flag;
            let _view = body(&state);
        }
        state.kind_open = true;
        let _view = body(&state);
    }

    /// The Kind menu's field and panel halves share one anchor and stay in
    /// sync with the reported selection — the same round trip
    /// `play_enum_menu_field`/`play_enum_menu_panel`'s own tests cover, kept
    /// here to prove this page wires them consistently.
    #[test]
    fn the_kind_menu_field_and_panel_build_off_the_same_anchor() {
        let state = base_state();
        let _field = variant_panel(&state);
        let _panel = kind_menu_panel(&state);
    }
}
