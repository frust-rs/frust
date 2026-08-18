//! [`radio`]/[`RadioView`]: a labelled, ring-and-dot radio button — the
//! Glyph selection control `frust::authoring`'s baseline `Radio` cannot fill
//! for a themed catalog: baseline's `RadioView` renders its label at
//! `TextStyle::default()` (no family/size/color the active `Theme` can ever
//! reach), so an app whose row wants a design-authored typeface and ink has
//! nothing to build on. This widget closes that gap: a single self-contained
//! [`View`]/[`Widget`] pair painting its own ring, dot and label glyph run,
//! resolving color from `Theme` exactly like every other Glyph widget.
//!
//! # No direct Glyph mockup source, and what that means here
//!
//! Unlike [`crate::toggle`] or [`crate::badge`], neither
//! `glyph-design-system.html` nor its light sibling authors a `.radio*` rule
//! — the vendored Glyph mockups (retrieved 2026-07-21) have no radio at all.
//! The nearest primary reference is the consumer this widget exists to
//! obsolete: `apps/muxr-app`'s Connect screen hand-composes a ring + dot +
//! label (`features/connect/presentation/widgets/trust.rs`'s `radio_dot`/
//! `trust_row`) and wraps it in a bolt-on accessibility node
//! (`chrome::radio_semantics`) because nothing in this catalog could carry a
//! `Role::RadioButton` with a themed label on its own — its own docs name the
//! deletion condition verbatim: "delete it the day `frust_glyph` grows a
//! real radio with a themeable label." This widget's geometry
//! ([`RING`]/[`RING_BORDER`]/[`DOT_INSET`]/[`GAP`]) is transcribed from that
//! row's own constants (`RADIO_RING`/`RADIO_RING_BORDER`/`RADIO_DOT_INSET`/
//! `RADIO_GAP`, itself sourced from muxr's `docs/splash.html` mock's
//! `.radio-dot{border:1.5px}` rule) precisely so a future swap-in matches the
//! row it replaces pixel-for-pixel, not because a Glyph design-system source
//! pins these numbers.
//!
//! # Label typography: Glyph-authored, theme-inked
//!
//! Following [`crate::list`]/[`crate::badge`]'s convention for a widget that
//! shapes its own label rather than nesting a `Text` child: family
//! (IBM Plex Mono — the UI face every Glyph body/label role uses, see
//! `crate::tokens::scales`), weight and size are fixed Glyph-authored
//! constants ([`LABEL_SIZE`], matching `crate::list`'s `TITLE_SIZE`'s 13px
//! floor), never theme-resolved; only the *ink* is — `on_surface`, themed,
//! falling back to the literal Glyph dark `--fg` hex unthemed. That is what
//! makes the label themeable in the sense that actually matters here: a
//! brightness swap re-inks it correctly, and the resolved color feeds the
//! same layout-time-baked-color contract every catalog text run keeps
//! (`docs/WIDGETS_CODE_STANDARDS.md`'s Theming & Animation Conventions), so a
//! theme swap forces a re-shape via `ChangeFlags::LAYOUT`.
//!
//! # Accent-role split
//!
//! The ring stroke is accent *ink* (`primary`, selected) or the pre-flattened
//! `outline` role (unselected) — never a wash; the inset dot is the accent
//! **bright fill** (`primary_container`), the same split
//! [`crate::toggle`]'s knob keeps and `docs/WIDGETS_CODE_STANDARDS.md`'s
//! Glyph accent-role-split precedent documents.
//!
//! # Controlled component (never self-mutating)
//!
//! Like [`crate::toggle`]/[`crate::segmented`]/[`crate::tabs`], `radio`
//! reports the *requested* selection through `on_select(state)` on a release
//! inside its bounds and never flips its own `selected`; the app's next
//! `rebuild` feeds the confirmed value back in. Unlike `toggle`, a release
//! fires unconditionally — an already-selected radio still reports the
//! request, mirroring baseline `Radio`'s `on_select` (a radio group's own
//! logic decides whether re-selecting the current option is a no-op).
//!
//! # No pressed, disabled, focus, or motion states in v1
//!
//! Mirrors [`crate::toggle`]'s documented v1 scope: a press is armed state
//! only (nothing paints differently while captured), there is no disabled
//! affordance, and the selection carries no transition — no Glyph source
//! authors radio motion, so animating it would be an invented design fact
//! rather than a transcribed one (`docs/CODE_STANDARDS.md`'s
//! **Community-approximate** rule bars exactly this). A future disabled
//! state should follow the baseline `Button`'s 0.38 disabled-opacity
//! convention, the same note `toggle`'s docs leave for itself.

use std::rc::Rc;

use frust::authoring::text::{
    FontFamily, FontWeight, GenericSlot, TextContext, TextLayout, TextStyle,
};
use frust::authoring::{Action, Role};
use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, ErasedCallback, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, PointerPhase, SemanticsCtx, View, Widget, erase_callback,
};
use frust::{ShapeScale, Theme};
use kurbo::{Point, Rect, RoundedRect, Shape, Size, Vec2};
use peniko::{Brush, Color};

use crate::press::presses;

/// Outer ring diameter, logical px. See the [module docs](self) for why this
/// is transcribed from muxr's own hand-composed row rather than a Glyph
/// mockup constant.
const RING: f64 = 16.0;
/// Ring stroke width, logical px (muxr's `.radio-dot{border:1.5px}` — the
/// mock's one non-hairline rule, per `chrome.rs`'s own `stroke_border` doc).
const RING_BORDER: f64 = 1.5;
/// The selected dot's inset inside the ring, logical px; the dot itself is
/// `RING - 2 * DOT_INSET` wide.
const DOT_INSET: f64 = 3.0;
/// Gap between the ring and the label, logical px.
const GAP: f64 = 9.0;
/// Corner-rounding tolerance for the ring's stroke path (the crate's shared
/// `RoundedRect::to_path` value — see `crate::badge`'s `PATH_TOLERANCE`).
const PATH_TOLERANCE: f64 = 0.1;

/// Label font size, logical px — matches `crate::list`'s `TITLE_SIZE` (the
/// mobile-legibility floor every Glyph body-adjacent role lands on; see
/// `crate::tokens::scales`' `GLYPH_BODY`).
const LABEL_SIZE: f32 = 13.0;

// ---- Unthemed fallback constants (Glyph **dark** values) -------------------

/// Unselected ring stroke — `outline` (`--border-bright`/`--border`
/// pre-flattened over `bg-surface`, `crate::tokens::color`'s dark scheme).
const RADIO_RING_OFF: Color = Color::from_rgb8(0x3e, 0x3f, 0x44);
/// The accent — `--amber`. Selected ring stroke *and* dot fill collapse onto
/// this one hex in the dark unthemed fallback (the themed path still splits
/// them into `primary`/`primary_container` — see the module docs).
const RADIO_ACCENT: Color = Color::from_rgb8(0xff, 0xb6, 0x27);
/// Label ink — `--fg`, themed `on_surface`.
const RADIO_LABEL_FG: Color = Color::from_rgb8(0xf2, 0xea, 0xd9);

/// The label's fixed style (family/weight/size are Glyph-authored constants,
/// not theme-resolved — see the module docs); only `color` varies.
fn label_style(color: Color) -> TextStyle {
    TextStyle {
        family: FontFamily::stack_with_generic(["IBM Plex Mono"], GenericSlot::Monospace),
        weight: FontWeight::REGULAR,
        ..TextStyle::new(LABEL_SIZE, color)
    }
}

/// The resolved radio palette: ring stroke, inset-dot fill, label ink.
struct RadioColors {
    ring: Color,
    dot: Color,
    label: Color,
}

/// Resolve the palette from the theme, falling back to the literal Glyph
/// **dark** constants with none threaded.
fn resolve_radio_colors(theme: Option<&Theme>, selected: bool) -> RadioColors {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            RadioColors {
                // Accent ink (never a wash) when selected; the pre-flattened
                // `outline` role otherwise — see `crate::badge`'s Neutral
                // variant for the same border-role precedent.
                ring: if selected {
                    scheme.primary
                } else {
                    scheme.outline
                },
                // Bright fill, never accent text/icon ink — the split
                // `docs/WIDGETS_CODE_STANDARDS.md` documents and
                // `crate::toggle`'s knob keeps.
                dot: scheme.primary_container,
                label: scheme.on_surface,
            }
        }
        None => RadioColors {
            ring: if selected {
                RADIO_ACCENT
            } else {
                RADIO_RING_OFF
            },
            dot: RADIO_ACCENT,
            label: RADIO_LABEL_FG,
        },
    }
}

/// Ring radius — `shape.full` resolved against the ring's own (always
/// square) box, so a themed pill/circle radius is token-driven rather than a
/// bare `RING / 2.0` guess (the `crate::toggle`/`crate::badge` precedent).
/// Unthemed: half the ring edge exactly, the same circle without a theme to
/// resolve.
fn ring_radius(theme: Option<&Theme>, size: Size) -> f64 {
    match theme {
        Some(theme) => ShapeScale::resolve(theme.shape.full, size.width, size.height),
        None => size.height / 2.0,
    }
}

/// A minimal retained text run: shapes lazily during layout and paints via
/// glyph runs. A self-contained mirror of `crate::text::TextWidget`'s cache
/// shape — every Glyph widget that shapes its own label keeps its own copy
/// rather than sharing one (see `crate::badge`'s `GlyphLabel`).
struct GlyphLabel {
    content: String,
    layout: Option<TextLayout>,
    laid_out_style: Option<TextStyle>,
}

impl GlyphLabel {
    fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            layout: None,
            laid_out_style: None,
        }
    }

    /// Replace the content, invalidating the cached shaped layout if it
    /// actually changed.
    fn set_content(&mut self, content: impl Into<String>) {
        let content = content.into();
        if self.content != content {
            self.content = content;
            self.layout = None;
        }
    }

    /// Shape (or reuse the cached shape of) this run at `style` — unbounded,
    /// single-line (a radio row's label is short by construction; no
    /// wrapping, matching `crate::badge`'s label). Re-shapes whenever `style`
    /// differs from the last pass, including a color-only change (the
    /// layout-time-baked-color contract, see the module docs).
    fn layout(&mut self, ctx: &mut LayoutCtx, style: &TextStyle) -> Size {
        if let Some(cached) = &self.layout
            && self.laid_out_style.as_ref() == Some(style)
        {
            return cached.size();
        }
        let text_ctx = ctx.text_context::<TextContext>();
        let laid = text_ctx.layout(&self.content, style, None);
        let size = laid.size();
        self.layout = Some(laid);
        self.laid_out_style = Some(style.clone());
        size
    }

    /// Emit this run's glyphs at absolute `origin`. A no-op before the first
    /// [`GlyphLabel::layout`] call.
    fn paint(&self, origin: Point, scene: &mut dyn PaintScene) {
        if let Some(layout) = &self.layout {
            for run in layout.to_scene_runs(origin) {
                scene.draw_glyph_run(run);
            }
        }
    }
}

/// A view-held, typed select callback (erased on build/rebuild).
type OnSelect<State> = Rc<dyn Fn(&mut State)>;

/// A declarative Glyph radio. See the [module docs](self).
pub struct RadioView<State: 'static> {
    selected: bool,
    label: Option<String>,
    on_select: OnSelect<State>,
}

/// Create a radio reflecting `selected` that fires `on_select(state)` on a
/// release inside its bounds — a **controlled** component (see the
/// [module docs](self)). Attach a visible, themed label with
/// [`RadioView::label`]; a labelless radio still paints and reports a
/// `Role::RadioButton`, just with no accessible name.
pub fn radio<State: 'static, F: Fn(&mut State) + 'static>(
    selected: bool,
    on_select: F,
) -> RadioView<State> {
    RadioView {
        selected,
        label: None,
        on_select: Rc::new(on_select),
    }
}

/// PascalCase alias for [`radio`], matching the widget-fn vocabulary
/// (`Button`/`Image`/…).
#[allow(non_snake_case)]
pub fn Radio<State: 'static, F: Fn(&mut State) + 'static>(
    selected: bool,
    on_select: F,
) -> RadioView<State> {
    radio(selected, on_select)
}

impl<State: 'static> RadioView<State> {
    /// Set the row's visible, themed label — also the semantics node's
    /// accessible name. See the [module docs](self) for why this is the gap
    /// baseline `Radio` cannot close.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

/// The retained widget for a [`RadioView`].
pub struct RadioWidget {
    /// The app-confirmed value (source of truth; adopted on `rebuild`).
    selected: bool,
    label: Option<GlyphLabel>,
    /// Retained for the semantics node's accessible name, independent of
    /// [`Self::label`]'s shaped-layout cache.
    label_text: Option<String>,
    label_size: Size,
    /// Armed by a `Down` (alongside `capture_pointer`), cleared on
    /// `Up`/`Cancel`. Nothing paints differently while armed — Glyph authors
    /// no pressed state for this control either (see the module docs).
    captured: bool,
    on_select: ErasedCallback,
}

fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

impl<State: 'static> View<State> for RadioView<State> {
    type Element = RadioWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> RadioWidget {
        RadioWidget {
            selected: self.selected,
            label: self.label.as_ref().map(GlyphLabel::new),
            label_text: self.label.clone(),
            label_size: Size::ZERO,
            captured: false,
            on_select: erase_callback(&self.on_select),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut RadioWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Closures aren't comparable — always reinstall the adapter.
        element.on_select = erase_callback(&self.on_select);
        let mut flags = ChangeFlags::NONE;
        if prev.selected != self.selected {
            // The app is the source of truth: adopt the value. No animation
            // to retarget — see the module docs.
            element.selected = self.selected;
            flags |= ChangeFlags::PAINT;
        }
        if prev.label != self.label {
            element.label_text = self.label.clone();
            match (&mut element.label, &self.label) {
                // Reuse the shaped-layout cache's allocation; content-equal
                // is already excluded by the `prev.label != self.label`
                // guard above.
                (Some(existing), Some(text)) => existing.set_content(text.clone()),
                (slot @ None, Some(text)) => *slot = Some(GlyphLabel::new(text.clone())),
                (slot, None) => *slot = None,
            }
            // The label width feeds layout (ring + gap + label), not just
            // paint.
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }
}

impl Widget for RadioWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let label_color = resolve_radio_colors(theme, self.selected).label;
        self.label_size = match &mut self.label {
            Some(label) => label.layout(ctx, &label_style(label_color)),
            None => Size::ZERO,
        };
        let gap = if self.label.is_some() { GAP } else { 0.0 };
        let width = RING + gap + self.label_size.width;
        let height = RING.max(self.label_size.height);
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let colors = resolve_radio_colors(theme, self.selected);
        let ring_size = Size::new(RING, RING);
        let radius = ring_radius(theme, ring_size);

        let origin = ctx.origin();
        let height = ctx.size().height;
        let ring_origin = origin + Vec2::new(0.0, ((height - RING) / 2.0).max(0.0));

        // Ring: stroked on its own centerline (inset by half the border
        // width) so the painted circle is border-box exact — the
        // `crate::toggle` track-border precedent.
        let inset = RING_BORDER / 2.0;
        let outline = RoundedRect::from_rect(
            Rect::from_origin_size(Point::ORIGIN, ring_size).inset(-inset),
            (radius - inset).max(0.0),
        );
        scene.stroke_path(
            ring_origin,
            &outline.to_path(PATH_TOLERANCE),
            RING_BORDER,
            &Brush::Solid(colors.ring),
        );

        // The inset dot paints only when selected.
        if self.selected {
            let dot_size = RING - 2.0 * DOT_INSET;
            scene.fill_rounded_rect(
                ring_origin + Vec2::new(DOT_INSET, DOT_INSET),
                Size::new(dot_size, dot_size),
                dot_size / 2.0,
                colors.dot,
            );
        }

        if let Some(label) = &self.label {
            let label_y = origin.y + ((height - self.label_size.height) / 2.0).max(0.0);
            label.paint(Point::new(origin.x + RING + GAP, label_y), scene);
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
                self.captured = true;
                ctx.capture_pointer();
                EventResult::Handled
            }
            PointerPhase::Move => {
                // Nothing paints differently while armed, so a captured move
                // is consumed without a redraw; `Up` re-tests the position.
                if self.captured {
                    EventResult::Handled
                } else {
                    EventResult::Ignored
                }
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.captured = false;
                if inside(p.position, ctx.size()) {
                    // Fire unconditionally: a release always requests "select
                    // me", never self-mutating `selected` — see the module
                    // docs.
                    (self.on_select)(ctx);
                }
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.captured = false;
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::RadioButton, |node| {
            node.set_selected(self.selected);
            node.add_action(Action::Click);
            if let Some(label) = &self.label_text {
                node.set_label(label.as_str());
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::{PointerButton, PointerEvent};
    use std::any::Any;

    #[derive(Default)]
    struct RadioState {
        selects: u32,
    }

    /// Records fills, strokes and glyph-run colors, in paint order:
    /// `rrects` = [dot] (only when selected), `strokes` = [ring],
    /// `glyph_colors` = [label] (only when labelled).
    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<Color>,
        glyph_colors: Vec<Color>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn stroke_path(&mut self, _o: Point, _p: &kurbo::BezPath, _w: f64, brush: &Brush) {
            if let Brush::Solid(c) = brush {
                self.strokes.push(*c);
            }
        }
        fn draw_glyph_run(&mut self, run: frust::authoring::scene::GlyphRun) {
            if let Brush::Solid(c) = run.brush {
                self.glyph_colors.push(c);
            }
        }
    }

    fn widget(selected: bool) -> RadioWidget {
        build(&labelled(selected))
    }

    fn labelled(selected: bool) -> RadioView<RadioState> {
        radio::<RadioState, _>(selected, |s: &mut RadioState| {
            s.selects += 1;
        })
        .label("daily")
    }

    fn build(view: &RadioView<RadioState>) -> RadioWidget {
        let mut counter = 0u64;
        View::<RadioState>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout_and_paint(w: &mut RadioWidget, theme: Option<&Theme>) -> Recorder {
        let mut tcx = TextContext::new();
        let mut lctx = match theme {
            Some(t) => {
                LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), Some(t as &dyn Any))
            }
            None => LayoutCtx::with_text_context(&mut tcx as &mut dyn Any),
        };
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 100.0)));
        let mut rec = Recorder::default();
        let mut pctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, size).with_theme(t),
            None => PaintCtx::new(Point::ZERO, size),
        };
        w.paint(&mut pctx, &mut rec);
        rec
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn secondary_ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Secondary,
        })
    }

    fn dispatch(w: &mut RadioWidget, state: &mut RadioState, event: &InputEvent, size: Size) {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, size);
        w.event(&mut ctx, event);
    }

    // ---- Paint: selected/unselected difference -----------------------------

    #[test]
    fn unthemed_unselected_paints_no_dot() {
        let mut w = widget(false);
        let rec = layout_and_paint(&mut w, None);
        assert_eq!(rec.strokes, vec![RADIO_RING_OFF], "unselected ring color");
        assert!(rec.rrects.is_empty(), "unselected paints no inset dot");
    }

    #[test]
    fn unthemed_selected_paints_the_accent_ring_and_dot() {
        let mut w = widget(true);
        let rec = layout_and_paint(&mut w, None);
        assert_eq!(rec.strokes, vec![RADIO_ACCENT], "selected ring color");
        assert_eq!(rec.rrects.len(), 1, "selected paints the inset dot");
        assert_eq!(rec.rrects[0].3, RADIO_ACCENT);
        assert_eq!(
            rec.rrects[0].1,
            Size::new(RING - 2.0 * DOT_INSET, RING - 2.0 * DOT_INSET)
        );
    }

    #[test]
    fn glyph_dark_theme_splits_ring_ink_from_the_bright_fill_dot() {
        let theme = crate::baseline();
        let scheme = theme.scheme();

        let mut off = widget(false);
        let rec = layout_and_paint(&mut off, Some(&theme));
        assert_eq!(rec.strokes, vec![scheme.outline]);
        assert!(rec.rrects.is_empty());

        let mut on = widget(true);
        let rec = layout_and_paint(&mut on, Some(&theme));
        assert_eq!(rec.strokes, vec![scheme.primary], "ring uses accent ink");
        assert_eq!(
            rec.rrects[0].3, scheme.primary_container,
            "dot uses the bright fill, never accent text ink"
        );
        // Dark collapses `primary`/`primary_container` onto the same hex
        // (both `D_AMBER`, see `crate::tokens::color`) — the roles are still
        // resolved distinctly (asserted above), the light baseline below is
        // where they actually diverge.
        assert_eq!(scheme.primary, scheme.primary_container);
    }

    #[test]
    fn glyph_light_theme_splits_ring_ink_from_the_dot_into_two_distinct_hexes() {
        let theme = crate::baseline().with_brightness(frust::Brightness::Light);
        let scheme = theme.scheme();
        let mut on = widget(true);
        let rec = layout_and_paint(&mut on, Some(&theme));
        assert_eq!(rec.strokes, vec![scheme.primary]);
        assert_eq!(rec.rrects[0].3, scheme.primary_container);
        assert_ne!(
            scheme.primary, scheme.primary_container,
            "light is where the accent-role split actually diverges — a \
             darkened text tone vs the still-bright fill"
        );
    }

    #[test]
    fn label_ink_resolves_on_surface_when_themed_and_the_fallback_hex_unthemed() {
        let mut w = widget(false);
        let rec = layout_and_paint(&mut w, None);
        assert_eq!(rec.glyph_colors, vec![RADIO_LABEL_FG]);

        let theme = crate::baseline();
        let mut w = widget(false);
        let rec = layout_and_paint(&mut w, Some(&theme));
        assert_eq!(rec.glyph_colors, vec![theme.scheme().on_surface]);
    }

    #[test]
    fn a_labelless_radio_paints_no_glyph_run_and_is_ring_width_only() {
        let view = radio::<RadioState, _>(false, |_s: &mut RadioState| {});
        let mut w = build(&view);
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 100.0)));
        assert_eq!(size, Size::new(RING, RING), "no label, no gap, ring-only");
        let rec = layout_and_paint(&mut w, None);
        assert!(rec.glyph_colors.is_empty());
    }

    // ---- Interaction ---------------------------------------------------------

    #[test]
    fn release_inside_fires_on_select_and_never_self_mutates() {
        let mut w = widget(false);
        let mut state = RadioState::default();
        let slab = Size::new(200.0, 24.0);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 8.0, 8.0), slab);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 8.0, 8.0), slab);
        assert_eq!(state.selects, 1);
        assert!(!w.selected, "radio must not mutate its own selected flag");
    }

    #[test]
    fn an_already_selected_radio_still_fires_on_release() {
        let mut w = widget(true);
        let mut state = RadioState::default();
        let slab = Size::new(200.0, 24.0);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 8.0, 8.0), slab);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 8.0, 8.0), slab);
        assert_eq!(state.selects, 1);
        assert!(w.selected, "still selected until the app rebuilds it");
    }

    #[test]
    fn release_outside_does_not_fire() {
        let mut w = widget(false);
        let mut state = RadioState::default();
        let slab = Size::new(200.0, 24.0);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 8.0, 8.0), slab);
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, 500.0, 8.0),
            slab,
        );
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 500.0, 8.0), slab);
        assert_eq!(state.selects, 0);
        assert!(!w.captured, "the release disarms either way");
    }

    #[test]
    fn cancel_disarms_the_press() {
        let mut w = widget(false);
        let mut state = RadioState::default();
        let slab = Size::new(200.0, 24.0);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 8.0, 8.0), slab);
        assert!(w.captured);
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Cancel, 8.0, 8.0),
            slab,
        );
        assert!(!w.captured);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 8.0, 8.0), slab);
        assert_eq!(state.selects, 0, "a post-cancel Up is not this gesture");
    }

    #[test]
    fn a_secondary_press_never_captures_or_fires() {
        let mut w = widget(false);
        let mut state = RadioState::default();
        let slab = Size::new(200.0, 24.0);
        dispatch(
            &mut w,
            &mut state,
            &secondary_ev(PointerPhase::Down, 8.0, 8.0),
            slab,
        );
        assert!(!w.captured);
        dispatch(
            &mut w,
            &mut state,
            &secondary_ev(PointerPhase::Up, 8.0, 8.0),
            slab,
        );
        assert_eq!(state.selects, 0);

        // The primary gesture still fires.
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 8.0, 8.0), slab);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 8.0, 8.0), slab);
        assert_eq!(state.selects, 1);
    }

    #[test]
    fn an_unarmed_move_is_ignored() {
        let mut w = widget(false);
        let mut state = RadioState::default();
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(200.0, 24.0));
        let result = w.event(&mut ctx, &ev(PointerPhase::Move, 8.0, 8.0));
        assert!(matches!(result, EventResult::Ignored));
        assert!(!ctx.needs_redraw(), "a hover must not request a redraw");
    }

    // ---- Semantics -------------------------------------------------------

    #[test]
    fn semantics_reports_a_labelled_selected_radio_button() {
        fn logic(_s: &mut RadioState) -> RadioView<RadioState> {
            radio::<RadioState, _>(true, |_s: &mut RadioState| {}).label("daily")
        }
        let mut root: frust_core::RenderRoot<RadioState, RadioView<RadioState>> =
            frust_core::RenderRoot::new();
        let mut state = RadioState::default();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(200.0, 200.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();

        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::RadioButton)
            .expect("the radio contributes a Role::RadioButton node");
        assert_eq!(node.is_selected(), Some(true));
        assert!(node.supports_action(Action::Click));
        assert_eq!(node.label(), Some("daily"));
    }

    #[test]
    fn semantics_mirrors_an_unselected_unlabelled_radio() {
        fn logic(_s: &mut RadioState) -> RadioView<RadioState> {
            radio::<RadioState, _>(false, |_s: &mut RadioState| {})
        }
        let mut root: frust_core::RenderRoot<RadioState, RadioView<RadioState>> =
            frust_core::RenderRoot::new();
        let mut state = RadioState::default();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(200.0, 200.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();

        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::RadioButton)
            .expect("the radio contributes a Role::RadioButton node");
        assert_eq!(node.is_selected(), Some(false));
        assert_eq!(node.label(), None);
    }

    // ---- Rebuild -----------------------------------------------------------

    #[test]
    fn a_selected_change_alone_never_forces_a_relayout() {
        let prev = radio::<RadioState, _>(false, |_s: &mut RadioState| {}).label("daily");
        let mut w = build(&prev);
        let next = radio::<RadioState, _>(true, |_s: &mut RadioState| {}).label("daily");
        let mut counter = 0u64;
        let flags =
            View::<RadioState>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(w.selected);
        assert!(flags.needs_paint());
        assert!(
            !flags.needs_layout(),
            "the ring/label geometry never changes on selection alone"
        );
    }

    #[test]
    fn a_label_change_forces_a_relayout_and_republish() {
        let prev = radio::<RadioState, _>(false, |_s: &mut RadioState| {});
        let mut w = build(&prev);
        assert_eq!(w.label_text, None);
        let next = radio::<RadioState, _>(false, |_s: &mut RadioState| {}).label("weekly");
        let mut counter = 0u64;
        let flags =
            View::<RadioState>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert_eq!(w.label_text.as_deref(), Some("weekly"));
        assert!(flags.needs_layout());
        assert!(flags.needs_paint());
    }
}
