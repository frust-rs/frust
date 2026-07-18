//! Widgets · AppBars screen — REAL (ported from `examples/catalog` in task 03).
//!
//! The catalog's AppBars tab: an embedded Material [`app_bar`] and a Cupertino
//! [`cupertino_nav_bar`] side by side, so both top-bar styles are visible
//! regardless of the live design language. The Material title uses M3
//! Expressive's `titleLarge`-emphasized token (task 6f-10).
//!
//! # Shape
//!
//! This exhibit is design-agnostic (it shows both bar styles unconditionally)
//! and holds no mutable state, so — unlike the Controls/Cards/Modals screens —
//! it needs no nested [`Component`]: it renders directly into the shell's
//! [`ShellState`] tree.

use forgekit::{
    AnyView, Column, FlexView, SizedBox, any, app_bar, cupertino_nav_bar, scroll_view, text,
};

use crate::ShellState;

/// The AppBars exhibit: a Material [`app_bar`] and a Cupertino
/// [`cupertino_nav_bar`] side by side.
pub fn appbars_screen() -> AnyView<ShellState> {
    any(scroll_view(appbars_column()))
}

/// The scrolling AppBars exhibit column.
fn appbars_column() -> FlexView<ShellState> {
    Column(vec![
        any(text("AppBars").size(24.0)),
        any(text(
            "Emphasized type specimen (task 6f-10): the title below uses M3 \
             Expressive's titleLarge-emphasized token (Medium weight) instead \
             of the plain baseline titleLarge.",
        )
        .size(12.0)),
        any(text("Material top AppBar (small, center-aligned):").size(14.0)),
        any(app_bar::<ShellState>("Section header")),
        any(SizedBox(None, Some(16.0))),
        any(text("Cupertino top NavBar:").size(14.0)),
        any(cupertino_nav_bar::<ShellState>("Section header")),
    ])
}
