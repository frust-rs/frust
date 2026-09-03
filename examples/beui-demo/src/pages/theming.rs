//! Theming: the beUI palette across both brightnesses, a live light/dark
//! toggle wired through `frust_beui`'s own [`theme_toggle`], and a typography
//! specimen. The motion-token visualizer is a stub: b-2x wires a real
//! timeline against `frust_beui::tokens::motion`'s curves and springs — this
//! page only names the surface.

use frust::{Color, Column, CrossAxisAlignment, Row, SizedBox, View, any, colored_box, text};
use frust_beui::components::theme_toggle::theme_toggle;
use frust_beui::{BEUI_DARK, BEUI_LIGHT, BeuiPalette};

use crate::AppState;
use crate::nav::{caption, heading};

/// Theming carries no interactive state of its own today: brightness lives in
/// the process-global override [`theme_toggle`] drives through
/// `frust::set_app_theme` — the same mechanism the top bar's own copy of the
/// toggle uses.
#[derive(Default)]
pub struct State;

/// One named swatch: a tile painted in `color` beside its token role name.
fn swatch(name: &str, color: Color) -> frust::AnyView<AppState> {
    any(Row(vec![
        any(colored_box().fill(color).radius(6.0).size(28.0, 28.0)),
        any(SizedBox(Some(8.0), None)),
        any(text(name.to_string()).size(13.0)),
    ])
    .cross_axis(CrossAxisAlignment::Center))
}

/// One brightness's whole swatch column.
fn swatch_column(title: &str, palette: BeuiPalette) -> frust::AnyView<AppState> {
    any(Column(vec![
        any(text(title.to_string()).size(14.0)),
        any(SizedBox(None, Some(8.0))),
        swatch("background", palette.background),
        any(SizedBox(None, Some(6.0))),
        swatch("foreground", palette.foreground),
        any(SizedBox(None, Some(6.0))),
        swatch("card", palette.card),
        any(SizedBox(None, Some(6.0))),
        swatch("accent", palette.accent),
        any(SizedBox(None, Some(6.0))),
        swatch("danger", palette.danger),
        any(SizedBox(None, Some(6.0))),
        swatch("success", palette.success),
    ]))
}

pub fn page(_state: &mut State) -> impl View<AppState> + use<> {
    Column(vec![
        any(heading("Theming")),
        any(SizedBox(None, Some(8.0))),
        any(caption(
            "frust_beui::install() seeds the beUI theme as this app's default; \
             the toggle below flips brightness through frust::set_app_theme — \
             the same override the top bar's own toggle drives.",
        )),
        any(SizedBox(None, Some(16.0))),
        any(theme_toggle::<AppState>()),
        any(SizedBox(None, Some(24.0))),
        any(text("Palette").size(16.0)),
        any(SizedBox(None, Some(12.0))),
        any(Row(vec![
            swatch_column("Light", BEUI_LIGHT),
            any(SizedBox(Some(32.0), None)),
            swatch_column("Dark", BEUI_DARK),
        ])),
        any(SizedBox(None, Some(24.0))),
        any(text("Typography").size(16.0)),
        any(SizedBox(None, Some(12.0))),
        any(
            text("Geist Variable — the sans stack every beUI text role resolves to.")
                .size(16.0)
                .family(frust_beui::tokens::sans_family()),
        ),
        any(SizedBox(None, Some(8.0))),
        any(
            text("Geist Mono Variable — the mono stack code and kbd rows resolve to.")
                .size(14.0)
                .family(frust_beui::tokens::mono_family()),
        ),
        any(SizedBox(None, Some(24.0))),
        any(text("Motion tokens").size(16.0)),
        any(SizedBox(None, Some(8.0))),
        any(caption(
            "TODO: a future task wires a live visualizer against \
             frust_beui::tokens::motion's curves and springs; this scaffold \
             only names the token surface.",
        )),
    ])
}
