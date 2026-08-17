//! `input`: shadcn's text field — the baseline [`frust::text_input`] wrapped in
//! shadcn chrome.
//!
//! Source: `apps/v4/registry/new-york-v4/ui/input.tsx` (shadcn/ui v4, rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved 2026-08-17) —
//! `h-9 w-full rounded-md border border-input bg-transparent px-3 py-1 text-base
//! shadow-xs`, plus the two state blocks
//! (`focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50`
//! and `aria-invalid:border-destructive aria-invalid:ring-destructive/20`) and
//! `disabled:cursor-not-allowed disabled:opacity-50`.
//!
//! # Wrapping the baseline, not forking it
//!
//! The editable itself is the framework's own text field: this widget owns a
//! single [`ChildPod`] holding a [`frust::TextInputView`] and paints shadcn's
//! chrome around it. Every editing concern — the [`TextEditor`], IME/focus
//! publication, the caret blink, selection, the controlled-value reconcile — stays
//! the baseline's. Three builder seams make its own chrome disappear so this one
//! can show:
//!
//! - `border_width(0.0)` — the baseline draws its frame as a border-colored
//!   rounded rect with the background inset by the border width, so a zero width
//!   leaves only the background fill. That fill is what makes the field opaque,
//!   which is why the border and the ring are painted **after** the child (a
//!   stroke over the fill) while the shadow is painted before it.
//! - `corner_radius` — baked from [`ShadcnRadius::md`] rather than re-resolved
//!   per theme, because the seam is a view-time builder value and no `Theme` is
//!   threaded into `View::build`. An app that swaps the radius scale on the theme
//!   extension moves this widget's border but not the child's background corner;
//!   the two agree for every shipped preset (the scale is
//!   brightness-invariant and identical in all seven).
//! - `padding(px-3, py-1)` — shadcn's own inner padding, so the caret and glyphs
//!   sit where the class list puts them.
//!
//! Two fidelity gaps follow from wrapping rather than forking, both deliberate:
//!
//! - **`dark:bg-input/30` is not painted.** The baseline resolves its own
//!   background from the theme's `surface` role (shadcn `--background`) and offers
//!   no seam to override it, so the dark-mode `input`-tinted wash cannot show
//!   through an opaque fill this widget does not own.
//! - **Disabled dims by two different factors.** This widget's chrome uses
//!   shadcn's [`style::DISABLED_OPACITY`] (50%); the child's glyphs/placeholder
//!   keep the baseline's own M3-derived content multiplier (38%), since
//!   `enabled(false)` is what makes the field inert and the dimming rides along
//!   with it.
//!
//! # Text size and family
//!
//! [`style::TEXT_BASE`] (16px) is the size the baseline's default `TextStyle`
//! already carries, so the field is left on it: shadcn authors `text-base` with a
//! `md:text-sm` step down, and this port keeps the mobile-first rung
//! ([`style::TEXT_BASE`]'s own note). The *family* stays the baseline's system
//! sans rather than the theme's bundled Inter — `TextInputView::text_style` is
//! all-or-nothing (setting it marks the glyph color explicit, which would freeze
//! a light-mode ink color into a dark-mode field), so keeping the theme's
//! `on_surface` resolution is worth the family.
//!
//! [`TextEditor`]: frust::authoring::text::TextEditor
//! [`ShadcnRadius::md`]: crate::ShadcnRadius::md

use std::rc::Rc;

use frust::authoring::{
    AnyView, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color, CursorIcon, EventCtx,
    EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, PointerPhase, Rect,
    RoundedRect, SemanticsCtx, Shape, Size, View, Widget, any, build_child, rebuild_child,
    route_event_single, teardown_child, visit_children,
};
use frust::{Theme, text_input};

use crate::style;
use crate::tokens::{ShadcnBase, ShadcnPalette, ShadcnRadius, ShadcnTokens};

/// Flattening tolerance for the stroked border path (the value every catalog's
/// stroked outline uses).
const PATH_TOLERANCE: f64 = 0.1;

/// Field width used when the incoming constraints are horizontally unbounded —
/// the baseline field's own fallback, restated here because this widget resolves
/// its width before handing the child tight constraints.
const UNBOUNDED_WIDTH: f64 = 200.0;

/// Ring alpha for an invalid control in light mode: `aria-invalid:ring-destructive/20`.
const INVALID_RING_ALPHA_LIGHT: f32 = 0.20;
/// Ring alpha for an invalid control in dark mode:
/// `dark:aria-invalid:ring-destructive/40`.
const INVALID_RING_ALPHA_DARK: f32 = 0.40;

/// The unthemed fallback token table: the `neutral` base preset's light-mode
/// palette — the same tables [`theme()`](crate::theme) folds, so a pass with no
/// theme threaded paints shadcn's own default light values rather than a
/// hand-copied hex per component.
pub(crate) const FALLBACK: ShadcnPalette = ShadcnBase::Neutral.light();

/// The `--input` border token every form control strokes: themed
/// `outline_variant` (the role `--input` folds onto), else the fallback table.
pub(crate) fn input_border(theme: Option<&Theme>) -> Color {
    theme.map_or(FALLBACK.input, |t| t.scheme().outline_variant)
}

/// The `--destructive` token: themed `error`, else the fallback table.
pub(crate) fn destructive(theme: Option<&Theme>) -> Color {
    theme.map_or(FALLBACK.destructive, |t| t.scheme().error)
}

/// The interaction state a form control's chrome is painted from.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct FieldChrome {
    /// Whether the control (or, for a group, its inner control) holds focus.
    pub focused: bool,
    /// Whether the control is in its `aria-invalid` state.
    pub invalid: bool,
    /// Whether the control is disabled.
    pub disabled: bool,
}

/// A form control's chrome, resolved off the theme.
///
/// Resolved *before* the wrapped child paints and consumed after it, because the
/// child paint borrows the `PaintCtx` mutably while a live `&Theme` borrows it
/// immutably — so the theme reads all happen up front and only owned colors
/// cross the child's paint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FieldBorder {
    /// `rounded-md`, resolved from the theme's radius scale.
    pub radius: f64,
    /// The 1px border color, with the disabled treatment already applied.
    pub border: Color,
    /// The ring to paint, if any.
    pub ring: FieldRing,
}

/// Which ring a focused control paints (none when it is not focused).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum FieldRing {
    /// No ring: the control is not focused. The `aria-invalid:ring-*` classes
    /// only *re-color* the ring — its width comes from `focus-visible:ring-[3px]`
    /// alone — so an unfocused invalid control rings at zero width.
    None,
    /// `focus-visible:ring-ring/50`, painted through [`style::draw_focus_ring`]
    /// (which owns that opacity). The carried color is the raw `--ring`.
    Focus(Color),
    /// `aria-invalid:ring-destructive/20` (light) or `/40` (dark) — a ring at an
    /// alpha [`style::draw_focus_ring`] does not paint, so the color travels
    /// pre-alpha'd for [`paint_ring`].
    Invalid(Color),
}

/// Resolve a shadcn form control's border/ring — shared by `input`, `textarea`,
/// `native_select` and `input_group`, whose class lists carry byte-identical
/// border/focus/invalid blocks.
pub(crate) fn resolve_field_border(theme: Option<&Theme>, chrome: FieldChrome) -> FieldBorder {
    // `aria-invalid:border-destructive` outranks `focus-visible:border-ring`
    // (later in the class list, and the state it reports is the more important
    // one).
    let border = if chrome.invalid {
        destructive(theme)
    } else {
        style::focus_border(input_border(theme), chrome.focused, theme)
    };
    let ring = match (chrome.focused, chrome.invalid) {
        (false, _) => FieldRing::None,
        (true, false) => FieldRing::Focus(style::ring_color(None, theme)),
        (true, true) => {
            let alpha = if style::is_dark(theme) {
                INVALID_RING_ALPHA_DARK
            } else {
                INVALID_RING_ALPHA_LIGHT
            };
            FieldRing::Invalid(style::with_alpha(destructive(theme), alpha))
        }
    };
    FieldBorder {
        radius: field_radius(theme),
        border: style::disabled_tint(border, chrome.disabled),
        ring,
    }
}

/// Paint a resolved form-control border and its ring.
///
/// Called **after** the wrapped child has painted: the child's own background
/// fill is opaque, so a border stroked before it would be covered.
pub(crate) fn paint_field_border(
    scene: &mut dyn PaintScene,
    origin: Point,
    size: Size,
    resolved: FieldBorder,
) {
    let rr = RoundedRect::from_rect(Rect::from_origin_size(Point::ORIGIN, size), resolved.radius);
    scene.stroke_path(
        origin,
        &rr.to_path(PATH_TOLERANCE),
        style::BORDER_WIDTH,
        &Brush::Solid(resolved.border),
    );
    match resolved.ring {
        FieldRing::None => {}
        FieldRing::Focus(ring) => {
            style::draw_focus_ring(scene, origin, size, resolved.radius, ring);
        }
        FieldRing::Invalid(tinted) => {
            paint_ring(scene, origin, size, resolved.radius, tinted);
        }
    }
}

/// Paint a ring in an explicitly-alpha'd `color` — the alpha-parameterized
/// counterpart to [`style::draw_focus_ring`], which bakes the `ring/50` opacity
/// its own class list names.
///
/// The invalid state needs `destructive/20` (light) or `destructive/40` (dark),
/// so it cannot go through the shared helper; the geometry is identical and
/// reads the same [`style::FOCUS_RING_WIDTH`], so the two cannot drift on width
/// or offset.
pub(crate) fn paint_ring(
    scene: &mut dyn PaintScene,
    origin: Point,
    size: Size,
    radius: f64,
    color: Color,
) {
    let half = style::FOCUS_RING_WIDTH / 2.0;
    let rr = RoundedRect::new(
        -half,
        -half,
        size.width + half,
        size.height + half,
        radius + half,
    );
    scene.stroke_path(
        origin,
        &rr.to_path(PATH_TOLERANCE),
        style::FOCUS_RING_WIDTH,
        &Brush::Solid(color),
    );
}

/// The corner radius a form control's chrome paints at: `rounded-md`.
pub(crate) fn field_radius(theme: Option<&Theme>) -> f64 {
    ShadcnTokens::resolve_radius(None, theme).md
}

/// A view-held, typed text callback (erased by the wrapped baseline field on
/// build).
type OnText<State> = Rc<dyn Fn(&mut State, String)>;

/// A declarative shadcn text field. See the [module docs](self).
pub struct InputView<State: 'static> {
    value: String,
    placeholder: String,
    invalid: bool,
    disabled: bool,
    password: bool,
    flush: bool,
    on_change: OnText<State>,
    on_submit: Option<OnText<State>>,
}

/// Create a controlled shadcn text field showing `value`, reporting each edit
/// through `on_change(state, new_text)`.
///
/// Controlled exactly like the baseline field it wraps: the widget never owns the
/// durable value, and an app that rejects or transforms the requested text in
/// `on_change` sees its own value win on the next rebuild.
pub fn input<State: 'static, F: Fn(&mut State, String) + 'static>(
    value: impl Into<String>,
    on_change: F,
) -> InputView<State> {
    InputView {
        value: value.into(),
        placeholder: String::new(),
        invalid: false,
        disabled: false,
        password: false,
        flush: false,
        on_change: Rc::new(on_change),
        on_submit: None,
    }
}

impl<State: 'static> InputView<State> {
    /// Set the placeholder shown while the field is empty and unfocused
    /// (`placeholder:text-muted-foreground`).
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Set the Enter handler (the baseline field's `on_submit`).
    pub fn on_submit<F: Fn(&mut State, String) + 'static>(mut self, on_submit: F) -> Self {
        self.on_submit = Some(Rc::new(on_submit));
        self
    }

    /// Put the field in its `aria-invalid` state: a `destructive` border, and a
    /// `destructive`-tinted ring while focused.
    pub fn invalid(mut self, invalid: bool) -> Self {
        self.invalid = invalid;
        self
    }

    /// Disable the field: inert (it refuses focus, so keyboard and IME editing
    /// are unreachable), dimmed, and asking for
    /// [`style::DISABLED_CURSOR`].
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Mask the rendered glyphs (`type="password"` upstream; the baseline
    /// field's `obscured` mode, which also publishes the secret IME hint).
    pub fn password(mut self, password: bool) -> Self {
        self.password = password;
        self
    }

    /// Suppress this field's own border, shadow and ring, for a field nested in
    /// an [`input_group`](crate::input_group) — the port of upstream's
    /// `InputGroupInput` (`border-0 shadow-none focus-visible:ring-0`), whose
    /// chrome the group paints instead.
    pub fn flush(mut self, flush: bool) -> Self {
        self.flush = flush;
        self
    }

    /// The wrapped baseline field, configured with its own chrome suppressed.
    fn control(&self) -> AnyView<State> {
        let on_change = self.on_change.clone();
        let mut field = text_input(self.value.clone(), move |state: &mut State, text| {
            on_change(state, text)
        })
        .placeholder(self.placeholder.clone())
        .enabled(!self.disabled)
        .obscured(self.password)
        .padding(style::spacing(3.0), style::spacing(1.0))
        .border_width(0.0)
        .corner_radius(ShadcnRadius::shadcn().md);
        if let Some(on_submit) = self.on_submit.clone() {
            field = field.on_submit(move |state: &mut State, text| on_submit(state, text));
        }
        any(field)
    }
}

/// The retained widget for an [`InputView`].
pub struct InputWidget {
    child: ChildPod,
    invalid: bool,
    disabled: bool,
    flush: bool,
}

impl<State: 'static> View<State> for InputView<State> {
    type Element = InputWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> InputWidget {
        InputWidget {
            child: build_child(&self.control(), ctx),
            invalid: self.invalid,
            disabled: self.disabled,
            flush: self.flush,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut InputWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(&prev.control(), &self.control(), &mut element.child, ctx);
        if element.invalid != self.invalid || element.flush != self.flush {
            element.invalid = self.invalid;
            element.flush = self.flush;
            flags |= ChangeFlags::PAINT;
        }
        if element.disabled != self.disabled {
            element.disabled = self.disabled;
            flags |= ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut InputWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.control(), &mut element.child, ctx);
    }
}

/// Whether `pos` (widget-local) lies inside a `size`-shaped box.
fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

impl Widget for InputWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            UNBOUNDED_WIDTH
        };
        // Tight constraints are what pin the wrapped field to `h-9`: the baseline
        // would otherwise report its own text-derived height, leaving its opaque
        // background smaller than this widget's border box.
        let size = Size::new(width, style::HEIGHT_DEFAULT);
        self.child.layout_child(ctx, &BoxConstraints::tight(size));
        self.child.set_origin(Point::ORIGIN);
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // The pod's focus path is authoritative (a container-routed blur never
        // reaches this widget's `event`); a disabled field never reads as focused.
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
        // The cursor is asked for from the uncaptured `Move` arm only, and after
        // routing so this widget's shape wins over the child's (the baseline field
        // asks for none of its own): `Text` over an editable, `NotAllowed` when
        // disabled.
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
        // Chrome only: the wrapped field contributes the `TextInput`/
        // `PasswordInput` node, its label/value and its disabled/read-only state.
        self.child.semantics_child(ctx);
    }

    visit_children!(child);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{PointerButton, PointerEvent};
    use frust::{Brightness, FrameTime};
    use frust_core::RenderRoot;
    use std::any::Any;

    /// A recording `PaintScene` capturing the ops these tests assert on: the
    /// rounded-rect fills (the wrapped field's background), the stroked paths
    /// (border + ring, as `(bbox, width, color)`), and the shadows.
    #[derive(Default)]
    pub(crate) struct Recorder {
        pub rrects: Vec<(Point, Size, f64, Color)>,
        pub strokes: Vec<(Rect, f64, Color)>,
        pub shadows: Vec<(Point, Size, f64, f64, Color)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn fill_rounded_rect(&mut self, origin: Point, size: Size, radius: f64, color: Color) {
            self.rrects.push((origin, size, radius, color));
        }
        fn stroke_path(
            &mut self,
            origin: Point,
            path: &frust::authoring::BezPath,
            width: f64,
            brush: &Brush,
        ) {
            let bbox = path.bounding_box() + origin.to_vec2();
            let color = match brush {
                Brush::Solid(c) => *c,
                _ => Color::TRANSPARENT,
            };
            self.strokes.push((bbox, width, color));
        }
        fn draw_shadow(
            &mut self,
            origin: Point,
            size: Size,
            radius: f64,
            std_dev: f64,
            color: Color,
        ) {
            self.shadows.push((origin, size, radius, std_dev, color));
        }
    }

    impl Recorder {
        /// The single 1px border stroke of the pass.
        fn border(&self) -> (Rect, f64, Color) {
            *self
                .strokes
                .iter()
                .find(|(_, w, _)| *w == style::BORDER_WIDTH)
                .expect("a 1px border was stroked")
        }

        /// The ring stroke of the pass, if any.
        fn ring(&self) -> Option<(Rect, f64, Color)> {
            self.strokes
                .iter()
                .find(|(_, w, _)| *w == style::FOCUS_RING_WIDTH)
                .copied()
        }
    }

    /// The window the harness lays out in.
    const WINDOW: Size = Size::new(240.0, 80.0);

    /// A field driven through a real `RenderRoot`, which is what makes the
    /// focus-path read (`PaintCtx::has_focus`) — and therefore the focus ring —
    /// observable at all: the root seeds it from its own focus mirror.
    struct Harness {
        root: RenderRoot<String, InputView<String>>,
        state: String,
        tcx: TextContext,
        invalid: bool,
        disabled: bool,
    }

    impl Harness {
        fn new() -> Self {
            Self::with(false, false)
        }

        fn with(invalid: bool, disabled: bool) -> Self {
            let mut h = Harness {
                root: RenderRoot::new(),
                state: String::new(),
                tcx: TextContext::new(),
                invalid,
                disabled,
            };
            h.pass();
            h
        }

        fn theme(&mut self, brightness: Brightness) {
            self.root
                .set_theme(Box::new(crate::theme().with_brightness(brightness)));
            self.pass();
        }

        fn pass(&mut self) {
            let (invalid, disabled) = (self.invalid, self.disabled);
            let mut logic = move |state: &mut String| {
                input::<String, _>(state.clone(), |s: &mut String, t| *s = t)
                    .placeholder("Email")
                    .invalid(invalid)
                    .disabled(disabled)
            };
            self.root.rebuild(&mut logic, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
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

    fn build<S: 'static>(view: &InputView<S>) -> InputWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut InputWidget, width: f64) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(width, 400.0)))
    }

    #[test]
    fn the_field_is_h9_and_the_wrapped_control_fills_it() {
        let view: InputView<String> = input("hi", |_s: &mut String, _t| {});
        let mut w = build(&view);
        let size = layout(&mut w, 200.0);
        assert_eq!(size, Size::new(200.0, style::HEIGHT_DEFAULT));
        // Tight constraints, so the baseline field reports exactly the border box
        // and its opaque background covers it.
        assert_eq!(w.child.size(), size);
        assert_eq!(w.child.origin(), Point::ORIGIN);
    }

    #[test]
    fn an_unbounded_width_falls_back_to_the_baseline_default() {
        let view: InputView<String> = input("", |_s: &mut String, _t| {});
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
    fn unthemed_chrome_is_the_input_token_at_rounded_md() {
        let mut h = Harness::new();
        let rec = h.paint();
        let (bbox, width, color) = rec.border();
        assert_eq!(width, style::BORDER_WIDTH);
        assert_eq!(color, FALLBACK.input, "border-input");
        assert_eq!(bbox.width(), WINDOW.width);
        assert_eq!(bbox.height(), style::HEIGHT_DEFAULT);
        // shadow-xs under the field, at the rounded-md radius.
        let (_, _, radius, std_dev, shadow) = rec.shadows[0];
        assert_eq!(radius, ShadcnRadius::shadcn().md);
        assert_eq!(std_dev, style::SHADOW_XS.std_dev);
        assert_eq!(shadow.components[3], style::SHADOW_XS.alpha);
        assert!(rec.ring().is_none(), "no ring at rest");
    }

    #[test]
    fn themed_chrome_resolves_the_input_role_and_the_theme_radius() {
        let mut h = Harness::new();
        h.theme(Brightness::Dark);
        let theme = crate::theme().with_brightness(Brightness::Dark);
        let rec = h.paint();
        assert_eq!(rec.border().2, theme.scheme().outline_variant);
        assert_eq!(
            rec.shadows[0].2,
            ShadcnTokens::resolve_radius(None, Some(&theme)).md
        );
        // The wrapped field's own background is the only rounded-rect fill whose
        // color is a surface role — proof the baseline still paints the field.
        assert!(
            rec.rrects
                .iter()
                .any(|(_, _, _, c)| *c == theme.scheme().surface),
            "the wrapped baseline field painted its background"
        );
    }

    #[test]
    fn focus_swaps_the_border_to_ring_and_paints_the_3px_ring() {
        let mut h = Harness::new();
        h.theme(Brightness::Light);
        let theme = crate::theme().with_brightness(Brightness::Light);
        // A `Down` inside the field focuses the wrapped editable; the root records
        // the focus path, and the next paint threads it back in.
        h.pointer(PointerPhase::Down, 40.0, 18.0);
        h.pointer(PointerPhase::Up, 40.0, 18.0);
        let rec = h.paint();
        let ring_color = style::ring_color(None, Some(&theme));
        assert_eq!(rec.border().2, ring_color, "focus-visible:border-ring");
        let (bbox, width, color) = rec.ring().expect("focus-visible:ring-[3px]");
        assert_eq!(width, style::FOCUS_RING_WIDTH);
        assert_eq!(
            color,
            style::with_alpha(ring_color, style::FOCUS_RING_OPACITY)
        );
        // Painted outside the border box, per Tailwind's non-inset ring.
        assert!(bbox.x0 < 0.0 && bbox.y0 < 0.0);
    }

    #[test]
    fn invalid_paints_a_destructive_border_and_only_rings_while_focused() {
        let mut h = Harness::with(true, false);
        h.theme(Brightness::Light);
        let theme = crate::theme().with_brightness(Brightness::Light);
        let rec = h.paint();
        assert_eq!(rec.border().2, theme.scheme().error, "border-destructive");
        assert!(
            rec.ring().is_none(),
            "the aria-invalid classes only re-color the ring; its width is focus-only"
        );

        h.pointer(PointerPhase::Down, 40.0, 18.0);
        h.pointer(PointerPhase::Up, 40.0, 18.0);
        let rec = h.paint();
        let (_, _, color) = rec.ring().expect("a focused invalid field rings");
        assert_eq!(
            color,
            style::with_alpha(theme.scheme().error, INVALID_RING_ALPHA_LIGHT)
        );
        // The border stays destructive, not ring — the invalid state outranks it.
        assert_eq!(rec.border().2, theme.scheme().error);

        // Dark mode rings at 40%.
        h.theme(Brightness::Dark);
        h.pointer(PointerPhase::Down, 40.0, 18.0);
        h.pointer(PointerPhase::Up, 40.0, 18.0);
        let dark = crate::theme().with_brightness(Brightness::Dark);
        let rec = h.paint();
        assert_eq!(
            rec.ring().expect("ring").2,
            style::with_alpha(dark.scheme().error, INVALID_RING_ALPHA_DARK)
        );
    }

    #[test]
    fn a_disabled_field_dims_its_border_refuses_focus_and_asks_not_allowed() {
        let mut h = Harness::with(false, true);
        let rec = h.paint();
        assert_eq!(
            rec.border().2.components[3],
            style::DISABLED_OPACITY,
            "disabled:opacity-50 on the chrome"
        );

        // Inert: the wrapped field refuses focus, so no ring can appear.
        h.pointer(PointerPhase::Down, 40.0, 18.0);
        h.pointer(PointerPhase::Up, 40.0, 18.0);
        assert!(h.paint().ring().is_none());
    }

    #[test]
    fn the_cursor_is_text_over_the_field_and_not_allowed_when_disabled() {
        let mut h = Harness::new();
        h.pointer(PointerPhase::Move, 40.0, 18.0);
        assert_eq!(h.root.cursor(), CursorIcon::Text);
        // Off the field: the pass resolves nothing, so the root falls back.
        h.pointer(PointerPhase::Move, 40.0, 70.0);
        assert_eq!(h.root.cursor(), CursorIcon::Default);

        let mut h = Harness::with(false, true);
        h.pointer(PointerPhase::Move, 40.0, 18.0);
        assert_eq!(h.root.cursor(), style::DISABLED_CURSOR);
    }

    #[test]
    fn typing_round_trips_through_the_callback_and_never_self_mutates() {
        let mut h = Harness::new();
        h.pointer(PointerPhase::Down, 40.0, 18.0);
        h.pointer(PointerPhase::Up, 40.0, 18.0);
        let key = |h: &mut Harness, ch: char| {
            h.root.event(
                &mut h.state,
                &InputEvent::Key(frust::authoring::KeyEvent {
                    key: frust::authoring::Key::Character(ch.to_string()),
                    modifiers: frust::authoring::Modifiers::default(),
                    repeat: false,
                }),
            );
        };
        key(&mut h, 'a');
        key(&mut h, 'b');
        assert_eq!(h.state, "ab", "each edit reported through on_change");

        // The app rejecting an edit wins: the next rebuild feeds its own value back
        // down, which is the controlled contract the baseline implements.
        h.state = "frozen".to_string();
        h.pass();
        assert_eq!(h.state, "frozen");
    }

    #[test]
    fn a_flush_field_paints_no_chrome_of_its_own() {
        let view: InputView<String> = input("", |_s: &mut String, _t| {}).flush(true);
        let mut w = build(&view);
        let size = layout(&mut w, 200.0);
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, size, FrameTime::ZERO);
        w.paint(&mut ctx, &mut rec);
        assert!(rec.strokes.is_empty(), "no border, no ring");
        assert!(rec.shadows.is_empty(), "no shadow-xs");
        assert!(
            !rec.rrects.is_empty(),
            "the wrapped field still paints its background"
        );
    }

    #[test]
    fn visit_children_publishes_the_wrapped_field() {
        let view: InputView<String> = input("", |_s: &mut String, _t| {});
        let w = build(&view);
        let mut seen = 0usize;
        Widget::visit_children(&w, &mut |_pod| seen += 1);
        assert_eq!(seen, 1);
    }

    #[test]
    fn semantics_forwards_the_wrapped_fields_node() {
        // A closure rather than an `fn` item: `rebuild` wants
        // `FnMut(&mut String)` exactly, which an `fn(&mut str)` cannot satisfy.
        let mut logic = |state: &mut String| {
            input(state.clone(), |s: &mut String, t| *s = t).placeholder("Email")
        };
        let mut root: RenderRoot<String, InputView<String>> = RenderRoot::new();
        let mut state = String::new();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);
        let update = root.semantics();
        assert!(
            update
                .nodes
                .iter()
                .any(|(_, n)| n.role() == frust::authoring::Role::TextInput),
            "the wrapped editable's node reaches the tree through the chrome"
        );
    }
}
