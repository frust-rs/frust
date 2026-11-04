//! [`card_switcher`]: the Glyph skin over `frust::motion::card_switcher`, plus
//! the stage's action bar ([`switcher_bar`]) and its outline chips
//! ([`switcher_chip`]).
//!
//! Use the skin in a Glyph app: it applies the Glyph stage tokens (the
//! [`card_stage`] shadow, radius 24, label and dots gaps 30/22; the scrim is
//! the theme's `scrim` role at 55 %, which the pattern resolves) and wires
//! [`page_dots`] into the pattern's `dots_for` slot. Reach for the raw pattern
//! when the stage needs another design system's look. The live-content swap
//! handshake (tap, `on_select`, `expanding`, swap) is the pattern's; see
//! [`CardSwitcherView::expanding`].
//!
//! **Dots.** The dots index is the strip index: `count` is peers + 1, and the
//! live card's dot is `live_index`. The dots follow drags, taps and expansions,
//! one `Housekeeping` pass behind the strip (the pattern's contract).
//!
//! **Back.** [`GlyphCardSwitcher::back`] takes part in back arbitration while
//! the mounted stage is open, through a [`SwitcherBack`] the app creates once;
//! its claim rules are on that method. The Glyph component set is indexed in
//! `docs/PLUGINS_ARCHITECTURE.md`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use frust::authoring::text::{
    FontFamily, FontWeight, GenericSlot, TextContext, TextLayout, TextStyle,
};
use frust::authoring::{
    Action, AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerPhase, Role, SemanticsCtx, View, ViewSeq,
    Widget, build_child, rebuild_children, route_event, teardown_child, visit_children,
};
use frust::motion::{CardSwitcherView, CardSwitcherWidget, card_switcher as stage};
use frust::{BackFallback, BackFallbackView, IconData, ShapeScale, Theme, back_fallback};
use kurbo::{Affine, BezPath, Point, Rect, RoundedRect, Shape, Size, Vec2};
use peniko::{Brush, Color};

pub use frust::motion::{PeerCard, peer_card};

use crate::badge::LayoutGuard;
use crate::page_dots::page_dots;
use crate::press::presses;
use crate::tokens::scales::card_stage;

/// The card's on-screen corner radius (the design's 24 px; Glyph's shape
/// scale tops out at 22, so no shape token carries it).
const CARD_RADIUS: f64 = 24.0;
/// Clear space between a label row and its card (design: 30 px).
const LABEL_GAP: f64 = 30.0;
/// Clear space between the card and the page dots (design: 22 px).
const DOTS_GAP: f64 = 22.0;

/// A chip's height, and an icon-only chip's width (design: 48 px).
const CHIP_SIZE: f64 = 48.0;
/// A labelled chip's minimum width (design: 128 px).
const LABELLED_MIN_WIDTH: f64 = 128.0;
/// Painted icon side.
const CHIP_ICON: f64 = 20.0;
/// A labelled chip's horizontal padding and icon-to-label gap.
const CHIP_PAD_X: f64 = 16.0;
const CHIP_ICON_GAP: f64 = 8.0;
/// Outline stroke width.
const CHIP_BORDER: f64 = 1.0;
/// Unthemed chip radius: Glyph's `--radius-md`.
const CHIP_RADIUS_FALLBACK: f64 = 10.0;
/// Label size (Glyph's button label, 12.5 px), the unthemed type-ramp fallback.
const CHIP_LABEL_SIZE: f32 = 12.5;
/// Opacity of the ink tint laid over a pressed chip.
const PRESSED_TINT: f32 = 0.10;
/// Disabled ink and outline opacities (`on_surface`).
const DISABLED_INK_ALPHA: f32 = 0.38;
const DISABLED_OUTLINE_ALPHA: f32 = 0.12;
const PATH_TOLERANCE: f64 = 0.1;

/// The bar's lift over the scrim: `on_surface` at 7 %.
const BAR_LIFT: f32 = 0.07;
/// Space between chips, and the bar's padding around the chip row.
const BAR_GAP: f64 = 12.0;
const BAR_PAD_X: f64 = 16.0;
const BAR_PAD_Y: f64 = 12.0;

// Unthemed fallbacks (Glyph dark values).
const FALLBACK_INK: Color = Color::from_rgb8(0xf2, 0xea, 0xd9); // fg
const FALLBACK_OUTLINE: Color = Color::from_rgb8(0x3e, 0x3f, 0x44); // border-bright

// --- Stage -------------------------------------------------------------------

/// The back-press handle for [`GlyphCardSwitcher::back`]: create it once (in
/// `Component::init`, under the reactive runtime) and pass it on every build.
/// Its `BackFallback` claims a press only while the mounted stage is open.
#[derive(Clone)]
pub struct SwitcherBack {
    fallback: BackFallback,
    open: Rc<Cell<bool>>,
}

/// The claim flag a stage's `on_close` clears, filled in by
/// [`GlyphCardSwitcher::back`] (which may run after `on_close` was set).
type ClaimSlot = Rc<RefCell<Option<Rc<Cell<bool>>>>>;

impl SwitcherBack {
    /// A handle whose back press runs `on_back`; it should close the stage.
    pub fn new(on_back: impl Fn() + 'static) -> Self {
        let open = Rc::new(Cell::new(false));
        Self {
            fallback: BackFallback::new(claims_while(&open), on_back),
            open,
        }
    }
}

/// The fallback's interest: whether the mounted stage is open.
fn claims_while(open: &Rc<Cell<bool>>) -> impl Fn() -> bool + 'static {
    let open = Rc::clone(open);
    move || open.get()
}

/// A Glyph-skinned card-switcher stage. See the [module docs](self).
pub struct GlyphCardSwitcher<State: 'static> {
    open: bool,
    stage: CardSwitcherView<State>,
    back: Option<BackFallbackView<BackMount>>,
    /// The handle's claim flag, written by `build`/`rebuild`/`teardown`.
    claim: Option<Rc<Cell<bool>>>,
    /// Shared with the `on_close` wrapper so a close clears the claim at once.
    close_slot: ClaimSlot,
}

/// Show `live` as a full-size page, or (`open`) shrunk into a card on the Glyph
/// switcher stage, with page dots below the strip.
pub fn card_switcher<State: 'static>(
    open: bool,
    live: impl View<State>,
) -> GlyphCardSwitcher<State> {
    let shadow = card_stage().shadow_dark;
    // The Glyph token stores the blur as CSS px; the pattern takes the
    // Gaussian std-dev, half of it.
    GlyphCardSwitcher {
        open,
        stage: stage(open, live)
            .card_radius(CARD_RADIUS)
            .label_gap(LABEL_GAP)
            .dots_gap(DOTS_GAP)
            .shadow(
                Color::BLACK.multiply_alpha(shadow.color_alpha),
                shadow.y_offset,
                shadow.blur_std_dev / 2.0,
            )
            .dots_for(|count, current| page_dots::<State>(count, current)),
        back: None,
        claim: None,
        close_slot: Rc::default(),
    }
}

impl<State: 'static> GlyphCardSwitcher<State> {
    /// The other cards on the strip, in strip order without the live card.
    pub fn peers(mut self, peers: impl IntoIterator<Item = PeerCard<State>>) -> Self {
        self.stage = self.stage.peers(peers);
        self
    }

    /// The live card's strip position (default `0`); its page dot.
    pub fn live_index(mut self, index: usize) -> Self {
        self.stage = self.stage.live_index(index);
        self
    }

    /// The label row drawn above the live card.
    pub fn live_label(mut self, label: impl View<State>) -> Self {
        self.stage = self.stage.live_label(label);
        self
    }

    /// Called with the **peer** index of a tapped peer once it is centred.
    pub fn on_select(mut self, on_select: impl Fn(&mut State, usize) + 'static) -> Self {
        self.stage = self.stage.on_select(on_select);
        self
    }

    /// Called with the **strip** index of the centred card when it changes.
    pub fn on_center_changed(mut self, on_change: impl Fn(&mut State, usize) + 'static) -> Self {
        self.stage = self.stage.on_center_changed(on_change);
        self
    }

    /// Called when the open stage asks to close; the app must set `open` false.
    /// The back claim is released before this runs, so declining to close
    /// leaves the stage open and unclaimed until the next rebuild.
    pub fn on_close(mut self, on_close: impl Fn(&mut State) + 'static) -> Self {
        let slot = Rc::clone(&self.close_slot);
        self.stage = self.stage.on_close(move |state| {
            if let Some(claim) = slot.borrow().as_ref() {
                claim.set(false);
            }
            on_close(state);
        });
        self
    }

    /// The action bar, flush with the bottom edge (see [`switcher_bar`]).
    pub fn action_bar(mut self, bar: impl View<State>) -> Self {
        self.stage = self.stage.action_bar(bar);
        self
    }

    /// The peer card growing into the live page, if any.
    pub fn expanding(mut self, peer: Option<usize>) -> Self {
        self.stage = self.stage.expanding(peer);
        self
    }

    /// The card's scale at full open (default `0.62`; the design's tablet
    /// value is `0.58`).
    pub fn card_scale(mut self, scale: f64) -> Self {
        self.stage = self.stage.card_scale(scale);
        self
    }

    /// Take part in back arbitration through `back`: its `frust::back_fallback`
    /// claims a press only while the mounted stage is open and runs the app's
    /// stateless `on_back`, which should do what `on_close` does (typically an
    /// `RwSignal` set; `BackFallback` callbacks see no `&mut State`). Call it
    /// from `Component::build` on every build. `build`/`rebuild` publish the
    /// view's `open` as the claim, `teardown` (or a rebuild without `.back`)
    /// clears it, and a fired `on_close` clears it before the app rebuilds. A
    /// close that bypasses `on_close` is seen at the next build: a press before
    /// that still runs `on_back` (idempotent for a close), and a press between
    /// `open` turning true and that build is not claimed.
    pub fn back(mut self, back: &SwitcherBack) -> Self {
        self.claim = Some(Rc::clone(&back.open));
        *self.close_slot.borrow_mut() = Some(Rc::clone(&back.open));
        self.back = Some(back_fallback(&back.fallback, BackMount));
        self
    }
}

impl<State: 'static> GlyphCardSwitcher<State> {
    /// Publish this view's `open` as the handle's claim (a no-op without `.back`).
    fn write_claim(&self) {
        if let Some(claim) = &self.claim {
            claim.set(self.open);
        }
    }
}

impl<State: 'static> View<State> for GlyphCardSwitcher<State> {
    type Element = CardSwitcherWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> CardSwitcherWidget {
        if let Some(back) = &self.back {
            View::<State>::build(back, ctx);
        }
        self.write_claim();
        self.stage.build(ctx)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut CardSwitcherWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        match (&self.back, &prev.back) {
            (Some(next), Some(old)) => {
                View::<State>::rebuild(next, old, &mut BackMountWidget, ctx);
            }
            (Some(next), None) => {
                View::<State>::build(next, ctx);
            }
            (None, Some(old)) => View::<State>::teardown(old, &mut BackMountWidget, ctx),
            (None, None) => {}
        }
        match &prev.claim {
            Some(old) if self.claim.is_none() => old.set(false),
            _ => self.write_claim(),
        }
        self.stage.rebuild(&prev.stage, element, ctx)
    }

    fn teardown(&self, element: &mut CardSwitcherWidget, ctx: &mut BuildCtx<'_>) {
        self.stage.teardown(element, ctx);
        if let Some(claim) = &self.claim {
            claim.set(false);
        }
        if let Some(back) = &self.back {
            View::<State>::teardown(back, &mut BackMountWidget, ctx);
        }
    }
}

/// The empty child a [`SwitcherBack`]'s `back_fallback` wrapper counts its
/// mounts over; the stage itself stays the element.
pub(crate) struct BackMount;

/// The stateless widget behind [`BackMount`].
pub(crate) struct BackMountWidget;

impl<State: 'static> View<State> for BackMount {
    type Element = BackMountWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> BackMountWidget {
        BackMountWidget
    }

    fn rebuild(&self, _: &Self, _: &mut BackMountWidget, _: &mut BuildCtx<'_>) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

impl Widget for BackMountWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::ZERO)
    }

    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
}

// --- Action bar --------------------------------------------------------------

/// The stage's action bar. See [`switcher_bar`].
pub struct SwitcherBar<State: 'static> {
    children: Vec<AnyView<State>>,
}

/// The bottom strip of the stage: full width, `on_surface` at 7 % over the
/// scrim, its chips centred in a row `12` px apart. It pads by the window's
/// bottom safe-area inset and consumes it, so pass it to
/// [`GlyphCardSwitcher::action_bar`] bare. A press on the strip but off a chip
/// is swallowed rather than read as a scrim tap.
pub fn switcher_bar<State: 'static, M>(children: impl ViewSeq<State, M>) -> SwitcherBar<State> {
    let mut erased = Vec::new();
    children.extend_views(&mut erased);
    SwitcherBar { children: erased }
}

impl<State: 'static> View<State> for SwitcherBar<State> {
    type Element = SwitcherBarWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> SwitcherBarWidget {
        SwitcherBarWidget {
            pods: self.children.iter().map(|c| build_child(c, ctx)).collect(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SwitcherBarWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        rebuild_children(
            &prev.children,
            &self.children,
            &mut element.pods,
            ctx,
            |view| view,
            |_| None,
        )
    }

    fn teardown(&self, element: &mut SwitcherBarWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.children.iter().zip(element.pods.iter_mut()) {
            teardown_child(view, pod, ctx);
        }
    }
}

/// The retained widget for a [`SwitcherBar`].
pub struct SwitcherBarWidget {
    pods: Vec<ChildPod>,
}

impl Widget for SwitcherBarWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let insets = ctx.window_insets();
        let bottom = insets.padding().bottom;
        let max = bc.max();
        let loose = BoxConstraints::loose(Size::new(max.width, CHIP_SIZE.max(max.height)));
        let pods = &mut self.pods;
        let sizes: Vec<Size> =
            ctx.with_window_insets(insets.consuming(false, false, false, true), |ctx| {
                pods.iter_mut()
                    .map(|pod| pod.layout_child(ctx, &loose))
                    .collect()
            });
        let gaps = BAR_GAP * sizes.len().saturating_sub(1) as f64;
        let row = sizes.iter().map(|s| s.width).sum::<f64>() + gaps;
        let row_height = sizes.iter().map(|s| s.height).fold(0.0, f64::max);
        let width = if max.width.is_finite() {
            max.width
        } else {
            row + 2.0 * BAR_PAD_X
        };
        let size = bc.constrain(Size::new(width, row_height + 2.0 * BAR_PAD_Y + bottom));
        let mut x = ((size.width - row) / 2.0).max(BAR_PAD_X);
        for (pod, child) in self.pods.iter_mut().zip(&sizes) {
            let y = BAR_PAD_Y + (row_height - child.height) / 2.0;
            pod.set_origin(Point::new(x, y));
            x += child.width + BAR_GAP;
        }
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let ink = Theme::from_paint_ctx(ctx).map_or(FALLBACK_INK, |t| t.scheme().on_surface);
        scene.fill_rect(ctx.origin(), ctx.size(), ink.multiply_alpha(BAR_LIFT));
        for pod in &mut self.pods {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let result = route_event(&mut self.pods, ctx, event);
        if result == EventResult::Ignored && matches!(event, InputEvent::Pointer(_)) {
            return EventResult::Handled;
        }
        result
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        for pod in &self.pods {
            pod.semantics_child(ctx);
        }
    }

    visit_children!(pods);
}

// --- Chip --------------------------------------------------------------------

/// A typed press callback (erased on build).
type OnPress<State> = Rc<dyn Fn(&mut State)>;

/// An outline chip for [`switcher_bar`]. See [`switcher_chip`].
pub struct SwitcherChip<State: 'static> {
    icon: IconData,
    on_press: OnPress<State>,
    label: Option<String>,
    semantic_label: Option<String>,
    disabled: bool,
}

/// A 48 × 48 outline chip showing `icon`, running `on_press` on release
/// inside. [`SwitcherChip::label`] makes it the labelled variant (at least
/// 128 px wide). Ink is `on_surface`, the stroke the `outline` role; a press
/// tints the fill at once, so `reduce_motion` has nothing to collapse.
pub fn switcher_chip<State: 'static>(
    icon: impl Into<IconData>,
    on_press: impl Fn(&mut State) + 'static,
) -> SwitcherChip<State> {
    SwitcherChip {
        icon: icon.into(),
        on_press: Rc::new(on_press),
        label: None,
        semantic_label: None,
        disabled: false,
    }
}

impl<State: 'static> SwitcherChip<State> {
    /// Show `label` after the icon (the labelled variant); also its name.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// The accessible name of an icon-only chip (a visible label wins).
    pub fn semantic_label(mut self, label: impl Into<String>) -> Self {
        self.semantic_label = Some(label.into());
        self
    }

    /// Disable the chip: dimmed, no press, no action.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl<State: 'static> View<State> for SwitcherChip<State> {
    type Element = SwitcherChipWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> SwitcherChipWidget {
        SwitcherChipWidget {
            icon: self.icon.clone(),
            path: icon_path(&self.icon),
            label: self.label.clone(),
            semantic_label: self.semantic_label.clone(),
            text: None,
            guard: LayoutGuard::default(),
            text_size: Size::ZERO,
            disabled: self.disabled,
            pressed: false,
            captured: false,
            on_press: frust::authoring::erase_callback(&self.on_press),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SwitcherChipWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_press = frust::authoring::erase_callback(&self.on_press);
        let mut flags = ChangeFlags::NONE;
        if prev.label != self.label || element.disabled != self.disabled {
            element.label = self.label.clone();
            element.text = None;
            element.disabled = self.disabled;
            if self.disabled {
                element.pressed = false;
                element.captured = false;
            }
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.semantic_label != self.semantic_label {
            element.semantic_label = self.semantic_label.clone();
            flags |= ChangeFlags::PAINT;
        }
        if !prev.icon.same(&self.icon) {
            element.icon = self.icon.clone();
            element.path = icon_path(&self.icon);
            flags |= ChangeFlags::PAINT;
        }
        flags
    }
}

/// The retained widget for a [`SwitcherChip`].
pub struct SwitcherChipWidget {
    icon: IconData,
    /// The icon path pre-scaled to `CHIP_ICON`.
    path: BezPath,
    label: Option<String>,
    semantic_label: Option<String>,
    text: Option<TextLayout>,
    guard: LayoutGuard,
    text_size: Size,
    disabled: bool,
    pressed: bool,
    captured: bool,
    on_press: frust::authoring::ErasedCallback,
}

impl SwitcherChipWidget {
    /// `(ink, outline)` for the current state.
    fn colors(&self, theme: Option<&Theme>) -> (Color, Color) {
        let (ink, outline) = theme.map_or((FALLBACK_INK, FALLBACK_OUTLINE), |t| {
            (t.scheme().on_surface, t.scheme().outline)
        });
        if self.disabled {
            (
                ink.multiply_alpha(DISABLED_INK_ALPHA),
                ink.multiply_alpha(DISABLED_OUTLINE_ALPHA),
            )
        } else {
            (ink, outline)
        }
    }

    /// The label's style: the theme's `label_large` role (Glyph's button label).
    fn label_style(theme: Option<&Theme>, color: Color) -> TextStyle {
        match theme {
            Some(t) => TextStyle {
                color,
                ..t.type_scale.label_large.clone()
            },
            None => TextStyle {
                family: FontFamily::stack_with_generic(["IBM Plex Mono"], GenericSlot::Monospace),
                weight: FontWeight::MEDIUM,
                ..TextStyle::new(CHIP_LABEL_SIZE, color)
            },
        }
    }

    fn icon_x(&self, width: f64) -> f64 {
        if self.label.is_some() {
            CHIP_PAD_X
        } else {
            (width - CHIP_ICON) / 2.0
        }
    }
}

impl Widget for SwitcherChipWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let Some(label) = &self.label else {
            self.text_size = Size::ZERO;
            return bc.constrain(Size::new(CHIP_SIZE, CHIP_SIZE));
        };
        let theme = Theme::from_layout_ctx(ctx);
        let style = Self::label_style(theme, self.colors(theme).0);
        if self.text.is_none() || !self.guard.matches(&style, None) {
            let stamp = LayoutGuard::stamp(&style);
            let laid = ctx
                .text_context::<TextContext>()
                .layout(label, &style, None);
            self.text_size = laid.size();
            self.text = Some(laid);
            self.guard.record(&style, None, stamp);
        }
        let content = CHIP_PAD_X * 2.0 + CHIP_ICON + CHIP_ICON_GAP + self.text_size.width;
        bc.constrain(Size::new(content.max(LABELLED_MIN_WIDTH), CHIP_SIZE))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let (ink, outline) = self.colors(theme);
        let (origin, size) = (ctx.origin(), ctx.size());
        let radius = theme.map_or(CHIP_RADIUS_FALLBACK, |t| {
            ShapeScale::resolve(t.shape.medium, size.width, size.height)
        });
        if self.pressed {
            scene.fill_rounded_rect(origin, size, radius, ink.multiply_alpha(PRESSED_TINT));
        }
        let inset = CHIP_BORDER / 2.0;
        let border = RoundedRect::from_rect(
            Rect::from_origin_size(Point::ORIGIN, size).inset(-inset),
            (radius - inset).max(0.0),
        );
        scene.stroke_path(
            origin,
            &border.to_path(PATH_TOLERANCE),
            CHIP_BORDER,
            &Brush::Solid(outline),
        );
        let icon_y = (size.height - CHIP_ICON) / 2.0;
        scene.fill_path(
            origin + Vec2::new(self.icon_x(size.width), icon_y),
            &self.path,
            &Brush::Solid(ink),
        );
        if let Some(text) = &self.text
            && self.label.is_some()
        {
            let x = CHIP_PAD_X + CHIP_ICON + CHIP_ICON_GAP;
            let y = (size.height - self.text_size.height) / 2.0;
            for run in text.to_scene_runs(origin + Vec2::new(x, y)) {
                scene.draw_glyph_run(run);
            }
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if self.disabled {
            return EventResult::Ignored;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        let inside = Rect::from_origin_size(Point::ORIGIN, ctx.size()).contains(p.position);
        match p.phase {
            PointerPhase::Down => {
                if !presses(p) || !inside {
                    return EventResult::Ignored;
                }
                self.pressed = true;
                self.captured = true;
                ctx.capture_pointer();
            }
            PointerPhase::Move => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                if inside == self.pressed {
                    return EventResult::Handled;
                }
                self.pressed = inside;
            }
            PointerPhase::Up | PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.pressed = false;
                self.captured = false;
                if p.phase == PointerPhase::Up && inside {
                    (self.on_press)(ctx);
                }
            }
        }
        ctx.request_redraw();
        EventResult::Handled
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let name = self.label.as_ref().or(self.semantic_label.as_ref());
        ctx.push_node(Role::Button, |node| {
            if let Some(name) = name {
                node.set_label(name.as_str());
            }
            if self.disabled {
                node.set_disabled();
            } else {
                node.add_action(Action::Click);
            }
        });
    }
}

/// The icon's path scaled from its design box to `CHIP_ICON`.
fn icon_path(icon: &IconData) -> BezPath {
    let (path, design) = icon.resolve();
    let scale = if design > 0.0 {
        CHIP_ICON / design
    } else {
        1.0
    };
    Affine::scale(scale) * path
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::FrameTime;
    use frust::authoring::{PointerButton, PointerEvent, WindowEdgeInsets, WindowInsets};
    use frust_reactive::{
        ReactiveRuntime, clear_can_pop_provider, handles_back, push_back_press, set_handles_back,
    };
    use frust_widgets::test_support::leaf;
    use std::any::Any;
    use std::sync::{Arc, Mutex};

    const PHONE: Size = Size::new(402.0, 874.0);
    const TABLET: Size = Size::new(1194.0, 834.0);

    #[derive(Clone, Debug, PartialEq)]
    enum Op {
        Fill(Rect, Color),
        Shadow(Rect, f64, f64, Color),
        Clip(Rect, f64),
        Rounded(Rect, Color),
    }

    #[derive(Default)]
    struct Ops(Vec<Op>);

    impl PaintScene for Ops {
        fn fill_rect(&mut self, o: Point, s: Size, color: Color) {
            self.0.push(Op::Fill(Rect::from_origin_size(o, s), color));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn draw_shadow(&mut self, o: Point, s: Size, radius: f64, std_dev: f64, color: Color) {
            let rect = Rect::from_origin_size(o, s);
            self.0.push(Op::Shadow(rect, radius, std_dev, color));
        }
        fn push_clip_rounded(&mut self, o: Point, s: Size, radius: f64) {
            self.0.push(Op::Clip(Rect::from_origin_size(o, s), radius));
        }
        fn fill_rounded_rect(&mut self, o: Point, s: Size, _radius: f64, color: Color) {
            self.0
                .push(Op::Rounded(Rect::from_origin_size(o, s), color));
        }
    }

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    fn near_rect(a: Rect, b: Rect) -> bool {
        near(a.x0, b.x0) && near(a.y0, b.y0) && near(a.x1, b.x1) && near(a.y1, b.y1)
    }

    fn rgb(c: Color) -> [f32; 3] {
        [c.components[0], c.components[1], c.components[2]]
    }

    /// An open stage of `peers` leaf peers around a leaf live card at strip 1.
    fn stage_view(open: bool, peers: usize) -> GlyphCardSwitcher<()> {
        card_switcher(open, leaf(1.0, 1.0))
            .peers((0..peers).map(|_| peer_card(leaf(1.0, 1.0))))
            .live_index(1)
            .on_close(|_: &mut ()| {})
    }

    /// A built, laid-out and painted stage driven by frame time and events.
    struct Rig {
        w: CardSwitcherWidget,
        size: Size,
        t: f64,
        theme: Theme,
    }

    impl Rig {
        fn new(view: &GlyphCardSwitcher<()>, size: Size) -> Self {
            let mut counter = 0u64;
            let w = View::<()>::build(view, &mut BuildCtx::new(&mut counter));
            let mut rig = Self {
                w,
                size,
                t: 0.0,
                theme: crate::baseline(),
            };
            rig.lay_out();
            rig.frame(0.0);
            rig
        }

        fn lay_out(&mut self) {
            let mut tcx = TextContext::new();
            let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
            self.w.layout(&mut lctx, &BoxConstraints::tight(self.size));
        }

        fn frame(&mut self, dt_ms: f64) -> Vec<Op> {
            self.t += dt_ms;
            let mut ctx =
                PaintCtx::for_test(Point::ZERO, self.size, ft_ms(self.t)).with_theme(&self.theme);
            let mut ops = Ops::default();
            self.w.paint(&mut ctx, &mut ops);
            ops.0
        }

        fn send(&mut self, event: &InputEvent) {
            let mut unit = ();
            let state: &mut dyn Any = &mut unit;
            let mut ctx = EventCtx::new(state, Point::ZERO, self.size);
            self.w.event(&mut ctx, event);
        }

        fn pointer(&mut self, phase: PointerPhase, x: f64) {
            self.send(&ev(phase, x, self.size.height / 2.0));
        }

        /// One app pass: the owed `Housekeeping` event, then a rebuild.
        fn pump(&mut self, prev: GlyphCardSwitcher<()>) -> GlyphCardSwitcher<()> {
            self.send(&InputEvent::Housekeeping);
            let next = stage_view(true, 3);
            let mut counter = 0u64;
            View::<()>::rebuild(&next, &prev, &mut self.w, &mut BuildCtx::new(&mut counter));
            self.lay_out();
            next
        }
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    /// The page dots' fills (6 px tall rounded rects), left to right.
    fn dots(ops: &[Op]) -> Vec<(Rect, Color)> {
        let mut dots: Vec<(Rect, Color)> = ops
            .iter()
            .filter_map(|op| match op {
                Op::Rounded(rect, color) if near(rect.height(), 6.0) => Some((*rect, *color)),
                _ => None,
            })
            .collect();
        dots.sort_by(|a, b| a.0.x0.total_cmp(&b.0.x0));
        dots
    }

    fn active_dot(ops: &[Op], theme: &Theme) -> usize {
        let dots = dots(ops);
        let primary = rgb(theme.scheme().primary);
        dots.iter()
            .position(|(_, c)| rgb(*c) == primary)
            .expect("an active dot in primary")
    }

    #[test]
    fn open_stage_paints_the_glyph_scrim_shadow_radius_and_four_dots() {
        let mut rig = Rig::new(&stage_view(true, 3), PHONE);
        let ops = rig.frame(16.0);
        let scrim = rig.theme.scheme().scrim.multiply_alpha(0.55);
        assert_eq!(
            ops[0],
            Op::Fill(PHONE.to_rect(), scrim),
            "the scrim token at 55 %"
        );
        assert_eq!(rgb(scrim), rgb(crate::tokens::color::dark().scrim));

        let card = Rect::from_center_size(PHONE.to_rect().center(), PHONE * 0.62);
        let spec = card_stage().shadow_dark;
        let ink = Color::BLACK.multiply_alpha(spec.color_alpha);
        let shadow = ops.iter().find_map(|op| match op {
            Op::Shadow(rect, radius, std_dev, color) if *color == ink => {
                Some((*rect, *radius, *std_dev))
            }
            _ => None,
        });
        let (rect, radius, std_dev) = shadow.expect("the centred card's card_stage shadow");
        assert!(
            near_rect(rect, card + Vec2::new(0.0, spec.y_offset)),
            "{rect:?}"
        );
        // The 48 px CSS blur token is a std-dev of 24 in the pattern's unit.
        assert_eq!((radius, std_dev), (CARD_RADIUS, 24.0));
        assert_eq!(std_dev, spec.blur_std_dev / 2.0);
        let clipped = ops
            .iter()
            .any(|op| matches!(op, Op::Clip(rect, r) if near_rect(*rect, card) && *r == 24.0));
        assert!(clipped, "the centred card is clipped at radius 24");

        let dots = dots(&ops);
        assert_eq!(dots.len(), 4, "page_dots with count = peers + 1");
        assert_eq!(active_dot(&ops, &rig.theme), 1, "the live card's dot");
        assert!(
            near(dots[0].0.y0, card.y1 + DOTS_GAP),
            "22 px below the card"
        );
    }

    #[test]
    fn closed_stage_paints_the_page_plainly() {
        let mut rig = Rig::new(&stage_view(false, 3), PHONE);
        let ops = rig.frame(16.0);
        assert!(dots(&ops).is_empty());
        assert!(!ops.iter().any(|op| matches!(op, Op::Shadow(..))));
    }

    #[test]
    fn the_dots_follow_the_strip_to_the_centred_card() {
        let mut view = stage_view(true, 3);
        let mut rig = Rig::new(&view, PHONE);
        // Card 2 peeks in at the right edge; a tap centres it.
        rig.pointer(PointerPhase::Down, 370.0);
        rig.frame(16.0);
        rig.pointer(PointerPhase::Up, 370.0);
        for _ in 0..60 {
            rig.frame(16.0);
            view = rig.pump(view);
        }
        let ops = rig.frame(16.0);
        assert_eq!(active_dot(&ops, &rig.theme), 2, "dots index = strip index");
    }

    #[test]
    fn the_back_interest_follows_the_open_the_stage_was_built_with() {
        let open = Rc::new(Cell::new(false));
        let claims = claims_while(&open);
        assert!(!claims(), "closed: the fallback does not claim");
        open.set(true);
        assert!(claims(), "open: a back press is claimed");
    }

    /// Serializes the back tests: they share `frust-reactive`'s process-wide
    /// back-press counter and `handles_back` flag (the facade's `back_glue`
    /// tests use the same discipline).
    static BACK_LOCK: Mutex<()> = Mutex::new(());

    /// Drop any live can-pop provider and the polled flag, so each back test
    /// starts with nothing claiming. Call under [`BACK_LOCK`] and a runtime.
    fn back_reset() {
        clear_can_pop_provider();
        set_handles_back(false);
    }

    #[test]
    fn an_open_stage_claims_back_and_a_press_runs_on_back_once() {
        let _guard = BACK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        back_reset();

        let fired = Rc::new(Cell::new(0u32));
        let handle = {
            let fired = Rc::clone(&fired);
            SwitcherBack::new(move || fired.set(fired.get() + 1))
        };
        let open = stage_view(true, 3).back(&handle);
        let _rig = Rig::new(&open, PHONE);
        assert!(handles_back(), "built open: the stage claims back");
        assert_eq!(fired.get(), 0, "building alone runs nothing");

        // A press delivered before the next build is consumed by that build's
        // `back`, which routes it to the claiming fallback.
        push_back_press();
        let _next = stage_view(true, 3).back(&handle);
        assert_eq!(fired.get(), 1, "the press ran on_back exactly once");
    }

    #[test]
    fn a_closed_rebuild_releases_the_claim() {
        let _guard = BACK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        back_reset();

        let fired = Rc::new(Cell::new(0u32));
        let handle = {
            let fired = Rc::clone(&fired);
            SwitcherBack::new(move || fired.set(fired.get() + 1))
        };
        let open = stage_view(true, 3).back(&handle);
        let mut rig = Rig::new(&open, PHONE);
        assert!(handles_back(), "built open: the stage claims back");

        let closed = stage_view(false, 3).back(&handle);
        let mut counter = 0u64;
        View::<()>::rebuild(&closed, &open, &mut rig.w, &mut BuildCtx::new(&mut counter));
        assert!(!handles_back(), "rebuilt closed: nothing claims back");

        push_back_press();
        let _next = stage_view(false, 3).back(&handle);
        assert_eq!(fired.get(), 0, "a press while closed is not claimed");
    }

    #[test]
    fn a_press_after_a_closed_rebuild_with_no_further_build_is_not_claimed() {
        let _guard = BACK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        back_reset();

        let fired = Rc::new(Cell::new(0u32));
        let handle = {
            let fired = Rc::clone(&fired);
            SwitcherBack::new(move || fired.set(fired.get() + 1))
        };
        let open = stage_view(true, 3).back(&handle);
        let mut rig = Rig::new(&open, PHONE);
        let closed = stage_view(false, 3).back(&handle);
        let mut counter = 0u64;
        View::<()>::rebuild(&closed, &open, &mut rig.w, &mut BuildCtx::new(&mut counter));

        // The press arrives with no app build in between: it must not be
        // claimed, so `handles_back` stays false and nothing runs.
        push_back_press();
        assert!(!handles_back(), "closed: the press is not claimed");
        assert_eq!(fired.get(), 0, "on_back never ran");
    }

    #[test]
    fn a_press_after_open_flips_false_without_a_rebuild_is_still_claimed() {
        let _guard = BACK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        back_reset();

        let fired = Rc::new(Cell::new(0u32));
        let handle = {
            let fired = Rc::clone(&fired);
            SwitcherBack::new(move || fired.set(fired.get() + 1))
        };
        let open = stage_view(true, 3).back(&handle);
        let _rig = Rig::new(&open, PHONE);
        assert!(handles_back(), "built open: the stage claims back");

        // The app closes the stage itself (its `open` is now false) but no
        // build has run: the documented residual window, so the press is
        // still claimed and `on_back` runs once.
        push_back_press();
        let _next = stage_view(true, 3).back(&handle);
        assert_eq!(fired.get(), 1, "the stale claim ran on_back exactly once");
    }

    #[test]
    fn on_close_clears_the_claim_before_the_app_rebuilds() {
        let _guard = BACK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        back_reset();

        let handle = SwitcherBack::new(|| {});
        let open = stage_view(true, 3).back(&handle);
        let mut rig = Rig::new(&open, PHONE);
        assert!(handles_back(), "built open: the stage claims back");

        // A press on the scrim (outside the card row) asks the stage to close.
        rig.send(&ev(PointerPhase::Down, 5.0, 5.0));
        assert!(!handles_back(), "closing released the claim at once");
    }

    #[test]
    fn tearing_the_stage_down_clears_the_claim() {
        let _guard = BACK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        back_reset();

        let handle = SwitcherBack::new(|| {});
        let open = stage_view(true, 3).back(&handle);
        let mut rig = Rig::new(&open, PHONE);
        assert!(handles_back(), "built open: the stage claims back");

        let mut counter = 0u64;
        View::<()>::teardown(&open, &mut rig.w, &mut BuildCtx::new(&mut counter));
        assert!(!handles_back(), "torn down: nothing claims back");
    }

    #[test]
    fn dropping_back_on_a_rebuild_clears_the_claim() {
        let _guard = BACK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        back_reset();

        let handle = SwitcherBack::new(|| {});
        let open = stage_view(true, 3).back(&handle);
        let mut rig = Rig::new(&open, PHONE);
        let plain = stage_view(true, 3);
        let mut counter = 0u64;
        View::<()>::rebuild(&plain, &open, &mut rig.w, &mut BuildCtx::new(&mut counter));
        assert!(!handles_back(), "rebuilt without .back: no claim");
    }

    #[test]
    fn a_stage_built_without_back_never_claims() {
        let _guard = BACK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        back_reset();

        let fired = Rc::new(Cell::new(0u32));
        // Alive but never passed to `.back`, so it is never wired.
        let _handle = {
            let fired = Rc::clone(&fired);
            SwitcherBack::new(move || fired.set(fired.get() + 1))
        };
        let _rig = Rig::new(&stage_view(true, 3), PHONE);
        assert!(!handles_back(), "open but without .back: no claim");

        // The handle was never wired, so no fallback exists to route a press.
        push_back_press();
        assert!(!handles_back(), "still nothing claims after a press");
        assert_eq!(fired.get(), 0, "the unwired handle's on_back never runs");
    }

    #[test]
    fn a_stage_without_back_builds_rebuilds_and_tears_down() {
        let prev = stage_view(true, 2);
        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        let mut w = View::<()>::build(&prev, &mut ctx);
        let next = stage_view(false, 2);
        View::<()>::rebuild(&next, &prev, &mut w, &mut ctx);
        View::<()>::teardown(&next, &mut w, &mut ctx);
    }

    // --- Chips and bar ---

    #[derive(Default)]
    struct Taps(u32);

    fn icon() -> IconData {
        IconData::from_path(Rect::new(4.0, 4.0, 20.0, 20.0).to_path(0.1), 24.0)
    }

    fn chip(label: Option<&str>) -> SwitcherChip<Taps> {
        let chip = switcher_chip(icon(), |t: &mut Taps| t.0 += 1);
        match label {
            Some(label) => chip.label(label),
            None => chip.semantic_label("search"),
        }
    }

    fn lay_out<W: Widget>(w: &mut W, bc: BoxConstraints, insets: WindowInsets) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        lctx.with_window_insets(insets, |ctx| w.layout(ctx, &bc))
    }

    fn build_chip(view: &SwitcherChip<Taps>) -> SwitcherChipWidget {
        let mut counter = 0u64;
        View::<Taps>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn loose() -> BoxConstraints {
        BoxConstraints::loose(Size::new(400.0, 400.0))
    }

    #[test]
    fn chips_are_48_square_icon_only_and_at_least_128_wide_labelled() {
        let mut icon_only = build_chip(&chip(None));
        let size = lay_out(&mut icon_only, loose(), WindowInsets::default());
        assert_eq!(size, Size::new(48.0, 48.0));
        let mut labelled = build_chip(&chip(Some("+ add")));
        let size = lay_out(&mut labelled, loose(), WindowInsets::default());
        assert_eq!(size.height, 48.0);
        assert!(size.width >= 128.0, "{size:?}");
    }

    #[test]
    fn a_press_fires_once_and_requests_no_frame() {
        let mut w = build_chip(&chip(Some("+ add")));
        let size = lay_out(&mut w, loose(), WindowInsets::default());
        let mut taps = Taps::default();
        for phase in [PointerPhase::Down, PointerPhase::Up] {
            let state: &mut dyn Any = &mut taps;
            let mut ctx = EventCtx::new(state, Point::ZERO, size);
            w.event(&mut ctx, &ev(phase, 20.0, 24.0));
            if phase == PointerPhase::Down {
                assert!(w.pressed, "the tint shows at once");
            }
        }
        assert_eq!(taps.0, 1);
        let theme = crate::baseline();
        let mut ctx = PaintCtx::new(Point::ZERO, size).with_theme(&theme);
        w.paint(&mut ctx, &mut Ops::default());
        assert!(!ctx.needs_frame(), "press feedback never animates");
    }

    #[test]
    fn a_disabled_chip_ignores_presses() {
        let mut w = build_chip(&chip(None).disabled(true));
        let size = lay_out(&mut w, loose(), WindowInsets::default());
        let mut taps = Taps::default();
        for phase in [PointerPhase::Down, PointerPhase::Up] {
            let state: &mut dyn Any = &mut taps;
            let mut ctx = EventCtx::new(state, Point::ZERO, size);
            assert_eq!(
                w.event(&mut ctx, &ev(phase, 24.0, 24.0)),
                EventResult::Ignored
            );
        }
        assert_eq!(taps.0, 0);
    }

    #[test]
    fn a_chip_is_a_labelled_button_with_click() {
        fn logic(_: &mut Taps) -> SwitcherChip<Taps> {
            switcher_chip(icon(), |_: &mut Taps| {}).label("+ add")
        }
        let mut root: frust_core::RenderRoot<Taps, SwitcherChip<Taps>> =
            frust_core::RenderRoot::new();
        let mut state = Taps::default();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(400.0, 200.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Button)
            .expect("a Role::Button node");
        assert_eq!(node.label(), Some("+ add"));
        assert!(node.supports_action(Action::Click));
    }

    #[test]
    fn the_bar_centres_three_chips_twelve_apart_on_phone_and_tablet() {
        for stage in [PHONE, TABLET] {
            let view = switcher_bar((chip(None), chip(Some("+ add server")), chip(None)));
            let mut counter = 0u64;
            let mut w = View::<Taps>::build(&view, &mut BuildCtx::new(&mut counter));
            let size = lay_out(
                &mut w,
                BoxConstraints::loose(stage),
                WindowInsets::default(),
            );
            assert_eq!(size.width, stage.width, "full width");
            assert_eq!(size.height, 48.0 + 2.0 * BAR_PAD_Y);
            let rects: Vec<Rect> = w
                .pods
                .iter()
                .map(|p| Rect::from_origin_size(p.origin(), p.size()))
                .collect();
            for pair in rects.windows(2) {
                assert!(near(pair[1].x0 - pair[0].x1, BAR_GAP), "{rects:?}");
            }
            assert!(rects[0].x0 >= BAR_PAD_X && rects[2].x1 <= stage.width - BAR_PAD_X);
            assert!(near(rects[0].x0, stage.width - rects[2].x1), "centred");
            assert!(rects.iter().all(|r| near(r.y0, BAR_PAD_Y)));
        }
    }

    #[test]
    fn the_bar_pads_and_paints_under_the_bottom_inset() {
        let view = switcher_bar((chip(None), chip(None)));
        let mut counter = 0u64;
        let mut w = View::<Taps>::build(&view, &mut BuildCtx::new(&mut counter));
        let edges = |bottom| WindowEdgeInsets {
            left: 0.0,
            top: 0.0,
            right: 0.0,
            bottom,
        };
        let insets = WindowInsets::new(edges(34.0), edges(0.0));
        let size = lay_out(&mut w, BoxConstraints::loose(PHONE), insets);
        assert_eq!(size.height, 48.0 + 2.0 * BAR_PAD_Y + 34.0);
        assert!(w.pods.iter().all(|p| near(p.origin().y, BAR_PAD_Y)));
        let theme = crate::baseline();
        let mut ctx = PaintCtx::new(Point::ZERO, size).with_theme(&theme);
        let mut ops = Ops::default();
        w.paint(&mut ctx, &mut ops);
        let lift = theme.scheme().on_surface.multiply_alpha(BAR_LIFT);
        assert_eq!(ops.0[0], Op::Fill(size.to_rect(), lift));
    }

    #[test]
    fn a_press_on_the_bar_off_a_chip_is_swallowed() {
        let view = switcher_bar(chip(None));
        let mut counter = 0u64;
        let mut w = View::<Taps>::build(&view, &mut BuildCtx::new(&mut counter));
        let size = lay_out(
            &mut w,
            BoxConstraints::loose(PHONE),
            WindowInsets::default(),
        );
        let mut taps = Taps::default();
        let state: &mut dyn Any = &mut taps;
        let mut ctx = EventCtx::new(state, Point::ZERO, size);
        let result = w.event(&mut ctx, &ev(PointerPhase::Down, 2.0, 2.0));
        assert_eq!(result, EventResult::Handled);
    }
}
