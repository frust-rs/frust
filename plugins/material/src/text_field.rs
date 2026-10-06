//! The M3 Expressive **text field**: filled and outlined variants, a floating
//! label, leading/trailing icon slots, supporting text and an error state,
//! painted as decoration *around* the framework's own [`frust::text_input`].
//!
//! Ported from `material_3_expressive` v1.0.8 (MIT, © 2026 Paa Developments)'s
//! `lib/components/text_fields/` — `m3e_text_fields.dart` (`M3ETextField`),
//! `styles/m3e_text_field_theme.dart` (`M3ETextFieldTheme`'s default metrics)
//! and `enums/m3e_text_field_variant.dart`. Upstream:
//! <https://github.com/paadevelopments/material_3_expressive>. Porting
//! decisions that depart from the Dart source are called out inline below
//! (disabled treatment, the container fill, the label crossfade).
//!
//! # Wrapping the baseline, not forking it
//!
//! The editable is the framework's own field: this widget holds a
//! [`ChildPod`] carrying a [`frust::TextInputView`] and paints Material's
//! decoration around it. Every editing concern — the text editor, focus/IME
//! publication, the caret blink, selection, the controlled-value reconcile —
//! stays the baseline's, reached through its public builder seams only:
//!
//! - `border_width(0.0)` + `focus_ring_width(0.0)` — the baseline draws its
//!   frame as a border-colored rounded rect with the background inset by the
//!   border width, so a zero width on both leaves only the background fill.
//!   That fill is opaque, which is why every piece of Material chrome here is
//!   painted **after** the child rather than under it.
//! - `corner_radius(RADIUS)` — the extra-small (4dp) M3 corner, baked as a
//!   constant because the seam is a view-time builder value and no `Theme` is
//!   threaded into `View::build`.
//! - `padding(x, y)` — `x` clears the leading icon slot, `y` reserves the
//!   floated label's band (`TEXT_TOP_WITH_LABEL`) so the label never
//!   overlaps the edited text. A field with no label passes `0.0`, which
//!   leaves the baseline's own vertical centering in charge.
//!
//! # Focus observation: the paint-pass focus path
//!
//! The wrapper never asks the editable whether it is focused, and holds no
//! focus bookkeeping of its own. It reads [`PaintCtx::has_focus`] — the pod's
//! recorded focus **path**, seeded by `ChildPod::paint_child` from the render
//! root's own focus mirror. Its contract, and the limits that follow:
//!
//! - **It reports path membership, not identity**: `true` means "this widget
//!   or a descendant of it holds focus". That is exactly what this widget
//!   wants (its one focusable descendant is the wrapped editable), and it is
//!   why no focus callback, `ImeState` inspection or `on_change` side channel
//!   is needed. A caller who puts a *focusable* view in the
//!   [`leading`](TextFieldView::leading)/[`trailing`](TextFieldView::trailing)
//!   slot therefore makes the field read as focused while that slot holds
//!   focus — the accent and the floated label follow the slot. Icon slots are
//!   the intended content and are not focusable, so this is a documented
//!   consequence, not a supported mode.
//! - **It is a paint-pass read, so chrome converges on the first paint after
//!   a focus change, never before it.** A focus change already dirties the
//!   frame, so the lag is at most that one frame; there is no staleness
//!   beyond it, and — unlike a widget-local `focused` flag — a
//!   container-routed blur (a tap on a sibling, which never calls this
//!   widget's `event`) is observed correctly, because the path is
//!   authoritative and this widget keeps no flag that could disagree with it.
//! - **A disabled field never reads as focused**, because the baseline
//!   refuses focus while disabled; the `&& self.enabled` guard here only
//!   makes that explicit for the frame on which a live field is disabled.
//!
//! # Container fill: a documented fidelity gap
//!
//! M3's filled variant fills its container with `surfaceContainerHighest`.
//! This port **does not paint that wash**, and the container fill an app sees
//! is the baseline's own background — the theme's `surface` role. The
//! baseline resolves that background itself and exposes no seam to override
//! or suppress it, so any wash painted here would be covered across the
//! field's interior; painting it only in the gutters the child's rounded rect
//! misses would leave a visible two-tone seam instead. The two variants
//! therefore differ by their stroke — a bottom indicator versus a full
//! outline — and not by their fill. This is the same class of gap
//! `frust_shadcn::input` records for its own `dark:bg-input/30` wash.
//!
//! # Floating label
//!
//! The label floats when the field is focused **or** holds content
//! (`floated = focused || !value.is_empty()`), animated over
//! [`MaterialMotion::SHORT_3`] with [`MaterialMotion::STANDARD`] easing —
//! the Dart source's `M3EMotion.short3`/`standard` pair.
//!
//! Both label states are shaped as their own retained runs — body-large at
//! rest, body-small floated — and the transition crossfades between them
//! while their shared vertical center travels from the field's middle to the
//! floated band. The Dart source instead interpolates one `TextStyle`
//! (`AnimatedDefaultTextStyle`); a crossfade of two pre-shaped runs is used
//! here because a per-frame interpolated font size would re-shape the label
//! on every frame of the transition, and text shaping happens in the layout
//! pass, not the paint pass this animation advances in.
//!
//! A field *mounted* with content starts floated rather than ramping up on
//! first appearance, through a short priming window
//! (`TextFieldWidget::float_priming`) — the controller cannot be seeded at
//! an endpoint, so it is driven to one unseen while the target is painted
//! directly. `reduce_motion` collapses the transition entirely: the fraction
//! is read straight off the target and no continuation frame is requested.
//!
//! # Known limitations
//!
//! - **Per-field text alignment.** The wrapped editable is always
//!   start-aligned regardless of `TextStyle::align`
//!   (`docs/WIDGETS_ARCHITECTURE.md`'s *Text Widget Alignment*: parley's
//!   `PlainEditor` exposes no alignment hook). This widget cannot work around
//!   it, and offers no alignment knob that would imply otherwise.
//! - **A long value runs under the trailing slot.** The wrapped field spans
//!   the whole decoration box so its opaque background is the container fill;
//!   the baseline offers a left inset (`padding`) but no right one, so an
//!   overflowing single-line value passes beneath a trailing icon.
//! - **The label is not notched into the outline.** M3 proper cuts the
//!   outlined variant's stroke where the floated label crosses it; the Dart
//!   source floats the label *inside* the container for both variants and
//!   strokes an unbroken border, which is what is ported here.
//! - **The editable's glyphs use the baseline's default family**, not the
//!   theme's Roboto Flex: `TextInputView::text_style` is all-or-nothing and
//!   setting it would also freeze the glyph color, losing the baseline's
//!   themed `on_surface` resolution (the same trade `frust_shadcn::input`
//!   documents). The catalog-owned label and supporting runs *do* shape in
//!   the theme's type scale.
//! - **Single-line only.** The decoration box is a fixed
//!   `DECORATION_HEIGHT`, so the Dart source's `maxLines` (and with it the
//!   baseline's `multiline` mode) is not exposed; neither are its
//!   `keyboardType`/`textInputAction`/`inputFormatters`/`onTapOutside` knobs.
//! - **A `Down` on an interactive slot blurs the field.** That is the shared
//!   container contract (`frust::authoring::route_event`'s blur-on-outside-tap:
//!   a `Down` that does not re-establish focus on the child it hits clears
//!   every focused sibling), not something this widget adds or can suppress —
//!   another reason the slots are meant for plain icons.

use std::rc::Rc;

use frust::authoring::Role;
use frust::authoring::text::{TextContext, TextLayout, TextStyle};
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, CursorIcon, EventCtx, EventResult,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerPhase, SemanticsCtx, View, Widget, any,
};
use frust::{AnimationController, Theme, text_input};
use kurbo::{Point, RoundedRect, Shape, Size};
use peniko::{Brush, Color};

use super::interaction::{DISABLED_CONTAINER_OPACITY, DISABLED_CONTENT_OPACITY};
use crate::tokens::{MaterialDimensions, MaterialMotion};

/// Decoration-box height, in logical px (`M3ETextFieldTheme.minHeight`). The
/// box is a fixed height here rather than a minimum: the wrapped field is
/// single-line, so nothing can grow it.
const DECORATION_HEIGHT: f64 = 56.0;
/// Horizontal inset from the decoration box's edges to its content
/// (`M3ETextFieldTheme.horizontalPadding`), in logical px.
const HPAD: f64 = 16.0;
/// Edge length of a leading/trailing icon slot, in logical px
/// (`M3ETextFieldTheme.iconSize`). The slot is reserved at this size whatever
/// the mounted view measures, so the editable's left inset is known at
/// `View::build` time (where the baseline's `padding` seam is set).
const ICON_SIZE: f64 = 24.0;
/// Gap between an icon slot and the editable (`M3ETextFieldTheme.iconGap`), in
/// logical px.
const ICON_GAP: f64 = 12.0;
/// Top inset of the floated label's band, in logical px
/// (`M3ETextFieldTheme.labelFloatingTopPadding`).
const LABEL_FLOAT_TOP: f64 = 8.0;
/// Top inset of the editable's glyphs while a label is present, in logical px:
/// [`LABEL_FLOAT_TOP`] plus the 16dp body-small line box the floated label
/// occupies (m3.material.io — *Components → Text fields → Specs*, filled field
/// with a populated label; retrieved 2026-08-19). Fed to the baseline's
/// `padding` seam, which floors the field's own vertical centering at it.
const TEXT_TOP_WITH_LABEL: f64 = 24.0;
/// Gap between the decoration box and the supporting text
/// (`M3ETextFieldTheme.supportingTextPadding.top`), in logical px.
const SUPPORTING_GAP: f64 = 4.0;
/// Corner radius of both variants, in logical px — M3's extra-small shape
/// token (`M3EShapes.radiusExtraSmall`).
const RADIUS: f64 = MaterialDimensions::RADIUS_EXTRA_SMALL;
/// Indicator/outline thickness at rest, in logical px.
const STROKE_REST: f64 = 1.0;
/// Indicator/outline thickness while focused, in logical px.
const STROKE_FOCUSED: f64 = 2.0;
/// Flattening tolerance for the outlined variant's stroked rounded rect —
/// matching `super::card`'s own outline tolerance.
const STROKE_TOLERANCE: f64 = 0.1;
/// Field width used when the incoming constraints are horizontally unbounded —
/// the baseline field's own fallback, restated here because this widget
/// resolves its width before handing the child tight constraints.
const UNBOUNDED_WIDTH: f64 = 200.0;

/// Unthemed fallback for the `primary` role (M3 baseline light `#6750A4`).
const PRIMARY: Color = Color::from_rgb8(0x67, 0x50, 0xA4);
/// Unthemed fallback for the `error` role (M3 baseline light `#B3261E`).
const ERROR: Color = Color::from_rgb8(0xB3, 0x26, 0x1E);
/// Unthemed fallback for the `on_surface` role (M3 baseline light `#1D1B20`).
const ON_SURFACE: Color = Color::from_rgb8(0x1D, 0x1B, 0x20);
/// Unthemed fallback for the `on_surface_variant` role (M3 baseline light
/// `#49454F`).
const ON_SURFACE_VARIANT: Color = Color::from_rgb8(0x49, 0x45, 0x4F);
/// Unthemed fallback for the `outline` role (M3 baseline light `#79747E`).
const OUTLINE: Color = Color::from_rgb8(0x79, 0x74, 0x7E);

/// Body-large size/line-height, in logical px — the resting label's type role
/// when no theme is threaded (M3 `bodyLarge`, 16/24).
const BODY_LARGE_SIZE: f32 = 16.0;
/// Body-small size, in logical px — the floated label's and the supporting
/// text's type role when no theme is threaded (M3 `bodySmall`, 12/16).
const BODY_SMALL_SIZE: f32 = 12.0;

/// The ink every catalog-owned run is *shaped* with. Never painted: each run
/// is re-brushed with its resolved state color at paint time, and keeping the
/// shaping color constant keeps the shape cache from missing on a recolor.
const SHAPING_INK: Color = Color::BLACK;

/// Which container treatment a field paints
/// (`M3ETextFieldVariant`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextFieldVariant {
    /// A bottom indicator only, thickening from `STROKE_REST` to
    /// `STROKE_FOCUSED` on focus. The M3 default.
    #[default]
    Filled,
    /// A full outline at `RADIUS` (4dp), thickening on focus like the
    /// indicator.
    Outlined,
}

/// A view-held, typed text callback (erased by the wrapped baseline field on
/// build).
type OnText<State> = Rc<dyn Fn(&mut State, String)>;

/// A declarative M3 text field. See the [module docs](self).
pub struct TextFieldView<State: 'static> {
    value: String,
    label: Option<String>,
    supporting_text: Option<String>,
    error_text: Option<String>,
    leading: Option<AnyView<State>>,
    trailing: Option<AnyView<State>>,
    variant: TextFieldVariant,
    obscured: bool,
    enabled: bool,
    on_change: OnText<State>,
    on_submit: Option<OnText<State>>,
}

/// Create a controlled M3 text field showing `value`, reporting each edit
/// through `on_change(state, new_text)`.
///
/// Controlled exactly like the baseline field it wraps: the widget never owns
/// the durable value, and an app that rejects or transforms the requested text
/// in `on_change` sees its own value win on the next rebuild.
pub fn text_field<State: 'static, F: Fn(&mut State, String) + 'static>(
    value: impl Into<String>,
    on_change: F,
) -> TextFieldView<State> {
    TextFieldView {
        value: value.into(),
        label: None,
        supporting_text: None,
        error_text: None,
        leading: None,
        trailing: None,
        variant: TextFieldVariant::Filled,
        obscured: false,
        enabled: true,
        on_change: Rc::new(on_change),
        on_submit: None,
    }
}

/// PascalCase alias for [`text_field`].
#[allow(non_snake_case)]
pub fn TextField<State: 'static, F: Fn(&mut State, String) + 'static>(
    value: impl Into<String>,
    on_change: F,
) -> TextFieldView<State> {
    text_field(value, on_change)
}

impl<State: 'static> TextFieldView<State> {
    /// Set the floating label (see the [module docs](self)' *Floating label*).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Set the helper line below the field. An
    /// [`error_text`](Self::error_text) always replaces it.
    pub fn supporting_text(mut self, text: impl Into<String>) -> Self {
        self.supporting_text = Some(text.into());
        self
    }

    /// Put the field in its error state: the indicator/outline, the floated
    /// label and the supporting line all recolor to `error`, and this text
    /// replaces [`supporting_text`](Self::supporting_text).
    pub fn error_text(mut self, text: impl Into<String>) -> Self {
        self.error_text = Some(text.into());
        self
    }

    /// Mount a leading view in a `ICON_SIZE` slot at the field's start edge.
    /// The slot's content owns its own ink and size; the field only reserves
    /// and positions the slot.
    pub fn leading(mut self, view: impl View<State>) -> Self {
        self.leading = Some(AnyView::new(view));
        self
    }

    /// Mount a trailing view in a `ICON_SIZE` slot at the field's end edge —
    /// see [`leading`](Self::leading).
    pub fn trailing(mut self, view: impl View<State>) -> Self {
        self.trailing = Some(AnyView::new(view));
        self
    }

    /// Choose the container treatment (default [`TextFieldVariant::Filled`]).
    pub fn variant(mut self, variant: TextFieldVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Mask the rendered glyphs (the baseline field's `obscured` mode, which
    /// also publishes the secret IME hint).
    pub fn obscured(mut self, obscured: bool) -> Self {
        self.obscured = obscured;
        self
    }

    /// Set whether the field accepts input (default `true`). A disabled field
    /// refuses focus and dims both the baseline's content and this widget's
    /// chrome.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Set the Enter handler (the baseline field's `on_submit`).
    pub fn on_submit<F: Fn(&mut State, String) + 'static>(mut self, on_submit: F) -> Self {
        self.on_submit = Some(Rc::new(on_submit));
        self
    }

    /// Whether an error text was set — the reference's `hasError`.
    fn has_error(&self) -> bool {
        self.error_text.is_some()
    }

    /// The supporting line actually shown: the error text outranks the helper.
    fn supporting_line(&self) -> Option<&str> {
        self.error_text
            .as_deref()
            .or(self.supporting_text.as_deref())
    }

    /// Left inset of the editable's glyphs (and of the label), in logical px.
    fn text_left(&self) -> f64 {
        if self.leading.is_some() {
            HPAD + ICON_SIZE + ICON_GAP
        } else {
            HPAD
        }
    }

    /// Top inset of the editable's glyphs, in logical px. `0.0` with no label,
    /// which leaves the baseline's own vertical centering in charge.
    fn text_top(&self) -> f64 {
        if self.label.is_some() {
            TEXT_TOP_WITH_LABEL
        } else {
            0.0
        }
    }

    /// The presence-of-slots shape; a change forces a full child rebuild.
    fn shape(&self) -> (bool, bool) {
        (self.leading.is_some(), self.trailing.is_some())
    }

    /// The wrapped baseline field, with its own chrome suppressed (see the
    /// [module docs](self)).
    fn field_view(&self) -> AnyView<State> {
        let on_change = self.on_change.clone();
        let mut field = text_input(self.value.clone(), move |state: &mut State, text| {
            on_change(state, text)
        })
        .enabled(self.enabled)
        .obscured(self.obscured)
        .padding(self.text_left(), self.text_top())
        .border_width(0.0)
        .focus_ring_width(0.0)
        .corner_radius(RADIUS);
        if let Some(on_submit) = self.on_submit.clone() {
            field = field.on_submit(move |state: &mut State, text| on_submit(state, text));
        }
        any(field)
    }
}

/// A cached, lazily-shaped text run whose ink is applied at paint time.
///
/// The catalog owns its label and supporting runs rather than nesting
/// `frust::text` children because both need a paint-time ink (the float
/// crossfade's alpha, the focus/error accent) that a baked-at-layout color
/// could only follow a relayout behind.
struct Run {
    content: String,
    layout: Option<TextLayout>,
    shaped_style: Option<TextStyle>,
}

impl Run {
    /// A run holding `content`, unshaped until the first [`layout`](Self::layout).
    fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            layout: None,
            shaped_style: None,
        }
    }

    /// Replace the text, dropping the cached layout only if it actually
    /// changed.
    fn set_content(&mut self, content: &str) {
        if self.content != content {
            self.content = content.to_string();
            self.layout = None;
        }
    }

    /// The text this run holds — the accessible name/description a
    /// `semantics` pass reports, without a second copy of the string.
    fn content(&self) -> &str {
        &self.content
    }

    /// The measured size of the last shaped run (`ZERO` before the first
    /// layout).
    fn size(&self) -> Size {
        self.layout.as_ref().map_or(Size::ZERO, |l| l.size())
    }

    /// Shape (or reuse) the run in `style` and return its measured size.
    fn layout(&mut self, ctx: &mut LayoutCtx, style: &TextStyle) -> Size {
        if let Some(cached) = &self.layout
            && self.shaped_style.as_ref() == Some(style)
        {
            return cached.size();
        }
        let text_ctx = ctx.text_context::<TextContext>();
        let laid = text_ctx.layout(&self.content, style, None);
        let size = laid.size();
        self.layout = Some(laid);
        self.shaped_style = Some(style.clone());
        size
    }

    /// Paint the run at `origin` in `color`, overriding the [`SHAPING_INK`] it
    /// was shaped with. A run that has never been laid out paints nothing.
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

/// The body-large type role the resting label shapes in. Themed:
/// `Theme::type_scale.body_large`. Unthemed: [`BODY_LARGE_SIZE`] at the
/// default family.
fn body_large(theme: Option<&Theme>) -> TextStyle {
    let mut style = match theme {
        Some(theme) => theme.type_scale.body_large.clone(),
        None => TextStyle::new(BODY_LARGE_SIZE, SHAPING_INK),
    };
    style.color = SHAPING_INK;
    style
}

/// The body-small type role the floated label and the supporting line shape
/// in — see [`body_large`].
fn body_small(theme: Option<&Theme>) -> TextStyle {
    let mut style = match theme {
        Some(theme) => theme.type_scale.body_small.clone(),
        None => TextStyle::new(BODY_SMALL_SIZE, SHAPING_INK),
    };
    style.color = SHAPING_INK;
    style
}

/// The resolved decoration inks for one paint pass.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Colors {
    /// The bottom indicator (filled) or the outline (outlined).
    stroke: Color,
    /// The label in its floated state.
    label_float: Color,
    /// The label in its resting state.
    label_rest: Color,
    /// The supporting line below the field.
    supporting: Color,
}

/// Resolve every decoration ink for one pass.
///
/// Enabled follows the Dart source: the accent is `error` in the error state
/// and `primary` otherwise; the stroke takes the accent while focused or in
/// error and the variant's idle role (`outline` / `onSurfaceVariant`)
/// otherwise; a floated label takes the accent and a resting one
/// `onSurfaceVariant`.
///
/// Disabled deliberately **departs** from the source, which dims only the
/// accent (and so leaves a disabled field's idle outline at full strength).
/// M3's disabled treatment is applied instead: container-class ink
/// ([`DISABLED_CONTAINER_OPACITY`], 12%) for the stroke and content-class ink
/// ([`DISABLED_CONTENT_OPACITY`], 38%) for the label and supporting line, both
/// over `onSurface` — the same rule the wrapped baseline field dims its own
/// glyphs by.
fn resolve_colors(
    theme: Option<&Theme>,
    variant: TextFieldVariant,
    enabled: bool,
    has_error: bool,
    focused: bool,
) -> Colors {
    let (primary, error, on_surface, on_surface_variant, outline) = match theme {
        Some(theme) => {
            let s = theme.scheme();
            (
                s.primary,
                s.error,
                s.on_surface,
                s.on_surface_variant,
                s.outline,
            )
        }
        None => (PRIMARY, ERROR, ON_SURFACE, ON_SURFACE_VARIANT, OUTLINE),
    };
    if !enabled {
        let container = on_surface.multiply_alpha(DISABLED_CONTAINER_OPACITY);
        let content = on_surface.multiply_alpha(DISABLED_CONTENT_OPACITY);
        return Colors {
            stroke: container,
            label_float: content,
            label_rest: content,
            supporting: content,
        };
    }
    let accent = if has_error { error } else { primary };
    let idle = match variant {
        TextFieldVariant::Filled => on_surface_variant,
        TextFieldVariant::Outlined => outline,
    };
    Colors {
        stroke: if focused || has_error { accent } else { idle },
        label_float: accent,
        label_rest: on_surface_variant,
        supporting: if has_error { error } else { on_surface_variant },
    }
}

/// Which entry of [`TextFieldWidget::children`] each slot occupies. The
/// wrapped field is always present; the icon slots are optional. Storing every
/// pod in one `Vec` lets the widget route events through
/// [`frust::authoring::route_event`] (topmost-first, so an interactive icon
/// wins over the field beneath it) and recurse uniformly for semantics.
#[derive(Clone, Copy)]
struct Slots {
    field: usize,
    leading: Option<usize>,
    trailing: Option<usize>,
}

/// The retained widget for a [`TextFieldView`].
pub struct TextFieldWidget {
    children: Vec<ChildPod>,
    slots: Slots,
    variant: TextFieldVariant,
    enabled: bool,
    has_error: bool,
    /// Whether the app-confirmed value is empty — half of the float condition
    /// (see the [module docs](self)).
    value_empty: bool,
    /// Left inset of the label runs, matching the editable's own.
    text_left: f64,
    /// The label at rest (body-large) and floated (body-small); both `None`
    /// when the field carries no label.
    label_rest: Option<Run>,
    label_float: Option<Run>,
    /// The supporting line actually shown (error text outranking helper).
    supporting: Option<Run>,
    /// Height the supporting line adds below the decoration box, gap included.
    supporting_height: f64,
    /// Drives the label's `0.0` (resting) .. `1.0` (floated) fraction.
    float: AnimationController,
    /// The float state the controller is currently driving toward.
    float_target: bool,
    /// The mount-priming window. [`AnimationController`] always starts at
    /// `0.0` and its duration cannot be changed after construction, so a field
    /// *built* with content would otherwise fade its label up over
    /// [`MaterialMotion::SHORT_3`] on first appearance. While this is set the
    /// painted fraction is read straight off the target and the controller is
    /// left to catch up unseen; it clears the first time the controller
    /// settles, from which point every transition is the controller's.
    float_priming: bool,
}

/// Build the ordered pod window `[field, leading?, trailing?]` and its
/// [`Slots`] index map. The field goes first because it is painted first
/// (bottom of the z-order), which is the order `route_event` hit-tests in
/// reverse.
fn build_children<State: 'static>(
    view: &TextFieldView<State>,
    ctx: &mut BuildCtx<'_>,
) -> (Vec<ChildPod>, Slots) {
    let mut children = vec![frust::authoring::build_child(&view.field_view(), ctx)];
    let field = children.len() - 1;
    let leading = view.leading.as_ref().map(|v| {
        children.push(frust::authoring::build_child(v, ctx));
        children.len() - 1
    });
    let trailing = view.trailing.as_ref().map(|v| {
        children.push(frust::authoring::build_child(v, ctx));
        children.len() - 1
    });
    (
        children,
        Slots {
            field,
            leading,
            trailing,
        },
    )
}

/// Tear down every pod through the view it was built from, per the [`Slots`]
/// index map.
fn teardown_children<State: 'static>(
    view: &TextFieldView<State>,
    slots: Slots,
    children: &mut [ChildPod],
    ctx: &mut BuildCtx<'_>,
) {
    for (index, pod) in children.iter_mut().enumerate() {
        if index == slots.field {
            frust::authoring::teardown_child(&view.field_view(), pod, ctx);
        } else if Some(index) == slots.leading
            && let Some(v) = view.leading.as_ref()
        {
            frust::authoring::teardown_child(v, pod, ctx);
        } else if Some(index) == slots.trailing
            && let Some(v) = view.trailing.as_ref()
        {
            frust::authoring::teardown_child(v, pod, ctx);
        }
    }
}

/// Reconcile an optional catalog-owned run against the text it should hold,
/// reporting whether the widget needs a fresh layout.
fn sync_run(slot: &mut Option<Run>, content: Option<&str>) -> bool {
    match (slot.as_mut(), content) {
        (Some(run), Some(text)) => {
            if run.content() == text {
                return false;
            }
            run.set_content(text);
            true
        }
        (None, Some(text)) => {
            *slot = Some(Run::new(text));
            true
        }
        (Some(_), None) => {
            *slot = None;
            true
        }
        (None, None) => false,
    }
}

fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

impl<State: 'static> View<State> for TextFieldView<State> {
    type Element = TextFieldWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> TextFieldWidget {
        let (children, slots) = build_children(self, ctx);
        let float_target = !self.value.is_empty();
        // The ramp started here is deliberately never *displayed* — it only
        // drives the controller to the endpoint a field mounted with content
        // already belongs at, which `float_priming` shows in the meantime.
        let mut float =
            AnimationController::new(MaterialMotion::SHORT_3).with_curve(MaterialMotion::STANDARD);
        if float_target {
            float.forward();
        }
        TextFieldWidget {
            children,
            slots,
            variant: self.variant,
            enabled: self.enabled,
            has_error: self.has_error(),
            value_empty: self.value.is_empty(),
            text_left: self.text_left(),
            label_rest: self.label.as_deref().map(Run::new),
            label_float: self.label.as_deref().map(Run::new),
            supporting: self.supporting_line().map(Run::new),
            supporting_height: 0.0,
            float,
            float_target,
            float_priming: true,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut TextFieldWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;

        if self.shape() != prev.shape() {
            // Slot presence changed: tear the whole pod set down (against the
            // previous view, whose shape the live pods still match) and rebuild.
            teardown_children(prev, element.slots, &mut element.children, ctx);
            let (children, slots) = build_children(self, ctx);
            element.children = children;
            element.slots = slots;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else {
            flags |= frust::authoring::rebuild_child(
                &prev.field_view(),
                &self.field_view(),
                &mut element.children[element.slots.field],
                ctx,
            );
            if let (Some(pv), Some(nv), Some(idx)) = (
                prev.leading.as_ref(),
                self.leading.as_ref(),
                element.slots.leading,
            ) {
                flags |= frust::authoring::rebuild_child(pv, nv, &mut element.children[idx], ctx);
            }
            if let (Some(pv), Some(nv), Some(idx)) = (
                prev.trailing.as_ref(),
                self.trailing.as_ref(),
                element.slots.trailing,
            ) {
                flags |= frust::authoring::rebuild_child(pv, nv, &mut element.children[idx], ctx);
            }
        }

        // Non-short-circuiting `|` on purpose: every run must reconcile, and
        // any one of them changing needs the same relayout.
        if sync_run(&mut element.label_rest, self.label.as_deref())
            | sync_run(&mut element.label_float, self.label.as_deref())
            | sync_run(&mut element.supporting, self.supporting_line())
        {
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.text_left != self.text_left() {
            element.text_left = self.text_left();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.variant != self.variant {
            element.variant = self.variant;
            flags |= ChangeFlags::PAINT;
        }
        if element.enabled != self.enabled {
            element.enabled = self.enabled;
            flags |= ChangeFlags::PAINT;
        }
        if element.has_error != self.has_error() {
            element.has_error = self.has_error();
            flags |= ChangeFlags::PAINT;
        }
        if element.value_empty != self.value.is_empty() {
            element.value_empty = self.value.is_empty();
            // The float retargets in `paint`, the one pass that can also read
            // the focus path the other half of the condition comes from.
            flags |= ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut TextFieldWidget, ctx: &mut BuildCtx<'_>) {
        teardown_children(self, element.slots, &mut element.children, ctx);
    }
}

impl Widget for TextFieldWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            UNBOUNDED_WIDTH
        };

        // Resolve the type roles into owned styles first: the theme borrow has
        // to end before anything takes `ctx` mutably (shaping, child layout).
        let (rest_style, small_style) = {
            let theme = Theme::from_layout_ctx(ctx);
            (body_large(theme), body_small(theme))
        };
        if let Some(run) = &mut self.label_rest {
            run.layout(ctx, &rest_style);
        }
        if let Some(run) = &mut self.label_float {
            run.layout(ctx, &small_style);
        }
        self.supporting_height = match &mut self.supporting {
            Some(run) => SUPPORTING_GAP + run.layout(ctx, &small_style).height,
            None => 0.0,
        };

        // Tight constraints are what make the wrapped field the container: its
        // opaque background covers the whole decoration box, so no seam can
        // show between it and anything this widget paints.
        let field = self.slots.field;
        self.children[field].layout_child(
            ctx,
            &BoxConstraints::tight(Size::new(width, DECORATION_HEIGHT)),
        );
        self.children[field].set_origin(Point::ORIGIN);

        let slot_bc = BoxConstraints::new(Size::ZERO, Size::new(ICON_SIZE, ICON_SIZE));
        if let Some(idx) = self.slots.leading {
            let size = self.children[idx].layout_child(ctx, &slot_bc);
            self.children[idx].set_origin(icon_origin(HPAD, size));
        }
        if let Some(idx) = self.slots.trailing {
            let size = self.children[idx].layout_child(ctx, &slot_bc);
            self.children[idx].set_origin(icon_origin(width - HPAD - ICON_SIZE, size));
        }

        bc.constrain(Size::new(width, DECORATION_HEIGHT + self.supporting_height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // The pod's focus path is authoritative (a container-routed blur never
        // reaches this widget's `event`) — see the module docs.
        let focused = ctx.has_focus() && self.enabled;
        let frame_time = ctx.frame_time();
        let origin = ctx.origin();
        let width = ctx.size().width;
        // Theme reads happen up front and only owned values cross the child's
        // paint, which borrows the `PaintCtx` mutably.
        let (colors, reduce_motion) = {
            let theme = Theme::from_paint_ctx(ctx);
            (
                resolve_colors(theme, self.variant, self.enabled, self.has_error, focused),
                theme.is_some_and(|t| t.motion.reduce_motion),
            )
        };

        let floated = focused || !self.value_empty;
        if self.float_target != floated {
            self.float_target = floated;
            if floated {
                self.float.forward();
            } else {
                self.float.reverse();
            }
        }
        let float_t = if reduce_motion {
            // Collapsed: the fraction is read straight off the target and no
            // continuation frame is asked for. The controller is left alone, so
            // it resumes (and converges) if `reduce_motion` is turned back off.
            if floated { 1.0 } else { 0.0 }
        } else {
            if self.float.advance(frame_time) {
                ctx.request_frame();
            }
            if self.float_priming {
                // Still catching up to the mount-time endpoint: show the
                // settled value, and hand the controller over once it agrees.
                if !self.float.is_animating() {
                    self.float_priming = false;
                }
                if floated { 1.0 } else { 0.0 }
            } else {
                self.float.value_clamped()
            }
        };

        // The wrapped field first: its opaque background is this field's
        // container fill, so every piece of chrome below is painted over it.
        self.children[self.slots.field].paint_child(ctx, scene);
        if let Some(idx) = self.slots.leading {
            self.children[idx].paint_child(ctx, scene);
        }
        if let Some(idx) = self.slots.trailing {
            self.children[idx].paint_child(ctx, scene);
        }

        let stroke_w = if focused { STROKE_FOCUSED } else { STROKE_REST };
        match self.variant {
            TextFieldVariant::Filled => scene.fill_rect(
                Point::new(origin.x, origin.y + DECORATION_HEIGHT - stroke_w),
                Size::new(width, stroke_w),
                colors.stroke,
            ),
            TextFieldVariant::Outlined => {
                // Inset by half the stroke so the line paints fully inside the
                // decoration box (a stroke is centered on its path) — the same
                // treatment `super::card`'s outlined variant uses.
                let half = stroke_w / 2.0;
                let rr = RoundedRect::new(
                    half,
                    half,
                    width - half,
                    DECORATION_HEIGHT - half,
                    (RADIUS - half).max(0.0),
                );
                scene.stroke_path(
                    origin,
                    &rr.to_path(STROKE_TOLERANCE),
                    stroke_w,
                    &Brush::Solid(colors.stroke),
                );
            }
        }

        if let (Some(rest), Some(float)) = (&self.label_rest, &self.label_float) {
            let x = origin.x + self.text_left;
            let (rest_h, float_h) = (rest.size().height, float.size().height);
            let rest_center = origin.y + DECORATION_HEIGHT / 2.0;
            let float_center = origin.y + LABEL_FLOAT_TOP + float_h / 2.0;
            let center = rest_center + (float_center - rest_center) * float_t;
            if float_t < 1.0 {
                rest.paint(
                    Point::new(x, center - rest_h / 2.0),
                    colors.label_rest.multiply_alpha(1.0 - float_t as f32),
                    scene,
                );
            }
            if float_t > 0.0 {
                float.paint(
                    Point::new(x, center - float_h / 2.0),
                    colors.label_float.multiply_alpha(float_t as f32),
                    scene,
                );
            }
        }

        if let Some(run) = &self.supporting {
            run.paint(
                Point::new(
                    origin.x + HPAD,
                    origin.y + DECORATION_HEIGHT + SUPPORTING_GAP,
                ),
                colors.supporting,
                scene,
            );
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let result = frust::authoring::route_event(&mut self.children, ctx, event);
        // The cursor is asked for from the uncaptured `Move` arm only, and
        // after routing so this widget's shape wins over a slot's: the I-beam
        // over an editable decoration box, `NotAllowed` when disabled.
        if let InputEvent::Pointer(p) = event
            && matches!(p.phase, PointerPhase::Move)
            && inside(p.position, Size::new(ctx.size().width, DECORATION_HEIGHT))
        {
            ctx.set_cursor(if self.enabled {
                CursorIcon::Text
            } else {
                CursorIcon::NotAllowed
            });
        }
        result
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // A labelled group around the wrapped editable's own node: the label
        // and the supporting/error line are this widget's text, not the
        // baseline field's, so nothing else would report them.
        ctx.push_container(
            Role::Group,
            |node| {
                if let Some(label) = &self.label_rest {
                    node.set_label(label.content());
                }
                if let Some(supporting) = &self.supporting {
                    node.set_description(supporting.content());
                }
            },
            |ctx| {
                for pod in &self.children {
                    pod.semantics_child(ctx);
                }
            },
        );
    }

    frust::authoring::visit_children!(children);
}

/// The origin of an icon slot whose reserved `ICON_SIZE` box starts at `x`:
/// the mounted view is centered in the slot, and in the decoration box
/// vertically.
fn icon_origin(x: f64, measured: Size) -> Point {
    Point::new(
        x + ((ICON_SIZE - measured.width) / 2.0).max(0.0),
        ((DECORATION_HEIGHT - measured.height) / 2.0).max(0.0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::{BezPath, Key, KeyEvent, Modifiers, PointerButton, PointerEvent, Rect};
    use frust::{Brightness, FrameTime};
    use frust_core::RenderRoot;
    use std::any::Any;

    /// The window every harness lays out in.
    const WINDOW: Size = Size::new(320.0, 160.0);
    /// A point inside the decoration box, clear of both icon slots.
    const IN_FIELD: Point = Point::new(120.0, 28.0);

    /// A recording `PaintScene` capturing everything these tests assert on: the
    /// rounded-rect fills (the wrapped field's background), the plain rects
    /// (the filled variant's indicator), the stroked paths (the outlined
    /// variant's outline) and the glyph runs (label + supporting text).
    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        rects: Vec<(Point, Size, Color)>,
        strokes: Vec<(Rect, f64, Color)>,
        runs: Vec<(f32, Point, Color)>,
        /// Index into `runs` at which this widget's *own* text begins.
        ///
        /// The wrapped field's glyphs share the label's body-large size, so a
        /// size filter alone cannot tell them apart. Paint order can: the
        /// indicator/outline is the last rect-or-stroke of the pass and is
        /// painted between the wrapped field's own paint and the label, so
        /// every run recorded after the final rect/stroke is chrome.
        chrome_start: usize,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, origin: Point, size: Size, color: Color) {
            self.rects.push((origin, size, color));
            self.chrome_start = self.runs.len();
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn fill_rounded_rect(&mut self, origin: Point, size: Size, radius: f64, color: Color) {
            self.rrects.push((origin, size, radius, color));
        }
        fn stroke_path(&mut self, origin: Point, path: &BezPath, width: f64, brush: &Brush) {
            let bbox = path.bounding_box() + origin.to_vec2();
            let color = match brush {
                Brush::Solid(c) => *c,
                _ => Color::TRANSPARENT,
            };
            self.strokes.push((bbox, width, color));
            self.chrome_start = self.runs.len();
        }
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            let color = match run.brush {
                Brush::Solid(c) => c,
                _ => Color::TRANSPARENT,
            };
            let t = run.transform.translation();
            self.runs.push((run.font_size, Point::new(t.x, t.y), color));
        }
    }

    impl Recorder {
        /// The indicator rect of a filled field — the last rect of the pass
        /// (the wrapped field's caret is a rect too, painted before it).
        fn indicator(&self) -> (Point, Size, Color) {
            *self
                .rects
                .last()
                .expect("a filled field paints an indicator")
        }

        /// The outline stroke of an outlined field.
        fn outline(&self) -> (Rect, f64, Color) {
            *self
                .strokes
                .last()
                .expect("an outlined field strokes an outline")
        }

        /// Every *chrome* glyph run recorded at `font_size`, in paint order —
        /// see [`Recorder::chrome_start`].
        fn runs_at(&self, font_size: f32) -> Vec<(Point, Color)> {
            self.runs[self.chrome_start..]
                .iter()
                .filter(|(size, _, _)| (*size - font_size).abs() < 1e-3)
                .map(|(_, origin, color)| (*origin, *color))
                .collect()
        }
    }

    /// A field driven through a real `RenderRoot`, which is what makes the
    /// focus-path read (`PaintCtx::has_focus`) — and therefore every focused
    /// treatment — observable at all.
    struct Harness {
        root: RenderRoot<String, TextFieldView<String>>,
        state: String,
        tcx: TextContext,
        variant: TextFieldVariant,
        label: Option<String>,
        supporting: Option<String>,
        error: Option<String>,
        enabled: bool,
    }

    impl Harness {
        fn new(variant: TextFieldVariant) -> Self {
            let mut h = Harness {
                root: RenderRoot::new(),
                state: String::new(),
                tcx: TextContext::new(),
                variant,
                label: Some("Email".to_string()),
                supporting: None,
                error: None,
                enabled: true,
            };
            h.pass();
            h
        }

        fn theme(&mut self, brightness: Brightness) -> Theme {
            let theme = crate::baseline().with_brightness(brightness);
            self.root.set_theme(Box::new(theme.clone()));
            self.pass();
            theme
        }

        fn pass(&mut self) {
            let (variant, enabled) = (self.variant, self.enabled);
            let (label, supporting, error) = (
                self.label.clone(),
                self.supporting.clone(),
                self.error.clone(),
            );
            let mut logic = move |state: &mut String| {
                let mut view = text_field::<String, _>(state.clone(), |s: &mut String, t| *s = t)
                    .variant(variant)
                    .enabled(enabled);
                if let Some(label) = &label {
                    view = view.label(label.clone());
                }
                if let Some(supporting) = &supporting {
                    view = view.supporting_text(supporting.clone());
                }
                if let Some(error) = &error {
                    view = view.error_text(error.clone());
                }
                view
            };
            self.root.rebuild(&mut logic, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
        }

        fn paint(&mut self) -> Recorder {
            self.paint_at(FrameTime::ZERO)
        }

        fn paint_at(&mut self, now: FrameTime) -> Recorder {
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, now);
            rec
        }

        fn pointer(&mut self, phase: PointerPhase, at: Point) {
            self.root.event(
                &mut self.state,
                &InputEvent::Pointer(PointerEvent {
                    phase,
                    position: at,
                    button: PointerButton::Primary,
                }),
            );
        }

        /// Tap the field, which focuses the wrapped editable.
        fn focus(&mut self) {
            self.pointer(PointerPhase::Down, IN_FIELD);
            self.pointer(PointerPhase::Up, IN_FIELD);
        }

        /// Blur by tapping below the decoration box (still inside the window,
        /// outside every pod).
        fn blur(&mut self) {
            self.pointer(PointerPhase::Down, Point::new(160.0, 140.0));
            self.pointer(PointerPhase::Up, Point::new(160.0, 140.0));
        }

        fn key(&mut self, ch: char) {
            self.root.event(
                &mut self.state,
                &InputEvent::Key(KeyEvent {
                    key: Key::Character(ch.to_string()),
                    modifiers: Modifiers::default(),
                    repeat: false,
                }),
            );
        }
    }

    fn secs(s: f64) -> FrameTime {
        FrameTime::from_nanos((s * 1_000_000_000.0) as u64)
    }

    fn build<S: 'static>(view: &TextFieldView<S>) -> TextFieldWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout_widget(w: &mut TextFieldWidget, width: f64) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(width, 400.0)))
    }

    // ---- geometry ---------------------------------------------------------

    #[test]
    fn the_wrapped_field_covers_the_whole_decoration_box() {
        let view: TextFieldView<String> = text_field("", |_s: &mut String, _t| {}).label("Email");
        let mut w = build(&view);
        let size = layout_widget(&mut w, 300.0);
        assert_eq!(
            size,
            Size::new(300.0, DECORATION_HEIGHT),
            "no supporting line"
        );
        let field = &w.children[w.slots.field];
        assert_eq!(field.origin(), Point::ORIGIN);
        assert_eq!(
            field.size(),
            Size::new(300.0, DECORATION_HEIGHT),
            "tight constraints make the baseline's background the container fill"
        );
    }

    #[test]
    fn a_supporting_line_grows_the_widget_below_the_decoration_box() {
        let view: TextFieldView<String> =
            text_field("", |_s: &mut String, _t| {}).supporting_text("Required");
        let mut w = build(&view);
        let size = layout_widget(&mut w, 300.0);
        assert!(size.height > DECORATION_HEIGHT);
        assert_eq!(size.height, DECORATION_HEIGHT + w.supporting_height);
        assert!(
            w.supporting_height > SUPPORTING_GAP,
            "gap plus a shaped line"
        );
        // The decoration box itself never grows: the wrapped field still spans
        // exactly it.
        assert_eq!(
            w.children[w.slots.field].size(),
            Size::new(300.0, DECORATION_HEIGHT)
        );
    }

    #[test]
    fn an_unbounded_width_falls_back_to_the_baseline_default() {
        let view: TextFieldView<String> = text_field("", |_s: &mut String, _t| {});
        let mut w = build(&view);
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let size = w.layout(
            &mut lctx,
            &BoxConstraints::loose(Size::new(f64::INFINITY, 400.0)),
        );
        assert_eq!(size.width, UNBOUNDED_WIDTH);
    }

    #[test]
    fn a_leading_slot_insets_the_text_and_both_slots_are_placed_and_centered() {
        let plain: TextFieldView<String> = text_field("", |_s: &mut String, _t| {});
        assert_eq!(plain.text_left(), HPAD);
        assert_eq!(plain.text_top(), 0.0, "no label leaves the field centered");

        let view: TextFieldView<String> = text_field("", |_s: &mut String, _t| {})
            .label("Email")
            .leading(frust::SizedBox(Some(20.0), Some(20.0)))
            .trailing(frust::SizedBox(Some(20.0), Some(20.0)));
        assert_eq!(view.text_left(), HPAD + ICON_SIZE + ICON_GAP);
        assert_eq!(view.text_top(), TEXT_TOP_WITH_LABEL);

        let mut w = build(&view);
        layout_widget(&mut w, 300.0);
        let leading = &w.children[w.slots.leading.expect("leading slot")];
        let trailing = &w.children[w.slots.trailing.expect("trailing slot")];
        assert_eq!(leading.origin(), Point::new(HPAD + 2.0, 18.0));
        assert_eq!(
            trailing.origin(),
            Point::new(300.0 - HPAD - ICON_SIZE + 2.0, 18.0)
        );
    }

    // ---- variant chrome ---------------------------------------------------

    #[test]
    fn a_filled_field_paints_an_indicator_and_an_outlined_one_an_outline() {
        let mut filled = Harness::new(TextFieldVariant::Filled);
        let theme = filled.theme(Brightness::Light);
        let rec = filled.paint();
        let (origin, size, color) = rec.indicator();
        assert_eq!(origin, Point::new(0.0, DECORATION_HEIGHT - STROKE_REST));
        assert_eq!(size, Size::new(WINDOW.width, STROKE_REST));
        assert_eq!(
            color,
            theme.scheme().on_surface_variant,
            "an idle filled indicator is onSurfaceVariant"
        );
        assert!(rec.strokes.is_empty(), "the filled variant strokes nothing");
        // The wrapped baseline field still paints the container fill.
        assert!(
            rec.rrects
                .iter()
                .any(|(_, _, _, c)| *c == theme.scheme().surface),
            "the wrapped field painted its background"
        );

        let mut outlined = Harness::new(TextFieldVariant::Outlined);
        let theme = outlined.theme(Brightness::Light);
        let rec = outlined.paint();
        let (bbox, width, color) = rec.outline();
        assert_eq!(width, STROKE_REST);
        assert_eq!(
            color,
            theme.scheme().outline,
            "an idle outline is `outline`"
        );
        assert_eq!(bbox.width(), WINDOW.width - STROKE_REST);
        assert_eq!(bbox.height(), DECORATION_HEIGHT - STROKE_REST);
        assert!(
            rec.rects.is_empty(),
            "the outlined variant fills no indicator"
        );
    }

    #[test]
    fn focus_thickens_and_accents_both_variants() {
        for variant in [TextFieldVariant::Filled, TextFieldVariant::Outlined] {
            let mut h = Harness::new(variant);
            let theme = h.theme(Brightness::Light);
            h.focus();
            let rec = h.paint();
            let (width, color) = match variant {
                TextFieldVariant::Filled => {
                    let (origin, size, color) = rec.indicator();
                    assert_eq!(origin.y, DECORATION_HEIGHT - STROKE_FOCUSED);
                    (size.height, color)
                }
                TextFieldVariant::Outlined => {
                    let (_, width, color) = rec.outline();
                    (width, color)
                }
            };
            assert_eq!(width, STROKE_FOCUSED, "{variant:?} thickens on focus");
            assert_eq!(
                color,
                theme.scheme().primary,
                "{variant:?} takes the accent"
            );
        }
    }

    #[test]
    fn an_error_field_accents_the_stroke_even_unfocused() {
        for variant in [TextFieldVariant::Filled, TextFieldVariant::Outlined] {
            let mut h = Harness::new(variant);
            h.error = Some("Enter a valid address".to_string());
            let theme = h.theme(Brightness::Light);
            let rec = h.paint();
            let (width, color) = match variant {
                TextFieldVariant::Filled => {
                    let (_, size, color) = rec.indicator();
                    (size.height, color)
                }
                TextFieldVariant::Outlined => {
                    let (_, width, color) = rec.outline();
                    (width, color)
                }
            };
            assert_eq!(color, theme.scheme().error, "{variant:?} strokes `error`");
            assert_eq!(
                width, STROKE_REST,
                "error alone does not thicken the stroke"
            );
        }
    }

    #[test]
    fn a_disabled_field_dims_container_and_content_and_refuses_focus() {
        for variant in [TextFieldVariant::Filled, TextFieldVariant::Outlined] {
            let mut h = Harness::new(variant);
            h.enabled = false;
            h.supporting = Some("Required".to_string());
            let theme = h.theme(Brightness::Light);
            let on_surface = theme.scheme().on_surface;
            let rec = h.paint();
            let stroke = match variant {
                TextFieldVariant::Filled => rec.indicator().2,
                TextFieldVariant::Outlined => rec.outline().2,
            };
            assert_eq!(
                stroke,
                on_surface.multiply_alpha(DISABLED_CONTAINER_OPACITY),
                "{variant:?} dims its container ink to 12%"
            );
            let content = on_surface.multiply_alpha(DISABLED_CONTENT_OPACITY);
            assert!(
                rec.runs.iter().any(|(_, _, c)| *c == content),
                "{variant:?} dims its label/supporting ink to 38%"
            );

            // Inert: the wrapped field refuses focus, so nothing thickens.
            h.focus();
            let rec = h.paint();
            let width = match variant {
                TextFieldVariant::Filled => rec.indicator().1.height,
                TextFieldVariant::Outlined => rec.outline().1,
            };
            assert_eq!(width, STROKE_REST);
        }
    }

    #[test]
    fn an_unthemed_pass_paints_the_fallback_roles() {
        let view: TextFieldView<String> = text_field("", |_s: &mut String, _t| {}).label("Email");
        let mut w = build(&view);
        let size = layout_widget(&mut w, 300.0);
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::new(Point::ORIGIN, size);
        w.paint(&mut ctx, &mut rec);
        assert_eq!(rec.indicator().2, ON_SURFACE_VARIANT);
        assert_eq!(
            rec.runs_at(BODY_LARGE_SIZE)[0].1,
            ON_SURFACE_VARIANT,
            "a resting label is onSurfaceVariant"
        );
    }

    // ---- label float ------------------------------------------------------

    /// The float condition's whole truth table: focus x content presence.
    #[test]
    fn the_label_floats_on_focus_or_content_and_rests_only_when_neither() {
        // 1. unfocused + empty -> resting.
        let mut h = Harness::new(TextFieldVariant::Filled);
        h.theme(Brightness::Light);
        let rec = h.paint();
        assert_eq!(rec.runs_at(BODY_SMALL_SIZE).len(), 0, "no floated run");
        assert_eq!(rec.runs_at(BODY_LARGE_SIZE).len(), 1, "the resting run");

        // 2. focused + empty -> floated (settled after the transition).
        h.focus();
        h.paint_at(secs(0.0));
        let rec = h.paint_at(secs(1.0));
        assert_eq!(rec.runs_at(BODY_LARGE_SIZE).len(), 0);
        assert_eq!(rec.runs_at(BODY_SMALL_SIZE).len(), 1);

        // 3. focused + content -> still floated.
        h.key('a');
        h.pass();
        assert_eq!(h.state, "a");
        let rec = h.paint_at(secs(2.0));
        assert_eq!(rec.runs_at(BODY_SMALL_SIZE).len(), 1);

        // 4. unfocused + content -> floated.
        h.blur();
        let rec = h.paint_at(secs(3.0));
        assert_eq!(
            rec.runs_at(BODY_SMALL_SIZE).len(),
            1,
            "content alone keeps the label up"
        );
        assert_eq!(rec.runs_at(BODY_LARGE_SIZE).len(), 0);
    }

    #[test]
    fn the_float_transition_crossfades_and_travels_upward() {
        let mut h = Harness::new(TextFieldVariant::Filled);
        h.theme(Brightness::Light);
        let resting = h.paint_at(secs(0.0)).runs_at(BODY_LARGE_SIZE)[0].0;

        h.focus();
        // Seed the controller's clock, then sample mid-transition.
        h.paint_at(secs(10.0));
        let mid = h.paint_at(secs(10.0 + MaterialMotion::SHORT_3.as_secs_f64() / 2.0));
        let rest_run = mid.runs_at(BODY_LARGE_SIZE);
        let float_run = mid.runs_at(BODY_SMALL_SIZE);
        assert_eq!(rest_run.len(), 1, "both layers are live mid-crossfade");
        assert_eq!(float_run.len(), 1);
        let rest_alpha = rest_run[0].1.components[3];
        let float_alpha = float_run[0].1.components[3];
        assert!(rest_alpha > 0.0 && rest_alpha < 1.0, "{rest_alpha}");
        assert!(float_alpha > 0.0 && float_alpha < 1.0, "{float_alpha}");
        assert!(
            (rest_alpha + float_alpha - 1.0).abs() < 1e-3,
            "the crossfade is complementary"
        );
        assert!(
            rest_run[0].0.y < resting.y,
            "the label has started travelling"
        );

        // Settled: only the floated layer, at the floated band.
        let done = h.paint_at(secs(11.0));
        let floated = done.runs_at(BODY_SMALL_SIZE);
        assert_eq!(floated.len(), 1);
        assert!(done.runs_at(BODY_LARGE_SIZE).is_empty());
        assert!(floated[0].0.y >= LABEL_FLOAT_TOP);
        assert!(floated[0].0.y < resting.y);
        assert_eq!(floated[0].1.components[3], 1.0, "fully opaque once settled");
    }

    #[test]
    fn a_floated_label_takes_the_accent_and_a_resting_one_does_not() {
        let mut h = Harness::new(TextFieldVariant::Filled);
        let theme = h.theme(Brightness::Light);
        let resting = h.paint().runs_at(BODY_LARGE_SIZE)[0].1;
        assert_eq!(resting, theme.scheme().on_surface_variant);

        h.focus();
        h.paint_at(secs(0.0));
        let floated = h.paint_at(secs(1.0)).runs_at(BODY_SMALL_SIZE)[0].1;
        assert_eq!(floated, theme.scheme().primary);

        // In the error state the floated label takes `error` instead.
        h.error = Some("Bad".to_string());
        h.pass();
        let floated = h.paint_at(secs(2.0)).runs_at(BODY_SMALL_SIZE)[0].1;
        assert_eq!(floated, theme.scheme().error);
    }

    #[test]
    fn reduce_motion_collapses_the_float_and_asks_for_no_frames() {
        let mut theme = crate::baseline().with_brightness(Brightness::Light);
        theme.motion.reduce_motion = true;
        let view: TextFieldView<String> = text_field("", |_s: &mut String, _t| {}).label("Email");
        let mut w = build(&view);
        let size = layout_widget(&mut w, 300.0);

        // Not focused, empty: resting immediately.
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::new(Point::ORIGIN, size).with_theme(&theme as &dyn Any);
        w.paint(&mut ctx, &mut rec);
        assert_eq!(rec.runs_at(BODY_LARGE_SIZE).len(), 1);
        assert!(!ctx.needs_frame(), "a collapsed float asks for no frame");

        // A value arrives: the floated layer is fully up on the very next
        // paint, with no transition frames in between.
        w.value_empty = false;
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::new(Point::ORIGIN, size).with_theme(&theme as &dyn Any);
        w.paint(&mut ctx, &mut rec);
        let floated = rec.runs_at(BODY_SMALL_SIZE);
        assert_eq!(floated.len(), 1);
        assert_eq!(floated[0].1.components[3], 1.0);
        assert!(rec.runs_at(BODY_LARGE_SIZE).is_empty());
        assert!(!ctx.needs_frame());
    }

    #[test]
    fn a_field_built_with_content_starts_floated() {
        let view: TextFieldView<String> =
            text_field("already here", |_s: &mut String, _t| {}).label("Email");
        let mut w = build(&view);
        let size = layout_widget(&mut w, 300.0);
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::new(Point::ORIGIN, size);
        w.paint(&mut ctx, &mut rec);
        assert_eq!(rec.runs_at(BODY_SMALL_SIZE).len(), 1, "no fade-in on mount");
        assert!(rec.runs_at(BODY_LARGE_SIZE).is_empty());
    }

    // ---- supporting text --------------------------------------------------

    #[test]
    fn the_error_text_outranks_the_helper_text() {
        let helper: TextFieldView<String> =
            text_field("", |_s: &mut String, _t| {}).supporting_text("Helper");
        assert_eq!(helper.supporting_line(), Some("Helper"));

        let both: TextFieldView<String> = text_field("", |_s: &mut String, _t| {})
            .supporting_text("Helper")
            .error_text("Wrong");
        assert_eq!(both.supporting_line(), Some("Wrong"), "error wins");
        assert!(both.has_error());

        let neither: TextFieldView<String> = text_field("", |_s: &mut String, _t| {});
        assert_eq!(neither.supporting_line(), None);

        // And the shown line is the error's, in the error ink.
        let mut h = Harness::new(TextFieldVariant::Filled);
        h.label = None;
        h.supporting = Some("Helper".to_string());
        let theme = h.theme(Brightness::Light);
        let helper_run = h.paint().runs_at(BODY_SMALL_SIZE);
        assert_eq!(helper_run.len(), 1);
        assert_eq!(helper_run[0].1, theme.scheme().on_surface_variant);

        h.error = Some("Wrong".to_string());
        h.pass();
        let error_run = h.paint().runs_at(BODY_SMALL_SIZE);
        assert_eq!(error_run.len(), 1, "one line, not two");
        assert_eq!(error_run[0].1, theme.scheme().error);
        assert_eq!(
            error_run[0].0,
            Point::new(HPAD, DECORATION_HEIGHT + SUPPORTING_GAP),
            "placed below the decoration box"
        );
    }

    // ---- behaviour --------------------------------------------------------

    #[test]
    fn typing_round_trips_through_the_callback_and_never_self_mutates() {
        let mut h = Harness::new(TextFieldVariant::Filled);
        h.focus();
        h.key('h');
        h.key('i');
        assert_eq!(h.state, "hi", "each edit reported through on_change");

        // The app rejecting an edit wins: the next rebuild feeds its own value
        // back down, which is the controlled contract the baseline implements.
        h.state = "frozen".to_string();
        h.pass();
        assert_eq!(h.state, "frozen");
    }

    #[test]
    fn the_cursor_is_text_over_the_field_and_not_allowed_when_disabled() {
        let mut h = Harness::new(TextFieldVariant::Filled);
        h.supporting = Some("Required".to_string());
        h.pass();
        h.pointer(PointerPhase::Move, IN_FIELD);
        assert_eq!(h.root.cursor(), CursorIcon::Text);
        // Over the supporting line, outside the decoration box: nothing asked.
        h.pointer(
            PointerPhase::Move,
            Point::new(120.0, DECORATION_HEIGHT + 6.0),
        );
        assert_eq!(h.root.cursor(), CursorIcon::Default);

        h.enabled = false;
        h.pass();
        h.pointer(PointerPhase::Move, IN_FIELD);
        assert_eq!(h.root.cursor(), CursorIcon::NotAllowed);
    }

    #[test]
    fn visit_children_publishes_every_mounted_pod() {
        let bare: TextFieldView<String> = text_field("", |_s: &mut String, _t| {});
        let w = build(&bare);
        let mut seen = 0usize;
        Widget::visit_children(&w, &mut |_pod| seen += 1);
        assert_eq!(seen, 1, "the wrapped field alone");

        let slotted: TextFieldView<String> = text_field("", |_s: &mut String, _t| {})
            .leading(frust::SizedBox(Some(20.0), Some(20.0)))
            .trailing(frust::SizedBox(Some(20.0), Some(20.0)));
        let w = build(&slotted);
        let mut seen = 0usize;
        Widget::visit_children(&w, &mut |_pod| seen += 1);
        assert_eq!(seen, 3);
    }

    #[test]
    fn semantics_is_a_labelled_group_around_the_editables_own_node() {
        let mut logic = |state: &mut String| {
            text_field(state.clone(), |s: &mut String, t| *s = t)
                .label("Email")
                .error_text("Enter a valid address")
        };
        let mut root: RenderRoot<String, TextFieldView<String>> = RenderRoot::new();
        let mut state = String::new();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let group = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Group)
            .expect("a group node");
        assert_eq!(group.1.label(), Some("Email"));
        assert_eq!(group.1.description(), Some("Enter a valid address"));
        assert!(
            update
                .nodes
                .iter()
                .any(|(_, n)| n.role() == frust::authoring::Role::TextInput),
            "the wrapped editable's node reaches the tree through the chrome"
        );
    }

    #[test]
    fn adding_a_slot_rebuilds_the_pod_set() {
        let mut root: RenderRoot<bool, TextFieldView<bool>> = RenderRoot::new();
        let mut state = false;
        let mut logic = |with_slot: &mut bool| {
            let view: TextFieldView<bool> = text_field("", |_s: &mut bool, _t| {});
            if *with_slot {
                view.leading(frust::SizedBox(Some(20.0), Some(20.0)))
            } else {
                view
            }
        };
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);
        state = true;
        root.rebuild(&mut logic, &mut state);
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);
        // The slot's arrival must move the editable's glyphs right; the field
        // view carries the new padding through the normal child rebuild.
        state = false;
        root.rebuild(&mut logic, &mut state);
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);
    }
}
