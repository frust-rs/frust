use super::*;

// ---------------------------------------------------------------------
// Overlay portal: an owner that floats a pod, a sibling painted after it,
// and a root that routes like any hand-written container.
// ---------------------------------------------------------------------

/// A rect well clear of the sibling (which sits at y >= 100).
fn floating_rect() -> Rect {
    Rect::new(120.0, 10.0, 200.0, 60.0)
}

#[test]
fn a_registered_pod_paints_after_a_later_sibling_at_its_window_rect() {
    let spec = SurfaceSpec::floating("popover", floating_rect());
    let mut h = OverlayHarness::new(vec![spec.clone()]);
    let scene = h.frame();

    assert_eq!(
        scene
            .texts
            .iter()
            .map(|(_, text)| text.as_str())
            .collect::<Vec<_>>(),
        vec!["owner", "sibling", "popover"],
        "the floated pod paints after the owner AND after the sibling painted \
         later than the owner — escaping paint order is the whole point"
    );
    assert_eq!(
        scene.texts[2].0,
        floating_rect().origin(),
        "and it paints at its registered window rect, not at its owner's origin"
    );
    assert_eq!(h.root.overlay_hits.len(), 1);
    assert_eq!(h.root.overlay_hits[0].key, spec.key);
    assert_eq!(h.root.overlay_hits[0].window_rect, floating_rect());
}

#[test]
fn two_bands_paint_floating_then_tooltip_whatever_the_registration_order() {
    // Registered tooltip-first, so registration order and band order
    // disagree: the band must win.
    let mut tooltip = SurfaceSpec::floating("tooltip", Rect::new(0.0, 0.0, 40.0, 20.0));
    tooltip.band = crate::overlay::OverlayBand::Tooltip;
    tooltip.input = OverlayInput::Transparent;
    let floating = SurfaceSpec::floating("floating", floating_rect());
    let mut h = OverlayHarness::new(vec![tooltip, floating]);
    let scene = h.frame();

    assert_eq!(
        scene
            .texts
            .iter()
            .map(|(_, text)| text.as_str())
            .collect::<Vec<_>>(),
        vec!["owner", "sibling", "floating", "tooltip"],
        "Floating paints below Tooltip regardless of who registered first"
    );
}

#[test]
fn the_registry_is_empty_at_the_start_of_every_paint() {
    let spec = SurfaceSpec::floating("popover", floating_rect());
    let mut h = OverlayHarness::new(vec![spec.clone()]);
    h.frame();
    assert_eq!(h.root.overlay_hits.len(), 1);

    // Paint again with nothing changed: the entry is re-registered, not
    // accumulated — an owner registering every frame must not grow the table.
    let scene = h.frame();
    assert_eq!(h.root.overlay_hits.len(), 1);
    assert_eq!(scene.texts.len(), 3);

    // The owner stops registering (its popover closed). There is nothing to
    // unregister: the next paint simply does not see it.
    h.state.specs[0].register = false;
    let scene = h.frame();
    assert!(
        h.root.overlay_hits.is_empty(),
        "a surface nobody registers stops existing after the next paint"
    );
    assert_eq!(
        scene
            .texts
            .iter()
            .map(|(_, text)| text.as_str())
            .collect::<Vec<_>>(),
        vec!["owner", "sibling"],
        "and stops painting"
    );

    // A `Down` inside where it used to be now reaches the main tree.
    h.clear_log();
    h.down(150.0, 30.0);
    assert!(h.owner_overlays().is_empty());
}

#[test]
fn the_window_size_reaches_a_childs_layout() {
    let h = OverlayHarness::new(vec![SurfaceSpec::floating("popover", floating_rect())]);
    assert_eq!(
        h.log.borrow().child_window_size,
        Some(Size::new(200.0, 200.0)),
        "a child lays out knowing the window, which is what an overlay pod is \
         sized against"
    );
}

#[test]
fn a_down_inside_a_floating_surface_reaches_only_its_owner_and_never_blurs() {
    let spec = SurfaceSpec::floating("popover", floating_rect());
    let mut h = OverlayHarness::new(vec![spec.clone()]);

    // A field elsewhere in the main tree takes focus first.
    h.down(30.0, 120.0);
    assert!(
        h.root.is_focus_active(),
        "the sibling holds a focus session"
    );
    let ime_before = h.root.ime_state();
    assert!(ime_before.is_some());
    let focus_gen_before = h.root.focus_ime_generation();
    h.clear_log();

    // Now press inside the floated surface.
    h.down(150.0, 30.0);

    assert_eq!(
        h.owner_overlays(),
        vec![(
            spec.key,
            OverlayEventKind::Pointer(PointerEvent {
                phase: PointerPhase::Down,
                position: Point::new(150.0, 30.0),
                button: PointerButton::Primary,
            })
        )],
        "the owner is reached by key, with a WINDOW-space payload"
    );
    assert_eq!(
        h.log.borrow().pod,
        vec![(
            "popover",
            InputEvent::Pointer(PointerEvent {
                phase: PointerPhase::Down,
                // 150-120, 30-10: the owner's one subtraction, the rect origin.
                position: Point::new(30.0, 20.0),
                button: PointerButton::Primary,
            })
        )],
        "and the owner forwards it into the pod in the pod's own space"
    );
    assert!(
        h.sibling_pointers().is_empty(),
        "the main tree never saw the press"
    );
    assert!(
        h.root.is_focus_active(),
        "and the press did NOT blur the field the surface belongs to"
    );
    assert_eq!(
        h.root.ime_state(),
        ime_before,
        "nor disturb its IME surface"
    );
    assert_eq!(
        h.root.focus_ime_generation(),
        focus_gen_before,
        "so no focus/IME edge fires at the shell either"
    );
    assert_eq!(
        h.log.borrow().sibling_focus.last(),
        Some(&true),
        "the focused leaf still reads as focused when the broadcast reaches it"
    );
}

#[test]
fn a_down_inside_a_transparent_surface_reaches_the_main_tree_normally() {
    // The transparent surface covers the sibling exactly.
    let mut spec = SurfaceSpec::floating("tooltip", Rect::new(0.0, 100.0, 60.0, 140.0));
    spec.input = OverlayInput::Transparent;
    spec.outside_tap = OutsideTap::Notify { consume: true };
    let mut h = OverlayHarness::new(vec![spec]);
    h.frame();
    h.clear_log();

    h.down(30.0, 120.0);

    assert!(
        h.owner_overlays().is_empty(),
        "a transparent surface is never hit-tested — not even for OutsideDown"
    );
    assert_eq!(
        h.sibling_pointers().len(),
        1,
        "the pointer passed straight through to the widget underneath"
    );
    assert!(h.root.is_focus_active(), "which claimed focus as usual");
}

#[test]
fn a_scroll_inside_a_surface_routes_to_its_owner_in_window_space() {
    let spec = SurfaceSpec::floating("popover", floating_rect());
    let mut h = OverlayHarness::new(vec![spec.clone()]);
    h.clear_log();

    h.dispatch(&InputEvent::Scroll {
        position: Point::new(150.0, 30.0),
        delta: ScrollDelta::Lines(0.0, 3.0),
    });

    assert_eq!(
        h.owner_overlays(),
        vec![(
            spec.key,
            OverlayEventKind::Scroll {
                position: Point::new(150.0, 30.0),
                delta: ScrollDelta::Lines(0.0, 3.0),
            }
        )]
    );
    assert_eq!(
        h.log.borrow().pod,
        vec![(
            "popover",
            InputEvent::Scroll {
                position: Point::new(30.0, 20.0),
                delta: ScrollDelta::Lines(0.0, 3.0),
            }
        )]
    );
}

#[test]
fn a_capture_from_inside_a_surface_routes_the_next_move_by_the_capture_path() {
    let mut spec = SurfaceSpec::floating("popover", floating_rect());
    spec.on_down = OwnerReaction::Capture;
    let mut h = OverlayHarness::new(vec![spec.clone()]);
    h.clear_log();

    h.down(150.0, 30.0);
    assert!(
        h.root.is_pointer_captured(),
        "a capture bubbled from an overlay Down opens a gesture exactly like a \
         hit-tested one"
    );
    h.clear_log();

    // Still INSIDE the surface's own rect — which is the case that
    // discriminates: the pre-pass would happily hit-test this one and route it
    // as another broadcast, and it must not, because the gesture is captured.
    h.dispatch(&pointer(PointerPhase::Move, 160.0, 45.0));
    assert!(
        h.owner_overlays().is_empty(),
        "a live capture short-circuits the overlay pre-pass even inside the \
         surface's own rect"
    );
    assert_eq!(
        h.log.borrow().owner,
        vec![InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Move,
            position: Point::new(160.0, 45.0),
            button: PointerButton::Primary,
        })],
        "the move reaches the owner by the ordinary captured path instead"
    );
    h.clear_log();

    // And the drag may wander far outside the surface — over the sibling, in
    // fact — without the sibling ever hearing about it.
    h.dispatch(&pointer(PointerPhase::Move, 30.0, 120.0));

    assert!(h.owner_overlays().is_empty());
    assert_eq!(
        h.log.borrow().owner,
        vec![InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Move,
            position: Point::new(30.0, 120.0),
            button: PointerButton::Primary,
        })],
        "which is what lets a drag begun inside a floated surface continue \
         outside it"
    );
    assert!(
        h.sibling_pointers().is_empty(),
        "and never reaches the widget it passed over"
    );

    // The `Up` closes the gesture through the ordinary pointer arm.
    h.dispatch(&pointer(PointerPhase::Up, 30.0, 120.0));
    assert!(!h.root.is_pointer_captured());
}

/// A capture claimed on a phase other than `Down` is a capture all the
/// same. The pods record their own active path on any phase, so a root that
/// mirrored the `Down` alone left the surface latched with no `Up` able to
/// reach it — and a latched surface diverts every later pointer event its
/// owner is reached by.
#[test]
fn a_capture_claimed_on_a_move_inside_a_surface_reaches_the_root() {
    let mut spec = SurfaceSpec::floating("popover", floating_rect());
    spec.on_move = OwnerReaction::Capture;
    let mut h = OverlayHarness::new(vec![spec]);

    h.dispatch(&pointer(PointerPhase::Move, 150.0, 30.0));
    assert!(
        h.root.is_pointer_captured(),
        "the root mirrors a claim the surface made mid-gesture"
    );

    // Which is what ends it: the mirrored capture short-circuits the overlay
    // pre-pass, so the `Up` routes down the capture path — outside the
    // surface's own rect, where a hit test would never have delivered it.
    h.dispatch(&pointer(PointerPhase::Up, 30.0, 120.0));
    assert!(
        !h.root.is_pointer_captured(),
        "and the gesture closes on the ordinary pointer arm"
    );
}

/// The release side of the root's overlay capture mirror. The claim is
/// mirrored on any phase, so the release has to be bounded on any phase too:
/// an `Up` reaches a surface as an overlay event exactly when no capture was
/// standing to divert it, and a claim made there has no gesture left to own.
/// A latch left standing short-circuits the overlay pre-pass, which stops
/// every floated surface taking input until some unrelated pointer release
/// happens along.
#[test]
fn a_capture_claimed_on_an_overlay_up_does_not_outlive_the_gesture() {
    let mut spec = SurfaceSpec::floating("popover", floating_rect());
    spec.on_up = OwnerReaction::Capture;
    let mut h = OverlayHarness::new(vec![spec]);

    h.down(150.0, 30.0);
    h.dispatch(&pointer(PointerPhase::Up, 150.0, 30.0));

    assert!(
        !h.root.is_pointer_captured(),
        "the phase that ends a gesture releases the mirror it just set"
    );

    // Which is what keeps the surfaces routable: a press inside one still
    // reaches its owner rather than being diverted down a capture path.
    h.clear_log();
    h.down(150.0, 30.0);
    assert_eq!(
        h.owner_overlays().len(),
        1,
        "the overlay pre-pass is still hit-testing"
    );
}

#[test]
fn a_focus_request_from_inside_a_surface_opens_a_session() {
    let mut spec = SurfaceSpec::floating("popover", floating_rect());
    spec.on_down = OwnerReaction::Focus;
    let mut h = OverlayHarness::new(vec![spec]);
    assert!(!h.root.is_focus_active());

    h.down(150.0, 30.0);

    assert!(
        h.root.is_focus_active(),
        "a text field inside a popover may claim focus — the overlay arm \
         honours the request even though it refuses the blur"
    );
}

/// The case the test above cannot express: the claim arrives while ANOTHER
/// branch already holds the session. Honouring it without retiring what it
/// supersedes leaves two branches believing they are focused — and leaves
/// the root describing, to the shell, a field that no longer owns anything.
#[test]
fn a_pod_focus_claim_retires_the_branch_it_supersedes() {
    let mut spec = SurfaceSpec::floating("popover", floating_rect());
    spec.pod_claims_focus = true;
    let mut h = OverlayHarness::new(vec![spec]);

    // A secure field in the main tree takes the session first.
    h.down(30.0, 120.0);
    h.frame();
    assert_eq!(
        h.branch_focus(),
        (false, true),
        "only the field's own branch reads focused while it holds the session"
    );
    assert_eq!(
        h.root.ime_state().map(|ime| ime.content_type),
        Some(ImeContentType::Password),
        "and the surface the shell configures its keyboard from is the \
         secure one that field published"
    );

    // Now the editable inside the floated surface claims focus.
    h.down(150.0, 30.0);
    assert!(h.root.is_focus_active(), "the claim is honoured");

    // The claim stamped its own chain with a session identity nothing older
    // carries, so the field's link stops counting on the very next pass —
    // no convergence frame, and nothing had to visit the branch the session
    // left in order to clear it.
    h.frame();
    assert_eq!(
        h.branch_focus(),
        (true, false),
        "exactly one branch believes it is focused: the one that claimed it"
    );
    assert_eq!(
        h.root.ime_state(),
        None,
        "and the surface that field published for the session it just lost \
         does NOT stand: the branch that took the session described none of \
         its own, so the shell is left configuring nothing rather than a \
         secure field nobody is in"
    );

    h.frame();
    assert_eq!(
        h.branch_focus(),
        (true, false),
        "which is a settled state, not a frame of transition"
    );
    assert!(
        h.root.is_focus_active(),
        "and the session the pod opened is still the live one"
    );
}

/// The return direction, which the test above never exercises: the session
/// goes to a floated surface and the user then presses the field in the main
/// tree again.
///
/// A press outside every floated rect is hit-tested through the containers
/// like any other, so the claim it produces is the main tree's by
/// construction — and the surface's own recorded link has to stop counting
/// the moment that claim lands, or the tree is de-seeded for the rest of the
/// pod's life and the secure surface the field publishes is thrown away
/// every frame.
#[test]
fn a_hit_tested_claim_takes_the_session_back_from_a_floated_surface() {
    let mut spec = SurfaceSpec::floating("popover", floating_rect());
    spec.pod_claims_focus = true;
    let mut h = OverlayHarness::new(vec![spec]);

    // The field takes the session, then the editable inside the surface
    // takes it away.
    h.down(30.0, 120.0);
    h.frame();
    h.down(150.0, 30.0);
    h.frame();
    assert_eq!(
        h.branch_focus(),
        (true, false),
        "the surface holds the session and the field's link no longer counts"
    );

    // The user presses the field again, outside `floating_rect()`, so the
    // press routes through the containers exactly as the first one did.
    h.down(30.0, 120.0);
    h.frame();

    assert_eq!(
        h.branch_focus(),
        (false, true),
        "the session is the field's again and the surface's link is retired"
    );
    assert_eq!(
        h.root.ime_state().map(|ime| ime.content_type),
        Some(ImeContentType::Password),
        "and the secure surface that field publishes reaches the shell"
    );
}

/// The same move, driven from the root's keyboard-class arm
/// (`Scroll | Key | Ime | EditCommand`) rather than a pointer `Down`. That
/// arm honours the claim and nothing else, so a mechanism that only retires
/// a surface's link on a press leaves the session split here.
#[test]
fn a_keyboard_class_claim_in_the_tree_retires_a_surfaces_link() {
    let mut spec = SurfaceSpec::floating("popover", floating_rect());
    spec.pod_claims_focus = true;
    let mut h = OverlayHarness::new(vec![spec]);

    h.down(150.0, 30.0);
    h.frame();
    assert!(h.branch_focus().0, "the surface holds the session");

    // A scroll over the field, outside every floated rect: hit-tested into
    // the main tree, but routed through the root's non-pointer focus arm.
    h.dispatch(&InputEvent::Scroll {
        position: Point::new(30.0, 120.0),
        delta: ScrollDelta::Lines(0.0, 3.0),
    });
    h.frame();

    assert_eq!(
        h.branch_focus(),
        (false, true),
        "a claim that arrives without a pointer `Down` retires the surface's \
         link just the same"
    );
    assert_eq!(
        h.root.ime_state().map(|ime| ime.content_type),
        Some(ImeContentType::Password),
    );
}

/// A pod dropped while it holds the link takes the session with it. Nothing
/// else can end that session: the widget that owned it no longer exists, so
/// no later pass can reach it to release it, and the root would otherwise go
/// on reporting a live focus session to the shell for a surface nobody can
/// see.
#[test]
fn a_pod_dropped_while_it_holds_the_link_ends_the_session_it_owned() {
    let mut spec = SurfaceSpec::floating("popover", floating_rect());
    spec.pod_claims_focus = true;
    let mut h = OverlayHarness::new(vec![spec]);

    h.down(150.0, 30.0);
    h.frame();
    assert!(h.root.is_focus_active(), "the surface holds the session");

    // The owner stops owning the surface: the spec goes, the pod is dropped
    // by the next rebuild.
    h.state.specs.clear();
    h.frame();

    assert!(
        !h.root.is_focus_active(),
        "the session dies with the widget that held it"
    );
    assert_eq!(h.root.ime_state(), None);

    // And the tree is not left de-seeded: the next press focuses normally.
    h.down(30.0, 120.0);
    h.frame();
    assert!(
        h.branch_focus().1,
        "a field in the tree takes the session as if no surface had existed"
    );
    assert_eq!(
        h.root.ime_state().map(|ime| ime.content_type),
        Some(ImeContentType::Password),
    );
}

/// The event-route half of the provenance rule. `paint_overlays` refuses a
/// paint-time publish from a pod holding no link; the dispatch that carries
/// an overlay broadcast has to refuse the same publish, or a surface the
/// user merely touched reconfigures the platform keyboard for a secure field
/// it has nothing to do with.
#[test]
fn a_pod_publish_from_an_event_cannot_overwrite_a_field_it_does_not_own() {
    let mut spec = SurfaceSpec::floating("popover", floating_rect());
    // Publishes from its `event`, and never claims focus.
    spec.pod_publishes_ime_on_event = true;
    let mut h = OverlayHarness::new(vec![spec]);

    h.down(30.0, 120.0);
    assert_eq!(
        h.root.ime_state().map(|ime| ime.content_type),
        Some(ImeContentType::Password),
        "the focused field's own surface"
    );

    // A press inside the surface. The pod publishes, holds no focus link,
    // and claims none.
    h.down(150.0, 30.0);

    assert_eq!(
        h.root.ime_state().map(|ime| ime.content_type),
        Some(ImeContentType::Password),
        "a publish from a branch that owns no session describes nobody's"
    );
    assert!(
        h.root.is_focus_active(),
        "and the field it did not own still holds the session"
    );
}

/// A pod publishing an IME surface it holds no focus link for describes
/// nobody's session. Accepting it would let a surface overwrite a focused
/// field's — including the content type that configures the platform
/// keyboard as a secure one.
#[test]
fn a_pod_publish_cannot_overwrite_the_surface_of_a_field_it_does_not_own() {
    let mut spec = SurfaceSpec::floating("popover", floating_rect());
    // Publishes on every paint, and never claims focus.
    spec.pod_publishes_ime = true;
    let mut h = OverlayHarness::new(vec![spec]);

    h.down(30.0, 120.0);
    assert_eq!(
        h.root.ime_state().map(|ime| ime.content_type),
        Some(ImeContentType::Password),
        "the focused field's own surface"
    );

    h.frame();

    assert!(
        h.root.is_focus_active(),
        "the field still holds the session"
    );
    assert_eq!(
        h.root.ime_state().map(|ime| ime.content_type),
        Some(ImeContentType::Password),
        "and it still describes it: the publishing pod holds no focus link, \
         so its surface is not the session's"
    );
}

/// The provenance rule is a check, not a refusal: a pod that DOES hold the
/// recorded focus path publishes exactly like a field in the tree.
#[test]
fn a_pod_that_holds_the_focus_path_publishes_its_own_surface() {
    let mut spec = SurfaceSpec::floating("popover", floating_rect());
    spec.pod_claims_focus = true;
    spec.pod_publishes_ime = true;
    let mut h = OverlayHarness::new(vec![spec]);

    h.down(30.0, 120.0);
    h.down(150.0, 30.0);
    h.frame();

    assert!(h.root.is_focus_active());
    assert_eq!(
        h.root.ime_state().map(|ime| ime.content_type),
        Some(ImeContentType::Normal),
        "the surface published by the branch that owns the session"
    );
}

#[test]
fn an_outside_press_notifies_a_consuming_surface_and_stops_there() {
    let mut spec = SurfaceSpec::floating("popover", floating_rect());
    spec.outside_tap = OutsideTap::Notify { consume: true };
    let mut h = OverlayHarness::new(vec![spec.clone()]);
    h.clear_log();

    // Inside the sibling, outside every floated rect.
    h.down(30.0, 120.0);

    assert_eq!(
        h.owner_overlays(),
        vec![(spec.key, OverlayEventKind::OutsideDown)],
        "the light-dismiss notification carries no position"
    );
    assert!(
        h.sibling_pointers().is_empty(),
        "and the press that dismissed the menu did not also activate what was \
         underneath it"
    );
    assert!(
        !h.root.is_focus_active(),
        "the main tree saw no Down at all, so nothing claimed focus"
    );
}

#[test]
fn a_pass_through_outside_press_notifies_and_still_reaches_the_main_tree() {
    let mut spec = SurfaceSpec::floating("popover", floating_rect());
    spec.outside_tap = OutsideTap::Notify { consume: false };
    let mut h = OverlayHarness::new(vec![spec.clone()]);
    h.clear_log();

    let outcome = h.down(30.0, 120.0);

    assert_eq!(
        h.owner_overlays(),
        vec![(spec.key, OverlayEventKind::OutsideDown)]
    );
    assert_eq!(
        h.sibling_pointers().len(),
        1,
        "consume: false means both — the owner hears, and the press continues"
    );
    assert!(h.root.is_focus_active(), "so the tapped field took focus");
    assert!(
        outcome.handled,
        "and the main dispatch's own outcome survives"
    );
}

#[test]
fn an_ignoring_surface_hears_nothing_about_an_outside_press() {
    // `Ignore` is the default; state it explicitly.
    let mut spec = SurfaceSpec::floating("popover", floating_rect());
    spec.outside_tap = OutsideTap::Ignore;
    let mut h = OverlayHarness::new(vec![spec]);
    h.clear_log();

    h.down(30.0, 120.0);

    assert!(
        h.owner_overlays().is_empty(),
        "a surface that dismisses some other way is never told"
    );
    assert_eq!(h.sibling_pointers().len(), 1);
}

#[test]
fn only_a_primary_press_dismisses() {
    let mut spec = SurfaceSpec::floating("popover", floating_rect());
    spec.outside_tap = OutsideTap::Notify { consume: true };
    let mut h = OverlayHarness::new(vec![spec]);
    h.clear_log();

    // A secondary press is a context gesture, not a dismissal.
    h.dispatch(&InputEvent::Pointer(PointerEvent {
        phase: PointerPhase::Down,
        position: Point::new(30.0, 120.0),
        button: PointerButton::Secondary,
    }));
    assert!(h.owner_overlays().is_empty());
    assert_eq!(h.sibling_pointers().len(), 1, "and it reaches the tree");

    // Nor does a move or a lift outside the surface.
    h.clear_log();
    h.dispatch(&pointer(PointerPhase::Move, 30.0, 120.0));
    h.dispatch(&pointer(PointerPhase::Up, 30.0, 120.0));
    assert!(h.owner_overlays().is_empty());
}

#[test]
fn an_overlay_press_leaves_the_main_trees_hover_standing() {
    let spec = SurfaceSpec::floating("popover", floating_rect());
    let mut h = OverlayHarness::new(vec![spec]);
    let epoch_before = h.root.hover_epoch;

    h.down(150.0, 30.0);

    assert_eq!(
        h.root.hover_epoch, epoch_before,
        "an overlay pass advances no hover epoch, so a live hover in the main \
         tree is not stranded by a press on a floated surface"
    );
}
