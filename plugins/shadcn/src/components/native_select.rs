//! `native_select`: shadcn's styled `<select>` — **the trigger look only**.
//!
//! Source: `apps/v4/registry/new-york-v4/ui/native-select.tsx` (shadcn/ui v4, rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved 2026-08-17) — a
//! `<select>` styled `h-9 w-full appearance-none rounded-md border border-input
//! bg-transparent px-3 py-2 pr-9 text-sm shadow-xs` (`data-[size=sm]:h-8`), with
//! an absolutely-positioned `ChevronDownIcon` at `right-3.5` in
//! `muted-foreground opacity-50`, the shared focus-visible/aria-invalid blocks,
//! and `dark:hover:bg-input/50`.
//!
//! # Why trigger-only, and what that means for the caller
//!
//! Upstream renders a real `<select>`: the browser owns the popup, its keyboard
//! model and its platform look. There is no drawn analog of that popup here — the
//! whole point of the element is that the *host* draws it — so this component
//! deliberately ports **only** the closed control: the bordered box, the current
//! option's label, and the chevron. Activating it (pointer release inside, or
//! Space/Enter while focused) fires
//! [`on_click`](fn@native_select), and the app decides what opens.
//!
//! An app that wants a drawn, in-tree dropdown wants
//! [`select`](crate::components::select) instead, whose trigger reuses this look.
//!
//! # Controlled
//!
//! `selected` is an index into `options`; the widget never moves it. With
//! `None` (or an out-of-range index) the placeholder is shown in
//! `muted-foreground`, mirroring an empty `<select>` with a placeholder option.

use std::rc::Rc;

use frust::authoring::{
    Action, AnyView, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color,
    ErasedCallback, EventCtx, EventResult, InputEvent, Key, LayoutCtx, NamedKey, PaintCtx,
    PaintScene, Point, PointerPhase, Role, SemanticsCtx, Size, ThemeTextColor, View, Widget, any,
    build_child, erase_callback, rebuild_child, route_event_single, teardown_child, visit_children,
};
use frust::{Theme, text};

use crate::components::input::{
    FALLBACK, FieldChrome, input_border, paint_field_border, resolve_field_border,
};
use crate::hit::{inside, presses};
use crate::style;

/// Right inset of the chevron's box: `right-3.5`.
const CHEVRON_RIGHT: f64 = 14.0;
/// Opacity the chevron is painted at on top of its `muted-foreground` color:
/// `opacity-50`.
const CHEVRON_OPACITY: f32 = 0.5;
/// Alpha of the dark-mode hover wash: `dark:hover:bg-input/50`.
const DARK_HOVER_ALPHA: f32 = 0.5;

/// The edge length of the box lucide draws its glyphs in (`viewBox="0 0 24 24"`)
/// — the denominator every coordinate below is normalized against.
const LUCIDE_VIEWBOX: f64 = 24.0;
/// Lucide's own stroke width, in `viewBox` units (`stroke-width="2"`).
const LUCIDE_STROKE: f64 = 2.0;

/// The control's height variants: `default` (`h-9`) and `sm`
/// (`data-[size=sm]:h-8`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NativeSelectSize {
    /// `h-9` — the default.
    #[default]
    Default,
    /// `data-[size=sm]:h-8`.
    Sm,
}

impl NativeSelectSize {
    /// This size's control height, in logical px.
    pub fn height(self) -> f64 {
        match self {
            NativeSelectSize::Default => style::HEIGHT_DEFAULT,
            NativeSelectSize::Sm => style::HEIGHT_SM,
        }
    }
}

/// Paint a lucide chevron centered on `center`, `extent` px on a side, rotated
/// `angle` radians clockwise.
///
/// Lucide's `chevron-down` is the polyline `m6 9 6 6 6-6` on a 24-unit viewBox
/// with a 2-unit stroke — three points and a width, all scaled by
/// `extent / 24` here. `angle` is what lets a disclosure chevron rotate
/// (`[&[data-state=open]>svg]:rotate-180`) and a horizontal one point sideways,
/// so no second glyph is needed.
///
/// Lives in this module because the closed select is the catalog's first chevron;
/// `accordion`, `collapsible` and `pagination` read it from here rather than each
/// re-deriving the same three points.
pub(crate) fn draw_chevron(
    scene: &mut dyn PaintScene,
    center: Point,
    extent: f64,
    angle: f64,
    color: Color,
) {
    let scale = extent / LUCIDE_VIEWBOX;
    let (sin, cos) = angle.sin_cos();
    // The three polyline points, viewBox-relative to its center (12, 12).
    let points = [(-6.0, -3.0), (0.0, 3.0), (6.0, -3.0)];
    let mut path = BezPath::new();
    for (i, (x, y)) in points.into_iter().enumerate() {
        let (x, y) = (x * scale, y * scale);
        let p = Point::new(x * cos - y * sin, x * sin + y * cos);
        if i == 0 {
            path.move_to(p);
        } else {
            path.line_to(p);
        }
    }
    scene.stroke_path(center, &path, LUCIDE_STROKE * scale, &Brush::Solid(color));
}

/// The `muted-foreground` token: themed `on_surface_variant`, else the fallback
/// table.
fn muted_foreground(theme: Option<&Theme>) -> Color {
    theme.map_or(FALLBACK.muted_foreground, |t| t.scheme().on_surface_variant)
}

/// A declarative shadcn native-select trigger. See the [module docs](self).
pub struct NativeSelectView<State: 'static> {
    options: Vec<String>,
    selected: Option<usize>,
    placeholder: String,
    size: NativeSelectSize,
    invalid: bool,
    disabled: bool,
    on_click: Rc<dyn Fn(&mut State)>,
}

/// Create a controlled native-select trigger over `options`, showing
/// `options[selected]` and firing `on_click` when it is activated (see the
/// [module docs](self) for why activation is all it does).
pub fn native_select<State: 'static, F: Fn(&mut State) + 'static>(
    options: impl IntoIterator<Item = impl Into<String>>,
    selected: Option<usize>,
    on_click: F,
) -> NativeSelectView<State> {
    NativeSelectView {
        options: options.into_iter().map(Into::into).collect(),
        selected,
        placeholder: String::new(),
        size: NativeSelectSize::default(),
        invalid: false,
        disabled: false,
        on_click: Rc::new(on_click),
    }
}

impl<State: 'static> NativeSelectView<State> {
    /// Set the text shown while nothing is selected.
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Set the control height (`default`/`sm`).
    pub fn size(mut self, size: NativeSelectSize) -> Self {
        self.size = size;
        self
    }

    /// Put the control in its `aria-invalid` state.
    pub fn invalid(mut self, invalid: bool) -> Self {
        self.invalid = invalid;
        self
    }

    /// Disable the control: inert, dimmed, [`style::DISABLED_CURSOR`].
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// The label currently displayed, and whether it is the placeholder.
    fn label(&self) -> (String, bool) {
        match self.selected.and_then(|i| self.options.get(i)) {
            Some(option) => (option.clone(), false),
            None => (self.placeholder.clone(), true),
        }
    }

    /// The label's text child: `text-sm`, `foreground` for a selected option and
    /// `muted-foreground` for the placeholder.
    fn label_view(&self) -> AnyView<State> {
        let (label, is_placeholder) = self.label();
        let role = if is_placeholder {
            ThemeTextColor::OnSurfaceVariant
        } else {
            ThemeTextColor::OnSurface
        };
        any(text(label)
            .size(style::TEXT_SM as f32)
            .family(crate::tokens::sans_family())
            .themed_role(role))
    }
}

/// The retained widget for a [`NativeSelectView`].
pub struct NativeSelectWidget {
    label: ChildPod,
    /// The option labels, for the semantics node's value/description.
    options: Vec<String>,
    selected: Option<usize>,
    size: NativeSelectSize,
    invalid: bool,
    disabled: bool,
    /// The latched hover flag (self-corrected from `PaintCtx::is_hovered`).
    hovered: bool,
    pressed: bool,
    captured: bool,
    on_click: ErasedCallback,
}

impl<State: 'static> View<State> for NativeSelectView<State> {
    type Element = NativeSelectWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> NativeSelectWidget {
        NativeSelectWidget {
            label: build_child(&self.label_view(), ctx),
            options: self.options.clone(),
            selected: self.selected,
            size: self.size,
            invalid: self.invalid,
            disabled: self.disabled,
            hovered: false,
            pressed: false,
            captured: false,
            on_click: erase_callback(&self.on_click),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut NativeSelectWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(
            &prev.label_view(),
            &self.label_view(),
            &mut element.label,
            ctx,
        );
        if element.size != self.size {
            element.size = self.size;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.invalid != self.invalid
            || element.disabled != self.disabled
            || element.selected != self.selected
        {
            element.invalid = self.invalid;
            element.disabled = self.disabled;
            element.selected = self.selected;
            flags |= ChangeFlags::PAINT;
        }
        element.options = self.options.clone();
        // Closures are not comparable; reinstalling the adapter is cheap.
        element.on_click = erase_callback(&self.on_click);
        flags
    }

    fn teardown(&self, element: &mut NativeSelectWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.label_view(), &mut element.label, ctx);
    }
}

/// Whether `key` activates a focused trigger (`Space`/`Enter`, the keys a native
/// `<select>` opens on).
///
/// Space has no [`NamedKey`] variant — the framework's key vocabulary enumerates
/// only keys with *editing* semantics and delivers everything else as resolved
/// text — so it is matched as the character it produces.
pub(crate) fn activates(key: &Key) -> bool {
    match key {
        Key::Named(NamedKey::Enter) => true,
        Key::Character(text) => text == " ",
        Key::Named(_) => false,
    }
}

impl NativeSelectWidget {
    /// Fire the activation callback and clear the press state.
    fn activate(&mut self, ctx: &mut EventCtx) {
        (self.on_click)(ctx);
        self.pressed = false;
        ctx.request_redraw();
    }
}

impl Widget for NativeSelectWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let height = self.size.height();
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        // `px-3 pr-9`: the label band stops short of the chevron's gutter.
        let left = style::spacing(3.0);
        let right = style::spacing(9.0);
        let label_bc = BoxConstraints::new(
            Size::ZERO,
            Size::new((width - left - right).max(0.0), height),
        );
        let label = self.label.layout_child(ctx, &label_bc);
        self.label
            .set_origin(Point::new(left, ((height - label.height) / 2.0).max(0.0)));
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // `PaintCtx::is_hovered` is authoritative — it is what drops the wash on
        // the frame the pointer moves onto something else, which no event can tell
        // this widget about.
        self.hovered = ctx.is_hovered();
        let chrome = FieldChrome {
            focused: ctx.has_focus() && !self.disabled,
            invalid: self.invalid,
            disabled: self.disabled,
        };
        let (origin, size) = (ctx.origin(), ctx.size());
        let theme = Theme::from_paint_ctx(ctx);
        let resolved = resolve_field_border(theme, chrome);
        let chevron = style::disabled_tint(
            style::with_alpha(muted_foreground(theme), CHEVRON_OPACITY),
            self.disabled,
        );
        // `dark:hover:bg-input/50` — a hover wash the light class list does not
        // have at all (there is nothing to tint over `bg-transparent`).
        let hover_wash = (self.hovered && !self.disabled && style::is_dark(theme))
            .then(|| style::with_alpha(input_border(theme), DARK_HOVER_ALPHA));

        style::draw_shadow(
            scene,
            origin,
            size,
            resolved.radius,
            style::SHADOW_XS,
            theme,
        );
        if let Some(wash) = hover_wash {
            scene.fill_rounded_rect(origin, size, resolved.radius, wash);
        }
        self.label.paint_child(ctx, scene);
        draw_chevron(
            scene,
            Point::new(
                origin.x + size.width - CHEVRON_RIGHT - style::ICON_SIZE / 2.0,
                origin.y + size.height / 2.0,
            ),
            style::ICON_SIZE,
            0.0,
            chevron,
        );
        paint_field_border(scene, origin, size, resolved);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // The label is a text leaf and consumes nothing, but a container still
        // forwards (a broadcast must reach it, and it keeps the pod live).
        let routed = route_event_single(&mut self.label, ctx, event);
        if event.is_broadcast() {
            return routed;
        }
        if let InputEvent::Key(key) = event {
            if self.disabled || !activates(&key.key) {
                return EventResult::Ignored;
            }
            self.activate(ctx);
            return EventResult::Handled;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        if self.disabled {
            // Still asks for the not-allowed cursor: upstream keeps pointer events
            // on a disabled `<select>` (`disabled:cursor-not-allowed`, no
            // `pointer-events-none`).
            if matches!(p.phase, PointerPhase::Move) && inside(p.position, ctx.size()) {
                ctx.set_cursor(style::DISABLED_CURSOR);
            }
            return EventResult::Ignored;
        }
        match p.phase {
            PointerPhase::Down => {
                if !presses(p) {
                    return EventResult::Ignored;
                }
                self.pressed = true;
                self.captured = true;
                ctx.capture_pointer();
                // Focus is what makes the ring appear (and what routes Space/Enter
                // here).
                ctx.request_focus();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if !self.captured {
                    let over = inside(p.position, ctx.size());
                    if over {
                        ctx.claim_hover();
                        ctx.set_cursor(style::ACTIVE_CURSOR);
                    }
                    if self.hovered != over {
                        self.hovered = over;
                        ctx.request_redraw();
                    }
                    return EventResult::Ignored;
                }
                ctx.set_cursor(style::ACTIVE_CURSOR);
                let over = inside(p.position, ctx.size());
                if self.pressed != over {
                    self.pressed = over;
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.captured = false;
                if inside(p.position, ctx.size()) {
                    self.activate(ctx);
                }
                self.pressed = false;
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                // A Cancel arm never touches app state — flags and a redraw only.
                self.captured = false;
                self.pressed = false;
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let value = self
            .selected
            .and_then(|i| self.options.get(i))
            .cloned()
            .unwrap_or_default();
        ctx.push_container(
            Role::ComboBox,
            |node| {
                node.set_value(value.clone());
                if self.disabled {
                    node.set_disabled();
                } else {
                    node.add_action(Action::Click);
                }
            },
            |ctx| self.label.semantics_child(ctx),
        );
    }

    visit_children!(label);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{KeyEvent, Modifiers, PointerButton, PointerEvent, Rect, Shape};
    use frust::{Brightness, CursorIcon, FrameTime};
    use frust_core::RenderRoot;
    use std::any::Any;

    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<(Rect, f64, Color)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn fill_rounded_rect(&mut self, origin: Point, size: Size, radius: f64, color: Color) {
            self.rrects.push((origin, size, radius, color));
        }
        fn stroke_path(&mut self, origin: Point, path: &BezPath, width: f64, brush: &Brush) {
            let color = match brush {
                Brush::Solid(c) => *c,
                _ => Color::TRANSPARENT,
            };
            self.strokes
                .push((path.bounding_box() + origin.to_vec2(), width, color));
        }
    }

    impl Recorder {
        fn border(&self) -> Color {
            self.strokes
                .iter()
                .find(|(_, w, _)| *w == style::BORDER_WIDTH)
                .expect("border")
                .2
        }

        fn ring(&self) -> Option<Color> {
            self.strokes
                .iter()
                .find(|(_, w, _)| *w == style::FOCUS_RING_WIDTH)
                .map(|(_, _, c)| *c)
        }

        /// The chevron stroke: the only one whose width is neither the border's
        /// nor the ring's.
        fn chevron(&self) -> (Rect, f64, Color) {
            *self
                .strokes
                .iter()
                .find(|(_, w, _)| *w != style::BORDER_WIDTH && *w != style::FOCUS_RING_WIDTH)
                .expect("chevron")
        }
    }

    const WINDOW: Size = Size::new(200.0, 60.0);

    #[derive(Default)]
    struct Clicks {
        count: u32,
    }

    struct Harness {
        root: RenderRoot<Clicks, NativeSelectView<Clicks>>,
        state: Clicks,
        tcx: TextContext,
        selected: Option<usize>,
        disabled: bool,
    }

    impl Harness {
        fn new(selected: Option<usize>, disabled: bool) -> Self {
            let mut h = Harness {
                root: RenderRoot::new(),
                state: Clicks::default(),
                tcx: TextContext::new(),
                selected,
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
            let (selected, disabled) = (self.selected, self.disabled);
            let mut logic = move |_s: &mut Clicks| {
                native_select::<Clicks, _>(["Light", "Dark"], selected, |s: &mut Clicks| {
                    s.count += 1
                })
                .placeholder("Theme")
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

        fn pointer(&mut self, phase: PointerPhase, x: f64, y: f64) -> bool {
            self.root
                .event(
                    &mut self.state,
                    &InputEvent::Pointer(PointerEvent {
                        phase,
                        position: Point::new(x, y),
                        button: PointerButton::Primary,
                    }),
                )
                .needs_redraw
        }

        fn key(&mut self, key: Key) {
            self.root.event(
                &mut self.state,
                &InputEvent::Key(KeyEvent {
                    key,
                    modifiers: Modifiers::default(),
                    repeat: false,
                }),
            );
        }
    }

    fn build<S: 'static>(view: &NativeSelectView<S>) -> NativeSelectWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    #[test]
    fn the_trigger_is_h9_and_h8_at_sm_with_the_label_inside_the_gutters() {
        let view: NativeSelectView<Clicks> =
            native_select(["Light"], Some(0), |_s: &mut Clicks| {});
        let mut w = build(&view);
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let size = w.layout(&mut lctx, &BoxConstraints::loose(WINDOW));
        assert_eq!(size.height, style::HEIGHT_DEFAULT);
        assert_eq!(w.label.origin().x, style::spacing(3.0), "px-3");
        assert!(
            w.label.size().width <= size.width - style::spacing(3.0) - style::spacing(9.0),
            "the label stops short of the pr-9 chevron gutter"
        );

        let small: NativeSelectView<Clicks> =
            native_select(["Light"], Some(0), |_s: &mut Clicks| {}).size(NativeSelectSize::Sm);
        let mut w = build(&small);
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        assert_eq!(
            w.layout(&mut lctx, &BoxConstraints::loose(WINDOW)).height,
            style::HEIGHT_SM
        );
    }

    #[test]
    fn the_chrome_is_a_bordered_box_with_a_muted_chevron_at_right_3_5() {
        let mut h = Harness::new(Some(0), false);
        let rec = h.paint();
        assert_eq!(rec.border(), FALLBACK.input);
        let (bbox, width, color) = rec.chevron();
        assert_eq!(color.components[3], CHEVRON_OPACITY, "opacity-50");
        assert_eq!(
            color.components[..3],
            FALLBACK.muted_foreground.components[..3]
        );
        assert!((width - LUCIDE_STROKE * style::ICON_SIZE / LUCIDE_VIEWBOX).abs() < 1e-9);
        // Centered in the pr-9 gutter: its right edge sits `right-3.5` in.
        let expected_center = WINDOW.width - CHEVRON_RIGHT - style::ICON_SIZE / 2.0;
        assert!((bbox.center().x - expected_center).abs() < 1e-9);
    }

    #[test]
    fn hover_claims_the_link_latches_and_washes_only_in_dark_mode() {
        let mut h = Harness::new(Some(0), false);
        h.theme(Brightness::Dark);
        assert!(h.paint().rrects.is_empty(), "no wash at rest");

        // The claim asks for no frame; the latch's changed-return is what does.
        assert!(h.pointer(PointerPhase::Move, 100.0, 18.0), "entry repaints");
        let dark = crate::theme().with_brightness(Brightness::Dark);
        let wash = style::with_alpha(dark.scheme().outline_variant, DARK_HOVER_ALPHA);
        assert_eq!(h.paint().rrects.first().map(|r| r.3), Some(wash));
        assert!(
            !h.pointer(PointerPhase::Move, 110.0, 18.0),
            "an unchanged latch asks for nothing"
        );

        // Moving off ends the link; the authoritative paint-time read is what
        // clears the flag, since no event reaches a widget the pointer left.
        h.pointer(PointerPhase::Move, 100.0, 55.0);
        assert!(h.paint().rrects.is_empty());

        // Light mode has no hover class at all.
        h.theme(Brightness::Light);
        h.pointer(PointerPhase::Move, 100.0, 18.0);
        assert!(h.paint().rrects.is_empty(), "no light-mode hover wash");
    }

    #[test]
    fn activation_fires_on_up_inside_never_on_down_and_never_outside() {
        let mut h = Harness::new(Some(0), false);
        h.pointer(PointerPhase::Down, 100.0, 18.0);
        assert_eq!(h.state.count, 0, "never on down");
        h.pointer(PointerPhase::Up, 100.0, 18.0);
        assert_eq!(h.state.count, 1);

        h.pointer(PointerPhase::Down, 100.0, 18.0);
        h.pointer(PointerPhase::Up, 100.0, 500.0);
        assert_eq!(h.state.count, 1, "release outside does not fire");

        h.pointer(PointerPhase::Down, 100.0, 18.0);
        h.pointer(PointerPhase::Cancel, 100.0, 18.0);
        assert_eq!(h.state.count, 1, "a cancelled press does not fire");
    }

    #[test]
    fn a_focused_trigger_rings_and_activates_on_space_or_enter() {
        let mut h = Harness::new(Some(0), false);
        h.theme(Brightness::Light);
        h.pointer(PointerPhase::Down, 100.0, 18.0);
        h.pointer(PointerPhase::Up, 100.0, 18.0);
        let theme = crate::theme().with_brightness(Brightness::Light);
        let ring = style::ring_color(None, Some(&theme));
        let rec = h.paint();
        assert_eq!(rec.border(), ring);
        assert_eq!(
            rec.ring(),
            Some(style::with_alpha(ring, style::FOCUS_RING_OPACITY))
        );

        let before = h.state.count;
        // Space arrives as the character it produces (see `activates`).
        h.key(Key::Character(" ".to_string()));
        h.key(Key::Named(NamedKey::Enter));
        assert_eq!(h.state.count, before + 2);
        // An unrelated key does nothing.
        h.key(Key::Named(NamedKey::Tab));
        assert_eq!(h.state.count, before + 2);
    }

    #[test]
    fn a_disabled_trigger_is_inert_dimmed_and_asks_not_allowed() {
        let mut h = Harness::new(Some(0), true);
        let rec = h.paint();
        assert_eq!(rec.border().components[3], style::DISABLED_OPACITY);
        assert_eq!(
            rec.chevron().2.components[3],
            CHEVRON_OPACITY * style::DISABLED_OPACITY,
            "the chevron dims on top of its own opacity-50"
        );
        h.pointer(PointerPhase::Down, 100.0, 18.0);
        h.pointer(PointerPhase::Up, 100.0, 18.0);
        assert_eq!(h.state.count, 0);
        h.pointer(PointerPhase::Move, 100.0, 18.0);
        assert_eq!(h.root.cursor(), CursorIcon::NotAllowed);
        h.key(Key::Named(NamedKey::Enter));
        assert_eq!(h.state.count, 0);
    }

    #[test]
    fn the_cursor_over_an_enabled_trigger_is_the_pointer() {
        let mut h = Harness::new(Some(0), false);
        h.pointer(PointerPhase::Move, 100.0, 18.0);
        assert_eq!(h.root.cursor(), style::ACTIVE_CURSOR);
    }

    #[test]
    fn an_unselected_trigger_shows_the_placeholder_and_reports_an_empty_value() {
        let view: NativeSelectView<Clicks> =
            native_select(["Light", "Dark"], None, |_s: &mut Clicks| {}).placeholder("Theme");
        assert_eq!(view.label(), ("Theme".to_string(), true));
        let selected: NativeSelectView<Clicks> =
            native_select(["Light", "Dark"], Some(1), |_s: &mut Clicks| {});
        assert_eq!(selected.label(), ("Dark".to_string(), false));
        // Out of range falls back to the placeholder rather than panicking.
        let stale: NativeSelectView<Clicks> =
            native_select(["Light"], Some(9), |_s: &mut Clicks| {}).placeholder("Theme");
        assert!(stale.label().1);
    }

    #[test]
    fn semantics_is_a_combobox_carrying_the_selected_option() {
        fn logic(_s: &mut Clicks) -> NativeSelectView<Clicks> {
            native_select(["Light", "Dark"], Some(1), |_s: &mut Clicks| {})
        }
        let mut root: RenderRoot<Clicks, NativeSelectView<Clicks>> = RenderRoot::new();
        let mut state = Clicks::default();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::ComboBox)
            .expect("a ComboBox node");
        assert_eq!(node.value(), Some("Dark"));
        assert!(node.supports_action(Action::Click));
        assert!(!node.children().is_empty(), "the label is a child node");
    }
}
