//! Ports shadcn/ui's **Toggle** (a two-state pressable button) from
//! `tmp/ui/apps/v4/registry/new-york-v4/ui/toggle.tsx` (rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved 2026-08-17).
//!
//! The upstream `cva` has two axes, and they become
//! [`ToggleVariant`]/[`ToggleSize`]:
//!
//! | class | here |
//! |---|---|
//! | `rounded-md` | `ShadcnRadius::md` via [`ShadcnTokens::resolve_radius`](crate::ShadcnTokens::resolve_radius) |
//! | `text-sm font-medium` | [`style::TEXT_SM`] at `FontWeight::MEDIUM` |
//! | `hover:bg-muted hover:text-muted-foreground` | [`ToggleVariant::Default`]'s hover pair |
//! | `variant=outline`: `border border-input shadow-xs hover:bg-accent hover:text-accent-foreground` | [`ToggleVariant::Outline`] |
//! | `data-[state=on]:bg-accent data-[state=on]:text-accent-foreground` | the pressed look, both variants |
//! | `h-9 min-w-9 px-2` / `h-8 min-w-8 px-1.5` / `h-10 min-w-10 px-2.5` | [`ToggleSize`]'s three rungs |
//! | `focus-visible:border-ring` + `ring-[3px] ring-ring/50` | [`style::focus_border`] + [`style::draw_focus_ring`] |
//! | `disabled:pointer-events-none disabled:opacity-50` | 50% opacity, inert, **and no cursor of its own** |
//!
//! # `on` beats `hover`
//!
//! Both `data-[state=on]:bg-accent` and `hover:bg-muted` can apply at once, and
//! which one a browser paints depends on Tailwind's variant ordering. This port
//! resolves it deliberately: **the pressed state wins**, so hovering a pressed
//! toggle never dims it back toward the resting look.
//!
//! # Controlled, never self-mutating
//!
//! `pressed` is a prop. A release inside the control reports
//! `on_pressed_change(state, !pressed)`; the widget's own `pressed` changes only
//! when the next `rebuild` feeds the app-confirmed value back down.
//!
//! # Hover: claim, latch, self-correct
//!
//! This is the first component in the catalog with real hover chrome, so it runs
//! the whole three-part contract (`docs/CODE_STANDARDS.md`'s Interaction
//! Semantics): it claims the link from its uncaptured `Move` arm on *every*
//! qualifying move, latches the same hit test into [`ToggleWidget::hovered`] with
//! the redraw gated on that flag actually changing, and re-syncs the flag from
//! `PaintCtx::is_hovered()` every paint — the authoritative read, and the only
//! thing that fixes the flag when the pointer leaves (no event ever says it did).
//!
//! `disabled:pointer-events-none` means a disabled toggle claims nothing and asks
//! for no cursor at all: it is not "a control that refuses", it is not a pointer
//! target (see [`style::DISABLED_CURSOR`]'s own note on the two disabled kinds).
//!
//! # The label is shaped once and re-brushed per paint
//!
//! The label's color depends on hover, which only ever requests a *repaint* — so
//! baking the color into the shaped run (what `frust::text` does at layout time)
//! would leave a hover color change one relayout behind. Instead the label is a
//! [`crate::text::LabelRun`]: shaped once with a sentinel color, then re-brushed
//! per paint, which keeps the shape cache keyed on geometry alone.

use std::rc::Rc;

use frust::Theme;
use frust::authoring::{
    Action, BoxConstraints, Brush, BuildCtx, ChangeFlags, Color, EventCtx, EventResult, InputEvent,
    Key, KeyEvent, LayoutCtx, NamedKey, PaintCtx, PaintScene, Point, PointerPhase, Rect, Role,
    RoundedRect, SemanticsCtx, Shape, Size, Toggled, View, Widget, erase_callback_arg,
    text::{FontWeight, TextStyle},
};

use crate::hit::{inside, presses};
use crate::style::{self, PATH_TOLERANCE, precedence_fill, precedence_ink};
use crate::text::{LabelRun, SHAPING_INK};
use crate::tokens::ShadcnTokens;

/// Unthemed fallback resting ink — the `neutral` preset's light `--foreground`.
const FALLBACK_FOREGROUND: Color = Color::from_rgb8(0x0A, 0x0A, 0x0A);
/// Unthemed fallback `--muted` (the default variant's hover fill).
const FALLBACK_MUTED: Color = Color::from_rgb8(0xF5, 0xF5, 0xF5);
/// Unthemed fallback `--muted-foreground`.
const FALLBACK_MUTED_FOREGROUND: Color = Color::from_rgb8(0x73, 0x73, 0x73);
/// Unthemed fallback `--accent` (the pressed fill).
const FALLBACK_ACCENT: Color = Color::from_rgb8(0xF5, 0xF5, 0xF5);
/// Unthemed fallback `--accent-foreground` (the pressed ink).
const FALLBACK_ACCENT_FOREGROUND: Color = Color::from_rgb8(0x17, 0x17, 0x17);
/// Unthemed fallback `--input` (the outline variant's border).
const FALLBACK_INPUT: Color = Color::from_rgb8(0xE5, 0xE5, 0xE5);

/// The toggle's `variant` axis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToggleVariant {
    /// `variant="default"`: no border, no shadow, transparent until hovered or
    /// pressed.
    #[default]
    Default,
    /// `variant="outline"`: a 1px `input` border plus `shadow-xs`, and an
    /// `accent` hover fill instead of `muted`.
    Outline,
}

/// The toggle's `size` axis: height, minimum width and horizontal padding.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToggleSize {
    /// `size="default"`: `h-9 min-w-9 px-2`.
    #[default]
    Default,
    /// `size="sm"`: `h-8 min-w-8 px-1.5`.
    Sm,
    /// `size="lg"`: `h-10 min-w-10 px-2.5`.
    Lg,
}

impl ToggleSize {
    /// The control height (`h-*`); the minimum width (`min-w-*`) is the same
    /// number in every rung, so it reads off this too.
    pub fn height(self) -> f64 {
        match self {
            ToggleSize::Default => style::HEIGHT_DEFAULT,
            ToggleSize::Sm => style::HEIGHT_SM,
            ToggleSize::Lg => style::HEIGHT_LG,
        }
    }

    /// Horizontal padding (`px-*`).
    pub fn padding_x(self) -> f64 {
        match self {
            ToggleSize::Default => style::spacing(2.0),
            ToggleSize::Sm => style::spacing(1.5),
            ToggleSize::Lg => style::spacing(2.5),
        }
    }
}

/// A view-held, typed change callback (erased on build).
type OnPressedChange<State> = Rc<dyn Fn(&mut State, bool)>;

/// A declarative shadcn toggle. See the [module docs](self).
pub struct ToggleView<State: 'static> {
    label: String,
    pressed: bool,
    variant: ToggleVariant,
    size: ToggleSize,
    disabled: bool,
    on_pressed_change: OnPressedChange<State>,
}

/// Create a toggle labelled `label` reflecting `pressed`, reporting
/// `on_pressed_change(state, !pressed)` on a release inside its bounds — a
/// **controlled** component (see the [module docs](self)).
pub fn toggle<State: 'static, F: Fn(&mut State, bool) + 'static>(
    label: impl Into<String>,
    pressed: bool,
    on_pressed_change: F,
) -> ToggleView<State> {
    ToggleView {
        label: label.into(),
        pressed,
        variant: ToggleVariant::default(),
        size: ToggleSize::default(),
        disabled: false,
        on_pressed_change: Rc::new(on_pressed_change),
    }
}

impl<State: 'static> ToggleView<State> {
    /// Pick the `variant` axis.
    pub fn variant(mut self, variant: ToggleVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Pick the `size` axis.
    pub fn size(mut self, size: ToggleSize) -> Self {
        self.size = size;
        self
    }

    /// Disable the control: 50% opacity, inert, and no pointer target at all
    /// (`disabled:pointer-events-none`).
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// The label's text style: the theme's own (Inter) family at the catalog's
/// `text-sm`/`font-medium`, or the bundled sans stack when no theme is threaded.
///
/// The size comes from [`style::TEXT_SM`] rather than a type-scale slot, per the
/// catalog's type rule (the scale carries the family; a shadcn component sizes its
/// own text from the Tailwind step its class list names).
fn label_style(theme: Option<&Theme>) -> TextStyle {
    let family = theme.map_or_else(crate::tokens::sans_family, |t| {
        t.type_scale.label_large.family.clone()
    });
    TextStyle {
        family,
        weight: FontWeight::MEDIUM,
        ..TextStyle::new(style::TEXT_SM as f32, SHAPING_INK)
    }
}

/// The resolved fill/ink pair for the toggle's current state, plus the outline
/// variant's border.
struct ToggleColors {
    /// The pressed fill (`data-[state=on]:bg-accent`).
    on_fill: Color,
    /// The pressed ink (`data-[state=on]:text-accent-foreground`).
    on_ink: Color,
    /// The hover fill: `muted` (default variant) or `accent` (outline).
    hover_fill: Color,
    /// The hover ink: `muted-foreground` (default) or `accent-foreground`
    /// (outline).
    hover_ink: Color,
    /// The resting ink (inherited `foreground`).
    ink: Color,
    /// The outline variant's border (`border-input`).
    border: Color,
}

/// Resolve the palette for `variant`, falling back to the `neutral` preset's
/// light values with no theme threaded.
fn resolve_colors(theme: Option<&Theme>, variant: ToggleVariant) -> ToggleColors {
    let (accent, accent_ink, muted, muted_ink, ink, border) = match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            (
                // shadcn's `--accent`/`--accent-foreground` ride
                // `primary_container`/`on_primary_container` (the token fold's
                // documented choice — shadcn has no M3 container pair).
                scheme.primary_container,
                scheme.on_primary_container,
                scheme.surface_container_highest,
                scheme.on_surface_variant,
                scheme.on_surface,
                scheme.outline_variant,
            )
        }
        None => (
            FALLBACK_ACCENT,
            FALLBACK_ACCENT_FOREGROUND,
            FALLBACK_MUTED,
            FALLBACK_MUTED_FOREGROUND,
            FALLBACK_FOREGROUND,
            FALLBACK_INPUT,
        ),
    };
    let (hover_fill, hover_ink) = match variant {
        ToggleVariant::Default => (muted, muted_ink),
        ToggleVariant::Outline => (accent, accent_ink),
    };
    ToggleColors {
        on_fill: accent,
        on_ink: accent_ink,
        hover_fill,
        hover_ink,
        ink,
        border,
    }
}

/// Whether `key` activates the control: `Space` (arriving as typed text — there
/// is no `NamedKey::Space`) or `Enter`.
fn is_activation_key(key: &KeyEvent) -> bool {
    match &key.key {
        Key::Named(NamedKey::Enter) => true,
        Key::Character(text) => text == " ",
        _ => false,
    }
}

impl<State: 'static> View<State> for ToggleView<State> {
    type Element = ToggleWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ToggleWidget {
        ToggleWidget {
            label: LabelRun::new(self.label.clone()),
            label_text: self.label.clone(),
            pressed: self.pressed,
            variant: self.variant,
            size: self.size,
            disabled: self.disabled,
            hovered: false,
            captured: false,
            on_pressed_change: erase_callback_arg(&self.on_pressed_change),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ToggleWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_pressed_change = erase_callback_arg(&self.on_pressed_change);
        let mut flags = ChangeFlags::NONE;
        if prev.label != self.label {
            element.label.set_content(self.label.clone());
            element.label_text = self.label.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.pressed != self.pressed {
            // The app is the source of truth: adopt the confirmed value.
            element.pressed = self.pressed;
            flags |= ChangeFlags::PAINT;
        }
        if prev.variant != self.variant {
            element.variant = self.variant;
            flags |= ChangeFlags::PAINT;
        }
        if prev.size != self.size {
            element.size = self.size;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.disabled != self.disabled {
            element.disabled = self.disabled;
            if self.disabled {
                element.captured = false;
                element.hovered = false;
            }
            flags |= ChangeFlags::PAINT;
        }
        flags
    }
}

/// The retained widget for a [`ToggleView`].
pub struct ToggleWidget {
    label: LabelRun,
    /// The label text, retained for the semantics node's accessible name.
    label_text: String,
    /// The app-confirmed pressed value (source of truth; adopted on `rebuild`).
    pressed: bool,
    variant: ToggleVariant,
    size: ToggleSize,
    disabled: bool,
    /// The latched hover flag: set from the uncaptured `Move` hit test (redraw
    /// gated on its change) and re-synced from `PaintCtx::is_hovered()` every
    /// paint — see the [module docs](self).
    hovered: bool,
    /// Armed by a `Down` inside, cleared on `Up`/`Cancel`. The source authors no
    /// `:active` rule, so nothing paints differently while armed.
    captured: bool,
    on_pressed_change: frust::authoring::ErasedArgCallback<bool>,
}

impl ToggleWidget {
    /// Set the latched hover flag, reporting whether it changed (the gate the
    /// redraw rides).
    fn set_hovered(&mut self, hovered: bool) -> bool {
        let changed = self.hovered != hovered;
        self.hovered = hovered;
        changed
    }

    /// The fill to paint under the current state, if any — see
    /// [`precedence_fill`].
    fn fill(&self, colors: &ToggleColors) -> Option<Color> {
        precedence_fill(
            self.pressed,
            self.hovered,
            colors.on_fill,
            colors.hover_fill,
        )
    }

    /// The label ink under the current state, on the same precedence.
    fn ink(&self, colors: &ToggleColors) -> Color {
        precedence_ink(
            self.pressed,
            self.hovered,
            colors.on_ink,
            colors.hover_ink,
            colors.ink,
        )
    }
}

impl Widget for ToggleWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let style = label_style(Theme::from_layout_ctx(ctx));
        let label = self.label.layout(ctx, &style);
        let height = self.size.height();
        // `min-w-9`/`min-w-8`/`min-w-10` equal the row height in every rung, so an
        // icon-only toggle stays square.
        let width = (label.width + self.size.padding_x() * 2.0).max(height);
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // `PaintCtx::is_hovered` is authoritative: it fixes the latched flag in
        // the case no event can (the pointer left, or a container cleared the
        // link).
        self.hovered = ctx.is_hovered() && !self.disabled;

        let theme = Theme::from_paint_ctx(ctx);
        let colors = resolve_colors(theme, self.variant);
        let radius = ShadcnTokens::resolve_radius(None, theme).md;
        let focused = ctx.has_focus();
        let origin = ctx.origin();
        let size = ctx.size();
        let tint = |color: Color| style::disabled_tint(color, self.disabled);

        if self.variant == ToggleVariant::Outline {
            style::draw_shadow(scene, origin, size, radius, style::SHADOW_XS, theme);
        }
        if let Some(fill) = self.fill(&colors) {
            scene.fill_rounded_rect(origin, size, radius, tint(fill));
        }
        // The outline variant always strokes its border; the default variant has
        // none, and only grows one while focused (`focus-visible:border-ring`).
        let border = match (self.variant, focused) {
            (ToggleVariant::Outline, _) => Some(style::focus_border(colors.border, focused, theme)),
            (ToggleVariant::Default, true) => Some(style::ring_color(None, theme)),
            (ToggleVariant::Default, false) => None,
        };
        if let Some(border) = border {
            let inset = style::BORDER_WIDTH / 2.0;
            let outline = RoundedRect::from_rect(
                Rect::from_origin_size(Point::ORIGIN, size).inset(-inset),
                (radius - inset).max(0.0),
            );
            scene.stroke_path(
                origin,
                &Shape::to_path(&outline, PATH_TOLERANCE),
                style::BORDER_WIDTH,
                &Brush::Solid(tint(border)),
            );
        }

        // `items-center justify-center`.
        let label = self.label.size();
        let label_origin = Point::new(
            origin.x + (size.width - label.width) / 2.0,
            origin.y + (size.height - label.height) / 2.0,
        );
        self.label
            .paint(label_origin, tint(self.ink(&colors)), scene);

        if focused {
            style::draw_focus_ring(scene, origin, size, radius, style::ring_color(None, theme));
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // `disabled:pointer-events-none`: a disabled toggle is not a pointer
        // target at all — no claim, no cursor, no capture.
        if self.disabled {
            return EventResult::Ignored;
        }
        match event {
            InputEvent::Key(key) => {
                if !is_activation_key(key) {
                    return EventResult::Ignored;
                }
                // Report the requested value; never self-toggle.
                (self.on_pressed_change)(ctx, !self.pressed);
                EventResult::Handled
            }
            InputEvent::Pointer(p) => {
                let size = ctx.size();
                match p.phase {
                    PointerPhase::Down => {
                        if !presses(p) || !inside(p.position, size) {
                            return EventResult::Ignored;
                        }
                        self.captured = true;
                        ctx.capture_pointer();
                        ctx.request_focus();
                        ctx.request_redraw();
                        EventResult::Handled
                    }
                    PointerPhase::Move => {
                        if !self.captured {
                            // Claim on every qualifying move (the claim is
                            // per-pass), latch the same hit test, and repaint only
                            // when the latch actually flipped — that gated redraw
                            // is the frame that makes hover chrome appear.
                            let over = inside(p.position, size);
                            if over {
                                ctx.claim_hover();
                                ctx.set_cursor(style::ACTIVE_CURSOR);
                            }
                            if self.set_hovered(over) {
                                ctx.request_redraw();
                            }
                            // Watching a move is not consuming it.
                            return EventResult::Ignored;
                        }
                        // Captured: re-ask so the shape survives a drag outside
                        // the control.
                        ctx.set_cursor(style::ACTIVE_CURSOR);
                        EventResult::Handled
                    }
                    PointerPhase::Up => {
                        if !self.captured {
                            return EventResult::Ignored;
                        }
                        self.captured = false;
                        if inside(p.position, size) {
                            (self.on_pressed_change)(ctx, !self.pressed);
                        }
                        EventResult::Handled
                    }
                    PointerPhase::Cancel => {
                        if !self.captured {
                            return EventResult::Ignored;
                        }
                        // Internal flags only.
                        self.captured = false;
                        EventResult::Handled
                    }
                }
            }
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // ARIA's toggle button is `role="button"` + `aria-pressed`, and accesskit
        // has no dedicated toggle-button role: `Toggled` carries the pressed state
        // on a Button node.
        ctx.push_node(Role::Button, |node| {
            node.set_label(self.label_text.as_str());
            node.set_toggled(Toggled::from(self.pressed));
            if self.disabled {
                node.set_disabled();
            } else {
                node.add_action(Action::Click);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{
        BezPath, EventOutcome, Modifiers, PointerButton, PointerEvent, SemanticsUpdate,
    };
    use frust::{CursorIcon, FrameTime};
    use std::any::Any;

    /// Records the fills, stroked paths, shadows and glyph-run brushes this
    /// widget emits.
    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<(Rect, f64, Color)>,
        shadows: Vec<(Point, Size, f64, f64, Color)>,
        glyph_inks: Vec<Color>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn stroke_path(&mut self, origin: Point, path: &BezPath, width: f64, brush: &Brush) {
            let bbox = path.bounding_box() + origin.to_vec2();
            let color = match brush {
                Brush::Solid(c) => *c,
                _ => Color::TRANSPARENT,
            };
            self.strokes.push((bbox, width, color));
        }
        fn draw_shadow(&mut self, o: Point, s: Size, radius: f64, std_dev: f64, color: Color) {
            self.shadows.push((o, s, radius, std_dev, color));
        }
        fn draw_glyph_run(&mut self, run: frust::authoring::scene::GlyphRun) {
            if let Brush::Solid(color) = run.brush {
                self.glyph_inks.push(color);
            }
        }
    }

    #[derive(Default)]
    struct Toggles {
        last: Option<bool>,
        count: u32,
    }

    fn view(pressed: bool, variant: ToggleVariant, size: ToggleSize) -> ToggleView<Toggles> {
        toggle::<Toggles, _>("Bold", pressed, |s: &mut Toggles, v: bool| {
            s.last = Some(v);
            s.count += 1;
        })
        .variant(variant)
        .size(size)
    }

    /// Build + lay out a toggle, returning it alongside its resolved size.
    fn laid_out(
        pressed: bool,
        variant: ToggleVariant,
        size: ToggleSize,
        theme: Option<&Theme>,
    ) -> (ToggleWidget, Size) {
        let mut counter = 0u64;
        let mut w = View::<Toggles>::build(
            &view(pressed, variant, size),
            &mut BuildCtx::new(&mut counter),
        );
        let mut tcx = TextContext::new();
        let mut ctx =
            LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), theme.map(|t| t as &dyn Any));
        let resolved = w.layout(
            &mut ctx,
            &BoxConstraints::new(Size::ZERO, Size::new(400.0, 400.0)),
        );
        (w, resolved)
    }

    fn paint(w: &mut ToggleWidget, size: Size, theme: Option<&Theme>) -> Recorder {
        let mut rec = Recorder::default();
        let mut ctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, size).with_theme(t),
            None => PaintCtx::new(Point::ZERO, size),
        };
        w.paint(&mut ctx, &mut rec);
        rec
    }

    fn pointer(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn space() -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Character(" ".into()),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    fn dispatch(
        w: &mut ToggleWidget,
        state: &mut Toggles,
        size: Size,
        event: &InputEvent,
    ) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, size);
        w.event(&mut ctx, event)
    }

    // ---- Axes / layout ----------------------------------------------------

    #[test]
    fn the_size_axis_carries_the_authored_heights_and_padding() {
        assert_eq!(ToggleSize::Default.height(), style::HEIGHT_DEFAULT);
        assert_eq!(ToggleSize::Sm.height(), style::HEIGHT_SM);
        assert_eq!(ToggleSize::Lg.height(), style::HEIGHT_LG);
        assert_eq!(ToggleSize::Default.padding_x(), 8.0);
        assert_eq!(ToggleSize::Sm.padding_x(), 6.0);
        assert_eq!(ToggleSize::Lg.padding_x(), 10.0);
        assert_eq!(ToggleSize::default(), ToggleSize::Default);
        assert_eq!(ToggleVariant::default(), ToggleVariant::Default);
    }

    #[test]
    fn layout_is_the_label_plus_padding_floored_at_the_min_width() {
        let (w, size) = laid_out(false, ToggleVariant::Default, ToggleSize::Default, None);
        assert_eq!(size.height, style::HEIGHT_DEFAULT);
        assert!(
            (size.width - (w.label.size().width + 16.0)).abs() < 1e-9,
            "label + px-2 on both sides"
        );

        // An empty label collapses onto `min-w-9`, keeping the control square.
        let mut counter = 0u64;
        let empty: ToggleView<Toggles> = toggle("", false, |_s: &mut Toggles, _v| {});
        let mut w = View::<Toggles>::build(&empty, &mut BuildCtx::new(&mut counter));
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), None);
        let size = w.layout(
            &mut ctx,
            &BoxConstraints::new(Size::ZERO, Size::new(400.0, 400.0)),
        );
        assert_eq!(
            size,
            Size::new(style::HEIGHT_DEFAULT, style::HEIGHT_DEFAULT)
        );
    }

    // ---- Paint ------------------------------------------------------------

    #[test]
    fn a_resting_default_variant_paints_only_its_label() {
        let (mut w, size) = laid_out(false, ToggleVariant::Default, ToggleSize::Default, None);
        let rec = paint(&mut w, size, None);
        assert!(rec.rrects.is_empty(), "`bg-transparent`");
        assert!(rec.strokes.is_empty(), "no border on the default variant");
        assert!(rec.shadows.is_empty(), "no shadow on the default variant");
        assert_eq!(rec.glyph_inks, vec![FALLBACK_FOREGROUND]);
    }

    #[test]
    fn the_pressed_state_paints_the_accent_fill_and_accent_ink() {
        let theme = crate::theme();
        let scheme = theme.scheme();
        let (mut w, size) = laid_out(
            true,
            ToggleVariant::Default,
            ToggleSize::Default,
            Some(&theme),
        );
        let rec = paint(&mut w, size, Some(&theme));
        assert_eq!(rec.rrects.len(), 1);
        let (_, painted, radius, fill) = rec.rrects[0];
        assert_eq!(painted, size);
        assert_eq!(radius, crate::ShadcnRadius::shadcn().md, "`rounded-md`");
        assert_eq!(fill, scheme.primary_container, "shadcn's `--accent`");
        assert_eq!(rec.glyph_inks, vec![scheme.on_primary_container]);
    }

    #[test]
    fn the_outline_variant_strokes_an_input_border_over_a_shadow() {
        let theme = crate::theme();
        let (mut w, size) = laid_out(false, ToggleVariant::Outline, ToggleSize::Sm, Some(&theme));
        let rec = paint(&mut w, size, Some(&theme));
        assert_eq!(rec.shadows.len(), 1, "`shadow-xs`");
        assert_eq!(rec.shadows[0].3, style::SHADOW_XS.std_dev);
        assert_eq!(rec.strokes.len(), 1);
        assert_eq!(rec.strokes[0].1, style::BORDER_WIDTH);
        assert_eq!(rec.strokes[0].2, theme.scheme().outline_variant);
        assert!(rec.rrects.is_empty(), "still `bg-transparent` at rest");
    }

    #[test]
    fn hover_fills_muted_on_the_default_variant_and_accent_on_the_outline_one() {
        let theme = crate::theme();
        let scheme = theme.scheme();
        let (mut default, size) = laid_out(
            false,
            ToggleVariant::Default,
            ToggleSize::Default,
            Some(&theme),
        );
        default.hovered = true;
        let rec = paint(&mut default, size, Some(&theme));
        // Paint self-corrects the flag from `PaintCtx::is_hovered`, which is
        // false in a bare context — so this exercises the resolver directly.
        let colors = resolve_colors(Some(&theme), ToggleVariant::Default);
        assert_eq!(colors.hover_fill, scheme.surface_container_highest);
        assert_eq!(colors.hover_ink, scheme.on_surface_variant);
        assert!(rec.rrects.is_empty(), "the paint-time self-correction won");

        let outline = resolve_colors(Some(&theme), ToggleVariant::Outline);
        assert_eq!(outline.hover_fill, scheme.primary_container);
        assert_eq!(outline.hover_ink, scheme.on_primary_container);
    }

    #[test]
    fn pressed_wins_over_hover() {
        let theme = crate::theme();
        let colors = resolve_colors(Some(&theme), ToggleVariant::Default);
        let (mut w, _) = laid_out(
            true,
            ToggleVariant::Default,
            ToggleSize::Default,
            Some(&theme),
        );
        w.hovered = true;
        assert_eq!(w.fill(&colors), Some(colors.on_fill));
        assert_eq!(w.ink(&colors), colors.on_ink);
    }

    #[test]
    fn disabled_paint_halves_every_painted_alpha() {
        let theme = crate::theme();
        let (mut enabled, size) = laid_out(
            true,
            ToggleVariant::Outline,
            ToggleSize::Default,
            Some(&theme),
        );
        let on = paint(&mut enabled, size, Some(&theme));

        let mut counter = 0u64;
        let disabled_view = view(true, ToggleVariant::Outline, ToggleSize::Default).disabled(true);
        let mut disabled = View::<Toggles>::build(&disabled_view, &mut BuildCtx::new(&mut counter));
        let mut tcx = TextContext::new();
        let mut lctx =
            LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), Some(&theme as &dyn Any));
        let size = disabled.layout(
            &mut lctx,
            &BoxConstraints::new(Size::ZERO, Size::new(400.0, 400.0)),
        );
        let off = paint(&mut disabled, size, Some(&theme));

        assert_eq!(
            off.rrects[0].3.components[3],
            on.rrects[0].3.components[3] * style::DISABLED_OPACITY
        );
        assert_eq!(
            off.glyph_inks[0].components[3],
            on.glyph_inks[0].components[3] * style::DISABLED_OPACITY
        );
    }

    // ---- Interaction ------------------------------------------------------

    #[test]
    fn up_inside_reports_the_requested_value_without_self_mutating() {
        let (mut w, size) = laid_out(false, ToggleVariant::Default, ToggleSize::Default, None);
        let mut state = Toggles::default();
        let hit = (size.width / 2.0, size.height / 2.0);
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, hit.0, hit.1),
        );
        assert!(w.captured);
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Up, hit.0, hit.1),
        );
        assert_eq!(state.last, Some(true));
        assert!(!w.pressed, "the app owns `pressed`");
        assert!(!w.captured);
    }

    #[test]
    fn up_outside_and_cancel_never_fire() {
        let (mut w, size) = laid_out(true, ToggleVariant::Default, ToggleSize::Default, None);
        let mut state = Toggles::default();
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, 5.0, 5.0),
        );
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Up, size.width + 40.0, 5.0),
        );
        assert_eq!(state.count, 0);

        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, 5.0, 5.0),
        );
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Cancel, 5.0, 5.0),
        );
        assert_eq!(state.count, 0);
        assert!(!w.captured);
    }

    #[test]
    fn space_activates_and_a_disabled_toggle_is_inert_to_everything() {
        let (mut w, size) = laid_out(true, ToggleVariant::Default, ToggleSize::Default, None);
        let mut state = Toggles::default();
        dispatch(&mut w, &mut state, size, &space());
        assert_eq!(state.last, Some(false));

        let mut counter = 0u64;
        let disabled_view = view(false, ToggleVariant::Default, ToggleSize::Default).disabled(true);
        let mut disabled = View::<Toggles>::build(&disabled_view, &mut BuildCtx::new(&mut counter));
        for event in [
            space(),
            pointer(PointerPhase::Down, 5.0, 5.0),
            pointer(PointerPhase::Move, 5.0, 5.0),
        ] {
            assert_eq!(
                dispatch(&mut disabled, &mut state, size, &event),
                EventResult::Ignored
            );
        }
        assert!(!disabled.hovered, "`pointer-events-none`: never hovered");
        assert_eq!(state.count, 1);
    }

    #[test]
    fn the_hover_latch_repaints_on_entry_and_exit_but_not_within() {
        let (mut w, size) = laid_out(false, ToggleVariant::Default, ToggleSize::Default, None);
        let mut state = Toggles::default();
        let state_any: &mut dyn Any = &mut state;

        let mut enter = EventCtx::new(state_any, Point::ZERO, size);
        w.event(&mut enter, &pointer(PointerPhase::Move, 5.0, 5.0));
        assert!(w.hovered);
        assert!(enter.needs_redraw(), "hover gain owes the frame");

        let state_any: &mut dyn Any = &mut state;
        let mut within = EventCtx::new(state_any, Point::ZERO, size);
        w.event(&mut within, &pointer(PointerPhase::Move, 6.0, 6.0));
        assert!(w.hovered);
        assert!(
            !within.needs_redraw(),
            "an unchanged latch asks for nothing"
        );

        let state_any: &mut dyn Any = &mut state;
        let mut leave = EventCtx::new(state_any, Point::ZERO, size);
        w.event(
            &mut leave,
            &pointer(PointerPhase::Move, size.width + 30.0, 5.0),
        );
        assert!(!w.hovered);
        assert!(leave.needs_redraw(), "hover loss owes one too");
    }

    #[test]
    fn paint_self_corrects_a_stale_hover_flag() {
        let (mut w, size) = laid_out(false, ToggleVariant::Default, ToggleSize::Default, None);
        w.hovered = true;
        // A bare `PaintCtx` reports no hover link, which is exactly the "the
        // pointer left and no event said so" case.
        let _ = paint(&mut w, size, None);
        assert!(!w.hovered);
    }

    #[test]
    fn rebuild_adopts_the_confirmed_value_and_a_size_change_relayouts() {
        let mut counter = 0u64;
        let prev = view(false, ToggleVariant::Default, ToggleSize::Default);
        let mut w = View::<Toggles>::build(&prev, &mut BuildCtx::new(&mut counter));
        let next = view(true, ToggleVariant::Outline, ToggleSize::Lg);
        let flags =
            View::<Toggles>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(w.pressed);
        assert_eq!(w.variant, ToggleVariant::Outline);
        assert_eq!(w.size, ToggleSize::Lg);
        assert!(flags.needs_layout());
    }

    // ---- Root-driven: hover chrome, focus ring, cursor, semantics -----------

    /// One toggle under a real `RenderRoot` — hover, focus and the cursor are all
    /// recorded by the root's event pass, so nothing else can exercise them.
    struct Harness {
        root: frust_core::RenderRoot<Toggles, ToggleView<Toggles>>,
        state: Toggles,
        tcx: TextContext,
    }

    impl Harness {
        fn new(pressed: bool, disabled: bool) -> Self {
            let mut h = Harness {
                root: frust_core::RenderRoot::new(),
                state: Toggles::default(),
                tcx: TextContext::new(),
            };
            h.root.set_theme(Box::new(crate::theme()));
            let mut logic = move |_s: &mut Toggles| {
                view(pressed, ToggleVariant::Default, ToggleSize::Default).disabled(disabled)
            };
            h.root.rebuild(&mut logic, &mut h.state);
            h.root
                .layout_with_text(Size::new(300.0, 200.0), &mut h.tcx as &mut dyn Any);
            h
        }

        fn dispatch(&mut self, event: &InputEvent) -> EventOutcome {
            self.root.event(&mut self.state, event)
        }

        fn paint(&mut self) -> Recorder {
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, FrameTime::ZERO);
            rec
        }

        fn semantics(&self) -> SemanticsUpdate {
            self.root.semantics()
        }
    }

    #[test]
    fn hovering_paints_the_muted_fill_on_the_frame_the_latch_asks_for() {
        let mut h = Harness::new(false, false);
        assert!(h.paint().rrects.is_empty(), "nothing filled at rest");

        let gain = h.dispatch(&pointer(PointerPhase::Move, 20.0, 18.0));
        assert!(gain.needs_redraw, "entering the toggle repaints");
        let rec = h.paint();
        assert_eq!(rec.rrects.len(), 1, "the hover fill");
        assert_eq!(
            rec.rrects[0].3,
            crate::theme().scheme().surface_container_highest,
            "`hover:bg-muted`"
        );
        assert_eq!(
            rec.glyph_inks[0],
            crate::theme().scheme().on_surface_variant,
            "`hover:text-muted-foreground`"
        );

        // Moving within changes nothing.
        let settled = h.dispatch(&pointer(PointerPhase::Move, 24.0, 18.0));
        assert!(!settled.needs_redraw);

        // Leaving drops the chrome again.
        h.dispatch(&pointer(PointerPhase::Move, 280.0, 180.0));
        assert!(h.paint().rrects.is_empty());
    }

    #[test]
    fn focus_paints_a_ring_and_a_ring_colored_border_on_the_default_variant() {
        let mut h = Harness::new(false, false);
        h.dispatch(&pointer(PointerPhase::Down, 20.0, 18.0));
        assert!(h.root.is_focus_active());
        let rec = h.paint();
        let ring = style::ring_color(None, Some(&crate::theme()));
        assert_eq!(rec.strokes.len(), 2, "border + focus ring");
        assert_eq!(rec.strokes[0].2, ring, "`focus-visible:border-ring`");
        assert_eq!(rec.strokes[1].1, style::FOCUS_RING_WIDTH);
        assert_eq!(rec.strokes[1].2.components[3], style::FOCUS_RING_OPACITY);
    }

    #[test]
    fn a_move_resolves_the_pointer_cursor_and_a_disabled_toggle_asks_for_none() {
        let mut h = Harness::new(false, false);
        h.dispatch(&pointer(PointerPhase::Move, 20.0, 18.0));
        assert_eq!(h.root.cursor(), style::ACTIVE_CURSOR);

        let mut disabled = Harness::new(false, true);
        disabled.dispatch(&pointer(PointerPhase::Move, 20.0, 18.0));
        assert_eq!(
            disabled.root.cursor(),
            CursorIcon::Default,
            "`pointer-events-none` asks for no shape, not `not-allowed`"
        );
    }

    #[test]
    fn semantics_reports_a_button_node_carrying_the_pressed_state() {
        let h = Harness::new(true, false);
        let update = h.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Button)
            .expect("a Role::Button node");
        assert_eq!(node.label(), Some("Bold"));
        assert_eq!(node.toggled(), Some(Toggled::True));
        assert!(node.supports_action(Action::Click));
    }
}
