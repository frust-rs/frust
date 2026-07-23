//! Foundations section (`c02`): live color/type/radius token specimens,
//! reproducing reference §01 Foundations off the *current* theme rather than
//! hardcoded hexes — every swatch/specimen below reads `use_context::<Theme>()`
//! at build time, so toggling the header's brightness switch repaints this
//! whole page from the live `ColorScheme`/`TypeScale`/`ShapeScale` (see the
//! page-fn contract in `pages/mod.rs`).
//!
//! # Color-swatch primitive
//!
//! There is no generic "filled rounded box" widget in the `frust` facade
//! (API friction — see this task's Completion Summary), so a swatch is a
//! [`SizedBox`] wrapping an [`Image`] over a 1×1 solid-color [`ImageSource`]
//! stretched with [`ImageFit::Fill`] — the same facade-only technique
//! `examples/huddle`'s `ui::solid_source` helper uses, reproduced locally here
//! (this task's scope is this file alone). Radius chips instead reuse
//! [`frust::glyph::skeleton`] (`.radius(..)`) as the closest existing
//! facade-level rounded-box primitive.
//!
//! # Unmapped Glyph text tokens
//!
//! The reference build's `fg-dim`/`fg-faintest` text tokens have no
//! `ColorScheme` role — `frust_theme::glyph::color`'s module docs record this
//! as a deliberate, already-documented leftover (`on_surface_variant` already
//! carries `fg-muted`). This page cannot resolve them from the live theme
//! without hardcoding a literal hex (which would violate the "never hardcode
//! hexes" rule below), so it labels them N/A rather than painting a swatch —
//! see the Completion Summary.

use frust::{
    AnyView, Axis, Color, CrossAxisAlignment, EdgeInsets, FlexView, GlyphInk, Image, ImageFit,
    ImageSource, Padding, Row, ShapeScale, SizedBox, StatusPalette, TextView, Theme, any, glyph,
    inflexible, text, use_context,
};

use crate::CatalogState;

// ---- Local facade-only helpers (this file's own scope; see module docs) ---

/// A 1×1 solid-color [`ImageSource`] — the facade-only way to paint an
/// arbitrary filled rectangle (mirrors `examples/huddle`'s
/// `ui::solid_source`, reproduced locally per this task's single-file scope).
fn solid_source(color: Color) -> ImageSource {
    let c = color.components;
    let to_u8 = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    ImageSource::from_rgba8(
        vec![to_u8(c[0]), to_u8(c[1]), to_u8(c[2]), to_u8(c[3])],
        1,
        1,
    )
}

/// Render `color` as an uppercase `#RRGGBB` hex string (alpha dropped — every
/// `ColorScheme`/`StatusPalette`/`GlyphInk` role is an opaque color).
fn hex(color: Color) -> String {
    let c = color.components;
    let to_u8 = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02X}{:02X}{:02X}", to_u8(c[0]), to_u8(c[1]), to_u8(c[2]))
}

const SWATCH_SIZE: f64 = 52.0;
const RADIUS_CHIP_SIZE: f64 = 44.0;

/// One color swatch: a filled box, the token's reference name, and its
/// live-resolved hex text — `color` is always a value read off the current
/// theme by the caller, never a literal.
fn swatch(name: &str, color: Color, label_color: Color) -> AnyView<CatalogState> {
    any(Padding(
        EdgeInsets::all(6.0),
        FlexView::new(
            Axis::Vertical,
            vec![
                inflexible(any(SizedBox(Some(SWATCH_SIZE), Some(SWATCH_SIZE))
                    .child(Image(solid_source(color)).fit(ImageFit::Fill)))),
                inflexible(any(mono_label(name.to_string(), label_color))),
                inflexible(any(mono_label(hex(color), label_color))),
            ],
        )
        .cross_axis(CrossAxisAlignment::Start),
    ))
}

/// A "N/A" placeholder swatch for a token with no live `ColorScheme` role
/// (see the module docs' "Unmapped Glyph text tokens" section) — same
/// footprint as [`swatch`] so the ramp stays visually aligned, but painted
/// with a dashed-feel dim label instead of a resolved color/hex.
fn swatch_na(name: &str, reason: &str, label_color: Color) -> AnyView<CatalogState> {
    any(Padding(
        EdgeInsets::all(6.0),
        FlexView::new(
            Axis::Vertical,
            vec![
                inflexible(any(mono_label(format!("{name} — N/A"), label_color))),
                inflexible(any(text(reason.to_string()).size(9.5).color(label_color))),
            ],
        )
        .cross_axis(CrossAxisAlignment::Start),
    ))
}

/// A small monospace-styled label (the Glyph type scale's `micro` slot),
/// used for every swatch name/hex line — see the module docs' "mono name"
/// requirement.
fn mono_label(content: String, color: Color) -> TextView {
    text(content).size(10.0).color(color)
}

/// Group `items` into rows of `per_row`, stacked vertically — the closest
/// approximation of a wrapping grid the facade offers today (no `Wrap`
/// widget; API friction, see the Completion Summary).
fn wrap_rows(mut items: Vec<AnyView<CatalogState>>, per_row: usize) -> AnyView<CatalogState> {
    let mut rows: Vec<AnyView<CatalogState>> = Vec::new();
    while !items.is_empty() {
        let rest = if items.len() > per_row {
            items.split_off(per_row)
        } else {
            Vec::new()
        };
        rows.push(any(Row(items)));
        items = rest;
    }
    any(
        FlexView::new(Axis::Vertical, rows.into_iter().map(inflexible).collect())
            .cross_axis(CrossAxisAlignment::Start),
    )
}

/// A titled section: a heading line, a small vertical gap, then `body`.
fn section(
    title: &str,
    subtitle: Option<&str>,
    body: AnyView<CatalogState>,
) -> AnyView<CatalogState> {
    let mut children: Vec<AnyView<CatalogState>> = vec![any(text(title.to_string()).size(15.0))];
    if let Some(sub) = subtitle {
        children.push(any(text(sub.to_string()).size(10.5)));
    }
    children.push(any(SizedBox(None, Some(8.0))));
    children.push(body);
    children.push(any(SizedBox(None, Some(20.0))));

    any(Padding(
        EdgeInsets::symmetric(16.0, 8.0),
        FlexView::new(
            Axis::Vertical,
            children.into_iter().map(inflexible).collect(),
        )
        .cross_axis(CrossAxisAlignment::Start),
    ))
}

/// One type-scale specimen: the sample text set in `style`, plus a
/// family/size caption underneath.
fn type_specimen(view: TextView, meta: &str, label_color: Color) -> AnyView<CatalogState> {
    any(Padding(
        EdgeInsets::symmetric(0.0, 6.0),
        FlexView::new(
            Axis::Vertical,
            vec![
                inflexible(any(view)),
                inflexible(any(text(meta.to_string()).size(10.0).color(label_color))),
            ],
        )
        .cross_axis(CrossAxisAlignment::Start),
    ))
}

/// One radius chip: a themed-fill rounded box (via [`glyph::skeleton`],
/// this file's closest available rounded-rect primitive — see the module
/// docs) at `radius`, sized against [`RADIUS_CHIP_SIZE`], labeled underneath.
fn radius_chip(label: &str, radius: f64, label_color: Color) -> AnyView<CatalogState> {
    let resolved = ShapeScale::resolve(radius, RADIUS_CHIP_SIZE, RADIUS_CHIP_SIZE);
    any(Padding(
        EdgeInsets::all(8.0),
        FlexView::new(
            Axis::Vertical,
            vec![
                inflexible(any(
                    glyph::skeleton(RADIUS_CHIP_SIZE, RADIUS_CHIP_SIZE).radius(resolved)
                )),
                inflexible(any(mono_label(label.to_string(), label_color))),
            ],
        )
        .cross_axis(CrossAxisAlignment::Start),
    ))
}

/// See the page-fn contract in [`crate::pages`].
pub fn page(_state: &CatalogState) -> AnyView<CatalogState> {
    // Live theme read: subscribes this build to `set_app_theme` writes, so a
    // header brightness/motion toggle repaints every swatch below from the
    // CURRENT scheme — never a hardcoded hex.
    let theme = use_context::<Theme>().unwrap_or_else(Theme::glyph_baseline);
    let scheme = theme.scheme();
    let muted = scheme.on_surface_variant;
    let status = theme
        .extension::<StatusPalette>()
        .map(|palette| *palette.colors(theme.brightness));
    let ink = theme.extension::<GlyphInk>().copied();
    let type_scale = theme.type_scale.clone();
    let shape = theme.shape;

    let mut page_children: Vec<AnyView<CatalogState>> = Vec::new();
    page_children.push(any(Padding(
        EdgeInsets::symmetric(16.0, 12.0),
        FlexView::new(
            Axis::Vertical,
            vec![
                inflexible(any(text("Foundations").size(24.0))),
                inflexible(any(text(
                    "Color, type, and radius tokens — every specimen below re-resolves \
                     from the live theme, so the header's brightness/motion toggles repaint \
                     this whole page.",
                )
                .size(11.0)
                .color(muted))),
            ],
        )
        .cross_axis(CrossAxisAlignment::Start),
    )));

    // ---- Color tokens -------------------------------------------------
    page_children.push(section(
        "Background ramp",
        Some("bg-void -> bg-base -> bg-surface -> bg-raised -> bg-overlay -> bg-hover"),
        wrap_rows(
            vec![
                swatch("bg-void", scheme.surface_container_lowest, muted),
                swatch("bg-base", scheme.surface_container_low, muted),
                swatch("bg-surface", scheme.surface, muted),
                swatch("bg-raised", scheme.surface_container_high, muted),
                swatch("bg-overlay", scheme.surface_container_highest, muted),
                swatch("bg-hover", scheme.surface_bright, muted),
            ],
            3,
        ),
    ));

    page_children.push(section(
        "Accent",
        Some("amber — the single accent hue, filled role"),
        wrap_rows(vec![swatch("amber", scheme.primary_container, muted)], 3),
    ));

    let (success, success_faint, warning, warning_faint) = match status {
        Some(colors) => (
            colors.success,
            colors.success_container,
            colors.warning,
            colors.warning_container,
        ),
        // Defensive fallback — every built-in baseline attaches a
        // `StatusPalette`, so this never actually triggers.
        None => (
            scheme.tertiary,
            scheme.tertiary_container,
            scheme.tertiary,
            scheme.tertiary_container,
        ),
    };
    page_children.push(section(
        "Semantic",
        Some("cyan (info) / success / warning / error, each with a -faint container"),
        wrap_rows(
            vec![
                swatch("cyan", scheme.tertiary, muted),
                swatch("cyan-faint", scheme.tertiary_container, muted),
                swatch("success", success, muted),
                swatch("success-faint", success_faint, muted),
                swatch("warning", warning, muted),
                swatch("warning-faint", warning_faint, muted),
                swatch("error", scheme.error, muted),
                swatch("error-faint", scheme.error_container, muted),
            ],
            4,
        ),
    ));

    page_children.push(section(
        "Text",
        Some(
            "fg / fg-muted resolve from ColorScheme; fg-dim / fg-faintest have no \
             live role (frust_theme::glyph::color, documented leftover)",
        ),
        wrap_rows(
            vec![
                swatch("fg", scheme.on_surface, muted),
                swatch("fg-muted", scheme.on_surface_variant, muted),
                swatch_na("fg-dim", "no ColorScheme role", muted),
                swatch_na("fg-faintest", "no ColorScheme role", muted),
            ],
            4,
        ),
    ));

    page_children.push(section(
        "Borders",
        Some("border -> outline_variant, border-bright -> outline"),
        wrap_rows(
            vec![
                swatch("border", scheme.outline_variant, muted),
                swatch("border-bright", scheme.outline, muted),
            ],
            4,
        ),
    ));

    if let Some(colors) = status {
        page_children.push(section(
            "StatusPalette extension",
            Some("theme.extension::<StatusPalette>() — success / warning / info"),
            wrap_rows(
                vec![
                    swatch("success", colors.success, muted),
                    swatch("warning", colors.warning, muted),
                    swatch("info", colors.info, muted),
                ],
                4,
            ),
        ));
    }

    if let Some(ink) = ink {
        page_children.push(section(
            "GlyphInk extension (brightness-invariant)",
            Some(
                "theme.extension::<GlyphInk>() — the terminal block + tooltip stay dark \
                 in BOTH Light and Dark; toggle the header brightness to confirm these \
                 swatches never move",
            ),
            wrap_rows(
                vec![
                    swatch("terminal-bg", ink.terminal_bg, muted),
                    swatch("terminal-head", ink.terminal_head, muted),
                    swatch("terminal-fg", ink.terminal_fg, muted),
                    swatch("terminal-out", ink.terminal_out, muted),
                    swatch("terminal-comment", ink.terminal_comment, muted),
                    swatch("terminal-prompt", ink.terminal_prompt, muted),
                    swatch("tooltip-bg", ink.tooltip_bg, muted),
                    swatch("tooltip-fg", ink.tooltip_fg, muted),
                ],
                4,
            ),
        ));
    }

    // ---- Type scale -----------------------------------------------------
    page_children.push(section(
        "Type scale",
        Some("6 roles, theme.type_scale — Space Mono (display/heading), IBM Plex Mono (rest)"),
        any(FlexView::new(
            Axis::Vertical,
            vec![
                inflexible(type_specimen(
                    text("Foundations").style(type_scale.display_large.clone()).color(scheme.on_surface),
                    "display/32 · Space Mono 700",
                    muted,
                )),
                inflexible(type_specimen(
                    text("Foundations").style(type_scale.headline_large.clone()).color(scheme.on_surface),
                    "heading/20 · Space Mono 700",
                    muted,
                )),
                inflexible(type_specimen(
                    text("Foundations").style(type_scale.title_large.clone()).color(scheme.on_surface),
                    "title/15 · IBM Plex Mono 500",
                    muted,
                )),
                inflexible(type_specimen(
                    text("The quick brown fox jumps.").style(type_scale.body_large.clone()).color(scheme.on_surface),
                    "body/12.5→13 (mobile floor) · IBM Plex Mono 400",
                    muted,
                )),
                inflexible(type_specimen(
                    text("The quick brown fox jumps.").style(type_scale.body_small.clone()).color(muted),
                    "caption/11 · IBM Plex Mono 400 · fg-muted",
                    muted,
                )),
                inflexible(type_specimen(
                    text("THE QUICK BROWN FOX".to_string()).style(type_scale.label_small.clone()).color(muted),
                    "micro/9.5 · IBM Plex Mono 500 · fg-dim(≈fg-muted, unmapped) · uppercase · +0.05em",
                    muted,
                )),
            ],
        )
        .cross_axis(CrossAxisAlignment::Start)),
    ));

    // ---- Radius scale -----------------------------------------------------
    page_children.push(section(
        "Radius scale",
        Some("4 / 6 / 10 / 16 / full, theme.shape — painted via glyph::skeleton(..).radius(..)"),
        any(Row(vec![
            radius_chip("4px", shape.extra_small, muted),
            radius_chip("6px", shape.small, muted),
            radius_chip("10px", shape.medium, muted),
            radius_chip("16px", shape.large, muted),
            radius_chip("full", shape.full, muted),
        ])),
    ));

    any(FlexView::new(
        Axis::Vertical,
        page_children.into_iter().map(inflexible).collect(),
    )
    .cross_axis(CrossAxisAlignment::Start))
}
