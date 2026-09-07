//! `Base`-design asset cases — one per `apps/website/widgets/base/assets/*.mdx`
//! page: `icon`, `icon-sets`, `image`.
//!
//! `icon-sets` is not itself a widget page
//! (`apps/website/widgets/base/assets/icon-sets.mdx`: "Not a widget. `frust::icons`
//! is a generated table of constants you pass to `icon`") — its case renders a
//! small grid through the [`icon`] widget instead, to show the vendored
//! vocabulary's variety rather than duplicate the plain `icon` case.

use frust_core::{AnyView, any};
use frust_widgets::icons;
use frust_widgets::{
    Column, CrossAxisAlignment, IconSource, Image, ImageFit, ImageSource, Row, SizedBox, container,
    icon, text,
};
use peniko::Color;

use crate::case::{Case, Design};

const ACCENT: Color = Color::from_rgb8(0x3B, 0x82, 0xF6);

fn icon_case() -> AnyView<()> {
    any(container(
        Column(vec![
            any(icon(icons::HOME).size(128.0).color(ACCENT).label("Home")),
            any(text("icon(icons::HOME).size(128.0)")
                .size(14.0)
                .color(Color::from_rgb8(0x33, 0x41, 0x55))),
        ])
        .cross_axis(CrossAxisAlignment::Center),
    )
    .fill(Color::from_rgb8(0xF1, 0xF5, 0xF9))
    .size_centered(360.0, 240.0))
}

/// One labelled swatch in the [`icon_sets_case`] grid.
fn icon_chip(source: IconSource, label: &'static str) -> AnyView<()> {
    any(container(
        Column(vec![
            any(icon(source).size(28.0).color(ACCENT).label(label)),
            any(text(label)
                .size(11.0)
                .color(Color::from_rgb8(0x33, 0x41, 0x55))),
        ])
        .cross_axis(CrossAxisAlignment::Center),
    )
    .fill(Color::WHITE)
    .radius(10.0)
    .size_centered(104.0, 84.0))
}

fn icon_sets_case() -> AnyView<()> {
    any(container(Column(vec![
        any(Row(vec![
            icon_chip(icons::HOME, "home"),
            icon_chip(icons::SEARCH, "search"),
            icon_chip(icons::SETTINGS, "settings"),
        ])),
        any(Row(vec![
            icon_chip(icons::PERSON, "person"),
            icon_chip(icons::STAR, "star"),
            icon_chip(icons::MOOD, "mood"),
        ])),
    ]))
    .fill(Color::from_rgb8(0xE2, 0xE8, 0xF0))
    .size_centered(360.0, 240.0))
}

/// A small procedurally generated checkerboard, decoded straight from RGBA8
/// bytes via [`ImageSource::from_rgba8`] — no network fetch, no file on disk
/// (the CPU-oracle recorder never touches either), matching the option this
/// task's environment notes call out explicitly.
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
    any(container(
        SizedBox(Some(320.0), Some(200.0)).child(Image(checkerboard_source()).fit(ImageFit::Cover)),
    )
    .fill(Color::from_rgb8(0x0F, 0x17, 0x2A))
    .size_centered(360.0, 240.0))
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
