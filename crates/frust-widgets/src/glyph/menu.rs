//! Glyph anchored dropdown menu: the framework's
//! first anchored popup — a small chooser that scales in from an anchor's own
//! corner rather than the screen center, the Glyph recipe over the existing
//! navigator transparent-push modal plumbing. The Glyph design system's
//! "overflow menu" section (retrieved 2026-07-23) is the primary source:
//! the AppBar's overflow kebab is its first consumer,
//! but the API is anchor-agnostic — any widget that can report
//! its own painted window-coordinate rect can open one.
//!
//! # Navigator-modal architecture (reuse, don't fork)
//!
//! Like [`crate::glyph::dialog`]/[`crate::glyph::command_palette`], a
//! [`GlyphMenuView`] is pushed as a **transparent navigator page** via
//! [`NavigatorController::push_with_options`] — the page below stays visible
//! under a fully transparent scrim (nothing is painted for it; only the
//! event pass treats "outside the panel" as a dismiss gesture — a dropdown
//! menu does not dim the screen behind it, unlike a dialog's opaque barrier).
//! [`show_glyph_menu`] wraps the push and wires dismissal/selection to
//! `controller.pop()`/`controller.pop_with_result(..)`. This module edits
//! **no** nav file: it consumes `push_with_options`/`pop`/`pop_with_result`
//! read-only.
//!
//! # Anchoring (v1: caller-supplied window-coordinate rect)
//!
//! [`GlyphMenuView::anchor`] takes a `kurbo::Rect` **in window coordinates**
//! — the same absolute space `ctx.origin()`/`ctx.size()` describe for a
//! transparent full-page widget (this is also the space every other
//! transparent-page overlay in this catalog already computes its own
//! full-page layout in, so no coordinate conversion is needed at the push
//! site). The caller is responsible for capturing that rect itself (e.g. an
//! `AppBar`'s trailing icon records its own painted bounds during `paint`,
//! mirroring how `HeroFrames` report bounds via paint). There is no
//! ancestor-bounds query a widget can make mid-layout
//! (the layout protocol gives a widget no visibility upward), so this
//! caller-reports-its-own-rect shape is the simplest anchoring contract that
//! works with the existing paint pipeline; a future addition could add an
//! automatic bounds-reporting channel, but that is out of scope here.
//!
//! The menu positions **below-trailing** the anchor by default: its trailing
//! (right) edge aligns with the anchor's trailing edge, and its top edge
//! sits just below the anchor's bottom edge — the standard "kebab in the
//! corner" shape the design system depicts. The whole panel is then clamped
//! inside the window bounds with an 8px margin on every side (an anchor near
//! a window edge — e.g. a kebab icon flush against the trailing edge — would
//! otherwise paint the panel partially off-window). The scale
//! transform-origin is pinned to the panel's own top-right corner
//! (`transform-origin:top right` in the design system's CSS) — a v1
//! simplification matching the fixed "AppBar kebab in the top-right" shape
//! the design system depicts; a future consumer opening a menu from a
//! bottom/leading anchor would want a dynamically-chosen corner, but nothing
//! in this catalog needs that yet.
//!
//! # Enter/exit staging + item stagger (widget-internal, not a navigator transition)
//!
//! Mirrors [`crate::glyph::dialog`]'s rationale: the panel drives its own
//! enter/exit animation from `PaintCtx::frame_time` rather than riding a
//! whole-page [`PageTransition`](crate::nav::transition::PageTransition), so
//! it is pushed with [`TransitionSpec::NONE`] — the navigator gives the
//! modal contract, the widget gives the motion. The panel **scales**
//! `0.85 → 1.0` about its top-right corner while sliding down from `-8px` to
//! `0px` (the design system: `transform:scale(0.85) translateY(-8px)` →
//! `scale(1) translateY(0)`); unlike the dialog/palette, the panel's own
//! fill/border/shadow are **never alpha-faded** — only geometry animates,
//! matching this catalog's established modal-panel precedent (dialog/palette
//! panels are likewise opacity-stable, animating scale only). Exit reverses
//! over a faster duration ("exits always faster than
//! entrances", the same rule dialog/palette apply).
//!
//! Independently, each **item** fades/slides in on its own staggered
//! sub-timeline ("items stagger in over ~90ms rather than
//! appearing all at once") — a `crate::motion::patterns::GlyphStagger`
//! (`per_item_delay: 90ms`) driven by its own dedicated
//! `frust_core::AnimationController`, sized to
//! `GlyphStagger::glyph().total_duration(n)` and armed once per item-list
//! change (mirrors `crate::glyph::term_block`'s one-shot stagger-reveal
//! idiom exactly — a **decoupled** timeline from the panel's own enter/exit
//! driver, so the stagger's literal per-item millisecond spacing stays
//! meaningful regardless of the panel's own duration). `reduce_motion`
//! collapses the per-item windows to one synchronized reveal (the reduced
//! variant `item_progress` already implements), matching `term_block`'s
//! precedent; the panel's own timing also collapses to a fast linear
//! crossfade under `reduce_motion` (dialog/palette's `resolve_timings`
//! shape).
//!
//! # Selection rides the same animated-exit + page-result path as cancel
//!
//! Unlike [`crate::glyph::dialog`] (whose *actions* pop the navigator
//! immediately, bypassing the widget's own exit animation, and only a
//! scrim/Escape *cancel* plays the staged exit) or
//! [`crate::glyph::command_palette`] (whose row selection fires an
//! immediate `on_select` callback with no dismissal at all), **every** menu
//! dismissal — a scrim tap, `Escape`, a routed back press, *and* an item
//! up-inside selection — stages the identical animated exit and only then
//! delivers its outcome through the pushed page's `on_result` (a selection
//! pops with `PopResult::of(index)`; every other path pops empty). A menu
//! item is a transient chooser, not an action button with its own identity,
//! so it gets no bypass: the panel always visibly collapses back toward its
//! anchor before anything happens, which is also *why* the widget carries no
//! `on_select` callback of its own — [`show_glyph_menu`]'s `on_result` is
//! the only selection channel.
//!
//! # Always dismissable (no `dismissable(bool)`)
//!
//! Unlike the dialog/palette's `dismissable(bool)` barrier flag, a
//! `GlyphMenuView` has none — a menu is a transient chooser the user can
//! always back out of with no answer required, never a modal gate guarding
//! an unavoidable decision. [`show_glyph_menu`] always pushes with
//! [`BackPolicy::DismissAnimated`]: a routed back press
//! bumps the shared dismiss-signal cell the widget's `paint` pass observes
//! (`observe_dismiss_signal`, mirroring dialog/palette's identical
//! back-press handling) and stages the same animated exit a scrim tap or
//! `Escape` does.
//!
//! # Leading icon slot (rider FINDINGS #46)
//!
//! Glyph's own overflow-menu design draws a leading icon per row, but
//! [`MenuItem`] originally carried only `{label, variant}` — no way to
//! vector one. [`MenuItem::icon`] (equivalently, [`MenuEntry::icon`] chained
//! straight off [`menu_item`]/[`menu_item_danger`]) attaches an
//! [`crate::icon::IconData`], painted at [`ITEM_ICON_SIZE`] just inside the
//! row's leading padding, tinted the item's own normal/danger ink (the same
//! color the label itself resolves), with [`ITEM_ICON_GAP`] before the
//! label. A [`MenuEntry::Separator`] has no icon slot — chaining `.icon(..)`
//! onto one is a no-op, not a panic (mirrors this catalog's general
//! silent-drop-on-inapplicable-slot precedent rather than making a
//! chainable builder fallible).
//!
//! **No `&str`-vs-icon precedence rule is needed here** (unlike
//! [`crate::glyph::empty_state::EmptyStateView`], which has one glyph slot
//! two representations compete for): `label` and `icon` are independent
//! slots on the same [`MenuItem`] that always render *together* when both
//! are set — the icon never replaces or hides the label text, so there is
//! nothing to arbitrate between them.
//!
//! # Semantics
//!
//! The whole menu contributes one [`Role::Menu`] container node with the
//! accesskit **modal** flag set (the five-modal-widgets precedent
//! `docs/ARCHITECTURE.md`'s Semantics pass names); each non-separator item
//! contributes a [`Role::Button`] child node (`Action::Click`) — a separator
//! contributes nothing (it carries no interactive meaning). Per-item bounds
//! would need `SemanticsCtx::descend`, which is crate-private to
//! `frust-core` (there is no `ChildPod` here to descend through — items are
//! painted internally, not built as real child widgets), so every button
//! node shares the whole menu's bounds — the same v1 limitation
//! [`crate::glyph::list`]'s `Role::ListItem` rows already accept.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use frust_core::accesskit::{Action, Role};
use frust_core::{
    AnimationController, BoxConstraints, BuildCtx, ChangeFlags, Curve, EventCtx, EventResult,
    FrameTime, InputEvent, Key, LayoutCtx, NamedKey, PaintCtx, PaintScene, PointerButton,
    PointerEvent, PointerPhase, SemanticsCtx, View, Widget, any,
};
use frust_text::{
    FontFamily, FontWeight, GenericSlot, LineHeight, TextContext, TextLayout, TextStyle,
};
use frust_theme::Theme;
use kurbo::{Affine, Point, Rect, RoundedRect, Shape, Size, Vec2};
use peniko::{Brush, Color};

use crate::Timing;
use crate::icon::IconData;
use crate::motion::patterns::GlyphStagger;
use crate::nav::navigator::{BackPolicy, NavigatorController, PopResult, PushOptions};
use crate::nav::transition::{TransitionDriver, TransitionSpec, make_driver};

/// The window-edge clamp margin, in logical px (an original, hand-picked value, kept
/// aligned with the dialog/palette catalog's typical panel gutter).
const WINDOW_MARGIN: f64 = 8.0;
/// Panel padding on all four edges (the design system: `.dropdown{padding:6px}`).
const PANEL_PAD: f64 = 6.0;
/// Horizontal padding inside an item row (the design system: `.dd-item{padding:9px 11px}`).
const ITEM_PAD_X: f64 = 11.0;
/// An item row's height, in logical px (an original, hand-picked value, sized off
/// `.dd-item`'s `9px` vertical padding plus its `11.5px` label at a `1.4`
/// line height).
const ITEM_ROW_H: f64 = 34.0;
/// A separator's total vertical footprint, in logical px (the design system:
/// `.dd-sep{height:1px;margin:5px 4px}` — `1px` line plus `5px` margin above
/// and below).
const SEP_TOTAL_H: f64 = 11.0;
/// A separator's horizontal inset from the panel's padded edges (the design
/// system: `.dd-sep{margin:5px 4px}`'s `4px`).
const SEP_INSET_X: f64 = 4.0;
/// An item's leading icon side length, in logical px (an original,
/// hand-picked value — the design system's own overflow-menu reference
/// doesn't specify a pixel size for its leading-icon rows, so this is sized
/// down from the catalog's default 24px icon box to sit comfortably inside
/// [`ITEM_ROW_H`] alongside the `11.5px` label — see the module docs'
/// Leading icon slot section).
const ITEM_ICON_SIZE: f64 = 16.0;
/// Gap between an item's leading icon and its label, in logical px (an
/// original, hand-picked value, matching this catalog's typical icon/label
/// gap — e.g. `crate::glyph::navbar`'s `NAV_ITEM_GAP`-scale spacing).
const ITEM_ICON_GAP: f64 = 8.0;
/// Minimum panel width (the design system: `.dropdown{min-width:170px}`).
const MIN_WIDTH: f64 = 170.0;
/// Maximum panel width (an original, hand-picked value — the design system leaves it
/// unbounded, but an anchored popup needs a cap to stay a "small chooser").
const MAX_WIDTH: f64 = 280.0;

/// Item label font size (the design system: `.dd-item{font-size:11.5px}`).
const ITEM_FONT_SIZE: f32 = 11.5;
const ITEM_LINE_HEIGHT: f32 = 1.4;

/// Unthemed-fallback panel surface fill (Glyph dark `bg-overlay` `#272d3d` —
/// matches `crate::glyph::command_palette`'s identical `CONTAINER` mapping).
const CONTAINER: Color = Color::from_rgb8(0x27, 0x2d, 0x3d);
/// Unthemed-fallback panel border (Glyph dark `border-bright`).
const BORDER: Color = Color::from_rgb8(0x3e, 0x3f, 0x44);
/// Unthemed-fallback normal-item label ink (Glyph dark `fg` `#f2ead9`).
const LABEL_INK: Color = Color::from_rgb8(0xf2, 0xea, 0xd9);
/// Unthemed-fallback danger-item label ink (Glyph dark `error` `#ff6b6b`).
const DANGER_INK: Color = Color::from_rgb8(0xff, 0x6b, 0x6b);
/// Unthemed-fallback separator line (Glyph dark `border`, the fainter of the
/// two border tokens).
const SEP_INK: Color = Color::from_rgb8(0x2a, 0x2f, 0x3a);
/// Unthemed-fallback pressed-row hover wash (Glyph dark `bg-hover`).
const HOVER: Color = Color::from_rgb8(0x22, 0x28, 0x35);
/// Unthemed-fallback danger pressed-row wash alpha over the error color
/// (the design system: `--error-faint: rgba(255,107,107,0.13)`).
const DANGER_HOVER_ALPHA: f32 = 0.13;
/// Panel corner radius (the design system: `.dropdown{border-radius:var(--radius-md)}`, 10px).
const RADIUS: f64 = 10.0;
/// Item corner radius (the design system: `.dd-item{border-radius:var(--radius-sm)}`, 6px).
const ITEM_RADIUS: f64 = 6.0;
const BORDER_WIDTH: f64 = 1.0;
/// Corner-rounding tolerance for border strokes (mirrors
/// `crate::glyph::dialog`'s constant of the same name).
const PATH_TOLERANCE: f64 = 0.1;

/// Chrome-level shadow fallback (the design system: `.dropdown{box-shadow:0 16px 40px rgba(0,0,0,0.45)}`).
const SHADOW_Y: f64 = 16.0;
const SHADOW_BLUR: f64 = 40.0;
const SHADOW_ALPHA: f32 = 0.45;
const SHADOW_BASE: Color = Color::from_rgb8(0x00, 0x00, 0x00);

/// Panel enter scale start (the design system: `transform:scale(0.85)`).
const ENTER_SCALE_START: f64 = 0.85;
/// Panel enter slide-in start, in logical px (the design system: `translateY(-8px)`).
const ENTER_SLIDE_START: f64 = -8.0;
/// Per-item slide-in start, in logical px (the design system:
/// `.dd-item{transform:translateY(-4px)}`).
const ITEM_SLIDE_START: f64 = -4.0;

/// Enter duration fallback (the design system's own `.dropdown` transition:
/// `.2s var(--ease-spring)`).
const ENTER_DURATION: Duration = Duration::from_millis(200);
/// Enter easing fallback — the design system's own `--ease-spring` literal
/// (`cubic-bezier(0.34,1.4,0.55,1)`), a slightly different overshoot than
/// the dialog/palette's own spring constant (each catalog widget cites its
/// own primary source rather than sharing one blended value).
const ENTER_CURVE: Curve = Curve::Cubic(0.34, 1.4, 0.55, 1.0);
/// Exit duration fallback — faster than the enter, per the design system's
/// "exits always faster than entrances" rule (the design system's own
/// isolated demo does not differentiate open/close timing, so this widget
/// applies the site-wide rule directly, mirroring dialog/palette's identical
/// override).
const EXIT_DURATION: Duration = Duration::from_millis(120);
/// Exit easing fallback — the design system's own `--ease-exit` literal
/// (`cubic-bezier(0.4,0,1,1)`).
const EXIT_CURVE: Curve = Curve::Cubic(0.4, 0.0, 1.0, 1.0);
/// `reduce_motion`'s collapsed crossfade duration (mirrors
/// `crate::glyph::command_palette::REDUCE_MOTION_DURATION`).
const REDUCE_MOTION_DURATION: Duration = Duration::from_millis(100);

fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// The `(enter, exit)` [`Timing`]s for the current `reduce_motion` state —
/// mirrors `crate::glyph::dialog`/`crate::glyph::command_palette`'s
/// `resolve_timings` shape exactly.
fn resolve_timings(theme: Option<&Theme>) -> (Timing, Timing) {
    if theme.map(|t| t.motion.reduce_motion).unwrap_or(false) {
        let t = Timing::Duration(REDUCE_MOTION_DURATION, Curve::Linear);
        return (t, t);
    }
    match theme {
        Some(t) => (
            Timing::Duration(
                Duration::from_secs_f64(t.motion.durations.fast / 1000.0),
                t.motion.easing.spatial,
            ),
            Timing::Duration(
                Duration::from_secs_f64(t.motion.durations.instant / 1000.0),
                t.motion.easing.exit,
            ),
        ),
        None => (
            Timing::Duration(ENTER_DURATION, ENTER_CURVE),
            Timing::Duration(EXIT_DURATION, EXIT_CURVE),
        ),
    }
}

fn resolve_container(theme: Option<&Theme>) -> Color {
    match theme {
        Some(t) => t.scheme().surface_container_highest,
        None => CONTAINER,
    }
}

fn resolve_border(theme: Option<&Theme>) -> Color {
    match theme {
        Some(t) => t.scheme().outline,
        None => BORDER,
    }
}

fn resolve_separator(theme: Option<&Theme>) -> Color {
    match theme {
        Some(t) => t.scheme().outline_variant,
        None => SEP_INK,
    }
}

fn resolve_label(theme: Option<&Theme>) -> Color {
    match theme {
        Some(t) => t.scheme().on_surface,
        None => LABEL_INK,
    }
}

/// Danger-item ink: the `error` role directly ("Danger uses
/// the error role").
fn resolve_danger(theme: Option<&Theme>) -> Color {
    match theme {
        Some(t) => t.scheme().error,
        None => DANGER_INK,
    }
}

fn resolve_hover(theme: Option<&Theme>) -> Color {
    match theme {
        Some(t) => t.scheme().surface_container_high,
        None => HOVER,
    }
}

/// The danger pressed-row wash: a soft error-tinted background (no
/// `ColorScheme` field for an "error faint" role exists, so this resolves
/// the `error` role itself down to the design system's own literal alpha — a
/// genuine token-scale gap, documented per the Theming Conventions).
fn resolve_danger_hover(theme: Option<&Theme>) -> Color {
    with_alpha(resolve_danger(theme), DANGER_HOVER_ALPHA)
}

fn resolve_radius(theme: Option<&Theme>) -> f64 {
    match theme {
        Some(t) => t.shape.medium,
        None => RADIUS,
    }
}

fn resolve_item_radius(theme: Option<&Theme>) -> f64 {
    match theme {
        Some(t) => t.shape.small,
        None => ITEM_RADIUS,
    }
}

fn resolve_shadow(theme: Option<&Theme>) -> (f64, f64, Color) {
    match theme {
        Some(t) => {
            let s = t.elevation.level5.shadow(t.brightness);
            (
                s.y_offset,
                s.blur_std_dev,
                with_alpha(t.scheme().shadow, s.color_alpha),
            )
        }
        None => (SHADOW_Y, SHADOW_BLUR, with_alpha(SHADOW_BASE, SHADOW_ALPHA)),
    }
}

fn finite_or_zero(v: f64) -> f64 {
    if v.is_finite() { v } else { 0.0 }
}

/// Clamp a floating panel's origin along one axis into
/// `[WINDOW_MARGIN, area − size − WINDOW_MARGIN]`, collapsing to
/// `WINDOW_MARGIN` when the panel doesn't fit with the margin on both sides
/// (a tiny window/huge panel).
fn clamp_axis(pref: f64, size: f64, area: f64) -> f64 {
    let max = (area - size - WINDOW_MARGIN).max(WINDOW_MARGIN);
    pref.clamp(WINDOW_MARGIN, max)
}

fn item_style(color: Color) -> TextStyle {
    TextStyle {
        family: FontFamily::stack_with_generic(["IBM Plex Mono"], GenericSlot::Monospace),
        weight: FontWeight::REGULAR,
        line_height: LineHeight::FontSizeRelative(ITEM_LINE_HEIGHT),
        ..TextStyle::new(ITEM_FONT_SIZE, color)
    }
}

/// The horizontal space an item's leading icon slot reserves before its
/// label — [`ITEM_ICON_SIZE`] plus [`ITEM_ICON_GAP`] when the item carries
/// an icon, `0.0` otherwise (no icon, no reserved space).
fn icon_reserve(item: &MenuItem) -> f64 {
    if item.icon.is_some() {
        ITEM_ICON_SIZE + ITEM_ICON_GAP
    } else {
        0.0
    }
}

/// A selectable menu item's visual emphasis — `Danger` for a destructive
/// action ("Danger uses the error role").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuItemVariant {
    Normal,
    Danger,
}

/// One selectable menu item: a label, its [`MenuItemVariant`], and an
/// optional leading icon (see the module docs' Leading icon slot section).
///
/// `icon` is compared via [`IconData::same`] rather than derived
/// `PartialEq` (which [`IconData`] doesn't implement — a generated
/// [`crate::icons`] source has no natural structural equality beyond its
/// `d`/`design` pair, which `same` already checks), so `MenuItem`/
/// [`MenuEntry`] implement `PartialEq`/`Eq` by hand below instead of via
/// `#[derive]`.
#[derive(Clone, Debug)]
pub struct MenuItem {
    pub label: String,
    pub variant: MenuItemVariant,
    pub icon: Option<IconData>,
}

impl PartialEq for MenuItem {
    fn eq(&self, other: &Self) -> bool {
        self.label == other.label
            && self.variant == other.variant
            && match (&self.icon, &other.icon) {
                (Some(a), Some(b)) => a.same(b),
                (None, None) => true,
                _ => false,
            }
    }
}

impl Eq for MenuItem {}

impl MenuItem {
    /// A normal-emphasis item.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            variant: MenuItemVariant::Normal,
            icon: None,
        }
    }

    /// A danger-emphasis item (a destructive action — e.g. "Kill session").
    pub fn danger(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            variant: MenuItemVariant::Danger,
            icon: None,
        }
    }

    /// Attach a leading vector icon to this item (see the module docs'
    /// Leading icon slot section).
    pub fn icon(mut self, icon: impl Into<IconData>) -> Self {
        self.icon = Some(icon.into());
        self
    }
}

/// One row of a [`GlyphMenuView`]: a selectable [`MenuItem`], or a
/// non-selectable [`MenuEntry::Separator`] (the design system's `.dd-sep`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuEntry {
    Item(MenuItem),
    Separator,
}

impl MenuEntry {
    /// Attach a leading vector icon to this entry — a no-op on
    /// [`MenuEntry::Separator`] (see the module docs' Leading icon slot
    /// section). Chainable straight off [`menu_item`]/[`menu_item_danger`],
    /// e.g. `menu_item("Rename").icon(icons::EDIT)`.
    pub fn icon(mut self, icon: impl Into<IconData>) -> Self {
        if let MenuEntry::Item(item) = &mut self {
            item.icon = Some(icon.into());
        }
        self
    }
}

impl From<MenuItem> for MenuEntry {
    fn from(item: MenuItem) -> Self {
        MenuEntry::Item(item)
    }
}

/// Convenience constructor for a normal-emphasis [`MenuEntry::Item`].
pub fn menu_item(label: impl Into<String>) -> MenuEntry {
    MenuEntry::Item(MenuItem::new(label))
}

/// Convenience constructor for a danger-emphasis [`MenuEntry::Item`].
pub fn menu_item_danger(label: impl Into<String>) -> MenuEntry {
    MenuEntry::Item(MenuItem::danger(label))
}

/// Convenience constructor for a [`MenuEntry::Separator`].
pub fn menu_separator() -> MenuEntry {
    MenuEntry::Separator
}

/// A state-free dismissal callback carrying the selected entry index (`None`
/// for every non-selection dismissal: scrim tap, Escape, back) — see the
/// [module docs](self)'s "Selection rides the same animated-exit" section.
type OnDismiss = Rc<dyn Fn(Option<usize>)>;

/// A declarative Glyph anchored dropdown menu. See the [module docs](self).
/// Not generic over an app `State` — it carries no app-supplied child views
/// (mirrors [`crate::glyph::toast::ToastView`]'s state-free shape); selection
/// rides [`show_glyph_menu`]'s `on_result` instead of a builder-held callback.
pub struct GlyphMenuView {
    anchor: Rect,
    entries: Vec<MenuEntry>,
    on_dismiss: Option<OnDismiss>,
    /// The shared back-press dismiss-signal cell (the `DismissAnimated`
    /// seam) — wired internally by [`show_glyph_menu`].
    dismiss_signal: Option<Rc<Cell<u64>>>,
}

/// Create a menu anchored to `anchor` (a window-coordinate rect — see the
/// [module docs](self)'s Anchoring section) over `entries`. Chain
/// [`on_dismiss`](GlyphMenuView::on_dismiss) to wire dismissal/selection —
/// [`show_glyph_menu`] does this for you.
pub fn glyph_menu(anchor: Rect, entries: Vec<MenuEntry>) -> GlyphMenuView {
    GlyphMenuView {
        anchor,
        entries,
        on_dismiss: None,
        dismiss_signal: None,
    }
}

/// PascalCase alias for [`glyph_menu`], matching the widget-fn vocabulary.
#[allow(non_snake_case)]
pub fn GlyphMenu(anchor: Rect, entries: Vec<MenuEntry>) -> GlyphMenuView {
    glyph_menu(anchor, entries)
}

impl GlyphMenuView {
    /// Set the state-free dismissal callback — invoked once, from paint,
    /// when the exit animation completes (see the [module docs](self)).
    /// [`show_glyph_menu`] wires this to `controller.pop()`/
    /// `controller.pop_with_result(..)` automatically.
    pub fn on_dismiss<F: Fn(Option<usize>) + 'static>(mut self, on_dismiss: F) -> Self {
        self.on_dismiss = Some(Rc::new(on_dismiss));
        self
    }
}

/// Push `build`'s menu as a transparent navigator page (see the
/// [module docs](self)) and register `on_result` for the value it pops with
/// — `Some(index)` (into the built menu's `entries`) on an item selection,
/// `None` on every other dismissal. Always pushes with
/// [`BackPolicy::DismissAnimated`] (a menu is always dismissable — see the
/// [module docs](self)'s "Always dismissable" section).
///
/// ```ignore
/// show_glyph_menu(
///     &state.nav,
///     anchor_rect,
///     || vec![menu_item("Rename"), menu_separator(), menu_item_danger("Kill")],
///     |state: &mut State, result: PopResult| {
///         if let Some(i) = result.take::<usize>() { /* entries[i] chosen */ }
///     },
/// );
/// ```
pub fn show_glyph_menu<State, B, R>(
    controller: &NavigatorController<State>,
    anchor: Rect,
    entries: B,
    on_result: R,
) where
    State: 'static,
    B: Fn() -> Vec<MenuEntry> + 'static,
    R: Fn(&mut State, PopResult) + 'static,
{
    let close_ctrl = controller.clone();
    let signal = Rc::new(Cell::new(0u64));
    let widget_signal = signal.clone();
    let options = PushOptions::transparent()
        .transition(TransitionSpec::NONE)
        .back(BackPolicy::DismissAnimated)
        .dismiss_signal(signal)
        .on_result(on_result);
    controller.push_with_options(
        move || {
            let ctrl = close_ctrl.clone();
            let mut view = glyph_menu(anchor, entries()).on_dismiss(move |result| match result {
                Some(i) => ctrl.pop_with_result(PopResult::of(i)),
                None => ctrl.pop(),
            });
            view.dismiss_signal = Some(widget_signal.clone());
            any::<State, _>(view)
        },
        options,
    );
}

/// A minimal retained text run (mirrors `crate::glyph::command_palette`'s
/// `GlyphLabel` — each catalog module duplicates this small helper rather
/// than sharing one, the existing per-module precedent).
struct GlyphLabel {
    content: String,
    layout: Option<TextLayout>,
    laid_out_style: Option<TextStyle>,
}

impl GlyphLabel {
    fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            layout: None,
            laid_out_style: None,
        }
    }

    fn layout(&mut self, ctx: &mut LayoutCtx, style: &TextStyle) -> Size {
        if let Some(cached) = &self.layout
            && self.laid_out_style.as_ref() == Some(style)
        {
            return cached.size();
        }
        let text_ctx = ctx.text_context::<TextContext>();
        let laid = text_ctx.layout(&self.content, style, None);
        let size = laid.size();
        self.layout = Some(laid);
        self.laid_out_style = Some(style.clone());
        size
    }

    fn paint(&self, origin: Point, scene: &mut dyn PaintScene) {
        if let Some(layout) = &self.layout {
            for run in layout.to_scene_runs(origin) {
                scene.draw_glyph_run(run);
            }
        }
    }
}

/// One shaped row: its entry data plus a lazily-shaped label (separators
/// carry no label).
struct Row {
    entry: MenuEntry,
    label: Option<GlyphLabel>,
    label_size: Size,
}

fn make_rows(entries: &[MenuEntry]) -> Vec<Row> {
    entries
        .iter()
        .map(|e| Row {
            entry: e.clone(),
            label: match e {
                MenuEntry::Item(item) => Some(GlyphLabel::new(item.label.clone())),
                MenuEntry::Separator => None,
            },
            label_size: Size::ZERO,
        })
        .collect()
}

/// The item-count-sized stagger controller (see the [module docs](self)) —
/// mirrors `crate::glyph::term_block::build_controller` exactly, sized off
/// the count of `MenuEntry::Item` entries only (a separator never reveals).
fn build_stagger(item_count: usize) -> AnimationController {
    let total_ms = GlyphStagger::glyph().total_duration(item_count);
    let mut c = AnimationController::new(Duration::from_millis(total_ms.round().max(0.0) as u64))
        .with_curve(Curve::Linear);
    c.forward();
    c
}

/// The menu's enter/exit lifecycle phase (mirrors
/// `crate::glyph::dialog::Phase`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Enter,
    Shown,
    Exit,
    Dismissed,
}

/// The retained widget for a [`GlyphMenuView`]. See the [module docs](self).
pub struct GlyphMenuWidget {
    anchor: Rect,
    rows: Vec<Row>,
    on_dismiss: Option<OnDismiss>,
    dismiss_signal: Option<Rc<Cell<u64>>>,
    last_seen_dismiss: u64,
    /// The panel rect in the widget's own local coordinate space.
    panel: Rect,
    /// Each row's rect (local space), parallel to `rows`.
    row_rects: Vec<Rect>,
    phase: Phase,
    driver: Option<TransitionDriver>,
    /// The per-item stagger reveal — armed once per item-list change (see
    /// [`build_stagger`]).
    stagger: AnimationController,
    /// The result to deliver once the exit animation completes: `Some(i)`
    /// for an item selection (index into `rows`/`entries`), `None` for a
    /// scrim/Escape/back dismissal.
    pending_result: Option<usize>,
    /// The row a press is in flight on (fires on up-inside).
    pressed_row: Option<usize>,
    /// A scrim (outside-panel) press is in flight.
    scrim_captured: bool,
}

impl GlyphMenuWidget {
    fn begin_exit(&mut self, result: Option<usize>) {
        if matches!(self.phase, Phase::Enter | Phase::Shown) {
            self.pending_result = result;
            self.phase = Phase::Exit;
            self.driver = None;
        }
    }

    fn observe_dismiss_signal(&mut self) {
        if let Some(signal) = &self.dismiss_signal {
            let current = signal.get();
            if current != self.last_seen_dismiss {
                self.last_seen_dismiss = current;
                self.begin_exit(None);
            }
        }
    }

    /// Advance the enter/exit phase machine to frame time `now`, returning
    /// `(scale, progress, animating)` — `progress` in `[0, 1]` drives both
    /// the slide offset and (while entering) the item stagger's `overall`
    /// input. Mirrors `crate::glyph::dialog::GlyphDialogWidget::advance`'s
    /// shape (the panel here never fades opacity — see the
    /// [module docs](self)).
    fn advance(&mut self, now: FrameTime, enter: Timing, exit: Timing) -> (f64, f64, bool) {
        match self.phase {
            Phase::Enter => {
                let adv = self
                    .driver
                    .get_or_insert_with(|| make_driver(enter).0)
                    .advance(now);
                if adv.done {
                    self.phase = Phase::Shown;
                    self.driver = None;
                }
                let p = adv.value.clamp(0.0, 1.0);
                let scale = ENTER_SCALE_START + (1.0 - ENTER_SCALE_START) * p;
                (scale, p, !adv.done)
            }
            Phase::Shown => (1.0, 1.0, false),
            Phase::Exit => {
                let adv = self
                    .driver
                    .get_or_insert_with(|| make_driver(exit).0)
                    .advance(now);
                let q = adv.value.clamp(0.0, 1.0);
                let scale = 1.0 - (1.0 - ENTER_SCALE_START) * q;
                if adv.done {
                    self.phase = Phase::Dismissed;
                    self.driver = None;
                    if let Some(on_dismiss) = &self.on_dismiss {
                        on_dismiss(self.pending_result);
                    }
                }
                (scale, 1.0 - q, !adv.done)
            }
            Phase::Dismissed => (ENTER_SCALE_START, 0.0, false),
        }
    }

    /// The item row index (into `rows`/`entries`) containing `pos` (local
    /// space) — `None` for a separator row or outside every row.
    fn item_at(&self, pos: Point) -> Option<usize> {
        self.row_rects.iter().enumerate().find_map(|(i, r)| {
            if r.contains(pos) && matches!(self.rows[i].entry, MenuEntry::Item(_)) {
                Some(i)
            } else {
                None
            }
        })
    }
}

impl<State: 'static> View<State> for GlyphMenuView {
    type Element = GlyphMenuWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> GlyphMenuWidget {
        let rows = make_rows(&self.entries);
        let item_count = rows
            .iter()
            .filter(|r| matches!(r.entry, MenuEntry::Item(_)))
            .count();
        GlyphMenuWidget {
            anchor: self.anchor,
            rows,
            on_dismiss: self.on_dismiss.clone(),
            dismiss_signal: self.dismiss_signal.clone(),
            last_seen_dismiss: self.dismiss_signal.as_ref().map(|s| s.get()).unwrap_or(0),
            panel: Rect::ZERO,
            row_rects: Vec::new(),
            phase: Phase::Enter,
            driver: None,
            stagger: build_stagger(item_count),
            pending_result: None,
            pressed_row: None,
            scrim_captured: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut GlyphMenuWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        element.anchor = self.anchor;
        if prev.anchor != self.anchor {
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.entries != self.entries {
            element.rows = make_rows(&self.entries);
            let item_count = element
                .rows
                .iter()
                .filter(|r| matches!(r.entry, MenuEntry::Item(_)))
                .count();
            element.stagger = build_stagger(item_count);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        // Closures aren't comparable; reinstall the dismiss adapter unconditionally.
        element.on_dismiss = self.on_dismiss.clone();
        let signal_changed = self.dismiss_signal.as_ref().map(Rc::as_ptr)
            != element.dismiss_signal.as_ref().map(Rc::as_ptr);
        if signal_changed {
            element.last_seen_dismiss = self.dismiss_signal.as_ref().map(|s| s.get()).unwrap_or(0);
        }
        element.dismiss_signal = self.dismiss_signal.clone();
        flags
    }

    fn teardown(&self, _element: &mut GlyphMenuWidget, _ctx: &mut BuildCtx<'_>) {}
}

impl Widget for GlyphMenuWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let area_w = finite_or_zero(bc.max().width);
        let area_h = finite_or_zero(bc.max().height);

        let (label_c, danger_c) = {
            let theme = Theme::from_layout_ctx(ctx);
            (resolve_label(theme), resolve_danger(theme))
        };

        let mut content_w: f64 = 0.0;
        for row in &mut self.rows {
            if let (Some(label), MenuEntry::Item(item)) = (&mut row.label, &row.entry) {
                let color = match item.variant {
                    MenuItemVariant::Normal => label_c,
                    MenuItemVariant::Danger => danger_c,
                };
                row.label_size = label.layout(ctx, &item_style(color));
                content_w = content_w.max(icon_reserve(item) + row.label_size.width);
            }
        }

        let panel_max_w = area_w.min(MAX_WIDTH);
        let lower = MIN_WIDTH.min(area_w);
        let panel_w = (content_w + 2.0 * ITEM_PAD_X).clamp(lower, panel_max_w.max(lower));

        let mut rows_h = 0.0;
        for row in &self.rows {
            rows_h += match row.entry {
                MenuEntry::Item(_) => ITEM_ROW_H,
                MenuEntry::Separator => SEP_TOTAL_H,
            };
        }
        let panel_h = PANEL_PAD * 2.0 + rows_h;

        let pref_x = self.anchor.x1 - panel_w;
        let pref_y = self.anchor.y1;
        let panel_x = clamp_axis(pref_x, panel_w, area_w);
        let panel_y = clamp_axis(pref_y, panel_h, area_h);
        self.panel = Rect::new(panel_x, panel_y, panel_x + panel_w, panel_y + panel_h);

        self.row_rects.clear();
        let mut ry = panel_y + PANEL_PAD;
        for row in &self.rows {
            let h = match row.entry {
                MenuEntry::Item(_) => ITEM_ROW_H,
                MenuEntry::Separator => SEP_TOTAL_H,
            };
            self.row_rects.push(match row.entry {
                MenuEntry::Item(_) => Rect::new(
                    panel_x + PANEL_PAD,
                    ry,
                    panel_x + panel_w - PANEL_PAD,
                    ry + h,
                ),
                MenuEntry::Separator => Rect::new(
                    panel_x + PANEL_PAD + SEP_INSET_X,
                    ry + (SEP_TOTAL_H - 1.0) / 2.0,
                    panel_x + panel_w - PANEL_PAD - SEP_INSET_X,
                    ry + (SEP_TOTAL_H - 1.0) / 2.0 + 1.0,
                ),
            });
            ry += h;
        }

        bc.constrain(Size::new(area_w, area_h))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.observe_dismiss_signal();
        let theme = Theme::from_paint_ctx(ctx);
        let reduce_motion = theme.map(|t| t.motion.reduce_motion).unwrap_or(false);
        let (enter, exit) = resolve_timings(theme);
        let (scale, progress, panel_animating) = self.advance(ctx.frame_time(), enter, exit);

        // The stagger is a decoupled timeline (see the module docs): a
        // 3+-item cascade (90ms/item) outlives the panel's enter phase, so it
        // keeps advancing through `Shown` until it self-completes — items
        // whose reveal windows open after the panel settles still have to
        // reveal. An early exit just leaves unrevealed items hidden.
        let stagger_animating = matches!(self.phase, Phase::Enter | Phase::Shown)
            && self.stagger.advance(ctx.frame_time());
        let stagger_overall = self.stagger.value_clamped();
        let item_count = self
            .rows
            .iter()
            .filter(|r| matches!(r.entry, MenuEntry::Item(_)))
            .count();
        let stagger_spec = GlyphStagger::glyph();

        let slide = ENTER_SLIDE_START * (1.0 - progress);
        let panel_origin = Point::new(
            ctx.origin().x + self.panel.x0,
            ctx.origin().y + self.panel.y0 + slide,
        );
        let panel_size = self.panel.size();
        // Scale about the panel's own top-right corner (the design system:
        // `transform-origin:top right` — see the module docs' Anchoring note).
        let pivot = Point::new(panel_origin.x + panel_size.width, panel_origin.y);
        let transform = Affine::translate(pivot.to_vec2())
            * Affine::scale(scale)
            * Affine::translate(-pivot.to_vec2());
        scene.push_transform(transform);

        let radius = resolve_radius(theme);
        let (shadow_y, shadow_blur, shadow_color) = resolve_shadow(theme);
        scene.draw_shadow(
            Point::new(panel_origin.x, panel_origin.y + shadow_y),
            panel_size,
            radius,
            shadow_blur,
            shadow_color,
        );
        scene.fill_rounded_rect(panel_origin, panel_size, radius, resolve_container(theme));
        let rr = RoundedRect::from_rect(Rect::from_origin_size(Point::ORIGIN, panel_size), radius);
        let path = rr.to_path(PATH_TOLERANCE);
        scene.stroke_path(
            panel_origin,
            &path,
            BORDER_WIDTH,
            &Brush::Solid(resolve_border(theme)),
        );

        let item_radius = resolve_item_radius(theme);
        // Per-item ink for the leading icon slot — the same normal/danger
        // resolution `layout`'s label shaping already applied (see the
        // module docs' Leading icon slot section).
        let (label_c, danger_c) = (resolve_label(theme), resolve_danger(theme));
        let mut item_idx = 0usize;
        for (i, row) in self.rows.iter().enumerate() {
            let Some(r) = self.row_rects.get(i) else {
                continue;
            };
            let r_origin = panel_origin - self.panel.origin().to_vec2()
                + r.origin().to_vec2()
                + Vec2::new(0.0, slide);
            match &row.entry {
                MenuEntry::Separator => {
                    scene.fill_rect(r_origin, r.size(), resolve_separator(theme));
                }
                MenuEntry::Item(item) => {
                    let alpha = stagger_spec.item_progress(
                        stagger_overall,
                        item_idx,
                        item_count,
                        reduce_motion,
                    ) as f32;
                    item_idx += 1;
                    if alpha <= 0.0 {
                        continue;
                    }
                    let item_dy = ITEM_SLIDE_START * (1.0 - alpha as f64);
                    let row_origin = r_origin + Vec2::new(0.0, item_dy);
                    scene.push_layer(row_origin, r.size(), alpha);
                    if self.pressed_row == Some(i) {
                        let wash = match item.variant {
                            MenuItemVariant::Normal => resolve_hover(theme),
                            MenuItemVariant::Danger => resolve_danger_hover(theme),
                        };
                        scene.fill_rounded_rect(row_origin, r.size(), item_radius, wash);
                    }
                    if let Some(icon_data) = &item.icon {
                        let color = match item.variant {
                            MenuItemVariant::Normal => label_c,
                            MenuItemVariant::Danger => danger_c,
                        };
                        let (path, design) = icon_data.resolve();
                        let scale = if design > 0.0 {
                            ITEM_ICON_SIZE / design
                        } else {
                            1.0
                        };
                        let scaled = Affine::scale(scale) * path;
                        let icon_origin = Point::new(
                            row_origin.x + ITEM_PAD_X,
                            row_origin.y + (r.size().height - ITEM_ICON_SIZE) / 2.0,
                        );
                        scene.fill_path(icon_origin, &scaled, &Brush::Solid(color));
                    }
                    if let Some(label) = &row.label {
                        let label_origin = Point::new(
                            row_origin.x + ITEM_PAD_X + icon_reserve(item),
                            row_origin.y + (r.size().height - row.label_size.height) / 2.0,
                        );
                        label.paint(label_origin, scene);
                    }
                    scene.pop_layer();
                }
            }
        }

        scene.pop_transform();

        if panel_animating || stagger_animating || self.phase == Phase::Dismissed {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // Claim focus on every Down. Load-bearing: the root treats a Down
        // that bubbles no claim as a blur (`release_focus_session` drops
        // focus + IME state), so the re-claim is what keeps the session alive
        // while this menu is up. Re-claiming while already focused is a
        // change-guarded no-op (no generation bump) — do not add a
        // claim-once guard, it kills the session on the second tap
        // (claim-once-hygiene review, 2026-08-06).
        if matches!(event, InputEvent::Pointer(p) if p.phase == PointerPhase::Down) {
            ctx.request_focus();
        }
        if let InputEvent::Key(key_event) = event {
            if key_event.key == Key::Named(NamedKey::Escape) {
                self.begin_exit(None);
                return EventResult::Handled;
            }
            return EventResult::Ignored;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                if let Some(i) = self.item_at(p.position) {
                    self.pressed_row = Some(i);
                    ctx.capture_pointer();
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                self.scrim_captured = true;
                ctx.capture_pointer();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if self.pressed_row.is_some() || self.scrim_captured {
                    EventResult::Handled
                } else {
                    EventResult::Ignored
                }
            }
            PointerPhase::Up => {
                if let Some(i) = self.pressed_row.take() {
                    if self.item_at(p.position) == Some(i) {
                        self.begin_exit(Some(i));
                    }
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                if self.scrim_captured {
                    let outside = !self.panel.contains(p.position);
                    if outside {
                        self.begin_exit(None);
                    }
                    self.scrim_captured = false;
                    return EventResult::Handled;
                }
                EventResult::Ignored
            }
            PointerPhase::Cancel => {
                // Cancel never touches state — just clear the in-flight flags.
                self.pressed_row = None;
                self.scrim_captured = false;
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // Per-row bounds would need `SemanticsCtx::descend`, which is
        // crate-private to `frust-core` (no `ChildPod` exists here to
        // descend through — items are painted internally, not built as real
        // child widgets); every `Role::Button` node below shares the whole
        // menu's bounds instead, the same v1 limitation
        // `crate::glyph::list`'s `Role::ListItem` rows accept.
        let rows = &self.rows;
        ctx.push_container(
            Role::Menu,
            |node| node.set_modal(),
            |ctx| {
                for row in rows.iter() {
                    if let MenuEntry::Item(item) = &row.entry {
                        ctx.push_node(Role::Button, |node| {
                            node.set_label(item.label.as_str());
                            node.add_action(Action::Click);
                        });
                    }
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nav::navigator::{NavigatorView, navigator};
    use frust_core::{BuildCtx, KeyEvent, Modifiers, RenderRoot, any as core_any};
    use frust_text::TextContext;
    use std::any::Any;

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn escape_event() -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::Escape),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    fn build(view: &GlyphMenuView) -> GlyphMenuWidget {
        let mut counter = 0u64;
        View::<()>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut GlyphMenuWidget, area: Size) {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::tight(area));
    }

    fn dispatch(w: &mut GlyphMenuWidget, event: &InputEvent, area: Size) -> EventResult {
        let mut dummy = ();
        let s: &mut dyn Any = &mut dummy;
        let mut ctx = EventCtx::new(s, Point::ZERO, area);
        w.event(&mut ctx, event)
    }

    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        rrects: Vec<(Point, Size, f64, Color)>,
        shadows: usize,
        strokes: Vec<Color>,
        path_fills: Vec<(Point, Rect, Color)>,
        transforms: Vec<Affine>,
        transform_pops: u32,
        layers: Vec<(Point, Size, f32)>,
        layer_pops: u32,
    }
    impl PaintScene for Recorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.rects.push((o, s, c));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn draw_shadow(&mut self, _o: Point, _s: Size, _r: f64, _b: f64, _c: Color) {
            self.shadows += 1;
        }
        fn stroke_path(&mut self, _o: Point, _p: &kurbo::BezPath, _w: f64, brush: &Brush) {
            if let Brush::Solid(c) = brush {
                self.strokes.push(*c);
            }
        }
        fn fill_path(&mut self, origin: Point, path: &kurbo::BezPath, brush: &Brush) {
            if let Brush::Solid(c) = brush {
                self.path_fills.push((origin, path.bounding_box(), *c));
            }
        }
        fn push_transform(&mut self, t: Affine) {
            self.transforms.push(t);
        }
        fn pop_transform(&mut self) {
            self.transform_pops += 1;
        }
        fn push_layer(&mut self, o: Point, s: Size, alpha: f32) {
            self.layers.push((o, s, alpha));
        }
        fn pop_layer(&mut self) {
            self.layer_pops += 1;
        }
    }

    fn three_items() -> Vec<MenuEntry> {
        vec![
            menu_item("Rename session"),
            menu_item("Duplicate layout"),
            menu_separator(),
            menu_item_danger("Kill session"),
        ]
    }

    // -- Positioning: clamps inside window bounds (anchor near right edge) --

    #[test]
    fn anchor_near_right_edge_clamps_panel_inside_window_bounds() {
        // A 20px-wide trailing icon flush against the window's right edge —
        // right-aligning the panel to its trailing edge would otherwise hang
        // the panel past the window (170 min-width panel from x=280 would
        // reach x=450, and even from x=130 reaches x=300, exactly the edge
        // with no margin).
        let area = Size::new(300.0, 600.0);
        let anchor = Rect::new(280.0, 10.0, 300.0, 50.0);
        let view = glyph_menu(anchor, three_items());
        let mut w = build(&view);
        layout(&mut w, area);

        assert!(
            w.panel.x0 >= WINDOW_MARGIN - 1e-9,
            "panel left edge stays inside the margin: {}",
            w.panel.x0
        );
        assert!(
            w.panel.x1 <= area.width - WINDOW_MARGIN + 1e-9,
            "panel right edge stays inside the margin: {}",
            w.panel.x1
        );
    }

    #[test]
    fn anchor_well_inside_window_positions_below_trailing_with_no_clamp() {
        let area = Size::new(800.0, 600.0);
        let anchor = Rect::new(700.0, 40.0, 740.0, 80.0);
        let view = glyph_menu(anchor, three_items());
        let mut w = build(&view);
        layout(&mut w, area);

        // Trailing-aligned: the panel's right edge matches the anchor's.
        assert!((w.panel.x1 - anchor.x1).abs() < 1e-6);
        // Below the anchor.
        assert!((w.panel.y0 - anchor.y1).abs() < 1e-6);
    }

    // -- Enter/exit staging (advance table) -----------------------------

    #[test]
    fn enter_staging_scales_panel_085_to_1() {
        let view = glyph_menu(Rect::ZERO, three_items());
        let mut w = build(&view);
        let (enter, exit) = resolve_timings(None);

        let (scale0, progress0, animating0) = w.advance(ft_ms(0.0), enter, exit);
        assert!((scale0 - ENTER_SCALE_START).abs() < 1e-6);
        assert!(progress0.abs() < 1e-6);
        assert!(animating0);
        assert_eq!(w.phase, Phase::Enter);

        let (scale1, progress1, animating1) = w.advance(ft_ms(400.0), enter, exit);
        assert!((scale1 - 1.0).abs() < 1e-6);
        assert!((progress1 - 1.0).abs() < 1e-6);
        assert!(!animating1);
        assert_eq!(w.phase, Phase::Shown);
    }

    #[test]
    fn exit_is_faster_than_enter() {
        let view = glyph_menu(Rect::ZERO, three_items());
        let mut w = build(&view);
        let (enter, exit) = resolve_timings(None);

        w.advance(ft_ms(0.0), enter, exit);
        w.advance(ft_ms(400.0), enter, exit); // -> Shown
        w.begin_exit(None);
        assert_eq!(w.phase, Phase::Exit);

        w.advance(ft_ms(400.0), enter, exit); // seed exit clock
        let (_, _, animating_mid) = w.advance(ft_ms(400.0 + 119.0), enter, exit);
        assert!(animating_mid, "still exiting just under 120ms");
        let (scale_end, progress_end, animating_end) = w.advance(ft_ms(400.0 + 200.0), enter, exit);
        assert!((scale_end - ENTER_SCALE_START).abs() < 1e-6);
        assert!(progress_end.abs() < 1e-6);
        assert!(!animating_end);
        assert_eq!(w.phase, Phase::Dismissed);
    }

    // -- Item stagger progresses per item; reduce_motion collapses ------

    #[test]
    fn stagger_reveals_items_progressively() {
        let spec = GlyphStagger::glyph();
        let n = 3; // three MenuEntry::Item rows in `three_items()`.
        let total = spec.total_duration(n);
        // Item 0 starts immediately; item 2 (last) starts at 2*90=180ms.
        assert!(spec.item_progress(0.0, 0, n, false) < 1e-9);
        let mid = (90.0 / total).clamp(0.0, 1.0); // just past item 0's window start
        assert!(spec.item_progress(mid, 0, n, false) > spec.item_progress(mid, 2, n, false));
        assert!((spec.item_progress(1.0, 2, n, false) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn reduce_motion_collapses_stagger_to_one_synchronized_reveal() {
        let spec = GlyphStagger::glyph();
        let n = 3;
        // Under reduce_motion every item tracks `overall` directly (no
        // per-item window offset).
        for i in 0..n {
            assert_eq!(spec.item_progress(0.4, i, n, true), 0.4);
        }
    }

    // -- Paint: nested transform + per-item layers -----------------------

    #[test]
    fn paint_emits_one_panel_transform_and_a_layer_per_item() {
        let view = glyph_menu(Rect::new(700.0, 40.0, 740.0, 80.0), three_items());
        let mut w = build(&view);
        let area = Size::new(800.0, 600.0);
        layout(&mut w, area);
        w.phase = Phase::Shown;
        w.stagger = build_stagger(3);
        // Fully settle the stagger: the first `advance` only seeds the clock
        // (zero delta — `AnimationController::advance`'s documented
        // contract), so a second call at a far-future time is required to
        // actually reach full progress.
        w.stagger.advance(ft_ms(0.0));
        w.stagger.advance(ft_ms(10_000.0));

        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, area);
        w.paint(&mut pctx, &mut rec);

        assert_eq!(rec.transforms.len(), 1, "one panel scale transform");
        assert_eq!(rec.transform_pops, 1);
        assert_eq!(rec.shadows, 1);
        assert_eq!(rec.rrects[0].3, resolve_container(None), "panel fill");
        // Three MenuEntry::Item rows -> three layers (one separator, no layer).
        assert_eq!(rec.layers.len(), 3);
        assert_eq!(rec.layer_pops, 3);
        for (_, _, alpha) in &rec.layers {
            assert!((*alpha - 1.0).abs() < 1e-3, "fully settled stagger");
        }
    }

    /// FINDINGS #59 regression: the 3-item cascade (330ms) outlives the
    /// panel's enter phase (200ms theme-less), so the tail item's reveal
    /// window opens only once the panel is already `Shown` — the stagger
    /// must keep advancing there or the item stays invisible forever.
    #[test]
    fn stagger_completes_after_panel_enter_ends() {
        let view = glyph_menu(Rect::new(700.0, 40.0, 740.0, 80.0), three_items());
        let mut w = build(&view);
        let area = Size::new(800.0, 600.0);
        layout(&mut w, area);

        // Seed both clocks, then step past the panel's enter but short of
        // the stagger's 330ms total.
        let mut rec = Recorder::default();
        w.paint(
            &mut PaintCtx::for_test(Point::ZERO, area, ft_ms(0.0)),
            &mut rec,
        );
        w.paint(
            &mut PaintCtx::for_test(Point::ZERO, area, ft_ms(250.0)),
            &mut rec,
        );
        assert_eq!(w.phase, Phase::Shown, "panel enter is over at 250ms");

        // Far past the stagger total: every item must be fully revealed.
        let mut rec = Recorder::default();
        w.paint(
            &mut PaintCtx::for_test(Point::ZERO, area, ft_ms(1_000.0)),
            &mut rec,
        );
        assert_eq!(rec.layers.len(), 3, "all three items paint");
        for (_, _, alpha) in &rec.layers {
            assert!((*alpha - 1.0).abs() < 1e-3, "stagger fully settled");
        }
    }

    // -- Leading icon slot (rider FINDINGS #46) --------------------------

    /// A 10×10-design filled diamond, mirroring
    /// `crate::glyph::navbar::tests::diamond_icon` — a known bounding box so
    /// the icon-fill assertions below are deterministic.
    fn diamond_icon() -> IconData {
        let mut p = kurbo::BezPath::new();
        p.move_to((5.0, 0.0));
        p.line_to((10.0, 5.0));
        p.line_to((5.0, 10.0));
        p.line_to((0.0, 5.0));
        p.close_path();
        IconData::from_path(p, 10.0)
    }

    #[test]
    fn icon_item_paints_a_filled_path_leading_the_label() {
        let entries = vec![
            menu_item("Rename session").icon(diamond_icon()),
            menu_item_danger("Kill session").icon(diamond_icon()),
        ];
        let view = glyph_menu(Rect::new(700.0, 40.0, 740.0, 80.0), entries);
        let mut w = build(&view);
        let area = Size::new(800.0, 600.0);
        layout(&mut w, area);
        w.phase = Phase::Shown;
        w.stagger = build_stagger(2);
        w.stagger.advance(ft_ms(0.0));
        w.stagger.advance(ft_ms(10_000.0));

        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, area);
        w.paint(&mut pctx, &mut rec);

        // The vector path is actually painted — a real fill_path call with a
        // non-degenerate bounding box scaled into the icon box, not merely
        // that the builder accepted a value.
        assert_eq!(rec.path_fills.len(), 2, "one fill_path per icon item");
        let (_, normal_bbox, normal_color) = rec.path_fills[0];
        assert!(
            (normal_bbox.width() - ITEM_ICON_SIZE).abs() < 1e-6,
            "icon scaled to ITEM_ICON_SIZE"
        );
        assert!((normal_bbox.height() - ITEM_ICON_SIZE).abs() < 1e-6);
        assert_eq!(normal_color, resolve_label(None), "normal item icon tint");
        let (_, _, danger_color) = rec.path_fills[1];
        assert_eq!(danger_color, resolve_danger(None), "danger item icon tint");
    }

    #[test]
    fn icon_on_separator_is_a_no_op() {
        let entry = menu_separator().icon(diamond_icon());
        assert_eq!(
            entry,
            MenuEntry::Separator,
            "icon is dropped on a separator"
        );
    }

    #[test]
    fn structural_diff_treats_a_same_source_icon_as_unchanged() {
        // Two `MenuItem`s built from the same generated `IconSource` compare
        // equal (same `d`/`design` pair) even though each `.into()` call
        // produces a fresh `IconData` handle — mirrors
        // `IconData::same`'s "generated source" branch.
        let a = MenuItem::new("Rename").icon(crate::icons::EDIT);
        let b = MenuItem::new("Rename").icon(crate::icons::EDIT);
        assert_eq!(a, b);
    }

    // -- Navigator integration: selection pops with Some(index) ---------

    #[derive(Default)]
    struct NavState {
        results: Vec<Option<usize>>,
    }

    fn app_page(
        controller: &NavigatorController<NavState>,
    ) -> impl FnMut(&mut NavState) -> NavigatorView<NavState> {
        let ctrl = controller.clone();
        move |_: &mut NavState| navigator(&ctrl, || core_any::<NavState, _>(sized(400.0, 600.0)))
    }

    struct Sized {
        size: Size,
    }
    struct SizedW {
        size: Size,
    }
    impl View<NavState> for Sized {
        type Element = SizedW;
        fn build(&self, _c: &mut BuildCtx<'_>) -> SizedW {
            SizedW { size: self.size }
        }
        fn rebuild(&self, _p: &Self, _e: &mut SizedW, _c: &mut BuildCtx<'_>) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    impl Widget for SizedW {
        fn layout(&mut self, _c: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(self.size)
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            scene.fill_rect(ctx.origin(), ctx.size(), Color::BLACK);
        }
    }
    fn sized(w: f64, h: f64) -> Sized {
        Sized {
            size: Size::new(w, h),
        }
    }

    #[test]
    fn item_up_inside_animates_exit_then_pops_with_selected_index() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut root: RenderRoot<NavState, NavigatorView<NavState>> = RenderRoot::new();
        let mut app = app_page(&controller);
        let mut state = NavState::default();
        let area = Size::new(400.0, 600.0);

        root.rebuild(&mut app, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(area, &mut tcx as &mut dyn Any);

        let anchor = Rect::new(360.0, 10.0, 392.0, 42.0);
        show_glyph_menu(
            &controller,
            anchor,
            three_items,
            |state: &mut NavState, result: PopResult| {
                state.results.push(result.take::<usize>());
            },
        );
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);
        root.paint(&mut Recorder::default(), ft_ms(0.0));
        root.paint(&mut Recorder::default(), ft_ms(400.0));

        // Tap item 1 ("Duplicate layout") — press+release inside its row.
        let target_row = {
            // Recover the widget's row rect via a probe paint-free layout call
            // is awkward through RenderRoot, so hit near the panel's second row
            // using the same geometry the widget's own layout would produce:
            // panel trailing-aligned to the anchor, below it, item rows stacked
            // from `PANEL_PAD` at `ITEM_ROW_H` each.
            let panel_x1 = anchor.x1;
            let panel_y0 = anchor.y1;
            Point::new(
                panel_x1 - 20.0,
                panel_y0 + PANEL_PAD + ITEM_ROW_H + ITEM_ROW_H / 2.0,
            )
        };
        root.event(
            &mut state,
            &ev(PointerPhase::Down, target_row.x, target_row.y),
        );
        root.event(
            &mut state,
            &ev(PointerPhase::Up, target_row.x, target_row.y),
        );
        assert!(
            state.results.is_empty(),
            "the exit animation has not finished yet"
        );

        root.paint(&mut Recorder::default(), ft_ms(500.0));
        root.paint(&mut Recorder::default(), ft_ms(800.0));
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);
        root.event(
            &mut state,
            &ev(PointerPhase::Move, target_row.x, target_row.y),
        );

        assert_eq!(state.results, vec![Some(1)], "index 1 was selected");
    }

    #[test]
    fn scrim_tap_animates_then_pops_with_no_selection() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut root: RenderRoot<NavState, NavigatorView<NavState>> = RenderRoot::new();
        let mut app = app_page(&controller);
        let mut state = NavState::default();
        let area = Size::new(400.0, 600.0);

        root.rebuild(&mut app, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(area, &mut tcx as &mut dyn Any);

        show_glyph_menu(
            &controller,
            Rect::new(360.0, 10.0, 392.0, 42.0),
            three_items,
            |state: &mut NavState, result: PopResult| {
                state.results.push(result.take::<usize>());
            },
        );
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);
        root.paint(&mut Recorder::default(), ft_ms(0.0));
        root.paint(&mut Recorder::default(), ft_ms(400.0));

        // Tap far outside the panel.
        root.event(&mut state, &ev(PointerPhase::Down, 5.0, 300.0));
        root.event(&mut state, &ev(PointerPhase::Up, 5.0, 300.0));

        root.paint(&mut Recorder::default(), ft_ms(500.0));
        root.paint(&mut Recorder::default(), ft_ms(800.0));
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);
        root.event(&mut state, &ev(PointerPhase::Move, 5.0, 300.0));

        assert_eq!(
            state.results,
            vec![None],
            "scrim tap cancels with no selection"
        );
    }

    #[test]
    fn back_request_dismiss_animated_animates_then_pops() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut root: RenderRoot<NavState, NavigatorView<NavState>> = RenderRoot::new();
        let mut app = app_page(&controller);
        let mut state = NavState::default();
        let area = Size::new(400.0, 600.0);

        root.rebuild(&mut app, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(area, &mut tcx as &mut dyn Any);

        show_glyph_menu(
            &controller,
            Rect::new(360.0, 10.0, 392.0, 42.0),
            three_items,
            |state: &mut NavState, result: PopResult| {
                state.results.push(result.take::<usize>());
            },
        );
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);
        root.paint(&mut Recorder::default(), ft_ms(0.0));
        root.paint(&mut Recorder::default(), ft_ms(400.0));

        controller.request_back();
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);
        root.paint(&mut Recorder::default(), ft_ms(400.0)); // observes the bump, begins exit

        root.paint(&mut Recorder::default(), ft_ms(500.0));
        root.paint(&mut Recorder::default(), ft_ms(800.0));
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);
        root.event(&mut state, &ev(PointerPhase::Move, 5.0, 5.0));

        assert_eq!(
            state.results,
            vec![None],
            "back request dismisses with no selection"
        );
    }

    // -- Escape (once focused) begins the exit -------------------------

    #[test]
    fn escape_begins_exit_with_no_selection() {
        let view = glyph_menu(Rect::new(360.0, 10.0, 392.0, 42.0), three_items());
        let mut w = build(&view);
        let area = Size::new(400.0, 600.0);
        layout(&mut w, area);

        dispatch(&mut w, &ev(PointerPhase::Down, 5.0, 5.0), area);
        dispatch(&mut w, &ev(PointerPhase::Up, 5.0, 5.0), area);
        dispatch(&mut w, &escape_event(), area);
        assert_eq!(w.phase, Phase::Exit);
        assert_eq!(w.pending_result, None);
    }

    // -- Every Down re-claims focus, self-healing an external blur (hygiene fix-2a) --

    #[test]
    fn a_down_reclaims_focus_after_an_external_blur() {
        use frust_core::ChildPod;

        let view = glyph_menu(Rect::new(360.0, 10.0, 392.0, 42.0), three_items());
        let area = Size::new(400.0, 600.0);
        let w = build(&view);
        let mut pod = ChildPod::new(Box::new(w));
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        pod.layout_child(&mut lctx, &BoxConstraints::tight(area));

        let mut dummy = ();
        {
            let s: &mut dyn Any = &mut dummy;
            let mut ctx = EventCtx::new(s, Point::ZERO, area);
            pod.event_child(&mut ctx, &ev(PointerPhase::Down, 5.0, 5.0));
        }
        assert!(pod.is_focused(), "the first Down claims focus");
        {
            let s: &mut dyn Any = &mut dummy;
            let mut ctx = EventCtx::new(s, Point::ZERO, area);
            pod.event_child(&mut ctx, &ev(PointerPhase::Up, 5.0, 5.0));
        }

        // Simulate an external blur (mirrors the root's own `Down`-with-no-
        // claim release path) so the second Down's own re-claim is what's
        // under test, not a leftover flag.
        pod.set_focused(false);
        {
            let s: &mut dyn Any = &mut dummy;
            let mut ctx = EventCtx::new(s, Point::ZERO, area);
            pod.event_child(&mut ctx, &ev(PointerPhase::Down, 5.0, 5.0));
        }
        assert!(
            pod.is_focused(),
            "a second Down must re-claim focus after an external blur — this is what \
             keeps the root's focus/IME session alive while the menu is up"
        );
    }

    // -- Semantics: modal Menu container + Button items ------------------

    #[test]
    fn render_root_semantics_reports_modal_menu_and_button_items() {
        // Built directly (not through a `Navigator`, which has no `semantics`
        // forwarding of its own yet — mirrors `crate::glyph::dialog`'s
        // identical `semantics_is_a_modal_dialog_labelled_by_title` test).
        fn logic(_s: &mut ()) -> GlyphMenuView {
            glyph_menu(Rect::new(360.0, 10.0, 392.0, 42.0), three_items())
        }
        let mut root: RenderRoot<(), GlyphMenuView> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(400.0, 600.0), &mut tcx as &mut dyn Any);

        let update = root.semantics();
        assert!(
            update
                .nodes
                .iter()
                .any(|(_, n)| n.role() == Role::Menu && n.is_modal()),
            "a modal Role::Menu node is contributed"
        );
        let button_count = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::Button)
            .count();
        assert_eq!(button_count, 3, "one Button node per MenuEntry::Item");
    }
}
