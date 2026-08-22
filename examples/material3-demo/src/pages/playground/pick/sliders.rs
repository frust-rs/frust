//! Sliders: the reference's `SlidersPlayground`.
//!
//! One preview whose control swaps across six of the crate's eight slider
//! constructors — [`slider`], [`wavy_slider`], [`centered_slider`],
//! [`slider`] again with `SliderView::divisions` for the discrete case,
//! [`vertical_slider`], and [`range_slider`] — the same six the reference's
//! `_SliderKind` enum names (`wavy_centered_slider`/`vertical_centered_slider`
//! round the crate's ported set out to all six named constructors, but this
//! playground's own reference never reaches either).
//!
//! Knobs live in a page-local [`Knobs`] (never [`AppState`]), per the page
//! contract in [`crate::pages::playground`]. `Kind` uses the two-piece
//! [`play_enum_menu_field`]/[`play_enum_menu_panel`] control (its own
//! [`OverlayAnchor`] held in `Knobs`, the panel mounted at this page's outer
//! [`Stack`], the `do_::buttons` precedent); `Icon position` — shown only
//! while `Kind` is `Vertical`, mirroring the reference's own conditional
//! control — fits a plain [`play_enum_segmented`] since it is a 2-way choice.
//!
//! The centered and discrete previews re-derive their displayed value from
//! the shared `0.0..=1.0` knob the same way the reference's `_value` does
//! (`(_value * 200) - 100` / `(_value * 5).roundToDouble()`), so every kind
//! shares one continuous knob rather than a value per constructor.

use frust::{AnyView, Component, SizedBox, Stack, any, component};
use frust_material::{
    HapticSignal, OverlayAnchor, SliderIconPosition, SliderRange, centered_slider, icons,
    range_slider, slider, vertical_slider, wavy_slider,
};

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{
    PlaySnippet, control_panel, play_enum_menu_field, play_enum_menu_panel, play_enum_segmented,
    play_preview_card, play_slider, play_snippet, play_switch, playground_body,
};

/// Which constructor the preview/snippet builds — the reference's
/// `_SliderKind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SliderKind {
    Continuous,
    Wavy,
    Centered,
    Discrete,
    Vertical,
    Range,
}

/// Every [`SliderKind`] the "Kind" menu offers, the reference's own
/// `_SliderKind.values` order.
const KINDS: [SliderKind; 6] = [
    SliderKind::Continuous,
    SliderKind::Wavy,
    SliderKind::Centered,
    SliderKind::Discrete,
    SliderKind::Vertical,
    SliderKind::Range,
];

/// Kind label for the menu/snippet — the reference's `_SliderKind.name`.
fn kind_label(kind: SliderKind) -> &'static str {
    match kind {
        SliderKind::Continuous => "continuous",
        SliderKind::Wavy => "wavy",
        SliderKind::Centered => "centered",
        SliderKind::Discrete => "discrete",
        SliderKind::Vertical => "vertical",
        SliderKind::Range => "range",
    }
}

/// Every [`SliderIconPosition`] the "Icon position" segmented control offers.
const ICON_POSITIONS: [SliderIconPosition; 2] =
    [SliderIconPosition::Start, SliderIconPosition::End];

/// Icon-position label — the reference's `M3ESliderIconPosition.name`.
fn icon_position_label(position: SliderIconPosition) -> &'static str {
    match position {
        SliderIconPosition::Start => "start",
        SliderIconPosition::End => "end",
    }
}

/// This page's own knob state — held by [`SlidersPlayground`], never
/// [`AppState`] (the page contract in [`crate::pages::playground`]).
struct Knobs {
    kind: SliderKind,
    /// The one continuous `0.0..=1.0` knob every kind's preview re-derives
    /// its own value from — the reference's own `_value` seed (`0.45`).
    value: f64,
    range: SliderRange,
    enabled: bool,
    track_thickness: f64,
    icon_position: SliderIconPosition,
    /// Shared with [`kind_menu_panel`] — the "Kind" dropdown's anchor.
    kind_anchor: OverlayAnchor,
    kind_open: bool,
}

impl Default for Knobs {
    /// The reference's own `_SlidersPlaygroundState` field initializers.
    fn default() -> Self {
        Self {
            kind: SliderKind::Continuous,
            value: 0.45,
            range: SliderRange::new(0.2, 0.7),
            enabled: true,
            track_thickness: 16.0,
            icon_position: SliderIconPosition::End,
            kind_anchor: OverlayAnchor::new(),
            kind_open: false,
        }
    }
}

/// The reference's `_num`: an integral value prints bare, a fractional one
/// keeps its decimal.
fn num_text(value: f64) -> String {
    if value == value.round() {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

/// The centered preview's displayed value — the reference's
/// `(_value * 200) - 100`, mapping the shared `0.0..=1.0` knob onto
/// `-100.0..=100.0`.
fn centered_value(value: f64) -> f64 {
    value * 200.0 - 100.0
}

/// The discrete preview's displayed value — the reference's
/// `(_value * 5).roundToDouble()`, mapping the shared knob onto `0.0..=5.0`
/// in whole steps.
fn discrete_value(value: f64) -> f64 {
    (value * 5.0).round()
}

/// The control this page's "Slider" preview shows for the current `Kind`.
fn preview_control(state: &Knobs) -> AnyView<Knobs> {
    match state.kind {
        SliderKind::Continuous => any(slider(state.value, |s: &mut Knobs, v: f64| s.value = v)
            .enabled(state.enabled)
            .track_thickness(state.track_thickness)),
        SliderKind::Wavy => any(
            wavy_slider(state.value, |s: &mut Knobs, v: f64| s.value = v)
                .enabled(state.enabled)
                .track_thickness(state.track_thickness),
        ),
        SliderKind::Centered => any(centered_slider(
            centered_value(state.value),
            |s: &mut Knobs, v: f64| {
                s.value = (v + 100.0) / 200.0;
            },
        )
        .range(-100.0, 100.0)
        .enabled(state.enabled)
        .track_thickness(state.track_thickness)),
        SliderKind::Discrete => any(slider(
            discrete_value(state.value),
            |s: &mut Knobs, v: f64| {
                s.value = v / 5.0;
            },
        )
        .range(0.0, 5.0)
        .divisions(5)
        .enabled(state.enabled)
        .track_thickness(state.track_thickness)
        .haptic(HapticSignal::Light)),
        SliderKind::Vertical => {
            any(
                vertical_slider(state.value, |s: &mut Knobs, v: f64| s.value = v)
                    .enabled(state.enabled)
                    .track_thickness(state.track_thickness)
                    .thumb_length(80.0)
                    .icon(icons::VOLUME_UP)
                    .icon_position(state.icon_position),
            )
        }
        SliderKind::Range => {
            any(
                range_slider(state.range, |s: &mut Knobs, v: SliderRange| s.range = v)
                    .enabled(state.enabled)
                    .track_thickness(state.track_thickness),
            )
        }
    }
}

/// The "Slider" preview card's content: the current control, boxed to the
/// reference's own dimensions per kind (`SizedBox(height: 180, width: 80)`
/// vertical, `SizedBox(width: 280)` otherwise).
fn preview(state: &Knobs) -> AnyView<Knobs> {
    let control = preview_control(state);
    if state.kind == SliderKind::Vertical {
        any(SizedBox::<Knobs>(Some(80.0), Some(180.0)).child(control))
    } else {
        any(SizedBox::<Knobs>(Some(280.0), None).child(control))
    }
}

/// The paste-ready snippet for the current knob state — the reference's
/// `_snippets`, in Frust rather than Dart.
fn snippet(state: &Knobs) -> PlaySnippet {
    let thickness = num_text(state.track_thickness);
    let enabled = state.enabled;
    let code = match state.kind {
        SliderKind::Continuous => format!(
            "slider({value}, on_changed)\n    .enabled({enabled})\n    .track_thickness({thickness});",
            value = num_text(state.value)
        ),
        SliderKind::Wavy => format!(
            "wavy_slider({value}, on_changed)\n    .enabled({enabled})\n    .track_thickness({thickness});",
            value = num_text(state.value)
        ),
        SliderKind::Centered => format!(
            "centered_slider({value}, on_changed)\n    .range(-100.0, 100.0)\n    .enabled({enabled})\n    .track_thickness({thickness});",
            value = num_text(centered_value(state.value))
        ),
        SliderKind::Discrete => format!(
            "slider({value}, on_changed)\n    .range(0.0, 5.0)\n    .divisions(5)\n    .enabled({enabled})\n    .track_thickness({thickness})\n    .haptic(HapticSignal::Light);",
            value = num_text(discrete_value(state.value))
        ),
        SliderKind::Vertical => format!(
            "vertical_slider({value}, on_changed)\n    .enabled({enabled})\n    .track_thickness({thickness})\n    .thumb_length(80.0)\n    .icon(icons::VOLUME_UP)\n    .icon_position(SliderIconPosition::{icon_position:?});",
            value = num_text(state.value),
            icon_position = state.icon_position,
        ),
        SliderKind::Range => format!(
            "range_slider(SliderRange::new({start}, {end}), on_changed)\n    .enabled({enabled})\n    .track_thickness({thickness});",
            start = num_text(state.range.start),
            end = num_text(state.range.end)
        ),
    };
    play_snippet("Slider", code)
}

/// "Appearance" controls: kind, track thickness, icon position (vertical
/// only), enabled.
fn controls_panel(state: &Knobs) -> AnyView<Knobs> {
    let mut children: Vec<AnyView<Knobs>> = vec![
        play_enum_menu_field(
            "Kind",
            state.kind,
            &KINDS,
            kind_label,
            &state.kind_anchor,
            state.kind_open,
            |s: &mut Knobs, open: bool| s.kind_open = open,
        ),
        play_slider(
            "Track thickness",
            state.track_thickness,
            8.0..=40.0,
            Some(16),
            |s: &mut Knobs, v: f64| s.track_thickness = v,
        ),
    ];
    if state.kind == SliderKind::Vertical {
        children.push(play_enum_segmented(
            "Icon position",
            state.icon_position,
            &ICON_POSITIONS,
            icon_position_label,
            |s: &mut Knobs, next: SliderIconPosition| s.icon_position = next,
        ));
    }
    children.push(play_switch(
        "Enabled",
        state.enabled,
        |s: &mut Knobs, v: bool| s.enabled = v,
    ));
    control_panel("Appearance", children)
}

/// The "Kind" menu's popup half — mounted at this page's outer [`Stack`].
fn kind_menu_panel(state: &Knobs) -> AnyView<Knobs> {
    play_enum_menu_panel(
        state.kind,
        &KINDS,
        kind_label,
        &state.kind_anchor,
        state.kind_open,
        |s: &mut Knobs, open: bool| s.kind_open = open,
        |s: &mut Knobs, next: SliderKind| s.kind = next,
    )
}

/// The page body: the playground content plus the "Kind" dropdown panel it
/// anchors, stacked so both can paint above the scrollable content.
fn body(state: &Knobs) -> AnyView<Knobs> {
    let content = playground_body(
        vec![play_preview_card("Slider", preview(state))],
        vec![snippet(state)],
        vec![controls_panel(state)],
    );
    any(Stack(vec![content, kind_menu_panel(state)]))
}

/// This page's knob component — see the [module docs](self).
struct SlidersPlayground;

impl Component for SlidersPlayground {
    type State = Knobs;

    fn init(&self) -> Self::State {
        Knobs::default()
    }

    fn build(&self, state: &mut Self::State) -> AnyView<Self::State> {
        body(state)
    }
}

/// See the page contract in [`crate::pages::playground`]. The reference's
/// `SlidersPlayground` shows no page-level heading of its own, like every
/// other real playground in this catalog, so `entry` goes unused here.
pub fn page(_entry: DemoEntry) -> AnyView<AppState> {
    any(component(SlidersPlayground))
}

#[cfg(test)]
mod tests {
    use super::{ICON_POSITIONS, KINDS, Knobs, SliderKind, body, snippet};

    #[test]
    fn the_page_builds_across_every_reachable_control_state() {
        let mut knobs = Knobs::default();
        for kind in KINDS {
            knobs.kind = kind;
            for enabled in [true, false] {
                knobs.enabled = enabled;
                for thickness in [8.0, 16.0, 40.0] {
                    knobs.track_thickness = thickness;
                    let _view = body(&knobs);
                    let _snippet = snippet(&knobs);
                }
            }
        }
        knobs.kind = SliderKind::Vertical;
        for position in ICON_POSITIONS {
            knobs.icon_position = position;
            let _view = body(&knobs);
        }
        knobs.kind_open = true;
        let _view = body(&knobs);
    }

    #[test]
    fn the_page_fn_builds_from_its_catalog_entry() {
        let entry = crate::catalog::find_by_id("sliders").expect("catalog entry exists");
        let _view = super::page(entry);
    }
}
