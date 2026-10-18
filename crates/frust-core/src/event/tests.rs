//! Unit tests for the event module.

use std::path::PathBuf;

use kurbo::{Affine, Point, Rect, Size, Vec2};

use crate::overlay::OverlayKey;

use super::ctx::ChildFold;
use super::*;

fn down(x: f64, y: f64) -> InputEvent {
    InputEvent::Pointer(PointerEvent {
        phase: PointerPhase::Down,
        position: Point::new(x, y),
        button: PointerButton::Primary,
    })
}

#[test]
fn state_mut_recovers_concrete_state() {
    let mut count = 3u32;
    let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::new(10.0, 10.0));
    *ctx.state_mut::<u32>() += 1;
    assert_eq!(count, 4);
}

#[test]
fn request_redraw_and_capture_set_flags() {
    let mut count = 0u32;
    let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
    assert!(!ctx.needs_redraw());
    assert!(!ctx.is_pointer_captured());
    ctx.request_redraw();
    ctx.capture_pointer();
    assert!(ctx.needs_redraw());
    assert!(ctx.is_pointer_captured());
}

#[test]
fn translated_shifts_pointer_position() {
    let e = down(20.0, 30.0);
    let local = e.translated(-Vec2::new(5.0, 7.0));
    assert_eq!(local.position(), Point::new(15.0, 23.0));
    // The original is untouched.
    assert_eq!(e.position(), Point::new(20.0, 30.0));
}

#[test]
fn transformed_maps_every_positioned_variant_like_translated() {
    // A pure translation through `transformed` must agree with `translated`
    // for every variant, positioned or not.
    let offset = Vec2::new(-5.0, -7.0);
    let affine = Affine::translate(offset);
    let scale = ScaleEvent {
        phase: ScalePhase::Update,
        scale_delta: 1.5,
        focal: Point::new(20.0, 30.0),
        velocity: 0.25,
    };
    let events = [
        down(20.0, 30.0),
        InputEvent::PointerContact {
            pointer_id: PointerId::touch(1),
            event: PointerEvent {
                phase: PointerPhase::Move,
                position: Point::new(20.0, 30.0),
                button: PointerButton::Primary,
            },
        },
        InputEvent::Scroll {
            position: Point::new(20.0, 30.0),
            delta: ScrollDelta::Pixels(3.0, 4.0),
        },
        InputEvent::Scale(scale),
        InputEvent::FileDrop(FileDropEvent {
            phase: FileDropPhase::Hover,
            position: Point::new(20.0, 30.0),
            paths: Vec::new(),
        }),
        InputEvent::Housekeeping,
    ];
    for event in &events {
        assert_eq!(event.transformed(&affine), event.translated(offset));
    }
}

#[test]
fn file_drop_is_hit_tested_and_translates_its_position_only() {
    let drop = InputEvent::FileDrop(FileDropEvent {
        phase: FileDropPhase::Drop,
        position: Point::new(20.0, 30.0),
        paths: vec![PathBuf::from("/tmp/a.txt"), PathBuf::from("/tmp/b.txt")],
    });
    assert_eq!(drop.position(), Point::new(20.0, 30.0));
    assert!(!drop.is_broadcast());
    assert!(!drop.is_focus_routed());

    let local = drop.translated(-Vec2::new(5.0, 7.0));
    let InputEvent::FileDrop(local_drop) = &local else {
        panic!("expected FileDrop");
    };
    assert_eq!(local_drop.position, Point::new(15.0, 23.0));
    assert_eq!(local_drop.phase, FileDropPhase::Drop);
    assert_eq!(
        local_drop.paths,
        vec![PathBuf::from("/tmp/a.txt"), PathBuf::from("/tmp/b.txt")],
        "translating must not disturb the dropped paths"
    );
    // The original is untouched.
    assert_eq!(drop.position(), Point::new(20.0, 30.0));
}

#[test]
fn only_the_ended_file_drop_phase_is_a_positionless_broadcast() {
    for phase in [
        FileDropPhase::Hover,
        FileDropPhase::Drop,
        FileDropPhase::Cancel,
    ] {
        let event = InputEvent::FileDrop(FileDropEvent {
            phase,
            position: Point::new(20.0, 30.0),
            paths: Vec::new(),
        });
        assert!(!event.is_broadcast(), "{phase:?} is hit-tested");
    }

    let ended = InputEvent::FileDrop(FileDropEvent {
        phase: FileDropPhase::Ended,
        position: Point::ZERO,
        paths: Vec::new(),
    });
    assert!(ended.is_broadcast(), "the follow-up reaches every widget");
    assert!(!ended.is_focus_routed());
    assert_eq!(ended.position(), Point::ZERO);
    assert_eq!(
        ended.translated(Vec2::new(5.0, 7.0)),
        ended,
        "a broadcast is never re-based by a container"
    );
    assert_eq!(ended.transformed(&Affine::scale(2.0)), ended);
}

#[test]
fn transformed_maps_positions_through_scale_and_keeps_magnitudes() {
    // Inverse of scale(2) then translate(10, 20): container (30, 60) is
    // local (10, 20).
    let inverse = (Affine::translate(Vec2::new(10.0, 20.0)) * Affine::scale(2.0)).inverse();
    assert_eq!(
        down(30.0, 60.0).transformed(&inverse).position(),
        Point::new(10.0, 20.0)
    );
    let scroll = InputEvent::Scroll {
        position: Point::new(30.0, 60.0),
        delta: ScrollDelta::Pixels(3.0, 4.0),
    };
    assert_eq!(
        scroll.transformed(&inverse),
        InputEvent::Scroll {
            position: Point::new(10.0, 20.0),
            delta: ScrollDelta::Pixels(3.0, 4.0),
        }
    );
    let scale = InputEvent::Scale(ScaleEvent {
        phase: ScalePhase::Begin,
        scale_delta: 1.25,
        focal: Point::new(30.0, 60.0),
        velocity: 2.0,
    });
    assert_eq!(
        scale.transformed(&inverse),
        InputEvent::Scale(ScaleEvent {
            phase: ScalePhase::Begin,
            scale_delta: 1.25,
            focal: Point::new(10.0, 20.0),
            velocity: 2.0,
        })
    );
}

#[test]
fn absorb_child_folds_flags_upward() {
    let mut count = 0u32;
    let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
    {
        let mut child = ctx.child_ctx(
            Point::new(1.0, 2.0),
            Size::new(3.0, 4.0),
            false,
            false,
            false,
        );
        child.request_redraw();
        child.capture_pointer();
        let (redraw, cap) = (child.needs_redraw(), child.is_pointer_captured());
        ctx.absorb_child(ChildFold {
            needs_redraw: redraw,
            captured: cap,
            ..ChildFold::default()
        });
    }
    assert!(ctx.needs_redraw());
    assert!(ctx.is_pointer_captured());
}

#[test]
fn request_and_release_focus_set_flags() {
    let mut count = 0u32;
    let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
    assert!(!ctx.is_focus_requested());
    assert!(!ctx.is_focus_released());
    assert!(!ctx.has_focus());
    ctx.request_focus();
    ctx.release_focus();
    assert!(ctx.is_focus_requested());
    assert!(ctx.is_focus_released());
}

#[test]
fn child_ctx_seeds_has_focus_from_pod_flag() {
    let mut count = 0u32;
    let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
    let focused_child = ctx.child_ctx(Point::ZERO, Size::ZERO, true, false, false);
    assert!(focused_child.has_focus());
    let unfocused_child = ctx.child_ctx(Point::ZERO, Size::ZERO, false, false, false);
    assert!(!unfocused_child.has_focus());
}

#[test]
fn absorb_child_folds_focus_and_ime_upward() {
    let mut count = 0u32;
    let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
    let published = ImeState {
        active: true,
        editing: EditingState {
            text: "hi".to_string(),
            selection_base: 2,
            selection_extent: 2,
            composing_base: -1,
            composing_extent: -1,
        },
        caret: Some(Rect::new(0.0, 0.0, 1.0, 10.0)),
        content_type: ImeContentType::Normal,
        suppress_soft_keyboard: false,
    };
    {
        let mut child = ctx.child_ctx(Point::ZERO, Size::ZERO, false, false, false);
        child.request_focus();
        child.publish_ime_state(published.clone());
        let (fr, frl, ime) = (
            child.is_focus_requested(),
            child.is_focus_released(),
            child.take_ime_state(),
        );
        ctx.absorb_child(ChildFold {
            focus_requested: fr,
            focus_released: frl,
            ime_state: ime,
            ..ChildFold::default()
        });
    }
    assert!(ctx.is_focus_requested());
    assert!(!ctx.is_focus_released());
    assert_eq!(ctx.take_ime_state(), Some(published));
}

#[test]
fn claim_hover_records_nothing_unless_the_pass_is_eligible() {
    let mut count = 0u32;
    let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
    // A bare context is ineligible by default — the safe direction: a widget
    // that claims on a pass the root never marked a hover pass records nothing.
    assert!(!ctx.is_hover_eligible());
    ctx.claim_hover();
    assert!(
        !ctx.is_hover_claimed(),
        "an ineligible claim records nothing"
    );

    ctx.set_hover_eligible(true);
    ctx.claim_hover();
    assert!(ctx.is_hover_claimed());
    // A claim never touches the redraw channel: the widget owns change
    // detection (see `claim_hover`'s docs).
    assert!(!ctx.needs_redraw());
}

#[test]
fn child_ctx_seeds_hover_and_absorb_bubbles_a_claim() {
    let mut count = 0u32;
    let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
    ctx.set_hover_epoch(7);
    {
        // A hovered, eligible child: it sees its own link and its claim
        // bubbles into the parent so the whole path records the same stamp.
        let mut child = ctx.child_ctx(Point::ZERO, Size::ZERO, false, true, true);
        assert!(child.is_hovered());
        assert_eq!(child.hover_epoch(), 7, "the live epoch threads down");
        assert_eq!(child.hover_claim_epoch(), 8, "a claim takes the next epoch");
        child.claim_hover();
        let claimed = child.is_hover_claimed();
        ctx.absorb_child(ChildFold {
            hover_claimed: claimed,
            ..ChildFold::default()
        });
    }
    assert!(ctx.is_hover_claimed());
}

#[test]
fn content_type_defaults_to_no_hint() {
    assert_eq!(ImeContentType::default(), ImeContentType::Normal);
    assert!(!ImeContentType::default().is_secret());
    assert!(!ImeContentType::default().suppresses_suggestions());
}

#[test]
fn content_type_predicates_classify_every_variant() {
    // `is_secret` gates secure entry; `suppresses_suggestions` gates the suggestion strip and
    // learning. A shell branches on these, never on a `_` arm (the enum is non-exhaustive).
    assert!(ImeContentType::Password.is_secret());
    assert!(ImeContentType::Password.suppresses_suggestions());

    assert!(!ImeContentType::NoSuggestions.is_secret());
    assert!(ImeContentType::NoSuggestions.suppresses_suggestions());

    assert!(!ImeContentType::Terminal.is_secret());
    assert!(ImeContentType::Terminal.suppresses_suggestions());

    assert!(!ImeContentType::Normal.is_secret());
    assert!(!ImeContentType::Normal.suppresses_suggestions());
}

#[test]
fn default_ime_state_is_cleared_and_unhinted() {
    let s = ImeState::default();
    assert!(!s.active);
    assert!(s.caret.is_none());
    assert_eq!(s.content_type, ImeContentType::Normal);
    // `−1` sentinels, not the derived zeros: no selection, no composition.
    assert_eq!(
        s.editing,
        EditingState {
            text: String::new(),
            selection_base: -1,
            selection_extent: -1,
            composing_base: -1,
            composing_extent: -1,
        }
    );
}

/// The content-type hint must survive core's opaque passthrough untouched —
/// core never inspects or rewrites it, it only carries it to the shell.
#[test]
fn publish_ime_state_round_trips_the_content_type() {
    let mut count = 0u32;
    let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
    let published = ImeState {
        active: true,
        editing: EditingState {
            text: "hunter2".to_string(),
            selection_base: 7,
            selection_extent: 7,
            composing_base: -1,
            composing_extent: -1,
        },
        caret: Some(Rect::new(0.0, 0.0, 1.0, 10.0)),
        content_type: ImeContentType::Password,
        suppress_soft_keyboard: false,
    };
    ctx.publish_ime_state(published.clone());
    let taken = ctx.take_ime_state().expect("published state");
    assert_eq!(taken, published);
    assert_eq!(taken.content_type, ImeContentType::Password);
    // The secret's text is published verbatim (the platform IME mirror needs it); the hint,
    // not redaction, tells the shell to lock the keyboard down.
    assert_eq!(taken.editing.text, "hunter2");
}

/// …and it survives the focus-chain bubble a real widget publication takes.
#[test]
fn content_type_bubbles_up_the_focus_chain() {
    let mut count = 0u32;
    let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
    let published = ImeState {
        content_type: ImeContentType::NoSuggestions,
        ..ImeState::default()
    };
    {
        let mut child = ctx.child_ctx(Point::ZERO, Size::ZERO, true, false, false);
        child.publish_ime_state(published.clone());
        let ime = child.take_ime_state();
        ctx.absorb_child(ChildFold {
            ime_state: ime,
            ..ChildFold::default()
        });
    }
    assert_eq!(ctx.take_ime_state(), Some(published));
}

/// The keyboard-suppression hint is carried, not interpreted, and prints plainly (a routing
/// hint, not a secret) so a trace shows why no keyboard came up.
#[test]
fn suppress_soft_keyboard_defaults_off_round_trips_and_prints_plainly() {
    assert!(
        !ImeState::default().suppress_soft_keyboard,
        "a publisher that says nothing must behave as it did before the hint existed"
    );
    let mut count = 0u32;
    let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
    let published = ImeState {
        active: true,
        suppress_soft_keyboard: true,
        ..ImeState::default()
    };
    ctx.publish_ime_state(published.clone());
    let taken = ctx.take_ime_state().expect("published state");
    assert_eq!(taken, published);
    assert!(taken.suppress_soft_keyboard);
    assert!(format!("{taken:?}").contains("suppress_soft_keyboard: true"));
}

#[test]
fn debug_redacts_a_secret_field_but_not_a_normal_one() {
    let secret = ImeState {
        active: true,
        editing: EditingState {
            text: "hunter2".to_string(),
            ..EditingState::default()
        },
        caret: None,
        content_type: ImeContentType::Password,
        suppress_soft_keyboard: false,
    };
    let rendered = format!("{secret:?}");
    assert!(
        !rendered.contains("hunter2"),
        "secret text leaked: {rendered}"
    );
    assert!(rendered.contains("<redacted>"));
    assert!(rendered.contains("Password"));

    let plain = ImeState {
        content_type: ImeContentType::Normal,
        ..secret
    };
    assert!(format!("{plain:?}").contains("hunter2"));
}

#[test]
fn key_and_ime_events_are_focus_routed_with_zero_position() {
    let key = InputEvent::Key(KeyEvent {
        key: Key::Named(NamedKey::Enter),
        modifiers: Modifiers::default(),
        repeat: false,
    });
    assert!(key.is_focus_routed());
    assert_eq!(key.position(), Point::ZERO);
    // translated is identity for focus-routed events.
    assert_eq!(key.translated(Vec2::new(5.0, 5.0)), key);

    let ime = InputEvent::Ime(ImeEvent::Commit("x".to_string()));
    assert!(ime.is_focus_routed());
    assert!(!down(1.0, 1.0).is_focus_routed());
}

#[test]
fn pending_result_flush_peek_observes_the_mark_without_draining_it() {
    // A frame gate peeks before deciding whether the frame runs, so the peek must not consume
    // the mark the rebuild drains. Drain first so no earlier mark on this thread leaks in.
    let _ = take_pending_result_flush();
    assert!(!has_pending_result_flush(), "starts clear");

    mark_pending_result_flush();
    assert!(has_pending_result_flush(), "the peek observes the mark");
    // Repeated peeks are idempotent — the mark survives every one of them.
    assert!(has_pending_result_flush());
    assert!(has_pending_result_flush());

    // Only the drain clears it, and the drain still reports the mark it took.
    assert!(take_pending_result_flush(), "the drain still sees the mark");
    assert!(
        !has_pending_result_flush(),
        "the drain is what clears it, not the peek"
    );
}

#[test]
fn a_hover_mark_belongs_to_the_root_whose_link_it_names() {
    // Two roots on one thread share epoch integers (each counter starts at 1), so the link and
    // the mark are qualified by root identity; two ids stand in for two `RenderRoot`s.
    const ROOT_A: u64 = 11;
    const ROOT_B: u64 = 22;
    const EPOCH: u64 = 7;

    set_live_hover_link(ROOT_B, EPOCH);
    assert!(
        live_hover_link_is(ROOT_B, EPOCH),
        "the publishing root's pod recognizes its own live link"
    );
    assert!(
        !live_hover_link_is(ROOT_A, EPOCH),
        "the same epoch integer under another root is not this link"
    );
    assert!(
        !live_hover_link_is(ROOT_B, 0),
        "epoch 0 is 'no link' and matches nothing"
    );

    // A mark raised for one root is not the other's to consume: draining the
    // wrong one must neither report nor clear it.
    mark_hover_orphaned(ROOT_A);
    assert!(
        !take_hover_orphaned(ROOT_B),
        "a root does not end its hover on another root's severance"
    );
    assert!(
        take_hover_orphaned(ROOT_A),
        "and the mark is still standing for the root that owns it"
    );
    assert!(
        !take_hover_orphaned(ROOT_A),
        "the drain is destructive for the matching root"
    );

    // Leave the thread-local at rest for anything else on this thread.
    set_live_hover_link(0, 0);
}

#[test]
fn a_cursor_request_is_one_slot_the_last_writer_owns() {
    // Clear first so nothing earlier on this thread leaks in, as `RenderRoot::event` does per
    // pass.
    clear_cursor_request();
    assert_eq!(take_cursor_request(), None, "absence means Default");

    let mut count = 0u32;
    let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
    ctx.set_cursor(CursorIcon::Text);
    // A second widget on the same routed path speaks later and therefore wins;
    // there is no per-pod recording to merge, only this one slot.
    {
        let mut child = ctx.child_ctx(Point::ZERO, Size::ZERO, false, false, false);
        child.set_cursor(CursorIcon::Pointer);
    }
    assert_eq!(
        take_cursor_request(),
        Some(CursorIcon::Pointer),
        "the last set_cursor of the pass is the resolved one"
    );
    assert_eq!(
        take_cursor_request(),
        None,
        "the drain is destructive — one request per pass"
    );
}

#[test]
fn a_nested_request_pass_resolves_its_own_and_hands_the_slot_back() {
    // A nested pass must neither eat the request the outer pass collected nor drain one the
    // outer pass is still owed; the bracket enforces that rather than a convention.
    let outer = RequestPass::enter();
    let mut count = 0u32;
    {
        let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
        ctx.set_cursor(CursorIcon::Grab);
    }
    // A nested pass starts from absence, like any other — and draining it
    // ends it, handing the enclosing pass's request straight back.
    assert_eq!(
        RequestPass::enter().take().cursor,
        None,
        "a nested pass starts from absence, like any other"
    );
    {
        let inner = RequestPass::enter();
        let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
        ctx.set_cursor(CursorIcon::Text);
        drop(ctx);
        assert_eq!(
            inner.take().cursor,
            Some(CursorIcon::Text),
            "and resolves exactly what was asked inside it"
        );
    }
    // `take` ends the outermost pass: the slot is cleared, not restored, so a `set_cursor` made
    // outside any pass cannot leak into the next.
    assert_eq!(
        outer.take().cursor,
        Some(CursorIcon::Grab),
        "the enclosing pass's request survived the nested one"
    );
    {
        let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
        ctx.set_cursor(CursorIcon::NotAllowed);
    }
    let next = RequestPass::enter();
    assert_eq!(
        next.take().cursor,
        None,
        "a request made between passes belongs to no pass"
    );
}

#[test]
fn the_default_cursor_is_the_platform_arrow() {
    // `Default::default()` is what an absent request resolves to at the root,
    // so the derive must land on the arrow and not on some named shape.
    assert_eq!(CursorIcon::default(), CursorIcon::Default);
}

#[test]
fn a_nested_request_pass_hands_the_clipboard_slots_back_too() {
    // The cursor's reentrancy proof for the two channels sharing its bracket: a nested pass
    // returns the enclosing pass's undrained copy and unanswered paste request.
    let outer = RequestPass::enter();
    let mut count = 0u32;
    {
        let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
        ctx.write_clipboard("outer".to_string());
        ctx.request_paste();
    }
    {
        let inner = RequestPass::enter();
        let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
        ctx.write_clipboard("inner".to_string());
        drop(ctx);
        let resolved = inner.take();
        assert_eq!(
            resolved.clipboard_write.as_deref(),
            Some("inner"),
            "a nested pass resolves exactly what was written inside it"
        );
        assert!(
            !resolved.paste_request,
            "and starts from absence rather than inheriting the enclosing ask"
        );
    }
    let resolved = outer.take();
    assert_eq!(
        resolved.clipboard_write.as_deref(),
        Some("outer"),
        "the enclosing pass's write survived the nested one"
    );
    assert!(resolved.paste_request, "and so did its paste request");

    // Outside any pass now: a stray write belongs to nobody.
    {
        let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
        ctx.write_clipboard("stray".to_string());
        ctx.request_paste();
    }
    let next = RequestPass::enter().take();
    assert_eq!(next.clipboard_write, None);
    assert!(!next.paste_request);
}

#[test]
fn the_last_clipboard_write_of_a_pass_wins() {
    let pass = RequestPass::enter();
    let mut count = 0u32;
    {
        let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
        ctx.write_clipboard("container".to_string());
        ctx.write_clipboard("leaf".to_string());
    }
    assert_eq!(
        pass.take().clipboard_write.as_deref(),
        Some("leaf"),
        "one write is resolved per pass, and the last caller is it"
    );
}

#[test]
fn a_paste_request_is_idempotent_within_a_pass() {
    let pass = RequestPass::enter();
    let mut count = 0u32;
    {
        let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
        ctx.request_paste();
        ctx.request_paste();
    }
    // Data-free: two asks owe one clipboard read, and the flag cannot
    // represent anything else.
    assert!(pass.take().paste_request);
}

#[test]
fn an_edit_command_is_focus_routed_with_zero_position() {
    let copy = InputEvent::EditCommand(EditCommand::Copy);
    assert!(copy.is_focus_routed(), "a clipboard verb follows the focus");
    assert!(!copy.is_broadcast(), "and is not a broadcast");
    assert_eq!(copy.position(), Point::ZERO);
    assert_eq!(
        copy.translated(Vec2::new(10.0, 20.0)),
        copy,
        "a positionless event is returned unchanged by a container's translate"
    );
}

/// An overlay event standing in for one routed into a floated surface.
fn overlay(kind: OverlayEventKind) -> InputEvent {
    InputEvent::Overlay(OverlayEvent {
        key: OverlayKey::next(),
        kind,
    })
}

#[test]
fn overlay_is_a_broadcast_and_housekeeping_is_the_only_other_one() {
    let routed = overlay(OverlayEventKind::Pointer(PointerEvent {
        phase: PointerPhase::Down,
        position: Point::new(120.0, 80.0),
        button: PointerButton::Primary,
    }));
    assert!(
        routed.is_broadcast(),
        "an overlay event reaches its owner by broadcast, wherever the owner sits"
    );
    assert!(
        !routed.is_focus_routed(),
        "and not down the focus chain — the surface's owner need not be focused"
    );
    assert!(InputEvent::Housekeeping.is_broadcast());
    // (A file drop's `Ended` follow-up is the third; see
    // `only_the_ended_file_drop_phase_is_a_positionless_broadcast`.)

    // ...and nothing else is. Spelled as an exhaustive walk rather than three
    // spot checks, so a variant added later has to state its own answer here.
    for event in [
        InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Down,
            position: Point::ZERO,
            button: PointerButton::Primary,
        }),
        InputEvent::Scroll {
            position: Point::ZERO,
            delta: ScrollDelta::Lines(0.0, 1.0),
        },
        InputEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        }),
        InputEvent::Ime(ImeEvent::Enabled),
        InputEvent::EditCommand(EditCommand::Copy),
    ] {
        assert!(
            !event.is_broadcast(),
            "only Housekeeping and Overlay broadcast, but {event:?} claims to"
        );
    }
}

#[test]
fn an_overlay_event_carries_no_local_position_and_is_never_translated() {
    let routed = overlay(OverlayEventKind::Pointer(PointerEvent {
        phase: PointerPhase::Move,
        position: Point::new(120.0, 80.0),
        button: PointerButton::Primary,
    }));
    assert_eq!(
        routed.position(),
        Point::ZERO,
        "a broadcast is never hit-tested, so it reports no position to hit-test on"
    );
    // The payload is in window space and the container chain describes where the owner sits,
    // so translating it would corrupt it.
    assert_eq!(
        routed.translated(Vec2::new(-10.0, -20.0)),
        routed,
        "the container chain must not shift a window-space payload"
    );
    let scrolled = overlay(OverlayEventKind::Scroll {
        position: Point::new(120.0, 80.0),
        delta: ScrollDelta::Pixels(0.0, 12.0),
    });
    assert_eq!(scrolled.translated(Vec2::new(5.0, 5.0)), scrolled);
    let outside = overlay(OverlayEventKind::OutsideDown);
    assert_eq!(outside.position(), Point::ZERO);
    assert_eq!(outside.translated(Vec2::new(5.0, 5.0)), outside);
}

#[test]
fn edit_commands_drain_in_dispatch_order_within_one_pass() {
    let pass = RequestPass::enter();
    let mut count = 0u32;
    let taken = {
        let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
        // The toolbar's "cut" answered as two verbs: order is meaning.
        ctx.dispatch_edit_command(EditCommand::Copy);
        ctx.dispatch_edit_command(EditCommand::SelectAll);
        ctx.dispatch_edit_command(EditCommand::Cut);
        ctx.take_edit_commands()
    };
    assert_eq!(
        taken,
        vec![EditCommand::Copy, EditCommand::SelectAll, EditCommand::Cut],
        "a FIFO, not a last-writer-wins slot"
    );

    // The drain empties the queue, so a second owner forwarding in the same
    // pass cannot re-apply the first owner's verbs.
    let mut second = 0u32;
    let mut ctx = EventCtx::new(&mut second, Point::ZERO, Size::ZERO);
    assert!(ctx.take_edit_commands().is_empty());
    drop(ctx);
    drop(pass.take());
}

#[test]
fn a_leaked_edit_command_is_cleared_with_the_pass_and_never_reaches_the_next() {
    {
        let pass = RequestPass::enter();
        let mut count = 0u32;
        let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
        // Dispatched and never drained: resolving the pass clears it, so a stale `Cut` cannot
        // apply to whatever is selected later.
        ctx.dispatch_edit_command(EditCommand::Cut);
        drop(ctx);
        drop(pass.take());
    }

    let next = RequestPass::enter();
    let mut count = 0u32;
    let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
    assert!(
        ctx.take_edit_commands().is_empty(),
        "a leaked command must not survive into the next pass"
    );
    drop(ctx);
    drop(next.take());
}

#[test]
fn a_nested_pass_hands_the_enclosing_passs_edit_commands_back() {
    let outer = RequestPass::enter();
    let mut count = 0u32;
    {
        let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
        ctx.dispatch_edit_command(EditCommand::Copy);
    }
    {
        // A nested dispatch (the overlay pre-pass) starts from an empty queue and must not eat
        // the enclosing pass's undrained command.
        let inner = RequestPass::enter();
        let mut inner_state = 0u32;
        let mut ctx = EventCtx::new(&mut inner_state, Point::ZERO, Size::ZERO);
        assert!(ctx.take_edit_commands().is_empty());
        ctx.dispatch_edit_command(EditCommand::SelectAll);
        assert_eq!(ctx.take_edit_commands(), vec![EditCommand::SelectAll]);
        drop(ctx);
        drop(inner.take());
    }
    let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
    assert_eq!(
        ctx.take_edit_commands(),
        vec![EditCommand::Copy],
        "the enclosing pass's queue is handed back intact"
    );
    drop(ctx);
    drop(outer.take());
}

#[test]
fn debug_redacts_a_pasted_payload() {
    // The clipboard has no content-type hint to key a decision off (see the
    // `Debug` impl), so the payload is redacted unconditionally.
    let rendered = format!("{:?}", EditCommand::Paste("hunter2".to_string()));
    assert!(
        !rendered.contains("hunter2"),
        "paste payload leaked: {rendered}"
    );
    assert!(rendered.contains("<redacted>"));
    assert!(
        rendered.contains("Paste"),
        "the verb still prints: {rendered}"
    );
    // No length either — that leaks too.
    assert!(!rendered.contains('7'));
    assert_eq!(format!("{:?}", EditCommand::SelectAll), "SelectAll");
}

#[test]
fn a_pointer_contact_is_positioned_and_translated_like_a_pointer() {
    let contact = InputEvent::PointerContact {
        pointer_id: PointerId::touch(2),
        event: PointerEvent {
            phase: PointerPhase::Move,
            position: Point::new(30.0, 40.0),
            button: PointerButton::Primary,
        },
    };
    assert_eq!(contact.position(), Point::new(30.0, 40.0));
    assert_eq!(
        contact.translated(Vec2::new(-10.0, -20.0)),
        InputEvent::PointerContact {
            pointer_id: PointerId::touch(2),
            event: PointerEvent {
                phase: PointerPhase::Move,
                position: Point::new(20.0, 20.0),
                button: PointerButton::Primary,
            },
        },
        "the position shifts; the id and the rest of the event do not"
    );
    // Hit-tested, like the pointer it wraps: neither class of non-positional
    // event.
    assert!(!contact.is_focus_routed());
    assert!(!contact.is_broadcast());
}

#[test]
fn pointer_ids_name_the_mouse_and_touch_slots() {
    assert_eq!(
        PointerId::MOUSE,
        PointerId {
            source: PointerSource::Mouse,
            slot: 0
        }
    );
    assert_eq!(
        PointerId::touch(3),
        PointerId {
            source: PointerSource::Touch,
            slot: 3
        }
    );
    assert_ne!(PointerId::MOUSE, PointerId::touch(0));
}

#[test]
fn a_context_reports_the_mouse_outside_any_contact_pass() {
    let mut count = 0u32;
    let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
    assert_eq!(ctx.pointer_id(), PointerId::MOUSE);
    // An opt-in with no pass open is still visible to the container that
    // reads it, but records nothing a later pass could inherit.
    ctx.capture_contacts();
    assert!(ctx.is_contact_capture_requested());
    drop(ctx);
    let pass = ContactPass::enter(PointerId::touch(0), false);
    assert!(!pass.contacts_requested());
}

#[test]
fn a_contact_pass_seeds_every_context_and_collects_the_opt_in() {
    let mut count = 0u32;
    let outer = ContactPass::enter(PointerId::touch(1), true);
    assert!(in_secondary_contact_pass());
    {
        // A fresh context (what a component builds over its own state)
        // reports the pass's contact, and so does every child context.
        let mut ctx = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
        assert_eq!(ctx.pointer_id(), PointerId::touch(1));
        let (contacts, id) = {
            let mut child = ctx.child_ctx(Point::ZERO, Size::ZERO, false, false, false);
            let id = child.pointer_id();
            child.capture_contacts();
            (child.is_contact_capture_requested(), id)
        };
        assert_eq!(id, PointerId::touch(1));
        ctx.absorb_child(ChildFold {
            contacts_requested: contacts,
            ..ChildFold::default()
        });
        assert!(ctx.is_contact_capture_requested(), "the opt-in bubbles");
    }
    assert!(outer.contacts_requested(), "and is recorded in the pass");

    // A nested pass scopes its own contact and opt-in, then hands back.
    {
        let inner = ContactPass::enter(PointerId::MOUSE, false);
        assert_eq!(current_pointer_id(), PointerId::MOUSE);
        assert!(!in_secondary_contact_pass());
        assert!(!inner.contacts_requested());
    }
    assert_eq!(current_pointer_id(), PointerId::touch(1));
    assert!(in_secondary_contact_pass());
    assert!(outer.contacts_requested());
    drop(outer);
    assert_eq!(current_pointer_id(), PointerId::MOUSE);
    assert!(!in_secondary_contact_pass());
}

#[test]
fn key_event_activation_is_a_bare_fresh_space_or_enter() {
    let press = |key: Key, modifiers: Modifiers, repeat: bool| {
        KeyEvent {
            key,
            modifiers,
            repeat,
        }
        .is_activation()
    };
    let none = Modifiers::default();
    let space = || Key::Character(" ".to_string());
    assert!(press(space(), none, false));
    assert!(press(Key::Named(NamedKey::Enter), none, false));
    assert!(!press(space(), none, true), "an auto-repeat");
    let ctrl = Modifiers { ctrl: true, ..none };
    assert!(!press(Key::Named(NamedKey::Enter), ctrl, false));
    assert!(!press(Key::Character("a".to_string()), none, false));
    assert!(!press(Key::Named(NamedKey::Tab), none, false));
}

#[test]
fn function_key_tables_are_inverses_over_f1_to_f24() {
    for n in 1..=24 {
        assert_eq!(
            NamedKey::function(n).and_then(NamedKey::function_number),
            Some(n),
            "F{n} round-trips"
        );
    }
    assert!(NamedKey::function(0).is_none() && NamedKey::function(25).is_none());
    assert_eq!(NamedKey::Enter.function_number(), None);
}
