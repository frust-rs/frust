//! `Base`-design cases for the website's `widgets/base/navigation` catalog
//! page set: `hero`, `navigator`. See the crate docs and `crate::base` for
//! the pure-`View`/slug-rule contract every case in this registry follows,
//! and [`super::framed`] for why neither case fills its own backdrop.

use frust_core::{AnyView, any};
use frust_text::FontWeight;
use frust_widgets::{
    CrossAxisAlignment, NavigatorController, SizedBox, column, container, hero, navigator, text,
};
use peniko::Color;

use super::framed;
use crate::case::{Case, Design};

/// Shared accent used for the hero's shared-element avatar swatch.
const ACCENT: Color = Color::from_rgb8(0x3B, 0x82, 0xF6);

/// `hero` is zero-cost outside a page transition (see its own docs) — a
/// single build has no second, same-tagged page to morph into, so this case
/// shows the "before" state: a tagged avatar swatch a `navigator` push/pop
/// would later morph, painted exactly as it renders at rest.
fn hero_case() -> AnyView<()> {
    any(framed(
        column()
            .child(hero(
                "preview-avatar",
                container(text("AB").size(16.0).color(Color::WHITE))
                    .fill(ACCENT)
                    .radius(32.0)
                    .size_centered(64.0, 64.0),
            ))
            .child(SizedBox(None, Some(12.0)))
            .child(text("hero(\"preview-avatar\", ..) — morphs across a matching tag").size(12.0))
            .cross_axis(CrossAxisAlignment::Center),
    ))
}

/// A `navigator` is a retained page stack (see its own docs); a single pure
/// build has no app-state controller driving push/pop, so this case shows
/// the stack's static "before" state: the initial page as it renders on
/// first mount, filling the frame itself rather than being wrapped in an
/// outer `framed(..)`.
fn navigator_case() -> AnyView<()> {
    any(navigator(&NavigatorController::<()>::new(), || {
        container(
            column()
                .child(text("Home").size(18.0).weight(FontWeight::BOLD))
                .child(SizedBox(None, Some(8.0)))
                .child(text("navigator(&controller, || home_page())").size(12.0))
                .cross_axis(CrossAxisAlignment::Center),
        )
        .size_centered(Case::DEFAULT_SIZE.width, Case::DEFAULT_SIZE.height)
    }))
}

/// This module's slice of the registry [`crate::cases`] concatenates.
pub const CASES: &[Case] = &[
    Case {
        slug: "hero",
        title: "Hero",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Base,
        build: hero_case,
    },
    Case {
        slug: "navigator",
        title: "Navigator",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Base,
        build: navigator_case,
    },
];
