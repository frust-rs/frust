//! `Glyph`-design cases (see [`crate::case::Design::Glyph`]) — one case per
//! non-index page under `apps/website/widgets/design-systems/glyph/`, slug
//! `glyph/<file-stem>`: `accordion`, `alert`, `app-bar`, `badge-and-tag`,
//! `card`, `command-palette`, `dialog`, `list`, `loaders`, `nav-bar`,
//! `tabs`, `term-block`, `toast`, `toggle`.
//!
//! Every case is a static composition of the real `plugins/glyph` widgets,
//! built directly (never through `frust_glyph::install`/`show_glyph_dialog`/
//! `show_command_palette`'s navigator-overlay path, which would need a
//! reactive runtime this pure-`View` registry doesn't have — see the crate
//! docs' pure-`View` constraint). `GlyphDialogView`/`CommandPaletteView` are
//! themselves plain `View`s, so they compose the same way every other case
//! here does: no [`crate::base::framed`] fill, [`Case::DEFAULT_SIZE`] unless
//! a case has a reason to differ.
//!
//! # Font registration (verified, not fixed here)
//!
//! Glyph's type scale stacks every slot as
//! `FontFamily::stack_with_generic(["Space Mono" | "IBM Plex Mono"],
//! GenericSlot::Monospace)` (`plugins/glyph/src/tokens/scales.rs`) — a named
//! family with a generic monospace fallback baked into the token itself.
//! `crates/frust-testing/src/snapshot.rs`'s `snapshot_text_context` only
//! registers the four bundled *test* faces (Latin/Arabic/CJK/emoji,
//! `crates/frust-testing/src/fonts.rs`) — it does not call
//! `frust_glyph::font_data`/`install`, so "Space Mono"/"IBM Plex Mono" never
//! resolve by name in this harness. Every case here uses plain ASCII/simple
//! Unicode (glyph markers, box-drawing characters) that the generic
//! `Monospace` fallback in the same stack covers, so text still shapes with
//! real glyphs (no tofu) — just via the host's system monospace face rather
//! than the bundled one. Registering the bundled Glyph faces into the
//! snapshot `TextContext` would require a change to
//! `crates/frust-testing/src/snapshot.rs` (and possibly `theme.rs`), both
//! out of this task's `write_files` scope — see the task summary.

use kurbo::Size;

use frust_core::{AnyView, any};
use frust_glyph::{
    AlertVariant, BadgeVariant, PaletteItem, StatDelta, TermLine, ToastVariant, accordion, alert,
    app_bar, badge, command_palette, dots_loader, empty_state, glyph_card, glyph_dialog,
    glyph_list, glyph_list_item, glyph_nav_bar, glyph_nav_item, progress, segmented_control,
    skeleton, stat_card, tabs, tag, term_block, toast, toggle,
};
use frust_widgets::{
    Axis, ButtonStyle, CrossAxisAlignment, FlexView, Row, SizedBox, button, flexible, inflexible,
    text,
};

use crate::base::{framed, framed_in};
use crate::case::{Case, Design};

/// A taller-than-[`Case::DEFAULT_SIZE`] frame for a case whose content (a
/// list plus its empty-state counterpart, or a card plus a stat-card row)
/// runs past 240 logical px of height.
const TALL_FRAME: Size = Size::new(360.0, 420.0);

/// A modestly taller frame for a case (three stacked alerts) that clips by
/// only a few px at [`Case::DEFAULT_SIZE`]'s 240.
const MEDIUM_FRAME: Size = Size::new(360.0, 300.0);

/// `glyph/accordion` — one settled-open panel (no tween on first build, per
/// the accordion doc's "already open" contract) above a settled-closed one.
fn accordion_case() -> AnyView<()> {
    framed(FlexView::new(
        Axis::Vertical,
        vec![
            inflexible(
                accordion(
                    "Deploy pipeline",
                    text("Builds, tests, and ships the release artifact on every push to main."),
                )
                .open(true),
            ),
            inflexible(SizedBox(None, Some(12.0))),
            inflexible(accordion("Rollback", text("Revert to the previous release.")).open(false)),
        ],
    ))
}

/// `glyph/alert` — the four severities stacked.
fn alert_case() -> AnyView<()> {
    framed_in(
        MEDIUM_FRAME,
        FlexView::new(
            Axis::Vertical,
            vec![
                inflexible(alert(
                    AlertVariant::Info,
                    "Heads up",
                    "A new workspace layout is available in settings.",
                )),
                inflexible(SizedBox(None, Some(10.0))),
                inflexible(alert(
                    AlertVariant::Success,
                    "Saved",
                    "Your changes have been saved.",
                )),
                inflexible(SizedBox(None, Some(10.0))),
                inflexible(alert(
                    AlertVariant::Warning,
                    "Low disk space",
                    "Free up space soon to avoid interruptions.",
                )),
            ],
        ),
    )
}

/// `glyph/app-bar` — the compact bar with a subtitle, elevated.
fn appbar_case() -> AnyView<()> {
    framed(
        app_bar::<()>("Deploys")
            .subtitle("production")
            .elevated(true),
    )
}

/// `glyph/badge-and-tag` — one badge per status variant, plus a removable and
/// a static tag.
fn badge_and_tag_case() -> AnyView<()> {
    framed(FlexView::new(
        Axis::Vertical,
        vec![
            inflexible(
                FlexView::new(
                    Axis::Horizontal,
                    vec![
                        inflexible(badge("connected", BadgeVariant::Success).dot(true)),
                        inflexible(SizedBox(Some(8.0), None)),
                        inflexible(badge("degraded", BadgeVariant::Warning).dot(true)),
                        inflexible(SizedBox(Some(8.0), None)),
                        inflexible(badge("offline", BadgeVariant::Error).dot(true)),
                    ],
                )
                .cross_axis(CrossAxisAlignment::Center),
            ),
            inflexible(SizedBox(None, Some(16.0))),
            inflexible(
                FlexView::new(
                    Axis::Horizontal,
                    vec![
                        inflexible(tag::<()>("stable")),
                        inflexible(SizedBox(Some(8.0), None)),
                        inflexible(tag("v0.44.1").on_remove(|_: &mut ()| {})),
                    ],
                )
                .cross_axis(CrossAxisAlignment::Center),
            ),
        ],
    ))
}

/// `glyph/card` — a three-slot card with a status footer, above a two-up
/// stat-card row mixing both delta directions.
fn card_case() -> AnyView<()> {
    framed_in(
        TALL_FRAME,
        FlexView::new(
            Axis::Vertical,
            vec![
                inflexible(
                    glyph_card::<()>()
                        .title(text("Deploy pipeline"))
                        .desc(text(
                            "Builds, tests, and ships the release artifact on every push to main.",
                        ))
                        .footer(Row(vec![any(
                            badge("stable", BadgeVariant::Success).dot(true)
                        )])),
                ),
                inflexible(SizedBox(None, Some(14.0))),
                inflexible(FlexView::new(
                    Axis::Horizontal,
                    vec![
                        flexible(1, stat_card("Sessions", "1,284")),
                        flexible(
                            1,
                            stat_card("Uptime", "99.98%").delta(StatDelta::Up, "0.4%"),
                        ),
                    ],
                )),
            ],
        ),
    )
}

/// `glyph/command-palette` — a live query over a short filtered list, built
/// directly (not through the navigator-pushed `show_command_palette`).
///
/// The hint is spelled out as `"Cmd+D"` rather than the literal `⌘D`.
/// Neither bundled Glyph face — Space Mono nor IBM Plex Mono
/// (`plugins/glyph/src/tokens/scales.rs`) — carries U+2318 PLACE OF INTEREST
/// SIGN (confirmed against both faces' `cmap` tables), so the raw symbol
/// falls back to `.notdef` and paints as an empty box. Registering a
/// symbol-covering fallback face was considered and rejected: the live tier
/// is payload-constrained (the lp-00 fonts already cost +560,157 B gzipped),
/// and reachability, not bundling, is what the wasm linker keeps, so a new
/// face earns its weight back into the bundle the moment anything reaches
/// it. A spelled-out hint costs nothing and keeps this case eligible for the
/// live tier; the one loss is that the catalog no longer demonstrates the
/// literal glyph the real widget would show on a Mac keyboard.
fn command_palette_case() -> AnyView<()> {
    framed(
        command_palette::<(), _, _>(
            vec![PaletteItem::new("Deploy").hint("Cmd+D")],
            |_: &mut (), _: String| {},
            |_: &mut (), _: usize| {},
        )
        .query("dep")
        .placeholder("Type a command or search…"),
    )
}

/// `glyph/dialog` — a confirm/cancel modal, built directly (not through the
/// navigator-pushed `show_glyph_dialog`).
fn dialog_case() -> AnyView<()> {
    framed(
        glyph_dialog::<()>()
            .title("Revoke observer-token?")
            .body("Any device using this token loses access immediately. This can't be undone.")
            .action(any(button("Cancel", |_: &mut ()| {})
                .style(ButtonStyle::Ghost)
                .small()))
            .action(any(button("Revoke token", |_: &mut ()| {})
                .style(ButtonStyle::Danger)
                .small())),
    )
}

/// `glyph/list` — a glyph-led list, above its zero-item `empty_state`
/// counterpart.
fn list_case() -> AnyView<()> {
    framed_in(
        TALL_FRAME,
        FlexView::new(
            Axis::Vertical,
            vec![
                inflexible(glyph_list::<()>(vec![
                    glyph_list_item("$", "deploy")
                        .sub("main → production")
                        .meta("2m ago")
                        .chevron(true),
                    glyph_list_item("#", "rollback")
                        .sub("v0.44.0 → v0.43.2")
                        .meta("1h ago")
                        .chevron(true),
                ])),
                inflexible(SizedBox(None, Some(14.0))),
                inflexible(
                    empty_state::<()>(
                        "No deployments yet",
                        "Trigger your first deploy to see activity here.",
                    )
                    .glyph("▪")
                    .action(button("Deploy now", |_: &mut ()| {}).style(ButtonStyle::Primary)),
                ),
            ],
        ),
    )
}

/// `glyph/loaders` — the determinate bar, the shimmer placeholder and the
/// indeterminate dot cycle.
fn loaders_case() -> AnyView<()> {
    framed(
        FlexView::new(
            Axis::Vertical,
            vec![
                inflexible(progress(0.65)),
                inflexible(SizedBox(None, Some(18.0))),
                inflexible(skeleton(220.0, 16.0)),
                inflexible(SizedBox(None, Some(18.0))),
                inflexible(dots_loader()),
            ],
        )
        .cross_axis(CrossAxisAlignment::Start),
    )
}

/// `glyph/nav-bar` — the glyph-character bottom bar.
fn navbar_case() -> AnyView<()> {
    framed(glyph_nav_bar::<(), _>(
        vec![
            glyph_nav_item("┌", "frame"),
            glyph_nav_item("─", "stream"),
            glyph_nav_item("╳", "close"),
        ],
        0,
        |_: &mut (), _: usize| {},
    ))
}

/// `glyph/tabs` — the label tab strip above a pill segmented control.
fn tabs_case() -> AnyView<()> {
    framed(FlexView::new(
        Axis::Vertical,
        vec![
            inflexible(tabs::<(), _>(
                vec![
                    "Overview".to_string(),
                    "Activity".to_string(),
                    "Settings".to_string(),
                ],
                0,
                |_: &mut (), _: usize| {},
            )),
            inflexible(SizedBox(None, Some(20.0))),
            inflexible(segmented_control::<(), _>(
                vec![
                    "List".to_string(),
                    "Grid".to_string(),
                    "Compact".to_string(),
                ],
                1,
                |_: &mut (), _: usize| {},
            )),
        ],
    ))
}

/// `glyph/term-block` — a prompt/output/comment transcript, unstaggered (a
/// staggered reveal only replays on a fresh widget build, never settles in a
/// single-paint snapshot — see the [module docs](self)'s font-registration
/// note for the same single-frame-paint constraint applied to a different
/// widget).
fn term_block_case() -> AnyView<()> {
    framed(term_block(vec![
        TermLine::prompt("frust build apk --release"),
        TermLine::output("Compiling glyph-catalog v0.1.0"),
        TermLine::output("Finished release [optimized] target(s) in 38.2s"),
        TermLine::comment("# staggered per-line reveal — replays below"),
    ]))
}

/// `glyph/toast` — three severities of the transient message itself (not the
/// host, whose enter fade never advances past its first frame in this
/// single-paint harness — see [`toast_host`](frust_glyph::toast_host)).
fn toast_case() -> AnyView<()> {
    framed(FlexView::new(
        Axis::Vertical,
        vec![
            inflexible(toast("New layout available").variant(ToastVariant::Info)),
            inflexible(SizedBox(None, Some(12.0))),
            inflexible(toast("Changes saved").variant(ToastVariant::Success)),
            inflexible(SizedBox(None, Some(12.0))),
            inflexible(toast("Low disk space").variant(ToastVariant::Warning)),
        ],
    ))
}

/// `glyph/toggle` — checked and unchecked, both labelled.
fn toggle_case() -> AnyView<()> {
    framed(FlexView::new(
        Axis::Vertical,
        vec![
            inflexible(toggle::<(), _>(true, |_: &mut (), _: bool| {}).label("Notifications")),
            inflexible(SizedBox(None, Some(16.0))),
            inflexible(toggle::<(), _>(false, |_: &mut (), _: bool| {}).label("Auto-sync")),
        ],
    ))
}

/// This module's slice of the registry [`crate::cases`]: one entry per
/// `apps/website/widgets/design-systems/glyph/*.mdx` page (see the module
/// docs' slug list).
pub const CASES: &[Case] = &[
    Case {
        slug: "glyph/accordion",
        title: "Accordion",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Glyph,
        build: accordion_case,
    },
    Case {
        slug: "glyph/alert",
        title: "Alert",
        size: MEDIUM_FRAME,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Glyph,
        build: alert_case,
    },
    Case {
        slug: "glyph/app-bar",
        title: "App bar",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Glyph,
        build: appbar_case,
    },
    Case {
        slug: "glyph/badge-and-tag",
        title: "Badge and tag",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Glyph,
        build: badge_and_tag_case,
    },
    Case {
        slug: "glyph/card",
        title: "Card",
        size: TALL_FRAME,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Glyph,
        build: card_case,
    },
    Case {
        slug: "glyph/command-palette",
        title: "Command palette",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Glyph,
        build: command_palette_case,
    },
    Case {
        slug: "glyph/dialog",
        title: "Dialog",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Glyph,
        build: dialog_case,
    },
    Case {
        slug: "glyph/list",
        title: "List",
        size: TALL_FRAME,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Glyph,
        build: list_case,
    },
    Case {
        slug: "glyph/loaders",
        title: "Loaders",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Glyph,
        build: loaders_case,
    },
    Case {
        slug: "glyph/nav-bar",
        title: "Nav bar",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Glyph,
        build: navbar_case,
    },
    Case {
        slug: "glyph/tabs",
        title: "Tabs and segmented control",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Glyph,
        build: tabs_case,
    },
    Case {
        slug: "glyph/term-block",
        title: "Terminal block",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Glyph,
        build: term_block_case,
    },
    Case {
        slug: "glyph/toast",
        title: "Toast",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Glyph,
        build: toast_case,
    },
    Case {
        slug: "glyph/toggle",
        title: "Toggle",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Glyph,
        build: toggle_case,
    },
];
