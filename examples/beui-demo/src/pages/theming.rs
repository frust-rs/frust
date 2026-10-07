//! Theming: the beUI palette across both brightnesses, a live light/dark
//! toggle wired through `frust_beui`'s own [`theme_toggle`], and a typography
//! specimen. The motion-token visualizer is a stub: a later pass wires a real
//! timeline against `frust_beui::tokens::motion`'s curves and springs — this
//! page only names the surface.

use frust::{Color, CrossAxisAlignment, SizedBox, View, colored_box, column, row, text};
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
fn swatch(name: &str, color: Color) -> impl frust::View<AppState> {
    row()
        .child(colored_box().fill(color).radius(6.0).size(28.0, 28.0))
        .child(SizedBox(Some(8.0), None))
        .child(text(name.to_string()).size(13.0))
        .cross_axis(CrossAxisAlignment::Center)
}

/// One brightness's whole swatch column.
fn swatch_column(title: &str, palette: BeuiPalette) -> impl frust::View<AppState> {
    column()
        .child(text(title.to_string()).size(14.0))
        .child(SizedBox(None, Some(8.0)))
        .child(swatch("background", palette.background))
        .child(SizedBox(None, Some(6.0)))
        .child(swatch("foreground", palette.foreground))
        .child(SizedBox(None, Some(6.0)))
        .child(swatch("card", palette.card))
        .child(SizedBox(None, Some(6.0)))
        .child(swatch("accent", palette.accent))
        .child(SizedBox(None, Some(6.0)))
        .child(swatch("danger", palette.danger))
        .child(SizedBox(None, Some(6.0)))
        .child(swatch("success", palette.success))
}

pub fn page(_state: &mut State) -> impl View<AppState> + use<> {
    column()
        .child(heading("Theming"))
        .child(SizedBox(None, Some(8.0)))
        .child(caption(
            "frust_beui::install() seeds the beUI theme as this app's default; \
             the toggle below flips brightness through frust::set_app_theme — \
             the same override the top bar's own toggle drives.",
        ))
        .child(SizedBox(None, Some(16.0)))
        .child(theme_toggle::<AppState>())
        .child(SizedBox(None, Some(24.0)))
        .child(text("Palette").size(16.0))
        .child(SizedBox(None, Some(12.0)))
        .child(
            row()
                .child(swatch_column("Light", BEUI_LIGHT))
                .child(SizedBox(Some(32.0), None))
                .child(swatch_column("Dark", BEUI_DARK)),
        )
        .child(SizedBox(None, Some(24.0)))
        .child(text("Typography").size(16.0))
        .child(SizedBox(None, Some(12.0)))
        .child(
            text("Geist Variable — the sans stack every beUI text role resolves to.")
                .size(16.0)
                .family(frust_beui::tokens::sans_family()),
        )
        .child(SizedBox(None, Some(8.0)))
        .child(
            text("Geist Mono Variable — the mono stack code and kbd rows resolve to.")
                .size(14.0)
                .family(frust_beui::tokens::mono_family()),
        )
        .child(SizedBox(None, Some(24.0)))
        .child(text("Motion tokens").size(16.0))
        .child(SizedBox(None, Some(8.0)))
        .child(caption(
            "TODO: a future task wires a live visualizer against \
             frust_beui::tokens::motion's curves and springs; this scaffold \
             only names the token surface.",
        ))
}
