//! Base-design cases for the website's `basics` catalog category
//! (`apps/website/widgets/base/basics/`): `any` / `AnyView`, `container` /
//! `colored_box`, `safe_area`, `scaffold`.
//!
//! Every case here forces its own full [`Case::DEFAULT_SIZE`] footprint —
//! `RenderRoot::layout_with_text` hands the root LOOSE constraints (zero up
//! to the window size), so an unsized case would shrink-wrap to its natural
//! extent and sit at the frame's top-left instead of filling the preview the
//! website renders — via `container(..).size_centered(w, h)`
//! ([`frust_widgets::ContainerView::size_centered`]), which forces an exact
//! size regardless of the incoming constraint and centers the child inside
//! it.

use frust_core::{AnyView, any};
use frust_widgets::{
    Column, CrossAxisAlignment, EdgeInsets, colored_box, container, safe_area, scaffold, text,
};
use peniko::Color;

use crate::case::{Case, Design};

const PAGE_BG: Color = Color::from_rgb8(0x11, 0x18, 0x27);
const CARD_A: Color = Color::from_rgb8(0x3B, 0x82, 0xF6);
const CARD_B: Color = Color::from_rgb8(0x10, 0xB9, 0x81);

/// `any` / `AnyView`: a heterogeneous child list — a plain text run and two
/// differently-decorated `container`s — held in one `Vec<AnyView<()>>`, the
/// exact erasure a `Column`'s children require (the page's "Why every
/// container needs it" section).
fn any_view_case() -> AnyView<()> {
    let children: Vec<AnyView<()>> = vec![
        any(text("AnyView erases").size(18.0).color(Color::WHITE)),
        any(container(text("child A").color(Color::WHITE))
            .fill(CARD_A)
            .radius(6.0)),
        any(container(text("child B").color(Color::WHITE))
            .fill(CARD_B)
            .radius(6.0)),
    ];
    any(
        container(Column(children).cross_axis(CrossAxisAlignment::Center))
            .fill(PAGE_BG)
            .size_centered(360.0, 240.0),
    )
}

pub(super) const ANY_VIEW: Case = Case {
    slug: "any-view",
    title: "any / AnyView",
    size: Case::DEFAULT_SIZE,
    scale: Case::DEFAULT_SCALE,
    time_ms: Case::DEFAULT_TIME_MS,
    design: Design::Base,
    build: any_view_case,
};

/// `container` / `colored_box`: a filled, rounded, bordered box around a
/// text child — the page's own swatch idiom (`examples/beui-demo`'s
/// `gpu_effects.rs`).
fn container_case() -> AnyView<()> {
    any(
        container(text("Hello, Frust").color(Color::WHITE).size(20.0))
            .fill(CARD_A)
            .radius(12.0)
            .border(Color::WHITE, 2.0)
            .size_centered(360.0, 240.0),
    )
}

pub(super) const CONTAINER: Case = Case {
    slug: "container",
    title: "container / colored_box",
    size: Case::DEFAULT_SIZE,
    scale: Case::DEFAULT_SCALE,
    time_ms: Case::DEFAULT_TIME_MS,
    design: Design::Base,
    build: container_case,
};

/// `safe_area`: pads a child by a floored per-edge minimum. This pure-`View`
/// recording attaches no host window, so the *dynamic* window inset
/// (`LayoutCtx::window_insets()`) resolves to zero — `.minimum(...)` is the
/// one knob that still shows here, so the case sets it explicitly, floating
/// the inner panel inward from the page background on every edge.
fn safe_area_case() -> AnyView<()> {
    any(container(
        safe_area(colored_box().fill(CARD_B).radius(8.0).expand()).minimum(EdgeInsets::all(24.0)),
    )
    .fill(PAGE_BG)
    .size_centered(360.0, 240.0))
}

pub(super) const SAFE_AREA: Case = Case {
    slug: "safe-area",
    title: "safe_area",
    size: Case::DEFAULT_SIZE,
    scale: Case::DEFAULT_SCALE,
    time_ms: Case::DEFAULT_TIME_MS,
    design: Design::Base,
    build: safe_area_case,
};

/// `scaffold`: the four fixed chrome slots — an app bar and a bottom bar
/// around the body — over a shared background. The frame's own bounded (if
/// loose) root constraints already give the scaffold its full width and
/// height (`ScaffoldWidget::layout`'s `bounded_height` arm), so no outer
/// `size_centered` wrapper is needed here.
fn scaffold_case() -> AnyView<()> {
    let app_bar = any(container(text("App Bar").color(Color::WHITE).size(16.0))
        .fill(CARD_A)
        .size_centered(360.0, 48.0));
    let bottom_bar = any(container(text("Bottom Bar").color(Color::WHITE).size(14.0))
        .fill(CARD_B)
        .size_centered(360.0, 40.0));
    let body = container(text("Body").color(Color::WHITE).size(18.0)).fill(PAGE_BG);
    any(scaffold(body)
        .app_bar(app_bar)
        .bottom_bar(bottom_bar)
        .background(PAGE_BG))
}

pub(super) const SCAFFOLD: Case = Case {
    slug: "scaffold",
    title: "scaffold",
    size: Case::DEFAULT_SIZE,
    scale: Case::DEFAULT_SCALE,
    time_ms: Case::DEFAULT_TIME_MS,
    design: Design::Base,
    build: scaffold_case,
};
