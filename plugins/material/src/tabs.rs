// Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
// `lib/components/tabs/m3e_tabs.dart` + `enums/m3e_tabs_variant.dart` +
// `models/m3e_tab.dart` + `styles/m3e_tab_theme.dart` (retrieved 2026-08-20).
// Upstream: <https://github.com/paadevelopments/material_3_expressive>

//! The Material 3 Expressive **tab bar**: [`TabsVariant::Primary`] (an
//! indicator pill sized to each tab's own label/icon content, matching
//! `m3e_tab_theme.dart`'s `indicatorFullWidth == false`) and
//! [`TabsVariant::Secondary`] (a full-width underline spanning the whole tab
//! slot). The active indicator **slides** between tabs on selection change —
//! see the [Indicator slide](#indicator-slide) section — and each tab paints
//! a `primary`-tinted state layer while pressed.
//!
//! [`tabs`] builds a [`TabsView`]; [`tab`] builds one [`Tab`] of it. It is a
//! **controlled component** (see `docs/CODE_STANDARDS.md`'s Interaction
//! Semantics): a tap fires `on_selected` with the *requested* index and
//! leaves `selected` untouched — the app feeds the confirmed index back on
//! the next rebuild, exactly like [`mod@crate::navbar`]'s
//! `NavigationBarView` (the semantics precedent this module follows — see
//! [Semantics](#semantics) below).
//!
//! # No scrollable variant — ported faithfully, not a gap
//!
//! The wider Material 3 spec (and this crate's own task brief) describes a
//! "scrollable" tabs mode alongside a "fixed" one. **This reference family
//! has no such mode.** `m3e_tabs.dart:154` lays every tab out as
//! `Row(children: [Expanded(child: ...)])` unconditionally — every tab
//! always gets an equal share of the available width, regardless of tab
//! count or content, with no `SingleChildScrollView`/scroll-physics path
//! anywhere in `lib/components/tabs/`. This port transcribes that rule
//! exactly ([`TabsWidget::layout`] divides the available width evenly, the
//! same shape [`mod@crate::navbar`]'s own fixed-slot layout uses) rather
//! than inventing a scrollable mode the upstream component never ships —
//! many/narrow tabs simply compress instead of scrolling, matching upstream
//! pixel-for-pixel.
//!
//! # Indicator slide
//!
//! [`IndicatorMotion`] springs the indicator's `(left, width)` rect from the
//! outgoing tab's geometry to the incoming one — the same `(from, to)` +
//! one-progress-[`AnimationController`] shape [`mod@crate::button`]'s own
//! `RadiusPaddingMotion` press morph uses
//! (`plugins/material/src/button/motion.rs`), reimplemented locally over a
//! position+width pair instead of a radius+padding one: an unseeded (mount
//! frame) target *snaps*; a repeated target inside
//! [`INDICATOR_RETARGET_TOLERANCE`] is a no-op (the dead band that keeps a
//! re-resolving layout pass from restarting the spring every frame); any
//! other retarget springs from **the value being painted right now** (never
//! the stale `from` a naive implementation would jump from), so a selection
//! change mid-flight morphs continuously.
//!
//! **Divergence from the reference.** `m3e_tabs.dart:156-162` drives this
//! slide with `AnimatedPositioned(duration: M3EMotion.medium2, curve:
//! M3EMotion.emphasized)` — a plain duration+curve tween, not a spring. But
//! `m3e_motion.dart`'s own doc comment states the M3 Expressive motion
//! vocabulary favours spring physics for spatial (position/size/shape)
//! movement, and every *other* spatial morph this crate has ported so far —
//! the button press morph, `button_group`'s squish — is spring-driven, never
//! a duration+curve tween. This port follows that established crate
//! convention over this one Dart file's own (arguably legacy) choice, using
//! [`INDICATOR_SPRING`] (= [`crate::tokens::MaterialSpring::SPATIAL_DEFAULT`],
//! `m3e_motion.dart:84-87`'s `spatialDefault` — the token that family names
//! for exactly this motion class, "size, position, and shape morphs").
//!
//! **Layout independence.** The indicator's geometry is a pure paint-time
//! overlay computed from already-laid-out tab rects — it never feeds back
//! into the tab row's own layout (unlike an animated-height widget, see
//! `docs/REVIEW_FOCUS.md`'s layout-skip hot spot). [`TabsWidget::paint`]
//! therefore drives the spring with [`frust::authoring::PaintCtx::request_frame`],
//! never `request_layout` — the same shape [`mod@crate::navbar`]'s own
//! selection-indicator fade uses.
//!
//! **Reduced motion.** `Theme::motion.reduce_motion` snaps the indicator
//! straight to its target rather than freezing wherever the spring happens
//! to be — the same choice [`mod@crate::expandable_list`]'s reveal-fraction
//! snap documents, not [`mod@crate::button_group`]'s squish-freeze (that
//! spring never has a discrete "target" to jump to; this one always does).
//! Since the snap is still a value change, [`TabsWidget::paint`] requests
//! exactly one more frame when it actually moved the painted rect — a bare
//! `request_frame`, never `request_layout`, since (as above) this motion
//! never feeds layout.
//!
//! # Tab content: label / icon / both
//!
//! [`Tab::label`]/[`Tab::icon`] mirror `M3ETab`'s own two optional slots
//! (`models/m3e_tab.dart:8-11` asserts at least one is present — transcribed
//! as [`tabs`]'s own `debug_assert!`, since this crate's builder attaches
//! `label`/`icon` *after* [`tab`] returns). Present content stacks icon over
//! label, centered, with **no gap** between them — `_buildTabContent`
//! (`m3e_tabs.dart:220-251`) is a plain `Column(mainAxisSize: min,
//! mainAxisAlignment: center)` with no separator child. **Porting decision:**
//! [`Tab::icon`] takes an [`frust::IconSource`] rather than the reference's
//! arbitrary `Widget? icon` — the same closed-shape substitution
//! [`mod@crate::segmented_button`]'s own "Checkmark" section documents for
//! `Segment::icon`, applied here for the same reason (a fixed, resolvable
//! icon geometry this widget can measure and recolor at paint time, rather
//! than an opaque child view).
//!
//! # Selection semantics + disabled tabs
//!
//! [`tabs`] `debug_assert!`s [`MIN_TABS`] (`m3e_tabs.dart:27`'s own
//! `tabs.length >= 2` constructor assert). A tap fires `on_selected(state,
//! index)` on release inside an *enabled* tab — fire-on-up-inside, like
//! every interactive widget in this crate
//! (`docs/CODE_STANDARDS.md`'s Interaction Semantics).
//!
//! **Disabled tabs are this port's own addition, not an upstream concept**:
//! `M3ETab`/`M3ETabs` carry no `enabled`/`disabled` field or branch anywhere
//! in `lib/components/tabs/`. [`Tab::enabled`] (default `true`) mirrors
//! [`mod@crate::button`]'s own `enabled` convention exactly: a disabled tab's
//! content dims to [`crate::interaction::DISABLED_CONTENT_OPACITY`] over
//! `on_surface` (`button/core.rs`'s `resolve_colors` is the precedent this
//! reuses the same opacity constant from), its `event` arm swallows nothing
//! (`Down`/`Up`/`Move` on it are simply ignored, arming no press), and its
//! semantics node reports [`frust::authoring::Node::set_disabled`]. A
//! disabled tab may still be the *selected* one — disabled and unselected
//! are independent axes, the same separation `docs/CODE_STANDARDS.md`'s
//! controlled-component convention assumes throughout this crate — so the
//! indicator still targets a disabled-but-selected tab's geometry normally.
//!
//! # State layer: always `primary`, regardless of selection
//!
//! `_buildTab`'s `Positioned.fill(child: ColoredBox(color:
//! scheme.primary.withValues(alpha: state.opacity)))` (`m3e_tabs.dart:200-207`)
//! paints the whole tab slot's press overlay in the *indicator's* color
//! (`primary`), never the tab's own (possibly `on_surface_variant`)
//! foreground — transcribed exactly, not "fixed" to the unselected tint.
//! Only press is tracked ([`crate::interaction::InteractionState::set_pressed`]),
//! not hover/focus — the same established scope
//! [`mod@crate::segmented_button`]'s own "State layer" section documents for
//! a row-of-items widget in this catalog (`navbar`, `button_group`, `chips`
//! all leave hover/focus unwired too); haptics fire
//! [`crate::interaction::HapticSignal::None`] on every confirmed tap via
//! [`crate::interaction::MaterialHaptics::fire`] — `M3ETappable`'s own
//! unoverridden default (`m3e_tappable.dart:36`), the same documented no-op
//! [`mod@crate::segmented_button`] fires.
//!
//! # Semantics
//!
//! The bar is one [`frust::authoring::Role::TabList`] container; each tab
//! contributes a [`frust::authoring::Role::Tab`] node labelled with its text
//! (absent for an icon-only tab, matching `semanticLabel: tab.label` passing
//! a null label straight through when there is none) and carrying
//! [`frust::authoring::Node::set_selected`] — [`mod@crate::navbar`]'s own
//! `NavigationBarWidget::semantics` shape exactly (`TabList`/`Tab`, label +
//! `set_selected`, no `Action::Click`), extended only with
//! [`frust::authoring::Node::set_disabled`] for a disabled tab (see
//! "Selection semantics + disabled tabs" above).

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::text::{FontWeight, LineHeight, TextContext, TextLayout, TextStyle};
use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, CornerRadii, ErasedCallback, EventCtx, EventResult,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerPhase, Role, SemanticsCtx, View, Widget,
};
use frust::{AnimationController, FrameTime, IconData, IconSource, SpringDesc, Theme};
use kurbo::{Affine, BezPath, Point, Rect, Size, Vec2};
use peniko::{Brush, Color};

use crate::interaction::{
    DISABLED_CONTENT_OPACITY, HapticSignal, InteractionState, MaterialHaptics,
};
use crate::press::presses;

/// Fewest tabs a bar may carry (`m3e_tabs.dart:27`'s own `tabs.length >= 2`
/// constructor assert).
pub const MIN_TABS: usize = 2;

/// Bar height, in logical px (`tabTheme.height`'s default,
/// `m3e_tab_theme.dart:11`).
const HEIGHT: f64 = 48.0;
/// Leading-icon side length, in logical px (`iconSize`'s default,
/// `m3e_tab_theme.dart:12`).
const ICON_SIZE: f64 = 24.0;
/// Indicator thickness, in logical px (`indicatorHeight`'s default,
/// `m3e_tab_theme.dart:13`).
const INDICATOR_HEIGHT: f64 = 3.0;
/// Indicator top-corner radius, in logical px (`indicatorCornerRadius`'s
/// default, `m3e_tab_theme.dart:15`) — bottom corners stay square (the
/// indicator sits flush against the bar's own bottom edge).
const INDICATOR_CORNER_RADIUS: f64 = 3.0;
/// Bottom divider stroke width, in logical px — Flutter `BorderSide`'s own
/// default width (`m3e_tabs.dart:146-147`'s `Border(bottom: BorderSide(color:
/// ...)))`, which never overrides `width`).
const DIVIDER_WIDTH: f64 = 1.0;

/// The indicator slide's spring — see the [module docs](self)'s Indicator
/// slide section for why this is a spring at all (a deliberate divergence
/// from the reference's own duration+curve tween) and why this particular
/// preset. Equal to [`crate::tokens::MaterialSpring::SPATIAL_DEFAULT`]
/// (`spring_matches_material_spatial_default` is the tripwire).
const INDICATOR_SPRING: SpringDesc = SpringDesc {
    mass: 1.0,
    stiffness: 700.0,
    damping_ratio: 0.9,
};
/// Nominal period seeding the indicator's [`AnimationController`] clock. The
/// motion is spring-driven ([`INDICATOR_SPRING`]) via `fling`, so this
/// duration only backs the controller's construction and is not itself a
/// timing (mirrors [`mod@crate::button`]'s own `PRESS_ANIM_PERIOD`).
const INDICATOR_ANIM_PERIOD: Duration = Duration::from_millis(300);
/// Launch velocity (px/sec) handed to each leg's [`AnimationController::fling`]
/// — the same modest kick [`mod@crate::button`]'s press morph uses, so a
/// selection change reads snappy rather than creeping off zero.
const INDICATOR_FLING_VELOCITY: f64 = 4.0;
/// The dead band (logical px) a new indicator target must exceed before it
/// starts a new spring leg — mirrors [`mod@crate::button`]'s own
/// `RETARGET_TOLERANCE`, preventing a re-resolving layout pass from
/// restarting the spring every frame at the same value.
const INDICATOR_RETARGET_TOLERANCE: f64 = 0.1;

/// Unthemed-fallback `primary` (a theme resolves `colors.primary`).
const PRIMARY: Color = Color::from_rgb8(0x67, 0x50, 0xA4);
/// Unthemed-fallback `surface`.
const SURFACE: Color = Color::from_rgb8(0xFE, 0xF7, 0xFF);
/// Unthemed-fallback `on_surface`.
const ON_SURFACE: Color = Color::from_rgb8(0x1D, 0x1B, 0x20);
/// Unthemed-fallback `on_surface_variant`.
const ON_SURFACE_VARIANT: Color = Color::from_rgb8(0x49, 0x45, 0x4F);
/// Unthemed-fallback `surface_container_highest`.
const SURFACE_CONTAINER_HIGHEST: Color = Color::from_rgb8(0xE6, 0xE0, 0xE9);

/// The `titleSmall` type-scale token, `(size, line_height, letter_spacing,
/// weight)` — the unthemed fallback for the tab label role
/// (`m3e_tab_theme.dart:53-57`'s `type.titleSmall`).
const TITLE_SMALL: (f32, f32, f32, FontWeight) = (14.0, 20.0, 0.1, FontWeight::MEDIUM);

/// The ink every label run is *shaped* with; never painted — recolored at
/// paint time instead of reshaped (see [`mod@crate::button`]'s `SHAPING_INK`
/// for why).
const SHAPING_INK: Color = Color::BLACK;

/// The two tab emphasis levels Material 3 defines
/// (`enums/m3e_tabs_variant.dart`). See the [module docs](self) for the
/// indicator-geometry difference between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TabsVariant {
    /// Primary tabs sit at the top level of the hierarchy; the indicator
    /// matches each tab's own label/icon content width.
    #[default]
    Primary,
    /// Secondary tabs sit within a content area; the indicator spans the
    /// whole tab slot.
    Secondary,
}

// ---- Colors -----------------------------------------------------------------

/// The resolved `(background, divider, primary, on_surface, on_surface_variant)`
/// color table — `M3ETabTheme.backgroundColor`/`.dividerColor`/
/// `.indicatorColor`/`.tabColor` (`styles/m3e_tab_theme.dart:38`-`:49`).
/// `indicatorColor` and a selected tab's `tabColor` both resolve to
/// `primary`, so this table names it once rather than duplicating the field.
struct Colors {
    background: Color,
    divider: Color,
    primary: Color,
    on_surface: Color,
    on_surface_variant: Color,
}

fn resolve_colors(theme: Option<&Theme>) -> Colors {
    match theme {
        Some(theme) => {
            let s = theme.scheme();
            Colors {
                background: s.surface,
                divider: s.surface_container_highest,
                primary: s.primary,
                on_surface: s.on_surface,
                on_surface_variant: s.on_surface_variant,
            }
        }
        None => Colors {
            background: SURFACE,
            divider: SURFACE_CONTAINER_HIGHEST,
            primary: PRIMARY,
            on_surface: ON_SURFACE,
            on_surface_variant: ON_SURFACE_VARIANT,
        },
    }
}

/// `color` with its alpha channel replaced by `alpha`.
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// A tab's foreground (icon/label) ink: disabled dims to
/// [`DISABLED_CONTENT_OPACITY`] over `on_surface`
/// (`docs/PLUGINS_CODE_STANDARDS.md`'s attribution note on this porting
/// decision — see the [module docs](self)); otherwise `primary` while
/// selected, `on_surface_variant` while not (`tabColor`,
/// `m3e_tab_theme.dart:46-49`).
fn tab_fg(colors: &Colors, selected: bool, enabled: bool) -> Color {
    if !enabled {
        return with_alpha(colors.on_surface, DISABLED_CONTENT_OPACITY);
    }
    if selected {
        colors.primary
    } else {
        colors.on_surface_variant
    }
}

/// The label's `titleSmall` type role (themed) or its unthemed [`TITLE_SMALL`]
/// fallback. The returned style always carries [`SHAPING_INK`] (see that
/// constant's doc) — selection/disabled tint is applied at paint time.
fn label_style(theme: Option<&Theme>) -> TextStyle {
    let mut style = match theme {
        Some(theme) => theme.type_scale.title_small.clone(),
        None => {
            let (size, line_height, letter_spacing, weight) = TITLE_SMALL;
            let mut style = TextStyle::new(size, SHAPING_INK);
            style.line_height = LineHeight::Absolute(line_height);
            style.letter_spacing = letter_spacing;
            style.weight = weight;
            style
        }
    };
    style.color = SHAPING_INK;
    style
}

/// Whether two optional icon sources name the same geometry (`d`/`design`
/// equality) — lets `rebuild` skip re-resolving an unchanged icon, mirroring
/// [`mod@crate::segmented_button`]'s own `same_icon_source`.
fn same_icon_source(a: Option<IconSource>, b: Option<IconSource>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => x.d == y.d && x.design == y.design,
        (None, None) => true,
        _ => false,
    }
}

// ---- The label run (sibling of segmented_button::LabelRun) ------------------

/// A lazily-shaped, paint-time-rebrushed label run — a sibling copy of
/// [`mod@crate::segmented_button`]'s own `LabelRun` (`pub(self)` to that
/// module and out of scope to import).
struct LabelRun {
    content: String,
    layout: Option<TextLayout>,
    shaped_for: Option<TextStyle>,
}

impl LabelRun {
    fn new(content: String) -> Self {
        Self {
            content,
            layout: None,
            shaped_for: None,
        }
    }

    fn set_content(&mut self, content: &str) {
        if self.content != content {
            self.content = content.to_string();
            self.layout = None;
            self.shaped_for = None;
        }
    }

    fn content(&self) -> &str {
        &self.content
    }

    fn shape(&mut self, ctx: &mut LayoutCtx, style: &TextStyle) -> Size {
        if let Some(cached) = &self.layout
            && self.shaped_for.as_ref() == Some(style)
        {
            return cached.size();
        }
        let text_ctx = ctx.text_context::<TextContext>();
        let laid = text_ctx.layout(&self.content, style, None);
        let size = laid.size();
        self.layout = Some(laid);
        self.shaped_for = Some(style.clone());
        size
    }

    fn size(&self) -> Size {
        self.layout.as_ref().map_or(Size::ZERO, TextLayout::size)
    }

    fn paint(&self, origin: Point, color: Color, scene: &mut dyn PaintScene) {
        let Some(layout) = &self.layout else {
            return;
        };
        for mut run in layout.to_scene_runs(origin) {
            run.brush = Brush::Solid(color);
            scene.draw_glyph_run(run);
        }
    }
}

// ---- Indicator motion ---------------------------------------------------------

/// Linear interpolation between `from` and `to` at `t` (unclamped — the
/// spring's overshoot rides past `t = 1`, the "expressive" spatial-spring
/// look; see the [module docs](self)' Indicator slide section).
fn lerp(from: f64, to: f64, t: f64) -> f64 {
    from + (to - from) * t
}

/// The indicator's spring-driven `(left, width)` slide between the outgoing
/// and incoming tab's geometry. See the [module docs](self)' Indicator slide
/// section for the shape and its precedent
/// ([`mod@crate::button`]'s `RadiusPaddingMotion`).
#[derive(Clone, Copy, Debug)]
struct IndicatorMotion {
    from_left: f64,
    to_left: f64,
    from_width: f64,
    to_width: f64,
    anim: AnimationController,
    /// Whether a first target has been seeded — the mount frame snaps to
    /// whatever it resolves rather than sliding in from nothing (mirrors the
    /// reference's own `_indicatorLeft == null` no-paint-yet state).
    seeded: bool,
}

impl IndicatorMotion {
    /// An unseeded motion resting at `(0, 0)` — the first [`Self::retarget`]
    /// snaps to its argument instead of springing to it, and nothing paints
    /// until then (see [`TabsWidget::paint`]).
    fn new() -> Self {
        Self {
            from_left: 0.0,
            to_left: 0.0,
            from_width: 0.0,
            to_width: 0.0,
            anim: AnimationController::new(INDICATOR_ANIM_PERIOD),
            seeded: false,
        }
    }

    /// Aim the indicator at `(left, width)`, returning whether this actually
    /// started a new spring leg. Three outcomes, in order: an unseeded motion
    /// **snaps** (mount frame); a target within
    /// [`INDICATOR_RETARGET_TOLERANCE`] of the live one is **ignored**;
    /// anything else **springs**, from the value being painted right now —
    /// see the [module docs](self)' continuity note.
    fn retarget(&mut self, left: f64, width: f64) -> bool {
        if !self.seeded {
            self.snap_to(left, width);
            return false;
        }
        if (self.to_left - left).abs() <= INDICATOR_RETARGET_TOLERANCE
            && (self.to_width - width).abs() <= INDICATOR_RETARGET_TOLERANCE
        {
            return false;
        }
        self.from_left = self.left();
        self.from_width = self.width();
        self.to_left = left;
        self.to_width = width;
        self.anim = AnimationController::new(INDICATOR_ANIM_PERIOD);
        self.anim.fling(INDICATOR_FLING_VELOCITY, INDICATOR_SPRING);
        true
    }

    /// Pin both channels to `(left, width)` with no motion at all, marking
    /// the motion seeded.
    fn snap_to(&mut self, left: f64, width: f64) {
        self.from_left = left;
        self.to_left = left;
        self.from_width = width;
        self.to_width = width;
        self.anim = AnimationController::new(INDICATOR_ANIM_PERIOD);
        self.seeded = true;
    }

    /// Fast-forward whatever leg is in flight straight to its target,
    /// without changing the target itself — `Theme::motion.reduce_motion`'s
    /// paint-time contract (see the [module docs](self)' Reduced motion
    /// section). Collapses `from` onto `to` (mirrors
    /// [`mod@crate::expandable_list`]'s own `Reveal::snap`) rather than
    /// forcing the driver's internal progress value, so [`Self::left`]/
    /// [`Self::width`] read the target regardless of the driver's own state.
    fn snap(&mut self) {
        self.from_left = self.to_left;
        self.from_width = self.to_width;
        self.anim.stop();
    }

    /// Advance the spring to frame time `now`, returning whether it is still
    /// animating (in which case the caller must request another frame).
    fn advance(&mut self, now: FrameTime) -> bool {
        self.anim.advance(now)
    }

    /// Whether a first target has been seeded — i.e. whether
    /// [`Self::left`]/[`Self::width`] mean anything yet.
    fn is_seeded(&self) -> bool {
        self.seeded
    }

    fn factor(&self) -> f64 {
        let raw = self.anim.value();
        if raw.is_finite() { raw } else { 0.0 }
    }

    /// The indicator's left edge to paint this frame.
    fn left(&self) -> f64 {
        lerp(self.from_left, self.to_left, self.factor())
    }

    /// The indicator's width to paint this frame (never negative — an
    /// under-damped overshoot toward a narrower target could otherwise drive
    /// it past zero, mirroring [`mod@crate::button`]'s own corner guard).
    fn width(&self) -> f64 {
        lerp(self.from_width, self.to_width, self.factor()).max(0.0)
    }
}

// ---- View ---------------------------------------------------------------------

/// One tab within a [`TabsView`] — the reference's `M3ETab`
/// (`models/m3e_tab.dart`).
pub struct Tab {
    label: Option<String>,
    icon: Option<IconSource>,
    enabled: bool,
}

/// Create a tab with neither label nor icon set — attach at least one via
/// [`Tab::label`]/[`Tab::icon`] (`debug_assert!`-checked by [`tabs`]; see the
/// [module docs](self)' Tab content section).
pub fn tab() -> Tab {
    Tab {
        label: None,
        icon: None,
        enabled: true,
    }
}

impl Tab {
    /// Attach a text label, shaped in the bar's `titleSmall` role.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Attach a leading icon, stacked above the label with no gap. See the
    /// [module docs](self)' Tab content section for why this takes an
    /// [`IconSource`] rather than an arbitrary child view.
    pub fn icon(mut self, icon: IconSource) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Whether this tab accepts a press (default `true`). See the [module
    /// docs](self)' Selection semantics + disabled tabs section — this is
    /// this port's own addition, not an upstream `M3ETab` field.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

/// A view-held selection callback (erased per-tab on build/rebuild).
type OnSelected<State> = Rc<dyn Fn(&mut State, usize)>;

/// A declarative M3 Expressive tab bar. See the [module docs](self).
pub struct TabsView<State: 'static> {
    tabs: Vec<Tab>,
    selected: usize,
    variant: TabsVariant,
    on_selected: OnSelected<State>,
}

/// Create a tab bar from `tabs` (2+, `debug_assert!`-checked — see
/// [`MIN_TABS`]), reflecting `selected` (the currently-confirmed index).
/// Primary variant by default; call
/// [`TabsView::variant`]`(`[`TabsVariant::Secondary`]`)` for the full-width
/// underline. `on_selected` fires with the *requested* index on release
/// inside an enabled tab — see the [module docs](self)' Selection semantics
/// section.
pub fn tabs<State: 'static, F: Fn(&mut State, usize) + 'static>(
    tabs: impl IntoIterator<Item = Tab>,
    selected: usize,
    on_selected: F,
) -> TabsView<State> {
    let tabs: Vec<Tab> = tabs.into_iter().collect();
    debug_assert!(
        tabs.len() >= MIN_TABS,
        "a tab bar needs {MIN_TABS}+ tabs (m3e_tabs.dart:27), got {}",
        tabs.len()
    );
    for t in &tabs {
        debug_assert!(
            t.label.is_some() || t.icon.is_some(),
            "a tab needs a label or an icon (m3e_tab.dart:8-11)"
        );
    }
    TabsView {
        tabs,
        selected,
        variant: TabsVariant::Primary,
        on_selected: Rc::new(on_selected),
    }
}

/// PascalCase alias for [`tabs`], matching this catalog's view-fn vocabulary.
#[allow(non_snake_case)]
pub fn Tabs<State: 'static, F: Fn(&mut State, usize) + 'static>(
    tabs_: impl IntoIterator<Item = Tab>,
    selected: usize,
    on_selected: F,
) -> TabsView<State> {
    tabs(tabs_, selected, on_selected)
}

impl<State: 'static> TabsView<State> {
    /// Select [`TabsVariant::Secondary`] (full-width underline) over the
    /// default [`TabsVariant::Primary`] (content-width pill).
    pub fn variant(mut self, variant: TabsVariant) -> Self {
        self.variant = variant;
        self
    }
}

// ---- Widget ---------------------------------------------------------------------

/// Build one bound tap handler per tab, type-erasing the reported value
/// (a plain `usize`, so no generic to erase — unlike
/// [`mod@crate::segmented_button`]'s own `bind_on_tap`).
fn bind_on_tap<State: 'static>(on_selected: &OnSelected<State>, idx: usize) -> ErasedCallback {
    let callback = on_selected.clone();
    Box::new(move |ctx: &mut EventCtx| {
        MaterialHaptics::fire(HapticSignal::None);
        let state = ctx.state_mut::<State>();
        callback(state, idx);
    })
}

fn bind_on_taps<State: 'static>(
    tabs: &[Tab],
    on_selected: &OnSelected<State>,
) -> Vec<ErasedCallback> {
    (0..tabs.len())
        .map(|i| bind_on_tap::<State>(on_selected, i))
        .collect()
}

/// The per-tab label-run/icon-geometry/enabled vectors [`build_tab_runtime`]
/// returns.
type TabRuntime = (
    Vec<Option<LabelRun>>,
    Vec<Option<(BezPath, f64)>>,
    Vec<bool>,
);

/// Build the per-tab runtime vectors from `tabs` — shared by `build` and a
/// structural (tab-count-changed) `rebuild`.
fn build_tab_runtime(tabs: &[Tab]) -> TabRuntime {
    let labels = tabs
        .iter()
        .map(|t| t.label.as_ref().map(|l| LabelRun::new(l.clone())))
        .collect();
    let icon_geoms = tabs
        .iter()
        .map(|t| t.icon.map(|src| IconData::from(src).resolve()))
        .collect();
    let enabled_flags = tabs.iter().map(|t| t.enabled).collect();
    (labels, icon_geoms, enabled_flags)
}

/// The retained widget for a [`TabsView`].
pub struct TabsWidget {
    /// One label run per tab (`None` for an icon-only tab).
    labels: Vec<Option<LabelRun>>,
    /// Each tab's own resolved icon geometry (`None` for a label-only tab).
    icon_geoms: Vec<Option<(BezPath, f64)>>,
    /// Per-tab enabled flag — see the [module docs](self)' disabled-tabs
    /// section.
    enabled_flags: Vec<bool>,
    selected: usize,
    variant: TabsVariant,
    /// Per-tab full slot rects in the widget's local space (equal-width
    /// division — see the [module docs](self)' "No scrollable variant"
    /// section), computed in [`Widget::layout`]; used for the secondary
    /// indicator target, the state-layer fill, and hit-testing.
    tab_rects: Vec<Rect>,
    /// Per-tab content (icon+label) rects, centered within their own
    /// [`Self::tab_rects`] slot; used for the primary indicator target and
    /// content painting.
    content_rects: Vec<Rect>,
    /// Per-tab hover/focus/pressed/dragged tracking; only `pressed` is ever
    /// set — see the [module docs](self)' State layer section.
    interactions: Vec<InteractionState>,
    /// The sliding indicator — see the [module docs](self)' Indicator slide
    /// section.
    indicator: IndicatorMotion,
    /// Whether a press is currently armed (a `Down` landed on an enabled tab).
    armed: bool,
    /// The tab a press is currently tracking.
    active_tab: Option<usize>,
    /// Whether the armed pointer is currently inside [`Self::active_tab`].
    pressed_inside: bool,
    /// One bound tap handler per tab — see [`bind_on_tap`].
    on_taps: Vec<ErasedCallback>,
}

impl TabsWidget {
    /// Which tab (if any) contains local point `pos`.
    fn tab_at(&self, pos: Point) -> Option<usize> {
        self.tab_rects.iter().position(|r| r.contains(pos))
    }

    /// The `(left, width)` the indicator should target for tab `i`, per the
    /// current [`Self::variant`] — [`crate::tabs::TabsVariant::Primary`]
    /// reads [`Self::content_rects`], [`TabsVariant::Secondary`] reads
    /// [`Self::tab_rects`] (see the [module docs](self)' variant-geometry
    /// difference).
    fn indicator_target(&self, i: usize) -> (f64, f64) {
        let rect = match self.variant {
            TabsVariant::Primary => self.content_rects.get(i).copied().unwrap_or(Rect::ZERO),
            TabsVariant::Secondary => self.tab_rects.get(i).copied().unwrap_or(Rect::ZERO),
        };
        (rect.x0, rect.width())
    }
}

impl<State: 'static> View<State> for TabsView<State> {
    type Element = TabsWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> TabsWidget {
        let (labels, icon_geoms, enabled_flags) = build_tab_runtime(&self.tabs);
        let on_taps = bind_on_taps::<State>(&self.tabs, &self.on_selected);
        let n = self.tabs.len();
        TabsWidget {
            labels,
            icon_geoms,
            enabled_flags,
            selected: self.selected,
            variant: self.variant,
            tab_rects: vec![Rect::ZERO; n],
            content_rects: vec![Rect::ZERO; n],
            interactions: vec![InteractionState::new(); n],
            indicator: IndicatorMotion::new(),
            armed: false,
            active_tab: None,
            pressed_inside: false,
            on_taps,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut TabsWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;

        if prev.tabs.len() != self.tabs.len() {
            // Structural: rebuild every per-tab vector fresh. The indicator
            // resets to unseeded so the upcoming layout snaps to the new
            // geometry rather than sliding from stale positions — mirrors
            // upstream's own `didUpdateWidget` clearing `_indicatorLeft`/
            // `_indicatorWidth` on a tab-count change.
            let (labels, icon_geoms, enabled_flags) = build_tab_runtime(&self.tabs);
            element.labels = labels;
            element.icon_geoms = icon_geoms;
            element.enabled_flags = enabled_flags;
            element.tab_rects = vec![Rect::ZERO; self.tabs.len()];
            element.content_rects = vec![Rect::ZERO; self.tabs.len()];
            element.interactions = vec![InteractionState::new(); self.tabs.len()];
            element.indicator = IndicatorMotion::new();
            element.armed = false;
            element.active_tab = None;
            element.pressed_inside = false;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else {
            for (i, (prev_t, next_t)) in prev.tabs.iter().zip(self.tabs.iter()).enumerate() {
                match (&prev_t.label, &next_t.label) {
                    (None, None) => {}
                    (Some(p), Some(n)) if p == n => {}
                    (_, Some(n)) => {
                        let run =
                            element.labels[i].get_or_insert_with(|| LabelRun::new(String::new()));
                        run.set_content(n);
                        flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                    }
                    (Some(_), None) => {
                        element.labels[i] = None;
                        flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                    }
                }
                if !same_icon_source(prev_t.icon, next_t.icon) {
                    element.icon_geoms[i] = next_t.icon.map(|src| IconData::from(src).resolve());
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
                if element.enabled_flags[i] != next_t.enabled {
                    element.enabled_flags[i] = next_t.enabled;
                    flags |= ChangeFlags::PAINT;
                }
            }
        }

        let n = self.tabs.len();
        let next_selected = if n == 0 { 0 } else { self.selected.min(n - 1) };
        let selection_or_variant_changed =
            element.selected != next_selected || element.variant != self.variant;
        element.selected = next_selected;
        element.variant = self.variant;

        if selection_or_variant_changed {
            flags |= ChangeFlags::PAINT;
            if !flags.needs_layout() {
                // Non-structural: retarget using the geometry cached from the
                // last layout — no relayout needed, since neither selection
                // nor variant changes the tab row's own shape (mirrors
                // `NavigationBar`'s own cheap indicator repaint). A
                // structural change above already flagged LAYOUT, which will
                // retarget with fresh geometry itself.
                let (left, width) = element.indicator_target(element.selected);
                element.indicator.retarget(left, width);
            }
        }

        // Closures aren't comparable — always reinstall, mirroring
        // `segmented_button`/`navbar`'s own callback rebinding.
        element.on_taps = bind_on_taps::<State>(&self.tabs, &self.on_selected);

        flags
    }
}

impl Widget for TabsWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        let n = self.labels.len();
        if n == 0 {
            return bc.constrain(Size::new(width, HEIGHT));
        }

        let style = {
            let theme = Theme::from_layout_ctx(ctx);
            label_style(theme)
        };
        // Equal-width division regardless of tab count/content — see the
        // [module docs](self)' "No scrollable variant" section.
        let slot_w = width / n as f64;

        self.tab_rects.clear();
        self.content_rects.clear();
        for i in 0..n {
            let has_icon = self.icon_geoms[i].is_some();
            let (label_w, label_h) = if let Some(run) = self.labels[i].as_mut() {
                let size = run.shape(ctx, &style);
                (size.width, size.height)
            } else {
                (0.0, 0.0)
            };
            let icon_w = if has_icon { ICON_SIZE } else { 0.0 };
            let icon_h = if has_icon { ICON_SIZE } else { 0.0 };
            // Column(mainAxisSize: min): width is the widest child, height is
            // the sum (no gap — see the [module docs](self)' Tab content
            // section).
            let content_w = label_w.max(icon_w);
            let content_h = icon_h + label_h;

            let slot_x = i as f64 * slot_w;
            self.tab_rects
                .push(Rect::new(slot_x, 0.0, slot_x + slot_w, HEIGHT));

            let content_x = slot_x + (slot_w - content_w) / 2.0;
            let content_y = (HEIGHT - content_h) / 2.0;
            self.content_rects.push(Rect::new(
                content_x,
                content_y,
                content_x + content_w,
                content_y + content_h,
            ));
        }

        let selected = self.selected.min(n - 1);
        let (left, target_width) = self.indicator_target(selected);
        self.indicator.retarget(left, target_width);

        bc.constrain(Size::new(width, HEIGHT))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Advance (or, under reduced motion, snap) the indicator before
        // reading its geometry below — see the [module docs](self)'
        // Indicator slide / Reduced motion sections. Paint-only: this motion
        // never feeds layout, so only `request_frame`, never
        // `request_layout`.
        if self.indicator.is_seeded() {
            let reduce_motion = Theme::from_paint_ctx(ctx).is_some_and(|t| t.motion.reduce_motion);
            if reduce_motion {
                let before = (self.indicator.left(), self.indicator.width());
                self.indicator.snap();
                let after = (self.indicator.left(), self.indicator.width());
                if before != after {
                    ctx.request_frame();
                }
            } else if self.indicator.advance(ctx.frame_time()) {
                ctx.request_frame();
            }
        }

        let origin = ctx.origin();
        let size = ctx.size();
        let n = self.tab_rects.len();

        let colors = {
            let theme = Theme::from_paint_ctx(ctx);
            resolve_colors(theme)
        };

        scene.fill_rect(origin, size, colors.background);
        scene.stroke_line(
            Point::new(origin.x, origin.y + size.height),
            Point::new(origin.x + size.width, origin.y + size.height),
            DIVIDER_WIDTH,
            colors.divider,
        );

        for i in 0..n {
            let tab_rect = self.tab_rects[i];
            let content_rect = self.content_rects[i];
            let enabled = self.enabled_flags[i];
            let selected = i == self.selected;

            // The state layer is always `primary`-tinted, regardless of
            // selection — see the [module docs](self)' State layer section.
            let opacity = self.interactions[i].resolve_opacity();
            if opacity > 0.0 {
                let tab_origin = Point::new(origin.x + tab_rect.x0, origin.y + tab_rect.y0);
                scene.fill_rect(
                    tab_origin,
                    tab_rect.size(),
                    with_alpha(colors.primary, opacity),
                );
            }

            let fg = tab_fg(&colors, selected, enabled);
            let has_icon = self.icon_geoms[i].is_some();
            let icon_w = if has_icon { ICON_SIZE } else { 0.0 };
            let icon_h = if has_icon { ICON_SIZE } else { 0.0 };
            let label_w = self.labels[i].as_ref().map_or(0.0, |run| run.size().width);

            if let Some((path, design)) = &self.icon_geoms[i] {
                let scale = if *design > 0.0 {
                    ICON_SIZE / *design
                } else {
                    1.0
                };
                let icon_x = content_rect.x0 + (content_rect.width() - icon_w) / 2.0;
                let icon_y = content_rect.y0;
                let transform = Affine::translate(Vec2::new(icon_x, icon_y)) * Affine::scale(scale);
                let scaled = transform * path.clone();
                scene.fill_path(origin, &scaled, &Brush::Solid(fg));
            }

            if let Some(run) = &self.labels[i] {
                let label_x = content_rect.x0 + (content_rect.width() - label_w) / 2.0;
                let label_y = content_rect.y0 + icon_h;
                run.paint(
                    Point::new(origin.x + label_x, origin.y + label_y),
                    fg,
                    scene,
                );
            }
        }

        if self.indicator.is_seeded() {
            let left = self.indicator.left();
            let width = self.indicator.width();
            let indicator_origin =
                Point::new(origin.x + left, origin.y + HEIGHT - INDICATOR_HEIGHT);
            scene.fill_rounded_rect_radii(
                indicator_origin,
                Size::new(width, INDICATOR_HEIGHT),
                CornerRadii::new(INDICATOR_CORNER_RADIUS, INDICATOR_CORNER_RADIUS, 0.0, 0.0),
                colors.primary,
            );
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                if !presses(p) {
                    return EventResult::Ignored;
                }
                let Some(i) = self.tab_at(p.position) else {
                    return EventResult::Ignored;
                };
                if !self.enabled_flags[i] {
                    // A disabled tab arms nothing — the same shape
                    // `button/core.rs`'s own disabled guard uses.
                    return EventResult::Ignored;
                }
                self.armed = true;
                self.active_tab = Some(i);
                self.pressed_inside = true;
                self.interactions[i].set_pressed(true);
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if !self.armed {
                    return EventResult::Ignored;
                }
                let Some(i) = self.active_tab else {
                    return EventResult::Ignored;
                };
                let inside = self
                    .tab_rects
                    .get(i)
                    .is_some_and(|r| r.contains(p.position));
                if inside != self.pressed_inside {
                    self.pressed_inside = inside;
                    self.interactions[i].set_pressed(inside);
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.armed {
                    return EventResult::Ignored;
                }
                if let Some(i) = self.active_tab {
                    self.interactions[i].set_pressed(false);
                    if self.pressed_inside {
                        (self.on_taps[i])(ctx);
                    }
                }
                self.armed = false;
                self.pressed_inside = false;
                self.active_tab = None;
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.armed {
                    return EventResult::Ignored;
                }
                if let Some(i) = self.active_tab {
                    self.interactions[i].set_pressed(false);
                }
                self.armed = false;
                self.pressed_inside = false;
                self.active_tab = None;
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let labels = &self.labels;
        let enabled_flags = &self.enabled_flags;
        let selected = self.selected;
        ctx.push_container(
            Role::TabList,
            |_| {},
            |ctx| {
                for i in 0..labels.len() {
                    let label = labels[i].as_ref().map(LabelRun::content);
                    let enabled = enabled_flags[i];
                    ctx.push_node(Role::Tab, |node| {
                        if let Some(l) = label {
                            node.set_label(l);
                        }
                        node.set_selected(i == selected);
                        if !enabled {
                            node.set_disabled();
                        }
                    });
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::{PointerButton, PointerEvent};
    use std::any::Any;

    fn ft_secs(s: f64) -> FrameTime {
        FrameTime::from_nanos((s * 1_000_000_000.0) as u64)
    }

    fn build_view(selected: usize) -> TabsView<Vec<usize>> {
        tabs(
            [tab().label("One"), tab().label("Two"), tab().label("Three")],
            selected,
            |s: &mut Vec<usize>, i| s.push(i),
        )
    }

    fn build(view: &TabsView<Vec<usize>>) -> TabsWidget {
        let mut counter = 0u64;
        View::<Vec<usize>>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout_themed(w: &mut TabsWidget, width: f64, theme: Option<&Theme>) -> Size {
        let bc = BoxConstraints::loose(Size::new(width, f64::INFINITY));
        let mut tcx = TextContext::new();
        let mut ctx =
            LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), theme.map(|t| t as &dyn Any));
        w.layout(&mut ctx, &bc)
    }

    fn layout(w: &mut TabsWidget, width: f64) -> Size {
        layout_themed(w, width, None)
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    #[allow(clippy::ptr_arg)]
    fn dispatch(w: &mut TabsWidget, state: &mut Vec<usize>, event: &InputEvent) {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(300.0, HEIGHT));
        w.event(&mut ctx, event);
    }

    // ---- IndicatorMotion: slide timeline (start/mid/end rects) -------------

    #[test]
    fn indicator_spring_matches_material_spatial_default() {
        let token = crate::tokens::MaterialSpring::SPATIAL_DEFAULT;
        assert_eq!(INDICATOR_SPRING.stiffness, token.stiffness);
        assert_eq!(INDICATOR_SPRING.damping_ratio, token.damping_ratio);
        assert_eq!(INDICATOR_SPRING.mass, 1.0);
        assert_eq!(INDICATOR_SPRING.stiffness, 700.0);
        assert_eq!(INDICATOR_SPRING.damping_ratio, 0.9);
    }

    #[test]
    fn indicator_slides_between_two_tabs_start_mid_end() {
        let mut motion = IndicatorMotion::new();
        assert!(
            !motion.retarget(0.0, 40.0),
            "the mount frame seeds, it does not animate"
        );
        assert_eq!(
            (motion.left(), motion.width()),
            (0.0, 40.0),
            "start: unseeded snap"
        );

        assert!(
            motion.retarget(120.0, 60.0),
            "a real geometry change starts a spring leg"
        );
        assert_eq!(
            (motion.left(), motion.width()),
            (0.0, 40.0),
            "mid-flight starts exactly where the previous frame painted"
        );

        motion.advance(ft_secs(0.0));
        motion.advance(ft_secs(0.05));
        let (mid_left, mid_width) = (motion.left(), motion.width());
        assert!(
            mid_left > 0.0 && mid_left < 120.0,
            "left sampled strictly between start and end, got {mid_left}"
        );
        assert!(
            mid_width > 40.0 && mid_width < 60.0,
            "width sampled strictly between start and end, got {mid_width}"
        );

        let mut t = 0.05;
        for _ in 0..600 {
            t += 1.0 / 60.0;
            if !motion.advance(ft_secs(t)) {
                break;
            }
        }
        assert_eq!(
            (motion.left(), motion.width()),
            (120.0, 60.0),
            "end: settles exactly on the new tab's geometry"
        );
    }

    #[test]
    fn a_target_inside_the_dead_band_is_ignored() {
        let mut motion = IndicatorMotion::new();
        motion.retarget(10.0, 40.0);
        assert!(
            !motion.retarget(
                10.0 + INDICATOR_RETARGET_TOLERANCE / 2.0,
                40.0 + INDICATOR_RETARGET_TOLERANCE / 2.0
            ),
            "a sub-tolerance change must not restart the spring"
        );
        assert!(motion.retarget(10.0 + INDICATOR_RETARGET_TOLERANCE * 2.0, 40.0));
    }

    // ---- Bounds (2+ tabs, label-or-icon) -------------------------------------

    #[test]
    #[should_panic(expected = "needs 2+ tabs")]
    fn fewer_than_two_tabs_panics() {
        let _ = tabs::<(), _>([tab().label("Only")], 0, |_s: &mut (), _i| {});
    }

    #[test]
    #[should_panic(expected = "label or an icon")]
    fn a_tab_with_neither_label_nor_icon_panics() {
        let _ = tabs::<(), _>(
            [
                Tab {
                    label: None,
                    icon: None,
                    enabled: true,
                },
                tab().label("B"),
            ],
            0,
            |_s: &mut (), _i| {},
        );
    }

    // ---- Layout: equal-width division, no scroll variant --------------------

    #[test]
    fn tabs_always_divide_the_available_width_evenly_no_scroll_variant() {
        let many: Vec<Tab> = (0..6).map(|i| tab().label(format!("Tab {i}"))).collect();
        let view: TabsView<()> = tabs(many, 0, |_s: &mut (), _i: usize| {});
        let mut counter = 0u64;
        let mut w = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        let size = layout(&mut w, 300.0);
        assert_eq!(size.width, 300.0);
        assert_eq!(size.height, HEIGHT);
        let slot_w = 300.0 / 6.0;
        for (i, rect) in w.tab_rects.iter().enumerate() {
            assert_eq!(rect.x0, i as f64 * slot_w);
            assert_eq!(rect.width(), slot_w);
        }
    }

    // ---- Indicator geometry per variant ---------------------------------------

    #[test]
    fn primary_indicator_matches_content_width_not_slot_width() {
        let view = build_view(0);
        let mut w = build(&view);
        layout(&mut w, 300.0);
        let (left, width) = w.indicator_target(0);
        let slot = w.tab_rects[0];
        assert!(
            width < slot.width(),
            "primary indicator narrower than the slot"
        );
        assert!(left > slot.x0, "content indented from the slot's left edge");
        assert!(
            left + width < slot.x1,
            "content indented from the slot's right edge too"
        );
    }

    #[test]
    fn secondary_indicator_spans_the_whole_slot() {
        let view = build_view(0).variant(TabsVariant::Secondary);
        let mut w = build(&view);
        layout(&mut w, 300.0);
        let (left, width) = w.indicator_target(0);
        let slot = w.tab_rects[0];
        assert_eq!((left, width), (slot.x0, slot.width()));
    }

    // ---- Selection: controlled, disabled tabs ---------------------------------

    #[test]
    fn is_controlled_selection_only_moves_via_rebuild() {
        let mut w = build(&build_view(0));
        layout(&mut w, 300.0);
        let mut state = Vec::new();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 150.0, 20.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 150.0, 20.0));
        assert_eq!(w.selected, 0, "widget did not self-mutate");
        assert_eq!(state, vec![1]);

        let next = build_view(1);
        let prev = build_view(0);
        let mut counter = 0u64;
        View::<Vec<usize>>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert_eq!(w.selected, 1);
    }

    #[test]
    fn tap_reports_the_tapped_tabs_index() {
        let mut w = build(&build_view(0));
        layout(&mut w, 300.0);
        let mut state = Vec::new();
        // Tab 2 ("Three") occupies x in [200, 300).
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 250.0, 20.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 250.0, 20.0));
        assert_eq!(state, vec![2]);
    }

    #[test]
    fn up_outside_the_pressed_tab_does_not_fire() {
        let mut w = build(&build_view(0));
        layout(&mut w, 300.0);
        let mut state = Vec::new();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 20.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 250.0, 20.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 250.0, 20.0));
        assert!(state.is_empty());
    }

    #[test]
    fn cancel_clears_the_press_without_firing() {
        let mut w = build(&build_view(0));
        layout(&mut w, 300.0);
        let mut state = Vec::new();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 20.0));
        assert!(w.armed);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Cancel, 10.0, 20.0));
        assert!(!w.armed);
        assert!(state.is_empty());
    }

    #[test]
    fn disabled_tab_swallows_the_press_and_does_not_fire() {
        let view: TabsView<Vec<usize>> = tabs(
            [tab().label("A"), tab().label("B").enabled(false)],
            0,
            |s: &mut Vec<usize>, i| s.push(i),
        );
        let mut counter = 0u64;
        let mut w = View::<Vec<usize>>::build(&view, &mut BuildCtx::new(&mut counter));
        layout(&mut w, 200.0);
        let mut state = Vec::new();
        // Tab 1 ("B", disabled) occupies x in [100, 200).
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 150.0, 20.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 150.0, 20.0));
        assert!(
            state.is_empty(),
            "a disabled tab must not report a selection"
        );
        assert!(!w.armed, "a disabled tab must not arm a press at all");
    }

    // ---- Paint: reduced motion, dividers -------------------------------------

    #[test]
    fn reduce_motion_snaps_the_indicator_and_requests_one_frame_never_layout() {
        let mut theme = crate::baseline();
        theme.motion.reduce_motion = true;

        let prev = build_view(0);
        let mut w = build(&prev);
        layout_themed(&mut w, 300.0, Some(&theme));

        let next = build_view(2);
        let mut counter = 0u64;
        View::<Vec<usize>>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));

        struct NullScene;
        impl PaintScene for NullScene {
            fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
            fn draw_text(&mut self, _o: Point, _t: &str) {}
        }
        let mut scene = NullScene;
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(300.0, HEIGHT)).with_theme(&theme);
        w.paint(&mut ctx, &mut scene);

        assert!(
            ctx.needs_frame(),
            "the snap moved the indicator, so one more frame must be requested"
        );
        assert!(
            !ctx.needs_layout(),
            "the indicator is paint-only — it must never feed layout"
        );

        // A second paint at the now-settled position requests nothing more.
        let mut ctx2 = PaintCtx::new(Point::ZERO, Size::new(300.0, HEIGHT)).with_theme(&theme);
        w.paint(&mut ctx2, &mut scene);
        assert!(
            !ctx2.needs_frame(),
            "already at rest — no further frame needed"
        );
    }

    #[test]
    fn animating_indicator_requests_a_frame_each_advance() {
        let prev = build_view(0);
        let mut w = build(&prev);
        layout(&mut w, 300.0);

        let next = build_view(2);
        let mut counter = 0u64;
        View::<Vec<usize>>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));

        struct NullScene;
        impl PaintScene for NullScene {
            fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
            fn draw_text(&mut self, _o: Point, _t: &str) {}
        }
        let mut scene = NullScene;
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(300.0, HEIGHT));
        w.paint(&mut ctx, &mut scene);
        assert!(
            ctx.needs_frame(),
            "mid-flight spring keeps asking for frames"
        );
        assert!(!ctx.needs_layout(), "never feeds layout");
    }

    #[test]
    fn divider_is_stroked_along_the_bottom_edge() {
        let mut w = build(&build_view(0));
        layout(&mut w, 300.0);

        #[derive(Default)]
        struct StrokeRecorder {
            lines: Vec<(Point, Point, f64)>,
        }
        impl PaintScene for StrokeRecorder {
            fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
            fn draw_text(&mut self, _o: Point, _t: &str) {}
            fn stroke_line(&mut self, p0: Point, p1: Point, width: f64, _c: Color) {
                self.lines.push((p0, p1, width));
            }
        }
        let mut rec = StrokeRecorder::default();
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(300.0, HEIGHT));
        w.paint(&mut ctx, &mut rec);
        assert_eq!(rec.lines.len(), 1);
        let (p0, p1, width) = rec.lines[0];
        assert_eq!(p0.y, HEIGHT);
        assert_eq!(p1.y, HEIGHT);
        assert_eq!(width, DIVIDER_WIDTH);
    }

    // ---- Semantics --------------------------------------------------------------

    #[test]
    fn semantics_yields_a_tablist_of_tabs_with_selection_and_disabled() {
        fn logic(_s: &mut ()) -> TabsView<()> {
            tabs::<(), _>(
                [tab().label("A"), tab().label("B").enabled(false)],
                1,
                |_s: &mut (), _i| {},
            )
        }
        let mut root: frust_core::RenderRoot<(), TabsView<()>> = frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(300.0, HEIGHT), &mut tcx as &mut dyn Any);
        let update = root.semantics();

        let tablist = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::TabList)
            .expect("a TabList container node is contributed");
        assert_eq!(tablist.1.children().len(), 2);

        let tabs_found: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::Tab)
            .collect();
        assert_eq!(tabs_found.len(), 2);

        let a = tabs_found
            .iter()
            .find(|(_, n)| n.label() == Some("A"))
            .expect("tab A is present");
        assert_eq!(a.1.is_selected(), Some(false));
        assert!(!a.1.is_disabled());

        let b = tabs_found
            .iter()
            .find(|(_, n)| n.label() == Some("B"))
            .expect("tab B is present");
        assert_eq!(b.1.is_selected(), Some(true));
        assert!(b.1.is_disabled());
    }
}
