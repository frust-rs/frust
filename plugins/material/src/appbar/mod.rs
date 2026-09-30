// Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
// `lib/components/app_bars/` — `m3e_app_bars.dart`, `enums/m3e_app_bar_enums.dart`,
// `styles/m3e_app_bar_theme.dart`, `components/m3e_app_bar_semantics.dart`
// (retrieved 2026-08-20). Upstream:
// https://github.com/paadevelopments/material_3_expressive
// Porting decisions (each restated with its reference site in the module docs
// below): the collapsing bar's scroll linkage is an app-fed controlled prop
// rather than a sliver-protocol participant; the small/collapsed band keeps M3's
// published 64dp rather than upstream's own 72dp content-padding model; and the
// bar's own edge/slot insets stay this catalog's 4dp rather than upstream's
// 0/8dp pair.

//! The M3 Expressive **app bar** family: four constructors over one set of
//! metrics, shape families, and densities.
//!
//! | Constructor | Upstream | Shape |
//! |---|---|---|
//! | [`app_bar`] | `M3EAppBar.top` | a fixed, single-line top bar: leading slot, title, actions |
//! | [`search_app_bar`] | `M3EAppBar.search` | the same bar with a [`crate::search_bar`] filling the title slot |
//! | [`bottom_app_bar`] | `M3EAppBar.bottom` | a bottom-docked action row with an optional prominent/FAB slot |
//! | [`sliver_app_bar`] | `M3EAppBar.sliver` | a collapsing bar whose headline interpolates between an expanded and a collapsed band |
//!
//! Every bar in the family shares the slot contract [`crate::toolbar`] and
//! [`crate::navbar`] also follow: a `leading` slot and an ordered `Vec` of
//! `actions` (plus the bottom bar's `fab` slot) are opaque caller-supplied
//! [`AnyView`] children routed through [`ChildPod`]s exactly like
//! [`frust::Row`]'s children — this family lays them out and routes events to
//! them, but never paints or tints their content itself (the leading slot's
//! `onSurface` / actions' `onSurfaceVariant` tinting guidance is the *caller's*
//! job to apply to whatever `AnyView` it supplies, e.g. an icon button; there is
//! no icon primitive here to tint, and upstream's own `IconTheme.merge`
//! ambient-icon-size push has no analogue on this framework's public surface).
//!
//! Every top and sliver bar also shifts its leading slot right by
//! `corner_insets.top_left.width` and its trailing edge left by
//! `corner_insets.top_right.width` whenever that corner's height is above zero
//! (the iPadOS 26+ window control). The band height is unchanged, the other
//! corners are ignored, the bottom bar is unaffected, and the value is never
//! consumed by `safe_area`, so a bar under a top-consuming safe area still sees it.
//!
//! The **title** is the one child a top/collapsing bar fully owns when given as
//! a string: composed as a child [`frust::TextView`] (no hand-shaped text),
//! themed `onSurface` (the `Text` default role) so it survives a live theme swap
//! through `Text`'s own layout-time color resolution, ellipsized to one line
//! (upstream's `overflow: TextOverflow.ellipsis` on both `titleText` paths).
//! [`AppBarView::title_view`] replaces it with any caller view — upstream's
//! `title` widget slot, and what [`search_app_bar`] itself uses.
//!
//! # Metrics ([`AppBarMetrics`]), densities, and the one deliberate divergence
//!
//! | Band | Regular | Compact | Source |
//! |---|---|---|---|
//! | small / collapsed | 64 | 56 | m3.material.io/components/top-app-bars/specs |
//! | medium expanded | 112 | 104 | `M3EAppBarTheme.mediumExpanded` (= the M3 spec) |
//! | large expanded | 152 | 144 | `M3EAppBarTheme.largeExpanded` (= the M3 spec) |
//! | bottom | 80 | 80 | `M3EAppBarTheme.bottomHeight` (= the M3 spec) |
//!
//! [`AppBarDensity::Compact`] subtracts upstream's own
//! `compactHeightReduction` (8dp) from every band **but the bottom one** —
//! `M3EAppBar.bottom` pins `density: regular` and reads `bottomHeight` straight
//! off the theme, so there is no compact bottom bar to port.
//!
//! **The divergence**: upstream's `smallHeight`/`collapsedHeight` are both 72dp
//! (its own "8 + 56 + 8" content-padding model, `M3EAppBarTheme`'s own comment);
//! this port keeps the published M3 small-top-app-bar height of 64dp, which is
//! also what this catalog already shipped and what
//! [`crate::selection_app_bar`]'s contextual band mirrors so the two read as one
//! continuous surface across a selection-mode swap. Upstream's 72dp is
//! reachable as an explicit [`AppBarView::toolbar_height`] (its own
//! `toolbarHeight` override). The same reasoning keeps this family's 4dp edge
//! inset and 4dp slot gap rather than upstream's `contentPadding` (0 horizontal)
//! + `titleGap` (8dp) pair.
//!
//! # Shape families
//!
//! [`AppBarShapeFamily::Square`] resolves `theme.shape.none` and
//! [`AppBarShapeFamily::Round`] resolves `theme.shape.small` (8dp) —
//! `M3EAppBarTheme.shape`'s own `M3EShapes.radiusNone`/`radiusSmall` pair. A
//! square bar fills a plain rect, a round one a rounded rect. Upstream's own
//! per-constructor defaults are ported as-is: square for the top/search bars,
//! round for the collapsing one, square (unconfigurable) for the bottom one.
//!
//! # Scroll-linked collapse: the app owns the wiring
//!
//! A widget here has no window or scroll handle — [`frust::ScrollView`]'s two
//! seams ([`on_scroll`](frust::ScrollView::on_scroll),
//! [`on_refresh_release`](frust::ScrollView::on_refresh_release)) are installed
//! on the scroll view at construction, and only the app holds both it and the
//! bar. So the collapse is a **controlled prop**, the same app-side-rule shape
//! [`crate::SearchViewMode::for_width`] takes for the search view's
//! presentation split: the app feeds the [`ScrollInfo`](frust::ScrollInfo) it
//! observes into [`SliverAppBarView::scroll_offset`] (or a fraction it computes
//! itself into [`SliverAppBarView::collapse`]), and this widget is a pure
//! function of it.
//!
//! ```
//! use frust::{ScrollInfo, scroll_view, text};
//! use frust_material::{AppBarVariant, sliver_app_bar};
//!
//! struct App {
//!     /// The last offset the body reported — the bar's collapse input.
//!     scrolled: f64,
//! }
//!
//! fn header(state: &App) -> frust_material::SliverAppBarView<App> {
//!     sliver_app_bar("Inbox")
//!         .variant(AppBarVariant::Large)
//!         .scroll_offset(state.scrolled)
//! }
//!
//! fn body(_state: &App) -> impl frust::View<App> + use<> {
//!     scroll_view(text("content"))
//!         .on_scroll(|state: &mut App, info: ScrollInfo| state.scrolled = info.offset)
//! }
//! ```
//!
//! [`AppBarCollapse`] is that mapping as a standalone pure function pair, so an
//! app can pin the same geometry its layout math needs (how tall the bar is at
//! a given offset, how far the scroll must travel to collapse it) without
//! building a view.
//!
//! There is no motion controller anywhere in this family: a collapse is a value
//! change, applied on the rebuild that carries it. That is already the
//! `reduce_motion` behavior every animated widget in this catalog falls back to,
//! so there is nothing here for that flag to collapse.
//!
//! # Semantics
//!
//! A top/collapsing bar is one accesskit container node of [`Role::TitleBar`]
//! (the closest accesskit vocabulary to "a window/app title bar region", chosen
//! over a generic `Role::GenericContainer` since accesskit publishes a
//! purpose-built role for exactly this region), labelled with
//! [`AppBarView::semantic_label`] or the title text, whose children are the
//! leading slot, the title, and each action in that order via
//! [`frust::authoring::SemanticsCtx::push_container`]. A bottom bar is a
//! [`Role::Toolbar`] container instead — the same role [`crate::toolbar`] uses,
//! since a bottom action row is not a title bar. This replaces upstream's
//! `M3ESliverSemantic` render-object wrapper (`m3e_app_bar_semantics.dart`),
//! which exists only because a Flutter sliver cannot carry a `Semantics` widget
//! directly.
//!
//! # Not ported
//!
//! * **`safeArea` / `MediaQuery.viewPadding`.** This family's own bars still
//!   never self-inset (`docs/CODE_STANDARDS.md`'s
//!   self-sizing-chrome-consumes-its-own-inset rule leaves the choice to the
//!   composer); a caller wraps a top/collapsing/bottom app bar in
//!   `frust::safe_area(...)`, the same way [`crate::selection_app_bar`]
//!   documents. [`crate::navigation_bar`] is this catalog's one
//!   self-insetting chrome widget (the bottom edge, `.safe_area(bool)`,
//!   default on) — an exception to this family's contract, not a precedent
//!   for it.
//! * **`automaticallyImplyLeading`.** Upstream reads `Navigator.maybeOf(context)`
//!   to synthesize a back button; a widget here has no navigator handle, and
//!   this catalog's convention is an explicit `leading` slot (supply
//!   [`fn@crate::icon_button`] with [`crate::icons::ARROW_BACK`]).
//! * **`floating`/`snap`.** Both need the scroll *direction* and a per-frame
//!   sliver-protocol callback this framework has no seam for; an app that wants
//!   reveal-on-scroll-up computes its own collapse fraction and feeds it in.
//!   [`SliverAppBarView::pinned`] *is* ported — it selects whether the bar
//!   bottoms out at the collapsed band or scrolls away entirely.
//! * **`elevation` / `Material` shadow.** Upstream's own default is `0`, so a
//!   ported default paints nothing; this family has no shadow to place behind an
//!   elevation token (the FAB/Card precedent owns that).
//! * **`clipBehavior`.** The collapsing bar clips its own content band once the
//!   bar is shorter than it (see [`mod@sliver`]); nothing else in the family
//!   overflows its box, so there is no clip knob to expose.
//! * **`foregroundColor` / `IconTheme.merge`.** Slot content is opaque here —
//!   the caller styles its own icon buttons, per the slot contract above.
//! * **`M3EAppBarTheme` as a theme extension.** MaterialTokens-only: every
//!   color/shape below resolves from [`frust::Theme`]/`ColorScheme` directly,
//!   with an explicit builder override winning over both, the same precedence
//!   [`mod@crate::list_item`]/[`mod@crate::card_list`] use.
//!
//! # Attribution
//!
//! See `plugins/material/NOTICE`'s "MIT License — Additional Copyright Holders
//! (Vendored Components)" section and its Module Attribution Header Convention.

use frust::authoring::text::{FontWeight, LineHeight, TextOverflow};
use frust::authoring::{AnyView, ChildPod, Role, ThemeTextType};
use frust::{ShapeScale, Theme, text};
use peniko::Color;

pub mod bottom;
pub mod sliver;
pub mod top;

pub use bottom::{BottomAppBar, BottomAppBarView, BottomAppBarWidget, bottom_app_bar};
pub use sliver::{SliverAppBar, SliverAppBarView, SliverAppBarWidget, sliver_app_bar};
pub use top::{AppBar, AppBarView, AppBarWidget, SearchAppBar, app_bar, search_app_bar};

/// Container height of the small/center-aligned Top App Bar at
/// [`AppBarDensity::Regular`], in logical px.
///
/// Source: m3.material.io/components/top-app-bars/specs (also
/// material-components-android `docs/components/TopAppBar.md`). Deliberately
/// *not* upstream's own 72dp `smallHeight`/`collapsedHeight` — see the [module
/// docs](self)' metrics table for why, and
/// [`AppBarView::toolbar_height`](top::AppBarView::toolbar_height) for the
/// per-bar override that reaches it.
const HEIGHT: f64 = 64.0;

/// Horizontal inset from a bar's leading/trailing edges to the leading/action
/// slots, in logical px (upstream's `contentPadding`, 0 horizontal).
const PAD_X: f64 = 4.0;

/// Gap between adjacent slots (leading↔title, title↔actions, action↔action), in
/// logical px (upstream's `titleGap`, 8dp).
const GAP: f64 = 4.0;

/// Height reduction [`AppBarDensity::Compact`] applies to every band but the
/// bottom bar's — upstream's `M3EAppBarTheme.compactHeightReduction`.
const COMPACT_REDUCTION: f64 = 8.0;

/// Expanded band of the medium collapsing bar, in logical px —
/// `M3EAppBarTheme.mediumExpanded`, which is also the published M3
/// medium-top-app-bar height.
const MEDIUM_EXPANDED: f64 = 112.0;

/// Expanded band of the large collapsing bar, in logical px —
/// `M3EAppBarTheme.largeExpanded`, which is also the published M3
/// large-top-app-bar height.
const LARGE_EXPANDED: f64 = 152.0;

/// Bottom app bar band, in logical px — `M3EAppBarTheme.bottomHeight`, which is
/// also the published M3 bottom-app-bar height. Density-independent (see the
/// [module docs](self)).
const BOTTOM_HEIGHT: f64 = 80.0;

/// Start and bottom inset of a collapsing bar's *expanded* headline, in logical
/// px — upstream's `FlexibleSpaceBar(titlePadding: start 16, bottom 16, end 16)`.
const HEADLINE_INSET: f64 = 16.0;

/// The collapsed title's M3 `titleLargeEmphasized` type-scale token, hardcoded
/// here rather than read from a live `Theme::type_scale`: unlike a themed
/// *color* or font *family*, `Text` has no layout-time-deferred *size/weight*
/// resolution seam (see `docs/CODE_STANDARDS.md`'s Theming conventions — only
/// a color role and an opted-in family role are resolved after `View::build`;
/// the title opts into the family, see `title_text_view`). Matches
/// `frust-theme::typography`'s `TITLE_LARGE_EMPHASIZED`: the baseline
/// `TITLE_LARGE` size/line-height (m3.material.io) with the weight stepped
/// Regular → Medium.
const TITLE_SIZE: f32 = 22.0;
/// Line height of [`TITLE_SIZE`]'s type token, in logical px.
const TITLE_LINE_HEIGHT: f32 = 28.0;
/// Weight shared by both title tokens — the M3E *emphasized* type-scale
/// consumption (Regular → Medium).
const TITLE_WEIGHT: FontWeight = FontWeight::MEDIUM;

/// The expanded headline's M3 `headlineSmallEmphasized` type-scale token, the
/// style upstream's `M3EAppBarTheme.titleStyle(collapsed: false)` resolves.
/// Hardcoded for the reason [`TITLE_SIZE`] is.
const HEADLINE_SIZE: f32 = 24.0;
/// Line height of [`HEADLINE_SIZE`]'s type token, in logical px.
const HEADLINE_LINE_HEIGHT: f32 = 32.0;

/// Unthemed fallback container fill for a top/collapsing bar (a theme resolves
/// this from `colors.surface`).
const CONTAINER: Color = Color::from_rgb8(0xFF, 0xFF, 0xFF);

/// Unthemed fallback container fill for a bottom bar (a theme resolves this from
/// `colors.surface_container`) — matches [`crate::toolbar`]'s and
/// [`crate::navbar`]'s own `CONTAINER` constant exactly, the same M3 "surface
/// container" role.
const BOTTOM_CONTAINER: Color = Color::from_rgb8(0xF3, 0xED, 0xF7);

/// Unthemed fallback corner radius for [`AppBarShapeFamily::Round`], in logical
/// px — `crate::tokens::shape_scale()`'s own `small` value.
const ROUND_RADIUS: f64 = 8.0;

/// Which collapsing-bar layout [`sliver_app_bar`] renders —
/// `M3EAppBarVariant`. The variant influences only the collapsing bar; the
/// fixed [`app_bar`] always renders the collapsed, single-line layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AppBarVariant {
    /// A single-line bar with no expanded headline (upstream returns no
    /// `flexibleSpace` at all for this one).
    Small,
    /// A two-line bar whose headline expands beneath the action row.
    #[default]
    Medium,
    /// A taller two-line bar with a larger expanded headline.
    Large,
}

/// The corner shape family of an app bar container — `M3EAppBarShapeFamily`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AppBarShapeFamily {
    /// Rounded container corners (`theme.shape.small`, 8dp).
    Round,
    /// Squared container corners (`theme.shape.none`).
    #[default]
    Square,
}

/// The vertical density of an app bar — `M3EAppBarDensity`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AppBarDensity {
    /// Standard M3 heights.
    #[default]
    Regular,
    /// Heights reduced by 8dp (upstream's own `compactHeightReduction`) — the
    /// bottom bar excepted, per the [module docs](self).
    Compact,
}

/// The resolved band heights of one density — upstream's `M3EAppBarMetrics`,
/// as produced by `M3EAppBarTheme.metrics(density)`.
///
/// ```
/// use frust_material::{AppBarDensity, AppBarMetrics, AppBarVariant};
///
/// let regular = AppBarMetrics::for_density(AppBarDensity::Regular);
/// assert_eq!(regular.small_height, 64.0);
/// assert_eq!(regular.expanded(AppBarVariant::Large), 152.0);
///
/// // Compact takes 8dp off every band but the bottom bar's.
/// let compact = AppBarMetrics::for_density(AppBarDensity::Compact);
/// assert_eq!(compact.small_height, 56.0);
/// assert_eq!(compact.bottom_height, regular.bottom_height);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AppBarMetrics {
    /// The fixed top bar's band, in logical px.
    pub small_height: f64,
    /// The collapsing bar's fully-collapsed band, in logical px.
    pub collapsed_height: f64,
    /// The [`AppBarVariant::Medium`] expanded band, in logical px.
    pub medium_expanded: f64,
    /// The [`AppBarVariant::Large`] expanded band, in logical px.
    pub large_expanded: f64,
    /// The bottom bar's band, in logical px (density-independent).
    pub bottom_height: f64,
}

impl AppBarMetrics {
    /// The bands `density` resolves to.
    pub const fn for_density(density: AppBarDensity) -> Self {
        let cut = match density {
            AppBarDensity::Regular => 0.0,
            AppBarDensity::Compact => COMPACT_REDUCTION,
        };
        Self {
            small_height: HEIGHT - cut,
            collapsed_height: HEIGHT - cut,
            medium_expanded: MEDIUM_EXPANDED - cut,
            large_expanded: LARGE_EXPANDED - cut,
            bottom_height: BOTTOM_HEIGHT,
        }
    }

    /// The fully-expanded band of `variant`, in logical px —
    /// `_buildSliver`'s own `switch (variant)`.
    pub const fn expanded(&self, variant: AppBarVariant) -> f64 {
        match variant {
            AppBarVariant::Small => self.small_height,
            AppBarVariant::Medium => self.medium_expanded,
            AppBarVariant::Large => self.large_expanded,
        }
    }
}

/// The collapse geometry of one [`sliver_app_bar`] configuration: the pure
/// mapping between an app's scroll offset, the bar's collapse fraction, and the
/// band it occupies.
///
/// Every method is clamp-disciplined and NaN-safe — a non-finite input resolves
/// to the resting (expanded) end rather than propagating.
///
/// ```
/// use frust_material::{AppBarCollapse, AppBarDensity, AppBarVariant};
///
/// let geometry = AppBarCollapse::new(AppBarVariant::Large, AppBarDensity::Regular, true);
/// assert_eq!(geometry.expanded_height(), 152.0);
/// assert_eq!(geometry.collapsed_height(), 64.0);
///
/// // A pinned bar bottoms out at its collapsed band, 88px of scroll later.
/// assert_eq!(geometry.travel(), 88.0);
/// assert_eq!(geometry.height(0.0), 152.0);
/// assert_eq!(geometry.height(1.0), 64.0);
/// assert_eq!(geometry.fraction_for_offset(44.0), 0.5);
///
/// // Past the end, and off the map, both clamp.
/// assert_eq!(geometry.fraction_for_offset(1_000.0), 1.0);
/// assert_eq!(geometry.fraction_for_offset(f64::NAN), 0.0);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AppBarCollapse {
    expanded: f64,
    collapsed: f64,
    min: f64,
}

impl AppBarCollapse {
    /// The geometry of a `variant` bar at `density`, bottoming out at its
    /// collapsed band when `pinned` and at zero height otherwise.
    pub const fn new(variant: AppBarVariant, density: AppBarDensity, pinned: bool) -> Self {
        let metrics = AppBarMetrics::for_density(density);
        let collapsed = metrics.collapsed_height;
        Self {
            expanded: metrics.expanded(variant),
            collapsed,
            min: if pinned { collapsed } else { 0.0 },
        }
    }

    /// The band at collapse `0.0`, in logical px.
    pub const fn expanded_height(&self) -> f64 {
        self.expanded
    }

    /// The collapsed band — the top row's own height, in logical px. A pinned
    /// bar bottoms out here; an unpinned one keeps shrinking past it.
    pub const fn collapsed_height(&self) -> f64 {
        self.collapsed
    }

    /// The band at collapse `1.0`, in logical px: the collapsed band when
    /// pinned, `0.0` otherwise.
    pub const fn min_height(&self) -> f64 {
        self.min
    }

    /// The scroll distance, in logical px, that carries this bar from fully
    /// expanded to fully collapsed — never negative.
    pub fn travel(&self) -> f64 {
        (self.expanded - self.min).max(0.0)
    }

    /// The collapse fraction a scroll `offset` (a [`ScrollInfo`](frust::ScrollInfo)'s
    /// own clamped `offset`) maps to, in `[0, 1]`.
    ///
    /// A bar with no travel at all ([`AppBarVariant::Small`], pinned) reports
    /// `1.0` for any positive offset and `0.0` at rest — it has no expanded
    /// state to leave, so the answer is only ever cosmetic.
    pub fn fraction_for_offset(&self, offset: f64) -> f64 {
        let travel = self.travel();
        if travel <= 0.0 {
            return if offset > 0.0 { 1.0 } else { 0.0 };
        }
        clamp01(offset / travel)
    }

    /// The band this bar occupies at collapse fraction `collapse`, in logical
    /// px — linear between [`Self::expanded_height`] and [`Self::min_height`].
    pub fn height(&self, collapse: f64) -> f64 {
        let t = clamp01(collapse);
        self.expanded + (self.min - self.expanded) * t
    }

    /// How far the *headline* has collapsed at collapse fraction `collapse`, in
    /// `[0, 1]`: `0.0` is the expanded headline (bottom-anchored,
    /// `headlineSmall`), `1.0` the collapsed one (in the top row, `titleLarge`).
    ///
    /// This is not [`Self::height`]'s own fraction: an unpinned bar keeps
    /// shrinking after the headline has fully collapsed, so the headline
    /// saturates first.
    pub fn headline(&self, collapse: f64) -> f64 {
        let span = self.expanded - self.collapsed;
        if span <= 0.0 {
            return 1.0;
        }
        clamp01((self.expanded - self.height(collapse)) / span)
    }
}

/// `value` clamped into `[0, 1]`, mapping NaN to the resting `0.0` rather than
/// propagating it into a layout.
fn clamp01(value: f64) -> f64 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, 1.0)
    }
}

/// Linear interpolation from `a` to `b` at an already-clamped `t`.
fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// `constraint` if finite, `0.0` otherwise — the degenerate unbounded-width
/// case every full-width bar in this family resolves the same way
/// ([`crate::toolbar`]'s docked variant included).
fn finite_or_zero(constraint: f64) -> f64 {
    if constraint.is_finite() {
        constraint
    } else {
        0.0
    }
}

/// A bar's title: either a string this family owns and styles, or a caller view
/// it merely places (upstream's `titleText` / `title` pair).
pub(crate) enum TitleSlot<State: 'static> {
    /// A string composed into a styled child [`frust::TextView`].
    Text(String),
    /// A caller-supplied view, placed but never styled.
    View(AnyView<State>),
}

impl<State: 'static> TitleSlot<State> {
    /// The semantics label this slot contributes on its own — a non-empty
    /// title string, or nothing for a caller view (which labels itself).
    fn label(&self) -> Option<&str> {
        match self {
            TitleSlot::Text(title) if !title.is_empty() => Some(title.as_str()),
            _ => None,
        }
    }
}

/// Build the title's type-erased child view at `size`/`line_height`: emphasized
/// title/headline text defaulting to the `Text` widget's own `OnSurface` themed
/// role (an app bar title reads as ordinary on-surface content), ellipsized to
/// one line like upstream's own `titleText`.
///
/// Its font family follows the live theme's `titleLargeEmphasized` role — the
/// collapsed, resting role — and by design that one family governs a sliver
/// bar's whole collapse. The sliver interpolates the title's size AND line
/// height between `headlineSmallEmphasized` (expanded) and
/// `titleLargeEmphasized` (collapsed) at build time, but a family cannot
/// interpolate, and switching faces mid-collapse would make the title visibly
/// jump. Under a type scale whose headline and title families differ — Glyph's
/// (Space Mono headline, IBM Plex Mono title) or Cupertino's (SF Pro Display
/// at 20pt and up, SF Pro Text below) — the expanded headline therefore
/// renders in the title face. The Material scale gives both roles Roboto Flex.
fn title_text_view<State: 'static>(title: &str, size: f32, line_height: f32) -> AnyView<State> {
    frust::authoring::any::<State, _>(
        text(title.to_string())
            .size(size)
            .weight(TITLE_WEIGHT)
            .line_height(LineHeight::Absolute(line_height))
            .max_lines(1)
            .overflow(TextOverflow::Ellipsis)
            .themed_family(ThemeTextType::TitleLargeEmphasized),
    )
}

/// Materialize `slot` into a view reference, using `storage` to hold the text
/// case's freshly-composed view for the caller's borrow.
///
/// The two title cases have to reach [`frust::authoring::rebuild_children`] as
/// one uniform `&AnyView` list, and only the string case has a view to compose;
/// this is the one-slot storage that lets both spell it the same way.
fn title_ref<'a, State: 'static>(
    slot: &'a TitleSlot<State>,
    storage: &'a mut Option<AnyView<State>>,
    size: f32,
    line_height: f32,
) -> &'a AnyView<State> {
    match slot {
        TitleSlot::Text(title) => {
            storage.insert(title_text_view::<State>(title, size, line_height))
        }
        TitleSlot::View(view) => view,
    }
}

/// The resolved container fill of a top/collapsing bar. Precedence is the
/// builder's own `background` (applied by the caller), then
/// `colors.surface`, then the [`CONTAINER`] fallback.
fn resolve_container(theme: Option<&Theme>) -> Color {
    match theme {
        Some(theme) => theme.scheme().surface,
        None => CONTAINER,
    }
}

/// The resolved container fill of a bottom bar — `colors.surface_container`
/// (upstream's `bottomBackgroundColor`), falling back to [`BOTTOM_CONTAINER`].
fn resolve_bottom_container(theme: Option<&Theme>) -> Color {
    match theme {
        Some(theme) => theme.scheme().surface_container,
        None => BOTTOM_CONTAINER,
    }
}

/// The resolved corner radius for `family` against a bar of `width`×`height`.
/// Themed: `shape.small` (round) / `shape.none` (square). Unthemed: the
/// equivalent literals, resolved the same way.
fn resolve_radius(
    theme: Option<&Theme>,
    family: AppBarShapeFamily,
    width: f64,
    height: f64,
) -> f64 {
    let token = match (theme, family) {
        (Some(theme), AppBarShapeFamily::Round) => theme.shape.small,
        (Some(theme), AppBarShapeFamily::Square) => theme.shape.none,
        (None, AppBarShapeFamily::Round) => ROUND_RADIUS,
        (None, AppBarShapeFamily::Square) => 0.0,
    };
    ShapeScale::resolve(token, width, height)
}

/// Fill a bar's container: a plain rect at a zero radius (so a square bar stays
/// one `fill_rect`, the shape every recorder scene observes), a rounded rect
/// otherwise.
fn fill_container(
    scene: &mut dyn frust::authoring::PaintScene,
    origin: kurbo::Point,
    size: kurbo::Size,
    radius: f64,
    color: Color,
) {
    if radius > 0.0 {
        scene.fill_rounded_rect(origin, size, radius, color);
    } else {
        scene.fill_rect(origin, size, color);
    }
}

/// Push the family's semantics container for `role`, labelled with `label` when
/// there is one, forwarding every slot pod in visual order.
fn push_bar_semantics(
    ctx: &mut frust::authoring::SemanticsCtx,
    role: Role,
    label: Option<&str>,
    slots: &[ChildPod],
) {
    ctx.push_container(
        role,
        |node| {
            if let Some(label) = label {
                node.set_label(label);
            }
        },
        |ctx| {
            for pod in slots {
                pod.semantics_child(ctx);
            }
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ported metric table, pinned against `M3EAppBarTheme`'s own defaults
    /// (and, where this port diverges, against the published M3 spec — see the
    /// module docs' metrics table).
    #[test]
    fn the_ported_metrics_match_the_reference_theme_defaults() {
        assert_eq!(MEDIUM_EXPANDED, 112.0);
        assert_eq!(LARGE_EXPANDED, 152.0);
        assert_eq!(BOTTOM_HEIGHT, 80.0);
        assert_eq!(COMPACT_REDUCTION, 8.0);
        assert_eq!(HEADLINE_INSET, 16.0);
        // The one deliberate divergence: upstream's own band is 72.
        assert_eq!(HEIGHT, 64.0);
    }

    /// The metrics [`crate::selection_app_bar`] mirrors verbatim (its
    /// `APP_BAR_HEIGHT`/`APP_BAR_PAD_X`/`APP_BAR_GAP`), so the contextual bar
    /// and an idle one keep reading as one continuous surface.
    #[test]
    fn the_selection_bar_coupling_metrics_are_unchanged() {
        assert_eq!(HEIGHT, 64.0);
        assert_eq!(PAD_X, 4.0);
        assert_eq!(GAP, 4.0);
        assert_eq!(TITLE_SIZE, 22.0);
        assert_eq!(TITLE_LINE_HEIGHT, 28.0);
        assert_eq!(TITLE_WEIGHT, FontWeight::MEDIUM);
    }

    #[test]
    fn compact_density_reduces_every_band_but_the_bottom_one() {
        let regular = AppBarMetrics::for_density(AppBarDensity::Regular);
        let compact = AppBarMetrics::for_density(AppBarDensity::Compact);
        assert_eq!(
            compact.small_height,
            regular.small_height - COMPACT_REDUCTION
        );
        assert_eq!(
            compact.collapsed_height,
            regular.collapsed_height - COMPACT_REDUCTION
        );
        assert_eq!(
            compact.medium_expanded,
            regular.medium_expanded - COMPACT_REDUCTION
        );
        assert_eq!(
            compact.large_expanded,
            regular.large_expanded - COMPACT_REDUCTION
        );
        assert_eq!(compact.bottom_height, regular.bottom_height);
    }

    #[test]
    fn each_variant_expands_to_its_own_band() {
        let metrics = AppBarMetrics::for_density(AppBarDensity::Regular);
        assert_eq!(metrics.expanded(AppBarVariant::Small), HEIGHT);
        assert_eq!(metrics.expanded(AppBarVariant::Medium), MEDIUM_EXPANDED);
        assert_eq!(metrics.expanded(AppBarVariant::Large), LARGE_EXPANDED);
    }

    #[test]
    fn a_pinned_collapse_bottoms_out_at_the_collapsed_band() {
        let geometry = AppBarCollapse::new(AppBarVariant::Medium, AppBarDensity::Regular, true);
        assert_eq!(geometry.min_height(), geometry.collapsed_height());
        assert_eq!(geometry.height(0.0), MEDIUM_EXPANDED);
        assert_eq!(geometry.height(1.0), HEIGHT);
        assert_eq!(geometry.height(0.5), (MEDIUM_EXPANDED + HEIGHT) / 2.0);
        // The headline saturates exactly when the band does, since a pinned
        // bar's travel *is* the headline span.
        assert_eq!(geometry.headline(1.0), 1.0);
        assert_eq!(geometry.headline(0.0), 0.0);
    }

    #[test]
    fn an_unpinned_collapse_scrolls_away_after_the_headline_saturates() {
        let geometry = AppBarCollapse::new(AppBarVariant::Medium, AppBarDensity::Regular, false);
        assert_eq!(geometry.min_height(), 0.0);
        assert_eq!(geometry.height(1.0), 0.0);
        // The headline is fully collapsed once the band reaches the collapsed
        // height — 48 of the 112px travel remains after that.
        let t_at_collapsed = (MEDIUM_EXPANDED - HEIGHT) / MEDIUM_EXPANDED;
        assert!((geometry.headline(t_at_collapsed) - 1.0).abs() < 1e-9);
        assert_eq!(geometry.headline(1.0), 1.0);
    }

    #[test]
    fn a_small_variant_has_no_headline_travel_at_all() {
        let geometry = AppBarCollapse::new(AppBarVariant::Small, AppBarDensity::Regular, true);
        assert_eq!(geometry.travel(), 0.0);
        assert_eq!(geometry.headline(0.0), 1.0);
        assert_eq!(geometry.fraction_for_offset(0.0), 0.0);
        assert_eq!(geometry.fraction_for_offset(10.0), 1.0);
        // The band never moves, whatever the fraction says.
        assert_eq!(geometry.height(0.0), HEIGHT);
        assert_eq!(geometry.height(1.0), HEIGHT);
    }

    #[test]
    fn every_collapse_input_is_clamped_and_nan_safe() {
        let geometry = AppBarCollapse::new(AppBarVariant::Large, AppBarDensity::Regular, true);
        assert_eq!(geometry.height(-5.0), LARGE_EXPANDED);
        assert_eq!(geometry.height(5.0), HEIGHT);
        assert_eq!(geometry.height(f64::NAN), LARGE_EXPANDED);
        assert_eq!(geometry.headline(f64::NAN), 0.0);
        assert_eq!(geometry.fraction_for_offset(-100.0), 0.0);
        assert_eq!(geometry.fraction_for_offset(f64::NAN), 0.0);
        assert_eq!(geometry.fraction_for_offset(f64::INFINITY), 1.0);
        assert_eq!(geometry.fraction_for_offset(f64::NEG_INFINITY), 0.0);
    }

    #[test]
    fn clamp_and_lerp_helpers_hold_their_contract() {
        assert_eq!(clamp01(f64::NAN), 0.0);
        assert_eq!(clamp01(-1.0), 0.0);
        assert_eq!(clamp01(2.0), 1.0);
        assert_eq!(lerp(10.0, 20.0, 0.25), 12.5);
        assert_eq!(finite_or_zero(f64::INFINITY), 0.0);
        assert_eq!(finite_or_zero(f64::NAN), 0.0);
        assert_eq!(finite_or_zero(320.0), 320.0);
    }

    #[test]
    fn the_shape_families_resolve_their_own_tokens() {
        let theme = crate::baseline();
        assert_eq!(
            resolve_radius(Some(&theme), AppBarShapeFamily::Square, 400.0, 64.0),
            0.0
        );
        assert_eq!(
            resolve_radius(Some(&theme), AppBarShapeFamily::Round, 400.0, 64.0),
            theme.shape.small
        );
        // Unthemed, the same pair of literals.
        assert_eq!(
            resolve_radius(None, AppBarShapeFamily::Round, 400.0, 64.0),
            ROUND_RADIUS
        );
        assert_eq!(
            resolve_radius(None, AppBarShapeFamily::Square, 400.0, 64.0),
            0.0
        );
    }
}
