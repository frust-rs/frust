//! `Base`-design asset cases — one per `apps/website/widgets/base/assets/*.mdx`
//! page: `icon`, `icon-sets`, `image`.
//!
//! `icon-sets` is not itself a widget page
//! (`apps/website/widgets/base/assets/icon-sets.mdx`: "Not a widget. `frust::icons`
//! is a generated table of constants you pass to `icon`") — its case renders a
//! small grid through the [`icon`] widget instead, to show the vendored
//! vocabulary's variety rather than duplicate the plain `icon` case.
//!
//! Every case takes its frame from [`super::framed`], which sizes without
//! filling: the icon glyphs and the image sit on the recorder's per-variant
//! `surface` clear, and captions painted onto that surface keep their themed
//! `on_surface` default (see [`super`]'s "The variant has to reach the
//! pixels").

use frust_core::{AnyView, View, any};
use frust_widgets::icons;
use frust_widgets::{
    CrossAxisAlignment, IconSource, Image, ImageFit, ImageSource, SizedBox, column, container,
    icon, row, text,
};
use peniko::Color;

use super::framed;
use crate::case::{Case, Design};

const ACCENT: Color = Color::from_rgb8(0x3B, 0x82, 0xF6);

/// The `icon-sets` chip fill: a light slate that still reads as a card on the
/// light surface (`#FAFAFA`) and obviously so on the dark one (`#121212`),
/// and dark enough for [`CHIP_INK`] to sit on it either way.
const CHIP: Color = Color::from_rgb8(0xE2, 0xE8, 0xF0);
/// The chip label's ink — explicit, because it is painted onto [`CHIP`]
/// rather than onto the variant's own surface.
const CHIP_INK: Color = Color::from_rgb8(0x33, 0x41, 0x55);

fn icon_case() -> AnyView<()> {
    any(framed(
        column()
            .child(icon(icons::HOME).size(128.0).color(ACCENT).label("Home"))
            .child(text("icon(icons::HOME).size(128.0)").size(14.0))
            .cross_axis(CrossAxisAlignment::Center),
    ))
}

/// One labelled swatch in the [`icon_sets_case`] grid.
fn icon_chip(source: IconSource, label: &'static str) -> impl View<()> {
    container(
        column()
            .child(icon(source).size(28.0).color(ACCENT).label(label))
            .child(text(label).size(11.0).color(CHIP_INK))
            .cross_axis(CrossAxisAlignment::Center),
    )
    .fill(CHIP)
    .radius(10.0)
    .size_centered(104.0, 84.0)
}

fn icon_sets_case() -> AnyView<()> {
    any(framed(
        column()
            .child(
                row()
                    .child(icon_chip(icons::HOME, "home"))
                    .child(icon_chip(icons::SEARCH, "search"))
                    .child(icon_chip(icons::SETTINGS, "settings")),
            )
            .child(
                row()
                    .child(icon_chip(icons::PERSON, "person"))
                    .child(icon_chip(icons::STAR, "star"))
                    .child(icon_chip(icons::MOOD, "mood")),
            ),
    ))
}

/// A small procedurally generated checkerboard, decoded straight from RGBA8
/// bytes via [`ImageSource::from_rgba8`] — no network fetch and no file on
/// disk, neither of which the CPU-oracle recorder can reach.
fn checkerboard_source() -> ImageSource {
    const SIDE: u32 = 64;
    const CELL: u32 = 8;
    let mut pixels = vec![0u8; (SIDE * SIDE * 4) as usize];
    for y in 0..SIDE {
        for x in 0..SIDE {
            let idx = ((y * SIDE + x) * 4) as usize;
            let (r, g, b) = if ((x / CELL) + (y / CELL)).is_multiple_of(2) {
                (0x3B, 0x82, 0xF6)
            } else {
                (0xFB, 0xBF, 0x24)
            };
            pixels[idx] = r;
            pixels[idx + 1] = g;
            pixels[idx + 2] = b;
            pixels[idx + 3] = 0xFF;
        }
    }
    ImageSource::from_rgba8(pixels, SIDE, SIDE)
}

fn image_case() -> AnyView<()> {
    any(framed(
        SizedBox(Some(300.0), Some(180.0)).child(Image(checkerboard_source()).fit(ImageFit::Cover)),
    ))
}

/// This module's slice of the registry [`crate::cases`] concatenates.
pub const CASES: &[Case] = &[
    Case {
        slug: "icon",
        title: "icon",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Base,
        build: icon_case,
    },
    Case {
        slug: "icon-sets",
        title: "Icon sets",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Base,
        build: icon_sets_case,
    },
    Case {
        slug: "image",
        title: "Image",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Base,
        build: image_case,
    },
];
