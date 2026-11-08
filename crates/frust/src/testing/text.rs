//! Text-run diagnostics for [`Tester`]: every text run in the tree with its geometry, and the
//! ones that were cut, ellipsized or are wider than their box.
//!
//! Runs come from the retained tree, so a label merged into a control (a button's, a checkbox's)
//! is listed although it has no semantics node. A widget that reports
//! [`InspectDetail::Clip`] (a scroll or list view, a page view, a viewport, a pan/zoom surface, a
//! navigator) clips its descendants: the part of a run outside it is hidden, never an overflow.
//! A `PushClip` by any other widget is not consulted, and a text input's contents are not covered.

use std::collections::HashMap;
use std::fmt::Write as _;

use frust_core::{InspectDetail, InspectNode, View, WidgetId};
use kurbo::{Rect, Size};

use super::Tester;

/// Slack on each edge when testing containment, since text bounds are fractional.
const TOLERANCE: f64 = 0.5;

/// One text run of the retained tree, from [`Tester::text_runs`].
///
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct TextRun {
    /// The text widget's id.
    pub widget: WidgetId,
    /// The text laid out (a rich text's spans concatenated).
    pub text: String,
    /// The run's border box in absolute window coordinates.
    pub bounds: Rect,
    /// The parent's bounds, or the window rectangle for a root.
    pub parent_bounds: Rect,
    /// Whether the visible part of the run leaves any non-clipping ancestor's bounds (beyond a
    /// 0.5 px tolerance). Parts hidden by a clipping ancestor are not counted.
    pub outside_parent: bool,
    /// Whether part of the run is hidden by a clipping ancestor.
    pub clipped: bool,
    /// Whether all of the run is hidden by a clipping ancestor (off-screen in a viewport).
    pub hidden: bool,
    /// The size the text actually occupied.
    pub laid_out: Size,
    /// The width constraint the layout ran under, if bounded.
    pub max_width: Option<f64>,
    /// The number of lines laid out.
    pub lines: usize,
    /// The line limit the widget was configured with, if any.
    pub max_lines: Option<usize>,
    /// Whether the text was cut by `max_lines`: it needed more lines than allowed, or the last
    /// allowed line is wider than `max_width`.
    pub did_exceed_max_lines: bool,
    /// Whether the last visible line was ellipsized.
    pub ellipsized: bool,
    /// Whether the laid-out content is wider than `max_width`.
    pub overflows_width: bool,
}

impl TextRun {
    /// Whether the run is cut by `max_lines` without an ellipsis. An ellipsized run does not
    /// count: the text was fitted on purpose.
    fn cut(&self) -> bool {
        self.did_exceed_max_lines && !self.ellipsized
    }

    /// Whether the run is cut without an ellipsis, wider than its box or, in its visible part,
    /// outside an ancestor. A fully hidden run is never an overflow: it is off-screen.
    fn overflows(&self) -> bool {
        !self.hidden && (self.cut() || self.overflows_width || self.outside_parent)
    }

    fn flags(&self) -> String {
        let mut flags = Vec::new();
        if self.cut() {
            flags.push("cut by max_lines");
        }
        if self.overflows_width {
            flags.push("wider than its box");
        }
        if self.outside_parent {
            flags.push("outside parent");
        }
        flags.join(", ")
    }
}

fn contains(outer: Rect, inner: Rect) -> bool {
    inner.x0 >= outer.x0 - TOLERANCE
        && inner.y0 >= outer.y0 - TOLERANCE
        && inner.x1 <= outer.x1 + TOLERANCE
        && inner.y1 <= outer.y1 + TOLERANCE
}

/// The part of `rect` inside `clip`, or `None` when nothing of a non-empty `rect` remains.
fn clip_to(rect: Rect, clip: Rect) -> Option<Rect> {
    let inside = Rect::new(
        rect.x0.max(clip.x0),
        rect.y0.max(clip.y0),
        rect.x1.min(clip.x1),
        rect.y1.min(clip.y1),
    );
    let empty = inside.x1 < inside.x0 || inside.y1 < inside.y0;
    let vanished = inside.area() <= 0.0 && rect.area() > 0.0;
    (!empty && !vanished).then_some(inside)
}

fn runs_of(nodes: &[InspectNode], window: Size) -> Vec<TextRun> {
    let by_id: HashMap<WidgetId, &InspectNode> = nodes.iter().map(|n| (n.id, n)).collect();
    let window = Rect::from_origin_size((0.0, 0.0), window);
    let mut runs = Vec::new();
    for node in nodes {
        let Some(InspectDetail::Text(t)) = &node.detail else {
            continue;
        };
        let parent_bounds = node
            .parent
            .and_then(|id| by_id.get(&id))
            .map_or(window, |parent| parent.bounds);
        let mut visible = Some(node.bounds);
        let mut clipped = false;
        let mut outside_parent = false;
        let mut at = node.parent;
        while let Some(ancestor) = at.and_then(|id| by_id.get(&id)) {
            if let Some(rect) = visible {
                if matches!(ancestor.detail, Some(InspectDetail::Clip)) {
                    visible = clip_to(rect, ancestor.bounds);
                    clipped |= visible != Some(rect);
                } else if !contains(ancestor.bounds, rect) {
                    outside_parent = true;
                }
            }
            at = ancestor.parent;
        }
        let hidden = visible.is_none();
        runs.push(TextRun {
            widget: node.id,
            text: t.text.clone(),
            bounds: node.bounds,
            parent_bounds,
            outside_parent,
            clipped,
            hidden,
            laid_out: t.laid_out,
            max_width: t.max_width,
            lines: t.lines,
            max_lines: t.max_lines,
            did_exceed_max_lines: t.did_exceed_max_lines,
            ellipsized: t.ellipsized,
            overflows_width: t.overflows_width,
        });
    }
    runs
}

impl<S: 'static, V: View<S>> Tester<S, V> {
    /// Every text run in the retained tree, in tree order, with its text and geometry.
    pub fn text_runs(&self) -> Vec<TextRun> {
        runs_of(&self.inspect(), self.size())
    }

    /// The runs that are cut without an ellipsis (`did_exceed_max_lines` and not `ellipsized`),
    /// wider than their box (`overflows_width`) or outside an ancestor's bounds
    /// (`outside_parent`).
    ///
    /// An ellipsized run is not an overflow by itself: the text was fitted on purpose. A run
    /// fully hidden by a clipping ancestor (`hidden`) is never one: it is off-screen. A run only
    /// partly hidden (`clipped`) is judged on the rules above.
    pub fn text_overflows(&self) -> Vec<TextRun> {
        let mut runs = self.text_runs();
        runs.retain(TextRun::overflows);
        runs
    }

    /// Panic if [`text_overflows`](Self::text_overflows) is not empty, listing each offender as
    /// `"<text>" (<w>x<h> in <parent w>x<parent h>): <flags>`.
    #[track_caller]
    pub fn assert_no_text_overflow(&self) {
        let offenders = self.text_overflows();
        if offenders.is_empty() {
            return;
        }
        let mut message = format!("{} text run(s) overflow:", offenders.len());
        for run in &offenders {
            let _ = write!(
                message,
                "\n\"{}\" ({}x{} in {}x{}): {}",
                run.text,
                run.laid_out.width,
                run.laid_out.height,
                run.parent_bounds.width(),
                run.parent_bounds.height(),
                run.flags()
            );
        }
        panic!("{message}");
    }
}

#[cfg(test)]
mod tests {
    use kurbo::Size;

    use crate::authoring::text::TextOverflow;
    use crate::testing::Tester;
    use crate::{
        button, checkbox, column, container, list_view, page_view, radio, rich_text, scroll_view,
        span, text,
    };

    const LONG: &str = "Supercalifragilisticexpialidocious";

    fn tester<V: crate::View<()>>(
        width: f64,
        build: impl FnMut(&mut ()) -> V + 'static,
    ) -> Tester<(), V> {
        Tester::new((), build, Size::new(width, 200.0))
    }

    #[test]
    fn fitting_texts_report_no_overflow() {
        let tester = tester(300.0, |_: &mut ()| {
            column()
                .child(text("one"))
                .child(text("two"))
                .child(text("three"))
        });
        assert_eq!(tester.text_runs().len(), 3);
        assert!(tester.text_overflows().is_empty());
        tester.assert_no_text_overflow();
    }

    #[test]
    fn overlong_button_label_overflows_its_button() {
        let tester = tester(120.0, |_: &mut ()| button::<(), _>(LONG, |_| {}));
        let runs = tester.text_runs();
        let run = runs.iter().find(|r| r.text == LONG).expect("label listed");
        assert!(run.overflows_width, "{run:?}");
        let button = tester
            .inspect()
            .into_iter()
            .find(|n| n.type_name.contains("Button"))
            .expect("button pod");
        assert_eq!(run.parent_bounds, button.bounds);
        assert_eq!(tester.text_overflows().len(), 1);
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            tester.assert_no_text_overflow();
        }))
        .expect_err("overflow must panic");
        let message = caught.downcast_ref::<String>().cloned().unwrap_or_default();
        assert!(message.contains(LONG), "{message}");
    }

    #[test]
    fn ellipsis_is_reported_but_is_not_an_overflow() {
        let words = "several short words that wrap onto many lines in a narrow box";
        let tester = tester(80.0, move |_: &mut ()| {
            text(words).max_lines(1).overflow(TextOverflow::Ellipsis)
        });
        let runs = tester.text_runs();
        assert_eq!(runs.len(), 1);
        let run = &runs[0];
        assert!(run.did_exceed_max_lines && run.ellipsized, "{run:?}");
        assert_eq!((run.lines, run.max_lines), (1, Some(1)));
        assert!(!run.overflows_width, "{run:?}");
        assert!(
            tester.text_overflows().is_empty(),
            "{:?}",
            tester.text_overflows()
        );
    }

    #[test]
    fn text_pushed_past_a_fixed_container_is_outside_its_parent() {
        let tester = tester(300.0, |_: &mut ()| {
            container(
                column()
                    .child(text("first"))
                    .child(text("second"))
                    .child(text("third")),
            )
            .size_centered(100.0, 30.0)
        });
        let outside: Vec<_> = tester
            .text_runs()
            .into_iter()
            .filter(|r| r.outside_parent)
            .map(|r| r.text)
            .collect();
        assert_eq!(outside, ["second", "third"]);
        assert_eq!(tester.text_overflows().len(), 2);
    }

    #[test]
    fn checkbox_and_radio_labels_report_the_control_bounds() {
        let tester = tester(300.0, |_: &mut ()| {
            column()
                .child(checkbox::<(), _>(true, "Check me", |_, _| {}))
                .child(radio::<()>(false, "Pick me"))
        });
        for (label, kind) in [("Check me", "Checkbox"), ("Pick me", "Radio")] {
            let run = tester
                .text_runs()
                .into_iter()
                .find(|r| r.text == label)
                .expect(label);
            let nodes = tester.inspect();
            let control = nodes
                .iter()
                .find(|n| n.type_name.contains(kind))
                .expect("control pod");
            assert_eq!(control.bounds, run.parent_bounds, "{label}");
        }
    }

    #[test]
    fn rich_text_is_one_run_with_concatenated_text() {
        let tester = tester(300.0, |_: &mut ()| {
            rich_text([span("Hello, "), span("world")])
        });
        let runs = tester.text_runs();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text, "Hello, world");
    }

    fn rows(n: usize) -> impl crate::View<()> {
        let mut col = column();
        for i in 0..n {
            col = col.child(text(format!("row {i}")));
        }
        col
    }

    #[test]
    fn scrolled_rows_are_clipped_not_overflowing() {
        let tester = tester(300.0, |_: &mut ()| scroll_view(rows(30)));
        let runs = tester.text_runs();
        assert_eq!(runs.len(), 30);
        assert!(tester.text_overflows().is_empty());
        tester.assert_no_text_overflow();
        let edge = runs.iter().find(|r| r.clipped && !r.hidden);
        assert!(edge.is_some(), "a row crosses the viewport edge");
        assert!(runs.last().expect("rows").hidden);
        assert!(!runs[0].clipped && !runs[0].hidden);
    }

    #[test]
    fn list_view_rows_past_the_edge_do_not_overflow() {
        let mut tester = tester(300.0, |_: &mut ()| {
            list_view(100, 70.0, |i| text(format!("item {i}")))
        });
        tester.pump_and_settle();
        let runs = tester.text_runs();
        assert!(runs.iter().any(|r| r.hidden), "{runs:?}");
        assert!(runs.iter().any(|r| r.clipped), "{runs:?}");
        assert!(tester.text_overflows().is_empty());
    }

    #[test]
    fn neighbour_pages_are_hidden() {
        let tester = tester(300.0, |_: &mut ()| {
            page_view((text("one"), text("two"), text("three")))
        });
        let runs = tester.text_runs();
        assert_eq!(runs.len(), 3);
        assert!(tester.text_overflows().is_empty());
        assert!(!runs[0].hidden, "{:?}", runs[0]);
        assert!(runs[1].hidden && runs[2].hidden, "{runs:?}");
    }

    #[test]
    fn clip_does_not_mask_overflow_inside_visible_content() {
        let tester = tester(120.0, |_: &mut ()| scroll_view(column().child(text(LONG))));
        let overflows = tester.text_overflows();
        assert_eq!(overflows.len(), 1, "{overflows:?}");
        assert!(overflows[0].overflows_width);
    }
}
