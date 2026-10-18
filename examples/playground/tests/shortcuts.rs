//! The "Shortcuts" page's key-precedence matrix, as an integration suite over
//! a real `RenderRoot` (no GPU, no window).
//!
//! The matrix is `{text input focused, not focused} x {palette open, closed}
//! x {Mod+K, Mod+B, '1', Escape, Tab, typed 'a'}`; the F2 and `?` chords have
//! their own rows in `function_and_symbol_chords_follow_precedence`. For each
//! cell it asserts
//! exactly who consumed the key: the page's own readout (`last_consumer`),
//! whether the root reported the key handled, whether keyboard focus moved,
//! and the state each consumer owns (palette, pane, tab, filter text, field
//! text). The harness is this file's own: `tests/smoke.rs`'s helpers are
//! private to it.

use std::any::Any;

use frust::{AnyView, GetUntracked, Set};
use frust_core::{
    DiscardScene, FrameTime, InputEvent, Key, KeyEvent, Modifiers, NamedKey, PointerButton,
    PointerEvent, PointerPhase, RenderRoot,
};
use frust_reactive::ReactiveRuntime;
use frust_text::TextContext;
use kurbo::{Point, Size};
use reactive_graph::owner::Owner;

use playground::PlaygroundState;
use playground::pages::{self, SECTION_LABELS};

const W: f64 = 900.0;
const H: f64 = 700.0;

/// Index of the Shortcuts section.
const SECTION: usize = 17;

/// How many Tab presses reach the page's `TextInput` from a clean start: the
/// header "Palette" button comes first, then the focus row `Button, Checkbox, Radio, Slider, TextInput`.
const TABS_TO_INPUT: usize = 6;

/// Where a probe click may land inside the palette while it is open.
const PALETTE_SCAN_Y: std::ops::Range<usize> = 40..280;

/// Where a probe click may land on the focus row: below the header button and
/// above the control row's bottom edge.
const CONTROL_ROW_SCAN_Y: std::ops::Range<usize> = 56..104;

struct Harness {
    _owner: Owner,
    tcx: TextContext,
    root: RenderRoot<PlaygroundState, AnyView<PlaygroundState>>,
    state: PlaygroundState,
    t_ms: u64,
}

/// What one key press did, read off the page's own state and the root.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Effect {
    /// The root reported the key handled.
    handled: bool,
    /// The page readout's new text, `None` if no page handler ran.
    consumer: Option<String>,
    /// For Tab only: where a follow-up Space press lands, which names the
    /// widget Tab focused (`None` for every other key).
    tab_landed_on: Option<String>,
    palette_open: bool,
    pane_open: bool,
    tab: usize,
    query: String,
    field: String,
}

impl Harness {
    fn new() -> Self {
        let _ = ReactiveRuntime::init(std::sync::Arc::new(|| {}));
        let owner = Owner::new();
        owner.set();
        let mut h = Harness {
            _owner: owner,
            tcx: TextContext::new(),
            root: RenderRoot::new(),
            state: PlaygroundState::new(),
            t_ms: 0,
        };
        h.frame();
        // Paint twice: shortcut hints describe the previous frame.
        h.frame();
        h
    }

    fn frame(&mut self) {
        let mut logic = |s: &mut PlaygroundState| pages::current(SECTION, s);
        self.root.rebuild(&mut logic, &mut self.state);
        let tcx: &mut dyn Any = &mut self.tcx;
        self.root.layout_with_text(Size::new(W, H), tcx);
        self.t_ms += 16;
        self.root.paint(
            &mut DiscardScene,
            FrameTime::from_nanos(self.t_ms * 1_000_000),
        );
    }

    /// Mod is Command on macOS/iOS and Ctrl elsewhere.
    fn mod_keys() -> Modifiers {
        if cfg!(any(target_os = "macos", target_os = "ios")) {
            Modifiers {
                meta: true,
                ..Modifiers::default()
            }
        } else {
            Modifiers {
                ctrl: true,
                ..Modifiers::default()
            }
        }
    }

    fn send(&mut self, key: Key, modifiers: Modifiers) -> bool {
        let outcome = self.root.event(
            &mut self.state,
            &InputEvent::Key(KeyEvent {
                key,
                modifiers,
                repeat: false,
            }),
        );
        self.frame();
        outcome.handled
    }

    fn click(&mut self, x: f64, y: f64) {
        for phase in [PointerPhase::Down, PointerPhase::Up] {
            self.root.event(
                &mut self.state,
                &InputEvent::Pointer(PointerEvent {
                    phase,
                    position: Point::new(x, y),
                    button: PointerButton::Primary,
                }),
            );
        }
        self.frame();
    }

    /// Press `key` and report what it did.
    fn press(&mut self, key: Key, modifiers: Modifiers) -> Effect {
        let sc = self.state.shortcuts;
        let before = sc.last_consumer.get_untracked();
        let handled = self.send(key.clone(), modifiers);
        let after = sc.last_consumer.get_untracked();
        let consumer = (after != before).then_some(after);
        let tab_landed_on = (key == Key::Named(NamedKey::Tab)).then(|| self.probe_focus());
        Effect {
            handled,
            consumer,
            tab_landed_on,
            palette_open: sc.palette_open.get_untracked(),
            pane_open: sc.pane_open.get_untracked(),
            tab: sc.tab.get_untracked(),
            query: sc.query.get_untracked(),
            field: sc.field.get_untracked(),
        }
    }

    /// Press Space and name what took it: a focused Button/Checkbox/Radio
    /// reports itself through the readout, a focused TextInput types a space
    /// into its own text, and nothing focused takes it at all.
    fn probe_focus(&mut self) -> String {
        let sc = self.state.shortcuts;
        // Clear the readout so a repeat consumer still shows as a change.
        sc.last_consumer.set("-".into());
        let before = sc.last_consumer.get_untracked();
        let (query, field) = (sc.query.get_untracked(), sc.field.get_untracked());
        self.send(ch(" "), Modifiers::default());
        let after = sc.last_consumer.get_untracked();
        if sc.query.get_untracked() != query {
            "palette filter".into()
        } else if sc.field.get_untracked() != field {
            "page TextInput".into()
        } else if after != before {
            after
        } else {
            "nothing".into()
        }
    }

    /// Move focus onto the page's `TextInput` with Tab.
    fn focus_page_input(&mut self) {
        for _ in 0..TABS_TO_INPUT {
            self.send(Key::Named(NamedKey::Tab), Modifiers::default());
        }
    }

    /// Move focus onto the palette's filter input by clicking inside the open
    /// palette until a typed probe character lands in the filter, then
    /// removing it again.
    fn focus_palette_input(&mut self) {
        let sc = self.state.shortcuts;
        for y in PALETTE_SCAN_Y.step_by(8) {
            for x in (20..420).step_by(20) {
                self.click(x as f64, y as f64);
                self.send(Key::Character("z".into()), Modifiers::default());
                if sc.query.get_untracked() == "z" {
                    self.send(Key::Named(NamedKey::Backspace), Modifiers::default());
                    assert_eq!(sc.query.get_untracked(), "");
                    return;
                }
            }
        }
        panic!("no click inside the palette focused its filter input");
    }

    /// Click the focus row's Checkbox: scan the row from the left until a click
    /// flips the value, as `focus_palette_input` does for the filter.
    fn click_checkbox(&mut self) {
        let sc = self.state.shortcuts;
        for y in CONTROL_ROW_SCAN_Y.step_by(4) {
            for x in (16..240).step_by(4) {
                let before = sc.checked.get_untracked();
                self.click(x as f64, y as f64);
                if sc.checked.get_untracked() != before {
                    return;
                }
            }
        }
        panic!("no click on the focus row toggled the Checkbox");
    }
}

fn ch(s: &str) -> Key {
    Key::Character(s.into())
}

fn named(k: NamedKey) -> Key {
    Key::Named(k)
}

#[test]
fn the_shortcuts_section_is_the_last_label() {
    assert_eq!(SECTION_LABELS[SECTION], "Shortcuts");
    assert_eq!(SECTION_LABELS.len(), SECTION + 1);
}

#[test]
fn the_page_mounts_and_declares_its_hints() {
    let h = Harness::new();
    let hints = h.root.shortcut_hints(|_| true);
    let labels: Vec<&str> = hints.iter().map(|hint| hint.label.as_str()).collect();
    for want in [
        "Open palette",
        "Toggle side pane",
        "Select tab one",
        "Select tab two",
        "Select tab three",
    ] {
        assert!(labels.contains(&want), "{want} missing from {labels:?}");
    }
}

#[test]
fn the_helpers_reach_the_intended_widgets() {
    // Tab x5 lands on the page TextInput: a typed character reaches it.
    let mut h = Harness::new();
    h.focus_page_input();
    let e = h.press(ch("a"), Modifiers::default());
    assert_eq!(e.field, "a");

    // The palette filter input takes typed text once clicked.
    let mut h = Harness::new();
    h.press(ch("k"), Harness::mod_keys());
    assert!(h.state.shortcuts.palette_open.get_untracked());
    h.focus_palette_input();
}

/// One matrix cell: the setup, the key and the expected effect.
struct Cell {
    input_focused: bool,
    palette_open: bool,
    name: &'static str,
    key: Key,
    mods: Modifiers,
}

fn run(cell: &Cell) -> Effect {
    let mut h = Harness::new();
    if cell.palette_open {
        h.press(ch("k"), Harness::mod_keys());
        assert!(h.state.shortcuts.palette_open.get_untracked());
        // Re-mark the readout so a masked key shows no change.
    }
    if cell.input_focused {
        if cell.palette_open {
            h.focus_palette_input();
        } else {
            h.focus_page_input();
        }
    }
    h.press(cell.key.clone(), cell.mods)
}

fn cells() -> Vec<Cell> {
    let keys: [(&str, Key, Modifiers); 6] = [
        ("Mod+K", ch("k"), Harness::mod_keys()),
        ("Mod+B", ch("b"), Harness::mod_keys()),
        ("1", ch("1"), Modifiers::default()),
        ("Escape", named(NamedKey::Escape), Modifiers::default()),
        ("Tab", named(NamedKey::Tab), Modifiers::default()),
        ("a", ch("a"), Modifiers::default()),
    ];
    let mut out = Vec::new();
    for input_focused in [false, true] {
        for palette_open in [false, true] {
            for (name, key, mods) in &keys {
                out.push(Cell {
                    input_focused,
                    palette_open,
                    name,
                    key: key.clone(),
                    mods: *mods,
                });
            }
        }
    }
    out
}

fn describe(e: &Effect) -> String {
    format!(
        "{} consumer={} palette={} pane={} tab={} query={:?} field={:?}{}",
        if e.handled { "handled" } else { "ignored" },
        e.consumer.as_deref().unwrap_or("-"),
        if e.palette_open { "open" } else { "closed" },
        if e.pane_open { "on" } else { "off" },
        e.tab,
        e.query,
        e.field,
        e.tab_landed_on
            .as_ref()
            .map(|w| format!(" focus->{w}"))
            .unwrap_or_default(),
    )
}

/// The precedence matrix. Each row is `(input focused, palette open, key,
/// expected)`; the comments say who consumed the key and why.
#[test]
fn precedence_matrix() {
    const OPEN: &str = "Button: open palette";
    #[rustfmt::skip]
    let expected: [(bool, bool, &str, String); 24] = [
        // -- nothing focused, palette closed: the registry owns the chords.
        (false, false, "Mod+K",  "handled consumer=Mod+K: palette opened palette=open pane=off tab=0 query=\"\" field=\"\"".into()),
        (false, false, "Mod+B",  "handled consumer=Mod+B: pane toggled palette=closed pane=on tab=0 query=\"\" field=\"\"".into()),
        (false, false, "1",      "handled consumer=1: tab one palette=closed pane=off tab=0 query=\"\" field=\"\"".into()),
        // Nobody consumes Escape or a bare letter; Tab is the root's, and the
        // first stop is the header button.
        (false, false, "Escape", "ignored consumer=- palette=closed pane=off tab=0 query=\"\" field=\"\"".into()),
        (false, false, "Tab",    format!("handled consumer=- palette=open pane=off tab=0 query=\"\" field=\"\" focus->{OPEN}")),
        (false, false, "a",      "ignored consumer=- palette=closed pane=off tab=0 query=\"\" field=\"\"".into()),
        // -- nothing focused, palette open: the modal masks every page chord.
        (false, true,  "Mod+K",  "ignored consumer=- palette=open pane=off tab=0 query=\"\" field=\"\"".into()),
        (false, true,  "Mod+B",  "ignored consumer=- palette=open pane=off tab=0 query=\"\" field=\"\"".into()),
        (false, true,  "1",      "ignored consumer=- palette=open pane=off tab=0 query=\"\" field=\"\"".into()),
        // Escape reaches the modal surface even though nothing is focused.
        (false, true,  "Escape", "handled consumer=Escape: palette closed palette=closed pane=off tab=0 query=\"\" field=\"\"".into()),
        // Tab traversal is confined to the modal: it lands on the filter.
        (false, true,  "Tab",    "handled consumer=- palette=open pane=off tab=0 query=\" \" field=\"\" focus->palette filter".into()),
        (false, true,  "a",      "ignored consumer=- palette=open pane=off tab=0 query=\"\" field=\"\"".into()),
        // -- page TextInput focused, palette closed: the input is first in line.
        // It declines a chord it does not decode (never typed), so the
        // registry still opens the palette and toggles the pane.
        (true,  false, "Mod+K",  "handled consumer=Mod+K: palette opened palette=open pane=off tab=0 query=\"\" field=\"\"".into()),
        (true,  false, "Mod+B",  "handled consumer=Mod+B: pane toggled palette=closed pane=on tab=0 query=\"\" field=\"\"".into()),
        // A bare digit is text, not the tab chord: typed text is never lost.
        (true,  false, "1",      "handled consumer=TextInput: typed text palette=closed pane=off tab=0 query=\"\" field=\"1\"".into()),
        // Escape blurs the field and is declined, so nothing answers it.
        (true,  false, "Escape", "ignored consumer=- palette=closed pane=off tab=0 query=\"\" field=\"\"".into()),
        // The input declines Tab, so the root traverses to the next stop.
        (true,  false, "Tab",    "handled consumer=- palette=closed pane=off tab=0 query=\"\" field=\"\" focus->Button: tab 1".into()),
        (true,  false, "a",      "handled consumer=TextInput: typed text palette=closed pane=off tab=0 query=\"\" field=\"a\"".into()),
        // -- palette filter focused, palette open: same input rules, in the modal.
        // The filter declines the chord and the open modal masks it.
        (true,  true,  "Mod+K",  "ignored consumer=- palette=open pane=off tab=0 query=\"\" field=\"\"".into()),
        (true,  true,  "Mod+B",  "ignored consumer=- palette=open pane=off tab=0 query=\"\" field=\"\"".into()),
        (true,  true,  "1",      "handled consumer=- palette=open pane=off tab=0 query=\"1\" field=\"\"".into()),
        // The focused filter blurs and declines Escape, so the modal's close
        // hook takes the first press.
        (true,  true,  "Escape", "handled consumer=Escape: palette closed palette=closed pane=off tab=0 query=\"\" field=\"\"".into()),
        (true,  true,  "Tab",    "handled consumer=- palette=open pane=off tab=0 query=\" \" field=\"\" focus->palette filter".into()),
        (true,  true,  "a",      "handled consumer=- palette=open pane=off tab=0 query=\"a\" field=\"\"".into()),
    ];
    let mut failures = Vec::new();
    for ((input, palette, key, want), cell) in expected.iter().zip(cells()) {
        assert_eq!(
            (*input, *palette, *key),
            (cell.input_focused, cell.palette_open, cell.name),
            "table order must follow cells()"
        );
        let got = describe(&run(&cell));
        if &got != want {
            failures.push(format!(
                "input_focused={input} palette_open={palette} key={key}\n  want: {want}\n  got:  {got}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A single Escape closes the palette while its filter is focused: the focused
/// filter blurs and declines Escape, so the modal's close hook takes the press.
#[test]
fn a_single_escape_closes_the_palette_with_the_filter_focused() {
    let mut h = Harness::new();
    h.press(ch("k"), Harness::mod_keys());
    h.focus_palette_input();
    let e = h.press(named(NamedKey::Escape), Modifiers::default());
    assert!(e.handled, "the modal's close hook consumed the Escape");
    assert!(!e.palette_open, "the first Escape closes the palette");
    assert_eq!(e.consumer.as_deref(), Some("Escape: palette closed"));
    assert_eq!(e.query, "");
}

/// A press on a baseline control keeps keyboard focus on it: Tab from the
/// Checkbox lands on Radio, the next stop, rather than restarting the row.
#[test]
fn a_click_on_a_baseline_control_keeps_keyboard_focus() {
    let mut h = Harness::new();
    h.click_checkbox();
    assert!(h.state.shortcuts.checked.get_untracked());
    h.send(named(NamedKey::Tab), Modifiers::default());
    assert_eq!(h.probe_focus(), "Radio");
}

/// F2 and `?` against the precedence rules. F2 is a function key: the focused
/// field declines it, so the registry fires, and the open modal masks it. `?` is
/// a symbol the field types as text, and opens the palette when nothing takes it.
#[test]
fn function_and_symbol_chords_follow_precedence() {
    const F2_CONSUMER: &str = "F2: tab one rename toggled";
    let shift = Modifiers {
        shift: true,
        ..Modifiers::default()
    };

    // Nothing focused: F2 toggles the rename; `?` (shifted or not) opens the palette.
    let mut h = Harness::new();
    let e = h.press(named(NamedKey::F2), Modifiers::default());
    assert!(e.handled);
    assert_eq!(e.consumer.as_deref(), Some(F2_CONSUMER));
    assert!(h.state.shortcuts.renamed.get_untracked());
    let e = h.press(ch("?"), shift);
    assert!(e.handled);
    assert!(e.palette_open);
    assert_eq!(e.consumer.as_deref(), Some("?: palette opened"));

    // The page field focused: it declines F2, so the registry fires; it types `?`.
    let mut h = Harness::new();
    h.focus_page_input();
    let e = h.press(named(NamedKey::F2), Modifiers::default());
    assert!(e.handled);
    assert_eq!(e.consumer.as_deref(), Some(F2_CONSUMER));
    assert_eq!(e.field, "");
    let e = h.press(ch("?"), shift);
    assert!(!e.palette_open, "a focused field takes the typed `?`");
    assert_eq!(e.consumer.as_deref(), Some("TextInput: typed text"));
    assert_eq!(e.field, "?");

    // Palette open, nothing focused: the modal masks F2 and `?`.
    let mut h = Harness::new();
    h.press(ch("k"), Harness::mod_keys());
    let e = h.press(named(NamedKey::F2), Modifiers::default());
    assert!(!e.handled);
    assert_eq!(e.consumer, None);
    assert!(!h.state.shortcuts.renamed.get_untracked());
    let e = h.press(ch("?"), shift);
    assert!(!e.handled);
    assert_eq!(e.consumer, None);
    assert!(e.palette_open);

    // Palette open, filter focused: F2 is declined and masked; `?` is typed into the filter.
    let mut h = Harness::new();
    h.press(ch("k"), Harness::mod_keys());
    h.focus_palette_input();
    let e = h.press(named(NamedKey::F2), Modifiers::default());
    assert!(!e.handled);
    assert!(!h.state.shortcuts.renamed.get_untracked());
    let e = h.press(ch("?"), shift);
    assert!(e.palette_open);
    assert_eq!(e.query, "?");
}

/// Shift+Tab walks backward from the start to the last stop, and Tab walks the
/// focus row in order after the header button: Button, Checkbox, Radio, Slider.
#[test]
fn tab_walks_the_focus_row_in_order_and_shift_tab_reverses() {
    let mut h = Harness::new();
    // The first stop is the header's palette button; probing it would open the
    // palette, so step onto it without probing.
    h.send(named(NamedKey::Tab), Modifiers::default());
    let mut order = Vec::new();
    for _ in 0..4 {
        h.send(named(NamedKey::Tab), Modifiers::default());
        order.push(h.probe_focus());
    }
    assert_eq!(
        order,
        [
            "Button", "Checkbox", "Radio",
            // Space on a Slider changes nothing and reports nothing.
            "nothing"
        ]
    );
    let back = Modifiers {
        shift: true,
        ..Modifiers::default()
    };
    h.send(named(NamedKey::Tab), back);
    assert_eq!(h.probe_focus(), "Radio");
}

/// The palette lists the live hints, each with its `Chord::display` label, and
/// the filter narrows them.
#[test]
fn the_palette_lists_hints_and_the_filter_narrows_them() {
    let mut h = Harness::new();
    h.press(ch("k"), Harness::mod_keys());
    let hints = h.root.shortcut_hints(|_| true);
    assert!(hints.iter().any(|hint| hint.label == "Toggle side pane"));
    let shown = frust::Chord::parse("Mod+B").unwrap().display();
    assert!(
        hints
            .iter()
            .any(|hint| hint.chord.display() == shown && hint.label == "Toggle side pane")
    );
    h.focus_palette_input();
    for c in ["p", "a", "n", "e"] {
        h.send(ch(c), Modifiers::default());
    }
    assert_eq!(h.state.shortcuts.query.get_untracked(), "pane");
}

/// Mod+B toggles twice, and '1'/'2'/'3' select their tab with nothing focused.
#[test]
fn chords_drive_the_pane_and_the_tabs() {
    let mut h = Harness::new();
    assert!(h.press(ch("b"), Harness::mod_keys()).pane_open);
    assert!(!h.press(ch("b"), Harness::mod_keys()).pane_open);
    assert_eq!(h.press(ch("2"), Modifiers::default()).tab, 1);
    assert_eq!(h.press(ch("3"), Modifiers::default()).tab, 2);
    assert_eq!(h.press(ch("1"), Modifiers::default()).tab, 0);
}
