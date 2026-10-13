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
//! The widgets here are the ones the layout-skip audit fixed: the select list's
//! trigger-width floor (an anchor cell the trigger's paint writes) and the
//! bubble's variant-driven padding. The label/badge/button inks are baked into
//! the run `layout` shapes, so their variant/disabled flips are also asserted
//! to reach the painted glyphs under skipping.

use std::any::Any;
use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use frust::authoring::scene::GlyphRun;
use frust::authoring::text::TextContext;
use frust::authoring::{
    AnyView, DiscardScene, InputEvent, PaintOutcome, PaintScene, PointerButton, PointerEvent,
    PointerPhase, View, WidgetId, any,
};
use frust::{Theme, column};
use frust_core::{FrameTime, RenderRoot};
use frust_shadcn::overlay::OverlayAnchor;
use frust_shadcn::{
    BadgeVariant, BubbleVariant, ButtonVariant, accordion, accordion_item, badge, bubble, button,
    collapsible, label, select, select_option, select_trigger,
};
use kurbo::{Point, Size};
use peniko::{Brush, Color};

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

    /// One layout pass exactly as a shell runs it: skipping, no audit.
    fn skipping_layout(&mut self) {
        self.root.set_layout_audit(false);
        self.root
            .layout_with_text(self.window, &mut self.text as &mut dyn Any);
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

    fn click(&mut self, at: Point) {
        for phase in [PointerPhase::Down, PointerPhase::Up] {
            let event = InputEvent::Pointer(PointerEvent {
                phase,
                position: at,
                button: PointerButton::Primary,
            });
            self.root.event(&mut self.state, &event);
        }
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
    frust_shadcn::theme()
}

fn reduced() -> Theme {
    let mut theme = frust_shadcn::theme();
    theme.motion.reduce_motion = true;
    theme
}

/// Records every glyph run's solid ink.
#[derive(Default)]
struct Inks(Vec<Color>);

impl PaintScene for Inks {
    fn fill_rect(&mut self, _: Point, _: Size, _: Color) {}
    fn draw_text(&mut self, _: Point, _: &str) {}
    fn draw_glyph_run(&mut self, run: GlyphRun) {
        if let Brush::Solid(color) = run.brush {
            self.0.push(color);
        }
    }
}

// ---------------------------------------------------------------------------
// The select list's width floor follows its trigger.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Picker {
    trigger_width: f64,
}

fn picker(anchor: &OverlayAnchor, state: &Picker) -> impl View<Picker> + use<> {
    let options = || {
        vec![
            select_option("Apple"),
            select_option("Banana"),
            select_option("Cherry"),
        ]
    };
    frust::stack()
        .child(
            frust::SizedBox(Some(state.trigger_width), Some(36.0)).child(select_trigger(
                anchor,
                options(),
                Some(0),
            )),
        )
        .child(
            select(options(), Some(0), |_: &mut Picker, _| {})
                .anchor(anchor)
                .open(true),
        )
}

fn picker_audit(theme: Theme) -> Audit<Picker> {
    let anchor = OverlayAnchor::new();
    Audit::new(
        Picker {
            trigger_width: 140.0,
        },
        theme,
        move |state: &mut Picker| any(picker(&anchor, state)),
    )
}

#[test]
fn a_select_list_follows_a_trigger_that_resizes() {
    for theme in [light(), reduced()] {
        let mut audit = picker_audit(theme);
        audit.frame();
        audit.settle();
        for width in [320.0, 180.0, 360.0] {
            audit.state.trigger_width = width;
            // One rebuild for the resize, then only ticks: the trigger's paint
            // writes the anchor cell after the list's last layout, so the list's
            // own paint is what has to notice.
            audit.frame();
            audit.settle();
        }
        audit.assert_clean("select list over a resizing trigger");
    }
}

// ---------------------------------------------------------------------------
// A bubble's variant decides its padding.
// ---------------------------------------------------------------------------

#[test]
fn a_bubble_variant_flip_reaches_layout() {
    let variant = Rc::new(Cell::new(BubbleVariant::Default));
    let live = variant.clone();
    let mut audit = Audit::new((), light(), move |_: &mut ()| {
        any(column().child(bubble("Hello there").variant(live.get())))
    });
    audit.frame();
    for next in [
        BubbleVariant::Ghost,
        BubbleVariant::Secondary,
        BubbleVariant::Ghost,
        BubbleVariant::Default,
    ] {
        variant.set(next);
        audit.frame();
        audit.tick();
    }
    audit.assert_clean("bubble variant flips");
}

// ---------------------------------------------------------------------------
// Baked label inks follow a variant / disabled flip.
// ---------------------------------------------------------------------------

/// The inks a view paints on the frame it flips from `from` to `to`. Under
/// skipping, an ink baked into the run at layout time only changes if the flip
/// relays the widget out.
fn inks_after_flip(
    from: impl Fn() -> AnyView<()> + 'static,
    to: impl Fn() -> AnyView<()> + 'static,
) -> Vec<Color> {
    let flipped = Rc::new(Cell::new(false));
    let live = flipped.clone();
    // Inside a column, so the widget sits in a pod the skip can pass over.
    let mut audit = Audit::new((), light(), move |_: &mut ()| {
        any(column().child(if live.get() { to() } else { from() }))
    });
    audit.frame();
    audit.settle();
    flipped.set(true);
    // A plain skipping pass: the audit's forced pass would re-shape every run
    // and hide exactly the stale ink this asserts on.
    audit.root.rebuild(&mut audit.view, &mut audit.state);
    audit.skipping_layout();
    let mut after = Inks::default();
    audit.paint(&mut after);
    audit.assert_clean("ink flip");
    after.0
}

/// The inks a fresh mount of `view` paints once settled.
fn painted_inks(view: impl Fn() -> AnyView<()> + 'static) -> Vec<Color> {
    let mut audit = Audit::new((), light(), move |_: &mut ()| any(column().child(view())));
    audit.frame();
    audit.settle();
    audit.root.rebuild(&mut audit.view, &mut audit.state);
    audit.layout();
    let mut inks = Inks::default();
    audit.paint(&mut inks);
    inks.0
}

#[test]
fn a_badge_variant_flip_repaints_the_new_ink() {
    let after = inks_after_flip(
        || any(badge::<()>("Beta")),
        || any(badge::<()>("Beta").variant(BadgeVariant::Outline)),
    );
    let fresh = painted_inks(|| any(badge::<()>("Beta").variant(BadgeVariant::Outline)));
    assert!(!fresh.is_empty(), "the badge paints its label");
    assert_eq!(after, fresh, "the flipped badge paints the outline ink");
}

#[test]
fn a_button_variant_flip_repaints_the_new_ink() {
    let after = inks_after_flip(
        || any(button::<()>("Save", |_| {})),
        || any(button::<()>("Save", |_| {}).variant(ButtonVariant::Outline)),
    );
    let fresh = painted_inks(|| any(button::<()>("Save", |_| {}).variant(ButtonVariant::Outline)));
    assert!(!fresh.is_empty(), "the button paints its label");
    assert_eq!(after, fresh, "the flipped button paints the outline ink");
}

#[test]
fn a_label_disabled_flip_repaints_the_dimmed_ink() {
    let after = inks_after_flip(
        || any(label("Email")),
        || any(label("Email").disabled(true)),
    );
    let fresh = painted_inks(|| any(label("Email").disabled(true)));
    assert!(!fresh.is_empty(), "the label paints its text");
    assert_eq!(after, fresh, "the flipped label paints the dimmed ink");
}

// ---------------------------------------------------------------------------
// The animated-height disclosures, both motion settings.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Disclosure {
    open: bool,
}

fn disclosures(state: &mut Disclosure) -> AnyView<Disclosure> {
    let open: Vec<usize> = if state.open { vec![0] } else { vec![] };
    any(column()
        .child(accordion(
            [
                accordion_item("Shipping", frust::SizedBox(Some(200.0), Some(80.0))),
                accordion_item("Returns", frust::SizedBox(Some(200.0), Some(40.0))),
            ],
            open,
            |state: &mut Disclosure, open: Vec<usize>| state.open = !open.is_empty(),
        ))
        .child(collapsible(
            label("More"),
            frust::SizedBox(Some(200.0), Some(60.0)),
            state.open,
            |state: &mut Disclosure, open| state.open = open,
        ))
        .child(frust::SizedBox(Some(200.0), Some(20.0))))
}

#[test]
fn accordion_and_collapsible_reveals_stay_laid_out() {
    for theme in [light(), reduced()] {
        let mut audit = Audit::new(Disclosure::default(), theme, disclosures);
        audit.frame();
        audit.settle();
        for open in [true, false, true] {
            audit.state.open = open;
            audit.frame();
            audit.settle();
        }
        // And driven by a click on the accordion's first trigger, no rebuild of
        // our own in between.
        audit.click(Point::new(40.0, 20.0));
        audit.frame();
        audit.settle();
        audit.assert_clean("accordion + collapsible");
    }
}
