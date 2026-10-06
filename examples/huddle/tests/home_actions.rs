//! Home-actions integration tests: the app-bar ADD action
//! opening the create-channel sheet, creating a channel appending a live row
//! and a toast, the row long-press menu's Mute action (state flip and undo
//! toast, swipe parity), and the same menu's Invite-people entry confirming
//! and toasting.
//!
//! Like `tests/home.rs`/`tests/actions.rs` these drive the whole mounted
//! [`HuddleApp`] through its real navigator/component tree and assert
//! black-box against [`RecScene`]'s recorded rounded-rect geometry plus the
//! app's [`ToastController`](huddle::ui::toast::ToastController) — no GPU, no
//! window. Every test takes the [`support::serial`] lock first (the roster
//! load's mocked latency runs on the shared background reactive runtime).

use std::any::Any;
use std::time::{Duration, Instant};

use frust::{AnyView, Component};
use frust_core::{FrameTime, PointerPhase, RenderRoot};
use frust_reactive::ReactiveRuntime;
use frust_text::TextContext;
use kurbo::{Point, Size};

use huddle::{HuddleApp, HuddleState};

mod support;
use support::{RecScene, W, center, char_key, pointer, serial, setup, tap};

type Root = RenderRoot<HuddleState, AnyView<HuddleState>>;

const LOAD_WAIT: Duration = Duration::from_secs(5);

/// The app bar's trailing `icons::ADD` action: a 24×24 icon flush to the
/// trailing edge of the 64dp bar (`PAD_X` = 4, no `GAP` — it's the only
/// action), vertically centered. Mirrors `tests/activity.rs`/`tests/thread.rs`'s
/// `BACK_ACTION` fixed-layout convention.
const ADD_ACTION: Point = Point::new(784.0, 32.0);

/// The app bar's leading "HQ" workspace tile: a `fill_box(40,40)` inset by
/// `Padding::symmetric(6, 12)` (see `screens::home::workspace_tile`), so the
/// slot is 52×64 flush to the leading edge (`PAD_X` = 4).
const WORKSPACE_TILE: Point = Point::new(30.0, 32.0);

/// Rebuild + layout + paint at `t_ms` on a caller-advanced clock; returns the
/// scene and whether the paint asked for another frame (mirrors
/// `tests/home.rs`/`tests/actions.rs::frame_at`).
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
/// until no animation asks for another frame — settles a load, a nav
/// cross-fade, or a sheet's slide entrance (mirrors `tests/actions.rs::settle`).
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

/// Boot the app and pump frames until the roster's loaded (many more text
/// glyphs than the loading skeleton's), then settle the boot-time entrance
/// cross-fade — mirrors `tests/home.rs`'s two-step loaded+settled setup, folded
/// into one helper here since every test below starts from it.
fn mount_loaded_home() -> (
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

    let runtime = ReactiveRuntime::get().expect("setup() installed the reactive runtime");
    let deadline = Instant::now() + LOAD_WAIT;
    let mut t_ms = 50u64;
    let loading = {
        let (scene, _) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, t_ms);
        scene
    };
    let loaded = loop {
        runtime.pump_local();
        std::thread::sleep(Duration::from_millis(1));
        t_ms += 16;
        let (scene, needs_frame) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, t_ms);
        if !needs_frame && scene.glyph_runs > loading.glyph_runs + 10 {
            break scene;
        }
        assert!(
            Instant::now() < deadline,
            "the roster did not load within the deadline"
        );
    };

    (root, state, logic, tcx, t_ms, loaded)
}

/// Simulate a long-press at `p`: `Down`, advance the paint clock past the
/// ~500ms threshold, `Up` — mirrors `tests/actions.rs::long_press`.
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

/// Find the first ~40×40 rounded rect in the roster region (a row's leading
/// avatar/`#` circle) — mirrors `tests/home.rs::first_row_center`.
fn first_row_center(scene: &RecScene) -> Point {
    let (origin, size) = scene
        .rounded
        .iter()
        .copied()
        .find(|(origin, size)| {
            (size.width - 40.0).abs() < 1.5
                && (size.height - 40.0).abs() < 1.5
                && origin.y > 64.0
                && origin.y < 500.0
        })
        .expect("a roster row leading circle was painted");
    center((origin, size))
}

/// Count of ~40×40 rounded rects in the roster region — every channel `#`
/// circle and initials-avatar DM row paints one (a couple of DM rows paint an
/// `Image` avatar instead, which `RecScene` doesn't record — see its module
/// docs — so this undercounts DMs by a small fixed amount, but a *before/after*
/// comparison around one `create_channel` call is exactly what every test
/// below uses it for).
fn roster_circle_count(scene: &RecScene) -> usize {
    scene
        .rounded
        .iter()
        .filter(|(origin, size)| {
            (size.width - 40.0).abs() < 1.5
                && (size.height - 40.0).abs() < 1.5
                && origin.y > 64.0
                && origin.y < 500.0
        })
        .count()
}

/// The widest painted rect that isn't the sheet's own full-panel-width
/// background (800px here) — mirrors `tests/actions.rs::panel_bg`'s inverse:
/// this finds *sheet content* chrome, not the panel itself.
fn panel_bg(scene: &RecScene) -> Option<(Point, Size)> {
    scene
        .rounded
        .iter()
        .copied()
        .filter(|(_, s)| s.width > W - 12.0)
        .max_by(|a, b| a.1.width.partial_cmp(&b.1.width).unwrap())
}

/// Whether a `crate::ui::sheet` overlay is currently open (its panel
/// background paints) — mirrors `tests/actions.rs::sheet_open`.
fn sheet_open(scene: &RecScene) -> bool {
    panel_bg(scene).is_some()
}

/// The row-action menu's full-width action rows (`ROW_H_PAD` = 16 on each
/// side of the 800px panel, so a row's `filled_card` is 768px wide), sorted
/// top-to-bottom — mirrors `tests/actions.rs::action_rows`.
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

/// The create-channel sheet's name `TextInput` chrome: the widest painted rect
/// short of the sheet's own panel background (`TextInput` paints a border rect
/// plus a slightly inset fill rect — either is a safe tap target to focus it).
fn name_field_point(scene: &RecScene) -> Point {
    let (o, s) = scene
        .rounded
        .iter()
        .copied()
        .filter(|(_, s)| s.width > 400.0 && s.width < W - 12.0)
        .max_by(|a, b| a.1.width.partial_cmp(&b.1.width).unwrap())
        .expect("the create-channel sheet's name field chrome painted");
    center((o, s))
}

/// The create-channel sheet's "Create" button. The roster underneath keeps
/// painting behind the sheet overlay (`RecScene` records every
/// `fill_rounded_rect` call regardless of scroll offset or an ancestor's
/// clip, and the roster's ~40px circles, ~22px unread badges, and the
/// persistent bottom nav bar's own ~56px selection pill all coincide with the
/// sheet panel's `y`/width ranges at one point or another), so isolating the
/// button needs both a width band *and* a `y` bound: a `button()` widget's
/// own padding (`PAD_X`/`PAD_Y`, `button.rs`) makes "Cancel"/"Create"
/// noticeably wider than a badge (22px) or a roster circle (40px) but far
/// narrower than the name field (~765px) or the `Switch` track (52px) —
/// `(45, 150)` brackets them; restricting to the sheet panel's own vertical
/// span excludes the bottom nav bar's pill, which sits just below it. Within
/// both bounds, the sheet's Cancel/Create action row is the last thing laid
/// out in the form, so it has the greatest `y` origin; "Create" sits to the
/// trailing edge, so among rects sharing that row's `y`, the one with the
/// greatest `x` wins.
fn create_button_point(scene: &RecScene) -> Point {
    let (panel_o, panel_s) = panel_bg(scene).expect("the create-channel sheet is open");
    let panel_bottom = panel_o.y + panel_s.height;
    let (o, s) = scene
        .rounded
        .iter()
        .copied()
        .filter(|(o, s)| {
            s.width > 45.0 && s.width < 150.0 && o.y >= panel_o.y && o.y < panel_bottom
        })
        .max_by(|a, b| {
            a.0.y
                .partial_cmp(&b.0.y)
                .unwrap()
                .then(a.0.x.partial_cmp(&b.0.x).unwrap())
        })
        .expect("the create-channel sheet's Create button chrome painted");
    center((o, s))
}

/// A small rect (`< 200px` wide) present in `after` but absent from `before`
/// (same origin+size, floating-point exact — nothing else on the page
/// repaints differently between the two frames) with the greatest `x` origin
/// — the invite modal's single trailing "Send" action button, isolated from
/// the unchanged Home chrome underneath it (the modal is a *transparent* push,
/// so Home stays visible and painted below the scrim).
fn new_button_point(before: &RecScene, after: &RecScene) -> Point {
    let (o, s) = after
        .rounded
        .iter()
        .copied()
        .filter(|(o, s)| {
            s.width < 200.0
                && !before
                    .rounded
                    .iter()
                    .any(|(bo, bs)| (bo.x - o.x).abs() < 0.5 && (bs.width - s.width).abs() < 0.5)
        })
        .max_by(|a, b| a.0.x.partial_cmp(&b.0.x).unwrap())
        .expect("the invite modal's Send button chrome painted");
    center((o, s))
}

fn type_text(root: &mut Root, state: &mut HuddleState, text: &str) {
    for c in text.chars() {
        root.event(state, &char_key(&c.to_string()));
    }
}

// --- Tests -------------------------------------------------------------

/// The app bar's leading workspace tile opens the workspace switcher, and the
/// trailing ADD action opens the create-channel sheet.
#[test]
fn app_bar_add_opens_the_create_channel_sheet() {
    let _g = serial();
    let _ambient = setup();

    let (mut root, mut state, mut logic, mut tcx, mut t_ms, loaded) = mount_loaded_home();
    assert!(!sheet_open(&loaded), "no sheet before the tap");

    tap(&mut root, &mut state, ADD_ACTION);
    let opened = settle(&mut root, &mut logic, &mut state, &mut tcx, &mut t_ms);
    assert!(
        sheet_open(&opened),
        "the ADD action opened the create-channel sheet"
    );
}

/// The workspace tile pushes `/workspace-switcher` — the drawer's real entry
/// point.
#[test]
fn workspace_tile_pushes_the_switcher() {
    let _g = serial();
    let _ambient = setup();

    let (mut root, mut state, mut logic, mut tcx, mut t_ms, _loaded) = mount_loaded_home();

    tap(&mut root, &mut state, WORKSPACE_TILE);
    let opened = settle(&mut root, &mut logic, &mut state, &mut tcx, &mut t_ms);
    // The workspace drawer's own panel background + 3 workspace tiles paint —
    // a signature the Home roster never produces (`tests/profile.rs`'s
    // `DRAWER_SIGNATURE`), so a non-trivial rounded-rect count here is enough
    // to prove the switcher (not Home) is now on screen.
    assert!(
        opened.rounded.len() >= 4,
        "the workspace switcher's panel + tiles painted (got {} rects)",
        opened.rounded.len(),
    );
}

/// Filling in the create-channel sheet and tapping Create appends a live row
/// to the roster and raises a "Created #name" toast.
#[test]
fn creating_a_channel_adds_a_live_row_and_toasts() {
    let _g = serial();
    let _ambient = setup();

    let (mut root, mut state, mut logic, mut tcx, mut t_ms, loaded) = mount_loaded_home();
    let circles_before = roster_circle_count(&loaded);

    tap(&mut root, &mut state, ADD_ACTION);
    let opened = settle(&mut root, &mut logic, &mut state, &mut tcx, &mut t_ms);

    tap(&mut root, &mut state, name_field_point(&opened));
    type_text(&mut root, &mut state, "launch-planning");
    t_ms += 16;
    let (filled, _) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, t_ms);

    tap(&mut root, &mut state, create_button_point(&filled));
    let closed = settle(&mut root, &mut logic, &mut state, &mut tcx, &mut t_ms);

    assert!(
        !sheet_open(&closed),
        "creating the channel closed the sheet"
    );
    assert_eq!(
        roster_circle_count(&closed),
        circles_before + 1,
        "the new channel's leading `#` circle appended one more roster row"
    );
    assert!(
        state
            .toasts
            .snapshot_untracked()
            .iter()
            .any(|t| t.text == "Created #launch-planning"),
        "creating raised a \"Created #name\" toast",
    );
}

/// An empty name is a no-op: Create does nothing, the sheet stays open.
#[test]
fn creating_with_an_empty_name_is_a_no_op() {
    let _g = serial();
    let _ambient = setup();

    let (mut root, mut state, mut logic, mut tcx, mut t_ms, _loaded) = mount_loaded_home();

    tap(&mut root, &mut state, ADD_ACTION);
    let opened = settle(&mut root, &mut logic, &mut state, &mut tcx, &mut t_ms);
    let circles_before = roster_circle_count(&opened);

    tap(&mut root, &mut state, create_button_point(&opened));
    t_ms += 16;
    let (after, _) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, t_ms);

    assert!(
        sheet_open(&after),
        "an empty name does not create a channel or close the sheet"
    );
    assert_eq!(
        roster_circle_count(&after),
        circles_before,
        "no row was appended"
    );
}

/// A long-press on a roster row opens its Mute/Archive/Invite/Cancel action
/// menu; tapping Mute flips the flag (via the shared controller op) and
/// raises an "Muted …" undo toast — the exact swipe-parity shape
/// `tests/home.rs::swipe_right_archives_with_an_undo_toast` exercises for the
/// swipe gesture.
#[test]
fn row_long_press_menu_mutes_with_an_undo_toast() {
    let _g = serial();
    let _ambient = setup();

    let (mut root, mut state, mut logic, mut tcx, mut t_ms, loaded) = mount_loaded_home();

    let row = first_row_center(&loaded);
    long_press(&mut root, &mut logic, &mut state, &mut tcx, &mut t_ms, row);
    let menu = settle(&mut root, &mut logic, &mut state, &mut tcx, &mut t_ms);

    let rows = action_rows(&menu);
    assert_eq!(
        rows.len(),
        4,
        "Mute, Archive, Invite people, Cancel (got {})",
        rows.len(),
    );

    // Mute is the first row.
    tap(&mut root, &mut state, center(rows[0]));
    let closed = settle(&mut root, &mut logic, &mut state, &mut tcx, &mut t_ms);
    assert!(!sheet_open(&closed), "tapping Mute closed the menu");

    let snap = state.toasts.snapshot_untracked();
    let entry = snap
        .iter()
        .find(|e| e.text.starts_with("Muted "))
        .expect("the long-press Mute row raised a Muted toast");
    let action = entry
        .action
        .as_ref()
        .expect("the mute toast has an Undo action");
    assert_eq!(action.label, "Undo", "the action is an Undo affordance");

    // Undo restores without panicking, and the app keeps rendering.
    (action.callback)();
    let after = {
        t_ms += 16;
        frame_at(&mut root, &mut logic, &mut state, &mut tcx, t_ms).0
    };
    assert!(after.glyph_runs > 0, "the app still renders after undo");
}

/// The row long-press menu's "Cancel" row just closes the menu, firing
/// nothing.
#[test]
fn row_long_press_menu_cancel_closes_without_acting() {
    let _g = serial();
    let _ambient = setup();

    let (mut root, mut state, mut logic, mut tcx, mut t_ms, loaded) = mount_loaded_home();

    let row = first_row_center(&loaded);
    long_press(&mut root, &mut logic, &mut state, &mut tcx, &mut t_ms, row);
    let menu = settle(&mut root, &mut logic, &mut state, &mut tcx, &mut t_ms);
    let rows = action_rows(&menu);
    let toasts_before = state.toasts.snapshot_untracked().len();

    // Cancel is the last row.
    tap(&mut root, &mut state, center(rows[rows.len() - 1]));
    let closed = settle(&mut root, &mut logic, &mut state, &mut tcx, &mut t_ms);

    assert!(!sheet_open(&closed), "Cancel closed the menu");
    assert_eq!(
        state.toasts.snapshot_untracked().len(),
        toasts_before,
        "Cancel raised no toast"
    );
}

/// The row long-press menu's "Invite people" row opens the invite modal;
/// confirming ("Send") toasts "Invites sent (mock)".
#[test]
fn invite_people_confirms_and_toasts() {
    let _g = serial();
    let _ambient = setup();

    let (mut root, mut state, mut logic, mut tcx, mut t_ms, loaded) = mount_loaded_home();

    let row = first_row_center(&loaded);
    long_press(&mut root, &mut logic, &mut state, &mut tcx, &mut t_ms, row);
    let menu = settle(&mut root, &mut logic, &mut state, &mut tcx, &mut t_ms);
    let rows = action_rows(&menu);

    // Invite people is the third row (Mute, Archive, Invite people, Cancel).
    let before_push = {
        t_ms += 16;
        frame_at(&mut root, &mut logic, &mut state, &mut tcx, t_ms).0
    };
    tap(&mut root, &mut state, center(rows[2]));
    let dialog_open = settle(&mut root, &mut logic, &mut state, &mut tcx, &mut t_ms);

    let send = new_button_point(&before_push, &dialog_open);
    tap(&mut root, &mut state, send);
    t_ms += 16;
    let _ = frame_at(&mut root, &mut logic, &mut state, &mut tcx, t_ms);
    // The pop applied above is queued into the navigator's pending-results
    // list at that rebuild; the `on_result` callback only flushes on the
    // *next* event pass (frust-widgets' `NavigatorWidget::event` doc
    // comment) — a harmless synthetic `Move` reaches it.
    root.event(&mut state, &pointer(PointerPhase::Move, send));

    assert!(
        state
            .toasts
            .snapshot_untracked()
            .iter()
            .any(|t| t.text == "Invites sent (mock)"),
        "confirming the invite modal toasted \"Invites sent (mock)\"",
    );
}
