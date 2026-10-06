//! FAB menu: the reference's `FabMenuPlayground`.
//!
//! **A large descope from the reference, per [`mod@frust_material::fab_menu`]'s
//! own module docs** — this page shows exactly the supported surface rather
//! than faking the rest:
//!
//! - `position`/`size`/`color` **do not exist on this port at all**.
//!   `frust_material::fab_menu` "fills its box constraints... meant to be the
//!   top layer of a full-area `frust::Stack`" and always anchors its trigger
//!   to its own bottom-right corner — "a v1 divergence from upstream, whose
//!   `M3EFabMenu` self-sizes and is externally positioned by the caller". So
//!   none of the reference's three `PlayEnumSegmented` controls
//!   (Position/Size/Color) has anything to drive; they are omitted, not
//!   stubbed.
//! - The reference's "Gradient fill" preview has no port to mirror either —
//!   the module docs' own "Not ported" list names "item elevation... and
//!   outline/gradient fills/foregrounds... a future addition, out of this
//!   task's scope".
//! - What *is* real: the trigger `icon`, the controlled `open` flag, `items`
//!   (icon + label + `on_select`, via [`fab_menu_item`]), and the accessible
//!   `label`. The preview below is a genuinely live, working `fab_menu` —
//!   tapping its trigger opens/closes it and tapping an item fires
//!   `on_select` — so unlike the other three `do_` pages, this page adds no
//!   separate "Controls" panel: there is no supported knob left to expose
//!   one for (the module docs' recommended `icon` swap on open/close is
//!   demonstrated directly in [`preview`] instead).
//!
//! Knobs live in a page-local [`Knobs`], owned by the nested
//! [`FabMenuPlayground`] `Component` (never [`AppState`]) per the page
//! contract in [`crate::pages::playground`].

use frust::{AnyView, Component, SizedBox, View, any, component, icon};
use frust_material::{fab_menu, fab_menu_item, icons};

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{play_preview_card, play_snippet, playground_body};

/// This page's own knob state — held by [`FabMenuPlayground`], never
/// [`AppState`] (the page contract in [`crate::pages::playground`]). The
/// menu's `open` flag is the one real controlled prop this family has (see
/// the [module docs](self)) — driven by tapping the live preview itself, not
/// by a separate control.
#[derive(Default)]
struct Knobs {
    open: bool,
}

/// The "FAB menu" preview: a real, working `fab_menu` inside a 280px-tall box
/// (the reference's own `SizedBox(height: 280)`) — unlike the reference's
/// `M3EFabMenu`, this port fills that box and self-positions its trigger at
/// its own bottom-right corner, so no `Align` wrapper is needed.
fn preview(state: &Knobs) -> AnyView<Knobs> {
    // Swapping the trigger icon on open/close is this port's documented
    // substitute for the reference's `closeIcon`/`expandIcon`/`collapseIcon`
    // quartet (see [module docs](self)).
    let trigger_icon = if state.open { icons::CLOSE } else { icons::ADD };
    let items = vec![
        fab_menu_item(any(icon(icons::IMAGE)), "Image", |_: &mut Knobs| {}),
        fab_menu_item(any(icon(icons::VIDEOCAM)), "Video", |_: &mut Knobs| {}),
        fab_menu_item(any(icon(icons::MIC)), "Audio", |_: &mut Knobs| {}),
    ];
    any(SizedBox::<Knobs>(None, Some(280.0)).child(fab_menu(
        any(icon(trigger_icon)),
        state.open,
        items,
        |state: &mut Knobs| state.open = !state.open,
    )))
}

/// The page body: one live preview, one static snippet (nothing here is
/// knob-driven — see the [module docs](self)), and no controls panel.
fn body(state: &Knobs) -> AnyView<Knobs> {
    playground_body(
        vec![play_preview_card("FAB menu", preview(state))],
        vec![play_snippet(
            "FAB menu",
            "let trigger_icon = if open { icons::CLOSE } else { icons::ADD };\n\
             fab_menu(\n    any(icon(trigger_icon)),\n    open,\n    vec![\n        \
             fab_menu_item(any(icon(icons::IMAGE)), \"Image\", on_select),\n        \
             fab_menu_item(any(icon(icons::VIDEOCAM)), \"Video\", on_select),\n        \
             fab_menu_item(any(icon(icons::MIC)), \"Audio\", on_select),\n    ],\n    \
             |state: &mut State| state.open = !state.open,\n);",
        )],
        vec![],
    )
}

/// This page's knob component — see the [module docs](self).
struct FabMenuPlayground;

impl Component for FabMenuPlayground {
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
    any(component(FabMenuPlayground))
}

#[cfg(test)]
mod tests {
    use super::{Knobs, body};

    #[test]
    fn the_page_builds_closed() {
        let knobs = Knobs::default();
        let _view = body(&knobs);
    }

    #[test]
    fn the_page_builds_open() {
        let knobs = Knobs { open: true };
        let _view = body(&knobs);
    }
}
