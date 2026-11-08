//! Text-run diagnostics for [`Tester`]: every text run in the tree with its geometry, and the
//! ones that were cut, ellipsized or are wider than their box.
//!
//! Runs come from the retained tree, so a label merged into a control (a button's, a checkbox's)
//! is listed although it has no semantics node. Engine clips (`PushClip`) are not consulted: a run
//! hidden by a scroll viewport's clip but inside its ancestors' boxes is not reported. A text
//! input's contents are not covered.

use std::fmt::Write as _;

use frust_core::{InspectDetail, InspectNode, View, WidgetId};
use kurbo::{Rect, Size};

use super::Tester;

/// Slack on each edge when testing containment, since text bounds are fractional.
const TOLERANCE: f64 = 0.5;

/// One text run of the retained tree, from [`Tester::text_runs`].
///
/// `did_exceed_max_lines` follows Flutter's `TextPainter.didExceedMaxLines`.
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
    /// Whether the run's bounds leave any ancestor's bounds (beyond a 0.5 px tolerance).
    pub outside_parent: bool,
    /// The size the text actually occupied.
    pub laid_out: Size,
    /// The width constraint the layout ran under, if bounded.
    pub max_width: Option<f64>,
    /// The number of lines laid out.
    pub lines: usize,
    /// The line limit the widget was configured with, if any.
    pub max_lines: Option<usize>,
    /// Whether the text needed more lines than `max_lines` allowed.
    pub did_exceed_max_lines: bool,
    /// Whether the last visible line was ellipsized.
    pub ellipsized: bool,
    /// Whether the laid-out content is wider than `max_width`.
    pub overflows_width: bool,
}

impl TextRun {
    /// Whether the run is cut without an ellipsis, wider than its box or outside an ancestor.
    /// An ellipsized run does not count: the text was fitted on purpose.
    fn cut(&self) -> bool {
        self.did_exceed_max_lines && !self.ellipsized
    }

    fn overflows(&self) -> bool {
        self.cut() || self.overflows_width || self.outside_parent
    }

    fn flags(&self) -> String {
        let mut flags = Vec::new();
        if self.cut() {
            flags.push("exceeds max_lines");
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

fn runs_of(nodes: &[InspectNode], window: Size) -> Vec<TextRun> {
    let find = |id: WidgetId| nodes.iter().find(|n| n.id == id);
    let window = Rect::from_origin_size((0.0, 0.0), window);
    let mut runs = Vec::new();
    for node in nodes {
        let Some(InspectDetail::Text(t)) = &node.detail else {
            continue;
        };
        let parent_bounds = node
            .parent
            .and_then(find)
            .map_or(window, |parent| parent.bounds);
        let mut outside_parent = false;
        let mut at = node.parent;
        while let Some(ancestor) = at.and_then(find) {
            outside_parent |= !contains(ancestor.bounds, node.bounds);
            at = ancestor.parent;
        }
        runs.push(TextRun {
            widget: node.id,
            text: t.text.clone(),
            bounds: node.bounds,
            parent_bounds,
            outside_parent,
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
    /// An ellipsized run is not an overflow by itself: the text was fitted on purpose.
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
    use crate::{button, checkbox, column, container, radio, rich_text, span, text};

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
        assert!(run.overflows_width || run.did_exceed_max_lines, "{run:?}");
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
}
