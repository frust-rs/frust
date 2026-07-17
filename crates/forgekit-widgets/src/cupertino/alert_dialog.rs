//! `CupertinoAlertDialog` (Phase 6c, PLAN.md D3/D5, task 13): the iOS alert —
//! a centered ~270pt panel with a title, optional message, and vertically
//! stacked action buttons separated by hairlines, over a dimming scrim.
//!
//! It is pushed as a **transparent** navigator page (task 06) via the
//! [`show_cupertino_alert`] convenience, entering with the
//! [`PageTransition::M3FadeThrough`](crate::PageTransition::M3FadeThrough)
//! preset (a fade is idiom-appropriate for an alert). An action tap pops the
//! page carrying the action's index as a [`PopResult`]; a scrim tap pops with
//! an empty result (dismiss). The pusher-registered `on_result` callback
//! (see [`NavigatorController::push_transparent_for_result`]) receives it.
//!
//! # Colors & metrics
//!
//! The panel corner radius is ~14pt (**community-approximate** — Apple
//! publishes no alert corner-radius spec, per this task's C13 flag). The title
//! is themed `on_surface` (iOS `label`) and the message `on_surface_variant`
//! (secondaryLabel), so both live-swap with the theme. Action labels are tinted
//! by [`CupertinoActionStyle`] from *community-measured* systemBlue/systemRed
//! (baked explicit — see [`crate::cupertino::tabbar`]'s Label color note for why
//! these accent colors don't live-swap).
//!
//! # Keyboard operability (task 07, 6d)
//!
//! **Escape-to-dismiss now works, once the alert has focus.** A `Down`
//! anywhere in the alert (scrim, panel background, or an action) claims focus
//! via `EventCtx::request_focus` — the alert already captures its whole area,
//! so this is a pure opt-in with no new hit-testing. Once focused, a
//! focus-routed `Key(Escape)` invokes the same `controller.pop()` dismiss path
//! as a scrim tap. **There is still no hook to focus the alert on appear**
//! (auto-focus-on-appear) — a caller must complete one pointer interaction
//! with the alert before Escape does anything; that gap is deferred to a
//! future focus-manager work item, not this task.
//!
//! Its semantics node carries [`Role::AlertDialog`] but **no** accesskit modal
//! flag (unlike [`crate::material::dialog`]/[`crate::material::sheet`]), so
//! there is nothing to reconcile with a future adapter on that front.
//!
//! # State layer (task 07, 6d)
//!
//! This alert has no `StateLayer` surface of its own (the scrim, panel, and
//! action rows are plain fills/hairlines, not an M3 interactive surface) —
//! there is nothing here for `StateLayer::set_focused` to wire into.
//!
//! # Liquid Glass panel (task 6f-14)
//!
//! When a [`Theme`] is threaded and its `glass.chrome` recipe is not the
//! opaque-material path ([`GlassMaterial::is_opaque`] — true on the Material
//! baseline, false on [`Theme::cupertino_baseline`]), the panel paints the
//! chrome fill-wash stack + a specular hairline + the tier's drop shadow
//! straight from `theme.glass.chrome`, and each action row paints as its own
//! nested "button" capsule inset from the panel edge, its corner radius
//! derived from the panel's outer radius via
//! [`ShapeScale::concentric_inner`] (the concentric-corner principle, iOS
//! 26+ — see that fn's docs). No theme (bare-core tests) or an opaque-chrome
//! theme (the Material baseline) falls back to the pre-26 single flat fill +
//! hairline-separator-row layout this widget always painted. The panel
//! corner radius itself also grows under a theme (`shape.extra_large`,
//! iOS-26+'s larger panel radii) versus the flat, pre-26 [`PANEL_RADIUS`]
//! fallback. Real background blur is out of scope (spike 16) — the fill
//! washes composite over whatever the scrim already painted, not a blurred
//! backdrop.
//!
//! Kit sizing evidence (RESEARCH.md, action-sheet container/action-row
//! records, **partially-verified** — cited as guidance, not adopted
//! verbatim): mined action-sheet containers ran 260×424-524px, action rows
//! 232×48px. This widget keeps its own established [`PANEL_W`]/[`ACTION_H`]
//! geometry (task 07's community-approximate ~270pt/~44pt) rather than
//! matching those mined figures exactly — changing panel/row *sizing* is out
//! of this task's scope (only the *material* and *corner radii* are
//! re-skinned).

use forgekit_core::accesskit::Role;
use forgekit_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    Key, LayoutCtx, NamedKey, PaintCtx, PaintScene, PointerPhase, SemanticsCtx, View, Widget,
};
use forgekit_text::{FontWeight, LineHeight};
use forgekit_theme::{Brightness, GlassFill, GlassMaterial, ShapeScale, Theme};
use kurbo::{Point, Rect, RoundedRect, Shape, Size};
use peniko::{Brush, Color};

use crate::nav::navigator::{NavigatorController, PopResult};
use crate::text;
use crate::text::ThemeTextColor;

/// Panel width, in logical px.
///
/// **Community-approximate**: iOS does not publish an exact alert width; ~270pt
/// is the community-converged value (per this task's C13 flag).
const PANEL_W: f64 = 270.0;
/// Unthemed-fallback panel corner radius, in logical px (**community-approximate**,
/// ~13-14pt — no published Apple alert corner spec, per C13). A themed panel
/// uses the larger `shape.extra_large` iOS-26+ Liquid Glass radius instead
/// (see [`resolve_panel_radius`] and the module docs' Liquid Glass panel
/// section) — this constant is now only the no-theme (bare-core test) path.
const PANEL_RADIUS: f64 = 14.0;
/// Inset, in logical px, between a nested action-row "button" capsule and the
/// glass panel's own edge/neighboring rows (task 6f-14's iOS-26+ idiom:
/// distinct rounded glass buttons rather than hairline-divided flush rows).
///
/// **Community-approximate**: iOS 26 groups alert/action-sheet actions as
/// separated glass elements with a visible gap; Apple publishes no exact gap
/// value, so a small, defensible inset is picked here.
const BUTTON_INSET: f64 = 4.0;
/// Flattening tolerance for the panel's specular-hairline outline path (see
/// [`crate::material::card`]'s identical precedent — a visually-lossless
/// value for on-screen corner radii).
const PATH_TOLERANCE: f64 = 0.1;
/// Horizontal content padding inside the panel, in logical px.
const H_PAD: f64 = 16.0;
/// Top padding above the title, in logical px.
const V_PAD_TOP: f64 = 20.0;
/// Bottom padding below the message block (above the action divider), px.
const V_PAD_BOTTOM: f64 = 20.0;
/// Gap between the title and the message, in logical px.
const TITLE_MESSAGE_GAP: f64 = 4.0;
/// Height of each stacked action row, in logical px.
///
/// **Community-approximate**: ~44pt matches the standard iOS tappable-row
/// height; no published alert-action-row spec exists.
const ACTION_H: f64 = 44.0;

/// Title type-role: SF *Headline*, 17pt Semibold (see [`super::navbar`]).
const TITLE_SIZE: f32 = 17.0;
const TITLE_LINE_HEIGHT: f32 = 22.0;
/// Message type-role: SF *Footnote*, 13pt Regular.
const MESSAGE_SIZE: f32 = 13.0;
const MESSAGE_LINE_HEIGHT: f32 = 18.0;
/// Action label type-role: SF *Body*, 17pt.
const ACTION_SIZE: f32 = 17.0;
const ACTION_LINE_HEIGHT: f32 = 22.0;

/// systemBlue — the default/cancel action tint.
///
/// **Kit-measured** (2026-07-18 refresh; see [`super::tabbar`]'s
/// `SYSTEM_BLUE`, which cites the iOS 27 UI Kit `System Colors/Light/8 Blue`
/// swatch this mirrors exactly): refines the pre-refresh community value
/// `#007AFF`.
pub(crate) const SYSTEM_BLUE: Color = Color::from_rgb8(0x00, 0x87, 0xFF);
/// systemRed — the destructive action tint.
///
/// **Kit-measured** (2026-07-18 refresh): the iOS 27 UI Kit's `System
/// Colors/Light/1 Red` swatch (`kit-colors-type-metrics.json`, `colors`),
/// matching `ColorScheme::cupertino_light().error` exactly (see `color.rs`'s
/// module docs) — refines the pre-refresh community value `#FF3B30`.
pub(crate) const SYSTEM_RED: Color = Color::from_rgb8(0xFF, 0x38, 0x3C);

/// Unthemed fallback panel fill (a theme resolves this from
/// `colors.surface_container_high`).
const PANEL_FILL: Color = Color::from_rgb8(0xF2, 0xF2, 0xF7);
/// Unthemed fallback hairline color (a theme resolves this from
/// `colors.outline_variant`, iOS separator).
///
/// **Composite-derived** (re-derived for the 2026-07-18 kit refresh — see
/// [`super::navbar`]'s `SEPARATOR` for the full derivation, mirrored here):
/// `0.12` black over white ≈ `0xE0E0E0`, superseding the pre-refresh
/// `#C6C6C8` approximation of the old translucent `rgba(60,60,67,0.29)`
/// token.
const SEPARATOR: Color = Color::from_rgb8(0xE0, 0xE0, 0xE0);
/// The scrim alpha the modal dims the page below with.
///
/// **Community-approximate**: iOS composites a blurred dim behind an alert; a
/// flat ~0.2 black scrim is the closest backend-agnostic approximation.
const SCRIM_ALPHA: f32 = 0.2;
/// Hairline stroke width, logical px (see [`super::navbar`]).
const HAIRLINE_W: f64 = 1.0;

/// How a [`CupertinoAlertDialog`] action button reads and tints.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CupertinoActionStyle {
    /// A normal action — systemBlue, regular weight. The default.
    #[default]
    Default,
    /// The cancel/dismiss action — systemBlue, **bold** (iOS emphasizes the
    /// cancel action's weight).
    Cancel,
    /// A destructive action — systemRed.
    Destructive,
}

impl CupertinoActionStyle {
    /// The label color for this style.
    pub(crate) fn color(self) -> Color {
        match self {
            CupertinoActionStyle::Default | CupertinoActionStyle::Cancel => SYSTEM_BLUE,
            CupertinoActionStyle::Destructive => SYSTEM_RED,
        }
    }

    /// The label font weight for this style (cancel is bold; the rest regular).
    pub(crate) fn weight(self) -> FontWeight {
        match self {
            CupertinoActionStyle::Cancel => FontWeight::SEMI_BOLD,
            _ => FontWeight::REGULAR,
        }
    }
}

/// One alert action: its label and [`CupertinoActionStyle`].
#[derive(Clone, Debug)]
pub struct CupertinoDialogAction {
    pub label: String,
    pub style: CupertinoActionStyle,
}

/// A [`CupertinoActionStyle::Default`] action labelled `label`.
pub fn action(label: impl Into<String>) -> CupertinoDialogAction {
    CupertinoDialogAction {
        label: label.into(),
        style: CupertinoActionStyle::Default,
    }
}

impl CupertinoDialogAction {
    /// Set this action's [`CupertinoActionStyle`].
    pub fn style(mut self, style: CupertinoActionStyle) -> Self {
        self.style = style;
        self
    }
}

/// Build an action label's type-erased child view (explicit accent color +
/// weight, centered).
pub(crate) fn action_label_view<State: 'static>(a: &CupertinoDialogAction) -> AnyView<State> {
    forgekit_core::any::<State, _>(
        text(a.label.clone())
            .size(ACTION_SIZE)
            .weight(a.style.weight())
            .color(a.style.color())
            .line_height(LineHeight::Absolute(ACTION_LINE_HEIGHT)),
    )
}

/// Push a Cupertino alert onto `controller`'s stack. `on_result` receives the
/// tapped action's index (`usize`) as a [`PopResult`], or an empty result when
/// the scrim was tapped to dismiss.
pub fn show_cupertino_alert<State: 'static>(
    controller: &NavigatorController<State>,
    title: impl Into<String>,
    message: Option<String>,
    actions: Vec<CupertinoDialogAction>,
    on_result: impl Fn(&mut State, PopResult) + 'static,
) {
    let title = title.into();
    let controller_for_builder = controller.clone();
    controller.push_transparent_for_result(
        move || {
            forgekit_core::any::<State, _>(CupertinoAlertDialogView {
                title: title.clone(),
                message: message.clone(),
                actions: actions.clone(),
                controller: controller_for_builder.clone(),
            })
        },
        crate::TransitionSpec::duration(crate::PageTransition::M3FadeThrough),
        on_result,
    );
}

/// A declarative iOS alert dialog page. Usually created via
/// [`show_cupertino_alert`]; exposed so it can also be tested/embedded directly.
pub struct CupertinoAlertDialogView<State: 'static> {
    pub title: String,
    pub message: Option<String>,
    pub actions: Vec<CupertinoDialogAction>,
    pub controller: NavigatorController<State>,
}

/// The retained widget for a [`CupertinoAlertDialogView`].
pub struct CupertinoAlertDialogWidget<State: 'static> {
    title: ChildPod,
    /// The title string, retained for the semantics container's label.
    title_text: String,
    message: Option<ChildPod>,
    actions: Vec<ChildPod>,
    controller: NavigatorController<State>,
    /// Panel rect in the widget's local coordinate space (computed in layout).
    panel_rect: Rect,
    /// Each action row's rect in local coordinates (computed in layout).
    action_rects: Vec<Rect>,
    /// Armed by a `Down`, cleared on `Up`/`Cancel` (fire-on-up-inside).
    captured: bool,
}

/// Return `color` with its alpha channel replaced by `alpha` (mirrors
/// [`crate::material::card`]'s helper of the same shape).
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// The resolved panel corner radius. Themed: `shape.extra_large` (the
/// iOS-26+ Liquid Glass idiom's larger panel radius). Unthemed:
/// [`PANEL_RADIUS`] exactly (see the module docs' Liquid Glass panel
/// section).
pub(crate) fn resolve_panel_radius(theme: Option<&Theme>) -> f64 {
    match theme {
        Some(theme) => theme.shape.extra_large,
        None => PANEL_RADIUS,
    }
}

/// The chrome-tier glass material for the panel — a one-line accessor kept
/// so the paint code below reads `theme.glass.chrome` in exactly one place.
fn resolve_chrome(theme: &Theme) -> &GlassMaterial {
    &theme.glass.chrome
}

/// The chrome material's fill-wash stack for `theme`'s *active* brightness
/// (`fills_dark` under [`Brightness::Dark`], `fills_light` otherwise).
fn resolve_fills<'a>(theme: &Theme, material: &'a GlassMaterial) -> &'a [GlassFill] {
    if theme.brightness == Brightness::Dark {
        &material.fills_dark
    } else {
        &material.fills_light
    }
}

fn title_view<State: 'static>(title: String) -> AnyView<State> {
    forgekit_core::any::<State, _>(
        text(title)
            .size(TITLE_SIZE)
            .weight(FontWeight::SEMI_BOLD)
            .themed_role(ThemeTextColor::OnSurface)
            .line_height(LineHeight::Absolute(TITLE_LINE_HEIGHT)),
    )
}

fn message_view<State: 'static>(message: String) -> AnyView<State> {
    forgekit_core::any::<State, _>(
        text(message)
            .size(MESSAGE_SIZE)
            .themed_role(ThemeTextColor::OnSurfaceVariant)
            .line_height(LineHeight::Absolute(MESSAGE_LINE_HEIGHT)),
    )
}

impl<State: 'static> View<State> for CupertinoAlertDialogView<State> {
    type Element = CupertinoAlertDialogWidget<State>;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> CupertinoAlertDialogWidget<State> {
        let title = crate::build_child(&title_view::<State>(self.title.clone()), ctx);
        let message = self
            .message
            .as_ref()
            .map(|m| crate::build_child(&message_view::<State>(m.clone()), ctx));
        let actions = self
            .actions
            .iter()
            .map(|a| crate::build_child(&action_label_view::<State>(a), ctx))
            .collect();
        CupertinoAlertDialogWidget {
            title,
            title_text: self.title.clone(),
            message,
            actions,
            controller: self.controller.clone(),
            panel_rect: Rect::ZERO,
            action_rects: Vec::new(),
            captured: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut CupertinoAlertDialogWidget<State>,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        element.controller = self.controller.clone();
        if prev.title != self.title {
            element.title_text = self.title.clone();
            flags |= crate::rebuild_child(
                &title_view::<State>(prev.title.clone()),
                &title_view::<State>(self.title.clone()),
                &mut element.title,
                ctx,
            );
        }
        // Message/action-set structural changes are not expected for a live
        // alert (its content is fixed at push time); a differing count would be
        // a rebuild against a different alert, which the navigator handles by
        // teardown+build, not in-place rebuild. Reconcile the common prefix.
        if let (Some(pm), Some(nm), Some(elem_m)) =
            (&prev.message, &self.message, element.message.as_mut())
        {
            flags |= crate::rebuild_child(
                &message_view::<State>(pm.clone()),
                &message_view::<State>(nm.clone()),
                elem_m,
                ctx,
            );
        }
        let common = prev.actions.len().min(self.actions.len());
        for i in 0..common {
            flags |= crate::rebuild_child(
                &action_label_view::<State>(&prev.actions[i]),
                &action_label_view::<State>(&self.actions[i]),
                &mut element.actions[i],
                ctx,
            );
        }
        flags
    }

    fn teardown(&self, element: &mut CupertinoAlertDialogWidget<State>, ctx: &mut BuildCtx<'_>) {
        crate::teardown_child(
            &title_view::<State>(self.title.clone()),
            &mut element.title,
            ctx,
        );
        if let (Some(m), Some(elem_m)) = (&self.message, element.message.as_mut()) {
            crate::teardown_child(&message_view::<State>(m.clone()), elem_m, ctx);
        }
        for (a, pod) in self.actions.iter().zip(element.actions.iter_mut()) {
            crate::teardown_child(&action_label_view::<State>(a), pod, ctx);
        }
    }
}

impl<State: 'static> Widget for CupertinoAlertDialogWidget<State> {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let full = bc.max();
        let content_w = PANEL_W - 2.0 * H_PAD;
        let content_bc = BoxConstraints::loose(Size::new(content_w, f64::INFINITY));

        let title_size = self.title.layout_child(ctx, &content_bc);
        let message_size = self
            .message
            .as_mut()
            .map(|m| m.layout_child(ctx, &content_bc));

        // Content block height (title [+ gap + message]) plus vertical padding.
        let mut content_h = V_PAD_TOP + title_size.height;
        if let Some(ms) = message_size {
            content_h += TITLE_MESSAGE_GAP + ms.height;
        }
        content_h += V_PAD_BOTTOM;

        let action_bc = BoxConstraints::loose(Size::new(content_w, ACTION_H));
        for pod in &mut self.actions {
            pod.layout_child(ctx, &action_bc);
        }

        let panel_h = content_h + self.actions.len() as f64 * ACTION_H;
        let panel_x = ((full.width - PANEL_W) / 2.0).max(0.0);
        let panel_y = ((full.height - panel_h) / 2.0).max(0.0);
        self.panel_rect = Rect::new(panel_x, panel_y, panel_x + PANEL_W, panel_y + panel_h);

        // Center the title (and message) horizontally within the panel.
        let title_x = panel_x + (PANEL_W - title_size.width) / 2.0;
        self.title
            .set_origin(Point::new(title_x, panel_y + V_PAD_TOP));
        if let (Some(pod), Some(ms)) = (self.message.as_mut(), message_size) {
            let msg_x = panel_x + (PANEL_W - ms.width) / 2.0;
            let msg_y = panel_y + V_PAD_TOP + title_size.height + TITLE_MESSAGE_GAP;
            pod.set_origin(Point::new(msg_x, msg_y));
        }

        // Stack action rows below the content block; center each label in its row.
        self.action_rects.clear();
        let actions_top = panel_y + content_h;
        for (i, pod) in self.actions.iter_mut().enumerate() {
            let row_y = actions_top + i as f64 * ACTION_H;
            self.action_rects.push(Rect::new(
                panel_x,
                row_y,
                panel_x + PANEL_W,
                row_y + ACTION_H,
            ));
            let label_size = pod.size();
            let lx = panel_x + (PANEL_W - label_size.width) / 2.0;
            let ly = row_y + (ACTION_H - label_size.height) / 2.0;
            pod.set_origin(Point::new(lx, ly));
        }

        bc.constrain(full)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let scrim_base = theme.map(|t| t.scheme().scrim).unwrap_or(Color::BLACK);

        let origin = ctx.origin();
        let size = ctx.size();
        // Scrim over the whole page (mechanism unchanged — restyled values
        // only, per the module docs' Liquid Glass panel note).
        scene.fill_rect(origin, size, with_alpha(scrim_base, SCRIM_ALPHA));

        // Panel geometry (local rects offset by the paint origin).
        let panel_origin = Point::new(origin.x + self.panel_rect.x0, origin.y + self.panel_rect.y0);
        let panel_size = Size::new(self.panel_rect.width(), self.panel_rect.height());
        let panel_radius = resolve_panel_radius(theme);

        let chrome = theme.map(resolve_chrome);
        match chrome {
            Some(material) if !material.is_opaque() => {
                let theme = theme.expect("chrome resolved from a threaded theme");
                // Drop shadow, straight from the token.
                if material.shadow.color_alpha > 0.0 {
                    let shadow_color =
                        with_alpha(theme.scheme().shadow, material.shadow.color_alpha);
                    scene.draw_shadow(
                        Point::new(panel_origin.x, panel_origin.y + material.shadow.y_offset),
                        panel_size,
                        panel_radius,
                        material.shadow.blur_std_dev,
                        shadow_color,
                    );
                }
                // Fill-wash stack, bottom-to-top.
                let fills = resolve_fills(theme, material);
                for wash in fills {
                    scene.fill_rounded_rect(panel_origin, panel_size, panel_radius, wash.color);
                }
                // Specular hairline — white at the token's alpha (see the
                // `GlassMaterial::hairline_alpha` docs: the color is always
                // white, only the alpha is thematic).
                let hairline = Color::new([1.0, 1.0, 1.0, material.hairline_alpha]);
                if material.hairline_alpha > 0.0 {
                    let local = Rect::new(0.0, 0.0, panel_size.width, panel_size.height);
                    let path = RoundedRect::from_rect(local, panel_radius).to_path(PATH_TOLERANCE);
                    scene.stroke_path(panel_origin, &path, HAIRLINE_W, &Brush::Solid(hairline));
                }
                // Each action row paints as its own nested "button" capsule,
                // concentric with the panel's outer corners.
                let nested_radius = ShapeScale::concentric_inner(panel_radius, BUTTON_INSET);
                let nested_fill = fills.last().map(|w| w.color).unwrap_or(hairline);
                for rect in &self.action_rects {
                    let btn_origin = Point::new(
                        origin.x + rect.x0 + BUTTON_INSET,
                        origin.y + rect.y0 + BUTTON_INSET,
                    );
                    let btn_size = Size::new(
                        (rect.width() - 2.0 * BUTTON_INSET).max(0.0),
                        (rect.height() - 2.0 * BUTTON_INSET).max(0.0),
                    );
                    scene.fill_rounded_rect(btn_origin, btn_size, nested_radius, nested_fill);
                }
            }
            _ => {
                // No theme, or an opaque-chrome (Material-baseline) theme:
                // the pre-26 single flat fill + hairline-separated rows.
                let panel_fill = theme
                    .map(|t| t.scheme().surface_container_high)
                    .unwrap_or(PANEL_FILL);
                scene.fill_rounded_rect(panel_origin, panel_size, panel_radius, panel_fill);

                let separator = theme
                    .map(|t| t.scheme().outline_variant)
                    .unwrap_or(SEPARATOR);
                for rect in &self.action_rects {
                    let y = origin.y + rect.y0 + HAIRLINE_W / 2.0;
                    scene.stroke_line(
                        Point::new(origin.x + rect.x0, y),
                        Point::new(origin.x + rect.x1, y),
                        HAIRLINE_W,
                        separator,
                    );
                }
            }
        }

        self.title.paint_child(ctx, scene);
        if let Some(m) = self.message.as_mut() {
            m.paint_child(ctx, scene);
        }
        for pod in &mut self.actions {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // Escape (once focused) dismisses through the same `controller.pop()`
        // path as a scrim tap — see the module docs' Keyboard operability note.
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
                // A press anywhere in the alert claims focus, so a subsequent
                // Escape has a focus chain to travel.
                ctx.request_focus();
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
                    // An action was tapped: pop carrying its index.
                    self.controller.pop_with_result(PopResult::of(idx));
                    ctx.request_redraw();
                } else if !self.panel_rect.contains(pos) {
                    // A scrim tap dismisses (empty result).
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
        let title_label = self.title_text.clone();
        let title_pod = &self.title;
        let message = &self.message;
        let actions = &self.actions;
        ctx.push_container(
            Role::AlertDialog,
            move |node| node.set_label(title_label.as_str()),
            |ctx| {
                title_pod.semantics_child(ctx);
                if let Some(m) = message {
                    m.semantics_child(ctx);
                }
                for pod in actions {
                    pod.semantics_child(ctx);
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forgekit_core::{BuildCtx, KeyEvent, Modifiers, PointerButton, PointerEvent};
    use forgekit_text::TextContext;
    use std::any::Any;

    fn escape_event() -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::Escape),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    fn alert_view() -> CupertinoAlertDialogView<()> {
        CupertinoAlertDialogView {
            title: "Delete?".into(),
            message: Some("This cannot be undone.".into()),
            actions: vec![
                action("Cancel").style(CupertinoActionStyle::Cancel),
                action("Delete").style(CupertinoActionStyle::Destructive),
            ],
            controller: NavigatorController::new(),
        }
    }

    fn build(view: &CupertinoAlertDialogView<()>) -> CupertinoAlertDialogWidget<()> {
        let mut counter = 0u64;
        View::<()>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut CupertinoAlertDialogWidget<()>, window: Size) {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), None);
        w.layout(&mut lctx, &BoxConstraints::tight(window));
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    /// Records filled rounded rects, shadows, and stroked paths — enough to
    /// assert the Liquid Glass panel's fill stack, hairline, and nested
    /// concentric-radius button capsules reach paint (task 6f-14).
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
        w: &mut CupertinoAlertDialogWidget<()>,
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
    fn glass_chrome_theme_paints_the_fill_stack_hairline_and_shadow() {
        let theme = Theme::cupertino_baseline(); // light brightness by default
        let mut w = build(&alert_view());
        let window = Size::new(400.0, 800.0);
        layout(&mut w, window);
        let rec = paint_with_theme(&mut w, window, Some(&theme));

        let radius = resolve_panel_radius(Some(&theme));
        for wash in &theme.glass.chrome.fills_light {
            assert!(
                rec.rrects
                    .iter()
                    .any(|(_, _, r, c)| *r == radius && *c == wash.color),
                "each chrome fill wash reaches paint as its own filled rounded rect: {:?}",
                wash
            );
        }
        assert!(
            !rec.strokes.is_empty(),
            "the chrome recipe's specular hairline is stroked"
        );
        assert!(
            !rec.shadows.is_empty(),
            "the chrome recipe's drop shadow is painted"
        );
    }

    #[test]
    fn nested_action_button_radius_is_concentric_with_the_panel() {
        let theme = Theme::cupertino_baseline();
        let mut w = build(&alert_view());
        let window = Size::new(400.0, 800.0);
        layout(&mut w, window);
        let rec = paint_with_theme(&mut w, window, Some(&theme));

        let panel_radius = resolve_panel_radius(Some(&theme));
        let expected = ShapeScale::concentric_inner(panel_radius, BUTTON_INSET);
        assert!(
            expected < panel_radius,
            "sanity: a nested inset radius is strictly smaller than the panel's"
        );
        assert!(
            rec.rrects
                .iter()
                .any(|(_, _, r, _)| (*r - expected).abs() < 1e-9),
            "a nested action-row capsule paints at the concentric-inner radius"
        );
    }

    #[test]
    fn opaque_material_theme_keeps_the_pre_26_flat_panel() {
        // Material baseline's chrome tier is opaque — no fill washes, no
        // hairline, no nested button capsules; the legacy flat-fill +
        // hairline-row layout paints instead (bullet 3: Material appearance
        // unchanged).
        let theme = Theme::m3_baseline();
        let mut w = build(&alert_view());
        let window = Size::new(400.0, 800.0);
        layout(&mut w, window);
        let rec = paint_with_theme(&mut w, window, Some(&theme));

        assert!(theme.glass.chrome.is_opaque());
        assert_eq!(
            rec.rrects.len(),
            1,
            "one flat panel fill, no chrome washes or nested button capsules"
        );
        assert_eq!(rec.rrects[0].3, theme.scheme().surface_container_high);
        assert!(
            rec.shadows.is_empty(),
            "the opaque path paints no glass shadow"
        );
    }

    #[test]
    fn unthemed_paint_keeps_the_pre_26_flat_panel() {
        let mut w = build(&alert_view());
        let window = Size::new(400.0, 800.0);
        layout(&mut w, window);
        let rec = paint_with_theme(&mut w, window, None);

        assert_eq!(
            rec.rrects.len(),
            1,
            "one flat panel fill, unthemed fallback"
        );
        assert_eq!(rec.rrects[0].2, PANEL_RADIUS);
        assert_eq!(rec.rrects[0].3, PANEL_FILL);
    }

    #[test]
    fn panel_is_270_wide_and_centered() {
        let mut w = build(&alert_view());
        let window = Size::new(400.0, 800.0);
        layout(&mut w, window);
        assert_eq!(w.panel_rect.width(), PANEL_W);
        let center_x = (w.panel_rect.x0 + w.panel_rect.x1) / 2.0;
        assert!(
            (center_x - 200.0).abs() < 1e-6,
            "panel centered horizontally"
        );
    }

    #[test]
    fn one_action_row_per_action() {
        let mut w = build(&alert_view());
        layout(&mut w, Size::new(400.0, 800.0));
        assert_eq!(w.action_rects.len(), 2);
        // Rows are stacked (row 1 below row 0).
        assert!(w.action_rects[1].y0 > w.action_rects[0].y0);
    }

    // --- Full modal wiring: drive a real navigator, settle the entering
    //     transition, then tap through to the pop + on_result callback. ---

    use crate::nav::navigator::{NavigatorView, navigator};
    use forgekit_core::{FrameTime, RenderRoot, any};

    #[derive(Default)]
    struct DialogState {
        /// Total `on_result` invocations (fires on every pop, action or scrim).
        calls: u32,
        /// The action index a pop carried, if any (empty for a scrim dismiss).
        received: Option<usize>,
    }

    fn ft_secs(s: f64) -> FrameTime {
        FrameTime::from_nanos((s * 1_000_000_000.0) as u64)
    }

    const WINDOW: Size = Size::new(400.0, 800.0);

    /// A harness owning a `RenderRoot<DialogState, NavigatorView>` plus its
    /// controller, and a probe copy of the alert widget for geometry lookup.
    struct ModalHarness {
        root: RenderRoot<DialogState, NavigatorView<DialogState>>,
        controller: NavigatorController<DialogState>,
        state: DialogState,
        tcx: TextContext,
        probe: CupertinoAlertDialogWidget<()>,
    }

    impl ModalHarness {
        fn new() -> Self {
            let controller: NavigatorController<DialogState> = NavigatorController::new();
            let root: RenderRoot<DialogState, NavigatorView<DialogState>> = RenderRoot::new();
            // A probe widget with identical content discovers the action-row
            // geometry the (transparent, full-window) alert page lays out to.
            let mut probe = {
                let mut counter = 0u64;
                View::<()>::build(&alert_view(), &mut BuildCtx::new(&mut counter))
            };
            let mut tcx = TextContext::new();
            let mut lctx = LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), None);
            probe.layout(&mut lctx, &BoxConstraints::tight(WINDOW));
            ModalHarness {
                root,
                controller,
                state: DialogState::default(),
                tcx: TextContext::new(),
                probe,
            }
        }

        fn app(&self) -> impl FnMut(&mut DialogState) -> NavigatorView<DialogState> + use<> {
            let ctrl = self.controller.clone();
            move |_: &mut DialogState| navigator(&ctrl, || any(text("base").size(17.0)))
        }

        fn rebuild_layout(&mut self) {
            let mut app = self.app();
            self.root.rebuild(&mut app, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
        }

        fn show(&mut self) {
            self.rebuild_layout();
            show_cupertino_alert(
                &self.controller,
                "Delete?",
                Some("This cannot be undone.".into()),
                vec![
                    action("Cancel").style(CupertinoActionStyle::Cancel),
                    action("Delete").style(CupertinoActionStyle::Destructive),
                ],
                |s: &mut DialogState, r: PopResult| {
                    s.calls += 1;
                    if let Some(i) = r.take::<usize>() {
                        s.received = Some(i);
                    }
                },
            );
            self.rebuild_layout();
        }

        /// Paint past the entering M3FadeThrough transition, then finalize it so
        /// the navigator stops blocking input to the alert page.
        fn settle(&mut self) {
            let mut scene = crate::test_support::RecordingScene::default();
            self.root.paint(&mut scene, ft_secs(0.0));
            self.root.paint(&mut scene, ft_secs(1.0));
            self.rebuild_layout();
        }

        fn drain(&mut self) {
            self.rebuild_layout();
            // Any event pass flushes a queued on_result callback with state.
            self.root
                .event(&mut self.state, &ev(PointerPhase::Move, 1.0, 1.0));
        }

        fn tap(&mut self, x: f64, y: f64) {
            self.root
                .event(&mut self.state, &ev(PointerPhase::Down, x, y));
            self.root
                .event(&mut self.state, &ev(PointerPhase::Up, x, y));
        }

        fn action_center(&self, idx: usize) -> (f64, f64) {
            let r = self.probe.action_rects[idx];
            ((r.x0 + r.x1) / 2.0, (r.y0 + r.y1) / 2.0)
        }

        fn panel_top_center(&self) -> (f64, f64) {
            let r = self.probe.panel_rect;
            ((r.x0 + r.x1) / 2.0, r.y0 + 2.0)
        }
    }

    #[test]
    fn action_tap_pops_with_its_index() {
        let mut nav = ModalHarness::new();
        nav.show();
        nav.settle();
        let (cx, cy) = nav.action_center(1);
        nav.tap(cx, cy);
        nav.drain();
        assert_eq!(nav.state.calls, 1, "the pop fired on_result once");
        assert_eq!(
            nav.state.received,
            Some(1),
            "carrying the tapped action index"
        );
    }

    #[test]
    fn scrim_tap_pops_to_dismiss() {
        let mut nav = ModalHarness::new();
        nav.show();
        nav.settle();
        // Tap the top-left corner — well outside the centered panel.
        nav.tap(5.0, 5.0);
        nav.drain();
        assert_eq!(
            nav.state.calls, 1,
            "a scrim tap pops the alert (on_result fires)"
        );
        assert_eq!(
            nav.state.received, None,
            "a dismiss carries no action index"
        );
    }

    #[test]
    fn tap_inside_panel_but_not_an_action_is_a_noop() {
        let mut nav = ModalHarness::new();
        nav.show();
        nav.settle();
        // A tap inside the panel but above the action rows neither pops nor
        // dismisses.
        let (px, py) = nav.panel_top_center();
        nav.tap(px, py);
        nav.drain();
        assert_eq!(nav.state.calls, 0, "a non-action panel tap is a no-op");
        // The alert is still up and interactive: an action tap now fires.
        let (cx, cy) = nav.action_center(0);
        nav.tap(cx, cy);
        nav.drain();
        assert_eq!(nav.state.calls, 1, "the alert survived the no-op tap");
        assert_eq!(nav.state.received, Some(0));
    }

    // --- Focus + Escape opt-in (task 07, 6d). ---

    #[test]
    fn escape_after_a_panel_tap_claims_focus_and_dismisses() {
        let mut nav = ModalHarness::new();
        nav.show();
        nav.settle();
        // A tap inside the panel but not on an action claims focus without
        // dismissing (mirrors `tap_inside_panel_but_not_an_action_is_a_noop`).
        let (px, py) = nav.panel_top_center();
        nav.tap(px, py);
        nav.drain();
        assert_eq!(nav.state.calls, 0, "the panel tap did not dismiss");

        // Escape now reaches the focused alert and dismisses it, the same
        // dismiss path as a scrim tap.
        nav.root.event(&mut nav.state, &escape_event());
        nav.drain();
        assert_eq!(nav.state.calls, 1, "Escape dismisses the focused alert");
        assert_eq!(nav.state.received, None, "Escape carries no action index");
    }

    #[test]
    fn escape_without_a_prior_tap_does_nothing() {
        let mut nav = ModalHarness::new();
        nav.show();
        nav.settle();
        // No prior tap — the alert never claimed focus, so Escape has no
        // focus chain to travel and is dropped.
        nav.root.event(&mut nav.state, &escape_event());
        nav.drain();
        assert_eq!(nav.state.calls, 0, "Escape without prior focus is a no-op");
    }

    #[test]
    fn destructive_action_tints_red_and_cancel_is_bold() {
        assert_eq!(CupertinoActionStyle::Destructive.color(), SYSTEM_RED);
        assert_eq!(CupertinoActionStyle::Default.color(), SYSTEM_BLUE);
        assert_eq!(CupertinoActionStyle::Cancel.weight(), FontWeight::SEMI_BOLD);
        assert_eq!(CupertinoActionStyle::Default.weight(), FontWeight::REGULAR);
    }

    #[test]
    fn semantics_is_an_alert_dialog_container() {
        fn logic(_s: &mut ()) -> CupertinoAlertDialogView<()> {
            CupertinoAlertDialogView {
                title: "Hi".into(),
                message: None,
                actions: vec![action("OK")],
                controller: NavigatorController::new(),
            }
        }
        let mut root: forgekit_core::RenderRoot<(), CupertinoAlertDialogView<()>> =
            forgekit_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(400.0, 800.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        assert!(
            update
                .nodes
                .iter()
                .any(|(_, n)| n.role() == Role::AlertDialog),
            "an AlertDialog container node is contributed"
        );
    }
}
