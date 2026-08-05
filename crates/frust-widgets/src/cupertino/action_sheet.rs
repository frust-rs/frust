//! `CupertinoActionSheet`: the iOS action
//! sheet — a bottom-anchored list of action rows plus a separate cancel block,
//! over a dimming scrim.
//!
//! It is pushed as a **transparent** navigator page via the
//! [`show_action_sheet`] convenience, entering with the
//! [`PageTransition::SlideUp`](crate::PageTransition::SlideUp) preset — the
//! whole page (scrim included) translates up from the bottom, so the scrim is
//! **page-owned** and drawn here at a fixed alpha rather than driven by the
//! transition (see `crate::nav::transition`'s `SlideUp` docs). An action tap
//! pops carrying the action's index as a [`PopResult`]; the cancel row or a
//! scrim tap pops with an empty result (dismiss).
//!
//! Reuses [`super::alert_dialog`]'s [`CupertinoActionStyle`]/
//! [`CupertinoDialogAction`]/[`action`] vocabulary. Colors and the ~14pt corner
//! carry the same community-approximate flags documented there.
//!
//! # Safe area
//!
//! The sheet anchors to a fixed bottom margin. The home-indicator safe-area
//! inset a real iOS sheet respects is **shell-future work**, not yet wired.
//!
//! # Keyboard operability
//!
//! **Escape-to-dismiss now works, once the action sheet has focus.** A `Down`
//! anywhere in the sheet (scrim, panel background, cancel row, or an action
//! row) claims focus via `EventCtx::request_focus` — the sheet already
//! captures its whole area, so this is a pure opt-in with no new hit-testing.
//! Once focused, a focus-routed `Key(Escape)` invokes the same
//! `controller.pop()` dismiss path as a scrim tap or the cancel row. **There
//! is still no hook to focus the sheet on appear** (auto-focus-on-appear) — a
//! caller must complete one pointer interaction with the sheet before Escape
//! does anything; that gap is deferred to future focus-manager work.
//!
//! Its semantics node is a [`Role::Menu`] container with **no** accesskit
//! modal flag, so there is no modal-audience concern to reconcile.
//!
//! # State layer
//!
//! This action sheet has no `StateLayer` surface of its own (the scrim,
//! panels, and rows are plain fills/hairlines, not an M3 interactive surface)
//! — there is nothing here for `StateLayer::set_focused` to wire into.
//!
//! # Liquid Glass panels
//!
//! When a [`Theme`] is threaded and its `glass.chrome` recipe is not the
//! opaque-material path ([`frust_theme::GlassMaterial::is_opaque`] — true
//! on the Material baseline, false on [`Theme::cupertino_baseline`]), *both*
//! the main action panel and the separate cancel block paint as their own
//! `theme.glass.chrome` panel — fill-wash stack + specular hairline + drop
//! shadow, straight from the token — and each row within either block paints
//! as a nested "button" capsule inset from its own panel's edge, corner
//! radius derived via [`frust_theme::ShapeScale::concentric_inner`] (see
//! [`super::alert_dialog`]'s identical Liquid Glass panel section for the
//! full rationale, shared verbatim here). No theme, or an opaque-chrome
//! theme, falls back to the pre-26 single flat fill + hairline-separator-row
//! layout this widget always painted. Real background blur is out of scope.
//!
//! Kit sizing evidence (community-mined from iOS system UI screenshots,
//! **partially-verified**, cited as guidance only): action-sheet containers
//! ran roughly 260×424-524px, action rows roughly 232×48px. This widget
//! keeps its established [`ROW_H`]/margin geometry rather than matching
//! those figures exactly — only the *material* and *corner radii* are
//! re-skinned here, not the sizing.

use frust_core::accesskit::Role;
use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    Key, LayoutCtx, NamedKey, PaintCtx, PaintScene, PointerPhase, SemanticsCtx, View, Widget,
};
use frust_theme::{Brightness, GlassFill, GlassMaterial, ShapeScale, Theme};
use kurbo::{Point, Rect, RoundedRect, Shape, Size};
use peniko::{Brush, Color};

use super::alert_dialog::{CupertinoActionStyle, CupertinoDialogAction, action, action_label_view};
use crate::nav::navigator::{NavigatorController, PopResult};

/// Horizontal margin from the window edges to the sheet panels, in logical px.
///
/// **Community-approximate**: ~8pt matches the inset iOS action sheets sit at;
/// no published margin spec exists.
const SIDE_MARGIN: f64 = 8.0;
/// Bottom margin from the window's bottom edge to the cancel block, px
/// (**community-approximate**; the home-indicator safe area is shell-future
/// work — see the [module docs](self)).
const BOTTOM_MARGIN: f64 = 8.0;
/// Gap between the main action panel and the separate cancel block, px.
const BLOCK_GAP: f64 = 8.0;
/// Height of each action/cancel row, in logical px.
///
/// **Community-approximate**: ~56pt matches the taller iOS action-sheet row
/// (vs. an alert's ~44pt); no published spec exists.
const ROW_H: f64 = 56.0;
/// Unthemed-fallback panel corner radius, logical px (**community-approximate**,
/// ~13-14pt — see [`super::alert_dialog`]'s identical flag). A themed panel
/// uses the larger `shape.extra_large` iOS-26+ Liquid Glass radius instead
/// (see [`resolve_panel_radius`] and the module docs' Liquid Glass panels
/// section) — this constant is now only the no-theme (bare-core test) path.
const PANEL_RADIUS: f64 = 14.0;
/// Inset, in logical px, between a nested row "button" capsule and its own
/// panel's edge (see [`super::alert_dialog`]'s identical `BUTTON_INSET`
/// rationale — shared verbatim here).
const BUTTON_INSET: f64 = 4.0;
/// Flattening tolerance for a panel's specular-hairline outline path (see
/// [`crate::material::card`]'s precedent).
const PATH_TOLERANCE: f64 = 0.1;

/// Unthemed fallback panel fill (a theme resolves this from
/// `colors.surface_container_high`).
const PANEL_FILL: Color = Color::from_rgb8(0xF2, 0xF2, 0xF7);
/// Unthemed fallback hairline color (a theme resolves this from
/// `colors.outline_variant`).
///
/// **Composite-derived** (re-derived for the 2026-07-18 kit refresh — see
/// [`super::navbar`]'s `SEPARATOR` for the full derivation, mirrored here):
/// `0.12` black over white ≈ `0xE0E0E0`, superseding the pre-refresh
/// `#C6C6C8` approximation of the old translucent `rgba(60,60,67,0.29)`
/// token.
const SEPARATOR: Color = Color::from_rgb8(0xE0, 0xE0, 0xE0);
/// The scrim alpha the modal dims the page below with (see
/// [`super::alert_dialog`]'s identical flag).
const SCRIM_ALPHA: f32 = 0.2;
/// Hairline stroke width, logical px.
const HAIRLINE_W: f64 = 1.0;

/// Push a Cupertino action sheet onto `controller`'s stack. `on_result`
/// receives the tapped action's index (`usize`) as a [`PopResult`], or an empty
/// result when the cancel row or scrim was tapped.
pub fn show_action_sheet<State: 'static>(
    controller: &NavigatorController<State>,
    actions: Vec<CupertinoDialogAction>,
    cancel: Option<String>,
    on_result: impl Fn(&mut State, PopResult) + 'static,
) {
    let controller_for_builder = controller.clone();
    controller.push_transparent_for_result(
        move || {
            frust_core::any::<State, _>(CupertinoActionSheetView {
                actions: actions.clone(),
                cancel: cancel.clone(),
                controller: controller_for_builder.clone(),
            })
        },
        crate::TransitionSpec::duration(crate::PageTransition::SlideUp),
        on_result,
    );
}

/// A declarative iOS action sheet page. Usually created via
/// [`show_action_sheet`]; exposed so it can also be tested/embedded directly.
pub struct CupertinoActionSheetView<State: 'static> {
    pub actions: Vec<CupertinoDialogAction>,
    pub cancel: Option<String>,
    pub controller: NavigatorController<State>,
}

/// The retained widget for a [`CupertinoActionSheetView`].
pub struct CupertinoActionSheetWidget<State: 'static> {
    actions: Vec<ChildPod>,
    cancel: Option<ChildPod>,
    controller: NavigatorController<State>,
    /// Main action panel rect in local coordinates (computed in layout).
    main_rect: Rect,
    /// Each action row's rect in local coordinates.
    action_rects: Vec<Rect>,
    /// The cancel block rect in local coordinates (if a cancel row exists).
    cancel_rect: Option<Rect>,
    /// Armed by a `Down`, cleared on `Up`/`Cancel` (fire-on-up-inside).
    captured: bool,
    /// Whether a `Down` has already claimed focus this open — the claim-once
    /// guard (see `event`'s Down arm). Seeding `false` in [`View::build`] is
    /// correct with no reset needed elsewhere: [`show_action_sheet`] pushes a
    /// fresh page per open, so a new widget instance (and a fresh `false`) is
    /// built every time the sheet opens; there is no retained instance to
    /// reset on close.
    focus_claimed: bool,
}

/// Return `color` with its alpha channel replaced by `alpha` (mirrors
/// [`super::alert_dialog`]'s helper of the same shape).
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// The resolved panel corner radius (shared by the main panel and the
/// cancel block). Themed: `shape.extra_large` (the iOS-26+ Liquid Glass
/// idiom's larger panel radius). Unthemed: [`PANEL_RADIUS`] exactly.
fn resolve_panel_radius(theme: Option<&Theme>) -> f64 {
    match theme {
        Some(theme) => theme.shape.extra_large,
        None => PANEL_RADIUS,
    }
}

/// The chrome-tier glass material — a one-line accessor kept so the paint
/// code below reads `theme.glass.chrome` in exactly one place (mirrors
/// [`super::alert_dialog::resolve_chrome`]).
fn resolve_chrome(theme: &Theme) -> &GlassMaterial {
    &theme.glass.chrome
}

/// The chrome material's fill-wash stack for `theme`'s active brightness.
fn resolve_fills<'a>(theme: &Theme, material: &'a GlassMaterial) -> &'a [GlassFill] {
    if theme.brightness == Brightness::Dark {
        &material.fills_dark
    } else {
        &material.fills_light
    }
}

/// Paint one glass-chrome panel (the main action panel or the cancel block):
/// the fill-wash stack, a specular hairline outline, the tier's drop shadow,
/// and a nested "button" capsule per `rows` inset from the panel's own edge
/// at the concentric-inner radius. Shared by both panels in
/// [`CupertinoActionSheetWidget::paint`] — see the module docs' Liquid Glass
/// panels section.
#[allow(clippy::too_many_arguments)]
fn paint_glass_panel(
    scene: &mut dyn PaintScene,
    page_origin: Point,
    panel_local: Rect,
    panel_radius: f64,
    theme: &Theme,
    material: &GlassMaterial,
    rows: &[Rect],
) {
    let panel_origin = Point::new(
        page_origin.x + panel_local.x0,
        page_origin.y + panel_local.y0,
    );
    let panel_size = Size::new(panel_local.width(), panel_local.height());

    if material.shadow.color_alpha > 0.0 {
        let shadow_color = with_alpha(theme.scheme().shadow, material.shadow.color_alpha);
        scene.draw_shadow(
            Point::new(panel_origin.x, panel_origin.y + material.shadow.y_offset),
            panel_size,
            panel_radius,
            material.shadow.blur_std_dev,
            shadow_color,
        );
    }

    let fills = resolve_fills(theme, material);
    for wash in fills {
        scene.fill_rounded_rect(panel_origin, panel_size, panel_radius, wash.color);
    }

    // Specular hairline — white at the token's alpha (see
    // `GlassMaterial::hairline_alpha`'s docs: the color is always white,
    // only the alpha is thematic).
    let hairline = Color::new([1.0, 1.0, 1.0, material.hairline_alpha]);
    if material.hairline_alpha > 0.0 {
        let local = Rect::new(0.0, 0.0, panel_size.width, panel_size.height);
        let path = RoundedRect::from_rect(local, panel_radius).to_path(PATH_TOLERANCE);
        scene.stroke_path(panel_origin, &path, HAIRLINE_W, &Brush::Solid(hairline));
    }

    let nested_radius = ShapeScale::concentric_inner(panel_radius, BUTTON_INSET);
    let nested_fill = fills.last().map(|w| w.color).unwrap_or(hairline);
    for rect in rows {
        let btn_origin = Point::new(
            page_origin.x + rect.x0 + BUTTON_INSET,
            page_origin.y + rect.y0 + BUTTON_INSET,
        );
        let btn_size = Size::new(
            (rect.width() - 2.0 * BUTTON_INSET).max(0.0),
            (rect.height() - 2.0 * BUTTON_INSET).max(0.0),
        );
        scene.fill_rounded_rect(btn_origin, btn_size, nested_radius, nested_fill);
    }
}

/// Build the cancel row's type-erased child view (bold systemBlue, centered).
fn cancel_view<State: 'static>(label: String) -> AnyView<State> {
    action_label_view::<State>(&action(label).style(CupertinoActionStyle::Cancel))
}

impl<State: 'static> View<State> for CupertinoActionSheetView<State> {
    type Element = CupertinoActionSheetWidget<State>;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> CupertinoActionSheetWidget<State> {
        let actions = self
            .actions
            .iter()
            .map(|a| crate::authoring::build_child(&action_label_view::<State>(a), ctx))
            .collect();
        let cancel = self
            .cancel
            .as_ref()
            .map(|c| crate::authoring::build_child(&cancel_view::<State>(c.clone()), ctx));
        CupertinoActionSheetWidget {
            actions,
            cancel,
            controller: self.controller.clone(),
            main_rect: Rect::ZERO,
            action_rects: Vec::new(),
            cancel_rect: None,
            captured: false,
            focus_claimed: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut CupertinoActionSheetWidget<State>,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        element.controller = self.controller.clone();
        let common = prev.actions.len().min(self.actions.len());
        for i in 0..common {
            flags |= crate::authoring::rebuild_child(
                &action_label_view::<State>(&prev.actions[i]),
                &action_label_view::<State>(&self.actions[i]),
                &mut element.actions[i],
                ctx,
            );
        }
        if let (Some(pc), Some(nc), Some(elem_c)) =
            (&prev.cancel, &self.cancel, element.cancel.as_mut())
        {
            flags |= crate::authoring::rebuild_child(
                &cancel_view::<State>(pc.clone()),
                &cancel_view::<State>(nc.clone()),
                elem_c,
                ctx,
            );
        }
        flags
    }

    fn teardown(&self, element: &mut CupertinoActionSheetWidget<State>, ctx: &mut BuildCtx<'_>) {
        for (a, pod) in self.actions.iter().zip(element.actions.iter_mut()) {
            crate::authoring::teardown_child(&action_label_view::<State>(a), pod, ctx);
        }
        if let (Some(c), Some(elem_c)) = (&self.cancel, element.cancel.as_mut()) {
            crate::authoring::teardown_child(&cancel_view::<State>(c.clone()), elem_c, ctx);
        }
    }
}

impl<State: 'static> Widget for CupertinoActionSheetWidget<State> {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let full = bc.max();
        let panel_w = (full.width - 2.0 * SIDE_MARGIN).max(0.0);
        let panel_x = SIDE_MARGIN;
        let row_bc = BoxConstraints::loose(Size::new(panel_w, ROW_H));

        for pod in &mut self.actions {
            pod.layout_child(ctx, &row_bc);
        }
        if let Some(c) = self.cancel.as_mut() {
            c.layout_child(ctx, &row_bc);
        }

        // Cancel block hugs the bottom; the main panel sits above it.
        let (cancel_top, main_bottom) = if self.cancel.is_some() {
            let cancel_bottom = full.height - BOTTOM_MARGIN;
            let cancel_top = cancel_bottom - ROW_H;
            self.cancel_rect = Some(Rect::new(
                panel_x,
                cancel_top,
                panel_x + panel_w,
                cancel_bottom,
            ));
            (cancel_top, cancel_top - BLOCK_GAP)
        } else {
            self.cancel_rect = None;
            (0.0, full.height - BOTTOM_MARGIN)
        };
        let _ = cancel_top;

        let main_h = self.actions.len() as f64 * ROW_H;
        let main_top = main_bottom - main_h;
        self.main_rect = Rect::new(panel_x, main_top, panel_x + panel_w, main_bottom);

        // Action rows stacked top-to-bottom in the main panel; center labels.
        self.action_rects.clear();
        for (i, pod) in self.actions.iter_mut().enumerate() {
            let row_y = main_top + i as f64 * ROW_H;
            let rect = Rect::new(panel_x, row_y, panel_x + panel_w, row_y + ROW_H);
            let ls = pod.size();
            pod.set_origin(Point::new(
                panel_x + (panel_w - ls.width) / 2.0,
                row_y + (ROW_H - ls.height) / 2.0,
            ));
            self.action_rects.push(rect);
        }

        // Center the cancel label in its block.
        if let (Some(c), Some(rect)) = (self.cancel.as_mut(), self.cancel_rect) {
            let ls = c.size();
            c.set_origin(Point::new(
                panel_x + (panel_w - ls.width) / 2.0,
                rect.y0 + (ROW_H - ls.height) / 2.0,
            ));
        }

        bc.constrain(full)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let scrim_base = theme.map(|t| t.scheme().scrim).unwrap_or(Color::BLACK);

        let origin = ctx.origin();
        let size = ctx.size();
        // Page-owned scrim (mechanism unchanged — restyled values only, per
        // the module docs; SlideUp does not fade layers — see the module docs).
        scene.fill_rect(origin, size, with_alpha(scrim_base, SCRIM_ALPHA));

        let panel_radius = resolve_panel_radius(theme);
        let chrome = theme.map(resolve_chrome);

        match chrome {
            Some(material) if !material.is_opaque() => {
                let theme = theme.expect("chrome resolved from a threaded theme");
                paint_glass_panel(
                    scene,
                    origin,
                    self.main_rect,
                    panel_radius,
                    theme,
                    material,
                    &self.action_rects,
                );
                if let Some(rect) = self.cancel_rect {
                    paint_glass_panel(
                        scene,
                        origin,
                        rect,
                        panel_radius,
                        theme,
                        material,
                        std::slice::from_ref(&rect),
                    );
                }
            }
            _ => {
                // No theme, or an opaque-chrome (Material-baseline) theme:
                // the pre-26 single flat fill + hairline-separated rows.
                let panel_fill = theme
                    .map(|t| t.scheme().surface_container_high)
                    .unwrap_or(PANEL_FILL);
                let separator = theme
                    .map(|t| t.scheme().outline_variant)
                    .unwrap_or(SEPARATOR);

                let main_origin =
                    Point::new(origin.x + self.main_rect.x0, origin.y + self.main_rect.y0);
                scene.fill_rounded_rect(
                    main_origin,
                    Size::new(self.main_rect.width(), self.main_rect.height()),
                    panel_radius,
                    panel_fill,
                );
                // Hairlines between action rows (skip the first row's top).
                for rect in self.action_rects.iter().skip(1) {
                    let y = origin.y + rect.y0 + HAIRLINE_W / 2.0;
                    scene.stroke_line(
                        Point::new(origin.x + rect.x0, y),
                        Point::new(origin.x + rect.x1, y),
                        HAIRLINE_W,
                        separator,
                    );
                }

                // Separate cancel block.
                if let Some(rect) = self.cancel_rect {
                    let c_origin = Point::new(origin.x + rect.x0, origin.y + rect.y0);
                    scene.fill_rounded_rect(
                        c_origin,
                        Size::new(rect.width(), rect.height()),
                        panel_radius,
                        panel_fill,
                    );
                }
            }
        }

        for pod in &mut self.actions {
            pod.paint_child(ctx, scene);
        }
        if let Some(c) = self.cancel.as_mut() {
            c.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // Escape (once focused) dismisses through the same `controller.pop()`
        // path as a scrim tap / cancel row — see the module docs' Keyboard
        // operability note.
        if let InputEvent::Key(key_event) = event {
            if key_event.key == Key::Named(NamedKey::Escape) {
                self.controller.pop();
                ctx.request_redraw();
                return EventResult::Handled;
            }
            return EventResult::Ignored;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                // The first press anywhere in the action sheet claims focus,
                // so a subsequent Escape has a focus chain to travel; the
                // claim is held until this page pops, so a later press
                // re-claiming would be redundant.
                if !self.focus_claimed {
                    ctx.request_focus();
                    self.focus_claimed = true;
                }
                self.captured = true;
                ctx.capture_pointer();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.captured = false;
                let pos = p.position;
                if let Some(idx) = self.action_rects.iter().position(|r| r.contains(pos)) {
                    self.controller.pop_with_result(PopResult::of(idx));
                    ctx.request_redraw();
                } else if self.cancel_rect.map(|r| r.contains(pos)).unwrap_or(false) {
                    // Cancel row → dismiss (empty result).
                    self.controller.pop();
                    ctx.request_redraw();
                } else if !self.main_rect.contains(pos) {
                    // Scrim tap → dismiss.
                    self.controller.pop();
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                self.captured = false;
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let actions = &self.actions;
        let cancel = &self.cancel;
        // An action sheet reads to a screen reader like a menu of choices.
        ctx.push_container(
            Role::Menu,
            |_| {},
            |ctx| {
                for pod in actions {
                    pod.semantics_child(ctx);
                }
                if let Some(c) = cancel {
                    c.semantics_child(ctx);
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nav::navigator::{NavigatorView, navigator};
    use frust_core::{
        BuildCtx, FrameTime, KeyEvent, Modifiers, PointerButton, PointerEvent, RenderRoot, any,
    };
    use frust_text::TextContext;
    use std::any::Any;

    const WINDOW: Size = Size::new(400.0, 800.0);

    fn ft_secs(s: f64) -> FrameTime {
        FrameTime::from_nanos((s * 1_000_000_000.0) as u64)
    }

    fn escape_event() -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::Escape),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    fn sheet_view() -> CupertinoActionSheetView<()> {
        CupertinoActionSheetView {
            actions: vec![
                action("Save"),
                action("Delete").style(CupertinoActionStyle::Destructive),
            ],
            cancel: Some("Cancel".into()),
            controller: NavigatorController::new(),
        }
    }

    fn build(view: &CupertinoActionSheetView<()>) -> CupertinoActionSheetWidget<()> {
        let mut counter = 0u64;
        View::<()>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut CupertinoActionSheetWidget<()>, window: Size) {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), None);
        w.layout(&mut lctx, &BoxConstraints::tight(window));
    }

    /// Records filled rounded rects, shadows, and stroked paths — enough to
    /// assert both glass panels' fill stacks, hairlines, and nested
    /// concentric-radius button capsules reach paint.
    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        shadows: Vec<(Point, Size, f64, f64, Color)>,
        strokes: Vec<(Point, f64, Color)>,
    }
    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn draw_shadow(&mut self, o: Point, s: Size, radius: f64, std_dev: f64, color: Color) {
            self.shadows.push((o, s, radius, std_dev, color));
        }
        fn stroke_path(
            &mut self,
            origin: Point,
            _path: &kurbo::BezPath,
            width: f64,
            brush: &Brush,
        ) {
            if let Brush::Solid(color) = brush {
                self.strokes.push((origin, width, *color));
            }
        }
    }

    fn paint_with_theme(
        w: &mut CupertinoActionSheetWidget<()>,
        window: Size,
        theme: Option<&Theme>,
    ) -> Recorder {
        let mut rec = Recorder::default();
        let mut pctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, window).with_theme(t),
            None => PaintCtx::new(Point::ZERO, window),
        };
        w.paint(&mut pctx, &mut rec);
        rec
    }

    #[test]
    fn glass_chrome_theme_paints_both_panels_fill_stack_hairline_and_shadow() {
        let theme = Theme::cupertino_baseline();
        let mut w = build(&sheet_view());
        layout(&mut w, WINDOW);
        let rec = paint_with_theme(&mut w, WINDOW, Some(&theme));

        let radius = resolve_panel_radius(Some(&theme));
        for wash in &theme.glass.chrome.fills_light {
            // Each wash paints on the main panel *and* the cancel block —
            // two matching rounded rects per wash color.
            let count = rec
                .rrects
                .iter()
                .filter(|(_, _, r, c)| *r == radius && *c == wash.color)
                .count();
            assert_eq!(
                count, 2,
                "wash {:?} reaches both the main panel and the cancel block",
                wash
            );
        }
        assert!(
            rec.strokes.len() >= 2,
            "both panels stroke their own specular hairline"
        );
        assert!(
            rec.shadows.len() >= 2,
            "both panels paint their own drop shadow"
        );
    }

    #[test]
    fn nested_row_capsule_radius_is_concentric_with_its_panel() {
        let theme = Theme::cupertino_baseline();
        let mut w = build(&sheet_view());
        layout(&mut w, WINDOW);
        let rec = paint_with_theme(&mut w, WINDOW, Some(&theme));

        let panel_radius = resolve_panel_radius(Some(&theme));
        let expected = ShapeScale::concentric_inner(panel_radius, BUTTON_INSET);
        assert!(expected < panel_radius);
        assert!(
            rec.rrects
                .iter()
                .any(|(_, _, r, _)| (*r - expected).abs() < 1e-9),
            "a nested row capsule (action row or cancel row) paints at the \
             concentric-inner radius"
        );
    }

    #[test]
    fn opaque_material_theme_keeps_the_pre_26_flat_panels() {
        let theme = Theme::m3_baseline();
        let mut w = build(&sheet_view());
        layout(&mut w, WINDOW);
        let rec = paint_with_theme(&mut w, WINDOW, Some(&theme));

        assert!(theme.glass.chrome.is_opaque());
        // One flat fill for the main panel, one for the cancel block — no
        // chrome washes or nested capsules.
        assert_eq!(rec.rrects.len(), 2);
        assert!(
            rec.rrects
                .iter()
                .all(|(_, _, _, c)| *c == theme.scheme().surface_container_high)
        );
        assert!(
            rec.shadows.is_empty(),
            "the opaque path paints no glass shadow"
        );
    }

    #[test]
    fn unthemed_paint_keeps_the_pre_26_flat_panels() {
        let mut w = build(&sheet_view());
        layout(&mut w, WINDOW);
        let rec = paint_with_theme(&mut w, WINDOW, None);

        assert_eq!(
            rec.rrects.len(),
            2,
            "one flat fill per panel, unthemed fallback"
        );
        assert!(
            rec.rrects
                .iter()
                .all(|(_, _, r, c)| *r == PANEL_RADIUS && *c == PANEL_FILL)
        );
    }

    #[test]
    fn panels_anchor_to_the_bottom_with_cancel_below_actions() {
        let mut w = build(&sheet_view());
        layout(&mut w, WINDOW);
        assert_eq!(w.action_rects.len(), 2);
        let cancel = w.cancel_rect.expect("cancel block present");
        // Cancel sits below the main panel.
        assert!(cancel.y0 > w.main_rect.y1 - 1e-6);
        // Cancel bottom respects the bottom margin.
        assert!((cancel.y1 - (WINDOW.height - BOTTOM_MARGIN)).abs() < 1e-6);
        // The whole sheet hugs the bottom half of the window.
        assert!(w.main_rect.y0 > WINDOW.height / 2.0);
    }

    #[test]
    fn panel_width_respects_side_margins() {
        let mut w = build(&sheet_view());
        layout(&mut w, WINDOW);
        assert!((w.main_rect.width() - (WINDOW.width - 2.0 * SIDE_MARGIN)).abs() < 1e-6);
        assert_eq!(w.main_rect.x0, SIDE_MARGIN);
    }

    // --- Modal wiring through a real navigator (mirrors alert_dialog's
    //     harness), settling the entering SlideUp transition first. ---

    #[derive(Default)]
    struct SheetState {
        calls: u32,
        received: Option<usize>,
    }

    struct Harness {
        root: RenderRoot<SheetState, NavigatorView<SheetState>>,
        controller: NavigatorController<SheetState>,
        state: SheetState,
        tcx: TextContext,
        probe: CupertinoActionSheetWidget<()>,
    }

    impl Harness {
        fn new() -> Self {
            let controller: NavigatorController<SheetState> = NavigatorController::new();
            let root: RenderRoot<SheetState, NavigatorView<SheetState>> = RenderRoot::new();
            let mut probe = build(&sheet_view());
            let mut tcx = TextContext::new();
            let mut lctx = LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), None);
            probe.layout(&mut lctx, &BoxConstraints::tight(WINDOW));
            Harness {
                root,
                controller,
                state: SheetState::default(),
                tcx: TextContext::new(),
                probe,
            }
        }

        fn app(&self) -> impl FnMut(&mut SheetState) -> NavigatorView<SheetState> + use<> {
            let ctrl = self.controller.clone();
            move |_: &mut SheetState| navigator(&ctrl, || any(crate::text::text("base").size(17.0)))
        }

        fn rebuild_layout(&mut self) {
            let mut app = self.app();
            self.root.rebuild(&mut app, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
        }

        fn show(&mut self) {
            self.rebuild_layout();
            show_action_sheet(
                &self.controller,
                vec![
                    action("Save"),
                    action("Delete").style(CupertinoActionStyle::Destructive),
                ],
                Some("Cancel".into()),
                |s: &mut SheetState, r: PopResult| {
                    s.calls += 1;
                    if let Some(i) = r.take::<usize>() {
                        s.received = Some(i);
                    }
                },
            );
            self.rebuild_layout();
        }

        fn settle(&mut self) {
            let mut scene = crate::test_support::RecordingScene::default();
            self.root.paint(&mut scene, ft_secs(0.0));
            self.root.paint(&mut scene, ft_secs(1.0));
            self.rebuild_layout();
        }

        fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
            InputEvent::Pointer(PointerEvent {
                phase,
                position: Point::new(x, y),
                button: PointerButton::Primary,
            })
        }

        fn tap(&mut self, x: f64, y: f64) {
            self.root
                .event(&mut self.state, &Self::ev(PointerPhase::Down, x, y));
            self.root
                .event(&mut self.state, &Self::ev(PointerPhase::Up, x, y));
        }

        fn drain(&mut self) {
            self.rebuild_layout();
            self.root
                .event(&mut self.state, &Self::ev(PointerPhase::Move, 1.0, 1.0));
        }

        fn action_center(&self, idx: usize) -> (f64, f64) {
            let r = self.probe.action_rects[idx];
            ((r.x0 + r.x1) / 2.0, (r.y0 + r.y1) / 2.0)
        }

        fn cancel_center(&self) -> (f64, f64) {
            let r = self.probe.cancel_rect.unwrap();
            ((r.x0 + r.x1) / 2.0, (r.y0 + r.y1) / 2.0)
        }
    }

    #[test]
    fn action_tap_pops_with_its_index() {
        let mut h = Harness::new();
        h.show();
        h.settle();
        let (cx, cy) = h.action_center(1);
        h.tap(cx, cy);
        h.drain();
        assert_eq!(h.state.calls, 1);
        assert_eq!(h.state.received, Some(1));
    }

    #[test]
    fn cancel_tap_dismisses() {
        let mut h = Harness::new();
        h.show();
        h.settle();
        let (cx, cy) = h.cancel_center();
        h.tap(cx, cy);
        h.drain();
        assert_eq!(h.state.calls, 1, "cancel pops the sheet");
        assert_eq!(h.state.received, None, "cancel carries no action index");
    }

    #[test]
    fn scrim_tap_dismisses() {
        let mut h = Harness::new();
        h.show();
        h.settle();
        // Top-left corner — above the bottom-anchored sheet.
        h.tap(5.0, 5.0);
        h.drain();
        assert_eq!(h.state.calls, 1);
        assert_eq!(h.state.received, None);
    }

    // --- Focus + Escape opt-in. ---

    #[test]
    fn escape_after_a_press_claims_focus_and_dismisses() {
        let mut h = Harness::new();
        h.show();
        h.settle();
        // A bare `Down` on the top-left scrim claims focus; only the matching
        // `Up` decides dismissal, so this alone does not dismiss.
        h.root
            .event(&mut h.state, &Harness::ev(PointerPhase::Down, 5.0, 5.0));
        h.drain();
        assert_eq!(h.state.calls, 0, "a bare Down does not dismiss");

        // Escape now reaches the focused sheet and dismisses it, the same
        // dismiss path as a scrim tap / cancel row.
        h.root.event(&mut h.state, &escape_event());
        h.drain();
        assert_eq!(h.state.calls, 1, "Escape dismisses the focused sheet");
        assert_eq!(h.state.received, None, "Escape carries no action index");
    }

    #[test]
    fn escape_without_a_prior_press_does_nothing() {
        let mut h = Harness::new();
        h.show();
        h.settle();
        // No prior press — the sheet never claimed focus, so Escape has no
        // focus chain to travel and is dropped.
        h.root.event(&mut h.state, &escape_event());
        h.drain();
        assert_eq!(h.state.calls, 0, "Escape without prior focus is a no-op");
    }

    #[test]
    fn second_press_does_not_reclaim_focus_after_one_open() {
        use frust_core::ChildPod;

        let view = sheet_view();
        let w = build(&view);
        let mut pod = ChildPod::new(Box::new(w));
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), None);
        pod.layout_child(&mut lctx, &BoxConstraints::tight(WINDOW));

        let mut dummy = ();
        {
            let s: &mut dyn Any = &mut dummy;
            let mut ctx = EventCtx::new(s, Point::ZERO, WINDOW);
            pod.event_child(&mut ctx, &Harness::ev(PointerPhase::Down, 5.0, 5.0));
        }
        assert!(pod.is_focused(), "the first Down claims focus");
        {
            let s: &mut dyn Any = &mut dummy;
            let mut ctx = EventCtx::new(s, Point::ZERO, WINDOW);
            pod.event_child(&mut ctx, &Harness::ev(PointerPhase::Up, 5.0, 5.0));
        }

        // Simulate an external clear so the second Down's own effect on the
        // recorded flag is isolated: if the guard holds, the widget itself
        // never re-calls `request_focus`, so the flag stays as we set it.
        pod.set_focused(false);
        {
            let s: &mut dyn Any = &mut dummy;
            let mut ctx = EventCtx::new(s, Point::ZERO, WINDOW);
            pod.event_child(&mut ctx, &Harness::ev(PointerPhase::Down, 5.0, 5.0));
        }
        assert!(
            !pod.is_focused(),
            "a second Down within the same open must not re-request focus"
        );
    }

    #[test]
    fn semantics_is_a_menu_container() {
        fn logic(_s: &mut ()) -> CupertinoActionSheetView<()> {
            sheet_view()
        }
        let mut root: RenderRoot<(), CupertinoActionSheetView<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);
        let update = root.semantics();
        assert!(
            update.nodes.iter().any(|(_, n)| n.role() == Role::Menu),
            "a Menu container node is contributed"
        );
    }
}
