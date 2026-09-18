// Ported from `material_3_expressive` v1.0.8 (MIT, © 2026 Paa Developments;
// `tmp/material_3_expressive/lib/components/time_pickers/components/` —
// `m3e_dial_time_picker.dart` (`M3EDialTimePicker`) and
// `m3e_time_dial_painter.dart` (`M3ETimeDialPainter`), plus
// `styles/m3e_time_picker_theme.dart`'s dial metrics, retrieved 2026-08-20).
// Upstream: <https://github.com/paadevelopments/material_3_expressive>
//
// Porting decisions (each documented with its reference site below): the
// header fields, period toggle and ring are painted by this one widget instead
// of a `Row` of `M3ETappable`s over a `CustomPaint`, so the whole dial keeps a
// single hit-test/paint pass; the `FittedBox(scaleDown)` wrapper is dropped in
// favour of `bc.constrain`; and the reference's `_mode` `setState` becomes
// widget-local state, since a `View` has no `setState`.

//! The M3E clock **dial**: an hour/minute ring with a draggable hand, above a
//! header of hour/minute fields and (in 12-hour mode) an AM/PM selector.
//!
//! [`time_dial`] is a controlled component: it reports the *requested*
//! [`TimeOfDay`] through `on_change` and never rewrites its own `value`
//! (`docs/CODE_STANDARDS.md`'s Interaction Semantics). The one piece of state
//! it does own is which unit the ring is editing ([`TimePickerMode`]) — the
//! reference's own `_M3EDialTimePickerState._mode`, ephemeral view state in
//! exactly the way `pressed` is.
//!
//! # Angle ↔ value, and the snap rules (cited)
//!
//! [`dial_fraction`] is `_handleDial`'s
//! `((atan2(dy, dx) + pi/2) / 2pi) % 1` verbatim: a `0..1` sweep starting at
//! the 12 o'clock slot and running clockwise. Each mode then maps that
//! fraction with the reference's own rounding:
//!
//! | Mode | Reference | Port |
//! |---|---|---|
//! | hour, 24h | `(fraction * 24).round() % 24` | [`hour_from_fraction_24`] |
//! | hour, 12h | `(fraction * 12).round() % 12`, slot `0` ⇒ 12 | [`hour_slot_from_fraction`] |
//! | minute | `((fraction * 60).round() ~/ 5) * 5 % 60` | [`minute_from_fraction`] |
//!
//! So **minutes snap to 5**, 12-hour hours snap to the 12 ring positions, and
//! 24-hour hours snap to all 24 — see the next section.
//!
//! # 24-hour mode is one ring of even hours, not two rings
//!
//! The reference draws a **single** 12-label ring in 24-hour mode
//! (`_dialLabels`: `00, 02, 04 … 22`) while its angle map resolves all 24
//! hours (`(fraction * 24).round()`), so an odd hour is reachable by pointing
//! *between* two labels and the hand then parks on the nearer even one
//! (`_selectedIndex`: `(hour / 2).round() % 12`). It is **not** Material's own
//! inner/outer dual ring, and this port does not add one — porting the
//! reference is the contract here. [`dial_labels`]/[`selected_index`] carry
//! that shape, and `hour_ring_is_even_labels_with_all_24_reachable` pins it.
//!
//! # Degenerate pointer input
//!
//! A non-finite position (or a non-finite/empty dial box) is refused outright:
//! [`dial_fraction`] returns `None` and the widget reports nothing, so a NaN
//! can never reach the `% 24` / `~/ 5` arithmetic below (the same discipline
//! `crate::slider`'s `ValueSpec::snap` documents). A press on the exact centre
//! is *not* refused — it resolves to the 3 o'clock slot, because `atan2(0, 0)`
//! is `0` in both Dart and Rust, which is what the reference does too. Both
//! are pinned by test.
//!
//! # No hour → minute auto-advance
//!
//! Committing an hour on the ring does **not** switch the ring to minutes in
//! the reference: `_handleDial` writes the value and never touches `_mode`,
//! which only ever changes from a header-field tap (`_buildField`'s `onTap`).
//! This port matches, and `committing_an_hour_does_not_auto_advance_the_mode`
//! pins it, so a later divergence is a deliberate change rather than a drift.

use std::f64::consts::PI;
use std::rc::Rc;

use frust::authoring::text::{FontFamily, FontWeight, TextContext, TextLayout, TextStyle};
use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, ErasedArgCallback, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, PointerPhase, Role, SemanticsCtx, TypedArgCallback, View,
    Widget, erase_callback_arg,
};
use frust::{Color, Theme};
use kurbo::{Point, Rect, Size};

use super::{TimeOfDay, TimePickerMode, TimePickerStrings};
use crate::interaction::{HapticSignal, MaterialHaptics};
use crate::press::presses;

/// Dial diameter (`M3ETimePickerTheme.dialSize`).
pub const DIAL_SIZE: f64 = 256.0;
/// Radius of the selection knob riding the hand (`dialKnobRadius`).
const DIAL_KNOB_RADIUS: f64 = 20.0;
/// Radius of the hand's centre pivot dot (`dialCenterRadius`).
const DIAL_CENTER_RADIUS: f64 = 4.0;
/// Gap between the knob's outer edge and the dial's rim (`dialRingInset`).
const DIAL_RING_INSET: f64 = 4.0;
/// Stroke width of the hand (`dialHandWidth`).
const DIAL_HAND_WIDTH: f64 = 2.0;
/// Font size of a ring label (`dialLabelFontSize`).
const DIAL_LABEL_FONT_SIZE: f32 = 16.0;
/// One hour/minute header field's box (`fieldSize`).
const FIELD_W: f64 = 96.0;
const FIELD_H: f64 = 80.0;
/// Horizontal margin flanking each header field (`fieldMargin`).
const FIELD_MARGIN: f64 = 4.0;
/// One AM/PM cell's box (`periodOptionSize`).
const PERIOD_W: f64 = 48.0;
const PERIOD_H: f64 = 40.0;
/// Gap between the minute field and the period toggle (`fieldPeriodGap`).
const FIELD_PERIOD_GAP: f64 = 12.0;
/// Gap between the header row and the dial (`headerDialGap`).
const HEADER_DIAL_GAP: f64 = 24.0;
/// The corner radius of a header field and of the period toggle
/// (`M3EShapes.radiusSmall`, `m3e_dimensions.dart:68`) — the theme's `small`
/// token when one is available.
const SMALL_RADIUS: f64 = 8.0;
/// Outline stroke width of the period toggle (Flutter's `Border.all` default).
const PERIOD_BORDER_WIDTH: f64 = 1.0;

/// The header field/colon type-scale token (M3 `displayMedium`, 45/52 w400 —
/// `crate::tokens::type_scale`'s `display_medium`). This size and the period
/// token below are the roles' literals; each run's *family* is read off the
/// live [`Theme`]'s type scale in the layout pass ([`Theme::from_layout_ctx`] —
/// see [`RunFamilies::resolve`]).
const HEADER_FONT_SIZE: f32 = 45.0;
/// The AM/PM cell's type-scale token (M3 `titleMedium`, 16/24 w500).
const PERIOD_FONT_SIZE: f32 = 16.0;
const PERIOD_FONT_WEIGHT: FontWeight = FontWeight::MEDIUM;

/// Number of labels drawn around the ring (`_dialLabels` always yields 12).
pub const DIAL_SLOTS: usize = 12;

/// Unthemed fallbacks, used only when no [`Theme`] reaches `paint` (a bare
/// `RenderRoot` in a test). Values are the M3 light-scheme roles the themed
/// path resolves.
const FALLBACK_DIAL: Color = Color::from_rgb8(0xE6, 0xE0, 0xE9);
const FALLBACK_ACCENT: Color = Color::from_rgb8(0x67, 0x50, 0xA4);
const FALLBACK_ON_ACCENT: Color = Color::WHITE;
const FALLBACK_ON_SURFACE: Color = Color::from_rgb8(0x1D, 0x1B, 0x20);

/// The ring's angle for slot `index` — `_angleFor`: `-pi/2 + index * step`,
/// i.e. slot `0` at 12 o'clock, running clockwise.
pub fn slot_angle(index: usize) -> f64 {
    let step = 2.0 * PI / DIAL_SLOTS as f64;
    -PI / 2.0 + index as f64 * step
}

/// The `0..1` clockwise-from-12-o'clock sweep a pointer at `position` lands on
/// inside a `dial`-sized box, or `None` for degenerate input.
///
/// `_handleDial`'s `((atan2(dy, dx) + pi/2) / (2 * pi)) % 1`, with a finiteness
/// guard the Dart has no need for (see the [module docs](self)' *Degenerate
/// pointer input*). `None` means "report nothing", never a substituted value.
pub fn dial_fraction(position: Point, dial: Size) -> Option<f64> {
    if !position.x.is_finite()
        || !position.y.is_finite()
        || !dial.width.is_finite()
        || !dial.height.is_finite()
        || dial.width <= 0.0
        || dial.height <= 0.0
    {
        return None;
    }
    let dx = position.x - dial.width / 2.0;
    let dy = position.y - dial.height / 2.0;
    // `atan2(0, 0) == 0` — an exact-centre press resolves to the quarter turn,
    // as upstream. `rem_euclid` is Dart's `%` on doubles (always non-negative).
    let fraction = ((dy.atan2(dx) + PI / 2.0) / (2.0 * PI)).rem_euclid(1.0);
    // `rem_euclid` can return exactly `1.0` for a fraction just below zero
    // after rounding; fold it back so every caller sees a half-open range.
    Some(if fraction >= 1.0 { 0.0 } else { fraction })
}

/// 24-hour-mode hour for a ring `fraction` — `(fraction * 24).round() % 24`.
pub fn hour_from_fraction_24(fraction: f64) -> u8 {
    let raw = (fraction * 24.0).round();
    if !raw.is_finite() {
        return 0;
    }
    (raw as i64).rem_euclid(24) as u8
}

/// 12-hour-mode ring slot for a `fraction` — `(fraction * 12).round() % 12`.
/// Slot `0` is the "12" label (`_setHour`'s `slot == 0 ? 12 : slot`).
pub fn hour_slot_from_fraction(fraction: f64) -> u8 {
    let raw = (fraction * 12.0).round();
    if !raw.is_finite() {
        return 0;
    }
    (raw as i64).rem_euclid(12) as u8
}

/// Minute for a ring `fraction` — `((fraction * 60).round() ~/ 5) * 5 % 60`,
/// i.e. snapped down to the enclosing 5-minute slot.
pub fn minute_from_fraction(fraction: f64) -> u8 {
    let raw = (fraction * 60.0).round();
    if !raw.is_finite() {
        return 0;
    }
    let snapped = (raw as i64).div_euclid(5) * 5;
    snapped.rem_euclid(60) as u8
}

/// The time `position` selects inside a `dial`-sized ring box, given the unit
/// being edited and the current `value` (which supplies the untouched half of
/// the pair and, in 12-hour mode, the AM/PM half). `None` for degenerate input.
pub fn dial_value_at(
    position: Point,
    dial: Size,
    mode: TimePickerMode,
    use_24_hour: bool,
    value: TimeOfDay,
) -> Option<TimeOfDay> {
    let fraction = dial_fraction(position, dial)?;
    Some(match (mode, use_24_hour) {
        (TimePickerMode::Hour, true) => value.with_hour(hour_from_fraction_24(fraction)),
        (TimePickerMode::Hour, false) => {
            let slot = hour_slot_from_fraction(fraction);
            let hour_12 = if slot == 0 { 12 } else { slot };
            TimeOfDay::from_hour_12(hour_12, value.is_pm(), value.minute())
        }
        (TimePickerMode::Minute, _) => value.with_minute(minute_from_fraction(fraction)),
    })
}

/// The 12 ring labels for a mode — `_dialLabels`.
pub fn dial_labels(mode: TimePickerMode, use_24_hour: bool) -> Vec<String> {
    match (mode, use_24_hour) {
        (TimePickerMode::Hour, true) => (0..DIAL_SLOTS).map(|i| format!("{:02}", i * 2)).collect(),
        (TimePickerMode::Hour, false) => (0..DIAL_SLOTS)
            .map(|i| if i == 0 { 12 } else { i }.to_string())
            .collect(),
        (TimePickerMode::Minute, _) => (0..DIAL_SLOTS).map(|i| format!("{:02}", i * 5)).collect(),
    }
}

/// Which ring slot the hand parks on — `_selectedIndex`.
pub fn selected_index(value: TimeOfDay, mode: TimePickerMode, use_24_hour: bool) -> usize {
    let raw = match (mode, use_24_hour) {
        (TimePickerMode::Hour, true) => (f64::from(value.hour()) / 2.0).round() as i64,
        (TimePickerMode::Hour, false) => i64::from(value.hour_of_12()),
        (TimePickerMode::Minute, _) => (f64::from(value.minute()) / 5.0).round() as i64,
    };
    raw.rem_euclid(DIAL_SLOTS as i64) as usize
}

// ============================================================================
// Geometry
// ============================================================================

/// The header/ring boxes the widget hit-tests and paints against, in
/// widget-local coordinates. Resolved from the laid-out size so a wider box
/// centres its content rather than pinning it left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct DialLayout {
    pub(super) hour_field: Rect,
    pub(super) minute_field: Rect,
    /// Centre of the ":" separator glyph run.
    pub(super) colon_center: Point,
    /// AM cell, PM cell, and the toggle's outlined box — 12-hour mode only.
    pub(super) period: Option<(Rect, Rect, Rect)>,
    pub(super) ring: Rect,
}

impl DialLayout {
    /// Header content width for a `colon_width`-wide separator.
    fn header_width(colon_width: f64, use_24_hour: bool) -> f64 {
        let fields = 2.0 * (FIELD_W + 2.0 * FIELD_MARGIN) + colon_width;
        if use_24_hour {
            fields
        } else {
            fields + FIELD_PERIOD_GAP + PERIOD_W
        }
    }

    /// The intrinsic content size: header band, gap, ring.
    fn content_size(colon_width: f64, use_24_hour: bool) -> Size {
        Size::new(
            Self::header_width(colon_width, use_24_hour).max(DIAL_SIZE),
            FIELD_H + HEADER_DIAL_GAP + DIAL_SIZE,
        )
    }

    fn resolve(size: Size, colon_width: f64, use_24_hour: bool) -> Self {
        let header_w = Self::header_width(colon_width, use_24_hour);
        let mut x = ((size.width - header_w) / 2.0).max(0.0);

        x += FIELD_MARGIN;
        let hour_field = Rect::new(x, 0.0, x + FIELD_W, FIELD_H);
        x += FIELD_W + FIELD_MARGIN;

        let colon_center = Point::new(x + colon_width / 2.0, FIELD_H / 2.0);
        x += colon_width + FIELD_MARGIN;

        let minute_field = Rect::new(x, 0.0, x + FIELD_W, FIELD_H);
        x += FIELD_W + FIELD_MARGIN;

        let period = (!use_24_hour).then(|| {
            let left = x + FIELD_PERIOD_GAP;
            let outline = Rect::new(left, 0.0, left + PERIOD_W, 2.0 * PERIOD_H);
            let am = Rect::new(left, 0.0, left + PERIOD_W, PERIOD_H);
            let pm = Rect::new(left, PERIOD_H, left + PERIOD_W, 2.0 * PERIOD_H);
            (am, pm, outline)
        });

        let ring_left = ((size.width - DIAL_SIZE) / 2.0).max(0.0);
        let ring_top = FIELD_H + HEADER_DIAL_GAP;
        let ring = Rect::new(
            ring_left,
            ring_top,
            ring_left + DIAL_SIZE,
            ring_top + DIAL_SIZE,
        );

        DialLayout {
            hour_field,
            minute_field,
            colon_center,
            period,
            ring,
        }
    }
}

/// The region a press landed in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DialRegion {
    HourField,
    MinuteField,
    Am,
    Pm,
    Ring,
}

// ============================================================================
// Shaped text runs
// ============================================================================

/// The families one layout pass resolves for the dial's runs: the live
/// theme's own for each run's role, or the platform system UI family unthemed.
struct RunFamilies {
    /// The hour/colon/minute header fields — `displayMedium`.
    header: FontFamily,
    /// The AM/PM cells — `titleMedium`.
    period: FontFamily,
    /// The ring labels. `dialLabelFontSize` is not a type-scale role; its
    /// 16px Medium is exactly `titleMedium`'s, so it borrows that role's
    /// family.
    ring: FontFamily,
}

impl RunFamilies {
    fn resolve(theme: Option<&Theme>) -> Self {
        match theme {
            Some(theme) => {
                let scale = &theme.type_scale;
                RunFamilies {
                    header: scale.display_medium.family.clone(),
                    period: scale.title_medium.family.clone(),
                    ring: scale.title_medium.family.clone(),
                }
            }
            None => RunFamilies {
                header: FontFamily::default(),
                period: FontFamily::default(),
                ring: FontFamily::default(),
            },
        }
    }
}

/// A lazily-shaped, paint-time-rebrushed text run — the same idiom
/// `crate::badge`'s `LabelRun` uses, so the ink under the hand can flip
/// without reshaping. Size and weight are the run's token literals; the cache
/// is keyed on the content *and* the family, so a theme swap that changes the
/// family reshapes instead of serving the old face.
struct Run {
    content: String,
    size: f32,
    weight: FontWeight,
    /// The family the cached layout was shaped in.
    family: FontFamily,
    layout: Option<TextLayout>,
}

impl Run {
    fn new(size: f32, weight: FontWeight) -> Self {
        Run {
            content: String::new(),
            size,
            weight,
            family: FontFamily::default(),
            layout: None,
        }
    }

    fn set_content(&mut self, content: &str) {
        if self.content != content {
            self.content = content.to_string();
            self.layout = None;
        }
    }

    /// The style the run shapes with.
    fn style(&self) -> TextStyle {
        TextStyle {
            family: self.family.clone(),
            weight: self.weight,
            ..TextStyle::new(self.size, Color::BLACK)
        }
    }

    /// Shape (or reuse) the run in `family`, returning its measured size.
    fn shape(&mut self, ctx: &mut LayoutCtx, family: &FontFamily) -> Size {
        if self.family != *family {
            self.family = family.clone();
            self.layout = None;
        }
        if let Some(layout) = &self.layout {
            return layout.size();
        }
        let style = self.style();
        let text_ctx = ctx.text_context::<TextContext>();
        let laid = text_ctx.layout(&self.content, &style, None);
        let size = laid.size();
        self.layout = Some(laid);
        size
    }

    /// Paint centred on `center`, in `color`.
    fn paint_centered(
        &self,
        origin: Point,
        center: Point,
        color: Color,
        scene: &mut dyn PaintScene,
    ) {
        let Some(layout) = &self.layout else {
            return;
        };
        let size = layout.size();
        let at = Point::new(
            origin.x + center.x - size.width / 2.0,
            origin.y + center.y - size.height / 2.0,
        );
        for mut run in layout.to_scene_runs(at) {
            run.brush = peniko::Brush::Solid(color);
            scene.draw_glyph_run(run);
        }
    }
}

// ============================================================================
// View / Widget
// ============================================================================

/// A declarative clock dial. See the [module docs](self).
pub struct TimeDialView<State: 'static> {
    value: TimeOfDay,
    use_24_hour: bool,
    strings: Rc<TimePickerStrings>,
    on_change: TypedArgCallback<State, TimeOfDay>,
}

/// Create a controlled clock dial showing `value` and reporting each requested
/// time through `on_change(state, requested)`.
pub fn time_dial<State: 'static, F: Fn(&mut State, TimeOfDay) + 'static>(
    value: TimeOfDay,
    on_change: F,
) -> TimeDialView<State> {
    TimeDialView {
        value,
        use_24_hour: false,
        strings: Rc::new(TimePickerStrings::default()),
        on_change: Rc::new(on_change),
    }
}

impl<State: 'static> TimeDialView<State> {
    /// Show a 24-hour ring and drop the AM/PM selector
    /// (`alwaysUse24HourFormat`). See the [module docs](self)' *24-hour mode*.
    pub fn use_24_hour(mut self, use_24_hour: bool) -> Self {
        self.use_24_hour = use_24_hour;
        self
    }

    /// Override the AM/PM labels and the ring's accessibility announcements.
    pub fn strings(mut self, strings: TimePickerStrings) -> Self {
        self.strings = Rc::new(strings);
        self
    }

    /// Share an already-`Rc`'d string bundle (used by
    /// [`crate::time_picker::time_picker`], which owns one for the whole
    /// dialog).
    pub(super) fn shared_strings(mut self, strings: Rc<TimePickerStrings>) -> Self {
        self.strings = strings;
        self
    }
}

/// The retained widget for a [`TimeDialView`].
pub struct TimeDialWidget {
    value: TimeOfDay,
    use_24_hour: bool,
    strings: Rc<TimePickerStrings>,
    /// Which unit the ring edits — widget-local, the reference's `_mode`.
    mode: TimePickerMode,
    hour_run: Run,
    colon_run: Run,
    minute_run: Run,
    am_run: Run,
    pm_run: Run,
    /// **Both** rings are shaped, and `paint` picks by [`Self::mode`]: a mode
    /// switch happens inside `event`, which has no `request_layout` seam
    /// (only `PaintCtx` has one), so a single mode-swapped run list would
    /// paint stale — or blank — digits until some unrelated pass relaid it
    /// out. Shaping 24 two-glyph runs instead of 12 makes the switch a pure
    /// repaint.
    hour_label_runs: Vec<Run>,
    minute_label_runs: Vec<Run>,
    layout: DialLayout,
    /// The region a live capture started in.
    captured: Option<DialRegion>,
    on_change: ErasedArgCallback<TimeOfDay>,
}

impl TimeDialWidget {
    /// Re-seed every run's content from the current value/format. Every caller
    /// is on a path that also raises `ChangeFlags::LAYOUT`, since a cleared run
    /// can only be re-shaped from `layout`.
    fn sync_runs(&mut self) {
        self.hour_run
            .set_content(&self.value.hour_label(self.use_24_hour));
        self.colon_run.set_content(":");
        self.minute_run.set_content(&self.value.minute_label());
        self.am_run.set_content(&self.strings.am);
        self.pm_run.set_content(&self.strings.pm);
        for (mode, runs) in [
            (TimePickerMode::Hour, &mut self.hour_label_runs),
            (TimePickerMode::Minute, &mut self.minute_label_runs),
        ] {
            let labels = dial_labels(mode, self.use_24_hour);
            if runs.len() != labels.len() {
                *runs = labels
                    .iter()
                    .map(|_| Run::new(DIAL_LABEL_FONT_SIZE, FontWeight::MEDIUM))
                    .collect();
            }
            for (run, label) in runs.iter_mut().zip(labels.iter()) {
                run.set_content(label);
            }
        }
    }

    /// The ring labels for the unit currently being edited.
    fn label_runs(&self) -> &[Run] {
        match self.mode {
            TimePickerMode::Hour => &self.hour_label_runs,
            TimePickerMode::Minute => &self.minute_label_runs,
        }
    }

    fn region_at(&self, position: Point) -> Option<DialRegion> {
        if self.layout.hour_field.contains(position) {
            return Some(DialRegion::HourField);
        }
        if self.layout.minute_field.contains(position) {
            return Some(DialRegion::MinuteField);
        }
        if let Some((am, pm, _)) = self.layout.period {
            if am.contains(position) {
                return Some(DialRegion::Am);
            }
            if pm.contains(position) {
                return Some(DialRegion::Pm);
            }
        }
        if self.layout.ring.contains(position) {
            return Some(DialRegion::Ring);
        }
        None
    }

    /// Map a widget-local pointer position inside the ring to a requested
    /// time. `None` when the input is degenerate.
    fn ring_value(&self, position: Point) -> Option<TimeOfDay> {
        let local = Point::new(
            position.x - self.layout.ring.x0,
            position.y - self.layout.ring.y0,
        );
        dial_value_at(
            local,
            self.layout.ring.size(),
            self.mode,
            self.use_24_hour,
            self.value,
        )
    }

    /// The ring's semantic label for the unit currently being edited.
    fn ring_label(&self) -> &str {
        match self.mode {
            TimePickerMode::Hour => &self.strings.hour_mode_announcement,
            TimePickerMode::Minute => &self.strings.minute_mode_announcement,
        }
    }
}

/// Resolved ink for one paint pass.
struct DialColors {
    dial: Color,
    accent: Color,
    on_accent: Color,
    on_surface: Color,
    field_active_bg: Color,
    field_active_fg: Color,
    field_idle_bg: Color,
    field_idle_fg: Color,
    period_selected_bg: Color,
    period_selected_fg: Color,
    period_idle_fg: Color,
    outline: Color,
}

/// Resolve the dial's ink from `theme` — `M3ETimePickerTheme`'s colour
/// getters (`containerColor`/`fieldBackgroundColor`/`fieldForegroundColor`/
/// `periodOption*Color`) plus `_buildDial`'s scheme reads.
fn resolve_colors(theme: Option<&Theme>) -> DialColors {
    match theme {
        Some(t) => {
            let s = t.scheme();
            DialColors {
                dial: s.surface_container_highest,
                accent: s.primary,
                on_accent: s.on_primary,
                on_surface: s.on_surface,
                field_active_bg: s.primary_container,
                field_active_fg: s.on_primary_container,
                field_idle_bg: s.surface_container_highest,
                field_idle_fg: s.on_surface,
                period_selected_bg: s.tertiary_container,
                period_selected_fg: s.on_tertiary_container,
                period_idle_fg: s.on_surface_variant,
                outline: s.outline,
            }
        }
        None => DialColors {
            dial: FALLBACK_DIAL,
            accent: FALLBACK_ACCENT,
            on_accent: FALLBACK_ON_ACCENT,
            on_surface: FALLBACK_ON_SURFACE,
            field_active_bg: FALLBACK_DIAL,
            field_active_fg: FALLBACK_ON_SURFACE,
            field_idle_bg: FALLBACK_DIAL,
            field_idle_fg: FALLBACK_ON_SURFACE,
            period_selected_bg: FALLBACK_DIAL,
            period_selected_fg: FALLBACK_ON_SURFACE,
            period_idle_fg: FALLBACK_ON_SURFACE,
            outline: FALLBACK_ON_SURFACE,
        },
    }
}

/// The theme's `small` corner token, or the reference's literal 8dp.
fn small_radius(theme: Option<&Theme>) -> f64 {
    theme.map_or(SMALL_RADIUS, |t| t.shape.small)
}

impl<State: 'static> View<State> for TimeDialView<State> {
    type Element = TimeDialWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> TimeDialWidget {
        let mut widget = TimeDialWidget {
            value: self.value,
            use_24_hour: self.use_24_hour,
            strings: self.strings.clone(),
            mode: TimePickerMode::Hour,
            hour_run: Run::new(HEADER_FONT_SIZE, FontWeight::REGULAR),
            colon_run: Run::new(HEADER_FONT_SIZE, FontWeight::REGULAR),
            minute_run: Run::new(HEADER_FONT_SIZE, FontWeight::REGULAR),
            am_run: Run::new(PERIOD_FONT_SIZE, PERIOD_FONT_WEIGHT),
            pm_run: Run::new(PERIOD_FONT_SIZE, PERIOD_FONT_WEIGHT),
            hour_label_runs: Vec::new(),
            minute_label_runs: Vec::new(),
            layout: DialLayout::resolve(
                DialLayout::content_size(0.0, self.use_24_hour),
                0.0,
                self.use_24_hour,
            ),
            captured: None,
            on_change: erase_callback_arg(&self.on_change),
        };
        widget.sync_runs();
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut TimeDialWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_change = erase_callback_arg(&self.on_change);
        let mut flags = ChangeFlags::NONE;
        if prev.value != self.value {
            element.value = self.value;
            // `LAYOUT` as well as `PAINT`: the header's digits are shaped text
            // whose cached run `sync_runs` below clears, and only `layout` can
            // re-shape it (see [`TimeDialWidget::sync_runs`]).
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.use_24_hour != self.use_24_hour {
            element.use_24_hour = self.use_24_hour;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.strings != self.strings {
            element.strings = self.strings.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        element.sync_runs();
        flags
    }

    fn teardown(&self, _element: &mut TimeDialWidget, _ctx: &mut BuildCtx<'_>) {}
}

impl Widget for TimeDialWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let families = RunFamilies::resolve(Theme::from_layout_ctx(ctx));
        self.hour_run.shape(ctx, &families.header);
        let colon = self.colon_run.shape(ctx, &families.header);
        self.minute_run.shape(ctx, &families.header);
        self.am_run.shape(ctx, &families.period);
        self.pm_run.shape(ctx, &families.period);
        for run in self
            .hour_label_runs
            .iter_mut()
            .chain(self.minute_label_runs.iter_mut())
        {
            run.shape(ctx, &families.ring);
        }
        let size = bc.constrain(DialLayout::content_size(colon.width, self.use_24_hour));
        self.layout = DialLayout::resolve(size, colon.width, self.use_24_hour);
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let colors = resolve_colors(theme);
        let radius = small_radius(theme);
        let o = ctx.origin();

        // --- header fields (`_buildField`) ---
        for (rect, run, active) in [
            (
                self.layout.hour_field,
                &self.hour_run,
                self.mode == TimePickerMode::Hour,
            ),
            (
                self.layout.minute_field,
                &self.minute_run,
                self.mode == TimePickerMode::Minute,
            ),
        ] {
            let (bg, fg) = if active {
                (colors.field_active_bg, colors.field_active_fg)
            } else {
                (colors.field_idle_bg, colors.field_idle_fg)
            };
            scene.fill_rounded_rect(
                Point::new(o.x + rect.x0, o.y + rect.y0),
                rect.size(),
                radius,
                bg,
            );
            run.paint_centered(o, rect.center(), fg, scene);
        }
        self.colon_run
            .paint_centered(o, self.layout.colon_center, colors.on_surface, scene);

        // --- AM/PM toggle (`_buildPeriodToggle`) ---
        if let Some((am, pm, outline)) = self.layout.period {
            let is_pm = self.value.is_pm();
            for (rect, run, selected) in [(am, &self.am_run, !is_pm), (pm, &self.pm_run, is_pm)] {
                if selected {
                    scene.fill_rect(
                        Point::new(o.x + rect.x0, o.y + rect.y0),
                        rect.size(),
                        colors.period_selected_bg,
                    );
                }
                let fg = if selected {
                    colors.period_selected_fg
                } else {
                    colors.period_idle_fg
                };
                run.paint_centered(o, rect.center(), fg, scene);
            }
            // The outline is drawn as four hairlines rather than a stroked
            // rounded rect: `PaintScene` has no rounded-rect *stroke*, and the
            // toggle's own fills already carry the corner shape via the clip
            // its container applies.
            let (x0, y0, x1, y1) = (
                o.x + outline.x0,
                o.y + outline.y0,
                o.x + outline.x1,
                o.y + outline.y1,
            );
            for (a, b) in [
                (Point::new(x0, y0), Point::new(x1, y0)),
                (Point::new(x1, y0), Point::new(x1, y1)),
                (Point::new(x1, y1), Point::new(x0, y1)),
                (Point::new(x0, y1), Point::new(x0, y0)),
            ] {
                scene.stroke_line(a, b, PERIOD_BORDER_WIDTH, colors.outline);
            }
        }

        // --- ring (`M3ETimeDialPainter.paint`) ---
        let ring = self.layout.ring;
        let center = Point::new(o.x + ring.center().x, o.y + ring.center().y);
        let dial_radius = ring.width().min(ring.height()) / 2.0;
        let ring_radius = (dial_radius - DIAL_KNOB_RADIUS - DIAL_RING_INSET).max(0.0);
        scene.fill_rounded_rect(
            Point::new(o.x + ring.x0, o.y + ring.y0),
            ring.size(),
            dial_radius,
            colors.dial,
        );

        let selected = selected_index(self.value, self.mode, self.use_24_hour);
        let angle = slot_angle(selected);
        let knob = Point::new(
            center.x + angle.cos() * ring_radius,
            center.y + angle.sin() * ring_radius,
        );
        scene.stroke_line(center, knob, DIAL_HAND_WIDTH, colors.accent);
        scene.fill_rounded_rect(
            Point::new(center.x - DIAL_CENTER_RADIUS, center.y - DIAL_CENTER_RADIUS),
            Size::new(2.0 * DIAL_CENTER_RADIUS, 2.0 * DIAL_CENTER_RADIUS),
            DIAL_CENTER_RADIUS,
            colors.accent,
        );
        scene.fill_rounded_rect(
            Point::new(knob.x - DIAL_KNOB_RADIUS, knob.y - DIAL_KNOB_RADIUS),
            Size::new(2.0 * DIAL_KNOB_RADIUS, 2.0 * DIAL_KNOB_RADIUS),
            DIAL_KNOB_RADIUS,
            colors.accent,
        );

        // Labels last, so the selected one reads over the knob in `onPrimary`
        // (`_paintLabel`'s contrast flip).
        for (i, run) in self.label_runs().iter().enumerate() {
            let a = slot_angle(i);
            let at = Point::new(
                ring.center().x + a.cos() * ring_radius,
                ring.center().y + a.sin() * ring_radius,
            );
            let ink = if i == selected {
                colors.on_accent
            } else {
                colors.on_surface
            };
            run.paint_centered(o, at, ink, scene);
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
                let Some(region) = self.region_at(p.position) else {
                    return EventResult::Ignored;
                };
                self.captured = Some(region);
                ctx.capture_pointer();
                match region {
                    DialRegion::HourField => self.mode = TimePickerMode::Hour,
                    DialRegion::MinuteField => self.mode = TimePickerMode::Minute,
                    DialRegion::Am | DialRegion::Pm => {
                        let requested = self.value.with_period(region == DialRegion::Pm);
                        (self.on_change)(ctx, requested);
                    }
                    DialRegion::Ring => {
                        // `onTapDown`: haptic first, then the tap's effect.
                        MaterialHaptics::fire(HapticSignal::SliderTick);
                        if let Some(requested) = self.ring_value(p.position) {
                            (self.on_change)(ctx, requested);
                        }
                    }
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if self.captured != Some(DialRegion::Ring) {
                    // A header/period press tracks no drag (the reference wires
                    // `onPanUpdate` to the ring alone).
                    return if self.captured.is_some() {
                        EventResult::Handled
                    } else {
                        EventResult::Ignored
                    };
                }
                if let Some(requested) = self.ring_value(p.position) {
                    (self.on_change)(ctx, requested);
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up | PointerPhase::Cancel => {
                if self.captured.is_none() {
                    return EventResult::Ignored;
                }
                self.captured = None;
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let hour_label = format!("{} {}", self.strings.hour_label, self.value.hour());
        let minute_label = format!("{} {}", self.strings.minute_label, self.value.minute());
        let hour_active = self.mode == TimePickerMode::Hour;
        ctx.push_container(
            Role::Group,
            |_| {},
            |ctx| {
                ctx.push_node(Role::Button, |node| {
                    node.set_label(hour_label);
                    node.set_selected(hour_active);
                });
                ctx.push_node(Role::Button, |node| {
                    node.set_label(minute_label);
                    node.set_selected(!hour_active);
                });
                if !self.use_24_hour {
                    let is_pm = self.value.is_pm();
                    ctx.push_node(Role::RadioButton, |node| {
                        node.set_label(self.strings.am.clone());
                        node.set_selected(!is_pm);
                    });
                    ctx.push_node(Role::RadioButton, |node| {
                        node.set_label(self.strings.pm.clone());
                        node.set_selected(is_pm);
                    });
                }
                ctx.push_node(Role::Slider, |node| {
                    node.set_label(self.ring_label().to_string());
                    node.set_numeric_value(f64::from(match self.mode {
                        TimePickerMode::Hour => self.value.hour(),
                        TimePickerMode::Minute => self.value.minute(),
                    }));
                });
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::{PointerButton, PointerEvent};
    use std::any::Any;

    /// A pointer position on the ring at the given clock `slot` (0 = 12
    /// o'clock, clockwise), in ring-local coordinates.
    fn at_slot(slot: usize, radius: f64) -> Point {
        let a = slot_angle(slot);
        Point::new(
            DIAL_SIZE / 2.0 + a.cos() * radius,
            DIAL_SIZE / 2.0 + a.sin() * radius,
        )
    }

    const RING: Size = Size::new(DIAL_SIZE, DIAL_SIZE);

    #[test]
    fn cardinal_points_map_to_the_quarter_fractions() {
        let r = 100.0;
        for (slot, expected) in [(0usize, 0.0), (3, 0.25), (6, 0.5), (9, 0.75)] {
            let f = dial_fraction(at_slot(slot, r), RING).expect("finite");
            assert!(
                (f - expected).abs() < 1e-9,
                "slot {slot} mapped to {f}, expected {expected}"
            );
        }
    }

    #[test]
    fn off_axis_points_map_to_their_own_slot_fraction() {
        let r = 90.0;
        for slot in 0..DIAL_SLOTS {
            let f = dial_fraction(at_slot(slot, r), RING).expect("finite");
            let expected = slot as f64 / DIAL_SLOTS as f64;
            assert!(
                (f - expected).abs() < 1e-9,
                "slot {slot} mapped to {f}, expected {expected}"
            );
        }
    }

    #[test]
    fn twelve_hour_ring_snaps_to_the_twelve_positions() {
        let noon = TimeOfDay::new(12, 30);
        let r = 100.0;
        for slot in 0..DIAL_SLOTS {
            let got = dial_value_at(at_slot(slot, r), RING, TimePickerMode::Hour, false, noon)
                .expect("finite");
            // Slot 0 is the "12" label; the AM/PM half is carried over from
            // the current value (PM here), so slot 0 stays 12:00 PM.
            let expected_12 = if slot == 0 { 12 } else { slot as u8 };
            assert_eq!(
                got.hour_of_12(),
                expected_12,
                "slot {slot} should select the {expected_12} label"
            );
            assert!(got.is_pm(), "the period half is carried over, not reset");
            assert_eq!(got.minute(), 30, "the minute half is untouched");
        }
    }

    #[test]
    fn hour_ring_is_even_labels_with_all_24_reachable() {
        // The 24-hour ring draws 12 even-hour labels ...
        assert_eq!(
            dial_labels(TimePickerMode::Hour, true),
            vec![
                "00", "02", "04", "06", "08", "10", "12", "14", "16", "18", "20", "22"
            ]
        );
        // ... but every one of the 24 hours is reachable by angle.
        let mut seen = std::collections::BTreeSet::new();
        for step in 0..24 {
            let fraction = step as f64 / 24.0;
            seen.insert(hour_from_fraction_24(fraction));
        }
        assert_eq!(seen.len(), 24, "all 24 hours reachable in 24-hour mode");

        // And the hand parks on the nearer even label for an odd hour.
        assert_eq!(
            selected_index(TimeOfDay::new(5, 0), TimePickerMode::Hour, true),
            3,
            "05:00 parks on the `06` label (slot 3)"
        );
        assert_eq!(
            selected_index(TimeOfDay::new(4, 0), TimePickerMode::Hour, true),
            2,
            "04:00 parks on its own `04` label"
        );
    }

    #[test]
    fn twenty_four_hour_ring_maps_each_slot_to_its_even_hour() {
        let base = TimeOfDay::new(0, 15);
        for slot in 0..DIAL_SLOTS {
            let got = dial_value_at(at_slot(slot, 100.0), RING, TimePickerMode::Hour, true, base)
                .expect("finite");
            assert_eq!(got.hour(), (slot * 2) as u8, "slot {slot}");
            assert_eq!(got.minute(), 15, "the minute half is untouched");
        }
    }

    #[test]
    fn minutes_snap_to_five() {
        // Every fraction inside a 5-minute slot resolves to that slot's floor.
        for raw in 0..60u8 {
            let fraction = f64::from(raw) / 60.0;
            assert_eq!(
                minute_from_fraction(fraction),
                (raw / 5) * 5,
                "minute {raw} should snap down to its 5-minute slot"
            );
        }
        // The wrap-around case: a fraction that rounds to 60 folds to 0.
        assert_eq!(minute_from_fraction(0.999), 0);
        let got = dial_value_at(
            at_slot(1, 100.0),
            RING,
            TimePickerMode::Minute,
            false,
            TimeOfDay::new(9, 0),
        )
        .expect("finite");
        assert_eq!(got.minute(), 5);
        assert_eq!(got.hour(), 9, "the hour half is untouched");
    }

    #[test]
    fn a_centre_press_resolves_to_three_o_clock_without_panicking() {
        let centre = Point::new(DIAL_SIZE / 2.0, DIAL_SIZE / 2.0);
        let f = dial_fraction(centre, RING).expect("a centre press is not degenerate");
        assert!(
            (f - 0.25).abs() < 1e-9,
            "`atan2(0, 0) == 0` puts the centre at the quarter turn, as upstream"
        );
        let got = dial_value_at(
            centre,
            RING,
            TimePickerMode::Hour,
            false,
            TimeOfDay::new(0, 0),
        )
        .expect("resolved");
        assert_eq!(got.hour_of_12(), 3);
    }

    #[test]
    fn non_finite_input_is_refused_rather_than_propagated() {
        for bad in [
            Point::new(f64::NAN, 10.0),
            Point::new(10.0, f64::NAN),
            Point::new(f64::INFINITY, 0.0),
            Point::new(0.0, f64::NEG_INFINITY),
        ] {
            assert!(
                dial_fraction(bad, RING).is_none(),
                "{bad:?} must be refused"
            );
            assert!(
                dial_value_at(
                    bad,
                    RING,
                    TimePickerMode::Minute,
                    false,
                    TimeOfDay::new(1, 1)
                )
                .is_none()
            );
        }
        // A degenerate dial box is refused too.
        for bad_box in [
            Size::new(0.0, 0.0),
            Size::new(f64::NAN, 10.0),
            Size::new(10.0, f64::INFINITY),
            Size::new(-5.0, 10.0),
        ] {
            assert!(dial_fraction(Point::new(1.0, 1.0), bad_box).is_none());
        }
    }

    #[test]
    fn the_snap_helpers_never_panic_on_a_non_finite_fraction() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(hour_from_fraction_24(bad), 0);
            assert_eq!(hour_slot_from_fraction(bad), 0);
            assert_eq!(minute_from_fraction(bad), 0);
        }
        // A wildly out-of-range fraction wraps instead of overflowing.
        assert_eq!(hour_from_fraction_24(-0.5), 12);
        assert_eq!(minute_from_fraction(-0.5), 30);
    }

    // ------------------------------------------------------------------
    // Widget-level behaviour
    // ------------------------------------------------------------------

    #[derive(Default)]
    struct App {
        value: TimeOfDay,
        changes: u32,
    }

    /// Build the dial widget directly (the harness `crate::switch`'s own tests
    /// use) and lay it out against a text context, so the resolved geometry is
    /// available to hit-test against.
    fn mount(value: TimeOfDay, use_24_hour: bool) -> (TimeDialWidget, TextContext) {
        let view = time_dial::<App, _>(value, |s: &mut App, t: TimeOfDay| {
            s.value = t;
            s.changes += 1;
        })
        .use_24_hour(use_24_hour);
        let mut counter = 0u64;
        let mut widget = View::<App>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut tcx = TextContext::new();
        {
            let mut layout_ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
            widget.layout(
                &mut layout_ctx,
                &BoxConstraints::loose(Size::new(400.0, 800.0)),
            );
        }
        (widget, tcx)
    }

    // ---- typeface: each run's family follows its role on the live theme ----

    /// A baseline theme whose `displayMedium`/`titleMedium` roles name
    /// `header`/`title`.
    fn theme_with(header: &str, title: &str) -> Theme {
        let mut theme = crate::baseline();
        let scale = &mut theme.type_scale;
        scale.display_medium.family = FontFamily::named(header);
        scale.title_medium.family = FontFamily::named(title);
        theme
    }

    /// Re-lay `widget` out against `tcx`, threading `theme` the way the render
    /// root does.
    fn layout_themed(widget: &mut TimeDialWidget, tcx: &mut TextContext, theme: Option<&Theme>) {
        let mut ctx =
            LayoutCtx::with_resources(Some(tcx as &mut dyn Any), theme.map(|t| t as &dyn Any));
        widget.layout(&mut ctx, &BoxConstraints::loose(Size::new(400.0, 800.0)));
    }

    /// Every ring-label run, both rings.
    fn ring_runs(widget: &TimeDialWidget) -> impl Iterator<Item = &Run> {
        widget
            .hour_label_runs
            .iter()
            .chain(widget.minute_label_runs.iter())
    }

    #[test]
    fn layout_takes_each_run_s_family_from_its_role() {
        let (mut widget, mut tcx) = mount(TimeOfDay::new(10, 30), false);
        let mut theme = theme_with("Header Probe", "Title Probe");
        // A decoy on a role the dial must not read: the ring labels borrow
        // `titleMedium` (the role whose 16px Medium they match), not this one.
        theme.type_scale.label_small.family = FontFamily::named("Label Small Decoy");
        layout_themed(&mut widget, &mut tcx, Some(&theme));
        let header = FontFamily::named("Header Probe");
        for run in [&widget.hour_run, &widget.colon_run, &widget.minute_run] {
            assert_eq!(run.family, header, "displayMedium");
            assert_eq!(run.size, HEADER_FONT_SIZE, "the token size stays");
        }
        let title = FontFamily::named("Title Probe");
        for run in [&widget.am_run, &widget.pm_run] {
            assert_eq!(run.family, title, "titleMedium");
        }
        assert_eq!(ring_runs(&widget).count(), 2 * DIAL_SLOTS);
        for run in ring_runs(&widget) {
            assert_eq!(run.family, title, "the ring labels borrow titleMedium");
            assert_eq!(run.size, DIAL_LABEL_FONT_SIZE, "the literal size stays");
        }
    }

    #[test]
    fn without_a_theme_the_runs_keep_the_unthemed_styles() {
        let (widget, _tcx) = mount(TimeOfDay::new(10, 30), false);
        let header = TextStyle {
            weight: FontWeight::REGULAR,
            ..TextStyle::new(HEADER_FONT_SIZE, Color::BLACK)
        };
        assert_eq!(widget.hour_run.style(), header);
        assert_eq!(widget.colon_run.style(), header);
        assert_eq!(widget.minute_run.style(), header);
        assert_eq!(
            widget.am_run.style(),
            TextStyle {
                weight: PERIOD_FONT_WEIGHT,
                ..TextStyle::new(PERIOD_FONT_SIZE, Color::BLACK)
            }
        );
        for run in ring_runs(&widget) {
            assert_eq!(
                run.style(),
                TextStyle {
                    weight: FontWeight::MEDIUM,
                    ..TextStyle::new(DIAL_LABEL_FONT_SIZE, Color::BLACK)
                }
            );
        }
    }

    #[test]
    fn a_theme_swap_reshapes_the_cached_runs() {
        let (mut widget, mut tcx) = mount(TimeOfDay::new(10, 30), false);
        let first = theme_with("Swap A", "Swap A");
        layout_themed(&mut widget, &mut tcx, Some(&first));

        // Control: the same theme again reuses every cached run outright.
        let settled = tcx.shape_cache_stats();
        layout_themed(&mut widget, &mut tcx, Some(&first));
        assert_eq!(
            tcx.shape_cache_stats(),
            settled,
            "an unchanged theme reshapes nothing"
        );

        let second = theme_with("Swap B", "Swap B");
        layout_themed(&mut widget, &mut tcx, Some(&second));
        assert!(
            tcx.shape_cache_stats().shapes > settled.shapes,
            "a family swap must reshape the dial's runs, not serve the old face"
        );
        assert_eq!(widget.hour_run.family, FontFamily::named("Swap B"));
        assert_eq!(widget.am_run.family, FontFamily::named("Swap B"));
    }

    fn ev(phase: PointerPhase, at: Point, button: PointerButton) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: at,
            button,
        })
    }

    fn dispatch(widget: &mut TimeDialWidget, state: &mut App, event: &InputEvent) {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(400.0, 400.0));
        widget.event(&mut ctx, event);
    }

    fn press(widget: &mut TimeDialWidget, state: &mut App, at: Point) {
        dispatch(
            widget,
            state,
            &ev(PointerPhase::Down, at, PointerButton::Primary),
        );
        dispatch(
            widget,
            state,
            &ev(PointerPhase::Up, at, PointerButton::Primary),
        );
    }

    /// A widget-local point on the ring at clock `slot`.
    fn ring_point(widget: &TimeDialWidget, slot: usize, radius: f64) -> Point {
        let local = at_slot(slot, radius);
        Point::new(
            widget.layout.ring.x0 + local.x,
            widget.layout.ring.y0 + local.y,
        )
    }

    #[test]
    fn a_ring_press_reports_the_requested_time_and_never_self_mutates() {
        let (mut widget, _tcx) = mount(TimeOfDay::new(10, 30), false);
        let mut state = App::default();
        let at = ring_point(&widget, 6, 100.0);
        press(&mut widget, &mut state, at);
        assert_eq!(state.changes, 1, "one change reported");
        // Slot 6 on the 12-hour ring is the "6" label; 10:30 is AM, so 06:30.
        assert_eq!(state.value, TimeOfDay::new(6, 30));
        assert_eq!(
            widget.value,
            TimeOfDay::new(10, 30),
            "the dial never rewrites its own value"
        );
    }

    #[test]
    fn a_drag_reports_every_position_it_crosses() {
        let (mut widget, _tcx) = mount(TimeOfDay::new(10, 30), false);
        let mut state = App::default();
        let points: Vec<Point> = (0..4)
            .map(|slot| ring_point(&widget, slot, 100.0))
            .collect();
        dispatch(
            &mut widget,
            &mut state,
            &ev(PointerPhase::Down, points[0], PointerButton::Primary),
        );
        for at in &points[1..] {
            dispatch(
                &mut widget,
                &mut state,
                &ev(PointerPhase::Move, *at, PointerButton::Primary),
            );
        }
        dispatch(
            &mut widget,
            &mut state,
            &ev(PointerPhase::Up, points[3], PointerButton::Primary),
        );
        assert_eq!(state.changes, 4, "one report per down/move, none on the up");
        assert_eq!(state.value.hour_of_12(), 3, "the last position wins");
    }

    #[test]
    fn a_move_with_no_press_reports_nothing() {
        let (mut widget, _tcx) = mount(TimeOfDay::new(10, 30), false);
        let mut state = App::default();
        let at = ring_point(&widget, 6, 100.0);
        dispatch(
            &mut widget,
            &mut state,
            &ev(PointerPhase::Move, at, PointerButton::Primary),
        );
        assert_eq!(state.changes, 0);
    }

    #[test]
    fn committing_an_hour_does_not_auto_advance_the_mode() {
        let (mut widget, _tcx) = mount(TimeOfDay::new(10, 30), false);
        let mut state = App::default();
        let at = ring_point(&widget, 6, 100.0);
        press(&mut widget, &mut state, at);
        assert_eq!(
            widget.mode,
            TimePickerMode::Hour,
            "the reference's `_handleDial` never writes `_mode`"
        );
    }

    #[test]
    fn tapping_the_minute_field_switches_the_ring_to_minutes() {
        let (mut widget, _tcx) = mount(TimeOfDay::new(10, 30), false);
        let mut state = App::default();
        let at = widget.layout.minute_field.center();
        press(&mut widget, &mut state, at);
        assert_eq!(widget.mode, TimePickerMode::Minute);
        assert_eq!(state.changes, 0, "a field tap changes no value");

        // ... and the ring now snaps minutes, not hours.
        let at = ring_point(&widget, 1, 100.0);
        press(&mut widget, &mut state, at);
        assert_eq!(state.value, TimeOfDay::new(10, 5));

        let at = widget.layout.hour_field.center();
        press(&mut widget, &mut state, at);
        assert_eq!(widget.mode, TimePickerMode::Hour, "and back again");
    }

    #[test]
    fn the_period_cells_flip_am_pm_without_moving_the_clock_face() {
        let (mut widget, _tcx) = mount(TimeOfDay::new(10, 30), false);
        let mut state = App::default();
        let (am, pm, _) = widget.layout.period.expect("12-hour mode shows the toggle");
        press(&mut widget, &mut state, pm.center());
        assert_eq!(state.value, TimeOfDay::new(22, 30), "10:30 AM -> 10:30 PM");

        let (mut widget, _tcx) = mount(TimeOfDay::new(22, 30), false);
        let (am2, _, _) = widget.layout.period.expect("toggle");
        assert_eq!(am, am2, "the toggle sits in the same place either way");
        press(&mut widget, &mut state, am2.center());
        assert_eq!(state.value, TimeOfDay::new(10, 30), "and back again");
    }

    #[test]
    fn a_secondary_press_starts_nothing() {
        let (mut widget, _tcx) = mount(TimeOfDay::new(10, 30), false);
        let mut state = App::default();
        let at = ring_point(&widget, 6, 100.0);
        dispatch(
            &mut widget,
            &mut state,
            &ev(PointerPhase::Down, at, PointerButton::Secondary),
        );
        assert_eq!(state.changes, 0);
        assert!(widget.captured.is_none(), "and no capture to wedge on");
    }

    #[test]
    fn a_press_outside_every_region_is_ignored() {
        let (mut widget, _tcx) = mount(TimeOfDay::new(10, 30), false);
        let mut state = App::default();
        press(&mut widget, &mut state, Point::new(2.0, 2.0));
        assert_eq!(state.changes, 0);
    }

    #[test]
    fn a_cancel_disarms_the_drag() {
        let (mut widget, _tcx) = mount(TimeOfDay::new(10, 30), false);
        let mut state = App::default();
        let at = ring_point(&widget, 6, 100.0);
        dispatch(
            &mut widget,
            &mut state,
            &ev(PointerPhase::Down, at, PointerButton::Primary),
        );
        assert!(widget.captured.is_some());
        dispatch(
            &mut widget,
            &mut state,
            &ev(PointerPhase::Cancel, at, PointerButton::Primary),
        );
        assert!(widget.captured.is_none());
        let before = state.changes;
        let elsewhere = ring_point(&widget, 9, 100.0);
        dispatch(
            &mut widget,
            &mut state,
            &ev(PointerPhase::Move, elsewhere, PointerButton::Primary),
        );
        assert_eq!(state.changes, before, "a disarmed drag reports nothing");
    }

    #[test]
    fn the_twenty_four_hour_dial_drops_the_period_toggle() {
        let (widget, _tcx) = mount(TimeOfDay::new(18, 45), true);
        assert!(widget.layout.period.is_none());
        assert_eq!(widget.hour_label_runs.len(), DIAL_SLOTS);
        assert_eq!(widget.hour_label_runs[1].content, "02");
        assert_eq!(widget.minute_label_runs[1].content, "05");
    }

    /// A mode switch happens inside `event`, which cannot ask for a layout
    /// pass, so both rings must already be shaped when the switch lands — see
    /// [`TimeDialWidget::hour_label_runs`].
    #[test]
    fn switching_mode_needs_no_reshaping() {
        let (mut widget, _tcx) = mount(TimeOfDay::new(10, 30), false);
        let mut state = App::default();
        assert!(
            widget.label_runs().iter().all(|r| r.layout.is_some()),
            "the hour ring is shaped after layout"
        );
        let at = widget.layout.minute_field.center();
        press(&mut widget, &mut state, at);
        assert_eq!(widget.mode, TimePickerMode::Minute);
        assert!(
            widget.label_runs().iter().all(|r| r.layout.is_some()),
            "and so is the minute ring, with no layout pass in between"
        );
        assert_eq!(widget.label_runs()[1].content, "05");
    }

    #[test]
    fn a_value_change_asks_for_a_layout_pass_so_the_header_digits_reshape() {
        let a = time_dial::<App, _>(TimeOfDay::new(10, 30), |_: &mut App, _| {});
        let b = time_dial::<App, _>(TimeOfDay::new(11, 30), |_: &mut App, _| {});
        let mut counter = 0u64;
        let mut widget = View::<App>::build(&a, &mut BuildCtx::new(&mut counter));
        let flags = View::<App>::rebuild(&b, &a, &mut widget, &mut BuildCtx::new(&mut counter));
        assert!(
            flags.contains(ChangeFlags::LAYOUT),
            "a cleared shaped run can only be re-shaped from `layout`"
        );
    }

    #[test]
    fn the_dial_body_is_the_reference_s_three_hundred_and_sixty() {
        let (widget, _tcx) = mount(TimeOfDay::new(10, 30), false);
        // 80 header + 24 gap + 256 ring — `dialDialogBodyHeight`.
        assert_eq!(widget.layout.ring.y1, 360.0);
        assert_eq!(widget.layout.ring.width(), DIAL_SIZE);
    }

    #[test]
    fn semantics_reports_the_fields_the_period_and_the_ring() {
        fn logic(state: &mut App) -> TimeDialView<App> {
            time_dial(state.value, |s: &mut App, t: TimeOfDay| s.value = t)
        }
        let mut root: frust_core::RenderRoot<App, TimeDialView<App>> =
            frust_core::RenderRoot::new();
        let mut state = App {
            value: TimeOfDay::new(10, 30),
            changes: 0,
        };
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(400.0, 800.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();

        let buttons: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::Button)
            .collect();
        assert_eq!(buttons.len(), 2, "an hour field and a minute field");
        assert_eq!(buttons[0].1.is_selected(), Some(true), "hour is active");
        let radios: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::RadioButton)
            .collect();
        assert_eq!(radios.len(), 2, "AM and PM");
        assert!(
            update
                .nodes
                .iter()
                .any(|(_, n)| n.role() == Role::Slider && n.label() == Some("Select hours"))
        );
    }
}
