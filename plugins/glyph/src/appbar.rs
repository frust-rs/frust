//! Glyph top AppBar — core: the compact
//! terminal-native top bar per the Glyph design system's sections 01/03/05,
//! retrieved 2026-07-22.
//!
//! # Anatomy
//!
//! A `52px` content bar of three zones — **leading** (a back arrow / brand mark
//! / nothing, an app-supplied [`AnyView`]), **title** (a title string plus an
//! optional 10px subtitle row, ellipsized), and **trailing** (0–2 app-supplied
//! action [`AnyView`]s). Icon hit targets are `40px`, `6px` horizontal edge
//! padding, `2px` slot gaps (the HTML's `.appbar`/`.ab-icon` rules).
//!
//! # Top-inset consumption (the Flutter parity point)
//!
//! Layout adds [`WindowInsets::padding().top`](frust::authoring::WindowInsets::padding)
//! to its own height and offsets its content **below** the inset; the background
//! paints the full extended height, so the bar surface runs under the status
//! bar. Content beneath a Glyph AppBar therefore never needs a top `SafeArea`
//! edge — exactly Flutter's `AppBar` behavior.
//!
//! # Window-control corners (iPadOS 26+)
//!
//! A windowed iPadOS 26+ app has a window control (traffic-light cluster) that
//! protrudes into the bar's top corners. Layout reads
//! `ctx.window_insets().corner_insets` (physical corners, never consumed): a
//! corner whose `height > 0.0` shifts that side's slot edge inward by the
//! corner's `width` — the leading edge starts at `PAD_X + left` and the trailing
//! edge ends at `width - PAD_X - right`. The bar's height never changes, and the
//! title anchoring rules are unchanged (they simply see the shifted edges). A
//! corner with zero height (full screen, every other platform) shifts nothing.
//!
//! The shift assumes the bar spans the window's top edge (its content band
//! starts at the safe-area top and its left/right edges are the window's). A bar
//! placed elsewhere (detail pane, sheet, dialog) opts out with
//! [`AppBarView::corner_shift`]`(false)`; no automatic detection exists (corners
//! are never consumed; see the `CornerInsets` rustdoc).
//!
//! # Elevation
//!
//! [`AppBarView::elevated`] is an app-fed flag (the app toggles it off its own
//! `on_scroll`, the same way the HTML flips `.elevated` at `y > 4`): a
//! transparent bottom border animates to `outline` + a drop shadow over
//! `~250ms`, collapsing instantly under `reduce_motion`.
//!
//! # Title crossfade
//!
//! When the title string changes across rebuilds, the bar stages the old and
//! new runs with a directional shift (`±14px` x-offset + fade over `~220ms`),
//! driven from `PaintCtx::frame_time` (the dialog/toast staging precedent — the
//! bar shapes its own runs like [`super::navbar`] rather than nesting a
//! `PatternSwitcher`). Direction is forward (new slides in from the right) by
//! default; [`AppBarView::title_direction`] flips it to back-nav.
//!
//! # Title position
//!
//! The compact title (+subtitle) anchors leading-aligned by default —
//! [`AppBarView::title_position`]`(`[`TitlePosition::Center`]`)` optically
//! centers it over the bar's full width instead, clamped to the free span
//! between the leading/trailing slots on collision (see
//! [`AppBarWidget::resolve_title_x`] for the exact rule). The crossfade above
//! keeps working under either position — its shift is relative to wherever
//! the run currently sits, not to a fixed leading edge.
//!
//! # Selection mode
//!
//! [`AppBarView::selection`]`(Some(`[`SelectionBar`]`))` morphs the bar into a
//! selection face: the surface tints with a `primary_container`-family wash, the
//! leading slot becomes a widget-owned `×`-close (firing `on_close` on
//! up-inside), the title becomes `"N selected"`, and the trailing swaps to the
//! caller-supplied bulk-action views. The title↔count face crosses over
//! vertically (`±6px` y + fade); the trailing action set swaps structurally on
//! the rebuild that toggled selection (a full dual-face child crossfade is out
//! of scope — the animated cues are the tint, the title↔count morph, and the
//! leading↔close swap).
//!
//! # Typeface
//!
//! Every run reads its family at layout from a live type-scale role — the one
//! whose Glyph family is the face that run has always painted: the compact
//! title and the selection count from `titleMedium`, the subtitle and the
//! connection banner from `bodySmall` (all IBM Plex Mono), and the large
//! variant's big title from `headlineSmall` (Space Mono). Sizes and weights
//! stay this module's own. Unthemed, each falls back to the matching Glyph
//! stack. The family is part of each run's cached style, so a theme swap
//! reshapes the bar (see [`super::badge`]'s Typeface section).
//!
//! # Colors (accent-role split)
//!
//! Every color resolves from [`Theme::scheme`] with a Glyph **dark** constant
//! fallback (the [`super::navbar`] `resolve_nav_colors` precedent) — no
//! `StatusPalette` here. The selection wash is `primary_container` (the bright
//! *fill* role) while the count/close ink is `primary` (the accent *ink* role);
//! conflating the two is the catalog's most common accent bug
//! (`docs/CODE_STANDARDS.md`'s Glyph accent-role split).
//!
//! # Controlled component
//!
//! All state — title, subtitle, elevation, selection, actions — flows in from
//! the view every rebuild; the widget never self-mutates app-visible state.

use std::rc::Rc;
use std::time::Duration;

use frust::Theme;
use frust::authoring::Role;
use frust::authoring::text::{
    FontFamily, FontWeight, GenericSlot, TextContext, TextLayout, TextStyle,
};
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, PointerPhase, SemanticsCtx, View, Widget,
};
use frust::{Curve, FrameTime};
use kurbo::{Affine, Point, Rect, Size};
use peniko::{Brush, Color};

use frust::Timing;
use frust::{TransitionDriver, make_driver};

use crate::press::presses;

// ---- Metrics (compact bar, retrieved 2026-07-22) ---------------------------

/// Compact content-bar height, logical px (`.appbar{height:52px}`). The top
/// inset is added on top of this (see the [module docs](self)).
const BAR_HEIGHT: f64 = 52.0;
/// Horizontal edge padding, logical px (`.appbar{padding:0 6px}`).
const PAD_X: f64 = 6.0;
/// Gap between adjacent slots, logical px (`.appbar{gap:2px}`).
const SLOT_GAP: f64 = 2.0;
/// Icon hit-target size, logical px (`.ab-icon{width:40px;height:40px}`). Also
/// the widget-owned selection close button's box.
const ICON_SIZE: f64 = 40.0;
/// Title font size, logical px (`.ab-title{font-size:13px}`).
const TITLE_SIZE: f32 = 13.0;
/// Subtitle font size, logical px (`.ab-subtitle{font-size:10px}`).
const SUBTITLE_SIZE: f32 = 10.0;
/// Selection count font size, logical px (`.sel-count{font-size:13px}`).
const COUNT_SIZE: f32 = 13.0;
/// Vertical gap between the title and subtitle rows, logical px (an
/// original, hand-picked value — a tight 1px separation matching the
/// design system's stacked rows).
const SUBTITLE_GAP: f64 = 1.0;

/// Title-crossfade x-shift, logical px (`.bn-title-a{translateX(-14px)}`).
const TITLE_SHIFT: f64 = 14.0;
/// Selection-morph y-shift, logical px (`.sel-normal{translateY(-6px)}`).
const SELECTION_SHIFT: f64 = 6.0;
/// Selection wash alpha over the surface (`--amber-faint` = accent at 12%; here
/// sourced from the `primary_container` fill role per the accent-role split).
const SELECTION_WASH_ALPHA: f32 = 0.12;

/// Elevation animation duration (`.appbar{transition:...box-shadow .25s}`).
const ELEVATION_DURATION: Duration = Duration::from_millis(250);
/// Selection-morph animation duration (`durations.base` — the same 220ms the
/// toast/dialog enter uses).
const SELECTION_DURATION: Duration = Duration::from_millis(220);
/// Title-crossfade duration (the design system's "small directional shift", `durations.base`).
const TITLE_DURATION: Duration = Duration::from_millis(220);
/// Title-crossfade easing (the HTML's `--ease-out` cubic).
const TITLE_CURVE: Curve = Curve::Cubic(0.16, 1.0, 0.3, 1.0);
/// `reduce_motion`'s collapsed crossfade duration (mirrors the toast/dialog
/// constant of the same name).
const REDUCE_MOTION_DURATION: Duration = Duration::from_millis(120);

/// Elevated drop-shadow recipe (`.appbar.elevated{box-shadow:0 6px 16px
/// rgba(0,0,0,0.28)}`).
const SHADOW_Y: f64 = 6.0;
const SHADOW_BLUR: f64 = 16.0;
const SHADOW_ALPHA: f32 = 0.28;

// ---- Large-variant metrics (retrieved 2026-07-22) --------------------------

/// Large-bar top padding at rest, logical px (`.appbar-large{padding-top:6px}`),
/// interpolated `6→0` by collapse progress (`paddingTop = 6 - 6*progress`).
const LARGE_PAD_TOP: f64 = 6.0;
/// Large-bar bottom padding at rest, logical px
/// (`.appbar-large{padding-bottom:10px}`), interpolated `10→0`
/// (`paddingBottom = 10 - 10*progress`).
const LARGE_PAD_BOTTOM: f64 = 10.0;
/// Big-title font size at rest, logical px (`.ab-big-title{font-size:19px}`).
const BIG_TITLE_SIZE_MAX: f32 = 19.0;
/// Big-title font-size collapse span (`fontSize = 19 - 6*progress`), so the
/// fully-collapsed size is `BIG_TITLE_SIZE_MAX - BIG_TITLE_SIZE_SPAN = 13px`,
/// exactly the compact [`TITLE_SIZE`].
const BIG_TITLE_SIZE_SPAN: f32 = 6.0;
/// Big-title horizontal padding, logical px (`.ab-big-title{padding:8px 12px}`).
const BIG_TITLE_PAD_X: f64 = 12.0;
/// Big-title top padding, logical px (the `8px` of `padding:8px 12px 2px`).
const BIG_TITLE_PAD_TOP: f64 = 8.0;
/// Big-title bottom padding at rest, logical px (the `2px` of
/// `padding:8px 12px 2px`), interpolated `2→0` (`paddingBottom = 2*(1-progress)`).
const BIG_TITLE_PAD_BOTTOM: f64 = 2.0;
/// Meta-row height at rest, logical px (`.ab-meta-row` shown at `max-height:20px`
/// in the HTML), interpolated `20→0` by collapse progress + a `1-progress` fade.
const META_HEIGHT: f64 = 20.0;
/// Meta-row horizontal padding, logical px (`.ab-meta-row{padding:0 12px}`).
const META_PAD_X: f64 = 12.0;

// ---- Connection-banner metrics ------------------------------------------

/// Banner strip height when fully open, logical px
/// (`.conn-banner.show{max-height:40px}`).
const BANNER_HEIGHT: f64 = 40.0;
/// Banner horizontal padding, logical px (`.conn-banner.show{padding:10px 16px}`).
const BANNER_PAD_X: f64 = 16.0;
/// Banner text size, logical px (`.conn-banner{font-size:11px}`).
const BANNER_FONT_SIZE: f32 = 11.0;
/// Banner warning-tint background alpha (`--warning-faint:rgba(...,0.15)`).
const BANNER_WARNING_BG_ALPHA: f32 = 0.15;
/// Banner success-tint background alpha (`--success-faint:rgba(...,0.12)`).
const BANNER_SUCCESS_BG_ALPHA: f32 = 0.12;
/// Banner bottom-border alpha, both variants (`--warning-border`/the
/// `rgba(95,216,143,0.35)` success border-color).
const BANNER_BORDER_ALPHA: f32 = 0.35;
/// Banner open/close animation duration (`max-height .32s` spring;
/// `--ease-spring` collapses to linear under `reduce_motion`).
const BANNER_DURATION: Duration = Duration::from_millis(320);
/// The HTML's `--ease-spring` (`cubic-bezier(0.34,1.4,0.55,1)`), the banner's
/// open/close curve.
const BANNER_CURVE: Curve = Curve::Cubic(0.34, 1.4, 0.55, 1.0);

// ---- Unthemed fallback constants (Glyph **dark** values) -------------------

/// Bar surface — `--bg-surface` / themed `surface_container`.
const FALLBACK_SURFACE: Color = Color::from_rgb8(0x16, 0x1a, 0x23);
/// Title ink — `--fg` / themed `on_surface`.
const FALLBACK_TITLE_INK: Color = Color::from_rgb8(0xf2, 0xea, 0xd9);
/// Subtitle ink — `--fg-muted` / themed `on_surface_variant`.
const FALLBACK_SUBTITLE_INK: Color = Color::from_rgb8(0xa3, 0x9c, 0x88);
/// Elevated border — `--border-bright` / themed `outline`.
const FALLBACK_BORDER: Color = Color::from_rgb8(0x3e, 0x3f, 0x44);
/// Accent ink (count / close) — `--amber` / themed `primary`.
const FALLBACK_ACCENT: Color = Color::from_rgb8(0xff, 0xb6, 0x27);
/// Selection wash fill — `--amber` fill / themed `primary_container`.
const FALLBACK_WASH: Color = Color::from_rgb8(0xff, 0xb6, 0x27);
/// Elevated shadow color base — black (themed `scheme.shadow`).
const FALLBACK_SHADOW: Color = Color::from_rgb8(0x00, 0x00, 0x00);
/// Banner warning ink — `--warning` / themed `StatusPalette::warning`.
const FALLBACK_BANNER_WARNING: Color = Color::from_rgb8(0xf5, 0xc8, 0x60);
/// Banner success ink — `--success` / themed `StatusPalette::success`.
const FALLBACK_BANNER_SUCCESS: Color = Color::from_rgb8(0x5f, 0xd8, 0x8f);

/// The resolved bar color set.
struct BarColors {
    surface: Color,
    title_ink: Color,
    subtitle_ink: Color,
    border: Color,
    accent: Color,
    wash: Color,
    shadow: Color,
}

/// Resolve every bar color from the theme, falling back to the literal Glyph
/// **dark** constants with no theme threaded (the [`super::navbar`]
/// `resolve_nav_colors` precedent).
fn resolve_colors(theme: Option<&Theme>) -> BarColors {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            BarColors {
                surface: scheme.surface_container,
                title_ink: scheme.on_surface,
                subtitle_ink: scheme.on_surface_variant,
                border: scheme.outline,
                accent: scheme.primary,
                wash: scheme.primary_container,
                shadow: scheme.shadow,
            }
        }
        None => BarColors {
            surface: FALLBACK_SURFACE,
            title_ink: FALLBACK_TITLE_INK,
            subtitle_ink: FALLBACK_SUBTITLE_INK,
            border: FALLBACK_BORDER,
            accent: FALLBACK_ACCENT,
            wash: FALLBACK_WASH,
            shadow: FALLBACK_SHADOW,
        },
    }
}

/// The resolved `(background, border, ink)` triple for a connection banner
/// variant. Success/Warning resolve `StatusPalette` first (the badge
/// Success/Warning precedent — extension-first, Glyph-dark constant fallback);
/// the tint/border are the ink at [`BANNER_WARNING_BG_ALPHA`]/
/// [`BANNER_SUCCESS_BG_ALPHA`] and [`BANNER_BORDER_ALPHA`] respectively.
fn resolve_banner_colors(theme: Option<&Theme>, variant: BannerVariant) -> (Color, Color, Color) {
    let ink = match theme {
        Some(theme) => match theme.extension::<frust::StatusPalette>() {
            Some(status) => {
                let c = status.colors(theme.brightness);
                match variant {
                    BannerVariant::Warning => c.warning,
                    BannerVariant::Success => c.success,
                }
            }
            None => match variant {
                BannerVariant::Warning => FALLBACK_BANNER_WARNING,
                BannerVariant::Success => FALLBACK_BANNER_SUCCESS,
            },
        },
        None => match variant {
            BannerVariant::Warning => FALLBACK_BANNER_WARNING,
            BannerVariant::Success => FALLBACK_BANNER_SUCCESS,
        },
    };
    let bg_alpha = match variant {
        BannerVariant::Warning => BANNER_WARNING_BG_ALPHA,
        BannerVariant::Success => BANNER_SUCCESS_BG_ALPHA,
    };
    (
        with_alpha(ink, bg_alpha),
        with_alpha(ink, BANNER_BORDER_ALPHA),
        ink,
    )
}

/// Replace `color`'s alpha channel with `alpha` (the per-module helper shape
/// used across `frust-widgets`).
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// The Glyph `body` (IBM Plex Mono) font stack: the unthemed family of every
/// run in this bar but the big title.
fn mono_family() -> FontFamily {
    FontFamily::stack_with_generic(["IBM Plex Mono"], GenericSlot::Monospace)
}

/// The Glyph `display` (Space Mono) font stack: the large-variant big title's
/// unthemed family (`.ab-big-title{font-family:var(--font-display)}`).
fn display_family() -> FontFamily {
    FontFamily::stack_with_generic(["Space Mono"], GenericSlot::Monospace)
}

/// Every run's family, resolved from the live type scale at layout (see the
/// [module docs](self)' Typeface section).
struct BarFamilies {
    /// The compact title: `titleMedium`.
    title: FontFamily,
    /// The subtitle: `bodySmall`.
    subtitle: FontFamily,
    /// The selection count, which replaces the title: `titleMedium`.
    count: FontFamily,
    /// The large variant's big title: `headlineSmall`.
    big_title: FontFamily,
    /// The connection banner: `bodySmall`.
    banner: FontFamily,
}

/// Resolve [`BarFamilies`] from the theme, falling back to the Glyph stacks
/// with no theme threaded.
fn resolve_families(theme: Option<&Theme>) -> BarFamilies {
    match theme {
        Some(theme) => {
            let scale = &theme.type_scale;
            BarFamilies {
                title: scale.title_medium.family.clone(),
                subtitle: scale.body_small.family.clone(),
                count: scale.title_medium.family.clone(),
                big_title: scale.headline_small.family.clone(),
                banner: scale.body_small.family.clone(),
            }
        }
        None => BarFamilies {
            title: mono_family(),
            subtitle: mono_family(),
            count: mono_family(),
            big_title: display_family(),
            banner: mono_family(),
        },
    }
}

/// The big-title style at the collapse-interpolated `size` (bold).
fn big_title_style(family: &FontFamily, size: f32, color: Color) -> TextStyle {
    TextStyle {
        family: family.clone(),
        weight: FontWeight::BOLD,
        ..TextStyle::new(size, color)
    }
}

/// The connection-banner text style (11px).
fn banner_style(family: &FontFamily, color: Color) -> TextStyle {
    TextStyle {
        family: family.clone(),
        weight: FontWeight::REGULAR,
        ..TextStyle::new(BANNER_FONT_SIZE, color)
    }
}

fn title_style(family: &FontFamily, color: Color) -> TextStyle {
    TextStyle {
        family: family.clone(),
        weight: FontWeight::MEDIUM,
        ..TextStyle::new(TITLE_SIZE, color)
    }
}

fn subtitle_style(family: &FontFamily, color: Color) -> TextStyle {
    TextStyle {
        family: family.clone(),
        weight: FontWeight::REGULAR,
        ..TextStyle::new(SUBTITLE_SIZE, color)
    }
}

fn count_style(family: &FontFamily, color: Color) -> TextStyle {
    TextStyle {
        family: family.clone(),
        weight: FontWeight::SEMI_BOLD,
        ..TextStyle::new(COUNT_SIZE, color)
    }
}

/// The `(enter, _)` [`Timing`] for the title crossfade, honoring
/// `reduce_motion` (a short linear crossfade) — mirrors the toast/dialog
/// `resolve_timings` shape.
fn title_timing(reduce_motion: bool) -> Timing {
    if reduce_motion {
        Timing::Duration(REDUCE_MOTION_DURATION, Curve::Linear)
    } else {
        Timing::Duration(TITLE_DURATION, TITLE_CURVE)
    }
}

/// Direction the title crossfade shifts on a title change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TitleDirection {
    /// The new title slides in from the right (default, forward navigation).
    #[default]
    Forward,
    /// The new title slides in from the left (back navigation).
    Back,
}

/// Horizontal anchor for the compact bar's title (+ subtitle, which always
/// shares the title's anchor — they move together) within the bar's
/// leading/title/trailing three-zone anatomy (see the [module docs](self)).
/// Set via [`AppBarView::title_position`]. Applies to the **compact title
/// face only** — selection mode's `"N selected"` count always anchors
/// leading-aligned regardless of this setting (its own face, not covered by
/// this knob), and the large scroll-collapse variant's big title has no
/// anchor knob of its own either.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TitlePosition {
    /// Flush against the leading zone (the pre-existing, only-ever behavior
    /// — the default, so every existing consumer's layout is byte-identical).
    #[default]
    Leading,
    /// Optically centered over the bar's full width (leading edge to
    /// trailing edge — the standard mobile convention), **clamped to never
    /// overlap the leading or trailing slot's own box**: if centering over
    /// the full width would collide with either slot, the title instead
    /// centers within the free span between them. See
    /// [`AppBarWidget::resolve_title_x`] for the exact rule.
    Center,
}

/// A minimal retained shaped text run — see [`super::navbar`]'s `GlyphLabel`.
struct ShapedRun {
    content: String,
    layout: Option<TextLayout>,
    laid_style: Option<TextStyle>,
    laid_max_width: Option<f32>,
}

impl ShapedRun {
    fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            layout: None,
            laid_style: None,
            laid_max_width: None,
        }
    }

    fn layout(&mut self, ctx: &mut LayoutCtx, style: &TextStyle, max_width: Option<f32>) -> Size {
        if let Some(cached) = &self.layout
            && self.laid_max_width == max_width
            && self.laid_style.as_ref() == Some(style)
        {
            return cached.size();
        }
        let text_ctx = ctx.text_context::<TextContext>();
        let laid = text_ctx.layout(&self.content, style, max_width);
        let size = laid.size();
        self.layout = Some(laid);
        self.laid_style = Some(style.clone());
        self.laid_max_width = max_width;
        size
    }

    fn size(&self) -> Size {
        self.layout.as_ref().map(|l| l.size()).unwrap_or(Size::ZERO)
    }

    fn paint(&self, origin: Point, scene: &mut dyn PaintScene) {
        if let Some(layout) = &self.layout {
            for run in layout.to_scene_runs(origin) {
                scene.draw_glyph_run(run);
            }
        }
    }
}

/// A staged title crossfade: the outgoing run plus its progress driver.
struct TitleStage {
    old: ShapedRun,
    driver: Option<TransitionDriver>,
    /// The last-advanced `0.0..=1.0` progress (`0.0` = old fully shown).
    progress: f64,
}

/// A view-held selection callback (`Fn(&mut State)`).
type OnClose<State> = Rc<dyn Fn(&mut State)>;

/// The selection-mode configuration: a count, a close callback, and the
/// bulk-action views that replace the normal trailing actions.
pub struct SelectionBar<State: 'static> {
    count: usize,
    on_close: OnClose<State>,
    actions: Vec<AnyView<State>>,
}

/// Enter selection mode with `count` selected items and an `on_close` callback
/// (fired when the `×`-close is tapped). Attach bulk actions with
/// [`SelectionBar::actions`].
pub fn selection_bar<State: 'static, F: Fn(&mut State) + 'static>(
    count: usize,
    on_close: F,
) -> SelectionBar<State> {
    SelectionBar {
        count,
        on_close: Rc::new(on_close),
        actions: Vec::new(),
    }
}

impl<State: 'static> SelectionBar<State> {
    /// Set the trailing bulk-action views (in reading order — the last sits
    /// closest to the trailing edge).
    pub fn actions(mut self, actions: Vec<AnyView<State>>) -> Self {
        self.actions = actions;
        self
    }
}

/// The `"N selected"` count string for a selection face.
fn count_text(count: usize) -> String {
    format!("{count} selected")
}

/// Which tint a [`BannerSpec`] paints: the connection-loss warning strip
/// or the `recovered` success flash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BannerVariant {
    /// The connection-loss warning strip (`--warning` tint).
    Warning,
    /// The reconnect-success flash (`--success` tint, the HTML's `recovered`
    /// state).
    Success,
}

/// A connection-loss banner strip: a caller-supplied `text` line under a
/// `variant` tint. The countdown/text content is app-driven (re-rendered per
/// rebuild); auto-dismiss timing is app-side, never baked in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BannerSpec {
    text: String,
    variant: BannerVariant,
}

/// Create a [`BannerSpec`] with `text` under the given `variant` tint.
pub fn banner_spec(text: impl Into<String>, variant: BannerVariant) -> BannerSpec {
    BannerSpec {
        text: text.into(),
        variant,
    }
}

/// The large scroll-collapse configuration: a `big_title` shown at 19px
/// display font over a caller-supplied `meta` row (heartbeat/latency
/// compositions come from the app). Collapse is driven by
/// [`AppBarView::collapse_progress`], not this struct.
pub struct LargeConfig<State: 'static> {
    big_title: String,
    meta: AnyView<State>,
}

/// Create a [`LargeConfig`] with `big_title` over the app-supplied `meta` row.
pub fn large_config<State: 'static>(
    big_title: impl Into<String>,
    meta: AnyView<State>,
) -> LargeConfig<State> {
    LargeConfig {
        big_title: big_title.into(),
        meta,
    }
}

/// A declarative Glyph top AppBar. See the [module docs](self).
pub struct AppBarView<State: 'static> {
    title: String,
    subtitle: Option<String>,
    leading: Option<AnyView<State>>,
    actions: Vec<AnyView<State>>,
    elevated: bool,
    corner_shift: bool,
    title_direction: TitleDirection,
    title_position: TitlePosition,
    selection: Option<SelectionBar<State>>,
    large: Option<LargeConfig<State>>,
    collapse_progress: f64,
    banner: Option<BannerSpec>,
}

/// Create a Glyph AppBar titled `title`, with no leading slot, actions,
/// subtitle, elevation, or selection (attach them with the builder methods).
pub fn app_bar<State: 'static>(title: impl Into<String>) -> AppBarView<State> {
    AppBarView {
        title: title.into(),
        subtitle: None,
        leading: None,
        actions: Vec::new(),
        elevated: false,
        corner_shift: true,
        title_direction: TitleDirection::Forward,
        title_position: TitlePosition::Leading,
        selection: None,
        large: None,
        collapse_progress: 0.0,
        banner: None,
    }
}

/// PascalCase alias for [`app_bar`], matching the widget-fn vocabulary.
#[allow(non_snake_case)]
pub fn AppBar<State: 'static>(title: impl Into<String>) -> AppBarView<State> {
    app_bar(title)
}

impl<State: 'static> AppBarView<State> {
    /// Set the optional subtitle row (10px, `on_surface_variant`), shown beneath
    /// the title in the normal (non-selection) face.
    pub fn subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    /// Set the leading slot (a back arrow, brand mark, or any app composition),
    /// erased as an [`AnyView`]. Its tint is the supplied view's responsibility.
    pub fn leading(mut self, leading: AnyView<State>) -> Self {
        self.leading = Some(leading);
        self
    }

    /// Set the trailing action slots (0–2, in reading order — the last sits
    /// closest to the trailing edge).
    pub fn actions(mut self, actions: Vec<AnyView<State>>) -> Self {
        self.actions = actions;
        self
    }

    /// Set the scrolled-elevation flag (the app feeds this off its own
    /// `on_scroll`; see the [module docs](self)).
    pub fn elevated(mut self, elevated: bool) -> Self {
        self.elevated = elevated;
        self
    }

    /// Whether the bar moves its leading/trailing slots out from under the
    /// iPadOS 26+ window control (`WindowInsets::corner_insets`). Default on;
    /// pass `false` for a bar that does NOT span the window's top edge (a bar in
    /// a detail pane of a split view, in a sheet, a dialog or below other
    /// content): layout cannot see the bar's window-space position and the
    /// corner value is never consumed, so such a bar would otherwise shift for a
    /// control it does not sit under.
    #[must_use]
    pub fn corner_shift(mut self, enabled: bool) -> Self {
        self.corner_shift = enabled;
        self
    }

    /// Set the title-crossfade direction for the next title change.
    pub fn title_direction(mut self, direction: TitleDirection) -> Self {
        self.title_direction = direction;
        self
    }

    /// Set the compact title's horizontal anchor — leading-aligned (the
    /// default) or optically centered over the bar's full width. See
    /// [`TitlePosition`] for the collision-clamp rule and its
    /// selection-mode/large-variant carve-outs.
    pub fn title_position(mut self, position: TitlePosition) -> Self {
        self.title_position = position;
        self
    }

    /// Enter/leave selection mode. `Some` morphs the bar into its
    /// selection face; `None` (the default) is the normal face.
    pub fn selection(mut self, selection: Option<SelectionBar<State>>) -> Self {
        self.selection = selection;
        self
    }

    /// Enable the large scroll-collapse variant: a big display-font title
    /// over an app-supplied meta row. Drive its collapse with
    /// [`Self::collapse_progress`]. Selection mode takes precedence — a
    /// bar that is both `large` and in `selection` renders the compact
    /// selection face.
    pub fn large(mut self, config: LargeConfig<State>) -> Self {
        self.large = Some(config);
        self
    }

    /// Set the large variant's collapse progress `0.0..=1.0`. The app
    /// computes this from its own scroll offset (`min(1, offset/60)` in the
    /// research HTML); the bar interpolates its padding, big-title size, and
    /// meta-row height/fade from it. No-op unless [`Self::large`] is set.
    pub fn collapse_progress(mut self, progress: f64) -> Self {
        self.collapse_progress = progress.clamp(0.0, 1.0);
        self
    }

    /// Attach (or clear) the connection-loss banner strip beneath the bar.
    /// `Some` animates the strip open across rebuilds; `None` animates it
    /// closed. The countdown/text content is app-driven; auto-dismiss timing is
    /// app-side, never baked in.
    pub fn banner(mut self, banner: Option<BannerSpec>) -> Self {
        self.banner = banner;
        self
    }
}

/// Collect the current face's interactive child views into one ordered slice —
/// the normal face's `leading? ++ actions`, or the selection face's bulk
/// actions (its close button is widget-owned, not a child view).
fn face_views<State: 'static>(view: &AppBarView<State>) -> Vec<&AnyView<State>> {
    match &view.selection {
        Some(sel) => sel.actions.iter().collect(),
        None => {
            let mut views = Vec::with_capacity(1 + view.actions.len());
            if let Some(leading) = &view.leading {
                views.push(leading);
            }
            views.extend(view.actions.iter());
            views
        }
    }
}

/// The retained widget for an [`AppBarView`]. See the [module docs](self).
pub struct AppBarWidget {
    title: ShapedRun,
    title_text: String,
    subtitle: Option<ShapedRun>,
    subtitle_text: Option<String>,
    count: ShapedRun,
    count_value: usize,
    /// The current face's interactive children (normal: `leading? ++ actions`;
    /// selection: bulk actions). Reconciled per-face — a face swap rebuilds it.
    interactive: Vec<ChildPod>,
    /// Whether `interactive[0]` is the normal-face leading slot.
    has_leading: bool,
    selection_present: bool,
    on_close: Option<frust::authoring::ErasedCallback>,
    title_direction: TitleDirection,
    /// Compact-face title/subtitle horizontal anchor — see
    /// [`AppBarWidget::resolve_title_x`]. Ignored by the selection face
    /// (`count_pos` always anchors leading) and the large variant (no
    /// compact title zone).
    title_position: TitlePosition,
    elevated_target: bool,
    // --- large variant ---
    /// Whether the current view carries a [`LargeConfig`] (its layout is only
    /// *active* when not also in selection mode — see `large_active`).
    large_present: bool,
    big_title: ShapedRun,
    big_title_text: String,
    /// The app-supplied meta row (heartbeat/latency), present iff `large_present`.
    meta: Option<ChildPod>,
    /// Input-driven collapse progress `0.0..=1.0` (not animated internally).
    collapse_progress: f64,
    // --- connection banner ---
    banner_present: bool,
    banner_run: ShapedRun,
    banner_text: Option<String>,
    banner_variant: BannerVariant,
    // --- animation state (advanced from `PaintCtx::frame_time`) ---
    elev_progress: f64,
    sel_progress: f64,
    /// Banner open/close progress (`0.0` = closed, `1.0` = fully open).
    banner_progress: f64,
    /// Whether the banner's open/close animation was still in flight at the last
    /// `advance` (drives `request_layout`, since banner height is layout-bound).
    banner_animating: bool,
    title_stage: Option<TitleStage>,
    last_time: Option<FrameTime>,
    // --- layout-cached geometry (local coordinates) ---
    top_inset: f64,
    /// Leading/trailing slot shifts from the window-control corners (zero when
    /// a corner has no height); set each layout pass.
    corner_shift: (f64, f64),
    /// Whether the corner shift is applied (`AppBarView::corner_shift`).
    corner_shift_enabled: bool,
    total_size: Size,
    /// The bar's own height (inset + content, excluding any banner strip). The
    /// surface/shadow/elevation border paint to this, so the banner area below
    /// stays uncovered.
    bar_height: f64,
    title_pos: Point,
    subtitle_pos: Point,
    count_pos: Point,
    /// Big-title origin + interpolated font size, large variant (local coords).
    big_title_pos: Point,
    big_title_size_px: f32,
    /// Meta-row origin + fade alpha, large variant (local coords).
    meta_pos: Point,
    meta_alpha: f32,
    /// The banner strip's box, in widget-local coordinates (height reflects the
    /// last-laid `banner_progress`).
    banner_rect: Rect,
    banner_text_pos: Point,
    /// The selection close button's hit box, in widget-local coordinates.
    close_rect: Rect,
    close_pressed: bool,
    close_captured: bool,
}

impl<State: 'static> View<State> for AppBarView<State> {
    type Element = AppBarWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> AppBarWidget {
        let interactive = face_views(self)
            .iter()
            .map(|v| frust::authoring::build_child(v, ctx))
            .collect();
        let selection_present = self.selection.is_some();
        let large_present = self.large.is_some();
        let meta = self
            .large
            .as_ref()
            .map(|l| frust::authoring::build_child(&l.meta, ctx));
        let banner_present = self.banner.is_some();
        AppBarWidget {
            title: ShapedRun::new(self.title.clone()),
            title_text: self.title.clone(),
            subtitle: self.subtitle.as_ref().map(ShapedRun::new),
            subtitle_text: self.subtitle.clone(),
            count: ShapedRun::new(count_text(self.selection.as_ref().map_or(0, |s| s.count))),
            count_value: self.selection.as_ref().map_or(0, |s| s.count),
            interactive,
            has_leading: !selection_present && self.leading.is_some(),
            selection_present,
            on_close: self
                .selection
                .as_ref()
                .map(|s| frust::authoring::erase_callback(&s.on_close)),
            title_direction: self.title_direction,
            title_position: self.title_position,
            elevated_target: self.elevated,
            large_present,
            big_title: ShapedRun::new(
                self.large
                    .as_ref()
                    .map_or(String::new(), |l| l.big_title.clone()),
            ),
            big_title_text: self
                .large
                .as_ref()
                .map_or(String::new(), |l| l.big_title.clone()),
            meta,
            collapse_progress: self.collapse_progress,
            banner_present,
            banner_run: ShapedRun::new(
                self.banner
                    .as_ref()
                    .map_or(String::new(), |b| b.text.clone()),
            ),
            banner_text: self.banner.as_ref().map(|b| b.text.clone()),
            banner_variant: self
                .banner
                .as_ref()
                .map_or(BannerVariant::Warning, |b| b.variant),
            elev_progress: if self.elevated { 1.0 } else { 0.0 },
            sel_progress: if selection_present { 1.0 } else { 0.0 },
            banner_progress: if banner_present { 1.0 } else { 0.0 },
            banner_animating: false,
            title_stage: None,
            last_time: None,
            top_inset: 0.0,
            corner_shift: (0.0, 0.0),
            corner_shift_enabled: self.corner_shift,
            total_size: Size::ZERO,
            bar_height: 0.0,
            title_pos: Point::ZERO,
            subtitle_pos: Point::ZERO,
            count_pos: Point::ZERO,
            big_title_pos: Point::ZERO,
            big_title_size_px: BIG_TITLE_SIZE_MAX,
            meta_pos: Point::ZERO,
            meta_alpha: 1.0,
            banner_rect: Rect::ZERO,
            banner_text_pos: Point::ZERO,
            close_rect: Rect::ZERO,
            close_pressed: false,
            close_captured: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AppBarWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        element.title_direction = self.title_direction;
        if prev.title_position != self.title_position {
            element.title_position = self.title_position;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        // Title change → stage a directional crossfade.
        if prev.title != self.title {
            element.title_stage = Some(TitleStage {
                old: ShapedRun::new(prev.title.clone()),
                driver: None,
                progress: 0.0,
            });
            element.title = ShapedRun::new(self.title.clone());
            element.title_text = self.title.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        // Subtitle reconcile.
        if prev.subtitle != self.subtitle {
            element.subtitle = self.subtitle.as_ref().map(ShapedRun::new);
            element.subtitle_text = self.subtitle.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        // Selection count reconcile.
        let count = self.selection.as_ref().map_or(0, |s| s.count);
        if prev.selection.as_ref().map_or(0, |s| s.count) != count {
            element.count = ShapedRun::new(count_text(count));
            element.count_value = count;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        // Elevation target.
        if prev.elevated != self.elevated {
            element.elevated_target = self.elevated;
            flags |= ChangeFlags::PAINT;
        }

        if prev.corner_shift != self.corner_shift {
            element.corner_shift_enabled = self.corner_shift;
            flags |= ChangeFlags::LAYOUT;
        }

        // Face reconcile. A selection-presence toggle is a full face swap: tear
        // down the old face's children and build the new face's set (the two
        // faces are disjoint view sets, so positional reconciliation would
        // mismatch). Clear any in-flight close-button capture at the same time.
        let selection_present = self.selection.is_some();
        if prev.selection.is_some() != selection_present {
            for (view, pod) in face_views(prev)
                .into_iter()
                .zip(element.interactive.iter_mut())
            {
                frust::authoring::teardown_child(view, pod, ctx);
            }
            element.interactive = face_views(self)
                .iter()
                .map(|v| frust::authoring::build_child(v, ctx))
                .collect();
            element.selection_present = selection_present;
            element.has_leading = !selection_present && self.leading.is_some();
            element.close_pressed = false;
            element.close_captured = false;
            element.on_close = self
                .selection
                .as_ref()
                .map(|s| frust::authoring::erase_callback(&s.on_close));
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else {
            // Same face: reconcile children in place, refresh the close adapter.
            let prev_views = face_views(prev);
            let next_views = face_views(self);
            flags |= frust::authoring::rebuild_children(
                &prev_views,
                &next_views,
                &mut element.interactive,
                ctx,
                |v: &&AnyView<State>| *v,
                |_| None,
            );
            if !selection_present && element.has_leading != self.leading.is_some() {
                element.has_leading = self.leading.is_some();
                flags |= ChangeFlags::LAYOUT;
            }
            element.on_close = self
                .selection
                .as_ref()
                .map(|s| frust::authoring::erase_callback(&s.on_close));
        }

        // Large variant reconcile. Collapse is input-driven — a progress
        // change relayouts via dirty flags (no internal animation controller),
        // so mark LAYOUT here rather than driving a paint-time timeline.
        element.collapse_progress = self.collapse_progress;
        if prev.collapse_progress != self.collapse_progress {
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        match (&prev.large, &self.large) {
            (Some(prev_l), Some(next_l)) => {
                if prev_l.big_title != next_l.big_title {
                    element.big_title = ShapedRun::new(next_l.big_title.clone());
                    element.big_title_text = next_l.big_title.clone();
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
                if let Some(pod) = element.meta.as_mut() {
                    flags |= frust::authoring::rebuild_child(&prev_l.meta, &next_l.meta, pod, ctx);
                }
            }
            (None, Some(next_l)) => {
                element.big_title = ShapedRun::new(next_l.big_title.clone());
                element.big_title_text = next_l.big_title.clone();
                element.meta = Some(frust::authoring::build_child(&next_l.meta, ctx));
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            (Some(prev_l), None) => {
                if let Some(pod) = element.meta.as_mut() {
                    frust::authoring::teardown_child(&prev_l.meta, pod, ctx);
                }
                element.meta = None;
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            (None, None) => {}
        }
        element.large_present = self.large.is_some();

        // Connection banner reconcile. A spec change (presence or
        // text/variant) restages the show/hide animation; banner height is
        // layout-bound, so its in-flight animation calls `request_layout` from
        // paint (see `AppBarWidget::advance`/`paint`). Clearing to `None` keeps
        // the last run/variant so the close animation fades the content out.
        if prev.banner != self.banner {
            element.banner_present = self.banner.is_some();
            if let Some(b) = &self.banner {
                element.banner_variant = b.variant;
                element.banner_text = Some(b.text.clone());
                element.banner_run = ShapedRun::new(b.text.clone());
            }
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        flags
    }

    fn teardown(&self, element: &mut AppBarWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in face_views(self)
            .into_iter()
            .zip(element.interactive.iter_mut())
        {
            frust::authoring::teardown_child(view, pod, ctx);
        }
        if let (Some(large), Some(pod)) = (&self.large, element.meta.as_mut()) {
            frust::authoring::teardown_child(&large.meta, pod, ctx);
        }
    }
}

/// Move `cur` linearly toward `target` by `dt / dur`, returning whether it is
/// still short of the target afterward. The elevation/selection morphs use this
/// (a reversible retarget the bounded `TransitionDriver` can't express); the
/// title crossfade uses a real driver instead.
fn step(cur: &mut f64, target: f64, dt: Duration, dur: Duration) -> bool {
    if (*cur - target).abs() < 1e-6 {
        *cur = target;
        return false;
    }
    let delta = dt.as_secs_f64() / dur.as_secs_f64();
    if *cur < target {
        *cur = (*cur + delta).min(target);
    } else {
        *cur = (*cur - delta).max(target);
    }
    (*cur - target).abs() >= 1e-6
}

impl AppBarWidget {
    /// Advance every animation to frame time `now`, returning whether any is
    /// still in flight (the caller requests another frame). Factored out of
    /// `paint` so the timelines are drivable with synthetic [`FrameTime`]s in a
    /// unit test (the toast/dialog `advance` precedent).
    fn advance(&mut self, now: FrameTime, reduce_motion: bool) -> bool {
        let dt = self
            .last_time
            .map(|t| now.saturating_sub(t))
            .unwrap_or(Duration::ZERO);
        self.last_time = Some(now);

        let mut animating = false;
        let elev_target = if self.elevated_target { 1.0 } else { 0.0 };
        let sel_target = if self.selection_present { 1.0 } else { 0.0 };
        let banner_target = if self.banner_present { 1.0 } else { 0.0 };
        if reduce_motion {
            self.elev_progress = elev_target;
            self.sel_progress = sel_target;
            self.banner_progress = banner_target;
            self.banner_animating = false;
        } else {
            animating |= step(&mut self.elev_progress, elev_target, dt, ELEVATION_DURATION);
            animating |= step(&mut self.sel_progress, sel_target, dt, SELECTION_DURATION);
            // The banner is height-animated (layout-bound): track its in-flight
            // state separately so `paint` can `request_layout` rather than a
            // plain `request_frame`. `BANNER_CURVE`'s spring overshoot is folded
            // into the eased height/fade at paint time, not the linear driver
            // here (the driver stays monotone so height never rewinds).
            self.banner_animating = step(
                &mut self.banner_progress,
                banner_target,
                dt,
                BANNER_DURATION,
            );
            animating |= self.banner_animating;
        }

        if let Some(stage) = &mut self.title_stage {
            let adv = stage
                .driver
                .get_or_insert_with(|| make_driver(title_timing(reduce_motion)).0)
                .advance(now);
            stage.progress = adv.value.clamp(0.0, 1.0);
            if adv.done {
                self.title_stage = None;
            } else {
                animating = true;
            }
        }
        animating
    }

    /// The `(new_dx, old_dx)` title-crossfade x-offsets for progress `p` under
    /// the widget's [`TitleDirection`]: the new run slides in, the old slides
    /// out the opposite way.
    fn title_shift(&self, p: f64) -> (f64, f64) {
        let sign = match self.title_direction {
            TitleDirection::Forward => 1.0,
            TitleDirection::Back => -1.0,
        };
        let new_dx = sign * (1.0 - p) * TITLE_SHIFT;
        let old_dx = -sign * p * TITLE_SHIFT;
        (new_dx, old_dx)
    }
}

impl Widget for AppBarWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // Resolve every color and family to an owned value up front so the
        // immutable theme borrow ends before the `&mut ctx` child-layout calls
        // below.
        let theme = Theme::from_layout_ctx(ctx);
        let colors = resolve_colors(theme);
        let families = resolve_families(theme);
        let banner_style = banner_style(
            &families.banner,
            resolve_banner_colors(theme, self.banner_variant).2,
        );

        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        let top_inset = ctx.window_insets().padding().top;
        self.top_inset = top_inset;
        self.corner_shift = if self.corner_shift_enabled {
            let corners = ctx.window_insets().corner_insets;
            (
                if corners.top_left.height > 0.0 {
                    corners.top_left.width
                } else {
                    0.0
                },
                if corners.top_right.height > 0.0 {
                    corners.top_right.width
                } else {
                    0.0
                },
            )
        } else {
            (0.0, 0.0)
        };

        // The bar's own height, laid out either as the large scroll-collapse
        // variant or the compact three-zone bar. Selection mode
        // takes precedence over `large`.
        let large_active = self.large_present && !self.selection_present;
        let bar_height = if large_active {
            self.layout_large(ctx, &colors, &families, width, top_inset)
        } else {
            self.layout_compact(ctx, &colors, &families, width, top_inset)
        };
        self.bar_height = bar_height;

        // Connection banner strip beneath the bar. Its height tracks the
        // eased open/close progress — layout-bound, so paint requests relayout
        // while it animates.
        let banner_height = self.layout_banner(ctx, &banner_style, width, bar_height);

        let total = bc.constrain(Size::new(width, bar_height + banner_height));
        self.total_size = total;
        total
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.paint_impl(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        self.event_impl(ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.semantics_impl(ctx);
    }

    frust::authoring::visit_children!(interactive, meta);
}

impl AppBarWidget {
    /// Lay out the compact three-zone bar, returning its total height
    /// (`top_inset + BAR_HEIGHT`). Unchanged from the original bar — factored
    /// out of `Widget::layout` so the large variant can slot in beside it.
    fn layout_compact(
        &mut self,
        ctx: &mut LayoutCtx,
        colors: &BarColors,
        families: &BarFamilies,
        width: f64,
        top_inset: f64,
    ) -> f64 {
        let content_top = top_inset;
        let icon_y = content_top + (BAR_HEIGHT - ICON_SIZE) / 2.0;
        let slot_bc = BoxConstraints::loose(Size::new(f64::INFINITY, BAR_HEIGHT));

        // Leading edge: the selection close button (owned) or the normal-face
        // leading child, whichever this face uses.
        let (shift_l, shift_r) = self.corner_shift;
        let lead_x = PAD_X + shift_l;
        let mut left = lead_x;
        let action_start;
        if self.selection_present {
            self.close_rect =
                Rect::from_origin_size(Point::new(lead_x, icon_y), Size::new(ICON_SIZE, ICON_SIZE));
            left = lead_x + ICON_SIZE + SLOT_GAP;
            action_start = 0;
        } else {
            self.close_rect = Rect::ZERO;
            if self.has_leading {
                let size = self.interactive[0].layout_child(ctx, &slot_bc);
                self.interactive[0].set_origin(Point::new(
                    left,
                    content_top + (BAR_HEIGHT - size.height) / 2.0,
                ));
                left += size.width + SLOT_GAP;
                action_start = 1;
            } else {
                action_start = 0;
            }
        }

        // Trailing actions, laid out right-to-left so the last lands flush
        // against the trailing edge (reading order preserved).
        let mut right = width - PAD_X - shift_r;
        for pod in self.interactive[action_start..].iter_mut().rev() {
            let size = pod.layout_child(ctx, &slot_bc);
            right -= size.width;
            pod.set_origin(Point::new(
                right,
                content_top + (BAR_HEIGHT - size.height) / 2.0,
            ));
            right -= SLOT_GAP;
        }

        // Title zone fills the middle; its runs are ellipsized to the
        // available width (`zone_w`) regardless of `title_position` — only
        // the horizontal anchor within the bar changes. `zone_x` (always
        // leading-aligned) anchors the selection face's count run; the
        // title/subtitle anchor is resolved separately below, since
        // selection mode ignores `title_position` (its own face — see
        // `Self::resolve_title_x`).
        let zone_x = left;
        let zone_w = (right - left).max(0.0);
        let title_style = title_style(&families.title, colors.title_ink);
        let title_size = self.title.layout(ctx, &title_style, Some(zone_w as f32));
        if let Some(stage) = &mut self.title_stage {
            stage.old.layout(ctx, &title_style, Some(zone_w as f32));
        }
        let subtitle_style = subtitle_style(&families.subtitle, colors.subtitle_ink);
        let subtitle_size = self
            .subtitle
            .as_mut()
            .map(|s| s.layout(ctx, &subtitle_style, Some(zone_w as f32)));
        self.count.layout(
            ctx,
            &count_style(&families.count, colors.accent),
            Some(zone_w as f32),
        );

        // Vertically center the title (+subtitle) stack inside the content
        // band; horizontally, resolve the shared title/subtitle anchor per
        // `title_position` — both share `title_x` (they move together).
        let run_width = title_size.width.max(subtitle_size.map_or(0.0, |s| s.width));
        let title_x = self.resolve_title_x(width, left, right, run_width);
        let stack_h = title_size.height + subtitle_size.map_or(0.0, |s| SUBTITLE_GAP + s.height);
        let title_y = content_top + (BAR_HEIGHT - stack_h) / 2.0;
        self.title_pos = Point::new(title_x, title_y);
        self.subtitle_pos = Point::new(title_x, title_y + title_size.height + SUBTITLE_GAP);
        let count_h = self.count.size().height;
        self.count_pos = Point::new(zone_x, content_top + (BAR_HEIGHT - count_h) / 2.0);

        top_inset + BAR_HEIGHT
    }

    /// Resolve the compact title (+subtitle) run's horizontal anchor per
    /// [`TitlePosition`], given the bar's own `width`, the free title zone's
    /// `left`/`right` edges (the leading/trailing slot boxes end/start
    /// there, including their gap), and `run_width` (the wider of the title
    /// and subtitle runs, already ellipsis-constrained to the free span).
    ///
    /// [`TitlePosition::Leading`] always returns `left` — byte-identical to
    /// the bar's original, only-ever layout.
    ///
    /// [`TitlePosition::Center`] optically centers the run over the bar's
    /// **full** width (`0..width`, the standard mobile convention — not just
    /// the free span between the slots). **Collision clamp:** if that
    /// full-width-centered box (`[x, x + run_width]`) would overlap either
    /// slot's own box (i.e. `x < left` or `x + run_width > right`), the run
    /// instead centers within the free span `[left, right]` alone, so it
    /// never overlaps a leading/trailing slot regardless of how much either
    /// occupies.
    fn resolve_title_x(&self, width: f64, left: f64, right: f64, run_width: f64) -> f64 {
        match self.title_position {
            TitlePosition::Leading => left,
            TitlePosition::Center => {
                let full_width_centered = (width - run_width) / 2.0;
                if full_width_centered >= left && full_width_centered + run_width <= right {
                    full_width_centered
                } else {
                    let free_span = (right - left).max(0.0);
                    left + ((free_span - run_width) / 2.0).max(0.0)
                }
            }
        }
    }

    /// Lay out the large scroll-collapse variant, returning its total
    /// height. Geometry interpolates on `collapse_progress`: paddings `6→0` /
    /// `10→0`, big-title size `19→13`, big-title bottom padding `2→0`, meta-row
    /// height `20→0` + fade. Row 1 (leading + trailing actions) reuses the
    /// compact slot geometry; the compact title/subtitle/count are unused here.
    fn layout_large(
        &mut self,
        ctx: &mut LayoutCtx,
        colors: &BarColors,
        families: &BarFamilies,
        width: f64,
        top_inset: f64,
    ) -> f64 {
        let p = self.collapse_progress;
        let pad_top = LARGE_PAD_TOP * (1.0 - p);
        let pad_bottom = LARGE_PAD_BOTTOM * (1.0 - p);
        let row1_top = top_inset + pad_top;

        // Row 1: leading + trailing actions, laid within an ICON_SIZE band. The
        // big title (below) is the title zone, so row 1 carries no compact title.
        self.close_rect = Rect::ZERO;
        let slot_bc = BoxConstraints::loose(Size::new(f64::INFINITY, ICON_SIZE));
        let (shift_l, shift_r) = self.corner_shift;
        let action_start = if self.has_leading {
            let size = self.interactive[0].layout_child(ctx, &slot_bc);
            self.interactive[0].set_origin(Point::new(
                PAD_X + shift_l,
                row1_top + (ICON_SIZE - size.height) / 2.0,
            ));
            1
        } else {
            0
        };
        let mut right = width - PAD_X - shift_r;
        for pod in self.interactive[action_start..].iter_mut().rev() {
            let size = pod.layout_child(ctx, &slot_bc);
            right -= size.width;
            pod.set_origin(Point::new(
                right,
                row1_top + (ICON_SIZE - size.height) / 2.0,
            ));
            right -= SLOT_GAP;
        }
        let row1_bottom = row1_top + ICON_SIZE;

        // Big title (display font, interpolated size).
        let size_px = BIG_TITLE_SIZE_MAX - BIG_TITLE_SIZE_SPAN * p as f32;
        self.big_title_size_px = size_px;
        let bt_max_w = (width - 2.0 * BIG_TITLE_PAD_X).max(0.0);
        let bt_size = self.big_title.layout(
            ctx,
            &big_title_style(&families.big_title, size_px, colors.title_ink),
            Some(bt_max_w as f32),
        );
        let bt_top = row1_bottom + BIG_TITLE_PAD_TOP;
        self.big_title_pos = Point::new(BIG_TITLE_PAD_X, bt_top);
        let bt_pad_bottom = BIG_TITLE_PAD_BOTTOM * (1.0 - p);
        let bt_bottom = bt_top + bt_size.height + bt_pad_bottom;

        // Meta row: collapses `20→0` with a `1-progress` fade.
        self.meta_alpha = (1.0 - p) as f32;
        let meta_h = META_HEIGHT * (1.0 - p);
        self.meta_pos = Point::new(META_PAD_X, bt_bottom);
        if let Some(pod) = self.meta.as_mut() {
            let meta_bc =
                BoxConstraints::loose(Size::new((width - 2.0 * META_PAD_X).max(0.0), META_HEIGHT));
            let msize = pod.layout_child(ctx, &meta_bc);
            pod.set_origin(Point::new(
                META_PAD_X,
                bt_bottom + (meta_h - msize.height).max(0.0) / 2.0,
            ));
        }

        bt_bottom + meta_h + pad_bottom
    }

    /// Lay out the connection banner strip beneath the bar, returning its
    /// eased height (`0` when fully closed and absent). The strip's box and the
    /// centered text position are cached in local coordinates.
    fn layout_banner(
        &mut self,
        ctx: &mut LayoutCtx,
        style: &TextStyle,
        width: f64,
        bar_bottom: f64,
    ) -> f64 {
        if !self.banner_present && self.banner_progress <= 0.0 {
            self.banner_rect = Rect::ZERO;
            return 0.0;
        }
        let eased = BANNER_CURVE.transform(self.banner_progress).max(0.0);
        let banner_h = BANNER_HEIGHT * eased;
        self.banner_rect =
            Rect::from_origin_size(Point::new(0.0, bar_bottom), Size::new(width, banner_h));
        let max_w = (width - 2.0 * BANNER_PAD_X).max(0.0);
        let tsize = self.banner_run.layout(ctx, style, Some(max_w as f32));
        self.banner_text_pos =
            Point::new(BANNER_PAD_X, bar_bottom + (banner_h - tsize.height) / 2.0);
        banner_h
    }

    fn paint_impl(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let reduce_motion = theme.map(|t| t.motion.reduce_motion).unwrap_or(false);
        let colors = resolve_colors(theme);
        // `banner_progress` is layout-bound (`layout_banner` reads it), so a
        // before/after comparison is what actually catches every case that
        // needs a relayout — `self.banner_animating` (set inside `advance`,
        // below) reports whether the hand-rolled `step()` helper was still
        // short of its target, which mirrors `AnimationController::advance`'s
        // own landing-frame gap: `step` reports "done" on the very call that
        // snaps `banner_progress` onto its target, so gating solely on it
        // would leave the last banner height `layout` ever saw a hair short
        // of rest.
        let banner_before = self.banner_progress;
        let animating = self.advance(ctx.frame_time(), reduce_motion);

        let banner_colors = resolve_banner_colors(theme, self.banner_variant);

        let origin = ctx.origin();
        let size = ctx.size();
        // The surface/shadow/elevation border paint to the bar's own height so
        // the banner strip below stays uncovered; `push_layer` bounds still use
        // the full size (they are only alpha-clip bounds).
        let bar_size = Size::new(size.width, self.bar_height);
        let large_active = self.large_present && !self.selection_present;

        // Elevated drop shadow, beneath the surface fill so the surface covers
        // the shadow's near edge.
        if self.elev_progress > 0.0 {
            scene.draw_shadow(
                Point::new(origin.x, origin.y + SHADOW_Y),
                bar_size,
                0.0,
                SHADOW_BLUR,
                with_alpha(colors.shadow, SHADOW_ALPHA * self.elev_progress as f32),
            );
        }

        // Surface: fills the bar's extended height (running under the status
        // bar), plus the selection wash overlay.
        scene.fill_rect(origin, bar_size, colors.surface);
        if self.sel_progress > 0.0 {
            scene.fill_rect(
                origin,
                bar_size,
                with_alpha(colors.wash, SELECTION_WASH_ALPHA * self.sel_progress as f32),
            );
        }

        // Elevated bottom hairline border.
        if self.elev_progress > 0.0 {
            scene.fill_rect(
                Point::new(origin.x, origin.y + bar_size.height - 1.0),
                Size::new(bar_size.width, 1.0),
                with_alpha(colors.border, self.elev_progress as f32),
            );
        }

        if large_active {
            // Large variant: row-1 children (no selection morph fade), the
            // big display title, and the collapse-faded meta row.
            for pod in &mut self.interactive {
                pod.paint_child(ctx, scene);
            }
            self.big_title.paint(
                Point::new(
                    origin.x + self.big_title_pos.x,
                    origin.y + self.big_title_pos.y,
                ),
                scene,
            );
            if self.meta_alpha > 0.0
                && let Some(pod) = self.meta.as_mut()
            {
                let layered = self.meta_alpha < 1.0;
                if layered {
                    scene.push_layer(origin, size, self.meta_alpha);
                }
                pod.paint_child(ctx, scene);
                if layered {
                    scene.pop_layer();
                }
            }
        } else {
            // Compact face.
            // Interactive children (current face), faded by the morph progress.
            let child_alpha = if self.selection_present {
                self.sel_progress
            } else {
                1.0 - self.sel_progress
            } as f32;
            if child_alpha > 0.0 {
                let layered = child_alpha < 1.0;
                if layered {
                    scene.push_layer(origin, size, child_alpha);
                }
                for pod in &mut self.interactive {
                    pod.paint_child(ctx, scene);
                }
                if layered {
                    scene.pop_layer();
                }
            }

            // Selection close button (owned), fading in with the morph.
            if self.sel_progress > 0.0 {
                self.paint_close(origin, colors.accent, self.sel_progress as f32, scene);
            }

            // Normal title-zone content (title + subtitle), fading/sliding out
            // under the selection morph.
            let normal_alpha = (1.0 - self.sel_progress) as f32;
            if normal_alpha > 0.0 {
                let dy = -SELECTION_SHIFT * self.sel_progress;
                scene.push_transform(Affine::translate((0.0, dy)));
                let layered = normal_alpha < 1.0;
                if layered {
                    scene.push_layer(origin, size, normal_alpha);
                }
                self.paint_title(origin, scene);
                if let Some(subtitle) = &self.subtitle {
                    subtitle.paint(
                        Point::new(
                            origin.x + self.subtitle_pos.x,
                            origin.y + self.subtitle_pos.y,
                        ),
                        scene,
                    );
                }
                if layered {
                    scene.pop_layer();
                }
                scene.pop_transform();
            }

            // Selection count content, fading/sliding in.
            if self.sel_progress > 0.0 {
                let dy = SELECTION_SHIFT * (1.0 - self.sel_progress);
                scene.push_transform(Affine::translate((0.0, dy)));
                let alpha = self.sel_progress as f32;
                let layered = alpha < 1.0;
                if layered {
                    scene.push_layer(origin, size, alpha);
                }
                self.count.paint(
                    Point::new(origin.x + self.count_pos.x, origin.y + self.count_pos.y),
                    scene,
                );
                if layered {
                    scene.pop_layer();
                }
                scene.pop_transform();
            }
        }

        // Connection banner strip beneath the bar.
        self.paint_banner(origin, banner_colors, scene);

        // The banner is height-animated (layout-bound): while it animates, ask
        // for a relayout (which implies another frame); otherwise a plain frame
        // request covers the paint-only elevation/selection/title timelines.
        // The before/after comparison also catches `step`'s landing frame
        // (see this function's opening comment) — a case `banner_animating`
        // alone reports `false` for.
        if self.banner_animating || self.banner_progress != banner_before {
            ctx.request_layout();
        } else if animating {
            ctx.request_frame();
        }
    }

    /// Paint the connection banner strip: a `variant`-tinted fill, a bottom
    /// hairline border, and the app-supplied text, all faded together by
    /// `banner_progress`. No-op when the strip is fully closed.
    fn paint_banner(
        &self,
        origin: Point,
        colors: (Color, Color, Color),
        scene: &mut dyn PaintScene,
    ) {
        let h = self.banner_rect.height();
        if h <= 0.0 {
            return;
        }
        let (bg, border, _ink) = colors;
        let alpha = self.banner_progress.clamp(0.0, 1.0) as f32;
        let bo = Point::new(
            origin.x + self.banner_rect.x0,
            origin.y + self.banner_rect.y0,
        );
        let bsize = Size::new(self.banner_rect.width(), h);
        let layered = alpha < 1.0;
        if layered {
            scene.push_layer(bo, bsize, alpha);
        }
        scene.fill_rect(bo, bsize, bg);
        scene.fill_rect(
            Point::new(bo.x, bo.y + h - 1.0),
            Size::new(bsize.width, 1.0),
            border,
        );
        self.banner_run.paint(
            Point::new(
                origin.x + self.banner_text_pos.x,
                origin.y + self.banner_text_pos.y,
            ),
            scene,
        );
        if layered {
            scene.pop_layer();
        }
    }

    fn event_impl(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // The widget-owned selection close button takes precedence over the
        // child action pods (it occupies the leading slot in selection mode).
        if self.selection_present
            && let InputEvent::Pointer(p) = event
        {
            if self.close_captured {
                match p.phase {
                    PointerPhase::Move => {
                        self.close_pressed = self.close_rect.contains(p.position);
                        ctx.request_redraw();
                        return EventResult::Handled;
                    }
                    PointerPhase::Up => {
                        if self.close_rect.contains(p.position)
                            && let Some(on_close) = &mut self.on_close
                        {
                            on_close(ctx);
                        }
                        self.close_pressed = false;
                        self.close_captured = false;
                        ctx.request_redraw();
                        return EventResult::Handled;
                    }
                    PointerPhase::Cancel => {
                        // Cancel never touches state — clear the flags only.
                        self.close_pressed = false;
                        self.close_captured = false;
                        ctx.request_redraw();
                        return EventResult::Handled;
                    }
                    PointerPhase::Down => {}
                }
            } else if p.phase == PointerPhase::Down
                && presses(p)
                && self.close_rect.contains(p.position)
            {
                self.close_pressed = true;
                self.close_captured = true;
                ctx.capture_pointer();
                ctx.request_redraw();
                return EventResult::Handled;
            }
        }
        // Row-1 / face children first, then the large variant's meta row (which
        // may itself hold interactive views).
        let result = frust::authoring::route_event(&mut self.interactive, ctx, event);
        if result == EventResult::Handled {
            return result;
        }
        if let Some(pod) = self.meta.as_mut() {
            let meta_result = frust::authoring::route_event_single(pod, ctx, event);
            if meta_result == EventResult::Handled {
                return meta_result;
            }
        }
        result
    }

    fn semantics_impl(&self, ctx: &mut SemanticsCtx) {
        let label = if self.selection_present {
            count_text(self.count_value)
        } else if self.large_present {
            self.big_title_text.clone()
        } else {
            self.title_text.clone()
        };
        let selection_present = self.selection_present;
        let has_leading = self.has_leading;
        let interactive = &self.interactive;
        let meta = self.meta.as_ref();
        let banner_text = self.banner_text.clone();
        let banner_present = self.banner_present;
        ctx.push_container(
            Role::TitleBar,
            |node| node.set_label(label.as_str()),
            |ctx| {
                if selection_present {
                    // Leading close button (owned) first, then bulk actions.
                    ctx.push_node(Role::Button, |node| node.set_label("Cancel selection"));
                    for pod in interactive {
                        pod.semantics_child(ctx);
                    }
                } else {
                    if has_leading {
                        interactive[0].semantics_child(ctx);
                    }
                    for pod in &interactive[if has_leading { 1 } else { 0 }..] {
                        pod.semantics_child(ctx);
                    }
                    // The large variant's meta row joins the container's children.
                    if let Some(pod) = meta {
                        pod.semantics_child(ctx);
                    }
                }
                // The connection banner text joins the container's children while
                // shown.
                if banner_present && let Some(text) = &banner_text {
                    ctx.push_node(Role::Label, |node| node.set_label(text.as_str()));
                }
            },
        );
    }
}

impl AppBarWidget {
    /// Paint the title zone's normal-face content: either the resting title run
    /// or the staged old/new crossfade pair.
    fn paint_title(&mut self, origin: Point, scene: &mut dyn PaintScene) {
        let base = Point::new(origin.x + self.title_pos.x, origin.y + self.title_pos.y);
        match &self.title_stage {
            Some(stage) => {
                let p = stage.progress;
                let (new_dx, old_dx) = self.title_shift(p);
                let full = self.total_size;
                // Old run fades/slides out.
                scene.push_transform(Affine::translate((old_dx, 0.0)));
                scene.push_layer(origin, full, (1.0 - p) as f32);
                stage.old.paint(base, scene);
                scene.pop_layer();
                scene.pop_transform();
                // New (resting) run fades/slides in.
                scene.push_transform(Affine::translate((new_dx, 0.0)));
                scene.push_layer(origin, full, p as f32);
                self.title.paint(base, scene);
                scene.pop_layer();
                scene.pop_transform();
            }
            None => self.title.paint(base, scene),
        }
    }

    /// Paint the widget-owned selection `×`-close button as two diagonal strokes
    /// centered in [`close_rect`](Self::close_rect), tinted `accent` at `alpha`
    /// (platform-deterministic — no font-fallback dependency for the glyph).
    fn paint_close(&self, origin: Point, accent: Color, alpha: f32, scene: &mut dyn PaintScene) {
        // A 12px arm span centered in the 40px box.
        const ARM: f64 = 6.0;
        const STROKE_W: f64 = 1.5;
        let cx = origin.x + self.close_rect.center().x;
        let cy = origin.y + self.close_rect.center().y;
        let color = with_alpha(accent, alpha);
        let mut a = kurbo::BezPath::new();
        a.move_to((cx - ARM, cy - ARM));
        a.line_to((cx + ARM, cy + ARM));
        let mut b = kurbo::BezPath::new();
        b.move_to((cx + ARM, cy - ARM));
        b.line_to((cx - ARM, cy + ARM));
        scene.stroke_path(Point::ZERO, &a, STROKE_W, &Brush::Solid(color));
        scene.stroke_path(Point::ZERO, &b, STROKE_W, &Brush::Solid(color));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{BuildCtx, PointerButton, PointerEvent, WindowEdgeInsets, WindowInsets};
    use frust_core::RenderRoot;
    use frust_widgets::test_support::leaf_any;
    use std::any::Any;
    use std::cell::Cell;

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn build(view: &AppBarView<()>) -> AppBarWidget {
        let mut counter = 0u64;
        View::<()>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn rebuild(prev: &AppBarView<()>, next: &AppBarView<()>, w: &mut AppBarWidget) -> ChangeFlags {
        let mut counter = 0u64;
        View::<()>::rebuild(next, prev, w, &mut BuildCtx::new(&mut counter))
    }

    /// A layout pass with a real `TextContext` (the title/subtitle/count are
    /// shaped runs, which panic without one) and an optional theme.
    fn layout(w: &mut AppBarWidget, size: Size, theme: Option<&Theme>) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = match theme {
            Some(t) => {
                LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), Some(t as &dyn Any))
            }
            None => LayoutCtx::with_text_context(&mut tcx as &mut dyn Any),
        };
        w.layout(&mut lctx, &BoxConstraints::loose(size))
    }

    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        shadows: usize,
        strokes: usize,
        glyph_runs: usize,
        transforms: usize,
        transform_pops: usize,
        layers: usize,
        layer_pops: usize,
    }
    impl PaintScene for Recorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.rects.push((o, s, c));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn draw_shadow(&mut self, _o: Point, _s: Size, _r: f64, _b: f64, _c: Color) {
            self.shadows += 1;
        }
        fn stroke_path(&mut self, _o: Point, _p: &kurbo::BezPath, _w: f64, _brush: &Brush) {
            self.strokes += 1;
        }
        fn draw_glyph_run(&mut self, _run: frust::authoring::scene::GlyphRun) {
            self.glyph_runs += 1;
        }
        fn push_transform(&mut self, _t: Affine) {
            self.transforms += 1;
        }
        fn pop_transform(&mut self) {
            self.transform_pops += 1;
        }
        fn push_layer(&mut self, _o: Point, _s: Size, _a: f32) {
            self.layers += 1;
        }
        fn pop_layer(&mut self) {
            self.layer_pops += 1;
        }
    }

    fn paint(w: &mut AppBarWidget, now: FrameTime, theme: Option<&Theme>) -> (Recorder, bool) {
        let mut rec = Recorder::default();
        let size = w.total_size;
        let mut pctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, size).with_theme(t),
            None => PaintCtx::new(Point::ZERO, size),
        };
        // The frame time is threaded via `RenderRoot::paint`; a bare `PaintCtx`
        // can't be seeded at an arbitrary time, so drive `advance` directly.
        let reduce = theme.map(|t| t.motion.reduce_motion).unwrap_or(false);
        let animating = w.advance(now, reduce);
        w.paint(&mut pctx, &mut rec);
        (rec, animating)
    }

    /// Like [`paint`] but also reports the `PaintCtx::needs_layout` flag the
    /// widget raised — the banner's layout-bound animation is asserted through
    /// this (a bare `advance` returns `needs_frame`, not `needs_layout`).
    fn paint_nl(
        w: &mut AppBarWidget,
        now: FrameTime,
        theme: Option<&Theme>,
    ) -> (Recorder, bool, bool) {
        let mut rec = Recorder::default();
        let size = w.total_size;
        let mut pctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, size).with_theme(t),
            None => PaintCtx::new(Point::ZERO, size),
        };
        let reduce = theme.map(|t| t.motion.reduce_motion).unwrap_or(false);
        let animating = w.advance(now, reduce);
        w.paint(&mut pctx, &mut rec);
        (rec, animating, pctx.needs_layout())
    }

    // -- Top-inset consumption (Flutter parity) -----------------------------

    #[test]
    fn layout_consumes_top_inset_and_offsets_content_below_it() {
        fn logic(_: &mut ()) -> AppBarView<()> {
            app_bar("Home").leading(leaf_any(40.0, 40.0))
        }
        let mut root: RenderRoot<(), AppBarView<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        root.set_insets(WindowInsets::new(
            WindowEdgeInsets::new(0.0, 24.0, 0.0, 0.0),
            WindowEdgeInsets::ZERO,
        ));
        let mut tcx = TextContext::new();
        let size = root.layout_with_text(Size::new(400.0, 200.0), &mut tcx as &mut dyn Any);
        // 52px bar + 24px top inset = 76px.
        assert_eq!(size.height, BAR_HEIGHT + 24.0);

        let id = root.root_id().expect("root built");
        let w = (root.tree().pod(id).expect("pod").widget() as &dyn Any)
            .downcast_ref::<AppBarWidget>()
            .expect("root is an AppBarWidget");
        assert_eq!(w.top_inset, 24.0);
        // The leading child and the title both sit below the 24px inset.
        assert!(
            w.interactive[0].origin().y >= 24.0,
            "leading below the inset"
        );
        assert!(w.title_pos.y >= 24.0, "title below the inset");
    }

    // -- Window-control corners (iPadOS 26+) ---------------------------------

    use frust::authoring::{CornerInset, CornerInsets};

    /// Lay `view` out through a `RenderRoot` under `insets`, returning the
    /// laid-out size and the root widget for inspection.
    fn laid_out_with_insets(
        view: AppBarView<()>,
        insets: WindowInsets,
        width: f64,
    ) -> (Size, RenderRoot<(), AppBarView<()>>) {
        let mut tcx = TextContext::new();
        laid_out_with_insets_in(view, insets, width, &mut tcx)
    }

    /// Like [`laid_out_with_insets`] but shaping through the caller's
    /// `TextContext`. A byte-identical comparison of two layouts must share one
    /// context: sibling tests in this binary register the Glyph faces
    /// process-wide, and a `TextContext` picks those fonts up only when it is
    /// constructed, so two contexts created on either side of that registration
    /// shape the title with different metrics (observed: title y 38.0 vs 34.55).
    fn laid_out_with_insets_in(
        view: AppBarView<()>,
        insets: WindowInsets,
        width: f64,
        tcx: &mut TextContext,
    ) -> (Size, RenderRoot<(), AppBarView<()>>) {
        let mut view = Some(view);
        let mut logic = move |_: &mut ()| view.take().expect("built once");
        let mut root: RenderRoot<(), AppBarView<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        root.set_insets(insets);
        let size = root.layout_with_text(Size::new(width, 300.0), tcx as &mut dyn Any);
        (size, root)
    }

    fn root_bar(root: &RenderRoot<(), AppBarView<()>>) -> &AppBarWidget {
        let id = root.root_id().expect("root built");
        (root.tree().pod(id).expect("pod").widget() as &dyn Any)
            .downcast_ref::<AppBarWidget>()
            .expect("root is an AppBarWidget")
    }

    fn corners(tl: (f64, f64), tr: (f64, f64)) -> CornerInsets {
        CornerInsets::new(
            CornerInset::new(tl.0, tl.1),
            CornerInset::new(tr.0, tr.1),
            CornerInset::ZERO,
            CornerInset::ZERO,
        )
    }

    fn top_padding(top: f64) -> WindowEdgeInsets {
        WindowEdgeInsets::new(0.0, top, 0.0, 0.0)
    }

    #[test]
    fn corner_shift_moves_leading_and_trailing_slots_out_from_under_the_control() {
        let view: AppBarView<()> = app_bar("Home")
            .leading(leaf_any(40.0, 40.0))
            .actions(vec![leaf_any(40.0, 40.0), leaf_any(40.0, 40.0)]);
        let insets = WindowInsets::new(top_padding(0.0), WindowEdgeInsets::ZERO)
            .with_corner_insets(corners((44.0, 30.0), (52.0, 30.0)));
        let (size, root) = laid_out_with_insets(view, insets, 400.0);
        let w = root_bar(&root);
        assert_eq!(size.height, BAR_HEIGHT);
        assert_eq!(w.interactive[0].origin().x, PAD_X + 44.0);
        let last = w.interactive.last().expect("trailing action");
        let right_edge = last.origin().x + 40.0;
        assert_eq!(right_edge, 400.0 - PAD_X - 52.0);
    }

    #[test]
    fn corner_with_zero_height_shifts_nothing() {
        let view: AppBarView<()> = app_bar("Home").leading(leaf_any(40.0, 40.0));
        let insets = WindowInsets::new(top_padding(0.0), WindowEdgeInsets::ZERO)
            .with_corner_insets(corners((44.0, 0.0), (52.0, 0.0)));
        let (_, root) = laid_out_with_insets(view, insets, 400.0);
        assert_eq!(root_bar(&root).interactive[0].origin().x, PAD_X);
    }

    #[test]
    fn corner_shift_stacks_with_the_top_inset() {
        let view: AppBarView<()> = app_bar("Home").leading(leaf_any(40.0, 40.0));
        let insets = WindowInsets::new(top_padding(24.0), WindowEdgeInsets::ZERO)
            .with_corner_insets(corners((44.0, 30.0), (0.0, 0.0)));
        let (_, root) = laid_out_with_insets(view, insets, 400.0);
        let w = root_bar(&root);
        assert_eq!(w.interactive[0].origin().x, PAD_X + 44.0);
        assert!(w.interactive[0].origin().y >= 24.0);
    }

    #[test]
    fn corner_shift_moves_the_selection_close_button() {
        let view: AppBarView<()> = app_bar("Home").selection(Some(selection_bar(3, |_| {})));
        let insets = WindowInsets::new(top_padding(0.0), WindowEdgeInsets::ZERO)
            .with_corner_insets(corners((44.0, 30.0), (0.0, 0.0)));
        let (_, root) = laid_out_with_insets(view, insets, 400.0);
        assert_eq!(root_bar(&root).close_rect.x0, PAD_X + 44.0);
    }

    #[test]
    fn corner_shift_moves_the_large_variants_row_one() {
        let view: AppBarView<()> = app_bar("host")
            .large(large_config("203.0.113.57", leaf_any(120.0, 12.0)))
            .leading(leaf_any(40.0, 40.0));
        let insets = WindowInsets::new(top_padding(0.0), WindowEdgeInsets::ZERO)
            .with_corner_insets(corners((44.0, 30.0), (0.0, 0.0)));
        let (_, root) = laid_out_with_insets(view, insets, 400.0);
        assert_eq!(root_bar(&root).interactive[0].origin().x, PAD_X + 44.0);
    }

    #[test]
    fn centered_title_stays_centered_on_the_full_width_when_it_fits_beside_shifted_slots() {
        let view: AppBarView<()> = app_bar("Hi").title_position(TitlePosition::Center);
        let insets = WindowInsets::new(top_padding(0.0), WindowEdgeInsets::ZERO)
            .with_corner_insets(corners((44.0, 30.0), (44.0, 30.0)));
        let (_, root) = laid_out_with_insets(view, insets, 600.0);
        let w = root_bar(&root);
        let run_width = w.title.size().width;
        let expected = (600.0 - run_width) / 2.0;
        assert!(
            (w.title_pos.x - expected).abs() < 0.5,
            "expected {expected}, got {}",
            w.title_pos.x
        );
    }

    #[test]
    fn zero_corners_are_byte_identical_to_todays_layout() {
        let build_view = || -> AppBarView<()> {
            app_bar("Home")
                .subtitle("sub")
                .leading(leaf_any(40.0, 40.0))
                .actions(vec![leaf_any(40.0, 40.0), leaf_any(30.0, 30.0)])
        };
        let plain = WindowInsets::new(top_padding(24.0), WindowEdgeInsets::ZERO);
        let zeroed = plain.with_corner_insets(CornerInsets::ZERO);
        let mut tcx = TextContext::new();
        let (sa, ra) = laid_out_with_insets_in(build_view(), plain, 400.0, &mut tcx);
        let (sb, rb) = laid_out_with_insets_in(build_view(), zeroed, 400.0, &mut tcx);
        let (a, b) = (root_bar(&ra), root_bar(&rb));
        assert_eq!(sa, sb);
        assert_eq!(a.interactive.len(), b.interactive.len());
        for (pa, pb) in a.interactive.iter().zip(b.interactive.iter()) {
            assert_eq!(pa.origin(), pb.origin());
            assert_eq!(pa.size(), pb.size());
        }
        assert_eq!(a.close_rect, b.close_rect);
        assert_eq!(a.title_pos, b.title_pos);
    }

    #[test]
    fn corner_shift_opt_out_is_byte_identical_with_nonzero_corners() {
        let build_view = |shift: bool| -> AppBarView<()> {
            app_bar("Home")
                .subtitle("sub")
                .corner_shift(shift)
                .leading(leaf_any(40.0, 40.0))
                .actions(vec![leaf_any(40.0, 40.0), leaf_any(30.0, 30.0)])
        };
        let plain = WindowInsets::new(top_padding(24.0), WindowEdgeInsets::ZERO);
        let cornered = plain.with_corner_insets(corners((44.0, 30.0), (52.0, 30.0)));
        let mut tcx = TextContext::new();
        let (sa, ra) = laid_out_with_insets_in(build_view(true), plain, 400.0, &mut tcx);
        let (sb, rb) = laid_out_with_insets_in(build_view(false), cornered, 400.0, &mut tcx);
        let (a, b) = (root_bar(&ra), root_bar(&rb));
        assert_eq!(sa, sb);
        assert_eq!(a.interactive.len(), b.interactive.len());
        for (pa, pb) in a.interactive.iter().zip(b.interactive.iter()) {
            assert_eq!(pa.origin(), pb.origin());
            assert_eq!(pa.size(), pb.size());
        }
        assert_eq!(a.close_rect, b.close_rect);
        assert_eq!(a.title_pos, b.title_pos);
    }

    #[test]
    fn corner_shift_defaults_on() {
        let view: AppBarView<()> = app_bar("Home").leading(leaf_any(40.0, 40.0));
        let insets = WindowInsets::new(top_padding(0.0), WindowEdgeInsets::ZERO)
            .with_corner_insets(corners((44.0, 30.0), (52.0, 30.0)));
        let (_, root) = laid_out_with_insets(view, insets, 400.0);
        let w = root_bar(&root);
        assert!(w.corner_shift_enabled);
        assert_eq!(w.interactive[0].origin().x, PAD_X + 44.0);
    }

    #[test]
    fn corner_shift_prop_change_marks_layout() {
        let prev = app_bar("Home");
        let mut w = build(&prev);
        assert!(w.corner_shift_enabled);
        let next = app_bar("Home").corner_shift(false);
        let flags = rebuild(&prev, &next, &mut w);
        assert!(!w.corner_shift_enabled);
        assert!(flags.contains(ChangeFlags::LAYOUT));
    }

    // -- Title crossfade staging ---------------------------------------------

    #[test]
    fn title_change_stages_a_crossfade_and_requests_frames_while_animating() {
        let prev = app_bar("Sessions");
        let mut w = build(&prev);
        layout(&mut w, Size::new(360.0, 100.0), None);

        let next = app_bar("Settings");
        let flags = rebuild(&prev, &next, &mut w);
        assert!(flags.needs_layout());
        assert!(w.title_stage.is_some(), "a title change stages a crossfade");
        assert_eq!(w.title_text, "Settings");
        layout(&mut w, Size::new(360.0, 100.0), None);

        // Seed the driver clock: still staging, another frame requested.
        let (_, animating0) = paint(&mut w, ft_ms(0.0), None);
        assert!(animating0);
        assert!(w.title_stage.is_some());

        // Past the 220ms crossfade: staging clears.
        let (_, animating1) = paint(&mut w, ft_ms(400.0), None);
        assert!(!animating1);
        assert!(w.title_stage.is_none(), "the crossfade settled");
    }

    #[test]
    fn title_direction_flips_the_crossfade_shift_sign() {
        let mut fwd = build(&app_bar("A"));
        fwd.title_direction = TitleDirection::Forward;
        let (fwd_new, fwd_old) = fwd.title_shift(0.0);
        // Forward: the new run enters from the right (+), old exits left (0 at p=0).
        assert!(fwd_new > 0.0);
        assert!(fwd_old.abs() < 1e-9);

        let mut back = build(&app_bar("A"));
        back.title_direction = TitleDirection::Back;
        let (back_new, _) = back.title_shift(0.0);
        assert!(back_new < 0.0, "back-nav enters from the left");
    }

    // -- Title position (leading default / center + collision clamp) --------

    #[test]
    fn title_position_defaults_to_leading_and_is_byte_identical_to_todays_layout() {
        let view: AppBarView<()> = app_bar("Sessions");
        assert_eq!(view.title_position, TitlePosition::Leading);
        let mut w = build(&view);
        layout(&mut w, Size::new(360.0, 100.0), None);
        // No leading child: the pre-`title_position` bar always put the
        // title flush against the leading edge (`left == PAD_X`) — pinned
        // here unchanged.
        assert_eq!(w.title_pos.x, PAD_X);
    }

    #[test]
    fn title_position_center_optically_centers_over_the_full_bar_width_when_it_fits() {
        // A leading child narrow enough that a full-width-centered title
        // still clears both slot boxes — Center should center over the
        // bar's full width, not just the free span between the slots (which
        // would land at a different x here, since the leading slot isn't
        // symmetric with the trailing edge).
        let view: AppBarView<()> = app_bar("Hi")
            .title_position(TitlePosition::Center)
            .leading(leaf_any(40.0, 40.0));
        let mut w = build(&view);
        layout(&mut w, Size::new(400.0, 100.0), None);

        let left = PAD_X + 40.0 + SLOT_GAP;
        let right = 400.0 - PAD_X;
        let run_width = w.title.size().width;
        let full_width_centered = (400.0 - run_width) / 2.0;
        assert!(
            full_width_centered >= left && full_width_centered + run_width <= right,
            "test setup: the centered title must not collide with either slot"
        );
        assert!(
            (w.title_pos.x - full_width_centered).abs() < 0.5,
            "expected {full_width_centered}, got {}",
            w.title_pos.x
        );
    }

    #[test]
    fn title_position_center_clamps_to_the_free_span_on_collision() {
        // A wide leading child pushes the free span far enough right that
        // centering over the bar's full width would land inside the
        // leading slot's own box — the clamp falls back to centering within
        // the free span between the slots instead.
        let view: AppBarView<()> = app_bar("Hi")
            .title_position(TitlePosition::Center)
            .leading(leaf_any(320.0, 40.0));
        let mut w = build(&view);
        layout(&mut w, Size::new(400.0, 100.0), None);

        let left = PAD_X + 320.0 + SLOT_GAP;
        let right = 400.0 - PAD_X;
        let run_width = w.title.size().width;
        let full_width_centered = (400.0 - run_width) / 2.0;
        assert!(
            full_width_centered < left,
            "test setup: an uncollided center would defeat the clamp assertion"
        );
        let expected = left + ((right - left - run_width).max(0.0)) / 2.0;
        assert!(
            (w.title_pos.x - expected).abs() < 0.5,
            "expected {expected}, got {}",
            w.title_pos.x
        );
        assert!(
            w.title_pos.x >= left,
            "the clamped title never overlaps the leading slot"
        );
    }

    #[test]
    fn title_position_center_crossfade_still_stages_both_runs() {
        let prev = app_bar("Sessions").title_position(TitlePosition::Center);
        let mut w = build(&prev);
        layout(&mut w, Size::new(360.0, 100.0), None);

        let next = app_bar("Settings").title_position(TitlePosition::Center);
        let flags = rebuild(&prev, &next, &mut w);
        assert!(flags.needs_layout());
        assert!(
            w.title_stage.is_some(),
            "a title change still stages a crossfade under Center"
        );
        layout(&mut w, Size::new(360.0, 100.0), None);

        // Seed the driver clock: still staging, another frame requested.
        let (_, animating0) = paint(&mut w, ft_ms(0.0), None);
        assert!(animating0);
        assert!(w.title_stage.is_some());

        // Past the 220ms crossfade: staging clears, same as under Leading.
        let (_, animating1) = paint(&mut w, ft_ms(400.0), None);
        assert!(!animating1);
        assert!(
            w.title_stage.is_none(),
            "the crossfade settled under Center too"
        );
    }

    // -- Selection mode morph + close ----------------------------------------

    #[test]
    fn selection_swaps_the_face_and_close_fires_on_up_inside_only() {
        let prev: AppBarView<()> = app_bar("Tokens").actions(vec![leaf_any(24.0, 24.0)]);
        let mut w = build(&prev);
        assert!(!w.selection_present);
        assert_eq!(w.interactive.len(), 1, "normal face: one action");

        // The close callback bumps a captured counter (State stays `()`), so the
        // controlled-component contract is visible without a typed state.
        let closes = Rc::new(Cell::new(0u32));
        let c = closes.clone();
        let next: AppBarView<()> = app_bar("Tokens").selection(Some(
            selection_bar(3, move |_: &mut ()| c.set(c.get() + 1))
                .actions(vec![leaf_any(24.0, 24.0), leaf_any(24.0, 24.0)]),
        ));
        let flags = rebuild(&prev, &next, &mut w);
        assert!(flags.needs_layout());
        assert!(w.selection_present, "selection face active");
        assert_eq!(w.count_value, 3);
        assert_eq!(w.interactive.len(), 2, "selection face: two bulk actions");

        // Lay out so the close_rect is populated.
        layout(&mut w, Size::new(360.0, 100.0), None);
        assert!(w.close_rect.width() > 0.0, "close button laid out");

        let (cx, cy) = (w.close_rect.center().x, w.close_rect.center().y);
        let ev = |phase, x, y| {
            InputEvent::Pointer(PointerEvent {
                phase,
                position: Point::new(x, y),
                button: PointerButton::Primary,
            })
        };

        // Down inside, up outside: no fire (up must land inside).
        {
            let mut dummy = ();
            let state: &mut dyn Any = &mut dummy;
            let mut ctx = EventCtx::new(state, Point::ZERO, w.total_size);
            w.event(&mut ctx, &ev(PointerPhase::Down, cx, cy));
            w.event(&mut ctx, &ev(PointerPhase::Up, 300.0, 20.0));
        }
        assert_eq!(closes.get(), 0, "up outside the close box does not fire");

        // Down + up inside: fires once.
        {
            let mut dummy = ();
            let state: &mut dyn Any = &mut dummy;
            let mut ctx = EventCtx::new(state, Point::ZERO, w.total_size);
            w.event(&mut ctx, &ev(PointerPhase::Down, cx, cy));
            w.event(&mut ctx, &ev(PointerPhase::Up, cx, cy));
        }
        assert_eq!(closes.get(), 1, "up-inside fires on_close exactly once");
    }

    #[test]
    fn selection_tint_animates_in_and_count_reads_n_selected() {
        let prev = app_bar("Tokens");
        let mut w = build(&prev);
        let next = app_bar("Tokens").selection(Some(selection_bar(5, |_: &mut ()| {})));
        rebuild(&prev, &next, &mut w);
        assert_eq!(w.count.content, "5 selected");
        layout(&mut w, Size::new(360.0, 100.0), None);

        // Seeded start: no wash yet (progress 0). It rises across frames.
        paint(&mut w, ft_ms(0.0), None);
        assert!(w.sel_progress < 1.0);
        paint(&mut w, ft_ms(400.0), None);
        assert!(
            (w.sel_progress - 1.0).abs() < 1e-6,
            "morph settles fully in"
        );
    }

    // -- Elevation animation + reduce_motion ----------------------------------

    #[test]
    fn elevation_animates_border_and_shadow_over_time() {
        let prev = app_bar("Home");
        let mut w = build(&prev);
        assert_eq!(w.elev_progress, 0.0);
        let next = app_bar("Home").elevated(true);
        rebuild(&prev, &next, &mut w);
        assert!(w.elevated_target);
        layout(&mut w, Size::new(360.0, 100.0), None);

        // Seed the clock: progress still ~0, no shadow yet, animation in flight.
        let (rec0, animating0) = paint(&mut w, ft_ms(0.0), None);
        assert!(animating0);
        assert_eq!(rec0.shadows, 0, "no shadow at progress 0");

        // Past the 250ms elevation: fully elevated, shadow + border drawn.
        let (rec1, _) = paint(&mut w, ft_ms(500.0), None);
        assert!((w.elev_progress - 1.0).abs() < 1e-6);
        assert_eq!(rec1.shadows, 1, "elevated shadow drawn");
    }

    #[test]
    fn reduce_motion_collapses_the_elevation_animation() {
        let mut theme = crate::baseline();
        theme.motion.reduce_motion = true;
        let prev = app_bar("Home");
        let mut w = build(&prev);
        let next = app_bar("Home").elevated(true);
        rebuild(&prev, &next, &mut w);
        layout(&mut w, Size::new(360.0, 100.0), Some(&theme));
        // One paint snaps straight to fully elevated under reduce_motion.
        let (rec, animating) = paint(&mut w, ft_ms(0.0), Some(&theme));
        assert!(!animating, "reduce_motion snaps, no follow-up frame");
        assert!((w.elev_progress - 1.0).abs() < 1e-6);
        assert_eq!(rec.shadows, 1);
    }

    // -- Themed color resolution -------------------------------------------

    #[test]
    fn glyph_dark_and_light_resolve_surface_and_accent() {
        let dark = crate::baseline();
        let light = dark.clone().with_brightness(frust::Brightness::Light);
        for theme in [&dark, &light] {
            let c = resolve_colors(Some(theme));
            assert_eq!(c.surface, theme.scheme().surface_container);
            assert_eq!(c.title_ink, theme.scheme().on_surface);
            assert_eq!(c.accent, theme.scheme().primary);
            assert_eq!(c.wash, theme.scheme().primary_container);
        }
        // Accent-role split: primary (ink) and primary_container (fill) differ
        // in Glyph light mode.
        let lc = resolve_colors(Some(&light));
        assert_ne!(lc.accent, lc.wash, "accent ink != container fill in light");
    }

    #[test]
    fn unthemed_paint_uses_fallback_surface() {
        let mut w = build(&app_bar("Home"));
        layout(&mut w, Size::new(300.0, 100.0), None);
        let (rec, _) = paint(&mut w, ft_ms(0.0), None);
        assert_eq!(
            rec.rects[0].2, FALLBACK_SURFACE,
            "surface fill is the fallback"
        );
    }

    // -- Semantics -------------------------------------------------------------

    #[test]
    fn semantics_is_a_titlebar_labelled_with_the_title_and_ordered_children() {
        fn logic(_: &mut ()) -> AppBarView<()> {
            app_bar("Inbox")
                .leading(leaf_any(40.0, 40.0))
                .actions(vec![leaf_any(24.0, 24.0)])
        }
        let mut root: RenderRoot<(), AppBarView<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(360.0, 60.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::TitleBar)
            .expect("a TitleBar container node");
        assert_eq!(node.label(), Some("Inbox"));
        // The leading/action pods are forwarded via `semantics_child` in
        // leading→actions order (the test-fixture leaves emit no node of their
        // own, so only the container itself is asserted here).
    }

    #[test]
    fn selection_semantics_labels_the_count_and_lists_the_close_button() {
        fn logic(_: &mut ()) -> AppBarView<()> {
            app_bar("Tokens").selection(Some(
                selection_bar(2, |_: &mut ()| {}).actions(vec![leaf_any(24.0, 24.0)]),
            ))
        }
        let mut root: RenderRoot<(), AppBarView<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(360.0, 60.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::TitleBar)
            .expect("a TitleBar container node");
        assert_eq!(node.label(), Some("2 selected"));
        // The owned close button contributes a Button node (the bulk-action
        // leaf emits none of its own).
        assert_eq!(node.children().len(), 1, "the close Button node");
        assert!(
            update.nodes.iter().any(|(_, n)| n.role() == Role::Button),
            "the close button contributes a Button node"
        );
    }

    // -- Large scroll-collapse variant ---------------------------------------

    fn large_view(progress: f64) -> AppBarView<()> {
        app_bar("host")
            .large(large_config("203.0.113.57", leaf_any(120.0, 12.0)))
            .collapse_progress(progress)
    }

    #[test]
    fn large_collapse_interpolates_height_and_title_size_to_match_the_html_states() {
        // Expanded (progress 0): 19px big title, full-opacity meta, tall bar.
        let v0 = large_view(0.0);
        let mut w = build(&v0);
        assert!(w.large_present);
        assert!(w.meta.is_some(), "meta child built with the large config");
        layout(&mut w, Size::new(360.0, 400.0), None);
        let h0 = w.bar_height;
        assert_eq!(w.big_title_size_px, BIG_TITLE_SIZE_MAX);
        assert_eq!(w.meta_alpha, 1.0);

        // Half-collapsed: font-size interpolates `19 - 6*0.5 = 16` (the HTML's
        // `fontSize = 19 - 6*progress`).
        let v_half = large_view(0.5);
        rebuild(&v0, &v_half, &mut w);
        layout(&mut w, Size::new(360.0, 400.0), None);
        assert!(
            (w.big_title_size_px - 16.0).abs() < 1e-4,
            "title size at 0.5 is 16px, got {}",
            w.big_title_size_px
        );
        assert!((w.meta_alpha - 0.5).abs() < 1e-6);

        // Fully collapsed (progress 1): 13px big title (== compact TITLE_SIZE),
        // meta faded out, and a strictly shorter bar than the expanded state.
        let v1 = large_view(1.0);
        rebuild(&v_half, &v1, &mut w);
        layout(&mut w, Size::new(360.0, 400.0), None);
        let h1 = w.bar_height;
        assert_eq!(w.big_title_size_px, TITLE_SIZE);
        assert_eq!(w.meta_alpha, 0.0);
        assert!(
            h0 > h1,
            "expanded large bar ({h0}) is taller than collapsed ({h1})"
        );
    }

    #[test]
    fn selection_takes_precedence_over_the_large_variant() {
        // A bar that is both `large` and in `selection` renders the compact
        // selection face (its owned close button is laid out).
        let v = app_bar("host")
            .large(large_config("203.0.113.57", leaf_any(120.0, 12.0)))
            .selection(Some(selection_bar(2, |_: &mut ()| {})));
        let mut w = build(&v);
        assert!(w.selection_present);
        layout(&mut w, Size::new(360.0, 400.0), None);
        assert!(
            w.close_rect.width() > 0.0,
            "selection close button laid out, not the large row"
        );
        // Bar height is the compact bar, not the tall large layout.
        assert_eq!(w.bar_height, BAR_HEIGHT);
    }

    // -- Connection banner open/close ----------------------------------------

    #[test]
    fn banner_none_to_some_animates_open_requesting_layout_while_in_flight() {
        let prev = app_bar("terminal — dev");
        let mut w = build(&prev);
        assert!(!w.banner_present);
        assert_eq!(w.banner_progress, 0.0);

        let next = app_bar("terminal — dev").banner(Some(banner_spec(
            "connection lost — retrying in 3s",
            BannerVariant::Warning,
        )));
        let flags = rebuild(&prev, &next, &mut w);
        assert!(flags.needs_layout());
        assert!(w.banner_present);
        assert_eq!(
            w.banner_text.as_deref(),
            Some("connection lost — retrying in 3s")
        );

        // First advance seeds the clock (dt 0), leaving the strip mid-open.
        assert!(w.advance(ft_ms(0.0), false), "banner still animating");
        assert!(w.banner_animating);
        // A step forward opens it partway.
        w.advance(ft_ms(100.0), false);
        assert!(
            w.banner_progress > 0.0 && w.banner_progress < 1.0,
            "banner opening mid-flight, got {}",
            w.banner_progress
        );
        assert!(w.banner_animating);

        // Paint mid-open raises needs_layout (banner height is layout-bound).
        layout(&mut w, Size::new(360.0, 120.0), None);
        let (_, _, needs_layout) = paint_nl(&mut w, ft_ms(100.0), None);
        assert!(needs_layout, "an in-flight banner requests relayout");

        // It settles fully open.
        w.advance(ft_ms(1000.0), false);
        assert!((w.banner_progress - 1.0).abs() < 1e-6);
        assert!(!w.banner_animating);
    }

    #[test]
    fn banner_reveal_requests_layout_through_the_settle_frame() {
        // The landing-frame relayout guard: `step` (this widget's hand-rolled
        // `AnimationController::advance` analogue) reports "done" on the very
        // call that snaps `banner_progress` onto its target, so gating solely
        // on `banner_animating` would leave the last banner height `layout`
        // ever saw a hair short of rest. Driven through `RenderRoot` (not the
        // `paint`/`paint_nl` helpers above, which each call `advance` twice
        // per simulated frame — once directly, once again inside `Widget::paint`
        // — collapsing the very landing transition this guard needs to observe).
        // See docs/REVIEW_FOCUS.md's layout-skip section.
        fn closed(_: &mut ()) -> AppBarView<()> {
            app_bar("terminal — dev")
        }
        fn open(_: &mut ()) -> AppBarView<()> {
            app_bar("terminal — dev").banner(Some(banner_spec(
                "connection lost — retrying in 3s",
                BannerVariant::Warning,
            )))
        }
        let mut state = ();
        let mut root: RenderRoot<(), AppBarView<()>> = RenderRoot::new();
        root.rebuild(&mut closed, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(360.0, 200.0), &mut tcx as &mut dyn Any);
        let mut scene = Recorder::default();
        let settled_closed = root.paint(&mut scene, FrameTime::ZERO);
        assert!(
            !settled_closed.needs_layout,
            "already-settled bannerless bar requests no layout"
        );

        root.rebuild(&mut open, &mut state);
        let mut t_ns = 0u64;
        let mut saw_animating_frame = false;
        let mut last_size =
            root.layout_with_text(Size::new(360.0, 200.0), &mut tcx as &mut dyn Any);
        loop {
            let mut scene = Recorder::default();
            let outcome = root.paint(&mut scene, FrameTime::from_nanos(t_ns));
            if outcome.needs_layout {
                saw_animating_frame = true;
                assert!(outcome.needs_frame, "request_layout implies needs_frame");
                last_size =
                    root.layout_with_text(Size::new(360.0, 200.0), &mut tcx as &mut dyn Any);
            } else {
                break;
            }
            t_ns += 16_000_000; // ~16ms per simulated frame
            assert!(
                t_ns < 2_000_000_000,
                "banner reveal should settle well under 2s of simulated frames"
            );
        }
        assert!(
            saw_animating_frame,
            "at least one paint during the reveal reported needs_layout"
        );

        // The settle-frame guarantee: the *last* layout this shell-honest
        // loop ever runs matches a freshly-built already-open bar (which
        // starts fully revealed with no animation at all, per `build`'s
        // seed) — never a hair short of it.
        let mut root_ref: RenderRoot<(), AppBarView<()>> = RenderRoot::new();
        root_ref.rebuild(&mut open, &mut state);
        let target = root_ref.layout_with_text(Size::new(360.0, 200.0), &mut tcx as &mut dyn Any);
        assert_eq!(
            last_size.height, target.height,
            "the last layout a shell-honest driver runs lands exactly on the target height"
        );

        // One more settled paint reports no further layout request.
        let mut scene = Recorder::default();
        let settled_open = root.paint(&mut scene, FrameTime::from_nanos(t_ns));
        assert!(
            !settled_open.needs_layout,
            "settled: no more layout requests"
        );
    }

    #[test]
    fn banner_some_to_none_animates_closed() {
        let prev = app_bar("terminal — dev")
            .banner(Some(banner_spec("connection lost", BannerVariant::Warning)));
        let mut w = build(&prev);
        assert!(w.banner_present);
        assert_eq!(w.banner_progress, 1.0, "starts fully open");

        let next = app_bar("terminal — dev");
        let flags = rebuild(&prev, &next, &mut w);
        assert!(flags.needs_layout());
        assert!(!w.banner_present);

        assert!(
            w.advance(ft_ms(0.0), false),
            "banner still animating closed"
        );
        assert!(w.banner_animating);
        w.advance(ft_ms(100.0), false);
        assert!(
            w.banner_progress > 0.0 && w.banner_progress < 1.0,
            "banner closing mid-flight, got {}",
            w.banner_progress
        );
        w.advance(ft_ms(1000.0), false);
        assert!(w.banner_progress.abs() < 1e-6, "banner fully closed");
        assert!(!w.banner_animating);
    }

    #[test]
    fn banner_warning_and_success_variants_resolve_distinct_status_colors() {
        let theme = crate::baseline();
        let status = theme
            .extension::<frust::StatusPalette>()
            .expect("glyph baseline attaches a StatusPalette");
        let c = status.colors(theme.brightness);

        let (w_bg, _w_border, w_ink) = resolve_banner_colors(Some(&theme), BannerVariant::Warning);
        let (s_bg, _s_border, s_ink) = resolve_banner_colors(Some(&theme), BannerVariant::Success);
        assert_eq!(
            w_ink, c.warning,
            "warning ink resolves StatusPalette warning"
        );
        assert_eq!(
            s_ink, c.success,
            "success ink resolves StatusPalette success"
        );
        assert_ne!(w_ink, s_ink, "warning and success tints are distinct");
        assert_eq!(w_bg, with_alpha(c.warning, BANNER_WARNING_BG_ALPHA));
        assert_eq!(s_bg, with_alpha(c.success, BANNER_SUCCESS_BG_ALPHA));
    }

    #[test]
    fn banner_paints_its_variant_tint_fill() {
        let theme = crate::baseline();
        let warn_ink = theme
            .extension::<frust::StatusPalette>()
            .unwrap()
            .colors(theme.brightness)
            .warning;

        let prev = app_bar("terminal — dev");
        let mut w = build(&prev);
        let next = app_bar("terminal — dev")
            .banner(Some(banner_spec("connection lost", BannerVariant::Warning)));
        rebuild(&prev, &next, &mut w);
        // Force a mid-open progress so the strip has height at layout time.
        w.banner_progress = 0.5;
        layout(&mut w, Size::new(360.0, 120.0), Some(&theme));
        assert!(
            w.banner_rect.height() > 0.0,
            "banner strip has height mid-open"
        );

        let (rec, _, _) = paint_nl(&mut w, ft_ms(0.0), Some(&theme));
        let want = with_alpha(warn_ink, BANNER_WARNING_BG_ALPHA);
        assert!(
            rec.rects.iter().any(|(_, _, c)| *c == want),
            "the warning-tint banner fill is recorded"
        );
    }

    // -- reduce_motion collapses the banner ----------------------------------

    #[test]
    fn reduce_motion_snaps_the_banner_open() {
        let mut theme = crate::baseline();
        theme.motion.reduce_motion = true;
        let prev = app_bar("terminal — dev");
        let mut w = build(&prev);
        let next = app_bar("terminal — dev")
            .banner(Some(banner_spec("connection lost", BannerVariant::Warning)));
        rebuild(&prev, &next, &mut w);
        layout(&mut w, Size::new(360.0, 120.0), Some(&theme));
        // One advance snaps straight to fully open under reduce_motion.
        let animating = w.advance(ft_ms(0.0), true);
        assert!(!animating, "reduce_motion snaps, no follow-up frame");
        assert!((w.banner_progress - 1.0).abs() < 1e-6);
        assert!(!w.banner_animating);
    }

    // -- Semantics: meta row + banner join the container ---------------------

    #[test]
    fn large_and_banner_semantics_join_the_titlebar_children() {
        fn logic(_: &mut ()) -> AppBarView<()> {
            app_bar("terminal — dev")
                .large(large_config("203.0.113.57", leaf_any(120.0, 12.0)))
                .banner(Some(banner_spec("connection lost", BannerVariant::Warning)))
        }
        let mut root: RenderRoot<(), AppBarView<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(360.0, 200.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::TitleBar)
            .expect("a TitleBar container node");
        // The large variant labels the bar with its big title.
        assert_eq!(node.label(), Some("203.0.113.57"));
        // The banner text joins the container as a Label node.
        assert!(
            update
                .nodes
                .iter()
                .any(|(_, n)| n.role() == Role::Label && n.label() == Some("connection lost")),
            "the banner text contributes a Label node"
        );
    }

    // ---- Typeface: every run's family follows its type-scale role ---------

    /// The compact face: title and subtitle.
    #[cfg(feature = "bundled-fonts")]
    fn compact(_: &mut ()) -> AppBarView<()> {
        app_bar("shell").subtitle("dev")
    }

    /// The large variant with an open connection banner: big title, banner.
    #[cfg(feature = "bundled-fonts")]
    fn large_with_banner(_: &mut ()) -> AppBarView<()> {
        app_bar("shell")
            .large(large_config("dev host", leaf_any(120.0, 12.0)))
            .banner(Some(banner_spec("connection lost", BannerVariant::Warning)))
    }

    /// Selection mode: the count face.
    #[cfg(feature = "bundled-fonts")]
    fn selecting(_: &mut ()) -> AppBarView<()> {
        app_bar("shell").selection(Some(selection_bar(3, |_: &mut ()| {})))
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn runs_paint_in_their_role_faces_under_the_glyph_theme() {
        use crate::badge::typeface_probe::{Face, painted_faces};
        let window = Size::new(360.0, 200.0);
        assert_eq!(
            painted_faces(compact, crate::baseline(), window),
            [Face::PlexMono, Face::PlexMono],
            "title, subtitle"
        );
        assert_eq!(
            painted_faces(large_with_banner, crate::baseline(), window),
            [Face::SpaceMono, Face::PlexMono],
            "big title, banner"
        );
        assert_eq!(
            painted_faces(selecting, crate::baseline(), window),
            [Face::PlexMono],
            "selection count"
        );
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn runs_follow_a_live_theme_family_swap() {
        use crate::badge::typeface_probe::{Face, faces_across_a_live_swap};
        let window = Size::new(360.0, 200.0);
        assert_eq!(
            faces_across_a_live_swap(compact, window).1,
            [Face::SpaceMono, Face::SpaceMono],
            "title, subtitle"
        );
        assert_eq!(
            faces_across_a_live_swap(large_with_banner, window).1,
            [Face::PlexMono, Face::SpaceMono],
            "big title, banner"
        );
        assert_eq!(
            faces_across_a_live_swap(selecting, window).1,
            [Face::SpaceMono],
            "selection count"
        );
    }
}
