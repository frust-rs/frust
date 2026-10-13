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
//! Covered: the dropdown panel's and the docked search view's widths (taken
//! from an anchor cell the trigger's paint writes), the anchored host's
//! placement against a trigger that moves, and the animated-height expandable
//! list `docs/REVIEW_FOCUS.md` names — each under both motion settings.

use std::any::Any;
use std::collections::HashMap;

use frust::authoring::text::TextContext;
use frust::authoring::{AnyView, DiscardScene, PaintOutcome, PaintScene, WidgetId, any};
use frust::{SizedBox, Theme, column, stack};
use frust_core::{FrameTime, RenderRoot};
use frust_material::overlay::{OverlayAnchor, anchored_overlay, overlay_anchor};
use frust_material::{
    dropdown, dropdown_field, dropdown_item, expandable_item, expandable_list, search_view,
};
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
    frust_material::baseline()
}

fn reduced() -> Theme {
    let mut theme = frust_material::baseline();
    theme.motion.reduce_motion = true;
    theme
}

fn both_motions() -> [Theme; 2] {
    [light(), reduced()]
}

// ---------------------------------------------------------------------------
// The dropdown panel takes its width from the field.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Picker {
    field_width: f64,
}

fn picker(anchor: &OverlayAnchor, state: &Picker) -> AnyView<Picker> {
    let items = || {
        vec![
            dropdown_item("Apple", "apple"),
            dropdown_item("Banana", "banana"),
            dropdown_item("Cherry", "cherry"),
        ]
    };
    any(stack()
        .child(
            SizedBox(Some(state.field_width), Some(56.0))
                .child(dropdown_field::<Picker>(items()).anchor(anchor).open(true)),
        )
        .child(
            dropdown(items(), |_: &mut Picker, _| {})
                .anchor(anchor)
                .open(true),
        ))
}

#[test]
fn a_dropdown_panel_follows_a_field_that_resizes() {
    for theme in both_motions() {
        let anchor = OverlayAnchor::new();
        let mut audit = Audit::new(
            Picker { field_width: 180.0 },
            theme,
            move |state: &mut Picker| picker(&anchor, state),
        );
        audit.frame();
        audit.settle();
        for width in [300.0, 220.0, 360.0] {
            audit.state.field_width = width;
            // One rebuild for the resize, then only ticks: the field's paint
            // writes the anchor cell after the panel's last layout.
            audit.frame();
            audit.settle();
        }
        audit.assert_clean("dropdown panel over a resizing field");
    }
}

// ---------------------------------------------------------------------------
// The docked search view takes its width from the bar's anchor; an anchored
// host re-places its content when the anchor moves.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Docked {
    bar_width: f64,
    drop: f64,
}

fn docked(anchor: &OverlayAnchor, state: &Docked) -> AnyView<Docked> {
    any(stack()
        .child(
            column()
                .child(SizedBox(Some(10.0), Some(state.drop)))
                .child(overlay_anchor(
                    anchor,
                    SizedBox(Some(state.bar_width), Some(56.0)),
                )),
        )
        .child(
            search_view(String::new(), |_: &mut Docked, _| {})
                .docked(anchor)
                .open(true),
        ))
}

fn hosted(anchor: &OverlayAnchor, state: &Docked) -> AnyView<Docked> {
    any(stack()
        .child(
            column()
                .child(SizedBox(Some(10.0), Some(state.drop)))
                .child(overlay_anchor(
                    anchor,
                    SizedBox(Some(state.bar_width), Some(40.0)),
                )),
        )
        .child(anchored_overlay(SizedBox(Some(160.0), Some(80.0))).anchor(anchor)))
}

/// One anchored-surface scene: the anchor and the app state to its view.
type Scene = fn(&OverlayAnchor, &Docked) -> AnyView<Docked>;

#[test]
fn anchored_surfaces_follow_their_anchor() {
    let scenes: [(&str, Scene); 2] = [("docked search view", docked), ("anchored host", hosted)];
    for (name, scene) in scenes {
        for theme in both_motions() {
            let anchor = OverlayAnchor::new();
            let mut audit = Audit::new(
                // Wider than the docked view's M3 minimum (360), so the bar's
                // width is the panel's.
                Docked {
                    bar_width: 370.0,
                    drop: 0.0,
                },
                theme,
                move |state: &mut Docked| scene(&anchor, state),
            );
            audit.frame();
            audit.settle();
            for (bar_width, drop) in [(395.0, 0.0), (395.0, 60.0), (380.0, 20.0)] {
                audit.state.bar_width = bar_width;
                audit.state.drop = drop;
                audit.frame();
                audit.settle();
            }
            audit.assert_clean(name);
        }
    }
}

// ---------------------------------------------------------------------------
// The expandable list's animated height (`docs/REVIEW_FOCUS.md`).
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Sections {
    expanded: Vec<usize>,
}

fn sections(state: &mut Sections) -> AnyView<Sections> {
    any(column()
        .child(
            expandable_list([
                expandable_item(
                    SizedBox(Some(240.0), Some(48.0)),
                    SizedBox(Some(240.0), Some(120.0)),
                ),
                expandable_item(
                    SizedBox(Some(240.0), Some(48.0)),
                    SizedBox(Some(240.0), Some(60.0)),
                ),
            ])
            .expanded(state.expanded.clone()),
        )
        .child(SizedBox(Some(100.0), Some(20.0))))
}

#[test]
fn an_expandable_list_reveal_stays_laid_out() {
    for theme in both_motions() {
        let mut audit = Audit::new(Sections::default(), theme, sections);
        audit.frame();
        audit.settle();
        for expanded in [vec![0], vec![0, 1], vec![1], vec![]] {
            audit.state.expanded = expanded;
            audit.frame();
            audit.settle();
        }
        audit.assert_clean("expandable list");
    }
}
