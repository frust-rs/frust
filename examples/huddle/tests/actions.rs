//! Message-action integration tests: the long-press context
//! menu, the emoji reaction picker, the mock attachment sheet, and the
//! composer's emoji/attach wiring.
//!
//! Like `tests/thread.rs`/`tests/feed.rs` these drive the whole mounted
//! [`HuddleApp`] through its real navigator and assert black-box against
//! [`RecScene`]'s recorded rounded-rect geometry plus the shared
//! [`MessagesController`] signals — no GPU, no window. The long-press timer is
//! measured across paints (see `frust-widgets`'s `gesture.rs`), so a press is
//! simulated by advancing the paint clock between `Down` and `Up`.
//!
//! # Locating sheet chrome
//!
//! The `crate::ui::sheet` overlay paints a full-panel-width background rect
//! (800px here) that a test anchors on, then its content: full-width-minus-inset
//! action rows (768px) or the emoji grid's per-column cells. `#general`'s feed
//! is mounted; the oldest message renders at the top of the scroll (offset 0),
//! so `snapshot()[0]` is the topmost bubble the tests long-press.

use std::any::Any;
use std::time::{Duration, Instant};

use frust::{AnyView, Component};
use frust_core::{FrameTime, RenderRoot};
use frust_reactive::ReactiveRuntime;
use frust_text::TextContext;
use kurbo::{Point, Size};

use huddle::features::messages::MessagesController;
use huddle::ui::sheet::{EMOJI_COLS, GRAB_HANDLE_H, REACTION_EMOJI};
use huddle::{HuddleApp, HuddleState};

mod support;
use support::{RecScene, W, center, pointer, serial, setup, tap};

use frust_core::PointerPhase;

type Root = RenderRoot<HuddleState, AnyView<HuddleState>>;

const LOAD_WAIT: Duration = Duration::from_secs(5);

/// Rebuild + layout + paint at `t_ms` on a caller-advanced clock; returns the
/// scene and whether the paint asked for another frame (mirrors
/// `tests/thread.rs::frame_at`).
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
/// until no animation asks for another frame — settles a load, a nav cross-fade,
/// or a sheet's slide-in entrance.
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

/// Push `/channel/{channel}`, pump past the mocked load, and settle the ensuing
/// cross-fade — leaving the feed live and interactive.
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
    let mut logic = |s: &mut HuddleState| AnyView::new(HuddleApp.build(s));
    let mut tcx = TextContext::new();

    root.rebuild(&mut logic, &mut state);
    state.nav.router().push(&format!("/channel/{channel}"));
    root.rebuild(&mut logic, &mut state);

    let runtime = ReactiveRuntime::get().expect("setup() installed the reactive runtime");
    let deadline = Instant::now() + LOAD_WAIT;
    let mut t_ms = 50u64;
    let scene = loop {
        runtime.pump_local();
        std::thread::sleep(Duration::from_millis(1));
        t_ms += 16;
        let (scene, needs_frame) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, t_ms);
        // Wait for the real feed (skeletons paint no text; a loaded feed paints
        // many glyph runs) AND the cross-fade to settle. `rounded.len() > 6`
        // alone matches the 6 static loading-skeleton cards, so gate on glyphs.
        if !needs_frame && scene.glyph_runs >= 15 {
            break scene;
        }
        assert!(
            Instant::now() < deadline,
            "the feed did not load within the deadline"
        );
    };

    (root, state, logic, tcx, t_ms, scene)
}

/// Simulate a long-press at `p`: `Down`, advance the paint clock past the
/// ~500ms threshold (recording `press_start` then marking it elapsed), `Up`.
fn long_press(
    root: &mut Root,
    logic: &mut impl FnMut(&mut HuddleState) -> AnyView<HuddleState>,
    state: &mut HuddleState,
    tcx: &mut TextContext,
    t_ms: &mut u64,
    p: Point,
) {
    root.event(state, &pointer(PointerPhase::Down, p));
    *t_ms += 16;
    frame_at(root, logic, state, tcx, *t_ms); // records press_start
    *t_ms += 600;
    frame_at(root, logic, state, tcx, *t_ms); // marks elapsed (>= 500ms)
    root.event(state, &pointer(PointerPhase::Up, p));
}

// --- Geometry helpers ------------------------------------------------------

/// The 64dp bottom navigation bar starts here; the feed's own content is above.
const CONTENT_BOTTOM: f64 = 528.0;

/// The sheet panel background: the widest rounded rect (the full panel width,
/// 800px). `None` if no sheet is open.
fn panel_bg(scene: &RecScene) -> Option<(Point, Size)> {
    scene
        .rounded
        .iter()
        .copied()
        .filter(|(_, s)| s.width > W - 12.0)
        .max_by(|a, b| a.1.width.partial_cmp(&b.1.width).unwrap())
}

/// Whether a sheet is currently open (its panel background paints).
fn sheet_open(scene: &RecScene) -> bool {
    panel_bg(scene).is_some()
}

/// The full-width action rows of a menu/attachment sheet, sorted top-to-bottom.
/// Rows are `panel_width - 2*ROW_H_PAD` (== 768px) wide `filled_card`s — narrower
/// than the 800px panel background, wider than any feed bubble/composer chrome.
fn action_rows(scene: &RecScene) -> Vec<(Point, Size)> {
    let mut rows: Vec<(Point, Size)> = scene
        .rounded
        .iter()
        .copied()
        .filter(|(_, s)| (s.width - (W - 32.0)).abs() < 6.0)
        .collect();
    rows.sort_by(|a, b| a.0.y.partial_cmp(&b.0.y).unwrap());
    rows
}

/// The tap point for emoji cell `col` in row 0 of an open picker, computed from
/// the panel background + the grid's known column count / grab-handle height.
fn emoji_cell_0(scene: &RecScene, col: usize) -> Point {
    let (o, _) = panel_bg(scene).expect("the picker paints its panel background");
    let col_w = W / EMOJI_COLS as f64;
    let x = (col as f64 + 0.5) * col_w;
    // Content starts below the grab handle; the grid adds an 8px top pad, and
    // a cell is tall enough that +24 lands inside its first row.
    let y = o.y + GRAB_HANDLE_H + 24.0;
    Point::new(x, y)
}

/// The composer field's chrome — the widest rounded rect within the feed's own
/// content (above the bottom bar). Its vertical center is the composer row's
/// center, which the attach/emoji buttons share (they are center-aligned).
fn composer_field(scene: &RecScene) -> (Point, Size) {
    scene
        .rounded
        .iter()
        .copied()
        .filter(|(o, _)| o.y < CONTENT_BOTTOM && o.y > 300.0)
        .max_by(|a, b| a.1.width.partial_cmp(&b.1.width).unwrap())
        .expect("the composer field paints its chrome")
}

/// The composer's two affordance buttons (attach, then emoji) precede the field
/// in the row `Padding(all 8, Row[attach, sp(4), emoji, sp(6), field, ..])`, and
/// are center-aligned with it. Both are square `filled_card`s of the same width,
/// so from the field's left edge (`fx = 8 + 2*bw + 10`) the button width and
/// centers follow directly — robust to the card's internal padding.
fn composer_buttons(scene: &RecScene) -> (Point, Point) {
    let (o, s) = composer_field(scene);
    let fx = o.x;
    let bw = (fx - 18.0) / 2.0; // 8 (pad) + bw + 4 + bw + 6 == fx
    let cy = o.y + s.height / 2.0;
    let attach = Point::new(8.0 + bw / 2.0, cy);
    let emoji = Point::new(8.0 + bw + 4.0 + bw / 2.0, cy);
    (attach, emoji)
}

fn attach_button(scene: &RecScene) -> Point {
    composer_buttons(scene).0
}

fn emoji_button(scene: &RecScene) -> Point {
    composer_buttons(scene).1
}

/// The topmost (other-user) message row's leading avatar tile — the leftmost
/// small rounded rect in the feed content region. The feed rows are FLAT (no
/// `filled_card`/`elevated_card` bubble background), so the row's only
/// recorded rounded chrome is its 40px avatar disc; it is the row anchor a
/// long-press target is derived from.
fn first_avatar_rect(scene: &RecScene) -> (Point, Size) {
    scene
        .rounded
        .iter()
        .copied()
        .filter(|(o, s)| o.y > 70.0 && o.y < 420.0 && s.width < 80.0)
        .min_by(|a, b| a.0.x.partial_cmp(&b.0.x).unwrap())
        .expect("the loaded feed paints a leading avatar on the topmost row")
}

/// A safe long-press target inside the topmost message's flat content column —
/// to the right of its avatar (the rows are flat, so this is anchored off
/// the avatar rather than a bubble card that no longer paints).
fn first_bubble(scene: &RecScene) -> Point {
    let (o, s) = first_avatar_rect(scene);
    Point::new(o.x + s.width + 40.0, o.y + s.height / 2.0)
}

// --- Tests -----------------------------------------------------------------

/// A long-press on a message bubble opens its context menu (a sheet with its
/// full-width action rows).
#[test]
fn long_press_opens_the_context_menu() {
    let _g = serial();
    let _ambient = setup();

    let (mut root, mut state, mut logic, mut tcx, mut t_ms, scene) = mount_feed("general");
    assert!(!sheet_open(&scene), "no sheet before the long-press");

    let bubble = first_bubble(&scene);
    long_press(
        &mut root, &mut logic, &mut state, &mut tcx, &mut t_ms, bubble,
    );

    let opened = settle(&mut root, &mut logic, &mut state, &mut tcx, &mut t_ms);
    assert!(
        sheet_open(&opened),
        "the long-press opened the context-menu sheet"
    );
    assert!(
        action_rows(&opened).len() >= 4,
        "the feed menu shows React / Reply / Copy / Delete (got {} rows)",
        action_rows(&opened).len(),
    );
}

/// Long-press → React → the emoji picker → tapping the first emoji toggles that
/// reaction on the message (observed via the shared controller signal).
#[test]
fn react_menu_then_picker_toggles_a_reaction() {
    let _g = serial();
    let _ambient = setup();

    let (mut root, mut state, mut logic, mut tcx, mut t_ms, scene) = mount_feed("general");
    let controller = MessagesController::for_channel("general");
    let target = controller.snapshot()[0].id;
    let emoji = REACTION_EMOJI[0];
    let base = reaction_count(&controller, target, emoji);

    // Open the menu on the topmost (oldest) bubble.
    long_press(
        &mut root,
        &mut logic,
        &mut state,
        &mut tcx,
        &mut t_ms,
        first_bubble(&scene),
    );
    let menu = settle(&mut root, &mut logic, &mut state, &mut tcx, &mut t_ms);

    // React is the first menu row.
    let react_row = action_rows(&menu)[0];
    tap(&mut root, &mut state, center(react_row));
    let picker = settle(&mut root, &mut logic, &mut state, &mut tcx, &mut t_ms);
    assert!(sheet_open(&picker), "React opens the emoji picker");

    // Tap the first emoji cell (column 0, row 0) → toggles REACTION_EMOJI[0].
    tap(&mut root, &mut state, emoji_cell_0(&picker, 0));
    let closed = settle(&mut root, &mut logic, &mut state, &mut tcx, &mut t_ms);

    assert!(!sheet_open(&closed), "picking an emoji dismisses the sheet");
    assert_eq!(
        reaction_count(&controller, target, emoji),
        base + 1,
        "tapping the emoji toggled the reaction on via the shared controller",
    );
}

/// A downward drag on an open sheet dismisses it without firing any row.
#[test]
fn drag_dismiss_closes_the_sheet_without_acting() {
    let _g = serial();
    let _ambient = setup();

    let (mut root, mut state, mut logic, mut tcx, mut t_ms, scene) = mount_feed("general");

    long_press(
        &mut root,
        &mut logic,
        &mut state,
        &mut tcx,
        &mut t_ms,
        first_bubble(&scene),
    );
    let menu = settle(&mut root, &mut logic, &mut state, &mut tcx, &mut t_ms);
    let (panel_o, _) = panel_bg(&menu).expect("the menu sheet is open");
    let toasts_before = state.toasts.snapshot_untracked().len();

    // Drag down from near the top of the panel, well past the dismiss threshold.
    let x = W / 2.0;
    let start = Point::new(x, panel_o.y + 20.0);
    root.event(&mut state, &pointer(PointerPhase::Down, start));
    root.event(
        &mut state,
        &pointer(PointerPhase::Move, Point::new(x, panel_o.y + 80.0)),
    );
    root.event(
        &mut state,
        &pointer(PointerPhase::Move, Point::new(x, panel_o.y + 240.0)),
    );
    root.event(
        &mut state,
        &pointer(PointerPhase::Move, Point::new(x, panel_o.y + 360.0)),
    );
    root.event(
        &mut state,
        &pointer(PointerPhase::Up, Point::new(x, panel_o.y + 360.0)),
    );

    let closed = settle(&mut root, &mut logic, &mut state, &mut tcx, &mut t_ms);
    assert!(!sheet_open(&closed), "the drag-down dismissed the sheet");
    assert_eq!(
        state.toasts.snapshot_untracked().len(),
        toasts_before,
        "a drag-dismiss fires no menu row (no toast raised)",
    );
}

/// The composer's emoji button opens the picker targeting the composer; a picked
/// emoji is inserted into the field (proven by submitting it as a message).
#[test]
fn composer_emoji_button_inserts_into_the_field() {
    let _g = serial();
    let _ambient = setup();

    let (mut root, mut state, mut logic, mut tcx, mut t_ms, scene) = mount_feed("general");
    let controller = MessagesController::for_channel("general");
    let before = controller.snapshot().len();
    let emoji = REACTION_EMOJI[0];

    // Open the composer emoji picker, pick the first emoji (inserted into the
    // composer text), which closes the sheet.
    tap(&mut root, &mut state, emoji_button(&scene));
    let picker = settle(&mut root, &mut logic, &mut state, &mut tcx, &mut t_ms);
    assert!(
        sheet_open(&picker),
        "the composer emoji button opens the picker"
    );
    tap(&mut root, &mut state, emoji_cell_0(&picker, 0));
    let after_pick = settle(&mut root, &mut logic, &mut state, &mut tcx, &mut t_ms);
    assert!(!sheet_open(&after_pick), "picking closes the sheet");

    // Submit the composer: focus the field and Shift+Enter. If the emoji reached
    // the composer, it is now sent as a message whose body is that emoji.
    tap(&mut root, &mut state, center(composer_field(&after_pick)));
    root.event(&mut state, &shift_enter());

    let runtime = ReactiveRuntime::get().expect("runtime installed by setup()");
    let deadline = Instant::now() + LOAD_WAIT;
    loop {
        if controller.snapshot().len() > before {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the composed emoji was never sent"
        );
        runtime.pump_local();
        std::thread::sleep(Duration::from_millis(2));
        t_ms += 16;
        frame_at(&mut root, &mut logic, &mut state, &mut tcx, t_ms);
    }

    let sent = controller.snapshot();
    assert!(
        sent.iter().any(|m| m.is_own()
            && matches!(&m.body,
            huddle::features::messages::FeedBody::Text(t) if t == emoji)),
        "the picked emoji was inserted into the composer and sent",
    );
}

/// The composer's attach button opens the mock attachment sheet; tapping a row
/// raises the matching toast and closes the sheet.
#[test]
fn attachment_row_toasts() {
    let _g = serial();
    let _ambient = setup();

    let (mut root, mut state, mut logic, mut tcx, mut t_ms, scene) = mount_feed("general");

    tap(&mut root, &mut state, attach_button(&scene));
    let sheet = settle(&mut root, &mut logic, &mut state, &mut tcx, &mut t_ms);
    assert!(
        sheet_open(&sheet),
        "the attach button opens the attachment sheet"
    );

    // Photo is the first row.
    let photo = action_rows(&sheet)[0];
    tap(&mut root, &mut state, center(photo));
    let closed = settle(&mut root, &mut logic, &mut state, &mut tcx, &mut t_ms);

    assert!(
        !sheet_open(&closed),
        "picking an attachment closes the sheet"
    );
    assert!(
        state
            .toasts
            .snapshot_untracked()
            .iter()
            .any(|t| t.text == "Attached Photo (mock)"),
        "the Photo row raised its mock toast",
    );
}

// --- Small helpers ---------------------------------------------------------

fn reaction_count(controller: &MessagesController, id: u32, emoji: &str) -> u32 {
    controller
        .message(id)
        .map(|m| {
            m.reactions
                .into_iter()
                .find(|r| r.emoji == emoji)
                .map(|r| r.count)
                .unwrap_or(0)
        })
        .unwrap_or(0)
}

/// A synthetic Shift+Enter — the multiline composer's submit chord.
fn shift_enter() -> frust_core::InputEvent {
    use frust_core::{Key, KeyEvent, Modifiers, NamedKey};
    frust_core::InputEvent::Key(KeyEvent {
        key: Key::Named(NamedKey::Enter),
        modifiers: Modifiers {
            shift: true,
            ..Modifiers::default()
        },
        repeat: false,
    })
}
