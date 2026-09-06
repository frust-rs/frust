//! Badges: the reference's `BadgesPlayground`.
//!
//! `frust_material::badge` wraps a child view with an optional dot/numeric
//! indicator — a straight port, since [`frust_material::BadgeView`]'s builder
//! surface already covers every prop the reference's `_BadgesPlaygroundState`
//! drives (`showDot`, `count`, `maxCount`, `alignment`). `showDot`/`count` are
//! exclusive builder calls here (`.dot()` vs `.count(n)`) rather than the
//! reference's two nullable constructor args, so [`body`]/[`snippet_code`]
//! branch on the knob pair instead of formatting a `null` literal.
//!
//! # `Wrap` becomes a plain `Row`
//!
//! This workspace has no flow/wrap layout primitive; the single "Badge"
//! preview lays its three badges out in a non-wrapping [`spaced_row`] (the
//! same idiom `crate::pages::playground::pick::chips`'s "All types" preview
//! uses) rather than the reference's `Wrap(spacing: 24, runSpacing: 16)`.

use frust::{AnyView, Component, CrossAxisAlignment, Row, SizedBox, any, component, icon};
use frust_material::{BadgeAlignment, badge, icons};

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{
    control_panel, play_enum_segmented, play_preview_card, play_slider, play_snippet, play_switch,
    playground_body,
};

/// This playground's knobs — the reference's `_BadgesPlaygroundState`. `count`/
/// `max_count` stay `f64` (the sliders' own currency, per
/// `crate::pages::playground::view::dividers`' identical precedent) and are
/// rounded to `u32` wherever [`frust_material::badge`] needs one.
struct Knobs {
    show_dot: bool,
    show_count: bool,
    count: f64,
    max_count: f64,
    alignment: BadgeAlignment,
}

impl Default for Knobs {
    fn default() -> Self {
        Self {
            show_dot: false,
            show_count: true,
            count: 8.0,
            max_count: 99.0,
            alignment: BadgeAlignment::TopRight,
        }
    }
}

/// Every [`BadgeAlignment`], for the segmented control.
const ALIGNMENTS: [BadgeAlignment; 3] = [
    BadgeAlignment::TopLeft,
    BadgeAlignment::TopCenter,
    BadgeAlignment::TopRight,
];

/// [`BadgeAlignment`]'s display label — the reference's
/// `labelOf` switch (`Left`/`Center`/`Right`).
fn alignment_label(alignment: BadgeAlignment) -> &'static str {
    match alignment {
        BadgeAlignment::TopLeft => "Left",
        BadgeAlignment::TopCenter => "Center",
        BadgeAlignment::TopRight => "Right",
    }
}

/// The FRUST snippet text for the current knob state — the reference's
/// `_snippets` getter, ported to real `frust_material` code rather than a
/// Dart string.
fn snippet_code(state: &Knobs) -> String {
    let count = state.count.round() as u32;
    let max_count = state.max_count.round() as u32;
    let indicator = if state.show_dot {
        "\n    .dot()".to_string()
    } else if state.show_count {
        format!("\n    .count({count})")
    } else {
        String::new()
    };
    format!(
        "badge(icon(icons::NOTIFICATIONS).size(28.0)){indicator}\n    .max_count({max_count})\n    .alignment(BadgeAlignment::{:?});",
        state.alignment
    )
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

/// The playground body for the current knob state.
fn body(state: &mut Knobs) -> AnyView<Knobs> {
    let count = state.count.round() as u32;
    let max_count = state.max_count.round() as u32;

    let mut primary = badge(icon(icons::NOTIFICATIONS).size(28.0))
        .max_count(max_count)
        .alignment(state.alignment);
    primary = if state.show_dot {
        primary.dot()
    } else if state.show_count {
        primary.count(count)
    } else {
        primary
    };
    let dot_only = badge(icon(icons::MENU).size(28.0)).dot();
    let overflowing = badge(icon(icons::EDIT).size(28.0))
        .count(120)
        .max_count(max_count);

    let preview = play_preview_card(
        "Badge",
        spaced_row(vec![any(primary), any(dot_only), any(overflowing)], 24.0),
    );
    let snippet = play_snippet("Badge", snippet_code(state));

    let controls = control_panel(
        "Primary badge",
        vec![
            play_switch("Show dot", state.show_dot, |s: &mut Knobs, v| {
                s.show_dot = v
            }),
            play_switch("Show count", state.show_count, |s: &mut Knobs, v| {
                s.show_count = v
            }),
            play_slider(
                "Count",
                state.count,
                0.0..=150.0,
                Some(150),
                |s: &mut Knobs, v| s.count = v,
            ),
            play_slider(
                "Max count",
                state.max_count,
                9.0..=99.0,
                Some(90),
                |s: &mut Knobs, v| s.max_count = v,
            ),
            play_enum_segmented(
                "Alignment",
                state.alignment,
                &ALIGNMENTS,
                alignment_label,
                |s: &mut Knobs, v: BadgeAlignment| s.alignment = v,
            ),
        ],
    );

    playground_body(vec![preview], vec![snippet], vec![controls])
}

/// The nested [`Component`] this page owns its knobs in. See the page
/// contract in [`crate::pages::playground`].
#[derive(Default)]
struct BadgesPlayground;

impl Component for BadgesPlayground {
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
    any(component(BadgesPlayground))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every state this file's own controls can reach still builds a body —
    /// the dot/count/none tri-state, the count/max-count sliders, and every
    /// alignment.
    #[test]
    fn the_page_builds_across_every_reachable_control_state() {
        let mut knobs = Knobs::default();
        for show_dot in [false, true] {
            knobs.show_dot = show_dot;
            let _view = body(&mut knobs);
        }
        knobs.show_dot = false;
        for show_count in [false, true] {
            knobs.show_count = show_count;
            let _view = body(&mut knobs);
        }
        knobs.show_count = true;
        for count in [0.0, 8.0, 150.0] {
            knobs.count = count;
            let _view = body(&mut knobs);
        }
        for max_count in [9.0, 99.0] {
            knobs.max_count = max_count;
            let _view = body(&mut knobs);
        }
        for alignment in ALIGNMENTS {
            knobs.alignment = alignment;
            let _view = body(&mut knobs);
        }
    }

    #[test]
    fn the_snippet_switches_between_dot_count_and_neither() {
        let mut knobs = Knobs::default();
        assert!(snippet_code(&knobs).contains(".count(8)"));
        knobs.show_dot = true;
        assert!(snippet_code(&knobs).contains(".dot()"));
        assert!(!snippet_code(&knobs).contains(".count("));
        knobs.show_dot = false;
        knobs.show_count = false;
        let code = snippet_code(&knobs);
        assert!(!code.contains(".dot()"));
        assert!(!code.contains(".count("));
    }
}
