//! A scaled theme pushed the way a shell pushes it reaches catalog text.
//!
//! The shell keeps the authored type scale and the OS factor, builds
//! `theme.with_text_scale(&authored, factor)` and hands it to the root with
//! `RenderRoot::set_theme`. These tests drive that path against a mounted
//! material component and read the laid-out text size back.

use std::any::Any;

use frust::authoring::text::TextContext;
use frust::authoring::{AnyView, any};
use frust::{Theme, column};
use frust_core::RenderRoot;
use frust_material::list_item;
use kurbo::Size;

const WINDOW: Size = Size::new(400.0, 600.0);

/// Heights of every laid-out text run in the tree, in tree order.
fn text_heights(root: &RenderRoot<(), AnyView<()>>) -> Vec<f64> {
    root.inspect()
        .into_iter()
        .filter(|n| n.type_name.ends_with("TextWidget"))
        .map(|n| n.bounds.height())
        .collect()
}

fn mount(theme: Theme) -> (RenderRoot<(), AnyView<()>>, TextContext) {
    let mut root: RenderRoot<(), AnyView<()>> = RenderRoot::new();
    root.set_theme(Box::new(theme));
    let mut view = |_: &mut ()| any(column().child(list_item("Headline")));
    root.rebuild(&mut view, &mut ());
    let mut text = TextContext::new();
    root.layout_with_text(WINDOW, &mut text as &mut dyn Any);
    (root, text)
}

#[test]
fn a_default_sized_list_item_headline_follows_a_scaled_theme_push() {
    let authored = frust_material::baseline();
    let (mut root, mut text) = mount(authored.clone());
    let base = text_heights(&root);
    assert_eq!(base.len(), 1, "one headline run");

    // The shell's delivery: a rescaled copy of the authored theme.
    root.set_theme(Box::new(
        authored.clone().with_text_scale(&authored.type_scale, 1.5),
    ));
    root.layout_with_text(WINDOW, &mut text as &mut dyn Any);
    let scaled = text_heights(&root);
    assert!(
        scaled[0] > base[0] * 1.3,
        "headline grew with the pushed scale: {} -> {}",
        base[0],
        scaled[0]
    );

    // Pushing the factor back restores the authored size exactly.
    root.set_theme(Box::new(
        authored.clone().with_text_scale(&authored.type_scale, 1.0),
    ));
    root.layout_with_text(WINDOW, &mut text as &mut dyn Any);
    assert_eq!(text_heights(&root), base);
}
