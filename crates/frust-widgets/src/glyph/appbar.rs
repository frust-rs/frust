//! Glyph top AppBar — core (task 04, glyph-refinements): the compact
//! terminal-native top bar per `research/glyph-appbar.html` sections 01/03/05,
//! retrieved 2026-07-22.
//!
//! # Anatomy (§01)
//!
//! A `52px` content bar of three zones — **leading** (a back arrow / brand mark
//! / nothing, an app-supplied [`AnyView`]), **title** (a title string plus an
//! optional 10px subtitle row, ellipsized), and **trailing** (0–2 app-supplied
//! action [`AnyView`]s). Icon hit targets are `40px`, `6px` horizontal edge
//! padding, `2px` slot gaps (the HTML's `.appbar`/`.ab-icon` rules).
//!
//! # Top-inset consumption (the Flutter parity point)
//!
//! Layout adds [`WindowInsets::padding().top`](frust_core::WindowInsets::padding)
//! to its own height and offsets its content **below** the inset; the background
//! paints the full extended height, so the bar surface runs under the status
//! bar. Content beneath a Glyph AppBar therefore never needs a top `SafeArea`
//! edge — exactly Flutter's `AppBar` behavior.
//!
//! # Elevation (§01)
//!
//! [`AppBarView::elevated`] is an app-fed flag (the app toggles it off its own
//! `on_scroll`, the same way the HTML flips `.elevated` at `y > 4`): a
//! transparent bottom border animates to `outline` + a drop shadow over
//! `~250ms`, collapsing instantly under `reduce_motion`.
//!
//! # Title crossfade (§03)
//!
//! When the title string changes across rebuilds, the bar stages the old and
//! new runs with a directional shift (`±14px` x-offset + fade over `~220ms`),
//! driven from `PaintCtx::frame_time` (the dialog/toast staging precedent — the
//! bar shapes its own runs like [`super::navbar`] rather than nesting a
//! `PatternSwitcher`). Direction is forward (new slides in from the right) by
//! default; [`AppBarView::title_direction`] flips it to back-nav.
//!
//! # Selection mode (§05)
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

use frust_core::accesskit::Role;
use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, Curve, EventCtx, EventResult,
    FrameTime, InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerPhase, SemanticsCtx, View,
    Widget,
};
use frust_text::{FontFamily, FontWeight, GenericSlot, TextContext, TextLayout, TextStyle};
use frust_theme::Theme;
use kurbo::{Affine, Point, Rect, Size};
use peniko::{Brush, Color};

use crate::Timing;
use crate::nav::transition::{TransitionDriver, make_driver};

// ---- Metrics (`glyph-appbar.html` §01, retrieved 2026-07-22) ---------------

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
/// Vertical gap between the title and subtitle rows, logical px (this task's
/// own choice — a tight 1px separation matching the HTML's stacked rows).
const SUBTITLE_GAP: f64 = 1.0;

/// Title-crossfade x-shift, logical px (`.bn-title-a{translateX(-14px)}`, §03).
const TITLE_SHIFT: f64 = 14.0;
/// Selection-morph y-shift, logical px (`.sel-normal{translateY(-6px)}`, §05).
const SELECTION_SHIFT: f64 = 6.0;
/// Selection wash alpha over the surface (`--amber-faint` = accent at 12%; here
/// sourced from the `primary_container` fill role per the accent-role split).
const SELECTION_WASH_ALPHA: f32 = 0.12;

/// Elevation animation duration (`.appbar{transition:...box-shadow .25s}`, §01).
const ELEVATION_DURATION: Duration = Duration::from_millis(250);
/// Selection-morph animation duration (`durations.base` — the same 220ms the
/// toast/dialog enter uses).
const SELECTION_DURATION: Duration = Duration::from_millis(220);
/// Title-crossfade duration (§03's "small directional shift", `durations.base`).
const TITLE_DURATION: Duration = Duration::from_millis(220);
/// Title-crossfade easing (the HTML's `--ease-out` cubic).
const TITLE_CURVE: Curve = Curve::Cubic(0.16, 1.0, 0.3, 1.0);
/// `reduce_motion`'s collapsed crossfade duration (mirrors the toast/dialog
/// constant of the same name).
const REDUCE_MOTION_DURATION: Duration = Duration::from_millis(120);

/// Elevated drop-shadow recipe (`.appbar.elevated{box-shadow:0 6px 16px
/// rgba(0,0,0,0.28)}`, §01).
const SHADOW_Y: f64 = 6.0;
const SHADOW_BLUR: f64 = 16.0;
const SHADOW_ALPHA: f32 = 0.28;

// ---- Large-variant metrics (`glyph-appbar.html` §02, retrieved 2026-07-22) --

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

// ---- Connection-banner metrics (`glyph-appbar.html` §06) --------------------

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
        Some(theme) => match theme.extension::<frust_theme::StatusPalette>() {
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

/// The Glyph `body` (IBM Plex Mono) font stack every run in this bar shapes
/// against.
fn mono_family() -> FontFamily {
    FontFamily::stack_with_generic(["IBM Plex Mono"], GenericSlot::Monospace)
}

/// The Glyph `display` (Space Mono) font stack the large-variant big title
/// shapes against (`.ab-big-title{font-family:var(--font-display)}`, §02).
fn display_family() -> FontFamily {
    FontFamily::stack_with_generic(["Space Mono"], GenericSlot::Monospace)
}

/// The big-title style at the collapse-interpolated `size` (display font, bold).
fn big_title_style(size: f32, color: Color) -> TextStyle {
    TextStyle {
        family: display_family(),
        weight: FontWeight::BOLD,
        ..TextStyle::new(size, color)
    }
}

/// The connection-banner text style (body mono, 11px).
fn banner_style(color: Color) -> TextStyle {
    TextStyle {
        family: mono_family(),
        weight: FontWeight::REGULAR,
        ..TextStyle::new(BANNER_FONT_SIZE, color)
    }
}

fn title_style(color: Color) -> TextStyle {
    TextStyle {
        family: mono_family(),
        weight: FontWeight::MEDIUM,
        ..TextStyle::new(TITLE_SIZE, color)
    }
}

fn subtitle_style(color: Color) -> TextStyle {
    TextStyle {
        family: mono_family(),
        weight: FontWeight::REGULAR,
        ..TextStyle::new(SUBTITLE_SIZE, color)
    }
}

fn count_style(color: Color) -> TextStyle {
    TextStyle {
        family: mono_family(),
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

/// Direction the title crossfade shifts on a title change (§03).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TitleDirection {
    /// The new title slides in from the right (default, forward navigation).
    #[default]
    Forward,
    /// The new title slides in from the left (back navigation).
    Back,
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

/// The selection-mode configuration (§05): a count, a close callback, and the
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

/// Which tint a [`BannerSpec`] paints (§06): the connection-loss warning strip
/// or the `recovered` success flash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BannerVariant {
    /// The connection-loss warning strip (`--warning` tint).
    Warning,
    /// The reconnect-success flash (`--success` tint, the HTML's `recovered`
    /// state).
    Success,
}

/// A connection-loss banner strip (§06): a caller-supplied `text` line under a
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

/// The large scroll-collapse configuration (§02): a `big_title` shown at 19px
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
    title_direction: TitleDirection,
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
        title_direction: TitleDirection::Forward,
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

    /// Set the title-crossfade direction for the next title change (§03).
    pub fn title_direction(mut self, direction: TitleDirection) -> Self {
        self.title_direction = direction;
        self
    }

    /// Enter/leave selection mode (§05). `Some` morphs the bar into its
    /// selection face; `None` (the default) is the normal face.
    pub fn selection(mut self, selection: Option<SelectionBar<State>>) -> Self {
        self.selection = selection;
        self
    }

    /// Enable the large scroll-collapse variant (§02): a big display-font title
    /// over an app-supplied meta row. Drive its collapse with
    /// [`Self::collapse_progress`]. Selection mode (§05) takes precedence — a
    /// bar that is both `large` and in `selection` renders the compact
    /// selection face.
    pub fn large(mut self, config: LargeConfig<State>) -> Self {
        self.large = Some(config);
        self
    }

    /// Set the large variant's collapse progress `0.0..=1.0` (§02). The app
    /// computes this from its own scroll offset (`min(1, offset/60)` in the
    /// research HTML); the bar interpolates its padding, big-title size, and
    /// meta-row height/fade from it. No-op unless [`Self::large`] is set.
    pub fn collapse_progress(mut self, progress: f64) -> Self {
        self.collapse_progress = progress.clamp(0.0, 1.0);
        self
    }

    /// Attach (or clear) the connection-loss banner strip beneath the bar
    /// (§06). `Some` animates the strip open across rebuilds; `None` animates it
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
    on_close: Option<crate::ErasedCallback>,
    title_direction: TitleDirection,
    elevated_target: bool,
    // --- large variant (§02) ---
    /// Whether the current view carries a [`LargeConfig`] (its layout is only
    /// *active* when not also in selection mode — see `large_active`).
    large_present: bool,
    big_title: ShapedRun,
    big_title_text: String,
    /// The app-supplied meta row (heartbeat/latency), present iff `large_present`.
    meta: Option<ChildPod>,
    /// Input-driven collapse progress `0.0..=1.0` (not animated internally).
    collapse_progress: f64,
    // --- connection banner (§06) ---
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
            .map(|v| crate::build_child(v, ctx))
            .collect();
        let selection_present = self.selection.is_some();
        let large_present = self.large.is_some();
        let meta = self
            .large
            .as_ref()
            .map(|l| crate::build_child(&l.meta, ctx));
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
                .map(|s| crate::erase_callback(&s.on_close)),
            title_direction: self.title_direction,
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

        // Title change → stage a directional crossfade (§03).
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
                crate::teardown_child(view, pod, ctx);
            }
            element.interactive = face_views(self)
                .iter()
                .map(|v| crate::build_child(v, ctx))
                .collect();
            element.selection_present = selection_present;
            element.has_leading = !selection_present && self.leading.is_some();
            element.close_pressed = false;
            element.close_captured = false;
            element.on_close = self
                .selection
                .as_ref()
                .map(|s| crate::erase_callback(&s.on_close));
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else {
            // Same face: reconcile children in place, refresh the close adapter.
            let prev_views = face_views(prev);
            let next_views = face_views(self);
            flags |= crate::rebuild_children(
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
                .map(|s| crate::erase_callback(&s.on_close));
        }

        // Large variant reconcile (§02). Collapse is input-driven — a progress
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
                    flags |= crate::rebuild_child(&prev_l.meta, &next_l.meta, pod, ctx);
                }
            }
            (None, Some(next_l)) => {
                element.big_title = ShapedRun::new(next_l.big_title.clone());
                element.big_title_text = next_l.big_title.clone();
                element.meta = Some(crate::build_child(&next_l.meta, ctx));
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            (Some(prev_l), None) => {
                if let Some(pod) = element.meta.as_mut() {
                    crate::teardown_child(&prev_l.meta, pod, ctx);
                }
                element.meta = None;
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            (None, None) => {}
        }
        element.large_present = self.large.is_some();

        // Connection banner reconcile (§06). A spec change (presence or
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
            crate::teardown_child(view, pod, ctx);
        }
        if let (Some(large), Some(pod)) = (&self.large, element.meta.as_mut()) {
            crate::teardown_child(&large.meta, pod, ctx);
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
        // Resolve every color to an owned value up front so the immutable theme
        // borrow ends before the `&mut ctx` child-layout calls below.
        let theme = Theme::from_layout_ctx(ctx);
        let colors = resolve_colors(theme);
        let banner_ink = resolve_banner_colors(theme, self.banner_variant).2;

        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        let top_inset = ctx.window_insets().padding().top;
        self.top_inset = top_inset;

        // The bar's own height, laid out either as the large scroll-collapse
        // variant (§02) or the compact three-zone bar (§01). Selection mode
        // (§05) takes precedence over `large`.
        let large_active = self.large_present && !self.selection_present;
        let bar_height = if large_active {
            self.layout_large(ctx, &colors, width, top_inset)
        } else {
            self.layout_compact(ctx, &colors, width, top_inset)
        };
        self.bar_height = bar_height;

        // Connection banner strip (§06) beneath the bar. Its height tracks the
        // eased open/close progress — layout-bound, so paint requests relayout
        // while it animates.
        let banner_height = self.layout_banner(ctx, banner_ink, width, bar_height);

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
}

impl AppBarWidget {
    /// Lay out the compact three-zone bar (§01), returning its total height
    /// (`top_inset + BAR_HEIGHT`). Unchanged from task 04 — factored out of
    /// `Widget::layout` so the large variant can slot in beside it.
    fn layout_compact(
        &mut self,
        ctx: &mut LayoutCtx,
        colors: &BarColors,
        width: f64,
        top_inset: f64,
    ) -> f64 {
        let content_top = top_inset;
        let icon_y = content_top + (BAR_HEIGHT - ICON_SIZE) / 2.0;
        let slot_bc = BoxConstraints::loose(Size::new(f64::INFINITY, BAR_HEIGHT));

        // Leading edge: the selection close button (owned) or the normal-face
        // leading child, whichever this face uses.
        let mut left = PAD_X;
        let action_start;
        if self.selection_present {
            self.close_rect =
                Rect::from_origin_size(Point::new(PAD_X, icon_y), Size::new(ICON_SIZE, ICON_SIZE));
            left = PAD_X + ICON_SIZE + SLOT_GAP;
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
        let mut right = width - PAD_X;
        for pod in self.interactive[action_start..].iter_mut().rev() {
            let size = pod.layout_child(ctx, &slot_bc);
            right -= size.width;
            pod.set_origin(Point::new(
                right,
                content_top + (BAR_HEIGHT - size.height) / 2.0,
            ));
            right -= SLOT_GAP;
        }

        // Title zone fills the middle; its runs are left-aligned and ellipsized
        // to the available width.
        let zone_x = left;
        let zone_w = (right - left).max(0.0);
        let title_size =
            self.title
                .layout(ctx, &title_style(colors.title_ink), Some(zone_w as f32));
        if let Some(stage) = &mut self.title_stage {
            stage
                .old
                .layout(ctx, &title_style(colors.title_ink), Some(zone_w as f32));
        }
        let subtitle_size = self.subtitle.as_mut().map(|s| {
            s.layout(
                ctx,
                &subtitle_style(colors.subtitle_ink),
                Some(zone_w as f32),
            )
        });
        self.count
            .layout(ctx, &count_style(colors.accent), Some(zone_w as f32));

        // Vertically center the title (+subtitle) stack inside the content band.
        let stack_h = title_size.height + subtitle_size.map_or(0.0, |s| SUBTITLE_GAP + s.height);
        let title_y = content_top + (BAR_HEIGHT - stack_h) / 2.0;
        self.title_pos = Point::new(zone_x, title_y);
        self.subtitle_pos = Point::new(zone_x, title_y + title_size.height + SUBTITLE_GAP);
        let count_h = self.count.size().height;
        self.count_pos = Point::new(zone_x, content_top + (BAR_HEIGHT - count_h) / 2.0);

        top_inset + BAR_HEIGHT
    }

    /// Lay out the large scroll-collapse variant (§02), returning its total
    /// height. Geometry interpolates on `collapse_progress`: paddings `6→0` /
    /// `10→0`, big-title size `19→13`, big-title bottom padding `2→0`, meta-row
    /// height `20→0` + fade. Row 1 (leading + trailing actions) reuses the
    /// compact slot geometry; the compact title/subtitle/count are unused here.
    fn layout_large(
        &mut self,
        ctx: &mut LayoutCtx,
        colors: &BarColors,
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
        let action_start = if self.has_leading {
            let size = self.interactive[0].layout_child(ctx, &slot_bc);
            self.interactive[0].set_origin(Point::new(
                PAD_X,
                row1_top + (ICON_SIZE - size.height) / 2.0,
            ));
            1
        } else {
            0
        };
        let mut right = width - PAD_X;
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
            &big_title_style(size_px, colors.title_ink),
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

    /// Lay out the connection banner strip (§06) beneath the bar, returning its
    /// eased height (`0` when fully closed and absent). The strip's box and the
    /// centered text position are cached in local coordinates.
    fn layout_banner(
        &mut self,
        ctx: &mut LayoutCtx,
        ink: Color,
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
        let tsize = self
            .banner_run
            .layout(ctx, &banner_style(ink), Some(max_w as f32));
        self.banner_text_pos =
            Point::new(BANNER_PAD_X, bar_bottom + (banner_h - tsize.height) / 2.0);
        banner_h
    }

    fn paint_impl(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let reduce_motion = theme.map(|t| t.motion.reduce_motion).unwrap_or(false);
        let colors = resolve_colors(theme);
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
            // Large variant (§02): row-1 children (no selection morph fade), the
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
            // Compact face (§01/§03/§05).
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

        // Connection banner strip (§06) beneath the bar.
        self.paint_banner(origin, banner_colors, scene);

        // The banner is height-animated (layout-bound): while it animates, ask
        // for a relayout (which implies another frame); otherwise a plain frame
        // request covers the paint-only elevation/selection/title timelines.
        if self.banner_animating {
            ctx.request_layout();
        } else if animating {
            ctx.request_frame();
        }
    }

    /// Paint the connection banner strip (§06): a `variant`-tinted fill, a bottom
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
            } else if p.phase == PointerPhase::Down && self.close_rect.contains(p.position) {
                self.close_pressed = true;
                self.close_captured = true;
                ctx.capture_pointer();
                ctx.request_redraw();
                return EventResult::Handled;
            }
        }
        // Row-1 / face children first, then the large variant's meta row (which
        // may itself hold interactive views).
        let result = crate::route_event(&mut self.interactive, ctx, event);
        if result == EventResult::Handled {
            return result;
        }
        if let Some(pod) = self.meta.as_mut() {
            let meta_result = crate::route_event_single(pod, ctx, event);
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
                // shown (§06).
                if banner_present && let Some(text) = &banner_text {
                    ctx.push_node(Role::Label, |node| node.set_label(text.as_str()));
                }
            },
        );
    }
}

impl AppBarWidget {
    /// Paint the title zone's normal-face content: either the resting title run
    /// or the staged old/new crossfade pair (§03).
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
    use crate::test_support::leaf_any;
    use frust_core::{
        BuildCtx, PointerButton, PointerEvent, RenderRoot, WindowEdgeInsets, WindowInsets,
    };
    use frust_text::TextContext;
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
        fn draw_glyph_run(&mut self, _run: frust_scene::GlyphRun) {
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

    // -- Acceptance 1: top-inset consumption (Flutter parity) ---------------

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

    // -- Acceptance 2: title crossfade staging ------------------------------

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

    // -- Acceptance 3: selection mode morph + close ------------------------

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

    // -- Acceptance 4: elevation animation + reduce_motion ------------------

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
        let mut theme = Theme::glyph_baseline();
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
        let dark = Theme::glyph_baseline();
        let light = dark.clone().with_brightness(frust_theme::Brightness::Light);
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

    // -- Acceptance 5: semantics -------------------------------------------

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

    // -- Acceptance 1 (task 13): large scroll-collapse variant (§02) ---------

    fn large_view(progress: f64) -> AppBarView<()> {
        app_bar("host")
            .large(large_config("100.71.31.57", leaf_any(120.0, 12.0)))
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
            .large(large_config("100.71.31.57", leaf_any(120.0, 12.0)))
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

    // -- Acceptance 2 (task 13): connection banner open/close (§06) ----------

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
        let theme = Theme::glyph_baseline();
        let status = theme
            .extension::<frust_theme::StatusPalette>()
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
        let theme = Theme::glyph_baseline();
        let warn_ink = theme
            .extension::<frust_theme::StatusPalette>()
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

    // -- Acceptance 4 (task 13): reduce_motion collapses the banner ----------

    #[test]
    fn reduce_motion_snaps_the_banner_open() {
        let mut theme = Theme::glyph_baseline();
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

    // -- Semantics (task 13): meta row + banner join the container ----------

    #[test]
    fn large_and_banner_semantics_join_the_titlebar_children() {
        fn logic(_: &mut ()) -> AppBarView<()> {
            app_bar("terminal — dev")
                .large(large_config("100.71.31.57", leaf_any(120.0, 12.0)))
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
        assert_eq!(node.label(), Some("100.71.31.57"));
        // The banner text joins the container as a Label node.
        assert!(
            update
                .nodes
                .iter()
                .any(|(_, n)| n.role() == Role::Label && n.label() == Some("connection lost")),
            "the banner text contributes a Label node"
        );
    }
}
