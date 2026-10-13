//! Sliver forwarding through the transparent wrappers.
//!
//! A scrolling viewport lays a child out as a sliver only when
//! [`Widget::as_sliver`] answers `Some`. The `Either` enums hold their arms
//! directly, so each arm's answer has to pass through unchanged: a sliver placed
//! in an arm that answered `None` would be laid out as a box instead.

use frust_core::{
    BoxConstraints, LayoutCtx, PaintCtx, PaintScene, SliverConstraints, SliverGeometry,
    SliverWidget, Widget,
};
use frust_widgets::{
    EitherOf3Widget, EitherOf4Widget, EitherOf5Widget, EitherOf6Widget, EitherOf7Widget,
    EitherOf8Widget, EitherWidget,
};
use kurbo::Size;

/// A widget that speaks the sliver protocol.
struct Sliver;

impl Widget for Sliver {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.max()
    }

    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}

    fn as_sliver(&mut self) -> Option<&mut dyn SliverWidget> {
        Some(self)
    }
}

impl SliverWidget for Sliver {
    fn layout_sliver(&mut self, _ctx: &mut LayoutCtx, _sc: &SliverConstraints) -> SliverGeometry {
        SliverGeometry::ZERO
    }
}

/// A plain box widget: it keeps the default `as_sliver` answer of `None`.
struct Boxed;

impl Widget for Boxed {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.max()
    }

    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
}

#[test]
fn either_forwards_a_sliver_in_either_arm() {
    let mut left: EitherWidget<Sliver, Sliver> = EitherWidget::Left(Sliver);
    let mut right: EitherWidget<Sliver, Sliver> = EitherWidget::Right(Sliver);
    assert!(left.as_sliver().is_some(), "left arm sliver must forward");
    assert!(right.as_sliver().is_some(), "right arm sliver must forward");
}

#[test]
fn either_keeps_a_box_arm_a_box() {
    let mut left: EitherWidget<Boxed, Boxed> = EitherWidget::Left(Boxed);
    let mut right: EitherWidget<Boxed, Boxed> = EitherWidget::Right(Boxed);
    assert!(left.as_sliver().is_none());
    assert!(right.as_sliver().is_none());
}

#[test]
fn either_of_three_forwards_every_arm() {
    let mut slivers = [
        EitherOf3Widget::A(Sliver),
        EitherOf3Widget::B(Sliver),
        EitherOf3Widget::C(Sliver),
    ];
    for arm in &mut slivers {
        assert!(arm.as_sliver().is_some());
    }
    let mut boxes = [
        EitherOf3Widget::A(Boxed),
        EitherOf3Widget::B(Boxed),
        EitherOf3Widget::C(Boxed),
    ];
    for arm in &mut boxes {
        assert!(arm.as_sliver().is_none());
    }
}

#[test]
fn either_of_four_forwards_every_arm() {
    let mut slivers = [
        EitherOf4Widget::A(Sliver),
        EitherOf4Widget::B(Sliver),
        EitherOf4Widget::C(Sliver),
        EitherOf4Widget::D(Sliver),
    ];
    for arm in &mut slivers {
        assert!(arm.as_sliver().is_some());
    }
    let mut boxes = [
        EitherOf4Widget::A(Boxed),
        EitherOf4Widget::B(Boxed),
        EitherOf4Widget::C(Boxed),
        EitherOf4Widget::D(Boxed),
    ];
    for arm in &mut boxes {
        assert!(arm.as_sliver().is_none());
    }
}

#[test]
fn either_of_five_forwards_every_arm() {
    let mut slivers = [
        EitherOf5Widget::A(Sliver),
        EitherOf5Widget::B(Sliver),
        EitherOf5Widget::C(Sliver),
        EitherOf5Widget::D(Sliver),
        EitherOf5Widget::E(Sliver),
    ];
    for arm in &mut slivers {
        assert!(arm.as_sliver().is_some());
    }
    let mut boxes = [
        EitherOf5Widget::A(Boxed),
        EitherOf5Widget::B(Boxed),
        EitherOf5Widget::C(Boxed),
        EitherOf5Widget::D(Boxed),
        EitherOf5Widget::E(Boxed),
    ];
    for arm in &mut boxes {
        assert!(arm.as_sliver().is_none());
    }
}

#[test]
fn either_of_six_forwards_every_arm() {
    let mut slivers = [
        EitherOf6Widget::A(Sliver),
        EitherOf6Widget::B(Sliver),
        EitherOf6Widget::C(Sliver),
        EitherOf6Widget::D(Sliver),
        EitherOf6Widget::E(Sliver),
        EitherOf6Widget::F(Sliver),
    ];
    for arm in &mut slivers {
        assert!(arm.as_sliver().is_some());
    }
    let mut boxes = [
        EitherOf6Widget::A(Boxed),
        EitherOf6Widget::B(Boxed),
        EitherOf6Widget::C(Boxed),
        EitherOf6Widget::D(Boxed),
        EitherOf6Widget::E(Boxed),
        EitherOf6Widget::F(Boxed),
    ];
    for arm in &mut boxes {
        assert!(arm.as_sliver().is_none());
    }
}

#[test]
fn either_of_seven_forwards_every_arm() {
    let mut slivers = [
        EitherOf7Widget::A(Sliver),
        EitherOf7Widget::B(Sliver),
        EitherOf7Widget::C(Sliver),
        EitherOf7Widget::D(Sliver),
        EitherOf7Widget::E(Sliver),
        EitherOf7Widget::F(Sliver),
        EitherOf7Widget::G(Sliver),
    ];
    for arm in &mut slivers {
        assert!(arm.as_sliver().is_some());
    }
    let mut boxes = [
        EitherOf7Widget::A(Boxed),
        EitherOf7Widget::B(Boxed),
        EitherOf7Widget::C(Boxed),
        EitherOf7Widget::D(Boxed),
        EitherOf7Widget::E(Boxed),
        EitherOf7Widget::F(Boxed),
        EitherOf7Widget::G(Boxed),
    ];
    for arm in &mut boxes {
        assert!(arm.as_sliver().is_none());
    }
}

#[test]
fn either_of_eight_forwards_every_arm() {
    let mut slivers = [
        EitherOf8Widget::A(Sliver),
        EitherOf8Widget::B(Sliver),
        EitherOf8Widget::C(Sliver),
        EitherOf8Widget::D(Sliver),
        EitherOf8Widget::E(Sliver),
        EitherOf8Widget::F(Sliver),
        EitherOf8Widget::G(Sliver),
        EitherOf8Widget::H(Sliver),
    ];
    for arm in &mut slivers {
        assert!(arm.as_sliver().is_some());
    }
    let mut boxes = [
        EitherOf8Widget::A(Boxed),
        EitherOf8Widget::B(Boxed),
        EitherOf8Widget::C(Boxed),
        EitherOf8Widget::D(Boxed),
        EitherOf8Widget::E(Boxed),
        EitherOf8Widget::F(Boxed),
        EitherOf8Widget::G(Boxed),
        EitherOf8Widget::H(Boxed),
    ];
    for arm in &mut boxes {
        assert!(arm.as_sliver().is_none());
    }
}

/// Source-scan guard: every delegating wrapper that holds its arm or child
/// directly must keep its `as_sliver` forward. A new transparent wrapper that
/// omits the forward turns a sliver into a box, silently, so the list below is
/// the record of wrappers that forward.
///
/// Forwarding wrappers:
/// - `EitherWidget` (`src/either.rs`), and the `either_of!` macro that generates
///   `EitherOf3Widget` .. `EitherOf8Widget`;
/// - `HeroWidget` (`src/nav/hero.rs`), the shared-element wrapper that answers as
///   its own sliver face through its child pod.
#[test]
fn transparent_wrappers_forward_as_sliver() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let headers = [
        (
            "src/either.rs",
            "impl<L: Widget, R: Widget> Widget for EitherWidget<L, R> {",
        ),
        ("src/nav/hero.rs", "impl Widget for HeroWidget {"),
    ];
    for (path, header) in headers {
        let text = std::fs::read_to_string(manifest.join(path))
            .unwrap_or_else(|e| panic!("reading {path}: {e}"));
        let at = text
            .find(header)
            .unwrap_or_else(|| panic!("{path}: missing `{header}`"));
        let block = block_from(&text[at..]);
        assert!(
            block.contains("fn as_sliver(&mut self)"),
            "{path}: `{header}` does not forward as_sliver"
        );
    }

    let either = std::fs::read_to_string(manifest.join("src/either.rs")).unwrap();
    assert!(
        either.contains("match self { $(Self::$V(w) => w.as_sliver(),)+ }"),
        "src/either.rs: the either_of! macro does not forward as_sliver"
    );
}

/// The text of the first `{ .. }` block in `text`, by brace depth.
fn block_from(text: &str) -> &str {
    let open = text.find('{').expect("impl block opens with a brace");
    let mut depth = 0usize;
    for (i, c) in text[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &text[..open + i + 1];
                }
            }
            _ => {}
        }
    }
    text
}
