//! Showcase · menu screen — REAL (shipped in task 02).
//!
//! The `/showcase` tab's landing page: a list of links into the theme, motion,
//! notes, and navigation-playground screens. Each link pushes its target
//! through the [`NavigatorController`] the route table cloned in (a within-page
//! push, exactly as `examples/navdemo`'s pages navigate — the router path is
//! reserved for the tab bar and deep links, where the live state / router is in
//! scope). Back-navigation is the navigator's own pop (or the iOS swipe-back);
//! the persistent bottom bar is always available to jump to another tab.

use forgekit::{AnyView, Button, Column, NavigatorController, SizedBox, any, text};

use crate::ShellSignals;
use crate::ShellState;
use crate::screens;

/// The showcase menu. `controller` pushes each sub-screen; `signals` is threaded
/// into the nav playground (its switcher/result banner read the shell bundle).
pub fn showcase_menu(
    controller: NavigatorController<ShellState>,
    signals: ShellSignals,
) -> AnyView<ShellState> {
    let theme_controller = controller.clone();
    let motion_controller = controller.clone();
    let notes_controller = controller.clone();
    let nav_controller = controller;

    any(Column(vec![
        any(text("Showcase").size(28.0)),
        any(text("Explore theme, motion, notes, and the navigation playground.").size(14.0)),
        any(SizedBox(None, Some(12.0))),
        any(Button("Theme \u{2192}", move |_s: &mut ShellState| {
            theme_controller.push(screens::theme::theme_screen);
        })),
        any(SizedBox(None, Some(8.0))),
        any(Button("Motion \u{2192}", move |_s: &mut ShellState| {
            motion_controller.push(screens::motion::motion_screen);
        })),
        any(SizedBox(None, Some(8.0))),
        any(Button("Notes \u{2192}", move |_s: &mut ShellState| {
            notes_controller.push(screens::notes::notes_screen);
        })),
        any(SizedBox(None, Some(8.0))),
        any(Button(
            "Navigation playground \u{2192}",
            move |_s: &mut ShellState| {
                let inner = nav_controller.clone();
                nav_controller
                    .push(move || screens::nav_playground::nav_playground(inner.clone(), signals));
            },
        )),
    ]))
}
