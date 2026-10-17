//! Selection-toolbar publish and generation tests for RenderRoot.

use super::*;

// Selection toolbar: publish, generation, clear.

/// A request with the menu up, anchored at `x` — the shape a field
/// publishes while its bar stands.
fn toolbar_request(x: f64) -> crate::selection_toolbar::SelectionToolbarRequest {
    crate::selection_toolbar::SelectionToolbarRequest {
        anchor: Rect::new(x, 100.0, x + 50.0, 120.0),
        actions: crate::selection_toolbar::SelectionToolbarActions {
            copy: true,
            cut: true,
            paste: false,
            select_all: true,
        },
        present_menu: true,
    }
}

/// The same request with no menu wanted — what a focused field publishes
/// with nothing on screen, so the platform can still answer "may I offer
/// Copy?" for a hardware shortcut.
fn toolbar_level(x: f64) -> crate::selection_toolbar::SelectionToolbarRequest {
    crate::selection_toolbar::SelectionToolbarRequest {
        present_menu: false,
        ..toolbar_request(x)
    }
}

#[test]
fn a_selection_toolbar_publish_resolves_and_only_a_menu_edge_moves_the_generation() {
    let mut h = OverlayHarness::new(vec![]);
    // A toolbar describes the FOCUSED field's selection, so open a session
    // first — a publish with nothing focused describes nothing.
    h.down(30.0, 120.0);
    assert!(h.root.is_focus_active());

    h.state.publish = Some(toolbar_request(10.0));
    h.frame();
    assert_eq!(h.root.selection_toolbar(), Some(toolbar_request(10.0)));
    let first_gen = h.root.selection_toolbar_generation();
    assert!(first_gen > 0, "appearing is an edge");

    // The field republishes the same request every frame its selection
    // stands: that must not ask the shell to re-present the menu per vsync.
    h.frame();
    h.frame();
    assert_eq!(h.root.selection_toolbar(), Some(toolbar_request(10.0)));
    assert_eq!(h.root.selection_toolbar_generation(), first_gen);

    // A moved anchor is stored (a shell re-reads it to place a shown menu) but is NOT
    // a menu edge: it moves on every touch sample of a selection drag.
    h.state.publish = Some(toolbar_request(60.0));
    h.frame();
    assert_eq!(h.root.selection_toolbar(), Some(toolbar_request(60.0)));
    assert_eq!(
        h.root.selection_toolbar_generation(),
        first_gen,
        "an anchor following the selection is not a reason to re-present"
    );

    // A verb changing IS: the menu's own contents just changed.
    let mut fewer_verbs = toolbar_request(60.0);
    fewer_verbs.actions.select_all = false;
    h.state.publish = Some(fewer_verbs);
    h.frame();
    assert_eq!(h.root.selection_toolbar_generation(), first_gen + 1);

    // The field blurs: it stops publishing, and the menu goes away with
    // nothing retracted.
    h.state.publish = None;
    h.frame();
    assert_eq!(h.root.selection_toolbar(), None);
    assert_eq!(
        h.root.selection_toolbar_generation(),
        first_gen + 2,
        "going away is an edge too, or a shell never learns to dismiss"
    );

    // ...and staying away is not.
    h.frame();
    assert_eq!(h.root.selection_toolbar_generation(), first_gen + 2);
}

#[test]
fn only_the_menu_flag_going_up_asks_a_shell_to_present() {
    let mut h = OverlayHarness::new(vec![]);
    h.down(30.0, 120.0);
    assert!(h.root.is_focus_active());

    // A focused field with no bar up publishes all the same, so a platform responder
    // chain can answer for a hardware shortcut.
    h.state.publish = Some(toolbar_level(10.0));
    h.frame();
    assert_eq!(h.root.selection_toolbar(), Some(toolbar_level(10.0)));
    let level_gen = h.root.selection_toolbar_generation();

    // Republishing the same level is not an edge, however many frames it
    // stands for.
    h.frame();
    h.frame();
    assert_eq!(h.root.selection_toolbar_generation(), level_gen);

    // The gesture fires and the field asks for a menu: THAT is the edge.
    h.state.publish = Some(toolbar_request(10.0));
    h.frame();
    assert_eq!(
        h.root.selection_toolbar_generation(),
        level_gen + 1,
        "the flag going false to true is what presents the menu"
    );

    // And dropping it again is the dismiss edge, with the field still
    // focused and still publishing its verbs.
    h.state.publish = Some(toolbar_level(10.0));
    h.frame();
    assert!(h.root.selection_toolbar().is_some());
    assert_eq!(h.root.selection_toolbar_generation(), level_gen + 2);
}

#[test]
fn a_pass_that_publishes_nothing_still_moves_the_generation() {
    // A pass nobody published in resolves to `None` (no menu, no verbs) and must
    // differ from whatever stood, so a shell learns to put a presented menu away.
    let mut h = OverlayHarness::new(vec![]);
    h.down(30.0, 120.0);
    h.state.publish = Some(toolbar_request(10.0));
    h.frame();
    let standing = h.root.selection_toolbar_generation();

    h.state.publish = None;
    h.frame();
    assert_eq!(h.root.selection_toolbar(), None);
    assert_eq!(h.root.selection_toolbar_generation(), standing + 1);
}

#[test]
fn a_blur_clears_the_selection_toolbar_on_the_event_pass() {
    let mut h = OverlayHarness::new(vec![]);
    h.down(30.0, 120.0);
    h.state.publish = Some(toolbar_request(10.0));
    h.frame();
    assert!(h.root.selection_toolbar().is_some());
    let gen_before = h.root.selection_toolbar_generation();

    // A press on chrome that claims no focus ends the session — and the menu
    // must go with it immediately, not a frame later.
    h.down(150.0, 30.0);
    assert!(!h.root.is_focus_active());
    assert_eq!(
        h.root.selection_toolbar(),
        None,
        "a toolbar cannot outlive the focus session its selection belonged to"
    );
    assert_eq!(h.root.selection_toolbar_generation(), gen_before + 1);

    // And a field still publishing into the blurred session cannot resurrect
    // it — the same refusal the IME republish makes.
    h.frame();
    assert_eq!(h.root.selection_toolbar(), None);
}
