// Ported from `material_3_expressive` v1.0.8's search family (MIT, © 2026 Paa
// Developments; `tmp/material_3_expressive/lib/components/search/` —
// `m3e_search_bar.dart`, `m3e_search_anchor.dart`,
// `components/m3e_search_bar_build.dart`, `components/m3e_search_bar_input.dart`,
// `components/m3e_search_view.dart`, `components/m3e_search_view_build.dart`,
// `components/m3e_search_view_route.dart`, `controllers/m3e_search_controller.dart`,
// `res/m3e_search_constants.dart`, `styles/m3e_search_bar_theme.dart`,
// `styles/m3e_search_view_theme.dart`, retrieved 2026-08-20).
// Upstream: <https://github.com/paadevelopments/material_3_expressive>
//
// Porting decisions (each also documented, with its reference site, in the
// module docs below): the presentation split is resolved by **window width**
// rather than by the reference's `TargetPlatform` switch; the bar is
// display-only (the reference's own `M3ESearchAnchor.bar` shape) so its M3
// container role survives; the view's header field is the framework's baseline
// editable, which carries its own opaque fill; the reference's transparent
// default divider is replaced by a visible one; and the anchor's fade-out is
// dropped because the docked panel covers the anchor rect outright.

//! The M3 Expressive **search** family: the search bar (a pill-shaped field
//! affordance) and the search view it opens (a full-screen route on compact
//! widths, an anchored panel on expanded ones).
//!
//! - [`mod@bar`] — [`search_bar`], the pill: leading icon, hint-or-query text,
//!   trailing actions, a press-driven expand spring, and a tap that opens the
//!   view.
//! - [`mod@view`] — [`search_view`], the view's shared content (edit-field
//!   header with a back affordance, divider, caller-supplied suggestion slot)
//!   plus the two presentations it terminates in:
//!   [`SearchView::full_screen`] (a modal-host page) and
//!   [`SearchView::docked`] (an anchored panel over the bar).
//!
//! # The presentation split, and the rule that picks it
//!
//! The reference picks its presentation from the **platform**, not from the
//! window: `_M3ESearchAnchorState._showFullScreenView()`
//! (`m3e_search_anchor.dart`) is
//!
//! ```text
//! widget.isFullScreen ?? switch (M3ETheme.platformOf(context)) {
//!   TargetPlatform.iOS || TargetPlatform.android || TargetPlatform.fuchsia => true,
//!   TargetPlatform.macOS || TargetPlatform.linux || TargetPlatform.windows => false,
//! }
//! ```
//!
//! — i.e. full-screen on mobile, docked on desktop. This port substitutes M3's
//! own **window-size-class** breakpoint: below
//! [`SEARCH_VIEW_COMPACT_MAX_WIDTH`] (600dp, the compact class's upper bound)
//! the view is full-screen, at or above it the view is docked
//! ([`SearchViewMode::for_width`]). Two reasons, in order:
//!
//! 1. A design-system plugin has no `TargetPlatform` to switch on —
//!    [`frust::Theme`]'s `design_language` names the *catalog*, not the host OS,
//!    and a catalog that guessed the OS from it would report "Material ⇒
//!    Android" on a Material-themed desktop app.
//! 2. The width rule reproduces the reference's outcome on real hardware
//!    anyway (a phone is compact, a desktop window is not) **and** additionally
//!    tracks a resized desktop window and a foldable, which the platform switch
//!    cannot. The reference itself already re-closes an open docked view when
//!    the screen size changes (`didChangeDependencies`), which is that same
//!    concern reached from the other side.
//!
//! The rule is a plain function over a width so an app can apply it wherever
//! it learns one — typically `use_context::<`[`frust::WindowMetrics`]`>()` in
//! `Component::build`, whose `size.width` is exactly the logical width this
//! breakpoint is expressed in. Both presentations stay reachable explicitly, so
//! an app that wants one unconditionally (the reference's `isFullScreen`
//! override) just calls that constructor.
//!
//! # Why the split is two constructors rather than one adaptive widget
//!
//! The two presentations are not two layouts of one widget: the full-screen
//! view is a **navigator route** (pushed through
//! [`crate::overlay::show_overlay_modal`], so it carries a
//! [`frust::BackPolicy`] and a staged exit ramp), while the docked view is a
//! **mounted panel** (an [`crate::overlay::anchored_overlay`] host the app
//! keeps mounted and toggles with `.open(bool)`). Only an app can push a route,
//! so the choice is necessarily made where the push is — not inside a widget's
//! `layout`. The width rule above is therefore a function the app applies, and
//! each presentation is its own view type.
//!
//! # Geometry
//!
//! Both presentations share [`mod@view`]'s content widget; the mode selects the
//! header band and the panel chrome:
//!
//! | | Full-screen | Docked |
//! |---|---|---|
//! | Panel | the whole area | [`place_anchored`](crate::overlay::place_anchored) over the bar's own rect |
//! | Width | the area's | `clamp(anchor.width, `[`SEARCH_VIEW_MIN_WIDTH`]`, area.width)` |
//! | Height | the area's | `clamp(area.height × 2⁄3, `[`SEARCH_VIEW_MIN_HEIGHT`]`, area.height)` |
//! | Header | [`FULL_SCREEN_HEADER_HEIGHT`] (72dp), 16/8dp inset | [`SEARCH_BAR_MIN_HEIGHT`] (56dp), 8dp inset |
//! | Corners | square | [`frust::Theme`]`.shape.extra_large` (28dp) |
//! | Chrome owner | the modal host | the content itself (the anchored host paints none) |
//!
//! The docked width/height rule is `M3ESearchViewRoute._updateTweens`'
//! (`m3e_search_view_route.dart`) verbatim — `clampDouble(anchorRect.width,
//! minWidth, maxWidth)` and `clampDouble(screenSize.height * 2 / 3, minHeight,
//! maxHeight)`, with the reference's own shift-back-on-screen step supplied by
//! [`crate::overlay::place_anchored`]'s `clamp`.
//!
//! # Fidelity decisions
//!
//! - **The bar is display-only.** The reference's `M3ESearchBar` embeds a live
//!   `EditableText`, but the shape every anchor uses —
//!   `M3ESearchAnchor.bar`/`_M3ESearchAnchorBarState` — passes `readOnly: true`
//!   with a `canRequestFocus: false` focus node, because *the view's* field
//!   owns editing. This port ships that shape only: the framework's baseline
//!   editable paints its own opaque `surface` background with no seam to
//!   suppress it, so a bar that embedded one would lose the
//!   `surfaceContainerHigh` pill that is the component's whole identity — see
//!   [`crate::text_field`]'s *Container fill* note for the same seam limit
//!   reached from the other side. Editing lives in the view, exactly as the
//!   reference's anchor bar intends.
//! - **The view's header field band paints the baseline editable's own
//!   `surface` fill**, not the view container's role, for that same reason: the
//!   header's editable spans the full header band so the fill reads as one
//!   uniform field band above the divider rather than as a patch behind the
//!   glyphs.
//! - **The container role is `surfaceContainerHigh` in *both* modes.** The
//!   reference paints a full-screen view in `scheme.surface`
//!   (`M3ESearchViewTheme.fullScreenBackgroundColor`) and a docked one in
//!   `scheme.surfaceContainerHigh`; M3's published search-view container role
//!   is `surfaceContainerHigh` for both, which is what this port uses.
//! - **The divider is visible by default.** `_resolveViewStyles`
//!   (`m3e_search_view_build.dart`) resolves `widget.dividerColor ??
//!   Colors.transparent`, so the reference's own default divider is invisible
//!   unless a caller passes a color; this port always draws
//!   [`crate::divider`]'s `outlineVariant` hairline between the header and the
//!   suggestions, which is what M3 itself shows.
//! - **The anchor does not fade.** `_M3ESearchAnchorState.build` wraps the
//!   trigger in an `AnimatedOpacity` that fades it to `0` while the view is
//!   open; the docked panel here covers the anchor's own rect outright (its
//!   top-left *is* the anchor's, per the table above) and is opaque, so there
//!   is nothing to fade out from under it.
//! - **The open transition is the host's ramp, not a rect tween.** The
//!   reference animates a `RectTween` from the anchor's rect to the view's over
//!   600ms `easeInOutCubicEmphasized`, with four staged interval fades layered
//!   on top (`M3ESearchConstants.view*FadeOnInterval`). Neither overlay host
//!   exposes a per-child geometry tween, so each mode takes its host's own
//!   entrance instead: the full-screen page slides down from the top edge
//!   ([`crate::overlay::OverlayEntrance::Slide`], the only entrance an
//!   edge-pinned panel's progress-driven geometry can take — see
//!   [`mod@view`]'s `full_screen_config`), the docked panel fades and scales
//!   toward the anchor ([`crate::overlay::anchored_overlay`]'s ramp). The bar's
//!   own focus-expand spring *is* ported ([`mod@bar`]).
//!
//! # IME: no programmatic focus
//!
//! The reference opens its view with `autoFocus: true` on the header field, so
//! the keyboard is up before the user touches anything. The framework's
//! baseline editable exposes **no programmatic-focus seam** — `TextInputView`
//! has no `autofocus`/`request_focus` builder, and `EventCtx::request_focus` is
//! reachable only from a widget's own event pass, which never runs for a field
//! nobody has touched yet. A user therefore taps the view's field once before
//! the keyboard appears. This is the same framework gap
//! [`mod@crate::overlay::modal`] records for `Escape` ("there is no
//! auto-focus-on-appear hook in the framework"), and it is not patched around
//! here.
//!
//! # Symbol surface
//!
//! Everything below, plus [`mod@bar`]'s and [`mod@view`]'s types and
//! constructors, is flat re-exported at the crate root like every other
//! component here. [`bar::EXPAND_REST`]/[`bar::EXPAND_ACTIVE`] are the one
//! carve-out and stay namespaced (`frust_material::search::bar::EXPAND_REST`):
//! they are one component's motion tuning rather than catalog surface — the
//! same carve-out [`crate::overlay`]'s token accessors take.

use frust::authoring::text::LineHeight;

pub mod bar;
pub mod view;

pub use bar::{SearchBarView, SearchBarWidget, search_bar};
pub use view::{
    DockedSearchViewView, SearchView, SearchViewContentWidget, SearchViewView, search_view,
    show_search_view,
};

/// The search bar's minimum height, in logical px — `M3ESearchBarTheme`'s own
/// `minHeight` default (56dp), and the M3 search-bar spec height. Doubles as
/// the **docked** view's header-band height, the same way the reference falls
/// back to `theme.searchBarTheme.minHeight` for a docked header
/// (`m3e_search_view_build.dart`'s `headerBlockHeight`).
pub const SEARCH_BAR_MIN_HEIGHT: f64 = 56.0;

/// The search bar's minimum width, in logical px — `M3ESearchBarTheme`'s own
/// `minWidth` default (360dp).
///
/// The reference applies it through a `ConstrainedBox(minWidth: 360)`, which
/// *overflows* a narrower parent; this port clamps it down to whatever width it
/// is offered instead (the same narrower-viewport clamp
/// [`crate::side_sheet`]'s fixed width takes).
pub const SEARCH_BAR_MIN_WIDTH: f64 = 360.0;

/// The docked search view's minimum width, in logical px —
/// `M3ESearchViewTheme`'s own `minWidth` default (360dp).
pub const SEARCH_VIEW_MIN_WIDTH: f64 = 360.0;

/// The docked search view's minimum height, in logical px —
/// `M3ESearchViewTheme`'s own `minHeight` default (240dp).
pub const SEARCH_VIEW_MIN_HEIGHT: f64 = 240.0;

/// The fraction of the window height a docked search view takes before its
/// [`SEARCH_VIEW_MIN_HEIGHT`] floor applies — `M3ESearchViewRoute._updateTweens`'
/// own `screenSize.height * 2 / 3`.
pub const SEARCH_VIEW_HEIGHT_FRACTION: f64 = 2.0 / 3.0;

/// The full-screen view's header-band height, in logical px —
/// `M3ESearchConstants.fullScreenBarHeight` / `M3ESearchViewTheme.headerHeight`
/// (both 72dp).
pub const FULL_SCREEN_HEADER_HEIGHT: f64 = 72.0;

/// The exclusive upper bound of M3's **compact** window-size class, in logical
/// px: a window narrower than this presents the search view full-screen, one at
/// least this wide presents it docked. See the [module docs](self)' presentation
/// split for why this replaces the reference's `TargetPlatform` switch.
pub const SEARCH_VIEW_COMPACT_MAX_WIDTH: f64 = 600.0;

/// Body-large glyph size, in logical px — the type role the reference's bar and
/// view header both read (`M3ESearchBarTheme.textStyle`/`hintStyle` and
/// `M3ESearchViewTheme.headerTextStyle`/`headerHintStyle` are all
/// `type.bodyLarge`).
///
/// Hardcoded rather than read from a live [`frust::Theme`]'s type scale for the
/// reason [`crate::side_sheet`]'s title metrics are: `Text` resolves only a
/// *color* role after `View::build`, never a size.
pub(crate) const BODY_LARGE_SIZE: f32 = 16.0;

/// Body-large line height, in logical px (M3 `bodyLarge`, 16/24).
pub(crate) const BODY_LARGE_LINE_HEIGHT: LineHeight = LineHeight::Absolute(24.0);

/// A slot wide enough for one M3 48dp touch target — what the reference's
/// `_wrapActionSlot` (`m3e_search_bar_build.dart`) reserves per leading/trailing
/// action, and the width both the bar and the view header lay their action
/// slots out in.
pub(crate) const ACTION_SLOT: f64 = 48.0;

/// The bar's and the docked header's horizontal inset —
/// `M3ESearchBarTheme.horizontalPadding` / `M3ESearchViewTheme.barHorizontalPadding`
/// (both 8dp).
pub(crate) const BAR_HORIZONTAL_PADDING: f64 = 8.0;

/// The opacity a disabled bar composites at —
/// `M3ESearchConstants.disabledOpacity` (0.38).
pub(crate) const DISABLED_OPACITY: f32 = 0.38;

/// Which presentation a search view takes. See the [module docs](self)'
/// presentation split.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SearchViewMode {
    /// A full-screen navigator route — the compact-width presentation, and the
    /// reference's mobile one.
    #[default]
    FullScreen,
    /// An anchored panel over the bar — the expanded-width presentation, and
    /// the reference's desktop one.
    Docked,
}

impl SearchViewMode {
    /// The presentation for a window `width` in logical px: [`Self::FullScreen`]
    /// below [`SEARCH_VIEW_COMPACT_MAX_WIDTH`], [`Self::Docked`] at or above it.
    ///
    /// ```
    /// use frust_material::{SearchViewMode, SEARCH_VIEW_COMPACT_MAX_WIDTH};
    ///
    /// assert_eq!(SearchViewMode::for_width(392.0), SearchViewMode::FullScreen);
    /// assert_eq!(SearchViewMode::for_width(1280.0), SearchViewMode::Docked);
    /// // The boundary itself is already expanded.
    /// assert_eq!(
    ///     SearchViewMode::for_width(SEARCH_VIEW_COMPACT_MAX_WIDTH),
    ///     SearchViewMode::Docked,
    /// );
    /// ```
    pub fn for_width(width: f64) -> Self {
        if width < SEARCH_VIEW_COMPACT_MAX_WIDTH {
            SearchViewMode::FullScreen
        } else {
            SearchViewMode::Docked
        }
    }

    /// Whether this is the full-screen presentation.
    pub fn is_full_screen(self) -> bool {
        matches!(self, SearchViewMode::FullScreen)
    }

    /// This mode's header-band height, in logical px: 72dp full-screen
    /// (`M3ESearchConstants.fullScreenBarHeight`), 56dp docked (the reference's
    /// `theme.searchBarTheme.minHeight` fallback).
    pub(crate) fn header_height(self) -> f64 {
        if self.is_full_screen() {
            FULL_SCREEN_HEADER_HEIGHT
        } else {
            SEARCH_BAR_MIN_HEIGHT
        }
    }

    /// This mode's header inset as `(horizontal, vertical)`, in logical px —
    /// `M3ESearchViewTheme.fullScreenHeaderPadding` (16/8) full-screen,
    /// `barPadding` (8/0) docked.
    pub(crate) fn header_padding(self) -> (f64, f64) {
        if self.is_full_screen() {
            (16.0, 8.0)
        } else {
            (BAR_HORIZONTAL_PADDING, 0.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_breakpoint_splits_compact_from_expanded() {
        // A phone, a small tablet in portrait, a desktop window.
        assert_eq!(SearchViewMode::for_width(360.0), SearchViewMode::FullScreen);
        assert_eq!(SearchViewMode::for_width(599.9), SearchViewMode::FullScreen);
        assert_eq!(SearchViewMode::for_width(600.0), SearchViewMode::Docked);
        assert_eq!(SearchViewMode::for_width(1440.0), SearchViewMode::Docked);
    }

    #[test]
    fn a_degenerate_width_still_answers_full_screen() {
        // A zero/negative width is the unbounded-constraint degenerate case
        // (`finite_or_zero`); the compact answer is the safe one — it needs no
        // anchor rect to place against.
        assert_eq!(SearchViewMode::for_width(0.0), SearchViewMode::FullScreen);
        assert_eq!(SearchViewMode::for_width(-1.0), SearchViewMode::FullScreen);
    }

    #[test]
    fn each_mode_carries_its_own_header_band() {
        assert_eq!(
            SearchViewMode::FullScreen.header_height(),
            FULL_SCREEN_HEADER_HEIGHT
        );
        assert_eq!(
            SearchViewMode::Docked.header_height(),
            SEARCH_BAR_MIN_HEIGHT
        );
        assert_eq!(SearchViewMode::FullScreen.header_padding(), (16.0, 8.0));
        assert_eq!(
            SearchViewMode::Docked.header_padding(),
            (BAR_HORIZONTAL_PADDING, 0.0)
        );
    }

    #[test]
    fn the_ported_metrics_match_the_reference_theme_defaults() {
        // `M3ESearchBarTheme` / `M3ESearchViewTheme` / `M3ESearchConstants`.
        assert_eq!(SEARCH_BAR_MIN_HEIGHT, 56.0);
        assert_eq!(SEARCH_BAR_MIN_WIDTH, 360.0);
        assert_eq!(SEARCH_VIEW_MIN_WIDTH, 360.0);
        assert_eq!(SEARCH_VIEW_MIN_HEIGHT, 240.0);
        assert_eq!(FULL_SCREEN_HEADER_HEIGHT, 72.0);
        assert_eq!(BAR_HORIZONTAL_PADDING, 8.0);
        assert_eq!(DISABLED_OPACITY, 0.38);
        assert!((SEARCH_VIEW_HEIGHT_FRACTION - 2.0 / 3.0).abs() < f64::EPSILON);
    }
}
