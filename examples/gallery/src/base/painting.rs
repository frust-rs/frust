//! `Base`-design painting cases — one per
//! `apps/website/widgets/base/painting/*.mdx` page: `platform-view`, `shield`.
//!
//! Both widgets exist to reserve/protect space for a *native* view that this
//! CPU-oracle recorder has no host for, so both cases lean on
//! [`PlatformViewView::debug_fill`] — "paint a translucent magenta fill so the
//! slot is visible on desktop, where no native host exists to show through it"
//! (`apps/website/widgets/base/painting/platform-view.mdx`) — exactly the
//! desktop/no-host situation this recorder runs under. `debug_fill` is a
//! `#[cfg(debug_assertions)]`-only builder method (see
//! `crates/frust-widgets/src/platform_view.rs`), so these cases only paint the
//! fill under a debug build; the standard verify gate (`cargo test`/`cargo
//! clippy`, both non-`--release`) always compiles it in.

use frust_core::{AnyView, any};
use frust_widgets::{
    Align, Alignment, EdgeInsets, Padding, Stack, button, container, platform_view, shield, text,
};
use peniko::Color;

use crate::case::{Case, Design};

fn platform_view_case() -> AnyView<()> {
    any(Stack(vec![
        any(platform_view("dev.frust.MapFactory")
            .params_json(r#"{"style":"dark"}"#)
            .size(360.0, 240.0)
            .debug_fill()
            .semantics_label("Map placeholder")),
        any(Align(
            Alignment::CENTER,
            container(
                text("platform_view (debug_fill)")
                    .color(Color::WHITE)
                    .size(15.0),
            )
            .fill(Color::from_rgb8(0x11, 0x18, 0x27))
            .radius(10.0)
            .size_centered(260.0, 40.0),
        )),
    ]))
}

fn shield_case() -> AnyView<()> {
    any(Stack(vec![
        any(platform_view("dev.frust.MapFactory")
            .interactive()
            .size(360.0, 240.0)
            .debug_fill()),
        any(Align(
            Alignment::TOP_LEFT,
            Padding(
                EdgeInsets::all(16.0),
                container(
                    text("shield: button stays tappable")
                        .color(Color::WHITE)
                        .size(13.0),
                )
                .fill(Color::from_rgb8(0x11, 0x18, 0x27))
                .radius(8.0)
                .size_centered(230.0, 34.0),
            ),
        )),
        any(Align(
            Alignment::BOTTOM_RIGHT,
            Padding(
                EdgeInsets::all(16.0),
                shield(button("Recenter", |_: &mut ()| {})),
            ),
        )),
    ]))
}

/// This module's slice of the registry [`crate::cases`] concatenates.
pub const CASES: &[Case] = &[
    Case {
        slug: "platform-view",
        title: "platform_view",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Base,
        build: platform_view_case,
    },
    Case {
        slug: "shield",
        title: "shield",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Base,
        build: shield_case,
    },
];
