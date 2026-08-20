//! Progress indicators: the reference's `ProgressPlayground`.
//!
//! One `M3EProgressIndicator` widget with four Dart named constructors maps
//! onto four sibling `frust_material` functions
//! ([`frust_material::circular_progress`]/[`circular_wavy_progress`]/
//! [`linear_progress`]/[`linear_wavy_progress`]) — this page's local
//! [`ProgressKind`] enum mirrors the reference's own `_ProgressKind`,
//! dispatching through [`build_indicator`] rather than inventing a unified
//! constructor this catalog has no seam for (the same shape
//! `crate::pages::playground::pick::chips`' `ChipKind` takes over four chip
//! constructors, and `crate::pages::playground::view::dividers`' local
//! `DividerAxis` takes over one builder flag).
//!
//! # Type picker: `play_enum_menu_field`/`play_enum_menu_panel`
//!
//! The reference's `PlayEnumMenu<_ProgressKind>` is this kit's two-piece
//! [`crate::widgets::playground::play_enum_menu_field`]/[`play_enum_menu_panel`]
//! pair — the field mounts in the control panel's row list, the panel mounts
//! at this page's own outer [`frust::Stack`], both anchored through
//! [`Knobs::menu_anchor`], exactly the pattern the kit's own module docs
//! prescribe (see `crate::pages::playground::pick::chips`' identical
//! precedent).
//!
//! # `Wrap` becomes a plain `Row`
//!
//! The "All styles" preview lays its four indicators out in a non-wrapping
//! [`spaced_row`] rather than the reference's `Wrap(spacing: 16, runSpacing:
//! 16)` — this workspace has no flow/wrap layout primitive (see
//! `crate::pages::playground::pick::chips`' module docs for the same
//! divergence).
//!
//! # Linear size is inert on the wavy variant, upstream-faithfully
//!
//! [`frust_material::LinearProgressView::size`] is documented inert on the
//! wavy variant (it sizes from its own stroke/amplitude); the reference
//! itself never passes `linearSize` to `M3EProgressIndicator.linearWavy`
//! either (`progress_playground.dart`'s own `_buildIndicator`), so
//! [`build_indicator`]/[`snippet_code`] only apply [`ProgressKind::Linear`]'s
//! size knob to the flat constructor, matching upstream exactly rather than
//! calling a documented-inert builder for symmetry.

use frust::{AnyView, Component, CrossAxisAlignment, Row, SizedBox, Stack, any, component};
use frust_material::{
    OverlayAnchor, ProgressSize, ProgressValue, circular_progress, circular_wavy_progress,
    linear_progress, linear_wavy_progress,
};

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{
    control_panel, play_enum_menu_field, play_enum_menu_panel, play_enum_segmented,
    play_preview_card, play_slider, play_snippet, play_switch, playground_body,
};

/// The reference's `_ProgressKind`, over this catalog's four sibling
/// constructors.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ProgressKind {
    Circular,
    CircularWavy,
    Linear,
    LinearWavy,
}

impl ProgressKind {
    const ALL: [ProgressKind; 4] = [
        ProgressKind::Circular,
        ProgressKind::CircularWavy,
        ProgressKind::Linear,
        ProgressKind::LinearWavy,
    ];

    /// The reference's `kind.name` — also this file's `frust_material`
    /// constructor-name stem.
    fn label(self) -> &'static str {
        match self {
            ProgressKind::Circular => "circular",
            ProgressKind::CircularWavy => "circularWavy",
            ProgressKind::Linear => "linear",
            ProgressKind::LinearWavy => "linearWavy",
        }
    }

    /// Whether this kind is one of the two linear constructors — gates the
    /// "Linear size" control and the primary preview's fixed-width wrapper,
    /// mirroring the reference's own `isLinear` local.
    fn is_linear(self) -> bool {
        matches!(self, ProgressKind::Linear | ProgressKind::LinearWavy)
    }
}

/// [`ProgressSize`]'s display label — the reference's `v.name` (`s`/`m`).
fn linear_size_label(size: ProgressSize) -> &'static str {
    match size {
        ProgressSize::S => "s",
        ProgressSize::M => "m",
    }
}

/// Every [`ProgressSize`], for the segmented control.
const LINEAR_SIZES: [ProgressSize; 2] = [ProgressSize::S, ProgressSize::M];

/// Fixed width the reference wraps its linear previews in (`SizedBox(width:
/// 220)`/`SizedBox(width: 160)` for the primary/"All styles" previews
/// respectively).
const PRIMARY_LINEAR_WIDTH: f64 = 220.0;
const ALL_STYLES_LINEAR_WIDTH: f64 = 160.0;

/// This playground's knobs — the reference's `_ProgressPlaygroundState`.
/// [`Knobs::menu_anchor`] is the one non-signal, shared field (see the
/// module docs' Type picker section).
struct Knobs {
    kind: ProgressKind,
    determinate: bool,
    value: f64,
    linear_size: ProgressSize,
    menu_open: bool,
    menu_anchor: OverlayAnchor,
}

impl Default for Knobs {
    fn default() -> Self {
        Self {
            kind: ProgressKind::Linear,
            determinate: true,
            value: 0.6,
            linear_size: ProgressSize::M,
            menu_open: false,
            menu_anchor: OverlayAnchor::new(),
        }
    }
}

/// The controlled [`ProgressValue`] the current knob state drives — the
/// reference's `_progress` getter.
fn progress_value(state: &Knobs) -> ProgressValue {
    if state.determinate {
        ProgressValue::Determinate(state.value)
    } else {
        ProgressValue::Indeterminate
    }
}

/// Build the indicator [`Knobs::kind`] selects, at `value` — the reference's
/// `_buildIndicator`. `linear_width` sizes the two linear variants (the
/// primary preview and the "All styles" preview use different fixed widths,
/// see [`PRIMARY_LINEAR_WIDTH`]/[`ALL_STYLES_LINEAR_WIDTH`]).
fn build_indicator(
    kind: ProgressKind,
    value: ProgressValue,
    linear_size: ProgressSize,
    linear_width: f64,
) -> AnyView<Knobs> {
    match kind {
        ProgressKind::Circular => any(circular_progress(value)),
        ProgressKind::CircularWavy => any(circular_wavy_progress(value)),
        ProgressKind::Linear => any(SizedBox::<Knobs>(Some(linear_width), None)
            .child(linear_progress(value).size(linear_size))),
        ProgressKind::LinearWavy => {
            any(SizedBox::<Knobs>(Some(linear_width), None).child(linear_wavy_progress(value)))
        }
    }
}

/// The FRUST snippet text for the current knob state — the reference's
/// `_snippets` getter, ported to real `frust_material` code rather than a
/// Dart string.
fn snippet_code(state: &Knobs) -> String {
    let value = if state.determinate {
        format!("ProgressValue::Determinate({:.2})", state.value)
    } else {
        "ProgressValue::Indeterminate".to_string()
    };
    let ctor = match state.kind {
        ProgressKind::Circular => "circular_progress",
        ProgressKind::CircularWavy => "circular_wavy_progress",
        ProgressKind::Linear => "linear_progress",
        ProgressKind::LinearWavy => "linear_wavy_progress",
    };
    let size_line = if state.kind == ProgressKind::Linear {
        format!("\n    .size(ProgressSize::{:?})", state.linear_size)
    } else {
        String::new()
    };
    format!("{ctor}({value}){size_line};")
}

/// Lay `items` out horizontally with `gap`px between each pair — see the
/// module docs' `Wrap` divergence.
fn spaced_row<State: 'static>(items: Vec<AnyView<State>>, gap: f64) -> AnyView<State> {
    let mut children: Vec<AnyView<State>> = Vec::with_capacity(items.len() * 2);
    for item in items {
        if !children.is_empty() {
            children.push(any(SizedBox::<State>(Some(gap), None)));
        }
        children.push(item);
    }
    any(Row(children).cross_axis(CrossAxisAlignment::Center))
}

/// The playground body for the current knob state: [`playground_body`]'s
/// scrollable content, plus the type picker's anchored panel at the outer
/// `Stack` — see the module docs.
fn body(state: &mut Knobs) -> AnyView<Knobs> {
    let value = progress_value(state);
    let is_linear = state.kind.is_linear();

    let primary_preview = play_preview_card(
        ProgressKind::label(state.kind),
        build_indicator(state.kind, value, state.linear_size, PRIMARY_LINEAR_WIDTH),
    );
    let all_styles_preview = play_preview_card(
        "All styles",
        spaced_row(
            vec![
                any(circular_progress(value)),
                any(circular_wavy_progress(value)),
                any(SizedBox::<Knobs>(Some(ALL_STYLES_LINEAR_WIDTH), None)
                    .child(linear_progress(value))),
                any(SizedBox::<Knobs>(Some(ALL_STYLES_LINEAR_WIDTH), None)
                    .child(linear_wavy_progress(value))),
            ],
            16.0,
        ),
    );
    let snippet = play_snippet(ProgressKind::label(state.kind), snippet_code(state));

    let kind_field = play_enum_menu_field::<Knobs, ProgressKind>(
        "Kind",
        state.kind,
        &ProgressKind::ALL,
        ProgressKind::label,
        &state.menu_anchor,
        state.menu_open,
        |s: &mut Knobs, open: bool| s.menu_open = open,
    );

    let mut rows: Vec<AnyView<Knobs>> = vec![kind_field];
    if is_linear {
        rows.push(play_enum_segmented(
            "Linear size",
            state.linear_size,
            &LINEAR_SIZES,
            linear_size_label,
            |s: &mut Knobs, v: ProgressSize| s.linear_size = v,
        ));
    }
    rows.push(play_switch(
        "Determinate",
        state.determinate,
        |s: &mut Knobs, v| s.determinate = v,
    ));
    if state.determinate {
        rows.push(play_slider(
            "Value",
            state.value,
            0.0..=1.0,
            None,
            |s: &mut Knobs, v| s.value = v,
        ));
    }
    let controls = control_panel("Appearance", rows);

    let content = playground_body(
        vec![primary_preview, all_styles_preview],
        vec![snippet],
        vec![controls],
    );

    let panel = play_enum_menu_panel::<Knobs, ProgressKind>(
        state.kind,
        &ProgressKind::ALL,
        ProgressKind::label,
        &state.menu_anchor,
        state.menu_open,
        |s: &mut Knobs, open: bool| s.menu_open = open,
        |s: &mut Knobs, next: ProgressKind| s.kind = next,
    );

    any(Stack(vec![content, panel]))
}

/// The nested [`Component`] this page owns its knobs in. See the page
/// contract in [`crate::pages::playground`].
#[derive(Default)]
struct ProgressPlayground;

impl Component for ProgressPlayground {
    type State = Knobs;

    fn init(&self) -> Knobs {
        Knobs::default()
    }

    fn build(&self, state: &mut Knobs) -> AnyView<Knobs> {
        body(state)
    }
}

/// See the page contract in [`crate::pages::playground`].
pub fn page(_entry: DemoEntry) -> AnyView<AppState> {
    any(component(ProgressPlayground))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every state this file's own controls can reach still builds a body,
    /// including the type picker's field+panel pair sharing one
    /// `frust_material::OverlayAnchor`.
    #[test]
    fn the_page_builds_across_every_reachable_control_state() {
        let mut knobs = Knobs::default();
        for kind in ProgressKind::ALL {
            knobs.kind = kind;
            for determinate in [true, false] {
                knobs.determinate = determinate;
                let _view = body(&mut knobs);
            }
            knobs.determinate = true;
            for size in LINEAR_SIZES {
                knobs.linear_size = size;
                let _view = body(&mut knobs);
            }
            for menu_open in [true, false] {
                knobs.menu_open = menu_open;
                let _view = body(&mut knobs);
            }
            knobs.menu_open = false;
        }
        for value in [0.0, 0.6, 1.0] {
            knobs.value = value;
            let _view = body(&mut knobs);
        }
    }

    #[test]
    fn the_snippet_reflects_determinate_value_and_kind() {
        let mut knobs = Knobs::default();
        assert!(snippet_code(&knobs).starts_with("linear_progress"));
        assert!(snippet_code(&knobs).contains("Determinate(0.60)"));
        assert!(snippet_code(&knobs).contains(".size(ProgressSize::M)"));

        knobs.determinate = false;
        assert!(snippet_code(&knobs).contains("Indeterminate"));

        knobs.kind = ProgressKind::LinearWavy;
        assert!(!snippet_code(&knobs).contains(".size("));

        knobs.kind = ProgressKind::CircularWavy;
        assert!(snippet_code(&knobs).starts_with("circular_wavy_progress"));
    }
}
