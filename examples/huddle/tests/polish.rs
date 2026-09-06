//! Polish integration tests: swipe-to-reply opens a message's thread,
//! a feed avatar tap opens the author's profile, an empty DM shows the
//! empty-conversation state, and the workspace drawer's entrance self-heals
//! across a disposed reactive owner (a previously reported flake).
//!
//! Like `tests/actions.rs`/`tests/feed.rs` these drive the whole mounted
//! [`HuddleApp`] through its real navigator and assert black-box against
//! [`RecScene`]'s recorded geometry plus the shared
//! [`MessagesController`](huddle::features::messages::MessagesController)
//! signals — no GPU, no window. Every test takes the [`support::serial`] lock
//! first (the mocked loads run on the shared background reactive runtime).

use std::any::Any;
use std::time::{Duration, Instant};

use frust::{AnyView, Component, GetUntracked};
use frust_core::{FrameTime, PointerPhase, RenderRoot};
use frust_reactive::ReactiveRuntime;
use frust_text::TextContext;
use kurbo::{Point, Size};

use huddle::features::messages::MessagesController;
use huddle::{HuddleApp, HuddleState};

mod support;
use support::{RecScene, W, center, pointer, serial, setup, tap};

type Root = RenderRoot<HuddleState, AnyView<HuddleState>>;

const LOAD_WAIT: Duration = Duration::from_secs(5);

/// Rebuild + layout + paint at `t_ms` on a caller-advanced clock; returns the
/// scene and whether the paint asked for another frame (mirrors
/// `tests/actions.rs::frame_at`).
fn frame_at(
    root: &mut Root,
    logic: &mut impl FnMut(&mut HuddleState) -> AnyView<HuddleState>,
    state: &mut HuddleState,
    tcx: &mut TextContext,
    t_ms: u64,
) -> (RecScene, bool) {
    root.rebuild(logic, state);
    let tcx_any: &mut dyn Any = tcx;
    root.layout_with_text(Size::new(W, support::H), tcx_any);
    let mut scene = RecScene::default();
    let outcome = root.paint(&mut scene, FrameTime::from_nanos(t_ms * 1_000_000));
    (scene, outcome.needs_frame)
}

/// Advance the clock (pumping the local queue + a real tick each iteration)
/// until no animation asks for another frame — settles a load or a nav
/// cross-fade (mirrors `tests/actions.rs::settle`).
fn settle(
    root: &mut Root,
    logic: &mut impl FnMut(&mut HuddleState) -> AnyView<HuddleState>,
    state: &mut HuddleState,
    tcx: &mut TextContext,
    t_ms: &mut u64,
) -> RecScene {
    let runtime = ReactiveRuntime::get().expect("setup() installed the reactive runtime");
    let deadline = Instant::now() + LOAD_WAIT;
    loop {
        runtime.pump_local();
        std::thread::sleep(Duration::from_millis(1));
        *t_ms += 16;
        let (scene, needs_frame) = frame_at(root, logic, state, tcx, *t_ms);
        if !needs_frame {
            return scene;
        }
        assert!(
            Instant::now() < deadline,
            "a transition/animation never settled"
        );
    }
}

/// Push `/channel/{channel}`, pump until its controller finishes loading (a
/// skeleton feed paints no bubbles but is still `loading`, so gate on the
/// controller signal, not on glyph counts — an empty DM never reaches the
/// bubble-glyph threshold `tests/actions.rs::mount_feed` uses), then settle the
/// ensuing cross-fade. Leaves the feed live and interactive.
fn mount_feed(
    channel: &str,
) -> (
    Root,
    HuddleState,
    impl FnMut(&mut HuddleState) -> AnyView<HuddleState>,
    TextContext,
    u64,
    RecScene,
) {
    let mut root: Root = RenderRoot::new();
    let mut state = HuddleApp.init();
    let mut logic = |s: &mut HuddleState| HuddleApp.build(s);
    let mut tcx = TextContext::new();

    root.rebuild(&mut logic, &mut state);
    state.nav.router().push(&format!("/channel/{channel}"));
    root.rebuild(&mut logic, &mut state);

    let controller = MessagesController::for_channel(channel);
    let runtime = ReactiveRuntime::get().expect("setup() installed the reactive runtime");
    let deadline = Instant::now() + LOAD_WAIT;
    let mut t_ms = 50u64;
    let scene = loop {
        runtime.pump_local();
        std::thread::sleep(Duration::from_millis(1));
        t_ms += 16;
        let (scene, needs_frame) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, t_ms);
        if !needs_frame && !controller.loading.get_untracked() {
            break scene;
        }
        assert!(
            Instant::now() < deadline,
            "the feed did not load within the deadline"
        );
    };

    (root, state, logic, tcx, t_ms, scene)
}

/// The topmost (other-user) message row anchor. The feed rows are FLAT (no
/// `filled_card`/`elevated_card` bubble background), so the row's only
/// recorded rounded chrome is its leading 40px avatar disc — the leftmost
/// small rounded rect in the content region. Its `y` is the row's top, a
/// safe row target.
fn first_bubble(scene: &RecScene) -> (Point, Size) {
    scene
        .rounded
        .iter()
        .copied()
        .filter(|(o, s)| o.y > 70.0 && o.y < 300.0 && s.width < 80.0)
        .min_by(|a, b| a.0.x.partial_cmp(&b.0.x).unwrap())
        .expect("the loaded feed paints a leading avatar on the topmost row")
}

/// The leading avatar of the topmost (other-user) message row — its center is a
/// tap target opening the author's profile. Same rect [`first_bubble`] anchors
/// on (the flat row's avatar disc).
fn first_avatar(scene: &RecScene) -> Point {
    center(first_bubble(scene))
}

/// Whether a message row is painted in the content region — false for an empty
/// conversation's empty state. A loaded feed paints a leading avatar disc
/// (small rounded rect) per other-user row; the empty state paints none.
fn has_message_bubble(scene: &RecScene) -> bool {
    scene
        .rounded
        .iter()
        .any(|(o, s)| o.y > 70.0 && o.y < 300.0 && s.width < 80.0)
}

// --- Tests -----------------------------------------------------------------

/// A committed swipe-right on an other-user's message row opens that message's
/// thread — reproducing exactly what a direct `/thread/:id` push renders.
#[test]
fn swipe_to_reply_opens_the_thread() {
    let _g = serial();
    let _ambient = setup();

    let (mut root, mut state, mut logic, mut tcx, mut t_ms, feed) = mount_feed("general");

    // Reference: the topmost bubble is message id 1 (Grace Hopper, not own).
    // Directly push its thread, capture the settled signature, then pop back.
    state.nav.router().push("/thread/1");
    let thread_ref = settle(&mut root, &mut logic, &mut state, &mut tcx, &mut t_ms);
    assert!(
        thread_ref.rounded.len() < feed.rounded.len(),
        "the thread screen is structurally simpler than the feed it came from",
    );
    state.nav.router().pop();
    let feed_again = settle(&mut root, &mut logic, &mut state, &mut tcx, &mut t_ms);

    // Swipe right on the topmost row: Down, a Move past the slop (takeover), a
    // Move well past the commit fraction (0.35 * 800 == 280px), then Up.
    let y = first_bubble(&feed_again).0.y + 20.0;
    root.event(
        &mut state,
        &pointer(PointerPhase::Down, Point::new(120.0, y)),
    );
    root.event(
        &mut state,
        &pointer(PointerPhase::Move, Point::new(200.0, y)),
    );
    root.event(
        &mut state,
        &pointer(PointerPhase::Move, Point::new(700.0, y)),
    );
    root.event(&mut state, &pointer(PointerPhase::Up, Point::new(700.0, y)));

    let swiped = settle(&mut root, &mut logic, &mut state, &mut tcx, &mut t_ms);
    assert_eq!(
        (swiped.rounded.len(), swiped.glyph_runs),
        (thread_ref.rounded.len(), thread_ref.glyph_runs),
        "the swipe reproduced the same screen a direct /thread/1 push renders",
    );
}

/// Tapping a feed message's avatar opens the author's profile — reproducing what
/// a direct `/user/:id` push renders.
#[test]
fn feed_avatar_tap_opens_the_profile() {
    let _g = serial();
    let _ambient = setup();

    let (mut root, mut state, mut logic, mut tcx, mut t_ms, _feed) = mount_feed("general");

    // Reference: the topmost bubble's author is user 2 — directly push their
    // profile, capture its signature, then pop back to the feed.
    state.nav.router().push("/user/2");
    let user_ref = settle(&mut root, &mut logic, &mut state, &mut tcx, &mut t_ms);
    state.nav.router().pop();
    let feed_again = settle(&mut root, &mut logic, &mut state, &mut tcx, &mut t_ms);

    let avatar = first_avatar(&feed_again);
    tap(&mut root, &mut state, avatar);
    let tapped = settle(&mut root, &mut logic, &mut state, &mut tcx, &mut t_ms);

    assert_eq!(
        (tapped.rounded.len(), tapped.glyph_runs),
        (user_ref.rounded.len(), user_ref.glyph_runs),
        "the avatar tap reproduced the same screen a direct /user/2 push renders",
    );
}

/// An empty DM (`dm-9`, no messages in the mock dataset) shows the
/// empty-conversation state — text, no message bubbles.
#[test]
fn empty_dm_shows_the_empty_state() {
    let _g = serial();
    let _ambient = setup();

    let (mut _root, _state, mut _logic, mut _tcx, _t_ms, scene) = mount_feed("dm-9");

    let controller = MessagesController::for_channel("dm-9");
    assert!(
        controller.snapshot().is_empty(),
        "dm-9 loads with no messages",
    );
    assert!(
        scene.glyph_runs > 0,
        "the empty DM still renders the empty-state text + app bar + composer",
    );
    assert!(
        !has_message_bubble(&scene),
        "an empty conversation paints no message bubbles",
    );
}

/// The workspace drawer's `entrance_progress` thread-local self-heals: mounting
/// the drawer, disposing the reactive owner, then re-mounting on the *same*
/// thread under a fresh owner must not panic on the now-disposed cached signal
/// (a previously reported flake, hardened here).
#[test]
fn drawer_entrance_survives_a_disposed_owner() {
    let _g = serial();

    // Block 1: mount, show the drawer (creating the cached entrance signal under
    // this owner), then dispose the owner by dropping everything.
    {
        let ambient = setup();
        let mut root: Root = RenderRoot::new();
        let mut state = HuddleApp.init();
        let mut logic = |s: &mut HuddleState| HuddleApp.build(s);
        let mut tcx = TextContext::new();

        root.rebuild(&mut logic, &mut state);
        state.nav.router().push("/workspace-switcher");
        // A couple of frames so the drawer page builds and `entrance_progress`
        // creates its cached signal under this owner.
        frame_at(&mut root, &mut logic, &mut state, &mut tcx, 66);
        frame_at(&mut root, &mut logic, &mut state, &mut tcx, 82);

        drop(root);
        drop(state);
        drop(ambient);
    }

    // Block 2: fresh owner + fresh mount on the SAME thread. The cached entrance
    // signal is now disposed; `entrance_progress` must recreate it rather than
    // hand back a disposed signal that panics on the next get/set.
    {
        let ambient = setup();
        let mut root: Root = RenderRoot::new();
        let mut state = HuddleApp.init();
        let mut logic = |s: &mut HuddleState| HuddleApp.build(s);
        let mut tcx = TextContext::new();

        root.rebuild(&mut logic, &mut state);
        state.nav.router().push("/workspace-switcher");
        let mut t_ms = 50u64;
        let scene = settle(&mut root, &mut logic, &mut state, &mut tcx, &mut t_ms);
        assert!(
            scene.glyph_runs > 0,
            "the drawer re-renders after owner disposal (self-heal, no panic)",
        );

        drop(ambient);
    }
}
