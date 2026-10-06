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
//! # `Wrap` becomes a plain `Row`, its linear pair flex-shared (G10)
//!
//! The "All styles" preview lays its four indicators out in a horizontal
//! [`frust::FlexView`] rather than the reference's `Wrap(spacing: 16,
//! runSpacing: 16)` — this workspace has no flow/wrap layout primitive (see
//! `crate::pages::playground::pick::chips`' module docs for the same
//! divergence taken elsewhere). Unlike `chips`' all-inflexible `spaced_row`,
//! [`all_styles_row`] gives the *two linear indicators* a [`frust::flexible`]
//! share of the row's remaining width rather than pinning each to a fixed
//! pixel [`SizedBox`]: a `Row` with every child inflexible reports a
//! `BoxConstraints`-clamped size but never shrinks its children to fit it
//! (`frust_widgets::flex`'s own contract — an all-inflexible flex
//! shrink-wraps to the *sum* of its children's natural extents, clamping only
//! its own reported size, not their laid-out positions). The two 160px-wide
//! linear `SizedBox`es this preview used to carry summed past a phone-ish
//! card's inner width alongside the two circular indicators, and the
//! trailing (rightmost, in draw order) `linear_wavy_progress` painted
//! straight through the card's right edge with nothing to clip it — the
//! wavy variant was the visible symptom only because it drew last, not
//! because its own painter overdrew its box (host-side probe: laying the
//! real row out under a phone-ish `BoxConstraints` and paint-recording it
//! confirmed the row's own children, not `frust_material`'s progress
//! painters, exceeded the available width). Splitting the row's remaining
//! width between the two linear indicators via `flexible` makes the row's
//! own reported width — and therefore every child's real extent — bounded by
//! the incoming constraint unconditionally, the same guarantee
//! [`crate::widgets::playground::play_preview_card`]'s Stretch/Align fix
//! (G4) relies on one level up.
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

use frust::{
    AnyView, Component, CrossAxisAlignment, SizedBox, View, any, component, inflexible, row, stack,
};
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

/// Fixed width the reference wraps its primary-preview linear indicator in
/// (`SizedBox(width: 220)`). The "All styles" preview's own pair of linear
/// indicators no longer takes a fixed width — see [`all_styles_row`] and the
/// module docs' `Wrap` divergence (G10).
const PRIMARY_LINEAR_WIDTH: f64 = 220.0;

/// Gap between adjacent indicators in [`all_styles_row`], in logical px —
/// the reference's `Wrap(spacing: 16, ...)`.
const ALL_STYLES_GAP: f64 = 16.0;

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
/// `_buildIndicator`. `linear_width` sizes the two linear variants for the
/// primary preview, see [`PRIMARY_LINEAR_WIDTH`] (the "All styles" preview's
/// own linear pair is sized by [`all_styles_row`] instead).
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

/// The "All styles" preview: all four constructors side by side at `value`.
/// The two circular indicators are `inflexible` (they size from their own
/// diameter); the two linear ones are [`flexible`], splitting the row's
/// remaining width between them rather than each claiming a fixed pixel
/// width — see the module docs' `Wrap` divergence (G10) for why an
/// all-inflexible row (this file's precedent before the fix, and still
/// `crate::pages::playground::pick::chips`' `spaced_row` shape) is unsafe
/// here specifically.
fn all_styles_row(value: ProgressValue) -> AnyView<Knobs> {
    let gap = || inflexible(SizedBox::<Knobs>(Some(ALL_STYLES_GAP), None));
    any(row()
        .child(circular_progress(value))
        .push(gap())
        .child(circular_wavy_progress(value))
        .push(gap())
        .flex(1, linear_progress(value))
        .push(gap())
        .flex(1, linear_wavy_progress(value))
        .cross_axis(CrossAxisAlignment::Center))
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
    let all_styles_preview = play_preview_card("All styles", all_styles_row(value));
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

    any(stack().child(content).child(panel))
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

    fn build(&self, state: &mut Knobs) -> impl View<Knobs> {
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

    /// G10 regression: [`all_styles_row`] must never paint past the "All
    /// styles" card's real inner width at a phone-ish device width. This is
    /// a genuine layout+paint probe, not an eyeball check — `frust::authoring`
    /// is the sanctioned route for an app to build/layout/paint a real widget
    /// host-side (see `docs/CODE_STANDARDS.md`'s State & Reactivity
    /// Conventions), so this needs no `frust-core`/`frust-widgets` dependency
    /// of its own.
    ///
    /// Reverting the fix (back to a `spaced_row` of two fixed-`SizedBox`
    /// 160px-wide linear indicators, see the module docs) fails this test:
    /// the four indicators' combined natural width (~456px plus three 16px
    /// gaps) is caught overflowing a phone-ish card's ~296px inner width by a
    /// wide margin, with the trailing `linear_wavy_progress` the one to cross
    /// the boundary — the exact shape G10 reported.
    #[test]
    fn all_styles_row_never_paints_past_the_cards_inner_width() {
        use frust::authoring::{
            BoxConstraints, BuildCtx, LayoutCtx, PaintCtx, PaintScene, View, Widget,
        };
        use frust::kurbo::{BezPath, PathEl, Point, Size};
        use frust::peniko::{Brush, Color};

        /// A representative phone-ish device width (Android's common
        /// 360dp baseline). The card's real inner width nets out this
        /// device width against the same two padding layers the real page
        /// applies: `playground_body`'s outer `EdgeInsets { left, right:
        /// MaterialSpacing::LG, .. }` and `play_preview_card`'s own
        /// `EdgeInsets::all(MaterialSpacing::LG)` — four `LG` insets total.
        const DEVICE_WIDTH: f64 = 360.0;
        let card_inner_width = DEVICE_WIDTH - 4.0 * frust_material::MaterialSpacing::LG;

        /// Records the furthest-right x any paint call reaches, in the
        /// widget's own local coordinate space (painted at [`Point::ZERO`]
        /// below, so this doubles as the painted extent from the card's
        /// left edge).
        #[derive(Default)]
        struct MaxXScene {
            max_x: f64,
        }
        impl PaintScene for MaxXScene {
            fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
            fn draw_text(&mut self, _origin: Point, _text: &str) {}
            fn fill_rounded_rect(
                &mut self,
                origin: Point,
                size: Size,
                _radius: f64,
                _color: Color,
            ) {
                self.max_x = self.max_x.max(origin.x + size.width);
            }
            fn stroke_line(&mut self, p0: Point, p1: Point, _width: f64, _color: Color) {
                self.max_x = self.max_x.max(p0.x).max(p1.x);
            }
            fn stroke_path(&mut self, _origin: Point, path: &BezPath, _width: f64, _brush: &Brush) {
                for el in path.elements() {
                    if let PathEl::MoveTo(p) | PathEl::LineTo(p) = el {
                        self.max_x = self.max_x.max(p.x);
                    }
                }
            }
        }

        let view = all_styles_row(ProgressValue::Determinate(0.6));
        let mut counter = 0u64;
        let mut widget = view.build(&mut BuildCtx::new(&mut counter));
        let bc = BoxConstraints::new(Size::ZERO, Size::new(card_inner_width, 100.0));
        let size = widget.layout(&mut LayoutCtx::new(), &bc);
        assert!(
            size.width <= card_inner_width + 1e-6,
            "the row's own reported size {size:?} must not exceed the card's \
             inner width {card_inner_width}"
        );

        let mut ctx = PaintCtx::new(Point::ZERO, size);
        let mut scene = MaxXScene::default();
        widget.paint(&mut ctx, &mut scene);
        assert!(
            scene.max_x <= card_inner_width + 1e-6,
            "G10 regressed: painted x {} exceeds the card's inner width {} \
             (row reported size {size:?})",
            scene.max_x,
            card_inner_width
        );
    }
}
