//! Per-pod layout skipping, proven against the layout audit.
//!
//! A `RenderRoot` returns a clean pod's recorded size without calling its
//! `layout`, so a widget that changes what its layout reads without signalling
//! dirty (a rebuild `LAYOUT`, a paint `request_layout`, a consumed event) lays
//! out stale. `RenderRoot::set_layout_audit` catches exactly that: after the
//! skipping pass it runs a forced full pass and counts every pod whose geometry
//! differs. Every scenario below runs with the audit on and asserts it found
//! nothing; the harness also names the offending widget types, by replaying the
//! audit's two passes itself, so a regression points at its widget.
//!
//! The widgets here are the ones the layout-skip audit fixed: the select,
//! combobox and multi-select lists' trigger-width floor (an anchor cell the
//! trigger's paint writes), the filtering lists' row presences, the dynamic
//! island / expandable tabs / notification stack boxes (a reduce-motion snap
//! lands them in `paint` without asking for a frame), and the input's message
//! row, which stops taking room on the paint its exit lands on.

use std::any::Any;
use std::collections::HashMap;

use frust::authoring::text::TextContext;
use frust::authoring::{AnyView, DiscardScene, PaintOutcome, PaintScene, WidgetId, any};
use frust::{SizedBox, Theme, column, stack};
use frust_beui::blocks::dynamic_island::{dynamic_island, dynamic_island_slot};
use frust_beui::blocks::expandable_tabs::{expandable_tabs, expandable_tabs_item};
use frust_beui::blocks::notification_stack::{notification, notification_stack};
use frust_beui::components::combobox::{combobox, combobox_option, combobox_trigger};
use frust_beui::components::input::input;
use frust_beui::components::multi_select::{
    multi_select, multi_select_option, multi_select_trigger,
};
use frust_beui::components::select::{select, select_option, select_trigger};
use frust_beui::overlay::OverlayAnchor;
use frust_core::{FrameTime, RenderRoot};
use kurbo::{Point, Size};

// ---------------------------------------------------------------------------
// The audit harness.
// ---------------------------------------------------------------------------

/// One frame's interval, in nanoseconds (60 Hz).
const FRAME_NS: u64 = 16_666_667;

/// A pod's geometry as the audit compares it: origin relative to its parent,
/// and size.
type Geometry = HashMap<WidgetId, (&'static str, Point, Size)>;

fn geometry<S: 'static>(root: &RenderRoot<S, AnyView<S>>) -> Geometry {
    let nodes = root.inspect();
    let origins: HashMap<WidgetId, Point> =
        nodes.iter().map(|n| (n.id, n.bounds.origin())).collect();
    nodes
        .into_iter()
        .map(|n| {
            let parent = n
                .parent
                .and_then(|p| origins.get(&p).copied())
                .unwrap_or(Point::ZERO);
            let origin = Point::new(n.bounds.x0 - parent.x, n.bounds.y0 - parent.y);
            (n.id, (n.type_name, origin, n.bounds.size()))
        })
        .collect()
}

/// The app's view function, as the harness holds it.
type ViewFn<S> = Box<dyn FnMut(&mut S) -> AnyView<S>>;

/// A mounted view driven frame by frame with the layout audit on.
struct Audit<S: 'static> {
    root: RenderRoot<S, AnyView<S>>,
    view: ViewFn<S>,
    state: S,
    text: TextContext,
    window: Size,
    now: u64,
    /// `type_name` of every pod the replayed audit found stale.
    offenders: Vec<&'static str>,
}

impl<S: 'static> Audit<S> {
    fn new(state: S, theme: Theme, view: impl FnMut(&mut S) -> AnyView<S> + 'static) -> Self {
        let mut audit = Audit {
            root: RenderRoot::new(),
            view: Box::new(view),
            state,
            text: TextContext::new(),
            window: Size::new(400.0, 600.0),
            now: 0,
            offenders: Vec::new(),
        };
        audit.root.set_theme(Box::new(theme));
        audit
    }

    /// The audit's own two passes, replayed so the offenders can be named: the
    /// skipping pass with the audit off, a snapshot, then the audited layout —
    /// whose skipping pass now finds every pod clean, so it is exactly the
    /// forced pass — and a second snapshot to compare.
    fn layout(&mut self) {
        self.root.set_layout_audit(false);
        self.root
            .layout_with_text(self.window, &mut self.text as &mut dyn Any);
        let skipped = geometry(&self.root);
        self.root.set_layout_audit(true);
        self.root
            .layout_with_text(self.window, &mut self.text as &mut dyn Any);
        for (id, (name, origin, size)) in geometry(&self.root) {
            if let Some((_, was_origin, was_size)) = skipped.get(&id) {
                let moved = (origin - *was_origin).hypot() > 1e-3
                    || (size.width - was_size.width).abs() > 1e-3
                    || (size.height - was_size.height).abs() > 1e-3;
                if moved {
                    self.offenders.push(name);
                }
            }
        }
    }

    fn paint(&mut self, scene: &mut dyn PaintScene) -> PaintOutcome {
        let outcome = self.root.paint(scene, FrameTime::from_nanos(self.now));
        self.now += FRAME_NS;
        outcome
    }

    /// A frame the app's state may have changed for: rebuild, layout, paint.
    fn frame(&mut self) -> PaintOutcome {
        self.root.rebuild(&mut self.view, &mut self.state);
        self.layout();
        self.paint(&mut DiscardScene)
    }

    /// A frame nothing rebuilt for (an animation tick, a paint-driven change).
    fn tick(&mut self) -> PaintOutcome {
        self.layout();
        self.paint(&mut DiscardScene)
    }

    /// Tick until nothing owes a frame, then twice more.
    fn settle(&mut self) {
        for _ in 0..600 {
            if !self.tick().needs_frame {
                break;
            }
        }
        self.tick();
        self.tick();
    }

    fn assert_clean(&self, scenario: &str) {
        assert!(
            self.offenders.is_empty(),
            "{scenario}: stale layout under skipping in {:?}",
            self.offenders
        );
        assert_eq!(
            self.root.layout_audit_mismatches(),
            0,
            "{scenario}: the layout audit reported mismatches"
        );
    }
}

fn light() -> Theme {
    frust_beui::theme()
}

fn reduced() -> Theme {
    let mut theme = frust_beui::theme();
    theme.motion.reduce_motion = true;
    theme
}

fn both_motions() -> [Theme; 2] {
    [light(), reduced()]
}

// ---------------------------------------------------------------------------
// The anchored lists' width floor follows their trigger.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Lists {
    trigger_width: f64,
    query: String,
}

/// Which anchored list a scenario mounts.
#[derive(Clone, Copy, Debug)]
enum List {
    Select,
    Combobox,
    MultiSelect,
}

fn list_view(which: List, anchor: &OverlayAnchor, state: &Lists) -> AnyView<Lists> {
    let fruits = ["Apple", "Banana", "Cherry", "Damson"];
    let trigger: AnyView<Lists> = match which {
        List::Select => any(select_trigger(anchor, Some("Apple".to_string()))),
        List::Combobox => any(combobox_trigger(
            anchor,
            state.query.clone(),
            |_: &mut Lists, _| {},
        )),
        List::MultiSelect => any(multi_select_trigger(
            anchor,
            fruits.iter().map(|f| multi_select_option(*f)).collect(),
            vec![0],
            state.query.clone(),
            |_: &mut Lists, _| {},
        )),
    };
    let list: AnyView<Lists> = match which {
        List::Select => any(select(
            fruits.iter().map(|f| select_option(*f)).collect(),
            Some(0),
            |_: &mut Lists, _| {},
        )
        .anchor(anchor)
        .open(true)),
        List::Combobox => any(combobox(
            fruits.iter().map(|f| combobox_option(*f)).collect(),
            Some(0),
            state.query.clone(),
            |_: &mut Lists, _| {},
        )
        .anchor(anchor)
        .open(true)),
        List::MultiSelect => any(multi_select(
            fruits.iter().map(|f| multi_select_option(*f)).collect(),
            vec![0],
            state.query.clone(),
            |_: &mut Lists, _| {},
        )
        .anchor(anchor)
        .open(true)),
    };
    any(stack()
        .child(SizedBox(Some(state.trigger_width), Some(40.0)).child(trigger))
        .child(list))
}

fn list_audit(which: List, theme: Theme) -> Audit<Lists> {
    let anchor = OverlayAnchor::new();
    Audit::new(
        Lists {
            trigger_width: 150.0,
            query: String::new(),
        },
        theme,
        move |state: &mut Lists| list_view(which, &anchor, state),
    )
}

#[test]
fn anchored_lists_follow_a_trigger_that_resizes() {
    for which in [List::Select, List::Combobox, List::MultiSelect] {
        for theme in both_motions() {
            let mut audit = list_audit(which, theme);
            audit.frame();
            audit.settle();
            for width in [340.0, 200.0, 380.0] {
                audit.state.trigger_width = width;
                // One rebuild for the resize, then only ticks: the trigger's
                // paint writes the anchor cell after the list's last layout.
                audit.frame();
                audit.settle();
            }
            audit.assert_clean(&format!("{which:?} over a resizing trigger"));
        }
    }
}

#[test]
fn filtering_lists_relayout_as_their_rows_come_and_go() {
    for which in [List::Combobox, List::MultiSelect] {
        for theme in both_motions() {
            let mut audit = list_audit(which, theme);
            audit.frame();
            audit.settle();
            for query in ["an", "zzz", "", "ch", ""] {
                audit.state.query = query.to_string();
                audit.frame();
                audit.settle();
            }
            audit.assert_clean(&format!("{which:?} filtering"));
        }
    }
}

// ---------------------------------------------------------------------------
// The sized blocks: island, expandable tabs, notification stack.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Blocks {
    island: Option<String>,
    tab: Option<String>,
    expanded: bool,
}

fn blocks(state: &mut Blocks) -> AnyView<Blocks> {
    any(column()
        .child(dynamic_island(
            state.island.clone(),
            SizedBox(Some(60.0), Some(16.0)),
            vec![
                dynamic_island_slot("music", SizedBox(Some(220.0), Some(64.0))),
                dynamic_island_slot("call", SizedBox(Some(160.0), Some(40.0))),
            ],
        ))
        .child(expandable_tabs(
            state.tab.clone(),
            vec![
                expandable_tabs_item(
                    "home",
                    "Home",
                    SizedBox(Some(16.0), Some(16.0)),
                    SizedBox(Some(120.0), Some(60.0)),
                ),
                expandable_tabs_item(
                    "inbox",
                    "Inbox",
                    SizedBox(Some(16.0), Some(16.0)),
                    SizedBox(Some(120.0), Some(90.0)),
                ),
            ],
            |state: &mut Blocks, tab| state.tab = tab,
        ))
        .child(notification_stack(
            vec![
                notification("a", "Build finished"),
                notification("b", "Review requested").description("2 files"),
                notification("c", "Deploy queued"),
            ],
            state.expanded,
            |state: &mut Blocks, expanded| state.expanded = expanded,
        ))
        .child(SizedBox(Some(100.0), Some(20.0))))
}

#[test]
fn the_sized_blocks_stay_laid_out_through_their_morphs() {
    for theme in both_motions() {
        let mut audit = Audit::new(Blocks::default(), theme, blocks);
        audit.frame();
        audit.settle();
        let steps: [(Option<&str>, Option<&str>, bool); 4] = [
            (Some("music"), Some("home"), true),
            (Some("call"), Some("inbox"), false),
            (None, None, true),
            (Some("music"), Some("home"), false),
        ];
        for (island, tab, expanded) in steps {
            audit.state.island = island.map(str::to_string);
            audit.state.tab = tab.map(str::to_string);
            audit.state.expanded = expanded;
            audit.frame();
            audit.settle();
        }
        audit.assert_clean("island + expandable tabs + notification stack");
    }
}

// ---------------------------------------------------------------------------
// The input's message row.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Field {
    error: Option<String>,
}

fn field(state: &mut Field) -> AnyView<Field> {
    let mut email = input(String::from("ada@"), |_: &mut Field, _| {}).label("Email");
    if let Some(error) = &state.error {
        email = email.error(error.clone());
    }
    any(column()
        .child(email)
        .child(SizedBox(Some(100.0), Some(20.0))))
}

#[test]
fn an_input_message_row_comes_and_goes_through_layout() {
    for theme in both_motions() {
        let mut audit = Audit::new(Field::default(), theme, field);
        audit.frame();
        audit.settle();
        for error in [Some("Enter a valid email"), None, Some("Required"), None] {
            audit.state.error = error.map(str::to_string);
            audit.frame();
            audit.settle();
        }
        audit.assert_clean("input message row");
    }
}
