//! `Base`-design painting cases — one per
//! `apps/website/widgets/base/painting/*.mdx` page: `platform-view`, `shield`.
//!
//! Both widgets exist to reserve/protect space for a *native* view that this
//! CPU-oracle recorder has no host for, so both cases lean on
//! [`PlatformViewView::debug_fill`] — "paint a translucent magenta fill so the
//! slot is visible on desktop, where no native host exists to show through it"
//! (`apps/website/widgets/base/painting/platform-view.mdx`) — exactly the
//! desktop/no-host situation this recorder runs under.
//!
//! `debug_fill` is declared under `#[cfg(debug_assertions)]`
//! (`crates/frust-widgets/src/platform_view.rs`), so the method does not
//! merely no-op in a release build — it does not exist, and naming it
//! unconditionally fails to compile any `--release` build of this crate (and
//! with it the whole root workspace). [`maybe_debug_fill`] is the cfg split
//! that keeps both profiles compiling; the consequence is that a release
//! recording of these two cases shows the reserved slot as bare surface with
//! only the overlay chrome on top, while the debug recording the standard
//! verify gate and `widget-snapshots` produce shows the magenta fill.

use frust_core::{AnyView, any};
use frust_widgets::{
    Align, Alignment, EdgeInsets, Padding, PlatformViewView, Stack, button, container,
    platform_view, shield, text,
};
use peniko::Color;

use crate::case::{Case, Design};

/// The overlay chrome's card fill and ink — explicit, because the label sits
/// on the platform-view slot rather than on the variant's own surface.
const CARD: Color = Color::from_rgb8(0x11, 0x18, 0x27);

/// Apply [`PlatformViewView::debug_fill`] only in a debug build. The method
/// itself is `#[cfg(debug_assertions)]`-gated on the builder, so a plain
/// conditional reassignment would leave an "unused `mut`" lint in release
/// builds; this free function sidesteps that. Mirrors
/// `examples/playground/src/pages/platform_views.rs`.
fn maybe_debug_fill(view: PlatformViewView) -> PlatformViewView {
    #[cfg(debug_assertions)]
    {
        view.debug_fill()
    }
    #[cfg(not(debug_assertions))]
    {
        view
    }
}

fn platform_view_case() -> AnyView<()> {
    any(Stack(vec![
        any(maybe_debug_fill(
            platform_view("dev.frust.MapFactory")
                .params_json(r#"{"style":"dark"}"#)
                .size(Case::DEFAULT_SIZE.width, Case::DEFAULT_SIZE.height),
        )
        .semantics_label("Map placeholder")),
        any(Align(
            Alignment::CENTER,
            container(
                text("platform_view (debug_fill)")
                    .color(Color::WHITE)
                    .size(15.0),
            )
            .fill(CARD)
            .radius(10.0)
            .size_centered(260.0, 40.0),
        )),
    ]))
}

fn shield_case() -> AnyView<()> {
    any(Stack(vec![
        any(maybe_debug_fill(
            platform_view("dev.frust.MapFactory")
                .interactive()
                .size(Case::DEFAULT_SIZE.width, Case::DEFAULT_SIZE.height),
        )),
        any(Align(
            Alignment::TOP_LEFT,
            Padding(
                EdgeInsets::all(16.0),
                container(
                    text("shield: button stays tappable")
                        .color(Color::WHITE)
                        .size(13.0),
                )
                .fill(CARD)
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
