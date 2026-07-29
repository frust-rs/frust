//! Thread screen tests.
//!
//! The screen is a private `mod screens` item (`src/lib.rs`, per
//! `src/README-phase-c.md`'s hub-file contract), so — mirroring
//! `tests/activity.rs`/`tests/search.rs` — these tests drive the whole
//! mounted [`HuddleApp`] and navigate through its real
//! [`frust::NavigatorController`] rather than constructing the screen's
//! view function directly. Paint output is asserted the same black-box way
//! every other screen test in this suite does (`RecScene`'s glyph and
//! rounded-rect geometry — no GPU, no window).
//!
//! Root message id `8` (`#engineering`, a plain `Text` body with no
//! reactions — see `src/mock/mod.rs`) is used throughout: its id is a
//! multiple of 4, so `features::messages::FeedMessage::from_mock` seeds it
//! with two canned replies, and its plain body/no-reactions shape keeps the
//! painted rounded-rect count simple to reason about (see
//! `thread_renders_root_and_existing_replies`'s doc comment for the exact
//! tally — see `screens::thread`'s module docs for the visual vocabulary).
//!
//! # Settling the boot/push cross-fades before driving input
//!
//! Exactly like `tests/activity.rs`/`tests/search.rs`: the shell's navigator
//! runs an `M3FadeThrough` cross-fade at boot and again on every push, and a
//! transition in flight suppresses ALL page input, so a composer/back tap
//! dispatched mid-transition never reaches the thread screen.
//! [`mount_thread`] therefore advances the paint clock (via the local
//! [`frame_at`]) while it pumps the loader, waiting for the load to resolve
//! *and* the cross-fade to settle together before a test starts tapping (see
//! its own doc comment); [`settle_transitions`] is the same wait, reused after
//! a later push/pop.
//!
//! # The cross-page consistency gap closed
//!
//! An earlier version of this suite documented (but deliberately did not
//! assert) a limitation: `screens::thread` and `screens::channel_feed` each
//! constructed their own independent `MessagesController` for the same
//! channel, so a reply composed here never showed up on an already-open
//! feed's "N replies" affordance. That gap was closed with
//! [`MessagesController::for_channel`](huddle::features::messages::MessagesController::for_channel),
//! a shared per-channel-id registry both screens now source their instance
//! from — see that module's docs. `a_reply_composed_in_the_thread_is_visible_on_the_feeds_shared_controller`
//! below is the real cross-page assertion this suite previously deferred.

use std::any::Any;
use std::time::{Duration, Instant};

use frust::{AnyView, Component};
use frust_core::{FrameTime, InputEvent, Key, KeyEvent, Modifiers, NamedKey, RenderRoot};
use frust_reactive::ReactiveRuntime;
use frust_text::TextContext;
use kurbo::{Point, Size};

use huddle::features::messages::MessagesController;
use huddle::{HuddleApp, HuddleState};

mod support;
use support::{RecScene, W, center, char_key, serial, setup, tap};

type Root = RenderRoot<HuddleState, AnyView<HuddleState>>;

/// How long [`mount_thread`] pumps frames waiting for the thread's mocked
/// load latency to resolve — the same ceiling `tests/activity.rs`'s
/// `LOAD_WAIT` uses.
const LOAD_WAIT: Duration = Duration::from_secs(5);

/// The app bar's back action: a 24dp icon flush to the leading edge, centered
/// 16px in from the left, at the 64dp bar's vertical middle (mirrors
/// `tests/activity.rs`'s `BACK_ACTION` — the same fixed `AppBarView` layout
/// every screen's app bar shares).
const BACK_ACTION: Point = Point::new(16.0, 32.0);

/// A synthetic Shift+Enter — the multiline composer's submit chord (`Enter`
/// alone inserts a newline in multiline mode; see `screens::channel_feed`'s
/// composer doc and `frust-widgets::textinput`'s `submit_on_enter ^
/// modifiers.shift` contract).
fn shift_enter() -> InputEvent {
    InputEvent::Key(KeyEvent {
        key: Key::Named(NamedKey::Enter),
        modifiers: Modifiers {
            shift: true,
            ..Modifiers::default()
        },
        repeat: false,
    })
}

/// Types `text` into the currently-focused field, one `char_key` per
/// character (mirrors `tests/search.rs::type_text`).
fn type_text(root: &mut Root, state: &mut HuddleState, text: &str) {
    for c in text.chars() {
        root.event(state, &char_key(&c.to_string()));
    }
}

/// The lower edge of the thread page's own content — above the persistent
/// 64dp bottom navigation bar (`y ∈ [536, 600]`, mirrors `tests/activity.rs`'s
/// `CONTENT_BOTTOM`), which paints its own selection-indicator pill and would
/// otherwise be mistaken for page chrome.
const CONTENT_BOTTOM: f64 = 520.0;

/// The composer field's chrome: the bottommost rounded rect within the
/// thread page's own content (above [`CONTENT_BOTTOM`]) — the field is the
/// last thing laid out before the persistent bottom bar, and the root
/// header's avatar disc (the only other rounded chrome the flat root/reply
/// rows paint as of task R2b) sits well above it near the header, so
/// "bottommost", not "widest" (unlike `tests/search.rs::field_chrome`, which
/// has no competing chrome), is what isolates it.
fn composer_field_point(scene: &RecScene) -> Point {
    let (origin, size) = scene
        .rounded
        .iter()
        .copied()
        .filter(|(o, _)| o.y < CONTENT_BOTTOM)
        .max_by(|a, b| a.0.y.partial_cmp(&b.0.y).unwrap())
        .expect("the composer field paints its chrome");
    center((origin, size))
}

/// Rebuild + layout + paint like [`support::frame`], but painting at `t_ms` on
/// a caller-advanced clock instead of the shared harness's pinned
/// [`FrameTime::ZERO`]. Returns the recorded scene together with whether the
/// paint asked for another frame (an in-flight cross-fade) — mirrors
/// `tests/activity.rs`/`tests/search.rs`'s helper of the same name.
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

/// Advances the paint clock (via `t_ms`) until no animation asks for another
/// frame — settling a boot/push/pop cross-fade so page input flows again.
/// Unlike `tests/activity.rs`/`tests/search.rs`'s helper of the same name
/// (whose pages have no pending background work), this also pumps the
/// UI-thread local queue and sleeps a real tick each iteration: both
/// `channel_feed` and `screens::thread` kick off a real `MessagesController`
/// load on mount, and a page revealed by a pop (or covered during a push's
/// cross-fade) needs real wall-clock time to resolve that load too, not just
/// paint-clock ticks.
fn settle_transitions(
    root: &mut Root,
    logic: &mut impl FnMut(&mut HuddleState) -> AnyView<HuddleState>,
    state: &mut HuddleState,
    tcx: &mut TextContext,
    t_ms: &mut u64,
) -> RecScene {
    let runtime = ReactiveRuntime::get().expect("setup() installed the reactive runtime");
    // A real-time deadline, not a `t_ms` (paint-clock) ceiling: `t_ms` is
    // threaded cumulatively across a whole test (through however many prior
    // `mount_thread`/`settle_transitions` calls already advanced it well past
    // any single transition's own budget), so only wall-clock time is a
    // meaningful "this is stuck" signal here.
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
            "a boot/push/pop transition never settled"
        );
    }
}

/// Navigates a fresh app to `/channel/{channel_id}` then `/thread/{root_id}`,
/// pumps past the thread's mocked load latency, and settles the ensuing
/// cross-fades — the common setup every test below starts from, leaving the
/// thread page live and accepting input. Returns the root/state/logic/tcx,
/// the current paint-clock reading (so callers keep advancing it), and the
/// loaded scene.
fn mount_thread(
    root_id: u32,
    channel_id: &str,
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

    // Build once at "/" before queuing a push, mirroring
    // `tests/shell.rs::routes_push_and_pop_without_panicking` — the
    // `NavigatorController`'s op queue is drained at the navigator's *next*
    // rebuild, so a push queued before the navigator widget exists has
    // nothing to drain into yet.
    root.rebuild(&mut logic, &mut state);
    state.nav.router().push(&format!("/channel/{channel_id}"));
    root.rebuild(&mut logic, &mut state);
    state.nav.router().push(&format!("/thread/{root_id}"));
    root.rebuild(&mut logic, &mut state);

    // Pump the spawned loader while advancing the paint clock, until BOTH the
    // thread has resolved past its mocked ~650ms load latency (painting the
    // root card + avatar + reply cards + the composer field on top of the
    // loading placeholder's single bottom-bar-pill rect) AND any boot/push
    // cross-fade has settled — checking them together (rather than the load
    // first, then settling separately) matters because a still-in-flight
    // cross-fade paints the covered `channel_feed` page's own chrome too,
    // which would otherwise be mistaken for the thread's.
    let runtime = ReactiveRuntime::get().expect("setup() installed the reactive runtime");
    let deadline = Instant::now() + LOAD_WAIT;
    let mut t_ms = 50u64;
    let scene = loop {
        runtime.pump_local();
        std::thread::sleep(Duration::from_millis(1));
        t_ms += 16;
        let (scene, needs_frame) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, t_ms);
        if !needs_frame && scene.rounded.len() > 1 {
            break scene;
        }
        assert!(
            Instant::now() < deadline,
            "the thread did not load within the deadline"
        );
    };

    (root, state, logic, tcx, t_ms, scene)
}

/// The loaded thread renders the root message plus its two mock-seeded
/// replies, painting four rounded rects: the root header's `fill_box` avatar
/// disc (1 — the root/reply rows themselves are FLAT as of
/// device-parity-round2 task R2b, painting no card chrome of their own), the
/// composer's `TextInput` chrome (2 — a border rect plus a slightly inset
/// fill rect, see `frust-widgets::textinput`'s paint impl), and the
/// persistent bottom navigation bar's own selection-indicator pill (1) — plus
/// real text glyphs for every header/body line. (Pre-R2b this was 7: the
/// root's `elevated_card` + avatar + one `outlined_card` per reply — task R2b
/// de-carded the root/reply rows the same way R2 de-carded the feed, so the
/// count dropped by the 3 removed card fills: 7 − 1 (root card) − 2 (reply
/// cards) = 4.)
#[test]
fn thread_renders_root_and_existing_replies() {
    let _g = serial();
    let _ambient = setup();

    let (_root, _state, _logic, _tcx, _t_ms, scene) = mount_thread(8, "engineering");

    assert!(
        scene.glyph_runs >= 6,
        "the root header/body and two replies render as text (got {} glyph runs)",
        scene.glyph_runs,
    );
    assert_eq!(
        scene.rounded.len(),
        4,
        "root avatar disc + the composer's 2-rect chrome + the bottom bar pill \
         (flat rows paint no card chrome of their own — task R2b)"
    );
}

/// Composing a reply (typing into the composer, then Shift+Enter to submit —
/// see [`shift_enter`]) appends it. As of device-parity-round2 task R2b the
/// reply row is FLAT (no `outlined_card`), so a new reply paints no rounded
/// chrome of its own — the rounded-rect count stays put and the new reply is
/// observed instead through its own author/text glyph runs (deliberate, not a
/// loosened assertion: pre-R2b this asserted `before + 1` off the removed
/// card's fill rect).
#[test]
fn composing_a_reply_appends_it() {
    let _g = serial();
    let _ambient = setup();

    let (mut root, mut state, mut logic, mut tcx, mut t_ms, scene) = mount_thread(8, "engineering");
    let before = scene.rounded.len();

    tap(&mut root, &mut state, composer_field_point(&scene));
    type_text(&mut root, &mut state, "Great point!");
    root.event(&mut state, &shift_enter());

    t_ms += 16;
    let (after, _) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, t_ms);
    assert_eq!(
        after.rounded.len(),
        before,
        "a flat reply row paints no rounded chrome of its own (task R2b)"
    );
    assert!(
        after.glyph_runs > scene.glyph_runs,
        "the new reply's author/text lines render"
    );
}

/// Popping back to the feed rebuilds cleanly (and the feed, independently
/// loaded, renders too), and reopening the *same* thread afterward still
/// shows the composed reply — the shared per-channel-id controller
/// ([`MessagesController::for_channel`](huddle::features::messages::MessagesController::for_channel))
/// persists the reply across the round trip regardless of which screen's
/// visit constructed it.
#[test]
fn back_then_reopening_the_same_thread_keeps_the_composed_reply() {
    let _g = serial();
    let _ambient = setup();

    let (mut root, mut state, mut logic, mut tcx, mut t_ms, scene) = mount_thread(8, "engineering");

    tap(&mut root, &mut state, composer_field_point(&scene));
    type_text(&mut root, &mut state, "Great point!");
    root.event(&mut state, &shift_enter());
    t_ms += 16;
    let (after_send, _) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, t_ms);

    tap(&mut root, &mut state, BACK_ACTION);
    let popped = settle_transitions(&mut root, &mut logic, &mut state, &mut tcx, &mut t_ms);
    assert!(root.root_id().is_some(), "popping back rebuilds cleanly");
    assert!(popped.glyph_runs > 0, "the feed renders after popping back");

    state.nav.router().push("/thread/8");
    let reopened = settle_transitions(&mut root, &mut logic, &mut state, &mut tcx, &mut t_ms);
    assert_eq!(
        reopened.rounded.len(),
        after_send.rounded.len(),
        "reopening the same thread still shows the composed reply"
    );
}

/// Task 19's real cross-page assertion: open the feed, open its thread,
/// compose a reply there, pop back to the feed — and the feed's own
/// controller (fetched the exact way `screens::channel_feed` sources it,
/// [`MessagesController::for_channel`]) already shows the incremented reply
/// count, with no reload. `RecScene` doesn't capture a chip's label text
/// (only glyph-run counts and rounded-rect geometry — see
/// `tests/support/mod.rs`), so a paint-assert on the feed's "N replies"
/// affordance can't distinguish "2 replies" from "3 replies" text; this is a
/// controller-signal assertion instead, reading the exact same
/// `MessagesController::messages` signal `screens::channel_feed::feed_body`
/// renders `FeedMessage::reply_count` from.
#[test]
fn a_reply_composed_in_the_thread_is_visible_on_the_feeds_shared_controller() {
    let _g = serial();
    let _ambient = setup();

    let (mut root, mut state, mut logic, mut tcx, mut t_ms, scene) = mount_thread(8, "engineering");
    let before = MessagesController::for_channel("engineering")
        .message(8)
        .expect("root message exists")
        .reply_count();

    tap(&mut root, &mut state, composer_field_point(&scene));
    type_text(&mut root, &mut state, "Great point!");
    root.event(&mut state, &shift_enter());
    t_ms += 16;
    let _ = frame_at(&mut root, &mut logic, &mut state, &mut tcx, t_ms);

    tap(&mut root, &mut state, BACK_ACTION);
    let popped = settle_transitions(&mut root, &mut logic, &mut state, &mut tcx, &mut t_ms);
    assert!(root.root_id().is_some(), "popping back rebuilds cleanly");
    assert!(popped.glyph_runs > 0, "the feed renders after popping back");

    let after = MessagesController::for_channel("engineering")
        .message(8)
        .expect("root message exists")
        .reply_count();
    assert_eq!(
        after,
        before + 1,
        "the feed's own controller (the same registry entry the thread just wrote to) \
         sees the reply composed in the thread"
    );
}
