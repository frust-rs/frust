//! `textarea`: shadcn's multi-line text field — the baseline
//! [`frust::text_input`]'s wrapped multi-line mode in shadcn chrome.
//!
//! Source: `apps/v4/registry/new-york-v4/ui/textarea.tsx` (shadcn/ui v4, rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved 2026-08-17) —
//! `field-sizing-content min-h-16 w-full rounded-md border border-input
//! bg-transparent px-3 py-2 text-base shadow-xs`, with the same
//! focus-visible/aria-invalid/disabled blocks `input.tsx` carries. The chrome is
//! therefore literally the same code: [`crate::components::input`]'s
//! [`resolve_field_border`]/[`paint_field_border`], which exist because these two
//! class lists differ only in padding and height.
//!
//! # Multi-line comes from the baseline
//!
//! [`frust::TextInputView::multiline`] is a real wrapped multi-line mode
//! (soft-wrap at the layout width, vertical growth per line, keep-caret-in-view
//! scrolling past the cap), so nothing here forks a text engine — see
//! [`crate::components::input`]'s module docs for the three builder seams that
//! hide the baseline's own chrome, and for the two gaps wrapping implies
//! (`dark:bg-input/30`, and the baseline's own 38% content dim).
//!
//! **One deliberate divergence:** upstream's `field-sizing-content` grows the box
//! with its content without an upper bound, while the baseline's multi-line mode
//! requires a visible-line cap (past it the text scrolls internally instead).
//! This port therefore grows from [`MIN_HEIGHT`] (`min-h-16`) up to
//! [`DEFAULT_MAX_VISIBLE_LINES`] lines and scrolls beyond that, with
//! [`TextareaView::max_visible_lines`] as the app's knob.

use std::rc::Rc;

use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, CursorIcon, EventCtx, EventResult,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, PointerPhase, SemanticsCtx, Size, View,
    Widget, any, build_child, rebuild_child, route_event_single, teardown_child, visit_children,
};
use frust::{Theme, text_input};

use crate::components::input::{FieldChrome, paint_field_border, resolve_field_border};
use crate::style;
use crate::tokens::ShadcnRadius;

/// The field's minimum height: `min-h-16`.
pub const MIN_HEIGHT: f64 = 64.0;

/// How many lines the box grows to before its content starts scrolling inside
/// it — the bound upstream's `field-sizing-content` does not have (see the
/// [module docs](self)). Six lines of `text-base` clears a comfortable comment
/// box while keeping a long paste from pushing the rest of a form off-screen.
pub const DEFAULT_MAX_VISIBLE_LINES: usize = 6;

/// Field width used when the incoming constraints are horizontally unbounded —
/// the baseline field's own fallback (see [`crate::components::input`]).
const UNBOUNDED_WIDTH: f64 = 200.0;

/// A view-held, typed text callback (erased by the wrapped baseline field on
/// build).
type OnText<State> = Rc<dyn Fn(&mut State, String)>;

/// A declarative shadcn multi-line text field. See the [module docs](self).
pub struct TextareaView<State: 'static> {
    value: String,
    placeholder: String,
    invalid: bool,
    disabled: bool,
    flush: bool,
    max_visible_lines: usize,
    on_change: OnText<State>,
}

/// Create a controlled shadcn textarea showing `value`, reporting each edit
/// through `on_change(state, new_text)`.
///
/// Controlled like the baseline field it wraps: Enter inserts a newline (the
/// baseline's multi-line default) and the widget never owns the durable value.
pub fn textarea<State: 'static, F: Fn(&mut State, String) + 'static>(
    value: impl Into<String>,
    on_change: F,
) -> TextareaView<State> {
    TextareaView {
        value: value.into(),
        placeholder: String::new(),
        invalid: false,
        disabled: false,
        flush: false,
        max_visible_lines: DEFAULT_MAX_VISIBLE_LINES,
        on_change: Rc::new(on_change),
    }
}

impl<State: 'static> TextareaView<State> {
    /// Set the placeholder shown while the field is empty and unfocused.
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Put the field in its `aria-invalid` state (`destructive` border, and a
    /// `destructive`-tinted ring while focused).
    pub fn invalid(mut self, invalid: bool) -> Self {
        self.invalid = invalid;
        self
    }

    /// Disable the field: inert, dimmed, and asking for
    /// [`style::DISABLED_CURSOR`].
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Raise or lower the growth cap (see the [module docs](self)); clamped to at
    /// least one line by the baseline.
    pub fn max_visible_lines(mut self, lines: usize) -> Self {
        self.max_visible_lines = lines;
        self
    }

    /// Suppress this field's own border, shadow and ring, for a textarea nested
    /// in an [`input_group`](crate::input_group) — the port of upstream's
    /// `InputGroupTextarea`.
    pub fn flush(mut self, flush: bool) -> Self {
        self.flush = flush;
        self
    }

    /// The wrapped baseline field in multi-line mode, its own chrome suppressed.
    fn control(&self) -> AnyView<State> {
        let on_change = self.on_change.clone();
        any(
            text_input(self.value.clone(), move |state: &mut State, text| {
                on_change(state, text)
            })
            .placeholder(self.placeholder.clone())
            .multiline(self.max_visible_lines)
            .enabled(!self.disabled)
            .padding(style::spacing(3.0), style::spacing(2.0))
            .border_width(0.0)
            .corner_radius(ShadcnRadius::shadcn().md),
        )
    }
}

/// The retained widget for a [`TextareaView`].
pub struct TextareaWidget {
    child: ChildPod,
    invalid: bool,
    disabled: bool,
    flush: bool,
}

impl<State: 'static> View<State> for TextareaView<State> {
    type Element = TextareaWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> TextareaWidget {
        TextareaWidget {
            child: build_child(&self.control(), ctx),
            invalid: self.invalid,
            disabled: self.disabled,
            flush: self.flush,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut TextareaWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(&prev.control(), &self.control(), &mut element.child, ctx);
        if element.invalid != self.invalid
            || element.disabled != self.disabled
            || element.flush != self.flush
        {
            element.invalid = self.invalid;
            element.disabled = self.disabled;
            element.flush = self.flush;
            flags |= ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut TextareaWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.control(), &mut element.child, ctx);
    }
}

/// Whether `pos` (widget-local) lies inside a `size`-shaped box.
fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

impl Widget for TextareaWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            UNBOUNDED_WIDTH
        };
        // A minimum height of `min-h-16` on the child's own constraints is what
        // makes the baseline report at least that (it `constrain`s its
        // content-derived height), so its opaque background always covers this
        // widget's border box exactly.
        let child_bc = BoxConstraints::new(
            Size::new(width, MIN_HEIGHT.min(bc.max().height)),
            Size::new(width, bc.max().height),
        );
        let size = self.child.layout_child(ctx, &child_bc);
        self.child.set_origin(Point::ORIGIN);
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let chrome = FieldChrome {
            focused: ctx.has_focus() && !self.disabled,
            invalid: self.invalid,
            disabled: self.disabled,
        };
        let (origin, size) = (ctx.origin(), ctx.size());
        let theme = Theme::from_paint_ctx(ctx);
        let resolved = resolve_field_border(theme, chrome);
        if !self.flush {
            style::draw_shadow(
                scene,
                origin,
                size,
                resolved.radius,
                style::SHADOW_XS,
                theme,
            );
        }
        self.child.paint_child(ctx, scene);
        if !self.flush {
            paint_field_border(scene, origin, size, resolved);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let result = route_event_single(&mut self.child, ctx, event);
        if let InputEvent::Pointer(p) = event
            && matches!(p.phase, PointerPhase::Move)
            && inside(p.position, ctx.size())
        {
            ctx.set_cursor(if self.disabled {
                style::DISABLED_CURSOR
            } else {
                CursorIcon::Text
            });
        }
        result
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // Chrome only: the wrapped field contributes the editable's node.
        self.child.semantics_child(ctx);
    }

    visit_children!(child);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::input::FALLBACK;
    use frust::authoring::text::TextContext;
    use frust::authoring::{Brush, Color, PointerButton, PointerEvent, Rect, Shape};
    use frust::{Brightness, FrameTime};
    use frust_core::RenderRoot;
    use std::any::Any;

    /// A recording `PaintScene` for the border/ring strokes and the shadow.
    #[derive(Default)]
    struct Recorder {
        strokes: Vec<(Rect, f64, Color)>,
        shadows: usize,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn stroke_path(
            &mut self,
            origin: Point,
            path: &frust::authoring::BezPath,
            width: f64,
            brush: &Brush,
        ) {
            let color = match brush {
                Brush::Solid(c) => *c,
                _ => Color::TRANSPARENT,
            };
            self.strokes
                .push((path.bounding_box() + origin.to_vec2(), width, color));
        }
        fn draw_shadow(
            &mut self,
            _origin: Point,
            _size: Size,
            _radius: f64,
            _std_dev: f64,
            _color: Color,
        ) {
            self.shadows += 1;
        }
    }

    impl Recorder {
        fn border(&self) -> (Rect, f64, Color) {
            *self
                .strokes
                .iter()
                .find(|(_, w, _)| *w == style::BORDER_WIDTH)
                .expect("a 1px border was stroked")
        }

        fn ring(&self) -> Option<Color> {
            self.strokes
                .iter()
                .find(|(_, w, _)| *w == style::FOCUS_RING_WIDTH)
                .map(|(_, _, c)| *c)
        }
    }

    const WINDOW: Size = Size::new(240.0, 400.0);

    struct Harness {
        root: RenderRoot<String, TextareaView<String>>,
        state: String,
        tcx: TextContext,
    }

    impl Harness {
        fn new(value: &str) -> Self {
            let mut h = Harness {
                root: RenderRoot::new(),
                state: value.to_string(),
                tcx: TextContext::new(),
            };
            h.pass();
            h
        }

        fn pass(&mut self) -> Size {
            let mut logic = |state: &mut String| {
                textarea::<String, _>(state.clone(), |s: &mut String, t| *s = t)
                    .placeholder("Tell us more")
            };
            self.root.rebuild(&mut logic, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any)
        }

        fn paint(&mut self) -> Recorder {
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, FrameTime::ZERO);
            rec
        }

        fn pointer(&mut self, phase: PointerPhase, x: f64, y: f64) {
            self.root.event(
                &mut self.state,
                &InputEvent::Pointer(PointerEvent {
                    phase,
                    position: Point::new(x, y),
                    button: PointerButton::Primary,
                }),
            );
        }
    }

    fn build<S: 'static>(view: &TextareaView<S>) -> TextareaWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut TextareaWidget, size: Size) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(size))
    }

    #[test]
    fn an_empty_textarea_is_min_h_16_and_the_control_fills_it() {
        let view: TextareaView<String> = textarea("", |_s: &mut String, _t| {});
        let mut w = build(&view);
        let size = layout(&mut w, WINDOW);
        assert_eq!(size, Size::new(WINDOW.width, MIN_HEIGHT));
        assert_eq!(w.child.size(), size);
        assert_eq!(w.child.origin(), Point::ORIGIN);
    }

    #[test]
    fn it_grows_past_the_minimum_and_stops_at_the_line_cap() {
        let lines = |n: usize, cap: usize| {
            let text = vec!["line"; n].join("\n");
            let view: TextareaView<String> =
                textarea(text, |_s: &mut String, _t| {}).max_visible_lines(cap);
            let mut w = build(&view);
            layout(&mut w, WINDOW).height
        };
        let short = lines(1, 6);
        let tall = lines(6, 6);
        assert_eq!(short, MIN_HEIGHT, "one line still reserves min-h-16");
        assert!(tall > MIN_HEIGHT, "six lines grew past the minimum");
        // Past the cap the box freezes and the baseline scrolls its content.
        assert_eq!(lines(20, 6), tall, "growth stops at max_visible_lines");
        assert!(lines(20, 3) < tall, "a lower cap is a shorter box");
    }

    #[test]
    fn the_chrome_is_the_input_border_at_rounded_md_with_shadow_xs() {
        let mut h = Harness::new("");
        let rec = h.paint();
        let (bbox, _, color) = rec.border();
        assert_eq!(color, FALLBACK.input);
        assert_eq!(bbox.height(), MIN_HEIGHT);
        assert_eq!(rec.shadows, 1, "shadow-xs under the field");
        assert!(rec.ring().is_none());
    }

    #[test]
    fn focus_rings_the_field_and_typing_round_trips_through_the_callback() {
        let mut h = Harness::new("");
        h.root
            .set_theme(Box::new(crate::theme().with_brightness(Brightness::Light)));
        h.pass();
        h.pointer(PointerPhase::Down, 40.0, 20.0);
        h.pointer(PointerPhase::Up, 40.0, 20.0);
        let theme = crate::theme().with_brightness(Brightness::Light);
        let ring = style::ring_color(None, Some(&theme));
        let rec = h.paint();
        assert_eq!(rec.border().2, ring, "focus-visible:border-ring");
        assert_eq!(
            rec.ring(),
            Some(style::with_alpha(ring, style::FOCUS_RING_OPACITY))
        );

        h.root.event(
            &mut h.state,
            &InputEvent::Key(frust::authoring::KeyEvent {
                key: frust::authoring::Key::Character("x".to_string()),
                modifiers: frust::authoring::Modifiers::default(),
                repeat: false,
            }),
        );
        assert_eq!(h.state, "x", "the edit was reported, not self-applied");
    }

    #[test]
    fn the_cursor_over_the_field_is_text() {
        let mut h = Harness::new("");
        h.pointer(PointerPhase::Move, 40.0, 20.0);
        assert_eq!(h.root.cursor(), CursorIcon::Text);
    }

    #[test]
    fn a_disabled_textarea_dims_its_border_and_asks_not_allowed() {
        let view: TextareaView<String> = textarea("", |_s: &mut String, _t| {}).disabled(true);
        let mut w = build(&view);
        let size = layout(&mut w, WINDOW);
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, size, FrameTime::ZERO);
        w.paint(&mut ctx, &mut rec);
        assert_eq!(rec.border().2.components[3], style::DISABLED_OPACITY);
    }

    #[test]
    fn a_flush_textarea_paints_no_chrome_of_its_own() {
        let view: TextareaView<String> = textarea("", |_s: &mut String, _t| {}).flush(true);
        let mut w = build(&view);
        let size = layout(&mut w, WINDOW);
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, size, FrameTime::ZERO);
        w.paint(&mut ctx, &mut rec);
        assert!(rec.strokes.is_empty());
        assert_eq!(rec.shadows, 0);
    }

    #[test]
    fn visit_children_publishes_the_wrapped_field() {
        let view: TextareaView<String> = textarea("", |_s: &mut String, _t| {});
        let w = build(&view);
        let mut seen = 0usize;
        Widget::visit_children(&w, &mut |_pod| seen += 1);
        assert_eq!(seen, 1);
    }
}
