//! Overlay portal routing, capture and focus-session tests for RenderRoot.

use super::*;

// Overlay portal: an owner that floats a pod, a sibling painted after it, and a root
// that routes like any hand-written container.

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

    // Still INSIDE the surface's rect: the pre-pass would hit-test and route it as
    // a broadcast, and must not, because the gesture is captured.
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

    // The mirrored capture short-circuits the overlay pre-pass, so the `Up` routes
    // down the capture path, outside the surface's rect.
    h.dispatch(&pointer(PointerPhase::Up, 30.0, 120.0));
    assert!(
        !h.root.is_pointer_captured(),
        "and the gesture closes on the ordinary pointer arm"
    );
}

/// The release side of the capture mirror. The claim is mirrored on any phase, so
/// the release is bounded on any phase too: a latch left standing short-circuits the
/// overlay pre-pass and stops every floated surface taking input.
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

    // The claim stamped its chain with a newer session identity, so the old link stops
    // counting on the next pass, with no convergence frame.
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

/// The return direction: the session goes to a floated surface and the user then
/// presses the main-tree field again.
///
/// That claim is the main tree's by construction, and the surface's recorded link
/// must stop counting at once or the secure surface is thrown away every frame.
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

// Declined keys: a modal pod's shortcut mask and Escape, and the editable owner's forward.

use crate::overlay::{OverlayBand, OverlayKeys, OverlayPod};
use crate::shortcuts::{Chord, ShortcutDecl, ShortcutId, ShortcutScope};

#[derive(Default)]
struct KeyLog {
    shortcuts: Vec<&'static str>,
    keys: Vec<(&'static str, Key)>,
    overlay_keys: Vec<Key>,
}

type KeyShared = Rc<RefCell<KeyLog>>;

/// Declares its chord every paint; an editable claims focus on `Down` and eats plain typing.
struct KeyLeaf {
    name: &'static str,
    chord: Option<(&'static str, ShortcutScope)>,
    id: ShortcutId,
    editable: bool,
    log: KeyShared,
}

impl KeyLeaf {
    fn pod(
        name: &'static str,
        chord: Option<(&'static str, ShortcutScope)>,
        editable: bool,
        log: &KeyShared,
    ) -> crate::widget::ChildPod {
        crate::widget::ChildPod::new(Box::new(KeyLeaf {
            name,
            chord,
            id: ShortcutId::next(),
            editable,
            log: Rc::clone(log),
        }))
    }
}

impl crate::widget::Widget for KeyLeaf {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(80.0, 40.0))
    }
    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        if let Some((chord, scope)) = self.chord {
            ctx.register_shortcut(ShortcutDecl {
                id: self.id,
                chord: Chord::parse(chord).unwrap(),
                label: self.name.to_string(),
                scope,
            });
        }
    }
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        match event {
            InputEvent::Shortcut(id) if *id == self.id => {
                self.log.borrow_mut().shortcuts.push(self.name);
                EventResult::Handled
            }
            InputEvent::Key(key) => {
                self.log
                    .borrow_mut()
                    .keys
                    .push((self.name, key.key.clone()));
                let typed = matches!(key.key, Key::Character(_)) && !key.modifiers.ctrl;
                if self.editable && typed {
                    EventResult::Handled
                } else {
                    EventResult::Ignored
                }
            }
            InputEvent::Pointer(p) if p.phase == PointerPhase::Down => {
                if self.editable {
                    ctx.request_focus();
                }
                EventResult::Handled
            }
            _ => EventResult::Ignored,
        }
    }
}

#[derive(Clone, Copy)]
struct KeySpec {
    input: OverlayInput,
    keys: OverlayKeys,
    outside_tap: OutsideTap,
    /// Whether the owner forwards its field's declined keys to its pod.
    forward: bool,
    /// Whether the owner consumes an `OverlayEventKind::Key`.
    pod_eats: bool,
}

const POD_RECT: Rect = Rect::new(120.0, 0.0, 200.0, 40.0);

/// A combobox/modal-shaped owner: an editable field in place and a floated `KeyLeaf` pod.
struct KeyOwner {
    key: OverlayKey,
    spec: KeySpec,
    field: crate::widget::ChildPod,
    pod: OverlayPod,
    log: KeyShared,
}

impl crate::widget::Widget for KeyOwner {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        self.field.layout_child(ctx, bc);
        let window = BoxConstraints::loose(ctx.window_size());
        self.pod.borrow_mut().layout_child(ctx, &window);
        bc.constrain(Size::new(80.0, 40.0))
    }
    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.field.paint_child(ctx, scene);
        ctx.register_overlay(OverlayEntry {
            key: self.key,
            band: OverlayBand::Floating,
            input: self.spec.input,
            outside_tap: self.spec.outside_tap,
            keys: self.spec.keys,
            window_rect: POD_RECT,
            pod: Rc::clone(&self.pod),
            insets: ctx.window_insets(),
        });
    }
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if let InputEvent::Overlay(overlay) = event
            && overlay.key == self.key
        {
            let mut pod = self.pod.borrow_mut();
            match &overlay.kind {
                OverlayEventKind::Key(key) => {
                    self.log.borrow_mut().overlay_keys.push(key.key.clone());
                    if self.spec.pod_eats {
                        ctx.consume_overlay_key();
                    }
                }
                OverlayEventKind::Shortcut(id) => {
                    pod.event_child(ctx, &InputEvent::Shortcut(*id));
                }
                OverlayEventKind::Pointer(p) => {
                    let position = p.position - POD_RECT.origin().to_vec2();
                    pod.event_child(ctx, &InputEvent::Pointer(PointerEvent { position, ..*p }));
                }
                _ => {}
            }
            return EventResult::Ignored;
        }
        if event.is_broadcast() {
            self.field.event_child(ctx, event);
            return EventResult::Ignored;
        }
        if event.is_focus_routed() {
            if self.pod.borrow().holds_live_focus() {
                return self.pod.borrow_mut().event_child(ctx, event);
            }
            if self.field.holds_live_focus() {
                return self.field.event_child(ctx, event);
            }
            return EventResult::Ignored;
        }
        if !self.field.contains(event.position()) {
            return EventResult::Ignored;
        }
        let result = self.field.event_child(ctx, event);
        if self.spec.forward {
            ctx.forward_declined_keys_to(self.key);
        }
        result
    }
}

/// A column: the owner, then main-tree leaves declaring `Ctrl+K` (Local), `Ctrl+J`
/// (AppGlobal) and `Down` (Local).
struct KeyRoot {
    children: Vec<crate::widget::ChildPod>,
}

impl crate::widget::Widget for KeyRoot {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        for (row, pod) in self.children.iter_mut().enumerate() {
            pod.layout_child(ctx, &BoxConstraints::loose(Size::new(80.0, 40.0)));
            pod.set_origin(Point::new(0.0, row as f64 * 40.0));
        }
        bc.constrain(Size::new(200.0, 200.0))
    }
    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        for pod in &mut self.children {
            pod.paint_child(ctx, scene);
        }
    }
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            for pod in &mut self.children {
                pod.event_child(ctx, event);
            }
            return EventResult::Ignored;
        }
        let focus_routed = event.is_focus_routed();
        for pod in &mut self.children {
            let reached = if focus_routed {
                pod.holds_live_focus()
            } else {
                pod.contains(event.position())
            };
            if reached {
                return pod.event_child(ctx, event);
            }
        }
        EventResult::Ignored
    }
}

struct KeyView {
    spec: KeySpec,
    log: KeyShared,
}

impl View<()> for KeyView {
    type Element = KeyRoot;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> KeyRoot {
        let log = &self.log;
        let owner = KeyOwner {
            key: OverlayKey::next(),
            spec: self.spec,
            field: KeyLeaf::pod("field", None, true, log),
            pod: Rc::new(RefCell::new(KeyLeaf::pod(
                "pod",
                Some(("Ctrl+K", ShortcutScope::Local)),
                true,
                log,
            ))),
            log: Rc::clone(log),
        };
        KeyRoot {
            children: vec![
                crate::widget::ChildPod::new(Box::new(owner)),
                KeyLeaf::pod("local", Some(("Ctrl+K", ShortcutScope::Local)), false, log),
                KeyLeaf::pod(
                    "global",
                    Some(("Ctrl+J", ShortcutScope::AppGlobal)),
                    false,
                    log,
                ),
                KeyLeaf::pod("nav", Some(("Down", ShortcutScope::Local)), false, log),
            ],
        }
    }
    fn rebuild(
        &self,
        _prev: &Self,
        _element: &mut KeyRoot,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

struct KeyHarness {
    root: RenderRoot<(), KeyView>,
    log: KeyShared,
}

impl KeyHarness {
    fn new(spec: KeySpec) -> Self {
        let log = KeyShared::default();
        let mut h = Self {
            root: RenderRoot::new(),
            log: Rc::clone(&log),
        };
        let mut build = move |_: &mut ()| KeyView {
            spec,
            log: Rc::clone(&log),
        };
        h.root.rebuild(&mut build, &mut ());
        h.root.layout(Size::new(200.0, 200.0));
        h.root
            .paint(&mut RecordingScene::default(), FrameTime::ZERO);
        h
    }

    fn press(&mut self, key: Key, ctrl: bool) -> EventOutcome {
        let modifiers = Modifiers {
            ctrl,
            ..Modifiers::default()
        };
        let event = KeyEvent {
            key,
            modifiers,
            repeat: false,
        };
        self.root.event(&mut (), &InputEvent::Key(event))
    }

    fn down(&mut self, x: f64, y: f64) {
        self.root.event(&mut (), &pointer(PointerPhase::Down, x, y));
    }

    fn shortcuts(&self) -> Vec<&'static str> {
        self.log.borrow().shortcuts.clone()
    }

    fn overlay_keys(&self) -> Vec<Key> {
        self.log.borrow().overlay_keys.clone()
    }
}

fn char_key(text: &str) -> Key {
    Key::Character(text.to_string())
}

const ESCAPE: Key = Key::Named(NamedKey::Escape);
const DOWN: Key = Key::Named(NamedKey::ArrowDown);

fn spec() -> KeySpec {
    KeySpec {
        input: OverlayInput::Interactive,
        keys: OverlayKeys::Pass,
        outside_tap: OutsideTap::Ignore,
        forward: false,
        pod_eats: false,
    }
}

fn modal_specs() -> [KeySpec; 2] {
    let consuming = OutsideTap::Notify { consume: true };
    [
        KeySpec {
            keys: OverlayKeys::Modal,
            ..spec()
        },
        KeySpec {
            outside_tap: consuming,
            ..spec()
        },
    ]
}

#[test]
fn a_modal_pod_masks_main_tree_shortcuts_and_its_own_declaration_fires() {
    for modal in modal_specs() {
        let mut h = KeyHarness::new(modal);
        assert!(h.press(char_key("k"), true).handled);
        assert_eq!(h.shortcuts(), vec!["pod"], "the pod's own tier answers");
        assert!(!h.press(char_key("j"), true).handled, "AppGlobal is masked");
        assert_eq!(h.shortcuts(), vec!["pod"]);
        h.press(DOWN, false);
        assert_eq!(h.shortcuts(), vec!["pod"], "and so is main-tree Local");
    }
    let mut h = KeyHarness::new(spec());
    h.press(char_key("j"), true);
    assert_eq!(
        h.shortcuts(),
        vec!["global"],
        "a non-modal pod masks nothing"
    );
}

#[test]
fn escape_with_a_modal_pod_open_and_nothing_focused_reaches_the_pod() {
    for modal in modal_specs() {
        let mut h = KeyHarness::new(KeySpec {
            pod_eats: true,
            ..modal
        });
        assert!(!h.root.is_focus_active());
        assert!(h.press(ESCAPE, false).handled, "the owner consumed it");
        assert_eq!(h.overlay_keys(), vec![ESCAPE]);
    }
    let mut h = KeyHarness::new(spec());
    h.press(ESCAPE, false);
    assert!(h.overlay_keys().is_empty(), "only a modal pod takes Escape");
}

#[test]
fn a_focused_editable_inside_the_modal_still_consumes_typed_keys() {
    let [modal, _] = modal_specs();
    let mut h = KeyHarness::new(modal);
    h.down(150.0, 20.0);
    assert!(h.root.is_focus_active(), "the pod's editable took focus");
    assert!(h.press(char_key("a"), false).handled);
    assert_eq!(h.log.borrow().keys, vec![("pod", char_key("a"))]);
    assert!(h.overlay_keys().is_empty() && h.shortcuts().is_empty());
    h.press(ESCAPE, false);
    assert_eq!(
        h.overlay_keys(),
        vec![ESCAPE],
        "an Escape the focused consumer ignored still reaches its modal pod"
    );
}

#[test]
fn a_declined_arrow_from_a_focused_editable_reaches_the_forwarded_pod() {
    let mut h = KeyHarness::new(KeySpec {
        forward: true,
        pod_eats: true,
        ..spec()
    });
    h.down(40.0, 20.0);
    assert!(
        h.press(char_key("a"), false).handled,
        "the field eats typing"
    );
    assert!(h.overlay_keys().is_empty());
    assert!(h.press(DOWN, false).handled);
    assert_eq!(h.overlay_keys(), vec![DOWN]);
    assert!(h.shortcuts().is_empty(), "consumed before the registry");

    h.down(40.0, 140.0);
    assert!(!h.root.is_focus_active(), "a press on the nav leaf blurs");
    h.press(DOWN, false);
    assert_eq!(
        h.overlay_keys(),
        vec![DOWN],
        "the forward ended with the session"
    );
    assert_eq!(h.shortcuts(), vec!["nav"]);
}

#[test]
fn a_forwarded_key_the_pod_ignores_continues_to_the_registry() {
    let mut h = KeyHarness::new(KeySpec {
        forward: true,
        ..spec()
    });
    h.down(40.0, 20.0);
    h.press(DOWN, false);
    assert_eq!(h.overlay_keys(), vec![DOWN]);
    assert_eq!(h.shortcuts(), vec!["nav"]);
}

#[test]
fn without_the_forward_setting_a_declined_arrow_falls_to_the_registry() {
    let mut h = KeyHarness::new(spec());
    h.down(40.0, 20.0);
    assert!(h.root.is_focus_active());
    assert!(h.press(DOWN, false).handled);
    assert!(h.overlay_keys().is_empty());
    assert_eq!(h.shortcuts(), vec!["nav"]);
}

#[test]
fn a_transparent_pod_never_takes_keys() {
    let mut h = KeyHarness::new(KeySpec {
        input: OverlayInput::Transparent,
        keys: OverlayKeys::Modal,
        forward: true,
        pod_eats: true,
        ..spec()
    });
    h.down(40.0, 20.0);
    h.press(DOWN, false);
    h.press(ESCAPE, false);
    assert!(h.overlay_keys().is_empty(), "neither forwarded nor modal");
    h.press(char_key("j"), true);
    assert_eq!(h.shortcuts(), vec!["nav", "global"], "and it masks nothing");
}
