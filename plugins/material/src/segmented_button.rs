// Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
// `lib/components/segmented_buttons/m3e_segmented_buttons.dart` +
// `models/m3e_segment.dart` + `styles/m3e_segmented_button_theme.dart` +
// `components/m3e_segment_divider.dart` (retrieved 2026-08-20).
// Upstream: <https://github.com/paadevelopments/material_3_expressive>

//! The Material 3 Expressive **segmented button**: a row of 2-5 connected
//! segments for selecting an option, switching a view, or sorting — single-
//! or multi-select, with an optional checkmark on the selected member(s).
//!
//! [`segmented_button`] builds a [`SegmentedButtonView`]; [`segment`] builds
//! one [`Segment`] of it. It is a **controlled component** (see
//! `docs/CODE_STANDARDS.md`'s Interaction Semantics): a tap fires
//! `on_selection_changed` with the *requested* selection and leaves
//! `selected` untouched — the app feeds the confirmed selection back on the
//! next rebuild, exactly like [`mod@crate::button_group`]/[`mod@crate::switch`].
//!
//! # One `Vec<T>` shape for both selection modes
//!
//! The reference carries a single `Set<T> selected` field and a single
//! `ValueChanged<Set<T>> onSelectionChanged` callback regardless of
//! `multiSelect` — `_handleTap` (`m3e_segmented_buttons.dart:47-61`) is what
//! branches on the mode, not the widget's own shape. This port mirrors that
//! exactly rather than splitting into two view-fns with two different
//! reported-value types (`T` for single-select, `Vec<T>` for multi-select):
//! [`segmented_button`] always takes/reports `Vec<T>` (this framework's
//! ordered stand-in for Dart's `Set<T>` — order is caller-controlled and
//! never reshuffled by this widget), and [`SegmentedButtonView::multi_select`]
//! only changes what [`next_selection`] computes internally on a tap. One
//! controlled component, one reported-value type, matching the reference's
//! own single-widget shape.
//!
//! # Selection semantics: single can't deselect, multi toggles
//!
//! [`next_selection`] transcribes `_handleTap` exactly:
//! - **Single-select** (the default) always *replaces* the whole selection
//!   with the tapped value alone (`next..clear()..add(value)`) — including
//!   when the tapped segment is already the sole selected member. A
//!   single-select segmented button can never be tapped down to zero
//!   selected members; there is no toggle-off branch in the reference for
//!   this mode.
//! - **Multi-select** ([`SegmentedButtonView::multi_select`]`(true)`) toggles
//!   the tapped value's membership: absent → inserted, present → removed. A
//!   multi-select group *can* reach zero selected members.
//!
//! # Checkmark: swaps in over the segment's own icon, even a label-only one
//!
//! [`SegmentedButtonView::show_selected_icon`] (default `true`, matching the
//! reference) makes a selected segment's leading icon slot resolve to
//! [`crate::icons::CHECK`] instead of [`Segment::icon`] —
//! `_resolveLeading` (`m3e_segmented_buttons.dart:268-286`) checks
//! `selected && showSelectedIcon` *first*, unconditionally, before falling
//! back to the segment's own icon. One consequence transcribed faithfully
//! rather than "fixed": a label-only segment (no [`Segment::icon`]) gains a
//! leading checkmark the instant it is selected, which **changes that
//! segment's measured width** — this port's [`Widget::layout`] recomputes
//! icon presence from the *current* selection every layout pass, so the row
//! reflows exactly as the reference's `Row` does. **Porting decision:**
//! [`Segment::icon`] takes an [`frust::IconSource`] rather than the
//! reference's arbitrary `Widget? icon` — the swap target is itself a
//! same-kind [`frust::IconSource`] ([`crate::icons::CHECK`]), so both slots
//! stay the same closed shape, mirroring [`mod@crate::switch`]'s
//! `selected_icon`/`unselected_icon` pair rather than
//! [`mod@crate::toggle_button`]'s arbitrary-`AnyView` icon slot (a label is
//! still a plain string, painted the same directly-shaped-and-rebrushed way
//! [`mod@crate::toggle_button`]'s own `LabelRun` is).
//!
//! # Connected geometry: outer pill, square inner edges, hairline dividers
//!
//! The reference wraps the whole row in one `Container` whose
//! `borderRadius` is `M3EShapes.resolve(height / 2)` — a literal pill radius
//! for the group's *own* height, not a themed shape-scale rung — and clips
//! the row to it via `ClipRRect`; individual members are plain, un-rounded
//! `Container`s (`m3e_segmented_buttons.dart:80-98`). This port reproduces
//! the same pixels without a clip primitive: [`outer_radius`] resolves
//! [`frust::ShapeScale::full`] against the group's own `(width, height)` —
//! the theme-scale token whose [`frust::ShapeScale::resolve`] contract
//! (`min(width, height) / 2`, clamped-not-literal) coincides with the
//! reference's `height / 2` for every realistic (wider-than-tall) group, and
//! degrades gracefully instead of overflowing for a pathologically narrow
//! one the reference does not defend against — and [`segment_radii`] gives
//! only the leftmost segment's left corners / rightmost segment's right
//! corners that resolved radius, **`0.0`** (square) on every other corner,
//! including every inner, adjacent-segment corner. This is a different inner
//! value than [`mod@crate::button_group`]'s own `member_radii` (a small
//! nonzero [`frust::ShapeScale::extra_small`] rung) — that module clips
//! nothing and rounds every member independently; this one has no clip to
//! stand in for, so inner corners are literally square, exactly as the
//! reference's own un-rounded member `Container`s paint. Each member's
//! background/state-layer overlay is painted with its own [`segment_radii`]
//! via [`frust::authoring::PaintScene::fill_rounded_rect_radii`] — the same
//! per-corner-native paint call [`mod@crate::toggle_button`]'s own connected-group
//! corners use, rather than [`mod@crate::button_group`]'s `kurbo`-path route;
//! both reach the same pixels.
//!
//! **Dividers — solid color, not the reference's sampled gradient.** The
//! reference's `M3ESegmentDivider` samples an optional gradient
//! across the whole row via a `GlobalKey`-hosted `RenderBox`
//! (`components/m3e_segment_divider.dart`) so every hairline lines up with
//! one continuous ramp instead of each stretching its own copy over a
//! sub-pixel width. This framework has no GPU shader-gradient divider
//! primitive and no cross-widget geometry query a `GlobalKey` read would
//! need; this port paints every divider as a solid
//! [`frust::authoring::PaintScene::stroke_line`] in the theme's `outline`
//! role (`segmentedButtonTheme.divider`'s own default, since
//! `dividerColor`/`dividerGradient` are never set by any of this crate's
//! constructors) — the visual result for the only configuration this crate
//! ships (solid dividers), with the gradient-sampling case left unported.
//!
//! # Generic selection model — no `T` on the retained widget
//!
//! Mirrors [`mod@crate::radio`]'s own "Generic selection model" section
//! exactly: [`segmented_button`] takes owned `T: PartialEq + Clone +
//! 'static` values, but the retained [`SegmentedButtonWidget`] carries no
//! generic parameter of its own. [`bind_on_tap`] closes over
//! `on_selection_changed`, the *current* `selected`/`multi_select`, and one
//! segment's own `value` at `build`/`rebuild` time (where `T` is still in
//! scope) into a plain [`frust::authoring::ErasedCallback`] — the type-erase-
//! to-avoid-a-generic idiom `docs/CODE_STANDARDS.md`'s Language Idioms
//! describes for a downstream-crate dependency, applied here to a generic
//! instead. Closures aren't comparable, so every `rebuild` reinstalls the
//! whole `Vec` of them unconditionally, same as [`mod@crate::radio`]/
//! [`mod@crate::switch`]/[`mod@crate::button_group`]'s own callback rebinding.
//!
//! # State layer + press via the interaction core, no hover/focus
//!
//! Each segment tracks its own [`crate::interaction::InteractionState`] (the
//! newer, precedence-resolving substrate — see that module's docs), but only
//! ever calls [`crate::interaction::InteractionState::set_pressed`]: no row-
//! of-items widget in this catalog wires hover or focus claiming today
//! ([`mod@crate::button_group`], `navbar`, `chips` all leave both unwired
//! too), so this module follows the same established scope rather than
//! inventing a new per-segment hover-claim mechanism this task does not
//! need. The reference's `pressedScale` stays at its class default (`1`,
//! i.e. no press-scale spring — `M3ESegmentedButton` never overrides
//! `M3ETappable.pressedScale`), so unlike [`mod@crate::button_group`]'s own
//! shape-morph press emphasis, a pressed segment here paints a plain
//! [`crate::interaction::InteractionState::resolve_opacity`] state-layer
//! fill in its own foreground color — the same mechanism
//! [`mod@crate::icon_button`]/[`mod@crate::toggle_button`] use, standing in for
//! the reference's `M3EStateLayerOverlay` (Flutter's `InkWell` ripple this
//! framework has no equivalent of — this crate's whole-catalog convention,
//! see `mod@crate::icon_button`'s `suppress_ink` doc).
//!
//! Haptics fire [`crate::interaction::HapticSignal::None`] on every confirmed
//! tap through [`crate::interaction::MaterialHaptics::fire`] — `M3ETappable`'s
//! own default (`m3e_tappable.dart:36`'s `haptic = M3EHapticFeedback.none`,
//! never overridden by `M3ESegmentedButton`), a documented no-op today, wired
//! for a future revision to override, exactly [`mod@crate::switch`]'s own
//! Haptics section documents.
//!
//! # Bounds: 2-5 segments, `debug_assert`-checked
//!
//! [`segmented_button`] `debug_assert!`s [`MIN_SEGMENTS`]`..=`[`MAX_SEGMENTS`].
//! The reference's own constructor `assert`s only the lower bound
//! (`segments.length >= 2`, `m3e_segmented_buttons.dart:26`) — the upper
//! bound of 5 is the class doc's own stated scope ("Presents 2-5 connected
//! `M3ESegment`s", `m3e_segmented_buttons.dart:14`), not a second runtime
//! `assert` in the Dart source. This port pins *both* ends as one
//! `debug_assert!`, matching [`mod@crate::slider`]'s own precedent of
//! `debug_assert!`-checking a documented-but-unenforced-upstream invariant
//! rather than leaving it unchecked. [`Segment`]'s own "a segment needs a
//! label or an icon" invariant (`m3e_segment.dart:8-11`) is likewise
//! `debug_assert!`-checked, in [`segmented_button`] rather than at
//! [`segment`]-construction time, since this crate's builder pattern adds
//! `label`/`icon` *after* [`segment`] returns.

use std::rc::Rc;

use frust::authoring::text::{FontWeight, LineHeight, TextContext, TextLayout, TextStyle};
use frust::authoring::{
    Action, BoxConstraints, BuildCtx, ChangeFlags, CornerRadii, ErasedCallback, EventCtx,
    EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerPhase, Role, SemanticsCtx,
    Toggled, TypedArgCallback, View, Widget,
};
use frust::{IconData, IconSource, ShapeScale, Theme};
use kurbo::{Affine, BezPath, Point, Rect, RoundedRect, Shape, Size, Vec2};
use peniko::{Brush, Color};

use crate::interaction::{HapticSignal, InteractionState, MaterialHaptics};
use crate::press::presses;

/// Fewest segments a group may carry — the reference's own `assert`
/// (`m3e_segmented_buttons.dart:26`).
pub const MIN_SEGMENTS: usize = 2;
/// Most segments a group may carry — the reference class doc's stated scope
/// (`m3e_segmented_buttons.dart:14`; see the [module docs](self)' Bounds
/// section for why this is a `debug_assert!` here rather than a Dart
/// `assert`).
pub const MAX_SEGMENTS: usize = 5;

/// Fixed row height, in logical px (`M3ESegmentedButtonTheme.height`'s
/// default). Unthemed: `frust::Theme` has no matching per-component height
/// slot, so — like [`crate::toggle_button::CONNECTED_INNER_RADIUS`] — this
/// stays a plain constant.
const HEIGHT: f64 = 40.0;
/// Leading-icon side length, in logical px (`iconSize`'s default).
const ICON_SIZE: f64 = 18.0;
/// Horizontal padding inside each segment around its content, in logical px
/// (`segmentHorizontalPadding`'s default).
const SEGMENT_H_PADDING: f64 = 12.0;
/// Gap between a segment's icon and label, in logical px (`iconLabelGap`'s
/// default).
const ICON_LABEL_GAP: f64 = 8.0;
/// Group outline / divider / member-boundary stroke width, in logical px
/// (`borderWidth`'s default).
const BORDER_WIDTH: f64 = 1.0;
/// Tessellation tolerance for the group outline's `kurbo` path (matches
/// [`crate::button_group`]'s own `PATH_TOLERANCE`).
const PATH_TOLERANCE: f64 = 0.1;

/// The ink every label run is *shaped* with; never painted — recolored at
/// paint time instead of reshaped (see [`mod@crate::button`]'s
/// `SHAPING_INK` for why).
const SHAPING_INK: Color = Color::BLACK;

/// One row of the M3 type scale, as `(size, line_height, letter_spacing,
/// weight)` — the unthemed fallback for the segment label role (`labelLarge`,
/// `m3e_segmented_buttons.dart:237`'s `theme.typeScale.labelLarge`).
const LABEL_LARGE: (f32, f32, f32, FontWeight) = (14.0, 20.0, 0.1, FontWeight::MEDIUM);

/// Unthemed-fallback `outline` (a theme resolves `colors.outline`).
const OUTLINE: Color = Color::from_rgb8(0x79, 0x74, 0x7E);
/// Unthemed-fallback `secondary_container`.
const SECONDARY_CONTAINER: Color = Color::from_rgb8(0xE8, 0xDE, 0xF8);
/// Unthemed-fallback `on_secondary_container`.
const ON_SECONDARY_CONTAINER: Color = Color::from_rgb8(0x1D, 0x19, 0x2B);
/// Unthemed-fallback `on_surface`.
const ON_SURFACE: Color = Color::from_rgb8(0x1D, 0x1B, 0x20);

// ---- Selection logic (m3e_segmented_buttons.dart:47-61) -------------------

/// The `_handleTap` selection-update rule, pure and directly testable. See
/// the [module docs](self)' Selection semantics section for the exact
/// single-vs-multi contract this transcribes.
fn next_selection<T: PartialEq + Clone>(current: &[T], multi_select: bool, tapped: &T) -> Vec<T> {
    if multi_select {
        let mut next = current.to_vec();
        if let Some(pos) = next.iter().position(|v| v == tapped) {
            next.remove(pos);
        } else {
            next.push(tapped.clone());
        }
        next
    } else {
        vec![tapped.clone()]
    }
}

/// Bind one segment's tap handler at `build`/`rebuild` time, type-erasing
/// `T` out of the retained widget. See the [module docs](self)' Generic
/// selection model section.
fn bind_on_tap<State: 'static, T: PartialEq + Clone + 'static>(
    on_selection_changed: &TypedArgCallback<State, Vec<T>>,
    current_selected: &[T],
    multi_select: bool,
    tapped_value: T,
) -> ErasedCallback {
    let on_selection_changed = on_selection_changed.clone();
    let next = next_selection(current_selected, multi_select, &tapped_value);
    Box::new(move |ctx: &mut EventCtx| {
        MaterialHaptics::fire(HapticSignal::None);
        let state = ctx.state_mut::<State>();
        on_selection_changed(state, next.clone());
    })
}

// ---- Geometry helpers -------------------------------------------------------

/// Per-segment corner radii: `outer` on the leftmost segment's left corners /
/// rightmost segment's right corners, `0.0` (square) on every other corner —
/// see the [module docs](self)' Connected geometry section for why this
/// differs from [`crate::button_group::member_radii`]'s nonzero inner rung.
/// A single-segment group (`count == 1`, never reached at runtime — see
/// [`MIN_SEGMENTS`]) is all outer corners, the same convention
/// `crate::button_group::member_radii` documents for its own `count == 1`
/// case.
fn segment_radii(index: usize, count: usize, outer: f64) -> CornerRadii {
    let is_first = index == 0;
    let is_last = index + 1 == count;
    let left = if is_first { outer } else { 0.0 };
    let right = if is_last { outer } else { 0.0 };
    CornerRadii::new(left, right, right, left)
}

/// The group's outer corner radius, resolved against its own `(width,
/// height)` — see the [module docs](self)' Connected geometry section for
/// why [`ShapeScale::full`] (rather than a themed shape-scale rung) is the
/// right token here, and how its `resolve` contract coincides with the
/// reference's literal `height / 2`.
fn outer_radius(theme: Option<&Theme>, width: f64, height: f64) -> f64 {
    match theme {
        Some(theme) => ShapeScale::resolve(theme.shape.full, width, height),
        None => width.min(height) / 2.0,
    }
}

/// `color` with its alpha channel replaced by `alpha`.
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// Whether two optional icon sources name the same geometry (`d`/`design`
/// equality) — lets `rebuild` skip re-resolving an unchanged icon, mirroring
/// [`mod@crate::switch`]'s own `same_icon_source`.
fn same_icon_source(a: Option<IconSource>, b: Option<IconSource>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => x.d == y.d && x.design == y.design,
        (None, None) => true,
        _ => false,
    }
}

/// The unthemed/themed `(outline, divider, selected_bg, selected_fg,
/// unselected_fg)` color table — `M3ESegmentedButtonTheme.outline`/
/// `.divider`/`.backgroundColor`/`.foregroundColor`
/// (`styles/m3e_segmented_button_theme.dart:83`-`:101`). `divider` defaults
/// to `outline` (never overridden by any constructor in this crate — see the
/// [module docs](self)' Dividers section).
struct Colors {
    outline: Color,
    divider: Color,
    selected_bg: Color,
    selected_fg: Color,
    unselected_fg: Color,
}

fn resolve_colors(theme: Option<&Theme>) -> Colors {
    match theme {
        Some(theme) => {
            let s = theme.scheme();
            Colors {
                outline: s.outline,
                divider: s.outline,
                selected_bg: s.secondary_container,
                selected_fg: s.on_secondary_container,
                unselected_fg: s.on_surface,
            }
        }
        None => Colors {
            outline: OUTLINE,
            divider: OUTLINE,
            selected_bg: SECONDARY_CONTAINER,
            selected_fg: ON_SECONDARY_CONTAINER,
            unselected_fg: ON_SURFACE,
        },
    }
}

/// The segment label's type role (`labelLarge`, themed) or its unthemed
/// [`LABEL_LARGE`] fallback. The returned style always carries
/// [`SHAPING_INK`] (see that constant's doc).
fn label_style(theme: Option<&Theme>) -> TextStyle {
    let mut style = match theme {
        Some(theme) => theme.type_scale.label_large.clone(),
        None => {
            let (size, line_height, letter_spacing, weight) = LABEL_LARGE;
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

// ---- The label run (sibling of toggle_button::LabelRun) -------------------

/// A lazily-shaped, paint-time-rebrushed label run — a sibling copy of
/// [`mod@crate::toggle_button`]'s own `LabelRun` (`pub(self)` to that module and
/// out of scope to import), scoped down to what this module needs: no
/// max-width squeeze (this module does not implement one — see the [module
/// docs](self)' Connected geometry section's sibling scope note).
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

// ---- View -------------------------------------------------------------------

/// One option within a [`SegmentedButtonView`] — the reference's
/// `M3ESegment<T>` (`models/m3e_segment.dart`).
pub struct Segment<T> {
    value: T,
    label: Option<String>,
    icon: Option<IconSource>,
}

/// Create a segment reporting `value` when tapped/selected. Attach
/// [`Segment::label`] and/or [`Segment::icon`] — a segment needs at least
/// one (`debug_assert!`-checked by [`segmented_button`]; see the [module
/// docs](self)' Bounds section).
pub fn segment<T>(value: T) -> Segment<T> {
    Segment {
        value,
        label: None,
        icon: None,
    }
}

impl<T> Segment<T> {
    /// Attach a text label, shaped in the group's `labelLarge` role.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Attach a leading icon — replaced by [`crate::icons::CHECK`] while
    /// this segment is selected and [`SegmentedButtonView::show_selected_icon`]
    /// is set (the default). See the [module docs](self)' Checkmark section.
    pub fn icon(mut self, icon: IconSource) -> Self {
        self.icon = Some(icon);
        self
    }
}

/// A view-held, typed selection callback (erased per-segment on build/rebuild
/// — see the [module docs](self)' Generic selection model section).
type OnSelectionChanged<State, T> = TypedArgCallback<State, Vec<T>>;

/// A declarative M3 Expressive segmented button. See the [module docs](self).
pub struct SegmentedButtonView<State: 'static, T: PartialEq + Clone + 'static> {
    segments: Vec<Segment<T>>,
    selected: Vec<T>,
    multi_select: bool,
    show_selected_icon: bool,
    on_selection_changed: OnSelectionChanged<State, T>,
}

/// Create a segmented button from `segments` (2-5, `debug_assert!`-checked —
/// see [`MIN_SEGMENTS`]/[`MAX_SEGMENTS`]), reflecting `selected` (the
/// currently-confirmed selection). Single-select by default; call
/// [`SegmentedButtonView::multi_select`]`(true)` to allow more than one
/// member selected at once. `on_selection_changed` fires with the
/// *requested* next selection on release inside a member — see the [module
/// docs](self)' Selection semantics section.
pub fn segmented_button<
    State: 'static,
    T: PartialEq + Clone + 'static,
    F: Fn(&mut State, Vec<T>) + 'static,
>(
    segments: impl IntoIterator<Item = Segment<T>>,
    selected: impl IntoIterator<Item = T>,
    on_selection_changed: F,
) -> SegmentedButtonView<State, T> {
    let segments: Vec<Segment<T>> = segments.into_iter().collect();
    debug_assert!(
        (MIN_SEGMENTS..=MAX_SEGMENTS).contains(&segments.len()),
        "a segmented button needs {MIN_SEGMENTS}-{MAX_SEGMENTS} segments \
         (m3e_segmented_buttons.dart:26 asserts >= 2 in code; its class doc \
         pins the upper bound at 5), got {}",
        segments.len()
    );
    for s in &segments {
        debug_assert!(
            s.label.is_some() || s.icon.is_some(),
            "a segment needs at least a label or an icon (m3e_segment.dart:8-11)"
        );
    }
    SegmentedButtonView {
        segments,
        selected: selected.into_iter().collect(),
        multi_select: false,
        show_selected_icon: true,
        on_selection_changed: Rc::new(on_selection_changed),
    }
}

/// PascalCase alias for [`segmented_button`], matching this catalog's
/// view-fn vocabulary.
#[allow(non_snake_case)]
pub fn SegmentedButton<
    State: 'static,
    T: PartialEq + Clone + 'static,
    F: Fn(&mut State, Vec<T>) + 'static,
>(
    segments: impl IntoIterator<Item = Segment<T>>,
    selected: impl IntoIterator<Item = T>,
    on_selection_changed: F,
) -> SegmentedButtonView<State, T> {
    segmented_button(segments, selected, on_selection_changed)
}

impl<State: 'static, T: PartialEq + Clone + 'static> SegmentedButtonView<State, T> {
    /// Allow more than one segment selected at once, toggling membership on
    /// tap instead of replacing the whole selection. See the [module
    /// docs](self)' Selection semantics section.
    pub fn multi_select(mut self, multi_select: bool) -> Self {
        self.multi_select = multi_select;
        self
    }

    /// Whether a selected segment swaps its leading icon for
    /// [`crate::icons::CHECK`] (default `true`). See the [module
    /// docs](self)' Checkmark section.
    pub fn show_selected_icon(mut self, show: bool) -> Self {
        self.show_selected_icon = show;
        self
    }
}

// ---- Widget -------------------------------------------------------------------

/// The retained widget for a [`SegmentedButtonView`] — carries no generic
/// parameter; see the [module docs](self)' Generic selection model section.
pub struct SegmentedButtonWidget {
    /// One label run per segment (`None` for an icon-only segment).
    labels: Vec<Option<LabelRun>>,
    /// Each segment's own resolved icon geometry (`None` for a label-only
    /// segment) — unaffected by selection; the checkmark swap happens in
    /// [`Self::icon_geom`].
    icon_geoms: Vec<Option<(BezPath, f64)>>,
    /// [`crate::icons::CHECK`]'s geometry, resolved once (never changes).
    check_icon_geom: (BezPath, f64),
    /// Per-segment selection membership, recomputed from `selected` on
    /// every build/rebuild.
    selected_flags: Vec<bool>,
    show_selected_icon: bool,
    multi_select: bool,
    /// Per-segment background rects in the widget's local space, computed in
    /// [`Widget::layout`] and used for paint + hit-testing.
    segment_rects: Vec<Rect>,
    /// Per-segment hover/focus/pressed/dragged tracking; only `pressed` is
    /// ever set — see the [module docs](self)' State layer section.
    interactions: Vec<InteractionState>,
    /// Whether a press is currently armed (a `Down` landed on a segment).
    armed: bool,
    /// The segment a press is currently tracking.
    active_segment: Option<usize>,
    /// Whether the armed pointer is currently inside [`Self::active_segment`].
    pressed_inside: bool,
    /// One bound tap handler per segment — see [`bind_on_tap`].
    on_taps: Vec<ErasedCallback>,
}

impl SegmentedButtonWidget {
    /// The geometry to paint in segment `i`'s leading-icon slot: the shared
    /// checkmark while selected and [`SegmentedButtonWidget::show_selected_icon`],
    /// else the segment's own icon (if any). See the [module docs](self)'
    /// Checkmark section.
    fn icon_geom(&self, i: usize) -> Option<&(BezPath, f64)> {
        if self.selected_flags[i] && self.show_selected_icon {
            Some(&self.check_icon_geom)
        } else {
            self.icon_geoms[i].as_ref()
        }
    }

    /// Whether segment `i` currently has an effective leading icon.
    fn has_icon(&self, i: usize) -> bool {
        self.icon_geom(i).is_some()
    }

    /// Which segment (if any) contains local point `pos`.
    fn segment_at(&self, pos: Point) -> Option<usize> {
        self.segment_rects.iter().position(|r| r.contains(pos))
    }
}

/// The per-segment label-run/icon-geometry vectors [`build_segment_runtime`]
/// returns.
type SegmentRuntime = (Vec<Option<LabelRun>>, Vec<Option<(BezPath, f64)>>);

/// Build the per-segment label-run/icon-geometry vectors from `segments` —
/// shared by `build` and a structural (segment-count-changed) `rebuild`.
fn build_segment_runtime<T>(segments: &[Segment<T>]) -> SegmentRuntime {
    let labels = segments
        .iter()
        .map(|s| s.label.as_ref().map(|l| LabelRun::new(l.clone())))
        .collect();
    let icon_geoms = segments
        .iter()
        .map(|s| s.icon.map(|src| IconData::from(src).resolve()))
        .collect();
    (labels, icon_geoms)
}

fn compute_selected_flags<T: PartialEq>(segments: &[Segment<T>], selected: &[T]) -> Vec<bool> {
    segments
        .iter()
        .map(|s| selected.iter().any(|v| v == &s.value))
        .collect()
}

fn bind_on_taps<State: 'static, T: PartialEq + Clone + 'static>(
    segments: &[Segment<T>],
    on_selection_changed: &OnSelectionChanged<State, T>,
    selected: &[T],
    multi_select: bool,
) -> Vec<ErasedCallback> {
    segments
        .iter()
        .map(|s| {
            bind_on_tap::<State, T>(
                on_selection_changed,
                selected,
                multi_select,
                s.value.clone(),
            )
        })
        .collect()
}

impl<State: 'static, T: PartialEq + Clone + 'static> View<State> for SegmentedButtonView<State, T> {
    type Element = SegmentedButtonWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> SegmentedButtonWidget {
        let (labels, icon_geoms) = build_segment_runtime(&self.segments);
        let selected_flags = compute_selected_flags(&self.segments, &self.selected);
        let on_taps = bind_on_taps::<State, T>(
            &self.segments,
            &self.on_selection_changed,
            &self.selected,
            self.multi_select,
        );
        let count = self.segments.len();
        SegmentedButtonWidget {
            labels,
            icon_geoms,
            check_icon_geom: IconData::from(crate::icons::CHECK).resolve(),
            selected_flags,
            show_selected_icon: self.show_selected_icon,
            multi_select: self.multi_select,
            segment_rects: vec![Rect::ZERO; count],
            interactions: vec![InteractionState::new(); count],
            armed: false,
            active_segment: None,
            pressed_inside: false,
            on_taps,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SegmentedButtonWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;

        if prev.segments.len() != self.segments.len() {
            // Structural: rebuild every per-segment vector fresh.
            let (labels, icon_geoms) = build_segment_runtime(&self.segments);
            element.labels = labels;
            element.icon_geoms = icon_geoms;
            element.segment_rects = vec![Rect::ZERO; self.segments.len()];
            element.interactions = vec![InteractionState::new(); self.segments.len()];
            element.armed = false;
            element.active_segment = None;
            element.pressed_inside = false;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else {
            for (i, (prev_s, next_s)) in prev.segments.iter().zip(self.segments.iter()).enumerate()
            {
                match (&prev_s.label, &next_s.label) {
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
                if !same_icon_source(prev_s.icon, next_s.icon) {
                    element.icon_geoms[i] = next_s.icon.map(|src| IconData::from(src).resolve());
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
            }
        }

        // Selection membership can affect both which icon shows and the
        // measured content width — see the [module docs](self)' Checkmark
        // section.
        let selected_flags = compute_selected_flags(&self.segments, &self.selected);
        if selected_flags != element.selected_flags {
            element.selected_flags = selected_flags;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        if element.show_selected_icon != self.show_selected_icon {
            element.show_selected_icon = self.show_selected_icon;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.multi_select != self.multi_select {
            element.multi_select = self.multi_select;
            flags |= ChangeFlags::PAINT;
        }

        // Closures aren't comparable — always reinstall, mirroring
        // `radio`/`switch`/`button_group`'s own callback rebinding.
        element.on_taps = bind_on_taps::<State, T>(
            &self.segments,
            &self.on_selection_changed,
            &self.selected,
            self.multi_select,
        );

        flags
    }
}

impl Widget for SegmentedButtonWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let count = self.labels.len();
        let style = {
            let theme = Theme::from_layout_ctx(ctx);
            label_style(theme)
        };

        let mut content_widths = Vec::with_capacity(count);
        for i in 0..count {
            let has_icon = self.has_icon(i);
            let has_label = self.labels[i].is_some();
            let label_w = if let Some(run) = self.labels[i].as_mut() {
                run.shape(ctx, &style).width
            } else {
                0.0
            };
            let icon_w = if has_icon { ICON_SIZE } else { 0.0 };
            let gap = if has_icon && has_label {
                ICON_LABEL_GAP
            } else {
                0.0
            };
            content_widths.push(icon_w + gap + label_w + SEGMENT_H_PADDING * 2.0);
        }

        let dividers = count.saturating_sub(1) as f64 * BORDER_WIDTH;
        let total_w: f64 = content_widths.iter().sum::<f64>() + dividers;
        let size = bc.constrain(Size::new(total_w, HEIGHT));

        self.segment_rects.clear();
        let mut x = 0.0_f64;
        for w in &content_widths {
            self.segment_rects.push(Rect::new(x, 0.0, x + w, HEIGHT));
            x += w + BORDER_WIDTH;
        }

        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let origin = ctx.origin();
        let size = ctx.size();
        let count = self.segment_rects.len();

        let (colors, outer) = {
            let theme = Theme::from_paint_ctx(ctx);
            (
                resolve_colors(theme),
                outer_radius(theme, size.width, size.height),
            )
        };

        // Per-segment backgrounds, state layer, and content.
        for i in 0..count {
            let rect = self.segment_rects[i];
            let radii = segment_radii(i, count, outer);
            let selected = self.selected_flags[i];
            let fg = if selected {
                colors.selected_fg
            } else {
                colors.unselected_fg
            };
            let seg_origin = Point::new(origin.x + rect.x0, origin.y + rect.y0);

            if selected {
                scene.fill_rounded_rect_radii(seg_origin, rect.size(), radii, colors.selected_bg);
            }

            let opacity = self.interactions[i].resolve_opacity();
            if opacity > 0.0 {
                scene.fill_rounded_rect_radii(
                    seg_origin,
                    rect.size(),
                    radii,
                    with_alpha(fg, opacity),
                );
            }

            let has_icon = self.has_icon(i);
            let has_label = self.labels[i].is_some();
            let icon_w = if has_icon { ICON_SIZE } else { 0.0 };
            let gap = if has_icon && has_label {
                ICON_LABEL_GAP
            } else {
                0.0
            };
            let label_w = self.labels[i].as_ref().map_or(0.0, |run| run.size().width);
            let content_w = icon_w + gap + label_w;
            let content_x = rect.x0 + (rect.width() - content_w) / 2.0;

            if let Some((path, design)) = self.icon_geom(i) {
                let scale = if *design > 0.0 {
                    ICON_SIZE / *design
                } else {
                    1.0
                };
                let icon_x = content_x;
                let icon_y = rect.y0 + (rect.height() - ICON_SIZE) / 2.0;
                let transform = Affine::translate(Vec2::new(icon_x, icon_y)) * Affine::scale(scale);
                let scaled = transform * path.clone();
                scene.fill_path(origin, &scaled, &Brush::Solid(fg));
            }

            if let Some(run) = &self.labels[i] {
                let label_x = content_x + icon_w + gap;
                let label_y = rect.y0 + (rect.height() - run.size().height) / 2.0;
                run.paint(
                    Point::new(origin.x + label_x, origin.y + label_y),
                    fg,
                    scene,
                );
            }
        }

        // Group outline ring.
        let ring_rect = Rect::from_origin_size(Point::ORIGIN, size);
        let ring_path = RoundedRect::from_rect(ring_rect, outer).to_path(PATH_TOLERANCE);
        scene.stroke_path(
            origin,
            &ring_path,
            BORDER_WIDTH,
            &Brush::Solid(colors.outline),
        );

        // Segment dividers.
        for i in 1..count {
            let x = self.segment_rects[i - 1].x1 + BORDER_WIDTH / 2.0;
            scene.stroke_line(
                Point::new(origin.x + x, origin.y),
                Point::new(origin.x + x, origin.y + size.height),
                BORDER_WIDTH,
                colors.divider,
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
                let Some(i) = self.segment_at(p.position) else {
                    return EventResult::Ignored;
                };
                self.armed = true;
                self.active_segment = Some(i);
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
                let Some(i) = self.active_segment else {
                    return EventResult::Ignored;
                };
                let inside = self
                    .segment_rects
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
                if let Some(i) = self.active_segment {
                    self.interactions[i].set_pressed(false);
                    if self.pressed_inside {
                        (self.on_taps[i])(ctx);
                    }
                }
                self.armed = false;
                self.pressed_inside = false;
                self.active_segment = None;
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.armed {
                    return EventResult::Ignored;
                }
                if let Some(i) = self.active_segment {
                    self.interactions[i].set_pressed(false);
                }
                self.armed = false;
                self.pressed_inside = false;
                self.active_segment = None;
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // Single-select reads as a RadioGroup of RadioButtons (mirroring
        // `crate::button_group`); multi-select reads as a plain Group of
        // CheckBoxes, since more than one member can be toggled true.
        let group_role = if self.multi_select {
            Role::Group
        } else {
            Role::RadioGroup
        };
        let multi_select = self.multi_select;
        let labels = &self.labels;
        let selected_flags = &self.selected_flags;
        ctx.push_container(
            group_role,
            |_| {},
            |ctx| {
                for (i, selected) in selected_flags.iter().enumerate() {
                    let child_role = if multi_select {
                        Role::CheckBox
                    } else {
                        Role::RadioButton
                    };
                    let label = labels[i].as_ref().map(LabelRun::content);
                    ctx.push_node(child_role, |node| {
                        if let Some(l) = label {
                            node.set_label(l);
                        }
                        if multi_select {
                            node.set_toggled(if *selected {
                                Toggled::True
                            } else {
                                Toggled::False
                            });
                        } else {
                            node.set_selected(*selected);
                        }
                        node.add_action(Action::Click);
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

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Density {
        Compact,
        Cozy,
        Comfortable,
    }

    fn build_view(selected: Vec<Density>) -> SegmentedButtonView<Vec<Density>, Density> {
        segmented_button(
            [
                segment(Density::Compact).label("Compact"),
                segment(Density::Cozy).label("Cozy"),
                segment(Density::Comfortable).label("Comfortable"),
            ],
            selected,
            |s: &mut Vec<Density>, next| *s = next,
        )
    }

    fn build_widget(selected: Vec<Density>) -> SegmentedButtonWidget {
        let view = build_view(selected);
        let mut counter = 0u64;
        View::<Vec<Density>>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    fn with_rects(w: &mut SegmentedButtonWidget, widths: &[f64]) {
        w.segment_rects.clear();
        let mut x = 0.0;
        for width in widths {
            w.segment_rects.push(Rect::new(x, 0.0, x + width, HEIGHT));
            x += width + BORDER_WIDTH;
        }
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    // `state` must stay `&mut Vec<Density>`, not `&mut [Density]`: it is
    // coerced to `&mut dyn Any` below, and `EventCtx::state_mut::<Vec<Density>>`
    // downcasts against that concrete type — a slice would erase the wrong
    // type and break every closure's downcast.
    #[allow(clippy::ptr_arg)]
    fn dispatch(w: &mut SegmentedButtonWidget, state: &mut Vec<Density>, event: &InputEvent) {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(300.0, HEIGHT));
        w.event(&mut ctx, event);
    }

    // ---- next_selection (the `_handleTap` rule) ---------------------------

    #[test]
    fn single_select_replaces_the_whole_selection() {
        let next = next_selection(&[Density::Compact], false, &Density::Cozy);
        assert_eq!(next, vec![Density::Cozy]);
    }

    #[test]
    fn single_select_tapping_the_already_selected_member_cannot_deselect() {
        // Upstream's `_handleTap` runs `clear()` then `add(value)`
        // unconditionally for single-select — there is no toggle-off branch.
        let next = next_selection(&[Density::Cozy], false, &Density::Cozy);
        assert_eq!(
            next,
            vec![Density::Cozy],
            "a single-select tap on the already-selected member must not clear it"
        );
    }

    #[test]
    fn multi_select_toggles_an_absent_value_in() {
        let next = next_selection(&[Density::Compact], true, &Density::Cozy);
        assert_eq!(next, vec![Density::Compact, Density::Cozy]);
    }

    #[test]
    fn multi_select_toggles_a_present_value_out() {
        let next = next_selection(&[Density::Compact, Density::Cozy], true, &Density::Cozy);
        assert_eq!(
            next,
            vec![Density::Compact],
            "multi-select can reach a reduced (even empty) selection"
        );
    }

    #[test]
    fn multi_select_can_reach_zero_selected_members() {
        let next = next_selection(&[Density::Compact], true, &Density::Compact);
        assert!(next.is_empty());
    }

    // ---- segment_radii ------------------------------------------------------

    #[test]
    fn single_segment_group_is_all_outer_corners() {
        let r = segment_radii(0, 1, 20.0);
        assert_eq!(r.top_left, 20.0);
        assert_eq!(r.top_right, 20.0);
        assert_eq!(r.bottom_right, 20.0);
        assert_eq!(r.bottom_left, 20.0);
    }

    #[test]
    fn end_segments_get_outer_on_their_outer_side_and_square_elsewhere() {
        let first = segment_radii(0, 3, 20.0);
        assert_eq!((first.top_left, first.bottom_left), (20.0, 20.0));
        assert_eq!((first.top_right, first.bottom_right), (0.0, 0.0));
        let last = segment_radii(2, 3, 20.0);
        assert_eq!((last.top_left, last.bottom_left), (0.0, 0.0));
        assert_eq!((last.top_right, last.bottom_right), (20.0, 20.0));
    }

    #[test]
    fn middle_segments_are_entirely_square() {
        let mid = segment_radii(1, 3, 20.0);
        assert_eq!(mid.top_left, 0.0);
        assert_eq!(mid.top_right, 0.0);
        assert_eq!(mid.bottom_right, 0.0);
        assert_eq!(mid.bottom_left, 0.0);
    }

    // ---- Bounds (2-5 segments, label-or-icon) --------------------------------

    #[test]
    #[should_panic(expected = "needs 2-5 segments")]
    fn fewer_than_two_segments_panics() {
        let _ = segmented_button::<(), u32, _>(
            [Segment {
                value: 0,
                label: Some("Only".into()),
                icon: None,
            }],
            [],
            |_s: &mut (), _v| {},
        );
    }

    #[test]
    #[should_panic(expected = "needs 2-5 segments")]
    fn more_than_five_segments_panics() {
        let segs: Vec<Segment<u32>> = (0..6).map(|i| segment(i).label(i.to_string())).collect();
        let _ = segmented_button::<(), u32, _>(segs, [], |_s: &mut (), _v| {});
    }

    #[test]
    fn exactly_two_and_exactly_five_segments_are_accepted() {
        let two: Vec<Segment<u32>> = (0..2).map(|i| segment(i).label(i.to_string())).collect();
        let _ = segmented_button::<(), u32, _>(two, [], |_s: &mut (), _v| {});
        let five: Vec<Segment<u32>> = (0..5).map(|i| segment(i).label(i.to_string())).collect();
        let _ = segmented_button::<(), u32, _>(five, [], |_s: &mut (), _v| {});
    }

    #[test]
    #[should_panic(expected = "label or an icon")]
    fn a_segment_with_neither_label_nor_icon_panics() {
        let _ = segmented_button::<(), u32, _>(
            [
                Segment {
                    value: 0,
                    label: None,
                    icon: None,
                },
                segment(1u32).label("B"),
            ],
            [],
            |_s: &mut (), _v| {},
        );
    }

    // ---- Generic T (enum), build/rebuild -------------------------------------

    #[test]
    fn build_creates_one_run_per_segment_for_a_generic_enum_value() {
        let w = build_widget(vec![Density::Cozy]);
        assert_eq!(w.labels.len(), 3);
        assert_eq!(w.selected_flags, vec![false, true, false]);
    }

    #[test]
    fn is_controlled_selection_only_moves_via_rebuild() {
        let mut w = build_widget(vec![Density::Compact]);
        with_rects(&mut w, &[80.0, 80.0, 80.0]);
        let mut state = vec![Density::Compact];
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 100.0, 20.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 100.0, 20.0));
        assert_eq!(
            w.selected_flags,
            vec![true, false, false],
            "widget did not self-mutate — its own flags still reflect the original selection"
        );

        let next = build_view(vec![Density::Cozy]);
        let prev = build_view(vec![Density::Compact]);
        let mut counter = 0u64;
        View::<Vec<Density>>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert_eq!(w.selected_flags, vec![false, true, false]);
    }

    #[test]
    fn tap_reports_the_tapped_segments_value_single_select() {
        let mut w = build_widget(vec![Density::Compact]);
        with_rects(&mut w, &[80.0, 80.0, 80.0]);
        let mut state = vec![Density::Compact];
        // Middle segment starts at x = 81 (80 + BORDER_WIDTH).
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 100.0, 20.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 100.0, 20.0));
        assert_eq!(state, vec![Density::Cozy]);
    }

    #[test]
    fn tap_on_the_selected_single_select_member_does_not_clear_it() {
        let mut w = build_widget(vec![Density::Compact]);
        with_rects(&mut w, &[80.0, 80.0, 80.0]);
        let mut state = vec![Density::Compact];
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 20.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 10.0, 20.0));
        assert_eq!(state, vec![Density::Compact]);
    }

    #[test]
    fn multi_select_tap_toggles_membership() {
        let view = segmented_button(
            [
                segment(Density::Compact).label("Compact"),
                segment(Density::Cozy).label("Cozy"),
                segment(Density::Comfortable).label("Comfortable"),
            ],
            vec![Density::Compact],
            |s: &mut Vec<Density>, next| *s = next,
        )
        .multi_select(true);
        let mut counter = 0u64;
        let mut w = View::<Vec<Density>>::build(&view, &mut BuildCtx::new(&mut counter));
        with_rects(&mut w, &[80.0, 80.0, 80.0]);
        let mut state = vec![Density::Compact];

        // Tap Cozy: should add, not replace.
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 100.0, 20.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 100.0, 20.0));
        assert_eq!(state, vec![Density::Compact, Density::Cozy]);
    }

    #[test]
    fn up_outside_the_pressed_segment_does_not_fire() {
        let mut w = build_widget(vec![Density::Compact]);
        with_rects(&mut w, &[80.0, 80.0, 80.0]);
        let mut state = vec![Density::Compact];
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 20.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 100.0, 20.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 100.0, 20.0));
        assert_eq!(state, vec![Density::Compact]);
    }

    #[test]
    fn cancel_clears_the_press_without_firing() {
        let mut w = build_widget(vec![Density::Compact]);
        with_rects(&mut w, &[80.0, 80.0, 80.0]);
        let mut state = vec![Density::Compact];
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 20.0));
        assert!(w.armed);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Cancel, 10.0, 20.0));
        assert!(!w.armed);
        assert_eq!(state, vec![Density::Compact]);
    }

    #[test]
    fn down_outside_all_segments_is_ignored() {
        let mut w = build_widget(vec![Density::Compact]);
        with_rects(&mut w, &[80.0, 80.0, 80.0]);
        let mut state = vec![Density::Compact];
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(300.0, HEIGHT));
        let result = w.event(&mut ctx, &ev(PointerPhase::Down, 1000.0, 20.0));
        assert!(matches!(result, EventResult::Ignored));
        assert!(w.active_segment.is_none());
    }

    // ---- Checkmark swap -------------------------------------------------------

    #[test]
    fn checkmark_swaps_in_only_for_the_selected_segment() {
        let w = build_widget(vec![Density::Cozy]);
        // Cozy (index 1) is selected: its slot resolves to the shared CHECK
        // geometry, not its own (absent) icon.
        assert!(w.has_icon(1));
        let (check_path, check_design) = &w.check_icon_geom;
        let (resolved_path, resolved_design) = w.icon_geom(1).expect("selected slot has an icon");
        assert_eq!(resolved_design, check_design);
        assert_eq!(resolved_path.elements().len(), check_path.elements().len());
        // The unselected, label-only segments have no icon at all.
        assert!(!w.has_icon(0));
        assert!(!w.has_icon(2));
    }

    #[test]
    fn show_selected_icon_false_suppresses_the_checkmark() {
        let view = segmented_button(
            [
                segment(Density::Compact).label("Compact"),
                segment(Density::Cozy).label("Cozy"),
            ],
            vec![Density::Cozy],
            |_s: &mut (), _v: Vec<Density>| {},
        )
        .show_selected_icon(false);
        let mut counter = 0u64;
        let w = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        assert!(
            !w.has_icon(1),
            "checkmark suppressed by show_selected_icon(false)"
        );
    }

    #[test]
    fn a_segments_own_icon_survives_while_unselected() {
        let view = segmented_button::<(), u32, _>(
            [
                segment(0u32).icon(crate::icons::SEARCH),
                segment(1u32).label("Text only"),
            ],
            [],
            |_s: &mut (), _v| {},
        );
        let mut counter = 0u64;
        let w = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        assert!(
            w.has_icon(0),
            "unselected segment 0 still shows its own icon"
        );
        assert!(
            !w.has_icon(1),
            "label-only, unselected segment 1 has no icon"
        );
    }

    // ---- Paint: dividers, outline, backgrounds ---------------------------------

    /// A recording scene capturing filled rounded-rect (per-corner) calls,
    /// stroked lines (dividers), and stroked paths (the outline ring).
    #[derive(Default)]
    struct PaintRecorder {
        radii_fills: Vec<(Point, Size, CornerRadii, Color)>,
        lines: usize,
        strokes: usize,
    }

    impl PaintScene for PaintRecorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect_radii(&mut self, o: Point, s: Size, radii: CornerRadii, color: Color) {
            self.radii_fills.push((o, s, radii, color));
        }
        fn stroke_line(&mut self, _p0: Point, _p1: Point, _w: f64, _c: Color) {
            self.lines += 1;
        }
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, _w: f64, _b: &Brush) {
            self.strokes += 1;
        }
        fn fill_path(&mut self, _o: Point, _p: &BezPath, _b: &Brush) {}
    }

    fn layout_widget(w: &mut SegmentedButtonWidget, width: f64) -> Size {
        let bc = BoxConstraints::loose(Size::new(width, f64::INFINITY));
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut ctx, &bc)
    }

    #[test]
    fn divider_count_is_segment_count_minus_one() {
        let mut w = build_widget(vec![Density::Compact]);
        layout_widget(&mut w, 400.0);
        let mut rec = PaintRecorder::default();
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(300.0, HEIGHT));
        w.paint(&mut ctx, &mut rec);
        assert_eq!(rec.lines, 2, "3 segments have 2 internal dividers");
        assert_eq!(rec.strokes, 1, "exactly one outline ring stroke");
    }

    #[test]
    fn only_the_selected_segment_paints_a_background_fill() {
        let mut w = build_widget(vec![Density::Cozy]);
        layout_widget(&mut w, 400.0);
        let mut rec = PaintRecorder::default();
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(300.0, HEIGHT));
        w.paint(&mut ctx, &mut rec);
        assert_eq!(
            rec.radii_fills.len(),
            1,
            "only the selected member fills a background"
        );
    }

    #[test]
    fn themed_paint_resolves_container_and_outline_tokens() {
        let theme = crate::baseline();
        let mut w = build_widget(vec![Density::Compact]);
        layout_widget(&mut w, 400.0);

        #[derive(Default)]
        struct ColorRec {
            fills: Vec<Color>,
        }
        impl PaintScene for ColorRec {
            fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
            fn draw_text(&mut self, _o: Point, _t: &str) {}
            fn fill_rounded_rect_radii(
                &mut self,
                _o: Point,
                _s: Size,
                _r: CornerRadii,
                color: Color,
            ) {
                self.fills.push(color);
            }
        }
        let mut rec = ColorRec::default();
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(300.0, HEIGHT)).with_theme(&theme);
        w.paint(&mut ctx, &mut rec);
        let scheme = theme.scheme();
        assert_eq!(rec.fills[0], scheme.secondary_container);
    }

    // ---- Semantics --------------------------------------------------------------

    #[test]
    fn semantics_yields_a_radiogroup_for_single_select() {
        fn logic(_s: &mut ()) -> SegmentedButtonView<(), u32> {
            segmented_button::<(), u32, _>(
                [segment(0u32).label("A"), segment(1u32).label("B")],
                [1u32],
                |_s: &mut (), _v| {},
            )
        }
        let mut root: frust_core::RenderRoot<(), SegmentedButtonView<(), u32>> =
            frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(300.0, 60.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();

        let group = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::RadioGroup)
            .expect("a RadioGroup container node is contributed");
        assert_eq!(group.1.children().len(), 2);

        let selected = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::RadioButton && n.label() == Some("B"))
            .expect("segment B is present");
        assert_eq!(selected.1.is_selected(), Some(true));
    }

    #[test]
    fn semantics_yields_a_group_of_checkboxes_for_multi_select() {
        fn logic(_s: &mut ()) -> SegmentedButtonView<(), u32> {
            segmented_button::<(), u32, _>(
                [segment(0u32).label("A"), segment(1u32).label("B")],
                [0u32, 1u32],
                |_s: &mut (), _v| {},
            )
            .multi_select(true)
        }
        let mut root: frust_core::RenderRoot<(), SegmentedButtonView<(), u32>> =
            frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(300.0, 60.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();

        let group = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Group)
            .expect("a Group container node is contributed");
        assert_eq!(group.1.children().len(), 2);

        let boxes: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::CheckBox)
            .collect();
        assert_eq!(boxes.len(), 2);
        assert!(
            boxes
                .iter()
                .all(|(_, n)| n.toggled() == Some(Toggled::True))
        );
    }
}
