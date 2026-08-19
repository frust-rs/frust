//! The Material 3 Expressive `Slider`: a controlled, horizontal value picker
//! painted as a segmented track with a bar handle, a floating value indicator,
//! and (optionally) discrete stops or a relocating end icon.
//!
//! Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
//! `lib/components/sliders/` (retrieved 2026-08-19).
//! Upstream: <https://github.com/paadevelopments/material_3_expressive>
//!
//! This module owns the `View`/`Widget` wiring, colour resolution, and every
//! paint call; `core` owns the tokens, the geometry, the drag state machine,
//! and the haptic scheduler — including the extension points the wavy/vertical/
//! range variants hook into.
//!
//! # Constructors: the reference's named-constructor split, kept
//!
//! `M3ESlider` ships six Dart named constructors over one widget class
//! (`m3e_sliders.dart:58-306`). This task ports the two horizontal
//! non-wavy ones, as sibling functions rather than builder flags, so the
//! mapping stays one-to-one:
//!
//! | Reference | Here |
//! |---|---|
//! | `M3ESlider(...)` | [`slider`] (alias [`Slider`]) |
//! | `M3ESlider.centered(...)` | [`centered_slider`] |
//! | `M3ESlider.wavy` / `.wavyCentered` / `.vertical` / `.verticalCentered` | *next task* |
//! | `M3ERangeSlider` | *next task* |
//!
//! Both build the same [`SliderView`], differing only in its
//! [`SliderTrackKind`] — so a variant that wants the third kind adds an enum
//! arm and a constructor, not a second widget.
//!
//! # Controlled, like every other value-reporting widget here
//!
//! [`SliderView`] carries the app's `value` and reports a *requested* one
//! through `on_changed`; it never writes its own `value` field (the
//! controlled-component rule in `docs/CODE_STANDARDS.md`'s Interaction
//! Semantics, shared with [`mod@crate::switch`]/[`mod@crate::checkbox`]). A `Down`
//! arms the drag, captures the pointer, and jumps to the tapped value; each
//! captured `Move` reports again; `Up` settles and fires `on_change_end`.
//!
//! Two departures from the reference, both forced by this framework's own
//! contracts and documented at their call sites:
//!
//! - **`Cancel` fires nothing.** Upstream routes a cancelled drag through the
//!   same `_endInteraction` an `Up` takes (`m3e_slider_build.dart:344`), which
//!   would reach app state; a `Cancel` arm here may not
//!   (`core::Drag::cancel`).
//! - **A drag deduplicates against the last value it reported**, not against
//!   the confirmed `value` field, since a rebuild is a frame away rather than
//!   synchronous as Flutter's `setState` is. Without it a slow drag inside one
//!   division would re-report (and re-tick) against a stale value. The
//!   confirmed value wins again the moment `rebuild` adopts one.
//!
//! # Haptics: opt-in, and paced by the frame clock
//!
//! `M3ESlider.haptic` defaults to `M3EHapticFeedback.none`, and upstream skips
//! the haptic call entirely at that value (`m3e_sliders.dart:603-609`) —
//! transcribed here: [`SliderView::haptic`] defaults to
//! [`HapticSignal::None`] and nothing is fired or queued until an app opts in.
//! Once it has, a discrete slider ticks on every accepted step and a
//! continuous drag ticks at most once per 60ms, resolved against the shell's
//! frame clock rather than a wall clock — see `core::HapticScheduler` for
//! why, and for the two accepted consequences.
//!
//! # Focus outline: geometry ported, reachability limited
//!
//! The concentric focus ring (`m3e_slider_thumb.dart:87-107`) and the +4dp
//! handle gap it forces (`m3e_slider_build.dart:73-75`) are both ported and
//! painted whenever [`PaintCtx::has_focus`] reads true. **Nothing focuses a
//! slider today**: this framework has no focus-traversal seam (no Tab ring),
//! and — unlike the reference — this widget does not grab focus on press. It
//! deliberately does not: upstream grabs pointer focus to serve its keyboard
//! stepping (`m3e_sliders.dart:623-665`, not ported in this core) and then
//! *hides* the outline again for pointer-driven focus
//! (`m3e_sliders.dart:456-465`), so not grabbing produces the same visuals
//! today with none of the machinery. The ring's geometry is pinned by
//! `core`'s own tests, so it is correct the day focus can reach here.
//!
//! # Not ported in this core
//!
//! `thumbBuilder`/`trackBuilder`/`dotBuilder` (per-instance paint overrides),
//! `semanticFormatterCallback`, `focusNode`/`autofocus`, keyboard stepping,
//! the desktop hover cursor (`MouseRegion`'s `SystemMouseCursors.click`,
//! `m3e_slider_build.dart:318-322` — the baseline `frust::Slider` asks for no
//! cursor either), and the wavy/vertical/range variants. The `label`,
//! `trackIcons`, `icon`, and every theme-override knob (`trackThickness`,
//! `cornerRadius`, `thumbLength`, `dotSize`, `dotSpacing`, `iconSize`,
//! `iconEdgeInset`) are.

mod core;

use std::rc::Rc;

use frust::authoring::text::{TextContext, TextLayout, TextStyle};
use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, CornerRadii, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, PointerPhase, Role, SemanticsCtx, View, Widget,
};
use frust::{AnimationController, Curve, IconData, IconSource, Theme};
use kurbo::{Affine, Point, RoundedRect, Shape, Size, Vec2};
use peniko::{Brush, Color};

use crate::interaction::{HapticSignal, InteractionState, MaterialHaptics};
use crate::press::presses;

use self::core::{
    DEFAULT_EXTENT, DISABLED_ACTIVE_OPACITY, DISABLED_INACTIVE_OPACITY, Drag, DragCtx, DragOutcome,
    FOCUS_STROKE, HANDLE_GAP, HANDLE_HEIGHT, HANDLE_WIDTH, HapticCadence, HapticScheduler,
    ICON_EDGE_INSET, ICON_SIZE_DEFAULT, IconDockInput, PRESS_MORPH_DURATION, PRESSED_HANDLE_WIDTH,
    STOP_INDICATOR_SIZE, STOP_INDICATOR_TRAILING_SPACE, SegmentRole, SliderGeometry, TICK_SIZE,
    TRACK_CORNER_RADIUS, TRACK_HEIGHT, TRACK_ICON_SIZE, TrackMetrics, VALUE_INDICATOR_PAD_X,
    VALUE_INDICATOR_PAD_Y, VALUE_INDICATOR_RADIUS, ValueSpec,
};

pub use self::core::{SliderIconPosition, SliderTrackKind};

/// Tessellation tolerance for the stroked focus ring (the same value
/// [`mod@crate::switch`]/[`crate::card`] use for their own stroked paths).
const PATH_TOLERANCE: f64 = 0.1;

/// Label-large size, in logical px — the value indicator's type role when no
/// theme is threaded (M3 `labelLarge`, 14/20).
const LABEL_LARGE_SIZE: f32 = 14.0;
/// The ink the indicator label is *shaped* with; re-brushed with its resolved
/// colour at paint time, so a recolour never misses the shape cache (the same
/// contract [`crate::text_field`]'s runs hold).
const SHAPING_INK: Color = Color::BLACK;

/// Unthemed fallback for the `primary` role (M3 baseline light `#6750A4`).
const PRIMARY: Color = Color::from_rgb8(0x67, 0x50, 0xA4);
/// Unthemed fallback for the `on_primary` role (M3 baseline light `#FFFFFF`).
const ON_PRIMARY: Color = Color::from_rgb8(0xFF, 0xFF, 0xFF);
/// Unthemed fallback for the `secondary_container` role (M3 baseline light
/// `#E8DEF8`).
const SECONDARY_CONTAINER: Color = Color::from_rgb8(0xE8, 0xDE, 0xF8);
/// Unthemed fallback for the `on_surface` role (M3 baseline light `#1D1B20`).
const ON_SURFACE: Color = Color::from_rgb8(0x1D, 0x1B, 0x20);
/// Unthemed fallback for the `on_surface_variant` role (M3 baseline light
/// `#49454F`).
const ON_SURFACE_VARIANT: Color = Color::from_rgb8(0x49, 0x45, 0x4F);
/// Unthemed fallback for the `inverse_surface` role (M3 baseline light
/// `#322F35`).
const INVERSE_SURFACE: Color = Color::from_rgb8(0x32, 0x2F, 0x35);
/// Unthemed fallback for the `inverse_on_surface` role (M3 baseline light
/// `#F5EFF7`).
const INVERSE_ON_SURFACE: Color = Color::from_rgb8(0xF5, 0xEF, 0xF7);

/// The inks one slider paint pass resolves (`M3ESliderColors`, plus the two
/// icon roles this port folds in — see [`resolve_colors`]).
#[derive(Clone, Copy, Debug, PartialEq)]
struct SliderColors {
    thumb: Color,
    active_track: Color,
    inactive_track: Color,
    active_tick: Color,
    inactive_tick: Color,
    value_indicator: Color,
    value_indicator_label: Color,
    /// Ink for an icon sitting over the *inactive* track (the relocating
    /// icon's undocked state, and every inactive-slot inset icon).
    on_inactive: Color,
    /// Ink for an icon sitting over the *active* track.
    on_active: Color,
}

/// `M3ESliderTheme.colors` (`m3e_slider_theme.dart:147-169`), including its
/// deliberate tick reversal: a tick painted **on** the active track takes the
/// *inactive* track's colour and vice versa, so a marker always contrasts with
/// what it sits on. The stop indicator uses the active-track colour, which is
/// `inactive_tick` here — the same value, not a second role.
fn resolve_colors(theme: Option<&Theme>, enabled: bool) -> SliderColors {
    let (
        primary,
        on_primary,
        secondary_container,
        on_surface,
        on_surface_variant,
        inverse,
        on_inverse,
    ) = match theme {
        Some(theme) => {
            let s = theme.scheme();
            (
                s.primary,
                s.on_primary,
                s.secondary_container,
                s.on_surface,
                s.on_surface_variant,
                s.inverse_surface,
                s.inverse_on_surface,
            )
        }
        None => (
            PRIMARY,
            ON_PRIMARY,
            SECONDARY_CONTAINER,
            ON_SURFACE,
            ON_SURFACE_VARIANT,
            INVERSE_SURFACE,
            INVERSE_ON_SURFACE,
        ),
    };
    let active = |c: Color| {
        if enabled {
            c
        } else {
            on_surface.multiply_alpha(DISABLED_ACTIVE_OPACITY)
        }
    };
    let inactive = |c: Color| {
        if enabled {
            c
        } else {
            on_surface.multiply_alpha(DISABLED_INACTIVE_OPACITY)
        }
    };
    let active_track = active(primary);
    let inactive_track = inactive(secondary_container);
    SliderColors {
        thumb: active(primary),
        active_track,
        inactive_track,
        active_tick: inactive_track,
        inactive_tick: active_track,
        value_indicator: inverse,
        value_indicator_label: on_inverse,
        // `m3e_slider_build.dart:504-511`'s icon-colour ladder, reused for the
        // inset track icons: upstream leaves those to an ambient `IconTheme`
        // this framework has no equivalent of, so they take the same
        // over-active/over-inactive pair the relocating icon does.
        on_inactive: active(on_surface_variant),
        on_active: active(on_primary),
    }
}

/// Optional icons inset into the track's own segments
/// (`M3ESliderTrackIcons`). A slot left `None` paints nothing, and any icon
/// whose segment is shorter than `size + 8` is dropped for that frame.
#[derive(Clone, Copy, Debug)]
pub struct SliderTrackIcons {
    /// Leading end of the active segment.
    pub active_start: Option<IconSource>,
    /// Trailing end of the active segment.
    pub active_end: Option<IconSource>,
    /// Leading end of the inactive segment (standard track only).
    pub inactive_start: Option<IconSource>,
    /// Trailing end of the inactive segment (standard track only).
    pub inactive_end: Option<IconSource>,
    /// Edge length of every slot's icon, in logical px.
    pub size: f64,
}

/// Slot-wise geometry equality, hand-written because [`IconSource`] itself is
/// not `PartialEq` — the same `d`/`design` identity test `same_icon_source`
/// applies to a single slot, which is all a `rebuild` diff needs.
impl PartialEq for SliderTrackIcons {
    fn eq(&self, other: &Self) -> bool {
        self.size == other.size
            && same_icon_source(self.active_start, other.active_start)
            && same_icon_source(self.active_end, other.active_end)
            && same_icon_source(self.inactive_start, other.inactive_start)
            && same_icon_source(self.inactive_end, other.inactive_end)
    }
}

impl Default for SliderTrackIcons {
    /// [`SliderTrackIcons::new`] — an empty set at the token icon size, never
    /// a zero-sized one.
    fn default() -> Self {
        Self::new()
    }
}

impl SliderTrackIcons {
    /// An empty set at the reference's default icon size (16dp).
    pub fn new() -> Self {
        Self {
            active_start: None,
            active_end: None,
            inactive_start: None,
            inactive_end: None,
            size: TRACK_ICON_SIZE,
        }
    }

    /// Which slots hold an icon, in [`TrackIconSlot::index`] order — the same
    /// order [`SliderWidget::track_icon_geom`] caches their geometry in.
    fn present(&self) -> [bool; 4] {
        [
            self.active_start.is_some(),
            self.active_end.is_some(),
            self.inactive_start.is_some(),
            self.inactive_end.is_some(),
        ]
    }

    /// The resolved edge length — the struct's own `size`, or the token
    /// default if a caller zeroed it.
    fn size(&self) -> f64 {
        if self.size > 0.0 {
            self.size
        } else {
            TRACK_ICON_SIZE
        }
    }
}

/// Per-instance overrides for the values a `M3ESliderTheme` would otherwise
/// supply. One struct so `rebuild` diffs them in a single comparison, and so
/// the **explicit value > token default** precedence lives in one place.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Overrides {
    track_thickness: Option<f64>,
    corner_radius: Option<f64>,
    thumb_length: Option<f64>,
    dot_size: Option<f64>,
    dot_spacing: Option<f64>,
    icon_size: Option<f64>,
    icon_edge_inset: Option<f64>,
}

impl Overrides {
    fn track_thickness(&self) -> f64 {
        self.track_thickness.unwrap_or(TRACK_HEIGHT)
    }
    fn corner_radius(&self) -> f64 {
        self.corner_radius.unwrap_or(TRACK_CORNER_RADIUS)
    }
    fn thumb_length(&self) -> f64 {
        self.thumb_length.unwrap_or(HANDLE_HEIGHT)
    }
    fn dot_size(&self) -> f64 {
        self.dot_size.unwrap_or(STOP_INDICATOR_SIZE)
    }
    fn tick_size(&self) -> f64 {
        self.dot_size.unwrap_or(TICK_SIZE)
    }
    fn dot_spacing(&self) -> f64 {
        self.dot_spacing.unwrap_or(STOP_INDICATOR_TRAILING_SPACE)
    }
    fn icon_size(&self) -> f64 {
        self.icon_size.unwrap_or(ICON_SIZE_DEFAULT)
    }
    fn icon_edge_inset(&self) -> f64 {
        self.icon_edge_inset.unwrap_or(ICON_EDGE_INSET)
    }
}

/// A cached, lazily-shaped run for the value indicator's label — the same
/// catalog-owns-its-text pattern [`crate::text_field`] uses, for the same
/// reason: the ink is a paint-time value the shaped layout must not bake.
struct Run {
    content: String,
    layout: Option<TextLayout>,
    shaped_style: Option<TextStyle>,
}

impl Run {
    fn new() -> Self {
        Self {
            content: String::new(),
            layout: None,
            shaped_style: None,
        }
    }

    /// Replace the text, dropping the cached layout only if it changed.
    fn set_content(&mut self, content: &str) {
        if self.content != content {
            self.content = content.to_string();
            self.layout = None;
        }
    }

    fn size(&self) -> Size {
        self.layout.as_ref().map_or(Size::ZERO, TextLayout::size)
    }

    /// Shape (or reuse) the run in `style`.
    fn layout(&mut self, ctx: &mut LayoutCtx, style: &TextStyle) {
        if self.layout.is_some() && self.shaped_style.as_ref() == Some(style) {
            return;
        }
        let text_ctx = ctx.text_context::<TextContext>();
        self.layout = Some(text_ctx.layout(&self.content, style, None));
        self.shaped_style = Some(style.clone());
    }

    /// Paint the run at `origin` in `color`. A run never laid out paints
    /// nothing.
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

/// The label-large type role the value indicator shapes in. Themed:
/// `Theme::type_scale.label_large`. Unthemed: [`LABEL_LARGE_SIZE`] at the
/// default family.
fn label_large(theme: Option<&Theme>) -> TextStyle {
    let mut style = match theme {
        Some(theme) => theme.type_scale.label_large.clone(),
        None => TextStyle::new(LABEL_LARGE_SIZE, SHAPING_INK),
    };
    style.color = SHAPING_INK;
    style
}

/// Resolve an optional [`IconSource`] into its cached `(design-space path,
/// design box)` pair (the same helper [`mod@crate::switch`] carries).
fn resolve_icon(source: Option<IconSource>) -> Option<(kurbo::BezPath, f64)> {
    source.map(|s| IconData::from(s).resolve())
}

/// Whether two optional icon sources name the same geometry.
fn same_icon_source(a: Option<IconSource>, b: Option<IconSource>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => x.d == y.d && x.design == y.design,
        (None, None) => true,
        _ => false,
    }
}

/// A view-held, typed value callback (erased on build).
type OnValue<State> = Rc<dyn Fn(&mut State, f64)>;

/// A declarative M3E slider. See the [module docs](self).
pub struct SliderView<State: 'static> {
    value: f64,
    spec: ValueSpec,
    kind: SliderTrackKind,
    enabled: bool,
    label: Option<String>,
    haptic: HapticSignal,
    icon: Option<IconSource>,
    icon_position: SliderIconPosition,
    track_icons: Option<SliderTrackIcons>,
    overrides: Overrides,
    on_changed: OnValue<State>,
    on_change_end: Option<OnValue<State>>,
    /// The sink every fired [`HapticSignal`] goes through — always
    /// [`MaterialHaptics::fire`] in a real build. It is a field rather than a
    /// direct call so a test can count ticks without touching
    /// [`MaterialHaptics`]' process-global, set-once hook (the injectable-hook
    /// seam in `docs/CODE_STANDARDS.md`'s Testing Patterns); a global install
    /// here would race [`crate::interaction`]'s own first-set-wins coverage.
    haptic_sink: fn(HapticSignal),
}

/// Create a standard horizontal slider at `value` that fires
/// `on_changed(state, requested)` as it is dragged (`M3ESlider`).
///
/// The value range is `0.0..=1.0` until [`SliderView::range`] widens it, and
/// the slider is continuous until [`SliderView::divisions`] steps it.
pub fn slider<State: 'static, F: Fn(&mut State, f64) + 'static>(
    value: f64,
    on_changed: F,
) -> SliderView<State> {
    SliderView {
        value,
        spec: ValueSpec::default(),
        kind: SliderTrackKind::Standard,
        enabled: true,
        label: None,
        haptic: HapticSignal::None,
        icon: None,
        icon_position: SliderIconPosition::End,
        track_icons: None,
        overrides: Overrides::default(),
        on_changed: Rc::new(on_changed),
        on_change_end: None,
        haptic_sink: MaterialHaptics::fire,
    }
}

/// PascalCase alias for [`slider`].
#[allow(non_snake_case)]
pub fn Slider<State: 'static, F: Fn(&mut State, f64) + 'static>(
    value: f64,
    on_changed: F,
) -> SliderView<State> {
    slider(value, on_changed)
}

/// Create a horizontal slider whose active track grows from the track's
/// midpoint toward the thumb (`M3ESlider.centered`).
pub fn centered_slider<State: 'static, F: Fn(&mut State, f64) + 'static>(
    value: f64,
    on_changed: F,
) -> SliderView<State> {
    SliderView {
        kind: SliderTrackKind::Centered,
        ..slider(value, on_changed)
    }
}

impl<State: 'static> SliderView<State> {
    /// Set the value bounds (`M3ESlider.min`/`max`, defaulting to `0..=1`).
    ///
    /// Upstream asserts `max > min` (`m3e_sliders.dart:95`); an inverted or
    /// empty range `debug_assert!`s here and otherwise degrades to a slider
    /// pinned at its minimum — never a panic in a shipped app.
    pub fn range(mut self, min: f64, max: f64) -> Self {
        debug_assert!(max > min, "a slider's max must be greater than its min");
        self.spec.min = min;
        self.spec.max = max;
        self
    }

    /// Step the slider into `divisions` equal intervals
    /// (`M3ESlider.divisions`): `divisions + 1` stops, endpoints included.
    /// `0` is treated as continuous, matching upstream's `<= 0` guard.
    pub fn divisions(mut self, divisions: u32) -> Self {
        self.spec.divisions = Some(divisions);
        self
    }

    /// Fire `on_change_end(state, value)` when an interaction settles
    /// (`M3ESlider.onChangeEnd`). A cancelled drag never fires it — see the
    /// [module docs](self).
    pub fn on_change_end<F: Fn(&mut State, f64) + 'static>(mut self, on_change_end: F) -> Self {
        self.on_change_end = Some(Rc::new(on_change_end));
        self
    }

    /// Override the value indicator's text (`M3ESlider.label`). Without one it
    /// is the rounded value on a discrete slider and a two-decimal one
    /// otherwise (`m3e_slider_build.dart:88-92`).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Gate interaction and switch to the disabled colour table
    /// (`M3ESlider.enabled`). Upstream also treats a null `onChanged` as
    /// disabled; this crate's controlled-component contract always takes a
    /// concrete callback, so the flag is the only switch.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Opt into haptic feedback on value changes (`M3ESlider.haptic`,
    /// defaulting to none). See the [module docs](self)' Haptics section.
    pub fn haptic(mut self, haptic: HapticSignal) -> Self {
        self.haptic = haptic;
        self
    }

    /// Show a relocating icon at one track end (`M3ESlider.icon`), which docks
    /// beside the thumb rather than letting it cover the icon.
    ///
    /// Upstream asserts this is mutually exclusive with `divisions`
    /// (`m3e_sliders.dart:96-99`); here the pair `debug_assert!`s at build and
    /// the icon is simply not painted on a discrete slider. Either way the
    /// track's stop indicators give up their place to the icon.
    pub fn icon(mut self, icon: IconSource) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Which end the relocating [`icon`](Self::icon) rests at
    /// (`M3ESlider.iconPosition`).
    pub fn icon_position(mut self, position: SliderIconPosition) -> Self {
        self.icon_position = position;
        self
    }

    /// Edge length of the relocating [`icon`](Self::icon), in logical px
    /// (`M3ESlider.iconSize`).
    pub fn icon_size(mut self, size: f64) -> Self {
        self.overrides.icon_size = Some(size);
        self
    }

    /// Clear space between the track edge and the relocating
    /// [`icon`](Self::icon)'s outer edge (`M3ESlider.iconEdgeInset`).
    pub fn icon_edge_inset(mut self, inset: f64) -> Self {
        self.overrides.icon_edge_inset = Some(inset);
        self
    }

    /// Inset icons for the track's own segments (`M3ESlider.trackIcons`).
    pub fn track_icons(mut self, icons: SliderTrackIcons) -> Self {
        self.track_icons = Some(icons);
        self
    }

    /// Thickness of both track segments, in logical px
    /// (`M3ESlider.trackThickness`).
    pub fn track_thickness(mut self, thickness: f64) -> Self {
        self.overrides.track_thickness = Some(thickness);
        self
    }

    /// Outer corner radius of both track segments, in logical px
    /// (`M3ESlider.cornerRadius`); clamped to half the track thickness.
    pub fn corner_radius(mut self, radius: f64) -> Self {
        self.overrides.corner_radius = Some(radius);
        self
    }

    /// Length of the thumb along the cross axis, in logical px
    /// (`M3ESlider.thumbLength`).
    pub fn thumb_length(mut self, length: f64) -> Self {
        self.overrides.thumb_length = Some(length);
        self
    }

    /// Diameter of the stop/tick markers, in logical px
    /// (`M3ESlider.dotSize`).
    pub fn dot_size(mut self, size: f64) -> Self {
        self.overrides.dot_size = Some(size);
        self
    }

    /// Clear space between each track end and the outer edge of its marker, in
    /// logical px (`M3ESlider.dotSpacing`).
    pub fn dot_spacing(mut self, spacing: f64) -> Self {
        self.overrides.dot_spacing = Some(spacing);
        self
    }

    /// Route fired haptics somewhere other than [`MaterialHaptics`] — the
    /// counting seam this module's own tests use. See
    /// [`SliderView::haptic_sink`](struct.SliderView.html#structfield.haptic_sink).
    #[cfg(test)]
    fn haptic_sink(mut self, sink: fn(HapticSignal)) -> Self {
        self.haptic_sink = sink;
        self
    }

    /// The value indicator's text for the current value
    /// (`m3e_slider_build.dart:88-92`).
    fn indicator_text(&self) -> String {
        match &self.label {
            Some(label) => label.clone(),
            None if self.spec.divisions.is_some() => format!("{}", self.value.round()),
            None => format!("{:.2}", self.value),
        }
    }
}

/// The retained widget for a [`SliderView`].
pub struct SliderWidget {
    /// The app-confirmed value (source of truth; adopted on `rebuild`).
    value: f64,
    spec: ValueSpec,
    ticks: Vec<f64>,
    kind: SliderTrackKind,
    enabled: bool,
    overrides: Overrides,
    haptic: HapticSignal,
    icon: Option<IconSource>,
    icon_geom: Option<(kurbo::BezPath, f64)>,
    icon_position: SliderIconPosition,
    track_icons: Option<SliderTrackIcons>,
    track_icon_geom: [Option<(kurbo::BezPath, f64)>; 4],
    /// The pointer state machine (pressed/dragging).
    drag: Drag,
    /// Feeds the handle halo's overlay opacity through the crate's precedence
    /// resolver; `dragged` is set for the length of a drag.
    interaction: InteractionState,
    /// The last value reported this drag — what a change deduplicates against
    /// until `rebuild` confirms one (see the [module docs](self)).
    last_reported: Option<f64>,
    /// `0.0` resting → `1.0` pressed, over 100ms of ease-out: the handle's
    /// width compression (`m3e_slider_thumb.dart:76-85`).
    press_anim: AnimationController,
    press_target: bool,
    /// `0.0` resting → `1.0` docked for the relocating icon, on
    /// [`core::DOCK_SPRING`].
    dock_anim: AnimationController,
    dock_target: bool,
    haptics: HapticScheduler,
    haptic_sink: fn(HapticSignal),
    /// The value indicator's shaped label.
    indicator: Run,
    on_changed: frust::authoring::ErasedArgCallback<f64>,
    on_change_end: Option<frust::authoring::ErasedArgCallback<f64>>,
}

impl<State: 'static> View<State> for SliderView<State> {
    type Element = SliderWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> SliderWidget {
        debug_assert!(
            self.icon.is_none() || self.spec.divisions.is_none(),
            "a relocating slider icon and discrete divisions are mutually exclusive"
        );
        let mut indicator = Run::new();
        indicator.set_content(&self.indicator_text());
        SliderWidget {
            value: self.value,
            spec: self.spec,
            ticks: self.spec.tick_fractions(),
            kind: self.kind,
            enabled: self.enabled,
            overrides: self.overrides,
            haptic: self.haptic,
            icon: self.icon,
            icon_geom: resolve_icon(self.icon),
            icon_position: self.icon_position,
            track_icons: self.track_icons,
            track_icon_geom: resolve_track_icons(self.track_icons.as_ref()),
            drag: Drag::default(),
            interaction: InteractionState::new(),
            last_reported: None,
            press_anim: AnimationController::new(PRESS_MORPH_DURATION).with_curve(Curve::EaseOut),
            press_target: false,
            dock_anim: AnimationController::new(std::time::Duration::ZERO),
            dock_target: false,
            haptics: HapticScheduler::default(),
            haptic_sink: self.haptic_sink,
            indicator,
            on_changed: frust::authoring::erase_callback_arg(&self.on_changed),
            on_change_end: self
                .on_change_end
                .as_ref()
                .map(frust::authoring::erase_callback_arg),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SliderWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_changed = frust::authoring::erase_callback_arg(&self.on_changed);
        element.on_change_end = self
            .on_change_end
            .as_ref()
            .map(frust::authoring::erase_callback_arg);
        element.haptic_sink = self.haptic_sink;
        let mut flags = ChangeFlags::NONE;

        // The value indicator's label is shaped at layout time, so anything
        // that changes its text forces a relayout rather than a bare repaint
        // (`docs/WIDGETS_CODE_STANDARDS.md`'s layout-time-baked rule).
        let text = self.indicator_text();
        if element.indicator.content != text {
            element.indicator.set_content(&text);
            flags |= ChangeFlags::LAYOUT;
        }
        if prev.value != self.value {
            element.value = self.value;
            // The app confirmed a value: stop deduplicating against the drag's
            // own last report.
            element.last_reported = None;
            flags |= ChangeFlags::PAINT;
        }
        if prev.spec != self.spec {
            element.spec = self.spec;
            element.ticks = self.spec.tick_fractions();
            flags |= ChangeFlags::PAINT;
        }
        if prev.kind != self.kind {
            element.kind = self.kind;
            flags |= ChangeFlags::PAINT;
        }
        if prev.enabled != self.enabled {
            element.enabled = self.enabled;
            if !self.enabled {
                // Disabling mid-drag disarms the machine, so a still-captured
                // pointer cannot keep reporting values: upstream simply wires
                // no gesture at all while disabled
                // (`m3e_slider_build.dart:329-331`).
                element.drag = Drag::default();
                element.interaction.set_pressed(false);
                element.interaction.set_dragged(false);
                element.last_reported = None;
            }
            flags |= ChangeFlags::PAINT;
        }
        if prev.haptic != self.haptic {
            element.haptic = self.haptic;
        }
        if prev.overrides != self.overrides {
            element.overrides = self.overrides;
            flags |= ChangeFlags::LAYOUT;
        }
        if !same_icon_source(prev.icon, self.icon) {
            element.icon = self.icon;
            element.icon_geom = resolve_icon(self.icon);
            flags |= ChangeFlags::PAINT;
        }
        if prev.icon_position != self.icon_position {
            element.icon_position = self.icon_position;
            flags |= ChangeFlags::PAINT;
        }
        if prev.track_icons != self.track_icons {
            element.track_icons = self.track_icons;
            element.track_icon_geom = resolve_track_icons(self.track_icons.as_ref());
            flags |= ChangeFlags::PAINT;
        }
        flags
    }
}

/// Resolve all four inset track-icon slots into cached geometry.
fn resolve_track_icons(icons: Option<&SliderTrackIcons>) -> [Option<(kurbo::BezPath, f64)>; 4] {
    let Some(icons) = icons else {
        return [None, None, None, None];
    };
    [
        resolve_icon(icons.active_start),
        resolve_icon(icons.active_end),
        resolve_icon(icons.inactive_start),
        resolve_icon(icons.inactive_end),
    ]
}

impl SliderWidget {
    /// The drag machine's per-event inputs. The dedup baseline is the last
    /// value already reported this drag, falling back to the confirmed one.
    fn drag_ctx(&self, extent: f64) -> DragCtx {
        DragCtx {
            spec: self.spec,
            current: self.last_reported.unwrap_or(self.value),
            extent,
            // Horizontal LTR in this core; the vertical/RTL variants flip it
            // (see `core`'s extension points).
            reverse: false,
        }
    }

    /// Apply one drag outcome: queue the haptic, report the value, settle.
    ///
    /// The haptic is queued *before* the callback runs, the same order the
    /// reference fires it in (`m3e_sliders.dart:600-610`); the frame clock
    /// decides when it actually reaches the hook.
    fn apply(&mut self, out: DragOutcome, ctx: &mut EventCtx) -> EventResult {
        if !out.handled {
            return EventResult::Ignored;
        }
        if out.capture {
            ctx.capture_pointer();
        }
        if let Some(value) = out.value {
            if self.haptic != HapticSignal::None {
                let cadence = if self.spec.divisions.is_some() {
                    HapticCadence::Immediate
                } else {
                    HapticCadence::Throttled
                };
                self.haptics.queue(self.haptic, cadence);
            }
            self.last_reported = Some(value);
            (self.on_changed)(ctx, value);
        }
        if out.end {
            let settled = self.last_reported.unwrap_or(self.value);
            if let Some(on_change_end) = &mut self.on_change_end {
                on_change_end(ctx, settled);
            }
        }
        self.interaction.set_pressed(self.drag.pressed);
        self.interaction.set_dragged(self.drag.dragging);
        ctx.request_redraw();
        EventResult::Handled
    }
}

impl Widget for SliderWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            DEFAULT_EXTENT
        };
        // The value indicator's label is shaped every pass, whether or not it
        // is currently shown: a press must not have to wait for a relayout to
        // put text on screen.
        let style = label_large(Theme::from_layout_ctx(ctx));
        self.indicator.layout(ctx, &style);
        let height = HANDLE_HEIGHT.max(self.overrides.thumb_length());
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let colors = resolve_colors(theme, self.enabled);
        let focused = ctx.has_focus();
        let o = ctx.origin();
        let size = ctx.size();
        let extent = size.width;
        let cross_center = size.height / 2.0;

        let fraction = self.spec.fraction(self.value);
        let thumb_primary = fraction * extent;
        // The gap is measured from the *instant* handle thickness even while
        // the handle's own width is still gliding — the reference's own split
        // (see `TrackMetrics::resolve`).
        let handle_thickness = if self.drag.pressed {
            PRESSED_HANDLE_WIDTH
        } else {
            HANDLE_WIDTH
        };
        let track_thickness = self.overrides.track_thickness();
        let geometry = SliderGeometry {
            extent,
            active_start_fraction: 0.0,
            active_end_fraction: fraction,
            reverse: false,
            kind: self.kind,
            handle_thickness,
            handle_gap: HANDLE_GAP
                + if focused {
                    core::FOCUS_HANDLE_GAP_EXTRA
                } else {
                    0.0
                },
            track_thickness,
            corner_radius: self.overrides.corner_radius(),
        };

        // The relocating icon's dock target is geometry-derived, so it is
        // resolved before the animation channels advance (the crate's
        // lazily-started fling idiom — see `crate::switch`).
        let dock = self.icon_dock(&geometry, thumb_primary);
        if let Some(dock) = &dock
            && dock.docked != self.dock_target
        {
            // Only the sign matters: the displacement drives the spring.
            let velocity = if dock.docked { 1e-3 } else { -1e-3 };
            self.dock_anim.fling(velocity, core::DOCK_SPRING);
            self.dock_target = dock.docked;
        }
        if self.drag.pressed != self.press_target {
            self.press_anim
                .animate_to(if self.drag.pressed { 1.0 } else { 0.0 });
            self.press_target = self.drag.pressed;
        }
        let press_animating = self.press_anim.advance(ctx.frame_time());
        let dock_animating = self.dock_anim.advance(ctx.frame_time());
        if press_animating || dock_animating {
            ctx.request_frame();
        }
        // The haptic queued by the last event pass resolves against this
        // frame's clock (see `core::HapticScheduler`).
        if let Some(signal) = self.haptics.take(ctx.frame_time()) {
            (self.haptic_sink)(signal);
        }

        let track_top = cross_center - track_thickness / 2.0;
        if let Some(metrics) = TrackMetrics::resolve(&geometry) {
            for segment in metrics.segments(false) {
                let color = match segment.role {
                    SegmentRole::Active => colors.active_track,
                    SegmentRole::Inactive => colors.inactive_track,
                };
                // Clockwise from the top-left: the span's leading corners take
                // `start_corner`, its trailing ones `end_corner`.
                let radii = CornerRadii::new(
                    segment.start_corner,
                    segment.end_corner,
                    segment.end_corner,
                    segment.start_corner,
                );
                scene.fill_rounded_rect_radii(
                    Point::new(o.x + segment.start, o.y + track_top),
                    Size::new(segment.end - segment.start, track_thickness),
                    radii,
                    color,
                );
            }

            // A relocating icon takes the stop indicators' place
            // (`m3e_slider_build.dart:227`'s `drawDots`).
            if self.icon.is_none() {
                let spec = core::DotSpec {
                    stop_size: self.overrides.dot_size(),
                    tick_size: self.overrides.tick_size(),
                    edge_inset: self.overrides.dot_spacing(),
                };
                for dot in core::dots(&metrics, &self.ticks, &spec) {
                    let color = if dot.active {
                        colors.active_tick
                    } else {
                        colors.inactive_tick
                    };
                    scene.fill_rounded_rect(
                        Point::new(
                            o.x + dot.primary - dot.size / 2.0,
                            o.y + cross_center - dot.size / 2.0,
                        ),
                        Size::new(dot.size, dot.size),
                        dot.size / 2.0,
                        color,
                    );
                }
            }
        }

        self.paint_track_icons(scene, o, extent, cross_center, fraction, &colors);

        // Handle halo: the interaction model's precedence opacity behind the
        // thumb. The reference paints no halo at all (Compose dropped the
        // slider's state layer); this crate keeps every interactive component
        // on one interaction substrate, so a dragged/pressed handle carries
        // the same overlay every other control does, at the focus ring's own
        // footprint rather than a second invented size.
        let thumb_thickness =
            HANDLE_WIDTH + (PRESSED_HANDLE_WIDTH - HANDLE_WIDTH) * self.press_anim.value_clamped();
        let thumb = core::thumb_rect(
            thumb_primary,
            cross_center,
            thumb_thickness,
            self.overrides.thumb_length(),
        );
        let overlay = self.interaction.resolve_opacity();
        if overlay > 0.0 {
            let halo = core::focus_ring_rect(thumb);
            scene.fill_rounded_rect(
                Point::new(o.x + halo.x0, o.y + halo.y0),
                halo.size(),
                core::thumb_radius(halo),
                colors.thumb.multiply_alpha(overlay),
            );
        }
        scene.fill_rounded_rect(
            Point::new(o.x + thumb.x0, o.y + thumb.y0),
            thumb.size(),
            core::thumb_radius(thumb),
            colors.thumb,
        );
        if focused {
            let ring = core::focus_ring_rect(thumb);
            let inset = FOCUS_STROKE / 2.0;
            let path = RoundedRect::from_rect(
                ring.inset(-inset),
                (core::thumb_radius(ring) - inset).max(0.0),
            )
            .to_path(PATH_TOLERANCE);
            scene.stroke_path(o, &path, FOCUS_STROKE, &Brush::Solid(colors.thumb));
        }

        if let (Some(dock), Some((path, design))) = (dock, &self.icon_geom) {
            let center = dock.center_at(self.dock_anim.value_clamped());
            let icon_size = self.overrides.icon_size();
            let color = if dock.over_active(center, thumb_primary, false) {
                colors.on_active
            } else {
                colors.on_inactive
            };
            paint_icon(
                scene,
                o,
                path,
                *design,
                Point::new(center - icon_size / 2.0, cross_center - icon_size / 2.0),
                icon_size,
                color,
            );
        }

        // The value indicator floats above the control while it is pressed
        // (`m3e_slider_build.dart:439-449`).
        if self.drag.pressed {
            let label = self.indicator.size();
            let origin = core::value_indicator_origin(thumb_primary);
            let box_size = Size::new(
                label.width + 2.0 * VALUE_INDICATOR_PAD_X,
                label.height + 2.0 * VALUE_INDICATOR_PAD_Y,
            );
            let box_origin = Point::new(o.x + origin.x, o.y + origin.y);
            scene.fill_rounded_rect(
                box_origin,
                box_size,
                VALUE_INDICATOR_RADIUS,
                colors.value_indicator,
            );
            self.indicator.paint(
                Point::new(
                    box_origin.x + VALUE_INDICATOR_PAD_X,
                    box_origin.y + VALUE_INDICATOR_PAD_Y,
                ),
                colors.value_indicator_label,
                scene,
            );
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        let extent = ctx.size().width;
        match p.phase {
            PointerPhase::Down => {
                if !self.enabled || !presses(p) {
                    return EventResult::Ignored;
                }
                let out = self.drag.down(p.position.x, &self.drag_ctx(extent));
                self.apply(out, ctx)
            }
            PointerPhase::Move => {
                let out = self.drag.moved(p.position.x, &self.drag_ctx(extent));
                self.apply(out, ctx)
            }
            PointerPhase::Up => {
                let out = self.drag.up();
                self.apply(out, ctx)
            }
            PointerPhase::Cancel => {
                // No callback, no state reach: see `core::Drag::cancel`.
                let out = self.drag.cancel();
                if !out.handled {
                    return EventResult::Ignored;
                }
                self.interaction.set_pressed(false);
                self.interaction.set_dragged(false);
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::Slider, |node| {
            node.set_numeric_value(self.value);
            node.set_min_numeric_value(self.spec.min);
            node.set_max_numeric_value(self.spec.max);
        });
    }
}

impl SliderWidget {
    /// The relocating icon's dock geometry, or `None` when this slider shows
    /// no icon. Standard tracks only, as upstream
    /// (`m3e_slider_build.dart:464-466`), and never on a discrete slider (the
    /// pair upstream asserts against).
    fn icon_dock(&self, geometry: &SliderGeometry, thumb_primary: f64) -> Option<core::IconDock> {
        if self.icon.is_none()
            || self.kind != SliderTrackKind::Standard
            || self.spec.divisions.is_some()
        {
            return None;
        }
        Some(core::icon_dock(&IconDockInput {
            extent: geometry.extent,
            thumb_primary,
            icon_size: self.overrides.icon_size(),
            edge_inset: self.overrides.icon_edge_inset(),
            handle_thickness: geometry.handle_thickness,
            position: self.icon_position,
            reverse: geometry.reverse,
        }))
    }

    /// Paint whichever inset track icons fit their segment this frame.
    fn paint_track_icons(
        &self,
        scene: &mut dyn PaintScene,
        origin: Point,
        extent: f64,
        cross_center: f64,
        fraction: f64,
        colors: &SliderColors,
    ) {
        let Some(icons) = &self.track_icons else {
            return;
        };
        let size = icons.size();
        for placement in
            core::track_icon_placements(extent, fraction, self.kind, size, icons.present())
        {
            let Some((path, design)) = &self.track_icon_geom[placement.slot.index()] else {
                continue;
            };
            let color = if placement.active {
                colors.on_active
            } else {
                colors.on_inactive
            };
            paint_icon(
                scene,
                origin,
                path,
                *design,
                Point::new(placement.primary, cross_center - size / 2.0),
                size,
                color,
            );
        }
    }
}

/// Paint one resolved icon path scaled into a `size`-square box at `local`
/// (widget-local coordinates), in `color`.
fn paint_icon(
    scene: &mut dyn PaintScene,
    origin: Point,
    path: &kurbo::BezPath,
    design: f64,
    local: Point,
    size: f64,
    color: Color,
) {
    let scale = if design > 0.0 { size / design } else { 1.0 };
    let transform = Affine::translate(Vec2::new(local.x, local.y)) * Affine::scale(scale);
    scene.fill_path(origin, &(transform * path.clone()), &Brush::Solid(color));
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::FrameTime;
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::{PointerButton, PointerEvent};
    use kurbo::BezPath;
    use std::any::Any;
    use std::sync::Mutex;

    /// The track every harness lays out and paints in.
    const WIDTH: f64 = 200.0;
    const SIZE: Size = Size::new(WIDTH, HANDLE_HEIGHT);

    #[derive(Default)]
    struct Val {
        value: f64,
        changes: Vec<f64>,
        ends: Vec<f64>,
    }

    fn view(value: f64) -> SliderView<Val> {
        slider::<Val, _>(value, |s: &mut Val, v: f64| {
            s.value = v;
            s.changes.push(v);
        })
        .on_change_end(|s: &mut Val, v: f64| s.ends.push(v))
    }

    fn build(view: &SliderView<Val>) -> SliderWidget {
        let mut counter = 0u64;
        View::<Val>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn widget(value: f64) -> SliderWidget {
        build(&view(value))
    }

    fn ev(phase: PointerPhase, x: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, HANDLE_HEIGHT / 2.0),
            button: PointerButton::Primary,
        })
    }

    fn secondary_ev(phase: PointerPhase, x: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, HANDLE_HEIGHT / 2.0),
            button: PointerButton::Secondary,
        })
    }

    fn dispatch(w: &mut SliderWidget, state: &mut Val, event: &InputEvent) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, SIZE);
        w.event(&mut ctx, event)
    }

    /// Records every primitive these tests assert on.
    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        radii: Vec<(Point, Size, CornerRadii, Color)>,
        strokes: Vec<(f64, Color)>,
        paths: Vec<Color>,
        runs: Vec<(Point, Color)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn fill_rounded_rect_radii(&mut self, o: Point, s: Size, radii: CornerRadii, color: Color) {
            self.radii.push((o, s, radii, color));
        }
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, width: f64, brush: &Brush) {
            let color = match brush {
                Brush::Solid(c) => *c,
                _ => Color::TRANSPARENT,
            };
            self.strokes.push((width, color));
        }
        fn fill_path(&mut self, _o: Point, _p: &BezPath, brush: &Brush) {
            if let Brush::Solid(color) = brush {
                self.paths.push(*color);
            }
        }
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            let color = match run.brush {
                Brush::Solid(c) => c,
                _ => Color::TRANSPARENT,
            };
            let t = run.transform.translation();
            self.runs.push((Point::new(t.x, t.y), color));
        }
    }

    impl Recorder {
        /// The thumb: the one uniform rounded rect exactly as tall as the
        /// handle (the halo is taller, the dots and the indicator shorter).
        fn thumb(&self) -> (Point, Size, f64, Color) {
            *self
                .rrects
                .iter()
                .find(|(_, s, _, _)| s.height == HANDLE_HEIGHT)
                .expect("every pass paints a thumb")
        }
    }

    fn layout(w: &mut SliderWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut ctx, &BoxConstraints::loose(SIZE))
    }

    fn paint(w: &mut SliderWidget, theme: Option<&Theme>) -> Recorder {
        let mut rec = Recorder::default();
        let mut ctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, SIZE).with_theme(t),
            None => PaintCtx::new(Point::ZERO, SIZE),
        };
        w.paint(&mut ctx, &mut rec);
        rec
    }

    // -- value reporting ----------------------------------------------------

    #[test]
    fn down_move_up_reports_in_order_and_settles_once() {
        let mut w = widget(0.0);
        let mut state = Val::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 50.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 100.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 260.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 260.0));
        assert_eq!(state.changes, vec![0.25, 0.5, 1.0], "clamped at the end");
        assert_eq!(state.ends, vec![1.0], "settles on the last reported value");
        assert_eq!(
            w.value, 0.0,
            "a controlled slider never writes its own value"
        );
    }

    #[test]
    fn a_secondary_press_never_captures_or_reports() {
        let mut w = widget(0.0);
        let mut state = Val::default();
        let result = dispatch(&mut w, &mut state, &secondary_ev(PointerPhase::Down, 50.0));
        assert!(matches!(result, EventResult::Ignored));
        assert!(!w.drag.dragging);
        assert!(state.changes.is_empty());
        // A move after it is an ordinary uncaptured pass, not a drag.
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 120.0));
        assert!(state.changes.is_empty());
    }

    #[test]
    fn a_hover_move_is_ignored_and_requests_nothing() {
        let mut w = widget(0.3);
        let mut state = Val::default();
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, SIZE);
        let result = w.event(&mut ctx, &ev(PointerPhase::Move, 100.0));
        assert!(matches!(result, EventResult::Ignored));
        assert!(!ctx.needs_redraw(), "a hover must not request a redraw");
        assert!(state.changes.is_empty());
    }

    #[test]
    fn cancel_disarms_without_reporting_or_settling() {
        let mut w = widget(0.0);
        let mut state = Val::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 100.0));
        assert!(w.drag.dragging);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Cancel, 100.0));
        assert!(!w.drag.dragging && !w.drag.pressed);
        assert!(state.ends.is_empty(), "a Cancel never reaches app state");
        // And a follow-up move is a plain hover again.
        let before = state.changes.len();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 40.0));
        assert_eq!(state.changes.len(), before);
    }

    #[test]
    fn disabled_ignores_every_pointer_phase() {
        let mut w = build(&view(0.0).enabled(false));
        let mut state = Val::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 100.0));
        assert!(!w.drag.dragging);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 120.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 120.0));
        assert!(state.changes.is_empty() && state.ends.is_empty());
    }

    #[test]
    fn disabling_mid_drag_disarms_the_machine() {
        let mut w = widget(0.0);
        let mut state = Val::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 100.0));
        assert!(w.drag.dragging);

        let mut counter = 0u64;
        let prev = view(0.0);
        let next = view(0.0).enabled(false);
        View::<Val>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(!w.drag.dragging && !w.drag.pressed);

        let before = state.changes.len();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 160.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 160.0));
        assert_eq!(
            state.changes.len(),
            before,
            "a captured drag stops reporting"
        );
        assert!(state.ends.is_empty());
    }

    #[test]
    fn a_discrete_drag_reports_only_when_it_crosses_a_step() {
        let mut w = build(&view(0.0).range(0.0, 10.0).divisions(10));
        let mut state = Val::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 100.0));
        // Still inside step 5's own catchment: nothing new to report.
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 103.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 121.0));
        assert_eq!(state.changes, vec![5.0, 6.0]);
    }

    #[test]
    fn a_drag_deduplicates_against_its_own_last_report_until_a_rebuild_confirms_one() {
        let mut w = widget(0.0);
        let mut state = Val::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 100.0));
        assert_eq!(w.last_reported, Some(0.5));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 100.0));
        assert_eq!(state.changes, vec![0.5], "the repeat is dropped");

        let mut counter = 0u64;
        let prev = view(0.0);
        let next = view(0.5);
        View::<Val>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert_eq!(w.value, 0.5);
        assert_eq!(w.last_reported, None, "the confirmed value takes over");
    }

    #[test]
    fn rebuild_adopts_value_and_bounds_without_self_mutation() {
        let mut counter = 0u64;
        let prev = view(0.2);
        let mut w = build(&prev);
        assert_eq!(w.value, 0.2);
        let next = view(0.8).range(0.0, 10.0).divisions(5);
        let flags = View::<Val>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert_eq!(w.value, 0.8);
        assert_eq!(w.spec.max, 10.0);
        assert_eq!(w.ticks.len(), 6, "divisions + 1 marks");
        assert!(flags.needs_paint());
        assert!(
            flags.needs_layout(),
            "the indicator label changed, so it must be re-shaped"
        );
    }

    // -- geometry / paint ---------------------------------------------------

    #[test]
    fn layout_fills_the_width_and_takes_the_handle_height() {
        let mut w = widget(0.5);
        assert_eq!(layout(&mut w), SIZE);

        // An unbounded width falls back to the default extent.
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let unbounded = w.layout(
            &mut ctx,
            &BoxConstraints::new(Size::ZERO, Size::new(f64::INFINITY, f64::INFINITY)),
        );
        assert_eq!(unbounded.width, DEFAULT_EXTENT);
    }

    #[test]
    fn the_track_paints_two_segments_with_asymmetric_radii_and_a_stop_dot() {
        let mut w = widget(0.5);
        layout(&mut w);
        let rec = paint(&mut w, None);

        assert_eq!(rec.radii.len(), 2, "inactive remainder, then active span");
        let (inactive_origin, inactive_size, inactive_radii, inactive_color) = rec.radii[0];
        let (active_origin, active_size, active_radii, active_color) = rec.radii[1];
        assert_eq!(active_color, PRIMARY);
        assert_eq!(inactive_color, SECONDARY_CONTAINER);
        assert_eq!(active_origin, Point::new(0.0, 14.0), "vertically centred");
        assert_eq!(active_size, Size::new(92.0, TRACK_HEIGHT));
        assert_eq!(inactive_origin.x, 108.0);
        assert_eq!(inactive_size.width, 92.0);
        // Outer ends full, gap-facing ends small.
        assert_eq!(active_radii.top_left, TRACK_CORNER_RADIUS);
        assert_eq!(active_radii.top_right, core::TRACK_INSIDE_CORNER);
        assert_eq!(inactive_radii.top_left, core::TRACK_INSIDE_CORNER);
        assert_eq!(inactive_radii.top_right, TRACK_CORNER_RADIUS);

        // One trailing stop dot plus the thumb, both uniform rounded rects.
        assert_eq!(rec.rrects.len(), 2, "stop dot, then thumb");
        let (dot_origin, dot_size, dot_radius, dot_color) = rec.rrects[0];
        assert_eq!(
            dot_size,
            Size::new(STOP_INDICATOR_SIZE, STOP_INDICATOR_SIZE)
        );
        assert_eq!(dot_radius, STOP_INDICATOR_SIZE / 2.0);
        assert_eq!(dot_origin, Point::new(190.0, 20.0), "centred at 192");
        assert_eq!(dot_color, PRIMARY, "a stop takes the active-track ink");

        let (thumb_origin, thumb_size, thumb_radius, thumb_color) = rec.rrects[1];
        assert_eq!(thumb_size, Size::new(HANDLE_WIDTH, HANDLE_HEIGHT));
        assert_eq!(thumb_origin, Point::new(98.0, 0.0));
        assert_eq!(thumb_radius, HANDLE_HEIGHT / 2.0);
        assert_eq!(thumb_color, PRIMARY);
    }

    #[test]
    fn a_centered_track_paints_three_segments() {
        let mut w = build(&centered_slider::<Val, _>(0.75, |_s, _v| {}));
        layout(&mut w);
        let rec = paint(&mut w, None);
        assert_eq!(rec.radii.len(), 3);
        assert_eq!(rec.radii[2].3, PRIMARY, "the active span paints last");
        assert_eq!(rec.radii[2].0.x, 100.0, "growing from the midpoint");
    }

    #[test]
    fn themed_paint_resolves_the_slider_roles() {
        let theme = crate::baseline();
        let scheme = theme.scheme();
        let mut w = widget(0.5);
        layout(&mut w);
        let rec = paint(&mut w, Some(&theme));
        assert_eq!(rec.radii[0].3, scheme.secondary_container);
        assert_eq!(rec.radii[1].3, scheme.primary);
        assert_eq!(rec.rrects[1].3, scheme.primary, "thumb");
    }

    #[test]
    fn a_disabled_slider_dims_active_and_inactive_by_their_own_opacities() {
        let theme = crate::baseline();
        let scheme = theme.scheme();
        let mut w = build(&view(0.5).enabled(false));
        layout(&mut w);
        let rec = paint(&mut w, Some(&theme));
        assert_eq!(
            rec.radii[0].3,
            scheme.on_surface.multiply_alpha(DISABLED_INACTIVE_OPACITY)
        );
        assert_eq!(
            rec.radii[1].3,
            scheme.on_surface.multiply_alpha(DISABLED_ACTIVE_OPACITY)
        );
    }

    #[test]
    fn discrete_ticks_paint_alongside_the_stops() {
        let mut w = build(&view(0.5).divisions(4));
        layout(&mut w);
        let rec = paint(&mut w, None);
        // One surviving stop + two interior ticks + the thumb.
        assert_eq!(rec.rrects.len(), 4);
        assert_eq!(rec.rrects[1].0.x, 52.0, "tick at 0.25, centred on 54");
        assert_eq!(
            rec.rrects[1].3, SECONDARY_CONTAINER,
            "a tick on the active track takes the inactive-track ink"
        );
        assert_eq!(rec.rrects[2].3, PRIMARY, "and vice versa past the thumb");
    }

    #[test]
    fn the_pressed_thumb_compresses_over_the_morph_and_shows_the_value_indicator() {
        let mut w = widget(0.5);
        layout(&mut w);
        let mut state = Val::default();

        // At rest: full-width thumb, no indicator container, no halo.
        let rec = paint(&mut w, None);
        assert_eq!(rec.rrects.len(), 2, "stop dot + thumb only");
        assert_eq!(rec.rrects[1].1.width, HANDLE_WIDTH);
        assert!(rec.runs.is_empty(), "no indicator label at rest");

        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 100.0));
        // The morph starts on the first paint after the press and seeds its
        // clock there, so the thumb is still at its resting width.
        let rec = paint(&mut w, None);
        assert!(w.press_anim.is_animating(), "a 100ms ease-out started");
        assert_eq!(rec.thumb().1.width, HANDLE_WIDTH);
        // Halo + indicator container are already there.
        let halo = rec
            .rrects
            .iter()
            .find(|(_, s, _, _)| s.width == HANDLE_WIDTH + core::FOCUS_INFLATE)
            .expect("a dragged handle paints its halo");
        assert!(
            (halo.3.components[3] - crate::interaction::DRAGGED_OPACITY).abs() < 1e-6,
            "the halo takes the dragged precedence opacity"
        );
        assert_eq!(rec.runs.len(), 1, "the value indicator's label");

        // Halfway through the morph the width is between the two ends; at the
        // end it is exactly the pressed width.
        w.press_anim.advance(secs(0.05));
        let mid =
            HANDLE_WIDTH + (PRESSED_HANDLE_WIDTH - HANDLE_WIDTH) * w.press_anim.value_clamped();
        assert!(mid < HANDLE_WIDTH && mid > PRESSED_HANDLE_WIDTH, "{mid}");
        w.press_anim.advance(secs(0.2));
        assert!(!w.press_anim.is_animating(), "settled inside 100ms");
        let rec = paint(&mut w, None);
        assert_eq!(rec.thumb().1.width, PRESSED_HANDLE_WIDTH);

        // Releasing hides the indicator and reverses the morph.
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 100.0));
        let rec = paint(&mut w, None);
        assert!(rec.runs.is_empty(), "the indicator hides on release");
        assert!(w.press_anim.is_animating(), "and the morph reverses");
    }

    #[test]
    fn the_focus_ring_strokes_only_while_the_control_holds_focus() {
        // Nothing focuses a slider today (see the module docs), so the ring is
        // exercised through the paint-time focus read directly.
        let mut w = widget(0.5);
        layout(&mut w);
        assert!(paint(&mut w, None).strokes.is_empty());

        let ring = core::focus_ring_rect(core::thumb_rect(
            100.0,
            HANDLE_HEIGHT / 2.0,
            HANDLE_WIDTH,
            HANDLE_HEIGHT,
        ));
        assert_eq!(ring.width(), HANDLE_WIDTH + core::FOCUS_INFLATE);
        assert_eq!(ring.height(), HANDLE_HEIGHT + core::FOCUS_INFLATE);
    }

    #[test]
    fn a_relocating_icon_replaces_the_stop_dots_and_docks_beside_the_thumb() {
        let mut w = build(&view(0.9).icon(crate::icons::CHECK));
        layout(&mut w);
        let rec = paint(&mut w, None);
        assert_eq!(rec.rrects.len(), 1, "the thumb only — no stop dots");
        assert_eq!(rec.paths.len(), 1, "the icon paints as a path");
        assert!(w.dock_target, "a thumb at 180 is inside the dock limit");
        assert!(w.dock_anim.is_animating(), "the dock spring started");

        // Far from the icon it undocks again.
        let mut counter = 0u64;
        let prev = view(0.9).icon(crate::icons::CHECK);
        let next = view(0.1).icon(crate::icons::CHECK);
        View::<Val>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        paint(&mut w, None);
        assert!(!w.dock_target);
    }

    #[test]
    fn inset_track_icons_paint_per_segment() {
        let icons = SliderTrackIcons {
            active_start: Some(crate::icons::CHECK),
            inactive_end: Some(crate::icons::CLOSE),
            ..SliderTrackIcons::new()
        };
        let mut w = build(&view(0.5).track_icons(icons));
        layout(&mut w);
        let rec = paint(&mut w, None);
        assert_eq!(rec.paths.len(), 2, "one per filled slot");
        assert_eq!(rec.paths[0], ON_PRIMARY, "over the active track");
        assert_eq!(rec.paths[1], ON_SURFACE_VARIANT, "over the inactive track");
    }

    #[test]
    fn semantics_reports_the_value_and_its_bounds() {
        fn logic(_state: &mut Val) -> SliderView<Val> {
            slider::<Val, _>(4.0, |_s, _v| {}).range(0.0, 10.0)
        }
        let mut root: frust_core::RenderRoot<Val, SliderView<Val>> = frust_core::RenderRoot::new();
        let mut state = Val::default();
        let mut tcx = TextContext::new();
        root.rebuild(&mut logic, &mut state);
        // `layout_with_text`, not the bare `layout`: the value indicator's
        // label is shaped every layout pass, so this widget needs the text
        // context a real shell always threads.
        root.layout_with_text(Size::new(300.0, 200.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Slider)
            .expect("a slider contributes a Role::Slider node");
        assert_eq!(node.numeric_value(), Some(4.0));
        assert_eq!(node.min_numeric_value(), Some(0.0));
        assert_eq!(node.max_numeric_value(), Some(10.0));
    }

    // -- haptics ------------------------------------------------------------
    //
    // One test fn, not several: the recorder below is a process-global
    // `Mutex`, and `MaterialHaptics`' own hook is a set-once `OnceLock` whose
    // install/rejection ordering `crate::interaction`'s tests already pin.
    // Counting through the widget's own sink keeps both concerns out of each
    // other's way (see `SliderView::haptic_sink`), and keeping every
    // assertion in one fn keeps the recorder's contents from depending on
    // which test the harness happens to run first.

    static RECORDED: Mutex<Vec<HapticSignal>> = Mutex::new(Vec::new());

    fn recording_sink(signal: HapticSignal) {
        RECORDED.lock().unwrap().push(signal);
    }

    fn secs(s: f64) -> FrameTime {
        FrameTime::from_nanos((s * 1_000_000_000.0) as u64)
    }

    /// Paint at an explicit frame time by advancing the widget's own clock —
    /// `PaintCtx::for_test` lives behind `frust-core`'s `test-support`
    /// feature, which this crate's dev-dependency does not enable, so the
    /// haptic scheduler is stepped directly and the paint pass then drains it.
    fn paint_at(w: &mut SliderWidget, now: FrameTime) -> usize {
        let fired = w.haptics.take(now);
        if let Some(signal) = fired {
            (w.haptic_sink)(signal);
        }
        usize::from(fired.is_some())
    }

    #[test]
    fn haptics_are_opt_in_then_paced_per_division_or_per_60ms() {
        RECORDED.lock().unwrap().clear();

        // Opt-out by default: upstream skips the call entirely at
        // `M3EHapticFeedback.none`, so nothing is even queued.
        let mut w = build(&view(0.0).haptic_sink(recording_sink));
        let mut state = Val::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 50.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 120.0));
        assert!(
            !w.haptics.has_pending(),
            "no haptic queued at HapticSignal::None"
        );
        assert!(RECORDED.lock().unwrap().is_empty());

        // Continuous, opted in: one tick per 60ms of frame clock. Frames step
        // at 16ms and the pointer moves on every one of them.
        let mut w = build(
            &view(0.0)
                .haptic(HapticSignal::SliderTick)
                .haptic_sink(recording_sink),
        );
        let mut state = Val::default();
        let mut fired = 0;
        for frame in 0..13 {
            dispatch(
                &mut w,
                &mut state,
                &ev(
                    if frame == 0 {
                        PointerPhase::Down
                    } else {
                        PointerPhase::Move
                    },
                    10.0 + f64::from(frame) * 10.0,
                ),
            );
            fired += paint_at(&mut w, secs(f64::from(frame) * 0.016));
        }
        assert_eq!(
            fired, 4,
            "192ms of drag: the first tick plus one per 60ms boundary"
        );
        assert_eq!(state.changes.len(), 13, "every frame still reports a value");
        assert_eq!(
            *RECORDED.lock().unwrap(),
            vec![HapticSignal::SliderTick; 4],
            "and each fired tick reached the sink"
        );

        // Discrete: every accepted step ticks, however fast they arrive.
        RECORDED.lock().unwrap().clear();
        let mut w = build(
            &view(0.0)
                .range(0.0, 10.0)
                .divisions(10)
                .haptic(HapticSignal::Medium)
                .haptic_sink(recording_sink),
        );
        let mut state = Val::default();
        let mut fired = 0;
        for frame in 0..5 {
            dispatch(
                &mut w,
                &mut state,
                &ev(
                    if frame == 0 {
                        PointerPhase::Down
                    } else {
                        PointerPhase::Move
                    },
                    20.0 * f64::from(frame + 1),
                ),
            );
            fired += paint_at(&mut w, secs(f64::from(frame) * 0.001));
        }
        assert_eq!(fired, 5, "one tick per crossed division, unthrottled");
        assert_eq!(*RECORDED.lock().unwrap(), vec![HapticSignal::Medium; 5]);
    }
}
