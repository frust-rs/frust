//! A scaled theme pushed the way a shell pushes it, against shadcn text.
//!
//! The shell keeps the authored type scale and the OS factor, builds
//! `theme.with_text_scale(&authored, factor)` and hands it to the root with
//! `RenderRoot::set_theme`. Default-sized baseline text under the shadcn theme
//! follows that push; the catalog's own `label` shapes a token-sized run and
//! does not, which the second test records.

use std::any::Any;

use frust::authoring::text::TextContext;
use frust::authoring::{AnyView, any};
use frust::{Theme, column, text};
use frust_core::RenderRoot;
use frust_shadcn::label;
use kurbo::Size;

const WINDOW: Size = Size::new(400.0, 600.0);

fn mount(
    theme: Theme,
    view: impl FnMut(&mut ()) -> AnyView<()> + 'static,
) -> (RenderRoot<(), AnyView<()>>, TextContext) {
    let mut root: RenderRoot<(), AnyView<()>> = RenderRoot::new();
    root.set_theme(Box::new(theme));
    let mut view = view;
    root.rebuild(&mut view, &mut ());
    let mut text = TextContext::new();
    root.layout_with_text(WINDOW, &mut text as &mut dyn Any);
    (root, text)
}

/// Height of the first node whose type name ends with `suffix`.
fn height_of(root: &RenderRoot<(), AnyView<()>>, suffix: &str) -> f64 {
    root.inspect()
        .into_iter()
        .find(|n| n.type_name.ends_with(suffix))
        .unwrap_or_else(|| panic!("no {suffix} in the tree"))
        .bounds
        .height()
}

fn push_scale(
    root: &mut RenderRoot<(), AnyView<()>>,
    text: &mut TextContext,
    authored: &Theme,
    factor: f32,
) {
    root.set_theme(Box::new(
        authored
            .clone()
            .with_text_scale(&authored.type_scale, factor),
    ));
    root.layout_with_text(WINDOW, text as &mut dyn Any);
}

#[test]
fn default_sized_text_under_the_shadcn_theme_follows_a_scaled_push() {
    let authored = frust_shadcn::theme();
    let (mut root, mut tcx) = mount(authored.clone(), |_| any(column().child(text("Account"))));
    let base = height_of(&root, "TextWidget");
    push_scale(&mut root, &mut tcx, &authored, 1.5);
    let scaled = height_of(&root, "TextWidget");
    assert!(scaled > base * 1.3, "text grew: {base} -> {scaled}");
}

#[test]
fn the_label_run_keeps_its_token_size_under_a_scaled_push() {
    // `label` shapes a run at the fixed `TEXT_SM` token rather than a type
    // scale slot, so it does not follow the push. This pins that so a catalog
    // change to follow the scale updates the test on purpose.
    let authored = frust_shadcn::theme();
    let (mut root, mut tcx) = mount(authored.clone(), |_| any(column().child(label("Email"))));
    let base = height_of(&root, "LabelWidget");
    push_scale(&mut root, &mut tcx, &authored, 1.5);
    assert_eq!(height_of(&root, "LabelWidget"), base);
}
