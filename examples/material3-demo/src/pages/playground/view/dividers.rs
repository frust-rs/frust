//! Dividers: the reference's `DividersPlayground`.

use frust::{
    Align, Alignment, AnyView, Column, Component, CrossAxisAlignment, Row, SizedBox, Theme, any,
    component, text,
};
use frust_material::divider;

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{
    PlaySnippet, ambient_theme, control_panel, play_enum_segmented, play_preview_card, play_slider,
    play_snippet, playground_body,
};

/// The axis this playground's divider is drawn on — the reference's
/// `M3EDividerAxis`. `frust_material::divider` has no enum of its own (a
/// plain `vertical()` builder flag), so this page defines its own knob type.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DividerAxis {
    Horizontal,
    Vertical,
}

impl DividerAxis {
    const ALL: [DividerAxis; 2] = [DividerAxis::Horizontal, DividerAxis::Vertical];

    fn label(self) -> &'static str {
        match self {
            DividerAxis::Horizontal => "Horizontal",
            DividerAxis::Vertical => "Vertical",
        }
    }
}

/// This page's knob state.
struct Knobs {
    axis: DividerAxis,
    thickness: f64,
    indent: f64,
    end_indent: f64,
}

struct DividersPlayground;

impl Component for DividersPlayground {
    type State = Knobs;

    fn init(&self) -> Knobs {
        Knobs {
            axis: DividerAxis::Horizontal,
            thickness: 1.0,
            indent: 0.0,
            end_indent: 0.0,
        }
    }

    fn build(&self, state: &mut Knobs) -> AnyView<Knobs> {
        body(state)
    }
}

/// See the page contract in [`crate::pages::playground`].
pub fn page(_entry: DemoEntry) -> AnyView<AppState> {
    any(component(DividersPlayground))
}

/// The playground body — everything that varies with `state`; the same
/// function [`DividersPlayground::build`] calls, and this file's tests
/// exercise directly across every knob state.
fn body(state: &Knobs) -> AnyView<Knobs> {
    let theme = ambient_theme();
    playground_body(
        vec![preview(&theme, state)],
        vec![snippet(state)],
        vec![controls(state)],
    )
}

fn preview(theme: &Theme, state: &Knobs) -> AnyView<Knobs> {
    let mut body_style = theme.type_scale.body_large.clone();
    body_style.color = theme.scheme().on_surface;

    let mut line = divider()
        .thickness(state.thickness)
        .indent(state.indent)
        .end_indent(state.end_indent);
    if state.axis == DividerAxis::Vertical {
        line = line.vertical();
    }

    let content = if state.axis == DividerAxis::Vertical {
        // A vertical divider needs a bounded height to stretch into — the
        // reference's own `SizedBox(height: 64)` wrapping the row.
        any(SizedBox::<Knobs>(None, Some(64.0)).child(
            Row(vec![
                any(Align(
                    Alignment::new(-1.0, 0.0),
                    text("Left").style(body_style.clone()),
                )),
                any(SizedBox::<Knobs>(Some(12.0), None)),
                any(line),
                any(SizedBox::<Knobs>(Some(12.0), None)),
                any(Align(
                    Alignment::new(-1.0, 0.0),
                    text("Right").style(body_style),
                )),
            ])
            .cross_axis(CrossAxisAlignment::Stretch),
        ))
    } else {
        any(Column(vec![
            any(text("Above").style(body_style.clone())),
            any(SizedBox::<Knobs>(None, Some(12.0))),
            any(line),
            any(SizedBox::<Knobs>(None, Some(12.0))),
            any(text("Below").style(body_style)),
        ])
        .cross_axis(CrossAxisAlignment::Stretch))
    };

    any(play_preview_card("Divider", content))
}

fn controls(state: &Knobs) -> AnyView<Knobs> {
    any(control_panel::<Knobs>(
        "Appearance",
        vec![
            play_enum_segmented::<Knobs, DividerAxis>(
                "Axis",
                state.axis,
                &DividerAxis::ALL,
                DividerAxis::label,
                |state: &mut Knobs, next: DividerAxis| state.axis = next,
            ),
            play_slider::<Knobs>(
                "Thickness",
                state.thickness,
                1.0..=8.0,
                Some(7),
                |state: &mut Knobs, next: f64| state.thickness = next,
            ),
            play_slider::<Knobs>(
                "Indent",
                state.indent,
                0.0..=48.0,
                Some(12),
                |state: &mut Knobs, next: f64| state.indent = next,
            ),
            play_slider::<Knobs>(
                "End indent",
                state.end_indent,
                0.0..=48.0,
                Some(12),
                |state: &mut Knobs, next: f64| state.end_indent = next,
            ),
        ],
    ))
}

fn snippet(state: &Knobs) -> PlaySnippet {
    let mut code = String::from("frust_material::divider()");
    code.push_str(&format!("\n    .thickness({:?})", state.thickness));
    code.push_str(&format!("\n    .indent({:?})", state.indent));
    code.push_str(&format!("\n    .end_indent({:?})", state.end_indent));
    if state.axis == DividerAxis::Vertical {
        code.push_str("\n    .vertical()");
    }
    code.push(';');
    play_snippet("Divider", code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_body_builds_across_every_knob_state() {
        for axis in DividerAxis::ALL {
            let state = Knobs {
                axis,
                thickness: 1.0,
                indent: 0.0,
                end_indent: 0.0,
            };
            let _view = body(&state);
        }
        let state = Knobs {
            axis: DividerAxis::Horizontal,
            thickness: 8.0,
            indent: 48.0,
            end_indent: 48.0,
        };
        let _view = body(&state);
    }
}
