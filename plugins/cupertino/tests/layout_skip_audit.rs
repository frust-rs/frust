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
//! Covered: the navigation bar's optional slots, the tab bar (whose item icon
//! pods are rebuilt in place on a selection change), the switch and the
//! activity indicator — each under both motion settings.

use std::any::Any;
use std::collections::HashMap;

use frust::authoring::text::TextContext;
use frust::authoring::{AnyView, DiscardScene, PaintOutcome, PaintScene, WidgetId, any};
use frust::{SizedBox, Theme, column};
use frust_core::{FrameTime, RenderRoot};
use frust_cupertino::{
    cupertino_activity_indicator, cupertino_button, cupertino_nav_bar, cupertino_switch,
    cupertino_tab_bar, tab_item,
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
    frust_cupertino::baseline()
}

fn reduced() -> Theme {
    let mut theme = frust_cupertino::baseline();
    theme.motion.reduce_motion = true;
    theme
}

fn both_motions() -> [Theme; 2] {
    [light(), reduced()]
}

#[derive(Default)]
struct Screen {
    back: bool,
    tab: usize,
    on: bool,
    busy: bool,
}

fn screen(state: &mut Screen) -> AnyView<Screen> {
    let mut bar = cupertino_nav_bar::<Screen>(if state.back {
        "Settings — Wi-Fi"
    } else {
        "Settings"
    });
    if state.back {
        bar = bar
            .leading(cupertino_button("Back", |_: &mut Screen| {}))
            .trailing(SizedBox(Some(28.0), Some(28.0)));
    }
    let items = (0..3)
        .map(|i| {
            tab_item(["Home", "Search", "Profile"][i]).icon(SizedBox(
                Some(if i == state.tab { 26.0 } else { 22.0 }),
                Some(22.0),
            ))
        })
        .collect();
    any(column()
        .child(bar)
        .child(cupertino_switch(state.on, |state: &mut Screen, on| {
            state.on = on
        }))
        .child(cupertino_activity_indicator().animating(state.busy))
        .child(cupertino_tab_bar(
            items,
            state.tab,
            |state: &mut Screen, tab| state.tab = tab,
        ))
        .child(SizedBox(Some(100.0), Some(20.0))))
}

#[test]
fn the_chrome_stays_laid_out_through_its_changes() {
    for theme in both_motions() {
        let mut audit = Audit::new(Screen::default(), theme, screen);
        audit.frame();
        audit.settle();
        for (back, tab, on, busy) in [
            (true, 1, true, true),
            (false, 2, false, false),
            (true, 0, true, false),
            (false, 1, false, true),
        ] {
            audit.state.back = back;
            audit.state.tab = tab;
            audit.state.on = on;
            audit.state.busy = busy;
            audit.frame();
            audit.settle();
        }
        audit.assert_clean("nav bar + tab bar + switch + activity indicator");
    }
}
