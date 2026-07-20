//! S6 — Text shaping stress (multilingual relayout on width animation).
//!
//! A fixed multilingual corpus (Latin, CJK, RTL Arabic/Hebrew, Devanagari) is
//! re-laid-out every frame under a width animation: animating the text column's
//! width forces a full reshape/reflow (Parley shaping + bidi + line-breaking)
//! each frame, the frust side of the head-to-head against the Flutter bench's
//! `s6_text.dart`. The corpus is byte-identical to that file's `_corpus` (the
//! fairness gate — same bytes, same width timeline both apps).
//!
//! # How the relayout is driven
//!
//! A [`WidthPulse`] widget owns a repeating [`AnimationController`] and, each
//! paint, writes the current width fraction (oscillating 0.4 → 1.0) into a
//! signal and requests another frame. The component reads that signal and wraps
//! the corpus [`Text`](frust::text) in a [`WidthBox`] whose width changes every
//! rebuild — so the view diff reports `ChangeFlags::LAYOUT`, forcing a relayout
//! (and thus a full `Text` reshape at the new wrap width) every frame on every
//! platform, including Android's otherwise layout-skipping frame path.
//!
//! Scenario markers: the driver's default `on_start`/`on_end` bracket the whole
//! window as `s6` (a continuous animation never settles, like S1), so the raw
//! per-frame stream inside is the shaping-stress series the harness slices.

use std::time::Duration;

use frust::{
    Align, Alignment, AnyView, Color, Component, Get, RwSignal, Set, Stack, any, component, text,
};
use frust_core::{
    AnimationController, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, View, Widget,
};
use kurbo::Size;

use super::{BenchState, Scenario};

/// The embedded corpus — byte-identical to the Flutter side's `_corpus`
/// (`benchmarks/flutter_bench/lib/scenarios/s6_text.dart`). Mixed scripts and
/// long paragraphs so shaping + bidi + line-breaking all run.
const CORPUS: &str = concat!(
    "The quick brown fox jumps over the lazy dog while contemplating the ",
    "nature of typography, line breaking, and the subtle art of glyph ",
    "positioning across scripts and writing systems.\n\n",
    "请注意，这段文字混合了多种语言与书写系统，用于压力测试文本整形与换行算法的性能表现，",
    "包括中文、日文以及韩文的排版。日本語のテキストもここに含まれています。",
    "한국어 텍스트도 여기에 포함됩니다。\n\n",
    "هذا نص عربي يُكتب من اليمين إلى اليسار لاختبار خوارزميات الاتجاه الثنائي ",
    "وتشكيل الحروف المتصلة في محرك عرض النصوص. ",
    "טקסט עברי נכתב אף הוא מימין לשמאל לבדיקת אלגוריתמי הכיווניות.\n\n",
    "यह देवनागरी लिपि में लिखा गया एक अनुच्छेद है जो जटिल संयुक्ताक्षरों और ",
    "मात्राओं के साथ पाठ आकार देने की प्रक्रिया का परीक्षण करता है।",
);

/// S6 — Text shaping stress.
pub struct S6;

impl Scenario for S6 {
    fn id(&self) -> &'static str {
        "s6"
    }

    fn title(&self) -> &'static str {
        "Text shaping stress (multilingual relayout)"
    }

    fn build(&self, _state: &mut BenchState) -> AnyView<BenchState> {
        any(component(S6Text))
    }
}

/// S6 — Text shaping stress component.
pub struct S6Text;

/// Retained S6 state: the width-fraction signal the [`WidthPulse`] drives and
/// the component reads.
pub struct S6State {
    frac: RwSignal<f64>,
}

impl Component for S6Text {
    type State = S6State;

    fn init(&self) -> S6State {
        S6State {
            frac: RwSignal::new(1.0),
        }
    }

    fn build(&self, state: &mut S6State) -> AnyView<S6State> {
        // Tracked read: a `WidthPulse` write wakes this rebuild each frame.
        let frac = state.frac.get();

        let column = width_box(
            frac,
            text(CORPUS)
                .size(18.0)
                .color(Color::from_rgba8(0xFF, 0xFF, 0xFF, 0xE6)),
        );

        any(Stack(vec![
            any(WidthPulse { frac: state.frac }),
            // Top-center so the corpus starts at the top and wraps downward.
            any(Align(Alignment::new(0.0, -1.0), column)),
        ]))
    }
}

// ---------------------------------------------------------------------------
// WidthPulse — drives the width fraction from a repeating controller
// ---------------------------------------------------------------------------

/// The width-oscillation period.
const PULSE_PERIOD: Duration = Duration::from_secs(3);
/// The narrowest the column gets (fraction of available width).
const MIN_FRAC: f64 = 0.4;
/// The widest the column gets (fraction of available width).
const MAX_FRAC: f64 = 1.0;

/// A zero-size paint-driven widget that advances a repeating controller each
/// frame and publishes the resulting width fraction (a 0.4 → 1.0 → 0.4 triangle
/// wave) into a signal, requesting another frame so the animation is perpetual.
pub struct WidthPulse {
    frac: RwSignal<f64>,
}

/// The retained pulse widget.
pub struct WidthPulseWidget {
    controller: AnimationController,
    frac: RwSignal<f64>,
}

impl<State: 'static> View<State> for WidthPulse {
    type Element = WidthPulseWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> WidthPulseWidget {
        let mut controller = AnimationController::new(PULSE_PERIOD);
        controller.repeat();
        WidthPulseWidget {
            controller,
            frac: self.frac,
        }
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut WidthPulseWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.frac = self.frac;
        ChangeFlags::NONE
    }
}

impl Widget for WidthPulseWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, _bc: &BoxConstraints) -> Size {
        Size::ZERO
    }

    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        self.controller.advance(ctx.frame_time());
        // `repeat` is a 0→1 sawtooth; fold it into a 0→1→0 triangle so the
        // column breathes symmetrically in and out.
        let saw = self.controller.value();
        let tri = if saw < 0.5 {
            saw * 2.0
        } else {
            2.0 - saw * 2.0
        };
        let frac = MIN_FRAC + tri * (MAX_FRAC - MIN_FRAC);
        self.frac.set(frac);
        ctx.request_frame();
    }
}

// ---------------------------------------------------------------------------
// WidthBox — constrains a child to a fraction of the available width
// ---------------------------------------------------------------------------

/// Wrap `child` in a box whose width is `frac` of the available width, forcing a
/// reshape of a text child every time `frac` changes.
pub fn width_box<State: 'static, V: View<State>>(frac: f64, child: V) -> WidthBox<State> {
    WidthBox {
        frac,
        child: any(child),
    }
}

/// A declarative width-constraining wrapper. See [`width_box`].
pub struct WidthBox<State: 'static> {
    frac: f64,
    child: AnyView<State>,
}

/// The retained width-box widget.
pub struct WidthBoxWidget {
    frac: f64,
    child: ChildPod,
}

fn build_child<State: 'static>(view: &AnyView<State>, ctx: &mut BuildCtx<'_>) -> ChildPod {
    let element: Box<dyn Widget> = view.build(ctx);
    ChildPod::new(Box::new(element))
}

fn rebuild_child<State: 'static>(
    prev: &AnyView<State>,
    next: &AnyView<State>,
    pod: &mut ChildPod,
    ctx: &mut BuildCtx<'_>,
) -> ChangeFlags {
    let element = pod
        .widget_mut()
        .downcast_mut::<Box<dyn Widget>>()
        .expect("width-box child element is a boxed AnyView widget");
    next.rebuild(prev, element, ctx)
}

fn teardown_child<State: 'static>(
    view: &AnyView<State>,
    pod: &mut ChildPod,
    ctx: &mut BuildCtx<'_>,
) {
    if let Some(element) = pod.widget_mut().downcast_mut::<Box<dyn Widget>>() {
        view.teardown(element, ctx);
    }
}

impl<State: 'static> View<State> for WidthBox<State> {
    type Element = WidthBoxWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> WidthBoxWidget {
        WidthBoxWidget {
            frac: self.frac,
            child: build_child(&self.child, ctx),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut WidthBoxWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(&prev.child, &self.child, &mut element.child, ctx);
        if prev.frac != self.frac {
            element.frac = self.frac;
            // A width change forces a relayout — this is what re-shapes the
            // corpus every frame, even under Android's layout-skip gate.
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut WidthBoxWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for WidthBoxWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let avail = bc.max();
        let w = (avail.width * self.frac).max(1.0);
        let child_bc = BoxConstraints::new(Size::ZERO, Size::new(w, avail.height));
        let child_size = self.child.layout_child(ctx, &child_bc);
        Size::new(w, child_size.height.min(avail.height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.child.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        self.child.event_child(ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.child.semantics_child(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corpus_is_multiscript_and_matches_flutter_shape() {
        // Sanity: the corpus embeds all four script families and the paragraph
        // breaks, so shaping/bidi/line-breaking all exercise.
        assert!(CORPUS.contains("quick brown fox")); // Latin
        assert!(CORPUS.contains("请注意")); // CJK
        assert!(CORPUS.contains("هذا نص عربي")); // Arabic (RTL)
        assert!(CORPUS.contains("טקסט עברי")); // Hebrew (RTL)
        assert!(CORPUS.contains("देवनागरी")); // Devanagari
        assert_eq!(CORPUS.matches("\n\n").count(), 3, "three paragraph breaks");
    }
}
