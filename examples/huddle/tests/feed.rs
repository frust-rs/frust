//! Channel-feed integration tests.
//!
//! Two layers:
//!
//! - **Controller behavior** (`#[tokio::test]`, driving
//!   [`MessagesController`](huddle::features::messages::MessagesController)
//!   directly with short mock latencies): load populates the feed, a send
//!   appends immediately and a canned reply arrives after advancing time, a
//!   reaction toggle updates the count, and `#firehose`'s `load_older`
//!   pagination walks back to the whole channel.
//! - **Render** (the headless `support` harness): the screen mounts and paints
//!   its bubbles + composer through the real `RenderRoot`, and the multiline
//!   composer field grows in height as newlines are inserted.
//!
//! Tests that touch process-global reactive state (the background executor's
//! timing) take the [`support::serial`] lock first, per the harness docs.

use std::time::{Duration, Instant};

use frust::{AnyView, Component, FrameTime, GetUntracked, Set, TextInputView, text_input};
use frust_core::{NamedKey, RenderRoot};
use frust_reactive::ReactiveRuntime;
use frust_text::TextContext;
use kurbo::Size;

use huddle::data::store;
use huddle::features::messages::{MessagesController, PAGE_SIZE};
use huddle::{HuddleApp, HuddleState};

mod support;
use support::{W, center, char_key, frame, named_key, serial, setup, tap};

/// A short-latency controller so a load / send / reply round trip is fast.
fn fast(channel: &str) -> MessagesController {
    let d = Duration::from_millis(4);
    MessagesController::with_latency(channel, d, d, d)
}

// ---------------------------------------------------------------------------
// Controller behavior
// ---------------------------------------------------------------------------

#[tokio::test]
async fn load_populates_the_channel_feed() {
    let controller = fast("general");
    assert!(controller.snapshot().is_empty(), "empty before load");

    controller.load().await;

    assert_eq!(
        controller.snapshot().len(),
        store::messages_for("general").len(),
        "a normal channel loads whole",
    );
    assert!(
        !controller.has_more.get_untracked(),
        "a normal channel does not paginate",
    );
    controller.as_ref().dispose();
}

#[tokio::test]
async fn send_appends_immediately_then_the_canned_reply_arrives() {
    let controller = fast("general");
    controller.load().await;
    let before = controller.snapshot().len();

    // Immediate append (the composer clears the instant this returns).
    let id = controller.send_now("shipping now");
    let after_send = controller.snapshot();
    assert_eq!(after_send.len(), before + 1, "the send appends immediately");
    assert_eq!(after_send.last().unwrap().id, id);
    assert!(
        after_send.last().unwrap().is_own(),
        "authored by the current user"
    );

    // Advancing time (awaiting the reply latency + typing interval) lands the
    // canned reply from another member.
    controller.deliver_reply().await;
    let after_reply = controller.snapshot();
    assert_eq!(
        after_reply.len(),
        before + 2,
        "the canned reply is appended"
    );
    assert!(
        !after_reply.last().unwrap().is_own(),
        "reply from another member"
    );
    assert!(
        !controller.typing.get_untracked(),
        "typing cleared afterwards"
    );
    controller.as_ref().dispose();
}

#[tokio::test]
async fn reaction_toggle_updates_the_count() {
    let controller = fast("general");
    controller.load().await;
    let target = controller.snapshot()[0].id;

    let count_of = |emoji: &str| {
        controller
            .message(target)
            .unwrap()
            .reactions
            .into_iter()
            .find(|r| r.emoji == emoji)
            .map(|r| r.count)
            .unwrap_or(0)
    };

    let base = count_of("\u{1F525}");
    controller.toggle_reaction(target, "\u{1F525}");
    assert_eq!(
        count_of("\u{1F525}"),
        base + 1,
        "toggling on bumps the count"
    );
    controller.toggle_reaction(target, "\u{1F525}");
    assert_eq!(count_of("\u{1F525}"), base, "toggling off reverts it");
    controller.as_ref().dispose();
}

#[tokio::test]
async fn firehose_pagination_extends_the_feed() {
    let controller = fast(store::FIREHOSE_ID);
    controller.load().await;
    assert_eq!(
        controller.snapshot().len(),
        PAGE_SIZE,
        "the newest page loads first",
    );
    assert!(
        controller.has_more.get_untracked(),
        "#firehose has older pages"
    );

    let mut guard = 0;
    while controller.has_more.get_untracked() {
        controller.load_older().await;
        guard += 1;
        assert!(guard < 20, "pagination terminates");
    }
    assert_eq!(
        controller.snapshot().len(),
        store::FIREHOSE_COUNT as usize,
        "load_older walks back to the whole firehose",
    );
    controller.as_ref().dispose();
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

type AppRoot = RenderRoot<HuddleState, AnyView<HuddleState>>;

/// Pushing `/channel/:id` mounts the feed screen, which loads and paints its
/// message bubbles (many text glyph runs) plus the composer chrome.
#[test]
fn feed_screen_loads_and_renders_bubbles() {
    let _g = serial();
    let _ambient = setup();

    let mut root: AppRoot = RenderRoot::new();
    let mut state = HuddleApp.init();
    let mut logic = |s: &mut HuddleState| HuddleApp.build(s);
    let mut tcx = TextContext::new();

    frust::provide_context(frust::Theme::m3_baseline());
    state.nav.router().push("/channel/general");

    // The first frame paints the loading state (app bar + composer, few glyphs).
    let mut scene = frame(&mut root, &mut logic, &mut state, &mut tcx);

    // Pump frames until the background load populates the feed (or a deadline).
    let runtime = ReactiveRuntime::get().expect("runtime installed by setup()");
    let deadline = Instant::now() + Duration::from_secs(4);
    while scene.glyph_runs < 12 && Instant::now() < deadline {
        runtime.pump_local();
        std::thread::sleep(Duration::from_millis(3));
        scene = frame(&mut root, &mut logic, &mut state, &mut tcx);
    }

    assert!(
        scene.glyph_runs >= 12,
        "the loaded feed renders message bubbles (got {} glyph runs)",
        scene.glyph_runs,
    );
    assert!(
        !scene.rounded.is_empty(),
        "the composer field / cards paint rounded chrome",
    );
}

/// The multiline composer field grows in height as newlines are inserted — the
/// `TextInput::multiline` growth the composer is built on.
#[test]
fn multiline_composer_grows_with_newlines() {
    let _g = serial();
    let _ambient = setup();

    let mut root: RenderRoot<String, TextInputView<String>> = RenderRoot::new();
    let mut state = String::new();
    let mut logic = |s: &mut String| {
        text_input(s.clone(), |st: &mut String, next: String| *st = next).multiline(5)
    };
    let mut tcx = TextContext::new();

    // First frame + focus the field (its chrome is the recorded rounded rect).
    let scene = frame(&mut root, &mut logic, &mut state, &mut tcx);
    let field = *scene.rounded.first().expect("the field paints its chrome");
    tap(&mut root, &mut state, center(field));

    // One line of text: the baseline height.
    root.event(&mut state, &char_key("first line"));
    let scene1 = frame(&mut root, &mut logic, &mut state, &mut tcx);
    let h1 = scene1.rounded.first().expect("field chrome").1.height;

    // Two inserted newlines grow the field to three visible lines.
    root.event(&mut state, &named_key(NamedKey::Enter));
    root.event(&mut state, &char_key("second line"));
    root.event(&mut state, &named_key(NamedKey::Enter));
    root.event(&mut state, &char_key("third line"));
    let scene2 = frame(&mut root, &mut logic, &mut state, &mut tcx);
    let h2 = scene2.rounded.first().expect("field chrome").1.height;

    assert!(
        h2 > h1,
        "the multiline composer grew from {h1} to {h2} as newlines were inserted",
    );
    // The submitted point is well within the window (guards against the field
    // being pushed off-screen by unbounded growth past the 5-line cap).
    assert!(h2 < W, "the field stays a sane height");
}

/// The keyed feed list with loading_older and typing singleton rows alongside
/// keyed message rows should not panic about mixed keyed/unkeyed children. The
/// singleton rows use collision-proof string keys ("loading_older", "typing")
/// distinct from message ids (u32), so all children are keyed (the list opts
/// into keyed reconciliation).
#[test]
fn keyed_feed_list_with_loading_and_typing_indicators() {
    let _g = serial();
    let _ambient = setup();

    let mut root: AppRoot = RenderRoot::new();
    let mut state = HuddleApp.init();
    let mut logic = |s: &mut HuddleState| HuddleApp.build(s);
    let mut tcx = TextContext::new();

    frust::provide_context(frust::Theme::m3_baseline());
    state.nav.router().push("/channel/general");

    // Load the feed (the first frame is the loading state).
    let mut t_ms = 0u64;
    let (_scene, mut needs_frame) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, t_ms);

    // Pump frames until the background load populates the feed.
    let runtime = ReactiveRuntime::get().expect("runtime installed by setup()");
    let deadline = Instant::now() + Duration::from_secs(5);
    while needs_frame && Instant::now() < deadline {
        runtime.pump_local();
        std::thread::sleep(Duration::from_millis(1));
        t_ms += 16;
        let result = frame_at(&mut root, &mut logic, &mut state, &mut tcx, t_ms);
        needs_frame = result.1;
        if !result.1 {
            break;
        }
    }

    // Set loading_older and typing to true to trigger the singleton rows
    // alongside the keyed message rows. This would panic if the rows were
    // unkeyed (the keyed reconciler requires all or nothing).
    let controller = MessagesController::for_channel("general");
    controller.loading_older.set(true);
    controller.typing.set(true);

    // Render a frame with both indicators and messages present.
    // This tests that the keyed list handles the mixed singleton/keyed children correctly.
    let (scene, _) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, t_ms);

    // The feed should still render (at least the composed UI elements are laid out).
    assert!(
        scene.glyph_runs > 0,
        "feed with loading+typing indicators renders without panic"
    );
}

/// Local frame_at helper — paint at a specific clock time (in milliseconds)
/// to test settled-clock rendering. This is a minimal version of the pattern
/// in `tests/thread.rs` / `tests/activity.rs`.
fn frame_at(
    root: &mut AppRoot,
    logic: &mut impl FnMut(&mut HuddleState) -> AnyView<HuddleState>,
    state: &mut HuddleState,
    tcx: &mut TextContext,
    t_ms: u64,
) -> (support::RecScene, bool) {
    root.rebuild(logic, state);
    let tcx_any: &mut dyn std::any::Any = tcx;
    root.layout_with_text(Size::new(support::W, support::H), tcx_any);
    let mut scene = support::RecScene::default();
    let outcome = root.paint(&mut scene, FrameTime::from_nanos(t_ms * 1_000_000));
    (scene, outcome.needs_frame)
}
