//! Headless interaction proof for text input / IME / keyed-children / image
//! wiring — ported from the (now-deleted) `examples/notes` demo so
//! the unconditional workspace gate keeps this coverage.
//!
//! Desktop UI automation (osascript/cliclick) is unavailable on this host, so
//! rather than driving a live GPU window this test feeds synthetic
//! [`InputEvent`]s straight into the framework's [`RenderRoot`] — the exact
//! event seam the desktop shell's `dispatch` uses (`RenderRoot::event`) and the
//! mobile shells reach through `AppTree::event`. Text is shaped through a real
//! `TextContext` (CPU-only, no GPU) and paint output is recorded, so every
//! assertion is deterministic and this doubles as a permanent regression gate
//! for `TextInput` / keyed-list / `Image` wiring.
//!
//! Coverage: the logo renders and the placeholder is
//! visible; focusing the field and typing fires `on_change` once per keystroke;
//! Enter submits a keyed note and clears the draft; select-all + type replaces
//! the whole field; the arrow/backspace editing matrix; the IME compose→commit
//! sequence; and the keyed-identity headline — adding three notes and deleting
//! the *middle* one by its own Delete button removes the right row, not
//! whatever index it sat at. Decode-once is asserted across many frames.
//!
//! A second fixture (see *Disabled and obscured fields*, below) drives the same
//! event seam over a bare `text_input` to cover
//! [`frust::TextInputView::enabled`]/[`frust::TextInputView::obscured`]: a
//! disabled field refuses focus and stays inert through the whole
//! pointer/key/IME matrix, and an obscured field paints bullets while every
//! edit still lands on the real value — including across a multi-byte grapheme.

use std::any::Any;
use std::sync::atomic::{AtomicUsize, Ordering};

use frust::{
    Axis, Button, Column, Component, FlexView, Image, ImageFit, ImageSource, Row, SizedBox,
    TextInputView, any, keyed, scroll_view, text, text_input,
};
use frust_core::{
    AnyView, FrameTime, ImeEvent, InputEvent, Key, KeyEvent, Modifiers, NamedKey, PaintScene,
    PointerButton, PointerEvent, PointerPhase, RenderRoot, View,
};
use frust_scene::GlyphRun;
use frust_text::TextContext;
use kurbo::{Point, Rect, Size};
use peniko::Color;

const W: f64 = 800.0;
const H: f64 = 600.0;

// --- In-test fixture app (ported from examples/notes/src/lib.rs) ---

/// A tiny in-repo test PNG (independent of `examples/notes`' asset, which is
/// deleted alongside the example — see `crates/frust/tests/assets/`).
const LOGO_PNG: &[u8] = include_bytes!("assets/test-logo.png");

/// How many times the logo has actually been decoded this process. The
/// demo's contract is **decode-once per run**: [`ImageSource`] lives in
/// [`AppState`] and is re-passed (a cheap `Arc` clone) into [`Image`] every
/// frame, so a single app run decodes exactly once no matter how many frames
/// render — this counter increments only inside [`AppState::new`], never in
/// [`NotesApp::build`].
static LOGO_DECODES: AtomicUsize = AtomicUsize::new(0);

/// Decode the embedded logo, counting the decode. Called exactly once per app
/// run, from [`AppState::new`] — never from the per-frame [`NotesApp::build`],
/// which only clones the resulting `Arc` handle.
fn decode_logo() -> ImageSource {
    LOGO_DECODES.fetch_add(1, Ordering::Relaxed);
    ImageSource::decode(LOGO_PNG).expect("the embedded logo PNG is valid")
}

/// The number of times the logo has been decoded this process — read by the
/// test to assert decode-once *across frames* (the count must not grow while
/// [`NotesApp::build`] runs, only when a new [`AppState`] is constructed).
fn logo_decode_count() -> usize {
    LOGO_DECODES.load(Ordering::Relaxed)
}

/// Notes demo state (the `app_logic` model).
///
/// The view is a pure function of these fields. `draft` is the controlled
/// [`text_input`]'s current text; `notes` is the submitted list, each entry a
/// `(stable id, text)` pair so the keyed rows survive a middle-of-list delete;
/// `next_id` hands out the ids.
struct AppState {
    /// The in-progress note text bound to the field (controlled component).
    draft: String,
    /// Submitted notes: `(stable id, text)`. The id is the [`keyed`] identity.
    notes: Vec<(u64, String)>,
    /// The next id to assign — monotonic, never reused, so keys stay unique.
    next_id: u64,
    /// The decode-once logo handle, re-passed into [`Image`] every frame.
    logo: ImageSource,
}

impl AppState {
    /// Build the initial state, decoding the embedded logo once.
    fn new() -> Self {
        Self {
            draft: String::new(),
            notes: Vec::new(),
            next_id: 0,
            logo: decode_logo(),
        }
    }

    /// Submit the current `draft` as a new keyed note and clear the field.
    ///
    /// Empty/whitespace-only drafts are dropped (still clearing the field), so a
    /// stray Enter never appends a blank row.
    fn submit_draft(&mut self, text: String) {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            let id = self.next_id;
            self.next_id += 1;
            self.notes.push((id, trimmed.to_string()));
        }
        self.draft.clear();
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

/// The notes demo's root [`Component`]: retained state is the [`AppState`]
/// above.
struct NotesApp;

impl Component for NotesApp {
    type State = AppState;

    /// Seed the initial state, decoding the embedded logo exactly once (see
    /// [`decode_logo`]) — never called again for the life of the app.
    fn init(&self) -> AppState {
        AppState::new()
    }

    /// Pure view function: renders `AppState` into the notes screen.
    /// Re-run every frame, so it is cheap by construction — the only non-trivial
    /// resource, the decoded logo, is an `Arc` clone, never a re-decode.
    fn build(&self, state: &mut AppState) -> AnyView<AppState> {
        let logo = state.logo.clone();
        let draft = state.draft.clone();

        // The submitted notes, each a keyed row: the note text plus a Delete button
        // that removes *this* id. The stable key means deleting the middle row drops
        // the middle row's widget, not whatever now sits at that index.
        let rows: Vec<frust::FlexChild<AppState>> = state
            .notes
            .iter()
            .map(|(id, note)| {
                let id = *id;
                keyed(
                    id,
                    Row(vec![
                        any(text(note.clone()).size(20.0)),
                        any(SizedBox(Some(12.0), None)),
                        any(Button("Delete", move |s: &mut AppState| {
                            s.notes.retain(|(nid, _)| *nid != id);
                        })),
                    ]),
                )
            })
            .collect();

        let mut children: Vec<AnyView<AppState>> = Vec::with_capacity(4);
        // The logo sizes to its natural box (see [`Image`] layout) and `Contain`-fits
        // into it — a square source, so it fills exactly with no letterbox.
        children.push(any(Image(logo).fit(ImageFit::Contain)));
        children.push(any(text("Notes").size(32.0)));
        children.push(any(text_input(draft, |s: &mut AppState, v: String| {
            s.draft = v;
        })
        .placeholder("Write a note, then press Enter")
        .on_submit(|s: &mut AppState, v: String| {
            s.submit_draft(v);
        })));
        children.push(any(scroll_view(FlexView::new(Axis::Vertical, rows))));

        any(Column(children))
    }
}

// --- Test harness ---

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
    root.paint(&mut scene, FrameTime::ZERO);
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
    let mut logic = |s: &mut AppState| NotesApp.build(s);
    let mut state = NotesApp.init();
    let mut tcx = TextContext::new();

    // At least one decode happened (the startup decode in `NotesApp::init`, via
    // `AppState::new`). The exact process-wide count is not asserted here
    // because sibling tests run in parallel and share this global — decode-once
    // is proved below by Arc identity, which is per-run and race-free.
    assert!(logo_decode_count() >= 1, "the logo decoded at startup");

    // The decode-once contract, proved by pointer identity: keep a handle to the
    // startup-decoded source, render many frames, and confirm `state.logo` is
    // still the *same* `Arc` — `NotesApp::build` only ever clones it, never
    // re-decodes (a re-decode would allocate a fresh, non-`same` Arc).
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
    let mut logic = |s: &mut AppState| NotesApp.build(s);
    let mut state = NotesApp.init();
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
    let mut logic = |s: &mut AppState| NotesApp.build(s);
    let mut state = NotesApp.init();
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
    let mut logic = |s: &mut AppState| NotesApp.build(s);
    let mut state = NotesApp.init();
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
    let mut logic = |s: &mut AppState| NotesApp.build(s);
    let mut state = NotesApp.init();
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
    let mut logic = |s: &mut AppState| NotesApp.build(s);
    let mut state = NotesApp.init();
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
    let mut logic = |s: &mut AppState| NotesApp.build(s);
    let mut state = NotesApp.init();
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
    let mut logic = |s: &mut AppState| NotesApp.build(s);
    let mut state = NotesApp.init();
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

// --- Disabled and obscured fields ---

/// A minimal one-field app state: the controlled value plus an `on_change`
/// counter, so a test can tell "no edit happened" from "an edit was reverted".
#[derive(Default)]
struct FieldState {
    value: String,
    changes: u32,
}

/// A bare controlled `text_input` with `enabled`/`obscured` dialled in.
fn field_logic(
    enabled: bool,
    obscured: bool,
) -> impl FnMut(&mut FieldState) -> TextInputView<FieldState> {
    move |state: &mut FieldState| {
        text_input(state.value.clone(), |s: &mut FieldState, v: String| {
            s.changes += 1;
            s.value = v;
        })
        .placeholder("secret")
        .enabled(enabled)
        .obscured(obscured)
    }
}

/// Build + lay out a root over `logic`, ready for events.
fn field_root(
    logic: &mut impl FnMut(&mut FieldState) -> TextInputView<FieldState>,
    state: &mut FieldState,
    tcx: &mut TextContext,
) -> RenderRoot<FieldState, TextInputView<FieldState>> {
    let mut root = RenderRoot::new();
    root.rebuild(logic, state);
    let tcx_any: &mut dyn Any = tcx;
    root.layout_with_text(Size::new(W, H), tcx_any);
    root
}

/// Records the glyph ids of every painted run, in order — enough to prove two
/// fields shape the *same characters* without a GPU.
#[derive(Default)]
struct GlyphIdScene {
    ids: Vec<u32>,
}

impl PaintScene for GlyphIdScene {
    fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
    fn fill_rounded_rect(&mut self, _o: Point, _s: Size, _r: f64, _c: Color) {}
    fn draw_text(&mut self, _origin: Point, _text: &str) {}
    fn draw_glyph_run(&mut self, run: GlyphRun) {
        self.ids.extend(run.glyphs.iter().map(|g| g.id));
    }
}

/// Paint `root` and return every painted glyph id.
fn glyph_ids(root: &mut RenderRoot<FieldState, TextInputView<FieldState>>) -> Vec<u32> {
    let mut scene = GlyphIdScene::default();
    root.paint(&mut scene, FrameTime::ZERO);
    scene.ids
}

#[test]
fn disabled_field_is_inert_through_the_whole_input_matrix() {
    let mut tcx = TextContext::new();
    let mut state = FieldState::default();
    let mut logic = field_logic(false, false);
    let mut root = field_root(&mut logic, &mut state, &mut tcx);

    // A tap neither focuses nor captures, so nothing is consumed.
    let outcome = root.event(&mut state, &pointer(PointerPhase::Down, 20.0, 20.0));
    assert!(
        !outcome.handled,
        "a disabled field consumes no pointer event"
    );
    assert!(!root.is_focus_active(), "…and never takes focus");
    assert!(
        root.ime_state().is_none(),
        "…so no IME surface is published"
    );

    // Keys and IME are focus-routed, so they cannot reach an unfocusable field;
    // injecting them anyway must still change nothing.
    for event in [
        ch("x"),
        named(NamedKey::Backspace, Modifiers::default()),
        named(NamedKey::Enter, Modifiers::default()),
        InputEvent::Ime(ImeEvent::Commit("ni".to_string())),
        InputEvent::Ime(ImeEvent::Compose {
            text: "ni".to_string(),
            cursor: Some((2, 2)),
        }),
    ] {
        root.event(&mut state, &event);
    }
    assert_eq!(state.value, "", "a disabled field is inert to every input");
    assert_eq!(state.changes, 0, "on_change never fires");

    // Drag-selection is armed by the same `Down` we refused, so a Move/Up pair
    // is inert too.
    root.event(&mut state, &pointer(PointerPhase::Move, 60.0, 20.0));
    root.event(&mut state, &pointer(PointerPhase::Up, 60.0, 20.0));
    assert_eq!(state.changes, 0);
}

#[test]
fn disabling_a_focused_field_drops_the_keyboard_and_releases_focus() {
    let mut tcx = TextContext::new();
    let mut state = FieldState::default();
    let mut enabled = field_logic(true, false);
    let mut root = field_root(&mut enabled, &mut state, &mut tcx);

    root.event(&mut state, &pointer(PointerPhase::Down, 20.0, 20.0));
    root.event(&mut state, &ch("h"));
    assert_eq!(state.value, "h");
    assert!(root.ime_state().expect("focused").active);

    // Flip to disabled on a rebuild while the field is focused.
    let mut disabled = field_logic(false, false);
    root.rebuild(&mut disabled, &mut state);
    let tcx_any: &mut dyn Any = &mut tcx;
    root.layout_with_text(Size::new(W, H), tcx_any);
    let mut scene = RecScene::default();
    let outcome = root.paint(&mut scene, FrameTime::ZERO);

    assert!(
        root.ime_state().is_none(),
        "the next paint publishes an inactive IME surface, which releases the session \
         (the shells serialise `None` to that same inactive wire form, so the keyboard \
         still drops — see `RenderRoot::ime_state`)"
    );
    assert!(
        !root.is_focus_active(),
        "the released session takes the root's focus mirror with it"
    );
    assert!(
        !outcome.needs_frame,
        "a disabled field stops asking for caret-blink frames"
    );

    // The first event to reach the field releases the stranded focus path, and
    // edits nothing on its way out.
    root.event(&mut state, &ch("i"));
    assert!(!root.is_focus_active(), "focus is released");
    assert!(root.ime_state().is_none());
    assert_eq!(state.value, "h", "the key never edited the field");
    assert_eq!(state.changes, 1, "only the pre-disable keystroke counted");
}

#[test]
fn obscured_field_paints_bullets_while_editing_the_real_value() {
    let mut tcx = TextContext::new();
    let mut state = FieldState::default();
    let mut logic = field_logic(true, true);
    let mut root = field_root(&mut logic, &mut state, &mut tcx);

    // Type a secret containing a 4-byte grapheme.
    root.event(&mut state, &pointer(PointerPhase::Down, 20.0, 20.0));
    for c in "pa\u{1F600}ss".chars() {
        root.event(&mut state, &ch(&c.to_string()));
    }
    assert_eq!(
        state.value, "pa\u{1F600}ss",
        "the app still receives the real text through on_change"
    );
    assert_eq!(
        root.ime_state().expect("focused").editing.text,
        "pa\u{1F600}ss",
        "the platform IME mirror is fed the real text (documented consequence)"
    );

    // What is *painted* is one bullet per char — byte-identical to a clear
    // field holding the equivalent bullet string, and different from a clear
    // field holding the secret itself.
    let secret_ids = glyph_ids(&mut root);

    let mut clear_logic = field_logic(true, false);
    let mut bullets_state = FieldState {
        value: "\u{2022}".repeat(5),
        ..FieldState::default()
    };
    let mut bullets = field_root(&mut clear_logic, &mut bullets_state, &mut tcx);
    let mut clear_state = FieldState {
        value: "pa\u{1F600}ss".to_string(),
        ..FieldState::default()
    };
    let mut clear = field_root(&mut clear_logic, &mut clear_state, &mut tcx);

    assert_eq!(
        secret_ids,
        glyph_ids(&mut bullets),
        "an obscured field paints one U+2022 per character (the emoji included)"
    );
    assert_ne!(
        secret_ids,
        glyph_ids(&mut clear),
        "…and never the real characters"
    );
}

#[test]
fn obscured_editing_matches_a_clear_field_across_a_multi_byte_grapheme() {
    let mut tcx = TextContext::new();
    let mut secret_state = FieldState::default();
    let mut secret_logic = field_logic(true, true);
    let mut secret = field_root(&mut secret_logic, &mut secret_state, &mut tcx);
    let mut clear_state = FieldState::default();
    let mut clear_logic = field_logic(true, false);
    let mut clear = field_root(&mut clear_logic, &mut clear_state, &mut tcx);

    let plain = Modifiers::default();
    let meta = Modifiers {
        meta: true,
        ..Modifiers::default()
    };
    // Caret motion over a 4-byte grapheme, a backspace that must remove the
    // whole emoji, and select-all + replace — masking must not perturb any of
    // the index arithmetic these depend on.
    let script = vec![
        pointer(PointerPhase::Down, 20.0, 20.0),
        ch("a"),
        ch("\u{1F600}"),
        ch("b"),
        named(NamedKey::ArrowLeft, plain),
        named(NamedKey::Backspace, plain),
        named(NamedKey::End, plain),
        ch("z"),
        InputEvent::Key(KeyEvent {
            key: Key::Character("a".to_string()),
            modifiers: meta,
            repeat: false,
        }),
        ch("q"),
    ];
    for (i, event) in script.iter().enumerate() {
        secret.event(&mut secret_state, event);
        clear.event(&mut clear_state, event);
        assert_eq!(
            secret_state.value, clear_state.value,
            "obscured editing diverged from clear editing at step {i}"
        );
        assert_eq!(secret_state.changes, clear_state.changes);
    }
    assert_eq!(
        secret_state.value, "q",
        "select-all + type replaced the field"
    );
    // The backspace mid-script removed the emoji as one grapheme in both.
    assert_eq!(clear_state.value, "q");
}
