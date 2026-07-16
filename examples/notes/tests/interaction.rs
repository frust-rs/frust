//! Headless interaction proof for the notes demo (Phase 4B exit gate).
//!
//! Desktop UI automation (osascript/cliclick) is unavailable on this host, so
//! rather than driving a live GPU window this test feeds synthetic
//! [`InputEvent`]s straight into the framework's [`RenderRoot`] — the exact
//! event seam the desktop shell's `dispatch` uses (`RenderRoot::event`) and the
//! mobile shells reach through `AppTree::event`. Text is shaped through a real
//! `TextContext` (CPU-only, no GPU) and paint output is recorded, so every
//! assertion is deterministic and this doubles as a permanent regression gate
//! for the notes demo's TextInput / keyed-list / Image wiring.
//!
//! Coverage (the acceptance matrix): the logo renders and the placeholder is
//! visible; focusing the field and typing fires `on_change` once per keystroke;
//! Enter submits a keyed note and clears the draft; select-all + type replaces
//! the whole field; the arrow/backspace editing matrix; the IME compose→commit
//! sequence; and the keyed-identity headline — adding three notes and deleting
//! the *middle* one by its own Delete button removes the right row, not
//! whatever index it sat at. Decode-once is asserted across many frames.

use std::any::Any;

use forgekit_core::{
    ImeEvent, InputEvent, Key, KeyEvent, Modifiers, NamedKey, PaintScene, PointerButton,
    PointerEvent, PointerPhase, RenderRoot, View,
};
use forgekit_scene::GlyphRun;
use forgekit_text::TextContext;
use kurbo::{Point, Rect, Size};
use notes::{AppState, app_logic, logo_decode_count};
use peniko::Color;

const W: f64 = 800.0;
const H: f64 = 600.0;

/// A GPU-free paint target: records rounded-rect geometry (the field chrome and
/// each Delete button), counts glyph runs (title/placeholder/note rows), and
/// records drawn images (the logo) so the test can locate widgets and observe
/// list contents.
#[derive(Default)]
struct RecScene {
    rounded: Vec<(Point, Size)>,
    glyph_runs: usize,
    images: Vec<Rect>,
}

impl PaintScene for RecScene {
    fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
    fn fill_rounded_rect(&mut self, origin: Point, size: Size, _radius: f64, _color: Color) {
        self.rounded.push((origin, size));
    }
    fn draw_text(&mut self, _origin: Point, _text: &str) {}
    fn draw_glyph_run(&mut self, _run: GlyphRun) {
        self.glyph_runs += 1;
    }
    fn draw_image(&mut self, _data: &peniko::ImageData, dest: Rect) {
        self.images.push(dest);
    }
}

/// One frame: rebuild the tree from `state`, lay it out at window size, and
/// record the paint output.
fn frame<V: View<AppState>>(
    root: &mut RenderRoot<AppState, V>,
    logic: &mut impl FnMut(&mut AppState) -> V,
    state: &mut AppState,
    tcx: &mut TextContext,
) -> RecScene {
    root.rebuild(logic, state);
    let tcx_any: &mut dyn Any = tcx;
    root.layout_with_text(Size::new(W, H), tcx_any);
    let mut scene = RecScene::default();
    root.paint(&mut scene);
    scene
}

fn pointer(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
    InputEvent::Pointer(PointerEvent {
        phase,
        position: Point::new(x, y),
        button: PointerButton::Primary,
    })
}

/// A press-and-release at `p` (focuses the field / fires a button).
fn tap<V: View<AppState>>(root: &mut RenderRoot<AppState, V>, state: &mut AppState, p: Point) {
    root.event(state, &pointer(PointerPhase::Down, p.x, p.y));
    root.event(state, &pointer(PointerPhase::Up, p.x, p.y));
}

fn ch(text: &str) -> InputEvent {
    InputEvent::Key(KeyEvent {
        key: Key::Character(text.to_string()),
        modifiers: Modifiers::default(),
        repeat: false,
    })
}

fn named(key: NamedKey, modifiers: Modifiers) -> InputEvent {
    InputEvent::Key(KeyEvent {
        key: Key::Named(key),
        modifiers,
        repeat: false,
    })
}

fn center((origin, size): (Point, Size)) -> Point {
    Point::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0)
}

/// The text field's chrome: the widest rounded rect (it spans the full window
/// width, dwarfing every Delete button).
fn field_rect(scene: &RecScene) -> (Point, Size) {
    scene
        .rounded
        .iter()
        .copied()
        .max_by(|a, b| a.1.width.partial_cmp(&b.1.width).unwrap())
        .expect("a text-field chrome rect")
}

/// The Delete-button rects (every rounded rect narrower than half the window),
/// returned top-to-bottom — so index `i` is note row `i`.
fn delete_buttons(scene: &RecScene) -> Vec<(Point, Size)> {
    let mut buttons: Vec<(Point, Size)> = scene
        .rounded
        .iter()
        .copied()
        .filter(|(_, s)| s.width < W / 2.0)
        .collect();
    buttons.sort_by(|a, b| a.0.y.partial_cmp(&b.0.y).unwrap());
    buttons
}

/// Focus the field, then type each note and press Enter — the demo's core loop.
/// Returns the field center (stable across frames; the field sits above the
/// scrolling list).
fn add_notes<V: View<AppState>>(
    root: &mut RenderRoot<AppState, V>,
    logic: &mut impl FnMut(&mut AppState) -> V,
    state: &mut AppState,
    tcx: &mut TextContext,
    labels: &[&str],
) -> Point {
    let scene = frame(root, logic, state, tcx);
    let field = center(field_rect(&scene));
    for label in labels {
        tap(root, state, field);
        for c in label.chars() {
            root.event(state, &ch(&c.to_string()));
        }
        root.event(state, &named(NamedKey::Enter, Modifiers::default()));
        frame(root, logic, state, tcx);
    }
    field
}

#[test]
fn renders_logo_and_placeholder_and_decodes_once() {
    let mut root = RenderRoot::new();
    let mut logic = app_logic;
    let mut state = AppState::new();
    let mut tcx = TextContext::new();

    // At least one decode happened (the startup decode in `AppState::new`). The
    // exact process-wide count is not asserted here because sibling tests run in
    // parallel and share this global — decode-once is proved below by Arc
    // identity, which is per-run and race-free.
    assert!(logo_decode_count() >= 1, "the logo decoded at startup");

    // The decode-once contract, proved by pointer identity: keep a handle to the
    // startup-decoded source, render many frames, and confirm `state.logo` is
    // still the *same* `Arc` — `app_logic` only ever clones it, never re-decodes
    // (a re-decode would allocate a fresh, non-`same` Arc).
    let handle = state.logo.clone();
    let mut last = RecScene::default();
    for _ in 0..5 {
        last = frame(&mut root, &mut logic, &mut state, &mut tcx);
    }
    assert!(
        state.logo.same(&handle),
        "no per-frame re-decode: the ImageSource Arc is stable across frames"
    );

    assert_eq!(last.images.len(), 1, "the logo is painted exactly once");
    assert!(
        last.glyph_runs > 0,
        "the title and the field placeholder shape into visible glyph runs"
    );
    assert!(
        !last.rounded.is_empty(),
        "the empty field still paints its chrome (a rounded rect)"
    );
}

#[test]
fn typing_fires_on_change_per_keystroke() {
    let mut root = RenderRoot::new();
    let mut logic = app_logic;
    let mut state = AppState::new();
    let mut tcx = TextContext::new();

    let scene = frame(&mut root, &mut logic, &mut state, &mut tcx);
    let field = center(field_rect(&scene));
    tap(&mut root, &mut state, field);

    // Each keystroke fires `on_change`, which sets `draft` to the new full text —
    // so the draft grows by exactly one char per key, proving per-keystroke
    // delivery (not a coalesced end-of-input flush).
    for (i, c) in "hello".chars().enumerate() {
        root.event(&mut state, &ch(&c.to_string()));
        assert_eq!(
            state.draft.chars().count(),
            i + 1,
            "on_change fired for keystroke {i} — draft is now {:?}",
            state.draft
        );
    }
    assert_eq!(state.draft, "hello");
    assert!(state.notes.is_empty(), "no submit yet, so no note appended");
}

#[test]
fn enter_submits_keyed_note_and_clears_draft() {
    let mut root = RenderRoot::new();
    let mut logic = app_logic;
    let mut state = AppState::new();
    let mut tcx = TextContext::new();

    add_notes(&mut root, &mut logic, &mut state, &mut tcx, &["hello"]);

    assert_eq!(
        state.notes,
        vec![(0u64, "hello".to_string())],
        "Enter submits the draft as a keyed note (id 0)"
    );
    assert_eq!(state.draft, "", "submitting clears the draft");
    assert_eq!(state.next_id, 1, "the id counter advanced");
}

#[test]
fn blank_submit_is_dropped() {
    let mut root = RenderRoot::new();
    let mut logic = app_logic;
    let mut state = AppState::new();
    let mut tcx = TextContext::new();

    let scene = frame(&mut root, &mut logic, &mut state, &mut tcx);
    let field = center(field_rect(&scene));
    tap(&mut root, &mut state, field);
    // Enter with an empty (and then whitespace-only) draft appends nothing.
    root.event(&mut state, &named(NamedKey::Enter, Modifiers::default()));
    for c in "   ".chars() {
        root.event(&mut state, &ch(&c.to_string()));
    }
    root.event(&mut state, &named(NamedKey::Enter, Modifiers::default()));
    assert!(
        state.notes.is_empty(),
        "an empty/whitespace draft never appends a blank row"
    );
}

#[test]
fn select_all_then_type_replaces_the_field() {
    let mut root = RenderRoot::new();
    let mut logic = app_logic;
    let mut state = AppState::new();
    let mut tcx = TextContext::new();

    let scene = frame(&mut root, &mut logic, &mut state, &mut tcx);
    let field = center(field_rect(&scene));
    tap(&mut root, &mut state, field);

    for c in "abc".chars() {
        root.event(&mut state, &ch(&c.to_string()));
    }
    assert_eq!(state.draft, "abc");

    // Ctrl/Cmd+A selects all, then a keystroke replaces the whole selection.
    let meta = Modifiers {
        meta: true,
        ..Modifiers::default()
    };
    root.event(
        &mut state,
        &InputEvent::Key(KeyEvent {
            key: Key::Character("a".to_string()),
            modifiers: meta,
            repeat: false,
        }),
    );
    root.event(&mut state, &ch("x"));
    assert_eq!(
        state.draft, "x",
        "select-all then typing replaces the entire field"
    );
}

#[test]
fn arrow_and_backspace_editing_matrix() {
    let mut root = RenderRoot::new();
    let mut logic = app_logic;
    let mut state = AppState::new();
    let mut tcx = TextContext::new();

    let scene = frame(&mut root, &mut logic, &mut state, &mut tcx);
    let field = center(field_rect(&scene));
    tap(&mut root, &mut state, field);

    for c in "abcd".chars() {
        root.event(&mut state, &ch(&c.to_string()));
    }
    assert_eq!(state.draft, "abcd");

    let plain = Modifiers::default();
    // Home, then Delete removes the FIRST char (forward delete at line start).
    root.event(&mut state, &named(NamedKey::Home, plain));
    root.event(&mut state, &named(NamedKey::Delete, plain));
    assert_eq!(state.draft, "bcd", "Home + Delete removes the leading char");

    // End, then Backspace removes the LAST char.
    root.event(&mut state, &named(NamedKey::End, plain));
    root.event(&mut state, &named(NamedKey::Backspace, plain));
    assert_eq!(
        state.draft, "bc",
        "End + Backspace removes the trailing char"
    );

    // ArrowLeft then insert lands the char before the caret.
    root.event(&mut state, &named(NamedKey::ArrowLeft, plain));
    root.event(&mut state, &ch("Z"));
    assert_eq!(
        state.draft, "bZc",
        "ArrowLeft moves the caret one grapheme back"
    );
}

#[test]
fn ime_compose_then_commit_inserts_composed_text() {
    let mut root = RenderRoot::new();
    let mut logic = app_logic;
    let mut state = AppState::new();
    let mut tcx = TextContext::new();

    let scene = frame(&mut root, &mut logic, &mut state, &mut tcx);
    let field = center(field_rect(&scene));
    tap(&mut root, &mut state, field);

    // A preedit (marked text) followed by a commit — the desktop winit IME path
    // and the shape mobile bridges drive. The committed text lands in the field.
    root.event(
        &mut state,
        &InputEvent::Ime(ImeEvent::Compose {
            text: "ni".to_string(),
            cursor: Some((2, 2)),
        }),
    );
    root.event(
        &mut state,
        &InputEvent::Ime(ImeEvent::Commit("你好".to_string())),
    );
    assert_eq!(
        state.draft, "你好",
        "the committed IME text replaces the preedit and lands in the draft"
    );
    assert!(
        state.notes.is_empty(),
        "an IME commit of text is not a submit"
    );
}

#[test]
fn keyed_delete_removes_the_correct_middle_row() {
    let mut root = RenderRoot::new();
    let mut logic = app_logic;
    let mut state = AppState::new();
    let mut tcx = TextContext::new();

    // Add three notes: ids 0/1/2 = one/two/three, top-to-bottom.
    add_notes(
        &mut root,
        &mut logic,
        &mut state,
        &mut tcx,
        &["one", "two", "three"],
    );
    assert_eq!(
        state.notes,
        vec![
            (0u64, "one".to_string()),
            (1, "two".to_string()),
            (2, "three".to_string()),
        ]
    );

    // Locate the three Delete buttons top-to-bottom and tap the MIDDLE one.
    let scene = frame(&mut root, &mut logic, &mut state, &mut tcx);
    let buttons = delete_buttons(&scene);
    assert_eq!(buttons.len(), 3, "one Delete button per note row");
    tap(&mut root, &mut state, center(buttons[1]));

    // The keyed identity: the middle row (id 1, "two") is gone; the outer rows
    // survive — not "the widget that now sits at index 1".
    assert_eq!(
        state.notes,
        vec![(0u64, "one".to_string()), (2, "three".to_string())],
        "deleting the middle Delete button removes id 1 (\"two\"), keeping one/three"
    );

    // And after reconciliation the surviving rows still shape their own text —
    // exactly two rows remain.
    let after = frame(&mut root, &mut logic, &mut state, &mut tcx);
    assert_eq!(
        delete_buttons(&after).len(),
        2,
        "two note rows remain after the middle delete"
    );
}
